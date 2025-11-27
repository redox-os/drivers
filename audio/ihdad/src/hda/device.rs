#![allow(dead_code)]

use std::cmp;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::fmt::Write;
use std::str;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::Poll;
use std::thread;
use std::time::Duration;

use common::dma::Dma;
use common::io::{Io, Mmio};
use common::timeout::Timeout;
use redox_scheme::scheme::SchemeSync;
use redox_scheme::CallerCtx;
use redox_scheme::OpenResult;
use syscall::error::{Error, Result, EACCES, EBADF, EINVAL, EIO, ENODEV, EWOULDBLOCK};

use spin::Mutex;
use syscall::schemev2::NewFdFlags;

use super::common::*;
use super::BitsPerSample;
use super::BufferDescriptorListEntry;
use super::CommandBuffer;
use super::HDANode;
use super::OutputStream;
use super::StreamBuffer;
use super::StreamDescriptorRegs;

// GCTL - Global Control
const CRST: u32 = 1 << 0; // 1 bit
const FNCTRL: u32 = 1 << 1; // 1 bit
const UNSOL: u32 = 1 << 8; // 1 bit

// CORBCTL
const CMEIE: u8 = 1 << 0; // 1 bit
const CORBRUN: u8 = 1 << 1; // 1 bit

// CORBSIZE
const CORBSZCAP: (u8, u8) = (4, 4);
const CORBSIZE: (u8, u8) = (0, 2);

// CORBRP
const CORBRPRST: u16 = 1 << 15;

// RIRBWP
const RIRBWPRST: u16 = 1 << 15;

// RIRBCTL
const RINTCTL: u8 = 1 << 0; // 1 bit
const RIRBDMAEN: u8 = 1 << 1; // 1 bit

// ICS
const ICB: u16 = 1 << 0;
const IRV: u16 = 1 << 1;

// CORB and RIRB offset

const COMMAND_BUFFER_OFFSET: usize = 0x40;

const NUM_SUB_BUFFS: usize = 32;
const SUB_BUFF_SIZE: usize = 2048;

enum Handle {
    Todo,
    Pcmout(usize, usize, usize), // Card, index, block_ptr
    Pcmin(usize, usize, usize),  // Card, index, block_ptr
    StrBuf(Vec<u8>),
}

#[repr(C, packed)]
#[allow(dead_code)]
struct Regs {
    gcap: Mmio<u16>,
    vmin: Mmio<u8>,
    vmaj: Mmio<u8>,
    outpay: Mmio<u16>,
    inpay: Mmio<u16>,
    gctl: Mmio<u32>,
    wakeen: Mmio<u16>,
    statests: Mmio<u16>,
    gsts: Mmio<u16>,
    rsvd0: [Mmio<u8>; 6],
    outstrmpay: Mmio<u16>,
    instrmpay: Mmio<u16>,
    rsvd1: [Mmio<u8>; 4],
    intctl: Mmio<u32>,
    intsts: Mmio<u32>,
    rsvd2: [Mmio<u8>; 8],
    walclk: Mmio<u32>,
    rsvd3: Mmio<u32>,
    ssync: Mmio<u32>,
    rsvd4: Mmio<u32>,

    corblbase: Mmio<u32>,
    corbubase: Mmio<u32>,
    corbwp: Mmio<u16>,
    corbrp: Mmio<u16>,
    corbctl: Mmio<u8>,
    corbsts: Mmio<u8>,
    corbsize: Mmio<u8>,
    rsvd5: Mmio<u8>,

    rirblbase: Mmio<u32>,
    rirbubase: Mmio<u32>,
    rirbwp: Mmio<u16>,
    rintcnt: Mmio<u16>,
    rirbctl: Mmio<u8>,
    rirbsts: Mmio<u8>,
    rirbsize: Mmio<u8>,
    rsvd6: Mmio<u8>,

    icoi: Mmio<u32>,
    irii: Mmio<u32>,
    ics: Mmio<u16>,
    rsvd7: [Mmio<u8>; 6],

    dplbase: Mmio<u32>, // 0x70
    dpubase: Mmio<u32>, // 0x74
}

pub struct IntelHDA {
    vend_prod: u32,

    base: usize,
    regs: &'static mut Regs,

    //corb_rirb_base_phys: usize,
    cmd: CommandBuffer,

    codecs: Vec<CodecAddr>,

    outputs: Vec<WidgetAddr>,
    inputs: Vec<WidgetAddr>,

    widget_map: HashMap<WidgetAddr, HDANode>,

    output_pins: Vec<WidgetAddr>,
    input_pins: Vec<WidgetAddr>,

    beep_addr: WidgetAddr,

    buff_desc: Dma<[BufferDescriptorListEntry; 256]>,

    output_streams: Vec<OutputStream>,

    buffs: Vec<Vec<StreamBuffer>>,

    int_counter: usize,
    handles: Mutex<BTreeMap<usize, Handle>>,
    next_id: AtomicUsize,
}

impl IntelHDA {
    pub unsafe fn new(base: usize, vend_prod: u32) -> Result<Self> {
        let regs = &mut *(base as *mut Regs);

        let buff_desc = Dma::<[BufferDescriptorListEntry; 256]>::zeroed()
            .expect("Could not allocate physical memory for buffer descriptor list.")
            .assume_init();

        log::debug!(
            "Virt: {:016X}, Phys: {:016X}",
            buff_desc.as_ptr() as usize,
            buff_desc.physical()
        );

        let cmd_buff = Dma::<[u8; 0x1000]>::zeroed()
            .expect("Could not allocate physical memory for CORB and RIRB.")
            .assume_init();

        log::debug!(
            "Virt: {:016X}, Phys: {:016X}",
            cmd_buff.as_ptr() as usize,
            cmd_buff.physical()
        );
        let mut module = IntelHDA {
            vend_prod,
            base,
            regs,

            cmd: CommandBuffer::new(base + COMMAND_BUFFER_OFFSET, cmd_buff),

            beep_addr: (0, 0),

            widget_map: HashMap::<WidgetAddr, HDANode>::new(),

            codecs: Vec::<CodecAddr>::new(),

            outputs: Vec::<WidgetAddr>::new(),
            inputs: Vec::<WidgetAddr>::new(),

            output_pins: Vec::<WidgetAddr>::new(),
            input_pins: Vec::<WidgetAddr>::new(),

            buff_desc,

            output_streams: Vec::<OutputStream>::new(),

            buffs: Vec::<Vec<StreamBuffer>>::new(),

            int_counter: 0,
            handles: Mutex::new(BTreeMap::new()),
            next_id: AtomicUsize::new(0),
        };

        module.init()?;

        module.info();
        module.enumerate()?;

        module.configure()?;
        log::debug!("IHDA: Initialization finished.");
        Ok(module)
    }

    pub fn init(&mut self) -> Result<()> {
        self.reset_controller()?;

        let use_immediate_command_interface = match self.vend_prod {
            0x8086_2668 => false,
            _ => true,
        };

        self.cmd.init(use_immediate_command_interface)?;
        self.init_interrupts();

        Ok(())
    }

    pub fn init_interrupts(&mut self) {
        // TODO: provide a function to enable certain interrupts
        // This just enables the first output stream interupt and the global interrupt

        let iss = self.num_input_streams();
        self.regs
            .intctl
            .write((1 << 31) | /* (1 << 30) |*/ (1 << iss));
    }

    pub fn irq(&mut self) -> bool {
        self.int_counter += 1;

        self.handle_interrupts()
    }

    pub fn int_count(&self) -> usize {
        self.int_counter
    }

    pub fn read_node(&mut self, addr: WidgetAddr) -> Result<HDANode> {
        let mut node = HDANode::new();
        let mut temp: u64;

        node.addr = addr;

        temp = self.cmd.cmd12(addr, 0xF00, 0x04)?;

        node.subnode_count = (temp & 0xff) as u16;
        node.subnode_start = ((temp >> 16) & 0xff) as u16;

        if addr == (0, 0) {
            return Ok(node);
        }
        temp = self.cmd.cmd12(addr, 0xF00, 0x04)?;

        node.function_group_type = (temp & 0xff) as u8;

        temp = self.cmd.cmd12(addr, 0xF00, 0x09)?;
        node.capabilities = temp as u32;

        temp = self.cmd.cmd12(addr, 0xF00, 0x0E)?;

        node.conn_list_len = (temp & 0xFF) as u8;

        node.connections = self.node_get_connection_list(&node)?;

        node.connection_default = self.cmd.cmd12(addr, 0xF01, 0x00)? as u8;

        node.config_default = self.cmd.cmd12(addr, 0xF1C, 0x00)? as u32;

        Ok(node)
    }

    pub fn node_get_connection_list(&mut self, node: &HDANode) -> Result<Vec<WidgetAddr>> {
        let len_field: u8 = (self.cmd.cmd12(node.addr, 0xF00, 0x0E)? & 0xFF) as u8;

        // Highest bit is if addresses are represented in longer notation
        // lower 7 is actual count

        let count: u8 = len_field & 0x7F;
        let use_long_addr: bool = (len_field >> 7) & 0x1 == 1;

        let mut current: u8 = 0;

        let mut list = Vec::<WidgetAddr>::new();

        while current < count {
            let response: u32 = (self.cmd.cmd12(node.addr, 0xF02, current)? & 0xFFFFFFFF) as u32;

            if use_long_addr {
                for i in 0..2 {
                    let addr_field = ((response >> (16 * i)) & 0xFFFF) as u16;
                    let addr = addr_field & 0x7FFF;

                    if addr == 0 {
                        break;
                    }

                    if (addr_field >> 15) & 0x1 == 0x1 {
                        for i in list.pop().unwrap().1..(addr + 1) {
                            list.push((node.addr.0, i));
                        }
                    } else {
                        list.push((node.addr.0, addr));
                    }
                }
            } else {
                for i in 0..4 {
                    let addr_field = ((response >> (8 * i)) & 0xff) as u16;
                    let addr = addr_field & 0x7F;

                    if addr == 0 {
                        break;
                    }

                    if (addr_field >> 7) & 0x1 == 0x1 {
                        for i in list.pop().unwrap().1..(addr + 1) {
                            list.push((node.addr.0, i));
                        }
                    } else {
                        list.push((node.addr.0, addr));
                    }
                }
            }

            current = list.len() as u8;
        }

        Ok(list)
    }

    pub fn enumerate(&mut self) -> Result<()> {
        self.output_pins.clear();
        self.input_pins.clear();

        let codec: u8 = 0;

        let root = self.read_node((codec, 0))?;

        log::debug!("{}", root);

        let root_count = root.subnode_count;
        let root_start = root.subnode_start;

        //FIXME: So basically the way this is set up is to only support one codec and hopes the first one is an audio
        for i in 0..root_count {
            let afg = self.read_node((codec, root_start + i))?;
            log::debug!("{}", afg);
            let afg_count = afg.subnode_count;
            let afg_start = afg.subnode_start;

            for j in 0..afg_count {
                let mut widget = self.read_node((codec, afg_start + j))?;
                widget.is_widget = true;
                match widget.widget_type() {
                    HDAWidgetType::AudioOutput => self.outputs.push(widget.addr),
                    HDAWidgetType::AudioInput => self.inputs.push(widget.addr),
                    HDAWidgetType::BeepGenerator => self.beep_addr = widget.addr,
                    HDAWidgetType::PinComplex => {
                        let config = widget.configuration_default();
                        if config.is_output() {
                            self.output_pins.push(widget.addr);
                        } else if config.is_input() {
                            self.input_pins.push(widget.addr);
                        }
                    }
                    _ => {}
                }

                log::debug!("{}", widget);
                self.widget_map.insert(widget.addr(), widget);
            }
        }

        Ok(())
    }

    pub fn find_best_output_pin(&mut self) -> Result<WidgetAddr> {
        let outs = &self.output_pins;
        if outs.len() == 1 {
            return Ok(outs[0]);
        } else if outs.len() > 1 {
            //TODO: change output based on "unsolicited response" interrupts
            // Check for devices in this order: Headphone, Speaker, Line Out
            for supported_device in &[DefaultDevice::HPOut, DefaultDevice::Speaker] {
                for &out in outs {
                    let widget = self.widget_map.get(&out).unwrap();
                    let cd = widget.configuration_default();
                    if cd.sequence() == 0 && &cd.default_device() == supported_device {
                        // Check for jack detect bit
                        let pin_caps = self.cmd.cmd12(widget.addr, 0xF00, 0x0C)?;
                        if pin_caps & (1 << 2) != 0 {
                            // Check for presence
                            let pin_sense = self.cmd.cmd12(widget.addr, 0xF09, 0)?;
                            if pin_sense & (1 << 31) == 0 {
                                // Skip if nothing is plugged in
                                continue;
                            }
                        }
                        return Ok(out);
                    }
                }
            }
        }
        Err(Error::new(ENODEV))
    }

    pub fn find_path_to_dac(&self, addr: WidgetAddr) -> Option<Vec<WidgetAddr>> {
        let widget = self.widget_map.get(&addr).unwrap();
        if widget.widget_type() == HDAWidgetType::AudioOutput {
            Some(vec![addr])
        } else {
            let connection = widget.connections.get(widget.connection_default as usize)?;
            let mut path = self.find_path_to_dac(*connection)?;
            path.insert(0, addr);
            Some(path)
        }
    }

    /*
      Here we update the buffers and split them into 128 byte sub chunks
      because each BufferDescriptorList needs to be 128 byte aligned,
      this makes it so each of the streams can have up to 128/16 (8) buffer descriptors
    */
    /*
      Vec of a Vec was doing something weird and causing the driver to hang.
      So now we have a set of variables instead.


      Fixed?
    */

    pub fn update_sound_buffers(&mut self) {
        /*
        for i in 0..self.buffs.len(){
            for j in 0.. min(self.buffs[i].len(), 128/16 ) {
                self.buff_desc[i * 128/16 + j].set_address(self.buffs[i][j].phys());
                self.buff_desc[i * 128/16 + j].set_length(self.buffs[i][j].length() as u32);
                self.buff_desc[i * 128/16 + j].set_interrupt_on_complete(true);
            }
        }*/

        let r = self.get_output_stream_descriptor(0).unwrap();

        self.output_streams
            .push(OutputStream::new(NUM_SUB_BUFFS, SUB_BUFF_SIZE, r));

        let o = self.output_streams.get_mut(0).unwrap();

        for i in 0..NUM_SUB_BUFFS {
            self.buff_desc[i].set_address((o.phys() + o.block_size() * i) as u64);
            self.buff_desc[i].set_length(o.block_size() as u32);
            self.buff_desc[i].set_interrupt_on_complete(true);
        }
    }

    pub fn configure(&mut self) -> Result<()> {
        let outpin = self.find_best_output_pin()?;

        log::debug!("Best pin: {:01X}:{:02X}", outpin.0, outpin.1);

        let path = self.find_path_to_dac(outpin).unwrap();

        let dac = *path.last().unwrap();
        let pin = *path.first().unwrap();

        log::debug!("Path to DAC: {:X?}", path);

        // Set power state 0 (on) for all widgets in path
        for &addr in &path {
            self.set_power_state(addr, 0)?;
        }

        // Pin enable (0x80 = headphone amp enable, 0x40 = output enable)
        self.cmd.cmd12(pin, 0x707, 0xC0)?;

        // EAPD enable
        self.cmd.cmd12(pin, 0x70C, 2)?;

        // Set DAC stream and channel
        self.set_stream_channel(dac, 1, 0)?;

        self.update_sound_buffers();

        log::debug!(
            "Supported Formats: {:08X}",
            self.get_supported_formats((0, 0x1))?
        );
        log::debug!("Capabilities: {:08X}", self.get_capabilities(path[0])?);

        // Create output stream
        let output = self.get_output_stream_descriptor(0).unwrap();
        output.set_address(self.buff_desc.physical());
        output.set_pcm_format(&super::SR_44_1, BitsPerSample::Bits16, 2);
        output.set_cyclic_buffer_length((NUM_SUB_BUFFS * SUB_BUFF_SIZE) as u32); // number of bytes
        output.set_stream_number(1);
        output.set_last_valid_index((NUM_SUB_BUFFS - 1) as u16);
        output.set_interrupt_on_completion(true);

        // Set DAC converter format
        self.set_converter_format(dac, &super::SR_44_1, BitsPerSample::Bits16, 2)?;

        // Get DAC converter format
        //TODO: should validate?
        self.cmd.cmd12(dac, 0xA00, 0)?;

        // Unmute and set gain to 0db for input and output amplifiers on all widgets in path
        for &addr in &path {
            // Read widget capabilities
            let caps = self.cmd.cmd12(addr, 0xF00, 0x09)?;

            //TODO: do we need to set any other indexes?
            let left = true;
            let right = true;
            let index = 0;
            let mute = false;

            // Check for input amp
            if (caps & (1 << 1)) != 0 {
                // Read input capabilities
                let in_caps = self.cmd.cmd12(addr, 0xF00, 0x0D)?;
                let in_gain = (in_caps & 0x7f) as u8;
                // Set input gain
                let output = false;
                let input = true;
                self.set_amplifier_gain_mute(
                    addr, output, input, left, right, index, mute, in_gain,
                )?;
                log::debug!("Set {:X?} input gain to 0x{:X}", addr, in_gain);
            }

            // Check for output amp
            if (caps & (1 << 2)) != 0 {
                // Read output capabilities
                let out_caps = self.cmd.cmd12(addr, 0xF00, 0x12)?;
                let out_gain = (out_caps & 0x7f) as u8;
                // Set output gain
                let output = true;
                let input = false;
                self.set_amplifier_gain_mute(
                    addr, output, input, left, right, index, mute, out_gain,
                )?;
                log::debug!("Set {:X?} output gain to 0x{:X}", addr, out_gain);
            }
        }

        //TODO: implement hda-verb?

        output.run();
        {
            log::debug!("Waiting for output 0 to start running...");
            let timeout = Timeout::from_secs(1);
            while output.control() & (1 << 1) == 0 {
                timeout.run().map_err(|()| {
                    log::error!("timeout on output running");
                    Error::new(EIO)
                })?;
            }
        }

        log::debug!(
            "Output 0 CONTROL {:#X} STATUS {:#X} POS {:#X}",
            output.control(),
            output.status(),
            output.link_position()
        );
        Ok(())
    }
    /*

    pub fn configure_vbox(&mut self) {

        let outpin = self.find_best_output_pin().expect("IHDA: No output pins?!");

        log::debug!("Best pin: {:01X}:{:02X}", outpin.0, outpin.1);

        let path = self.find_path_to_dac(outpin).unwrap();
        log::debug!("Path to DAC: {:X?}", path);

        // Pin enable
        self.cmd.cmd12((0,0xC), 0x707, 0x40);


        // EAPD enable
        self.cmd.cmd12((0,0xC), 0x70C, 2);

        self.set_stream_channel((0,0x3), 1, 0);

        self.update_sound_buffers();


        log::debug!("Supported Formats: {:08X}", self.get_supported_formats((0,0x1)));
        log::debug!("Capabilities: {:08X}", self.get_capabilities((0,0x1)));

        let output = self.get_output_stream_descriptor(0).unwrap();

        output.set_address(self.buff_desc_phys);

        output.set_pcm_format(&super::SR_44_1, BitsPerSample::Bits16, 2);
        output.set_cyclic_buffer_length((NUM_SUB_BUFFS * SUB_BUFF_SIZE) as u32);
        output.set_stream_number(1);
        output.set_last_valid_index((NUM_SUB_BUFFS - 1) as u16);
        output.set_interrupt_on_completion(true);


        self.set_power_state((0,0x3), 0); // Power state 0 is fully on
        self.set_converter_format((0,0x3), &super::SR_44_1, BitsPerSample::Bits16, 2);


        self.cmd.cmd12((0,0x3), 0xA00, 0);

        // Unmute and set gain for pin complex and DAC
        self.set_amplifier_gain_mute((0,0x3), true, true, true, true, 0, false, 0x7f);
        self.set_amplifier_gain_mute((0,0xC), true, true, true, true, 0, false, 0x7f);

        output.run();

        self.beep(1);

    }

    */

    pub fn dump_codec(&self, codec: u8) -> String {
        let mut string = String::new();

        for (_, widget) in self.widget_map.iter() {
            let _ = writeln!(string, "{}", widget);
        }

        string
    }

    // BEEP!!
    pub fn beep(&mut self, div: u8) {
        let addr = self.beep_addr;
        if addr != (0, 0) {
            let _ = self.cmd.cmd12(addr, 0xF0A, div);
        }
    }

    pub fn reset_controller(&mut self) -> Result<()> {
        self.cmd.stop()?;

        self.regs.statests.write(0x7FFF);

        // 3.3.7
        {
            let timeout = Timeout::from_secs(1);
            self.regs.gctl.writef(CRST, false);
            loop {
                if !self.regs.gctl.readf(CRST) {
                    break;
                }
                timeout.run().map_err(|()| {
                    log::error!("failed to start reset");
                    Error::new(EIO)
                })?;
            }
        }

        thread::sleep(Duration::from_millis(1));

        {
            let timeout = Timeout::from_secs(1);
            self.regs.gctl.writef(CRST, true);
            loop {
                if self.regs.gctl.readf(CRST) {
                    break;
                }
                timeout.run().map_err(|()| {
                    log::error!("failed to finish reset");
                    Error::new(EIO)
                })?;
            }
        }

        thread::sleep(Duration::from_millis(2));

        let mut ticks: u32 = 0;
        while self.regs.statests.read() == 0 {
            ticks += 1;
            if ticks > 10000 {
                break;
            }
        }

        let statests = self.regs.statests.read();
        log::debug!("Statests: {:04X}", statests);

        for i in 0..15 {
            if (statests >> i) & 0x1 == 1 {
                self.codecs.push(i as CodecAddr);
            }
        }
        Ok(())
    }

    pub fn num_output_streams(&self) -> usize {
        let gcap = self.regs.gcap.read();
        ((gcap >> 12) & 0xF) as usize
    }

    pub fn num_input_streams(&self) -> usize {
        let gcap = self.regs.gcap.read();
        ((gcap >> 8) & 0xF) as usize
    }

    pub fn num_bidirectional_streams(&self) -> usize {
        let gcap = self.regs.gcap.read();
        ((gcap >> 3) & 0xF) as usize
    }

    pub fn num_serial_data_out(&self) -> usize {
        let gcap = self.regs.gcap.read();
        ((gcap >> 1) & 0x3) as usize
    }

    pub fn info(&self) {
        log::debug!(
            "Intel HD Audio Version {}.{}",
            self.regs.vmaj.read(),
            self.regs.vmin.read()
        );
        log::debug!("IHDA: Input Streams: {}", self.num_input_streams());
        log::debug!("IHDA: Output Streams: {}", self.num_output_streams());
        log::debug!(
            "IHDA: Bidirectional Streams: {}",
            self.num_bidirectional_streams()
        );
        log::debug!("IHDA: Serial Data Outputs: {}", self.num_serial_data_out());
        log::debug!("IHDA: 64-Bit: {}", self.regs.gcap.read() & 1 == 1);
    }

    fn get_input_stream_descriptor(
        &self,
        index: usize,
    ) -> Option<&'static mut StreamDescriptorRegs> {
        unsafe {
            if index < self.num_input_streams() {
                Some(&mut *((self.base + 0x80 + index * 0x20) as *mut StreamDescriptorRegs))
            } else {
                None
            }
        }
    }

    fn get_output_stream_descriptor(
        &self,
        index: usize,
    ) -> Option<&'static mut StreamDescriptorRegs> {
        unsafe {
            if index < self.num_output_streams() {
                Some(
                    &mut *((self.base + 0x80 + self.num_input_streams() * 0x20 + index * 0x20)
                        as *mut StreamDescriptorRegs),
                )
            } else {
                None
            }
        }
    }

    fn get_bidirectional_stream_descriptor(
        &self,
        index: usize,
    ) -> Option<&'static mut StreamDescriptorRegs> {
        unsafe {
            if index < self.num_bidirectional_streams() {
                Some(
                    &mut *((self.base
                        + 0x80
                        + self.num_input_streams() * 0x20
                        + self.num_output_streams() * 0x20
                        + index * 0x20) as *mut StreamDescriptorRegs),
                )
            } else {
                None
            }
        }
    }

    fn set_dma_position_buff_addr(&mut self, addr: u64) {
        let addr_val = addr & !0x7F;
        self.regs.dplbase.write((addr_val & 0xFFFFFFFF) as u32);
        self.regs.dpubase.write((addr_val >> 32) as u32);
    }

    fn set_stream_channel(&mut self, addr: WidgetAddr, stream: u8, channel: u8) -> Result<()> {
        let val = ((stream & 0xF) << 4) | (channel & 0xF);
        self.cmd.cmd12(addr, 0x706, val)?;
        Ok(())
    }

    fn set_power_state(&mut self, addr: WidgetAddr, state: u8) -> Result<()> {
        self.cmd.cmd12(addr, 0x705, state & 0xF)?;
        Ok(())
    }

    fn get_supported_formats(&mut self, addr: WidgetAddr) -> Result<u32> {
        Ok(self.cmd.cmd12(addr, 0xF00, 0x0A)? as u32)
    }

    fn get_capabilities(&mut self, addr: WidgetAddr) -> Result<u32> {
        Ok(self.cmd.cmd12(addr, 0xF00, 0x09)? as u32)
    }

    fn set_converter_format(
        &mut self,
        addr: WidgetAddr,
        sr: &super::SampleRate,
        bps: BitsPerSample,
        channels: u8,
    ) -> Result<()> {
        let fmt = super::format_to_u16(sr, bps, channels);
        self.cmd.cmd4(addr, 0x2, fmt)?;
        Ok(())
    }

    fn set_amplifier_gain_mute(
        &mut self,
        addr: WidgetAddr,
        output: bool,
        input: bool,
        left: bool,
        right: bool,
        index: u8,
        mute: bool,
        gain: u8,
    ) -> Result<()> {
        let mut payload: u16 = 0;

        if output {
            payload |= 1 << 15;
        }
        if input {
            payload |= 1 << 14;
        }
        if left {
            payload |= 1 << 13;
        }
        if right {
            payload |= 1 << 12;
        }
        if mute {
            payload |= 1 << 7;
        }
        payload |= ((index as u16) & 0x0F) << 8;
        payload |= (gain as u16) & 0x7F;

        self.cmd.cmd4(addr, 0x3, payload)?;
        Ok(())
    }

    pub fn write_to_output(&mut self, index: u8, buf: &[u8]) -> Poll<Result<usize>> {
        let output = self.get_output_stream_descriptor(index as usize).unwrap();
        let os = self.output_streams.get_mut(index as usize).unwrap();

        //let sample_size:usize = output.sample_size();
        let open_block = (output.link_position() as usize) / os.block_size();

        //log::trace!("Status: {:02X} Pos: {:08X} Output CTL: {:06X}", output.status(), output.link_position(), output.control());

        if os.current_block() == (open_block + 3) % NUM_SUB_BUFFS {
            // Block if we already are 3 buffers ahead
            Poll::Pending
        } else {
            Poll::Ready(os.write_block(buf))
        }
    }

    pub fn handle_interrupts(&mut self) -> bool {
        let intsts = self.regs.intsts.read();
        if ((intsts >> 31) & 1) == 1 {
            // Global Interrupt Status
            if ((intsts >> 30) & 1) == 1 {
                // Controller Interrupt Status
                self.handle_controller_interrupt();
            }

            let sis = intsts & 0x3FFFFFFF;
            if sis != 0 {
                self.handle_stream_interrupts(sis);
            }
        }
        intsts != 0
    }

    pub fn handle_controller_interrupt(&mut self) {}

    pub fn handle_stream_interrupts(&mut self, sis: u32) {
        let iss = self.num_input_streams();
        let oss = self.num_output_streams();
        let bss = self.num_bidirectional_streams();

        for i in 0..iss {
            if ((sis >> i) & 1) == 1 {
                let input = self.get_input_stream_descriptor(i).unwrap();
                input.clear_interrupts();
            }
        }

        for i in 0..oss {
            if ((sis >> (i + iss)) & 1) == 1 {
                let output = self.get_output_stream_descriptor(i).unwrap();
                output.clear_interrupts();
            }
        }

        for i in 0..bss {
            if ((sis >> (i + iss + oss)) & 1) == 1 {
                let bid = self.get_bidirectional_stream_descriptor(i).unwrap();
                bid.clear_interrupts();
            }
        }
    }

    fn validate_path(&mut self, path: &Vec<&str>) -> bool {
        log::debug!("Path: {:?}", path);
        let mut it = path.iter();
        match it.next() {
            Some(card_str) if (*card_str).starts_with("card") => {
                match usize::from_str_radix(&(*card_str)[4..], 10) {
                    Ok(card_num) => {
                        log::debug!("Card# {}", card_num);
                        match it.next() {
                            Some(codec_str) if (*codec_str).starts_with("codec#") => {
                                match usize::from_str_radix(&(*codec_str)[6..], 10) {
                                    Ok(_codec_num) => {
                                        //let id = self.next_id.fetch_add(1, Ordering::SeqCst);
                                        //self.handles.lock().insert(id, Handle::Disk(disk.clone(), 0));
                                        true
                                    }
                                    _ => false,
                                }
                            }
                            Some(pcmout_str) if (*pcmout_str).starts_with("pcmout") => {
                                match usize::from_str_radix(&(*pcmout_str)[6..], 10) {
                                    Ok(pcmout_num) => {
                                        log::debug!("pcmout {}", pcmout_num);
                                        true
                                    }
                                    _ => false,
                                }
                            }
                            Some(pcmin_str) if (*pcmin_str).starts_with("pcmin") => {
                                match usize::from_str_radix(&(*pcmin_str)[6..], 10) {
                                    Ok(pcmin_num) => {
                                        log::debug!("pcmin {}", pcmin_num);
                                        true
                                    }
                                    _ => false,
                                }
                            }
                            _ => false,
                        }
                    }
                    _ => false,
                }
            }
            Some(cards_str) if *cards_str == "cards" => true,
            _ => false,
        }
    }
}

impl Drop for IntelHDA {
    fn drop(&mut self) {
        log::debug!("IHDA: Deallocating IHDA driver.");
    }
}

impl SchemeSync for IntelHDA {
    fn open(&mut self, path: &str, _flags: usize, ctx: &CallerCtx) -> Result<OpenResult> {
        //let path: Vec<&str>;
        /*
        match str::from_utf8(_path) {
            Ok(p)  => {
                    path = p.split("/").collect();
                    if !self.validate_path(&path) {
                        return Err(Error::new(EINVAL));

                },
            Err(_) => {return Err(Error::new(EINVAL));},
        }*/

        // TODO:
        if ctx.uid != 0 {
            return Err(Error::new(EACCES));
        }
        let handle = match path.trim_matches('/') {
            //TODO: allow multiple codecs
            "codec" => Handle::StrBuf(self.dump_codec(0).into_bytes()),
            _ => Handle::Todo,
        };
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.handles.lock().insert(id, handle);

        // TODO: always positioned?
        Ok(OpenResult::ThisScheme {
            number: id,
            flags: NewFdFlags::POSITIONED,
        })
    }

    fn read(
        &mut self,
        id: usize,
        buf: &mut [u8],
        offset: u64,
        _flags: u32,
        _ctx: &CallerCtx,
    ) -> Result<usize> {
        let handles = self.handles.lock();
        let Some(Handle::StrBuf(strbuf)) = handles.get(&id) else {
            return Err(Error::new(EBADF));
        };

        let src = usize::try_from(offset)
            .ok()
            .and_then(|o| strbuf.get(o..))
            .unwrap_or(&[]);
        let len = src.len().min(buf.len());
        buf[..len].copy_from_slice(&src[..len]);
        Ok(len)
    }

    fn write(
        &mut self,
        id: usize,
        buf: &[u8],
        _offset: u64,
        _flags: u32,
        _ctx: &CallerCtx,
    ) -> Result<usize> {
        let index = {
            let mut handles = self.handles.lock();
            match handles.get_mut(&id).ok_or(Error::new(EBADF))? {
                Handle::Todo => 0,
                _ => return Err(Error::new(EBADF)),
            }
        };

        //log::debug!("Int count: {}", self.int_counter);

        match self.write_to_output(index, buf) {
            Poll::Ready(r) => r,
            Poll::Pending => Err(Error::new(EWOULDBLOCK)),
        }
    }

    fn fpath(&mut self, id: usize, buf: &mut [u8], _ctx: &CallerCtx) -> Result<usize> {
        let mut handles = self.handles.lock();
        let _handle = handles.get_mut(&id).ok_or(Error::new(EBADF))?;

        let mut i = 0;
        let scheme_path = b"/scheme/audiohw";
        while i < buf.len() && i < scheme_path.len() {
            buf[i] = scheme_path[i];
            i += 1;
        }
        Ok(i)
    }

    fn on_close(&mut self, id: usize) {
        let _ = self.handles.lock().remove(&id);
    }
}
