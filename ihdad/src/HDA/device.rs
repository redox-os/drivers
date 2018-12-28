#![allow(dead_code)]

use std::{mem, thread, ptr, fmt};
use std::cmp;
use std::cmp::{max, min};
use std::ptr::copy_nonoverlapping;
use std::sync::Arc;
use std::cell::RefCell;
use std::collections::HashMap;
use std::str;
use std::string::String;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
extern crate syscall;


use syscall::PHYSMAP_WRITE;
use syscall::error::{Error, EACCES, EBADF, Result, EINVAL};
use syscall::flag::{SEEK_SET, SEEK_CUR, SEEK_END};
use syscall::io::{Mmio, Io};
use syscall::scheme::SchemeMut;

use spin::Mutex;

use super::BufferDescriptorListEntry;
use super::common::*;
use super::StreamDescriptorRegs;
use super::StreamBuffer;
use super::BitsPerSample;
use super::CommandBuffer;
use super::HDANode;
use super::OutputStream;

// GCTL - Global Control
const CRST:   u32 = 1 << 0; // 1 bit
const FNCTRL: u32 = 1 << 1; // 1 bit
const UNSOL:  u32 = 1 << 8; // 1 bit

// CORBCTL
const CMEIE:   u8 = 1 << 0; // 1 bit
const CORBRUN: u8 = 1 << 1; // 1 bit

// CORBSIZE
const CORBSZCAP: (u8,u8) = (4, 4);
const CORBSIZE:  (u8,u8) = (0, 2);

// CORBRP
const CORBRPRST: u16 = 1 << 15;

// RIRBWP
const RIRBWPRST: u16 = 1 << 15;

// RIRBCTL
const RINTCTL:   u8 = 1 << 0; // 1 bit
const RIRBDMAEN: u8 = 1 << 1; // 1 bit

// ICS
const ICB:      u16 = 1 << 0;
const IRV:      u16 = 1 << 1;


// CORB and RIRB offset

const COMMAND_BUFFER_OFFSET: usize = 0x40;


const NUM_SUB_BUFFS: usize = 2;
const SUB_BUFF_SIZE: usize = 0x4000;


enum Handle {
	Pcmout(usize, usize, usize), // Card, index, block_ptr
	Pcmin(usize, usize, usize),  // Card, index, block_ptr
	StrBuf(Vec<u8>,usize),
}



#[repr(packed)]
#[allow(dead_code)]
struct Regs {
	gcap:       Mmio<u16>,
	vmin:       Mmio<u8>,
	vmaj:       Mmio<u8>,
	outpay:     Mmio<u16>,
	inpay:      Mmio<u16>,
	gctl:       Mmio<u32>,
	wakeen:     Mmio<u16>,
	statests:   Mmio<u16>,
	gsts:       Mmio<u16>,
	rsvd0:      [Mmio<u8>;  6],
	outstrmpay: Mmio<u16>,
	instrmpay:  Mmio<u16>,
	rsvd1:      [Mmio<u8>;  4],
	intctl:     Mmio<u32>,
	intsts:     Mmio<u32>,
	rsvd2:      [Mmio<u8>;  8],
	walclk:     Mmio<u32>,
	rsvd3:      Mmio<u32>,
	ssync:      Mmio<u32>,
	rsvd4:      Mmio<u32>,

	corblbase:  Mmio<u32>,
	corbubase:  Mmio<u32>,
	corbwp:     Mmio<u16>,
	corbrp:     Mmio<u16>,
	corbctl:    Mmio<u8>,
	corbsts:    Mmio<u8>,
	corbsize:   Mmio<u8>,
	rsvd5:      Mmio<u8>,

	rirblbase:  Mmio<u32>,
	rirbubase:  Mmio<u32>,
	rirbwp:     Mmio<u16>,
	rintcnt:    Mmio<u16>,
	rirbctl:    Mmio<u8>,
	rirbsts:    Mmio<u8>,
	rirbsize:   Mmio<u8>,
	rsvd6:      Mmio<u8>,

	icoi:       Mmio<u32>,
	irii:       Mmio<u32>,
	ics:        Mmio<u16>,
	rsvd7:      [Mmio<u8>;  6],

	dplbase:    Mmio<u32>, // 0x70
	dpubase:    Mmio<u32>, // 0x74
	
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
	

	buff_desc: &'static mut [BufferDescriptorListEntry; 256],
	buff_desc_phys: usize,
	
	
	output_streams:         Vec<OutputStream>,

	buffs: Vec<Vec<StreamBuffer>>,

	int_counter: usize,
	handles: Mutex<BTreeMap<usize, Handle>>,
	next_id: AtomicUsize,
}


impl IntelHDA {
	pub unsafe fn new(base: usize, vend_prod:u32) -> Result<Self> {
	
		let regs = &mut *(base as *mut Regs);
		
		

		let buff_desc_phys = unsafe {
			syscall::physalloc(0x1000)
				.expect("Could not allocate physical memory for buffer descriptor list.")
		};

		
		let buff_desc_virt = unsafe { 
			syscall::physmap(buff_desc_phys, 0x1000, PHYSMAP_WRITE)
				.expect("ihdad: failed to map address for buffer descriptor list.") 
		};

		print!("Virt: {:016X}, Phys: {:016X}\n", buff_desc_virt, buff_desc_phys);
		

		let buff_desc = &mut *(buff_desc_virt as *mut [BufferDescriptorListEntry;256]);
		
		

		let cmd_buff_address = unsafe {
			syscall::physalloc(0x1000)
				.expect("Could not allocate physical memory for CORB and RIRB.")
		};

		
		let cmd_buff_virt = unsafe { syscall::physmap(cmd_buff_address, 0x1000, PHYSMAP_WRITE).expect("ihdad: failed to map address for CORB/RIRB buff") };

		print!("Virt: {:016X}, Phys: {:016X}\n", cmd_buff_virt, cmd_buff_address);
		let mut module = IntelHDA {
			vend_prod: vend_prod,
			base: base,
			regs: regs,
			
			cmd: CommandBuffer::new(base + COMMAND_BUFFER_OFFSET, cmd_buff_address, cmd_buff_virt),

			beep_addr: (0,0),
			
			widget_map: HashMap::<WidgetAddr, HDANode>::new(),


			codecs: Vec::<CodecAddr>::new(),
			
			outputs: Vec::<WidgetAddr>::new(),
			inputs: Vec::<WidgetAddr>::new(),

			output_pins: Vec::<WidgetAddr>::new(),
			input_pins: Vec::<WidgetAddr>::new(),

			buff_desc: buff_desc,
			buff_desc_phys: buff_desc_phys,



			output_streams: Vec::<OutputStream>::new(),


			buffs: Vec::<Vec<StreamBuffer>>::new(),


			int_counter: 0,
			handles: Mutex::new(BTreeMap::new()),
			next_id: AtomicUsize::new(0), 
		};
		
		module.init();
		
		// module.info();
		module.enumerate();
		
		module.configure();
		print!("IHDA: Initialization finished.\n");
		Ok(module)

	}

	pub fn init(&mut self) -> bool {
		self.reset_controller();

		let use_immediate_command_interface = match self.vend_prod {
			
			0x8086_2668 => false,
			_ => true,
		};	

		self.cmd.init(use_immediate_command_interface);	
		self.init_interrupts();
		
		true
	}

	pub fn init_interrupts(&mut self) {
		// TODO: provide a function to enable certain interrupts
		// This just enables the first output stream interupt and the global interrupt

		// TODO: No magic numbers! Bad Schemm.
		self.regs.intctl.write((1 << 31) | /* (1 << 30) |*/ (1 << 4));
	}


	

	pub fn irq(&mut self) -> bool {		
		self.int_counter += 1;
		
		self.handle_interrupts();

		true
	}

	pub fn int_count(&self) -> usize {
		self.int_counter
	}

	pub fn read_node(&mut self, addr: WidgetAddr) -> HDANode {
		let mut node = HDANode::new();
		let mut temp:u64;

		node.addr = addr;
		
		temp = self.cmd.cmd12( addr, 0xF00, 0x04);

		node.subnode_count = (temp & 0xff) as u16;
		node.subnode_start = ((temp >> 16) & 0xff) as u16;
		
		if addr == (0,0) {
			return node;
		}
		temp = self.cmd.cmd12(addr, 0xF00, 0x04);

		node.function_group_type = (temp & 0xff) as u8;

		temp = self.cmd.cmd12(addr, 0xF00, 0x09);
		node.capabilities = temp as u32;


		temp = self.cmd.cmd12(addr, 0xF00, 0x0E);

		node.conn_list_len = (temp & 0xFF) as u8;

		node.connections = self.node_get_connection_list(&node);
		

		node.config_default = self.cmd.cmd12(addr, 0xF1C, 0x00) as u32;

		node

	}

	pub fn node_get_connection_list(&mut self, node: &HDANode) -> Vec<WidgetAddr> {
		
		let len_field: u8 = (self.cmd.cmd12(node.addr, 0xF00, 0x0E) & 0xFF) as u8;
		
		// Highest bit is if addresses are represented in longer notation
		// lower 7 is actual count
			
		let count:u8 = len_field & 0x7F;
		let use_long_addr: bool = (len_field >> 7) & 0x1 == 1;
		
		let mut current: u8 = 0;
		
		let mut list = Vec::<WidgetAddr>::new();

		while current < count {
			
			let response: u32 = (self.cmd.cmd12(node.addr, 0xF02, current) & 0xFFFFFFFF) as u32;
			
			if use_long_addr {
				for i in 0..2 {
					let addr_field = ((response >> (16 * i)) & 0xFFFF) as u16;
					let addr = (addr_field & 0x7FFF);

					if addr == 0 { break; }
					
					if (addr_field >> 15) & 0x1 == 0x1 {
						for i in (list.pop().unwrap().1 .. (addr + 1)) {
							list.push((node.addr.0, i));
						}
					} else {
						list.push((node.addr.0, addr));
					}
				}

			} else {

				for i in 0..4 {
					let addr_field = ((response >> (8 * i)) & 0xff) as u16;
					let addr = (addr_field & 0x7F);
					
					if addr == 0 { break; }				

					if (addr_field >> 7) & 0x1 == 0x1 {
						for i in (list.pop().unwrap().1 .. (addr + 1)) {
							list.push((node.addr.0, i));
						}
					} else {
						list.push((node.addr.0, addr));
					}
				}

			}
			
			current = list.len() as u8;

		}
		list
		
	}
	pub fn enumerate(&mut self) {


		self.output_pins.clear();
		self.input_pins.clear();


		let codec:u8 = 0;
		
		let root = self.read_node((codec,0));
		
		// print!("{}\n", root);

		let root_count = root.subnode_count;
		let root_start = root.subnode_start;
		

		//FIXME: So basically the way this is set up is to only support one codec and hopes the first one is an audio
		for i in 0..root_count { 
			let afg = self.read_node((codec, root_start + i));
			// print!("{}\n", afg);
			let afg_count = afg.subnode_count;
			let afg_start = afg.subnode_start;
			
			for j in 0..afg_count {
				
				let mut widget = self.read_node((codec, afg_start + j));
				widget.is_widget = true;
				match widget.widget_type() {
					HDAWidgetType::AudioOutput => {self.outputs.push(widget.addr)},
					HDAWidgetType::AudioInput  => {self.inputs.push(widget.addr)},
					HDAWidgetType::BeepGenerator => {self.beep_addr = widget.addr },
					HDAWidgetType::PinComplex => {
						let config = widget.configuration_default();
						if config.is_output() {
							self.output_pins.push(widget.addr);
						} else if (config.is_input()) {
							self.input_pins.push(widget.addr);
						}


						print!("{:02X}{:02X} {}\n", widget.addr().0, widget.addr().1, config);
						
					},
					_ => {},
				}
				
				print!("{}\n", widget);
				self.widget_map.insert(widget.addr(), widget);
			}	
		}
	}

	


	pub fn find_best_output_pin(&self) -> Option<WidgetAddr>{
		let outs = &self.output_pins;
		if outs.len() == 0 {
			None
		} else if outs.len() == 1 {
			Some(outs[0])
		} else {
			// TODO: Somehow find the best.
			// Slightly okay is find the speaker with the lowest sequence number.
						
			for &out in outs {
				let widget = self.widget_map.get(&out).unwrap();
				
				let cd = widget.configuration_default();
				if cd.sequence() == 0 && cd.default_device() == DefaultDevice::Speaker {
					return Some(out);
				}
			}

			None
		}
	}

	

	pub fn find_path_to_dac(&self, addr: WidgetAddr) -> Option<Vec<WidgetAddr>>{
		let widget = self.widget_map.get(&addr).unwrap();
		if widget.widget_type() == HDAWidgetType::AudioOutput {
			return Some(vec![addr]);
		}else{
			if widget.connections.len() == 0 {
				return None;
			}else{
				// TODO: do more than just first widget

				let mut res = self.find_path_to_dac(widget.connections[0]);
				match res {
					Some(p) => {
						let mut ret = p.clone();
						ret.insert(0, addr);
						Some(ret)
					},
					None => {None},
				}
			}

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

		
		self.output_streams.push(OutputStream::new(2, 0x4000, r));

		let o = self.output_streams.get_mut(0).unwrap();


		self.buff_desc[0].set_address(o.phys());
		self.buff_desc[0].set_length(o.block_size() as u32);
		self.buff_desc[0].set_interrupt_on_complete(true);		

		
		self.buff_desc[1].set_address(o.phys() + o.block_size());
		self.buff_desc[1].set_length(o.block_size() as u32);
		self.buff_desc[1].set_interrupt_on_complete(true);
		

	}


	

	pub fn configure(&mut self) {
		
		let outpin = self.find_best_output_pin().expect("IHDA: No output pins?!");
		
		//print!("Best pin: {:01X}:{:02X}\n", outpin.0, outpin.1);

		let path = self.find_path_to_dac(outpin).unwrap();

		let dac = *path.last().unwrap();
		let pin = *path.first().unwrap();

		//print!("Path to DAC: {:?}\n", path);

		// Pin enable
		self.cmd.cmd12(pin, 0x707, 0x40);
		

		// EAPD enable
		self.cmd.cmd12(pin, 0x70C, 2);

		self.set_stream_channel(dac, 1, 0);

		self.update_sound_buffers();


		//print!("Supported Formats: {:08X}\n", self.get_supported_formats((0,0x1)));
		//print!("Capabilities: {:08X}\n", self.get_capabilities(path[0]));

		let output = self.get_output_stream_descriptor(0).unwrap();
		
		
		
		output.set_address(self.buff_desc_phys);


		output.set_pcm_format(&super::SR_44_1, BitsPerSample::Bits16, 2);
		output.set_cyclic_buffer_length(0x8000); // number of bytes
		output.set_stream_number(1);
		output.set_last_valid_index(1);
		output.set_interrupt_on_completion(true);


		self.set_power_state(dac, 0); // Power state 0 is fully on
		self.set_converter_format(dac, &super::SR_44_1, BitsPerSample::Bits16, 2);
		
		
		self.cmd.cmd12(dac, 0xA00, 0);

		// Unmute and set gain for pin complex and DAC
		self.set_amplifier_gain_mute(dac, true, true, true, true, 0, false, 0x7f);
		self.set_amplifier_gain_mute(pin, true, true, true, true, 0, false, 0x7f);

		output.run();
		
	}
	/*

	pub fn configure_vbox(&mut self) {
		
		let outpin = self.find_best_output_pin().expect("IHDA: No output pins?!");
		
		print!("Best pin: {:01X}:{:02X}\n", outpin.0, outpin.1);

		let path = self.find_path_to_dac(outpin).unwrap();
		print!("Path to DAC: {:?}\n", path);

		// Pin enable
		self.cmd.cmd12((0,0xC), 0x707, 0x40);
		

		// EAPD enable
		self.cmd.cmd12((0,0xC), 0x70C, 2);

		self.set_stream_channel((0,0x3), 1, 0);

		self.update_sound_buffers();


		print!("Supported Formats: {:08X}\n", self.get_supported_formats((0,0x1)));
		print!("Capabilities: {:08X}\n", self.get_capabilities((0,0x1)));

		let output = self.get_output_stream_descriptor(0).unwrap();
		
		output.set_address(self.buff_desc_phys);

		output.set_pcm_format(&super::SR_44_1, BitsPerSample::Bits16, 2);
		output.set_cyclic_buffer_length(0x8000); 
		output.set_stream_number(1);
		output.set_last_valid_index(1);
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

	// BEEP!!
	pub fn beep(&mut self, div:u8) {
		let addr = self.beep_addr;
		if addr != (0,0) {
			
			let _ = self.cmd.cmd12(addr, 0xF0A, div);
			
		}

	}
	
	pub fn read_beep(&mut self) -> u8 {
		let addr = self.beep_addr;
		if addr != (0,0) {

			self.cmd.cmd12(addr, 0x70A, 0) as u8
		}else{
			0
		}

	}

	pub fn reset_controller(&mut self) -> bool {

		self.regs.statests.write(0xFFFF);

		// 3.3.7
		self.regs.gctl.writef(CRST, false);
		loop {
			if ! self.regs.gctl.readf(CRST) {
				break;
			}
		}
		self.regs.gctl.writef(CRST, true);
		loop {
			if self.regs.gctl.readf(CRST) {
				break;
			}
		}

		let mut ticks:u32 = 0;
		while self.regs.statests.read() == 0 {
			ticks += 1;
			if ticks > 10000 { break;}

		}

		let statests = self.regs.statests.read();
		print!("Statests: {:04X}\n", statests);

		for i in 0..15 {
			if (statests >> i) & 0x1 == 1 {
				self.codecs.push(i as CodecAddr);
			} 
		}
		true

	}
	
	pub fn num_output_streams(&self) -> usize{
		let gcap = self.regs.gcap.read();
		((gcap >> 12) & 0xF) as usize
	}
	
	pub fn num_input_streams(&self) -> usize{
		let gcap = self.regs.gcap.read();
		((gcap >> 8) & 0xF) as usize
	}

	pub fn num_bidirectional_streams(&self) -> usize{
		let gcap = self.regs.gcap.read();
		((gcap >> 3) & 0xF) as usize
	}

	pub fn num_serial_data_out(&self) -> usize{
		let gcap = self.regs.gcap.read();
		((gcap >> 1) & 0x3) as usize
	}

	pub fn info(&self) {
		print!("Intel HD Audio Version {}.{}\n", self.regs.vmaj.read(), self.regs.vmin.read());
		print!("IHDA: Input Streams: {}\n", self.num_input_streams());
		print!("IHDA: Output Streams: {}\n", self.num_output_streams());
		print!("IHDA: Bidirectional Streams: {}\n", self.num_bidirectional_streams());
		print!("IHDA: Serial Data Outputs: {}\n", self.num_serial_data_out());
		print!("IHDA: 64-Bit: {}\n", self.regs.gcap.read() & 1 == 1);
	}

	

	fn get_input_stream_descriptor(&self, index: usize) -> Option<&'static mut StreamDescriptorRegs> {
		unsafe {
			if index < self.num_input_streams() {
				Some(&mut *((self.base + 0x80 + index * 0x20) as *mut StreamDescriptorRegs))
			}else{
				None
			}
		}
	}

	fn get_output_stream_descriptor(&self, index: usize) -> Option<&'static mut StreamDescriptorRegs> {
		unsafe {
			if index < self.num_output_streams() {
				Some(&mut *((self.base + 0x80 + 
							self.num_input_streams() * 0x20 +
							index * 0x20) as *mut StreamDescriptorRegs))
			}else{
				None
			}
		}
	}


	fn get_bidirectional_stream_descriptor(&self, index: usize) -> Option<&'static mut StreamDescriptorRegs> {
		unsafe {
			if index < self.num_bidirectional_streams() {
				Some(&mut *((self.base + 0x80 + 
							self.num_input_streams() * 0x20 +
							self.num_output_streams() * 0x20 +
							index * 0x20) as *mut StreamDescriptorRegs))
			}else{
				None
			}
		}
	}

	fn set_dma_position_buff_addr(&mut self, addr: usize) {
		let addr_val = addr & !0x7F;
		self.regs.dplbase.write((addr_val & 0xFFFFFFFF) as u32);
		self.regs.dpubase.write((addr_val >> 32) as u32);
	}


	fn set_stream_channel(&mut self, addr: WidgetAddr, stream: u8, channel:u8) {
		let val = ((stream & 0xF) << 4) | (channel & 0xF);
		self.cmd.cmd12(addr, 0x706, val);
	}

	fn set_power_state(&mut self, addr:WidgetAddr, state:u8) {
		self.cmd.cmd12(addr, 0x705, state & 0xF) as u32;
	}

	fn get_supported_formats(&mut self, addr: WidgetAddr) -> u32 {
		self.cmd.cmd12(addr, 0xF00, 0x0A) as u32
	}

	fn get_capabilities(&mut self, addr: WidgetAddr) -> u32 {
		self.cmd.cmd12(addr, 0xF00, 0x09) as u32
	}

	fn set_converter_format(&mut self, addr:WidgetAddr, sr: &super::SampleRate, bps: BitsPerSample, channels:u8) {
		let fmt = super::format_to_u16(sr, bps, channels);
		self.cmd.cmd4(addr, 0x2, fmt);
	}

	

	fn set_amplifier_gain_mute(&mut self, addr: WidgetAddr, output:bool, input:bool, left:bool, right:bool, index:u8, mute:bool, gain: u8) {

		let mut payload: u16 = 0;
		
		if output { payload |= (1 << 15); }
		if input  { payload |= (1 << 14); }
		if left   { payload |= (1 << 13); }
		if right  { payload |= (1 << 12); }
		if mute   { payload |= (1 <<  7); }
		payload |= ((index as u16) & 0x0F) << 8;
		payload |= ((gain  as u16) & 0x7F);

		self.cmd.cmd4(addr, 0x3, payload);

	
	}

	pub fn write_to_output(&mut self, index:u8, buf: &[u8]) -> Result<usize> {

		let mut output = self.get_output_stream_descriptor(index as usize).unwrap();
		let mut os = self.output_streams.get_mut(index as usize).unwrap();
		
		
		
		let sample_size:usize = output.sample_size();
		let mut open_block = (output.link_position() as usize) / os.block_size();



				
		if open_block == 0 {
			open_block = 1;
		} else {
			open_block = open_block - 1;
		}

		//print!("Status: {:02X} Pos: {:08X} Output CTL: {:06X}\n", output.status(), output.link_position(), output.control());
		while open_block == os.current_block() {

			open_block = (output.link_position() as usize) / os.block_size();
				
			if open_block == 0 {
				open_block = os.block_count() - 1;
			} else {
				open_block = open_block - 1;
			}
			
			
			thread::yield_now();
		}
		
		let len = min(os.block_size(), buf.len());
		
		os.write_block(buf)
	}

	pub fn handle_interrupts(&mut self) {

		let intsts = self.regs.intsts.read();
		let sis = intsts & 0x3FFFFFFF;      
		// print!("IHDA INTSTS: {:08X}\n", intsts);     
		if ((intsts >> 31) & 1) == 1 {           // Global Interrupt Status
			if ((intsts >> 30) & 1) == 1 {   // Controller Interrupt Status
				self.handle_controller_interrupt();
			} 

			if sis != 0 {
				self.handle_stream_interrupts(sis);
			}
		}
	}

	pub fn handle_controller_interrupt(&mut self) {
		
	}

	pub fn handle_stream_interrupts(&mut self, sis: u32) {
		let oss = self.num_output_streams();
		let iss = self.num_input_streams();
		let bss = self.num_bidirectional_streams();

		for i in 0..iss {
			if ((sis >> i) & 1 ) == 1 {
				let mut input = self.get_input_stream_descriptor(i).unwrap();	
				input.clear_interrupts();
			}
		}

		for i in 0..oss {
			if ((sis >> (i + iss)) & 1 ) == 1 {
				let mut output = self.get_output_stream_descriptor(i).unwrap();	
				output.clear_interrupts();
			}
		}

		for i in 0..bss {
			if ((sis >> (i + iss + oss)) & 1 ) == 1 {
				let mut bid = self.get_bidirectional_stream_descriptor(i).unwrap();	
				bid.clear_interrupts();
			}
		}
	}
	
	fn validate_path(&mut self, path: &Vec<&str>) -> bool {

		print!("Path: {:?}\n", path);
		let mut it = path.iter();
		match it.next() {
			Some(card_str) if (*card_str).starts_with("card") => {	
				match usize::from_str_radix(&(*card_str)[4..], 10) {
					Ok(card_num) => {
						print!("Card# {}\n", card_num);
						match it.next() {
							Some(codec_str) if (*codec_str).starts_with("codec#") => {	
								match usize::from_str_radix(&(*codec_str)[6..], 10) {
									Ok(codec_num) => {

										let id = self.next_id.fetch_add(1, Ordering::SeqCst);
                    								//self.handles.lock().insert(id, Handle::Disk(disk.clone(), 0));
										true

									},
									_ => false,
								}	
							},
							Some(pcmout_str) if (*pcmout_str).starts_with("pcmout") => {	
								match usize::from_str_radix(&(*pcmout_str)[6..], 10) {
									Ok(pcmout_num) => {
										print!("pcmout {}\n", pcmout_num);
										true
									},
									_ => false,
								}	
							},
							Some(pcmin_str) if (*pcmin_str).starts_with("pcmin") => {	
								match usize::from_str_radix(&(*pcmin_str)[6..], 10) {
									Ok(pcmin_num) => {
										print!("pcmin {}\n", pcmin_num);
										true
									},
									_ => false,
								}	
							},
							_ => false,
						}
					},
					_ => false,
				}	
			},
			Some(cards_str) if *cards_str == "cards" => {
				true
			},
			_ => false,	
		}
	}	
}


impl Drop for IntelHDA {
	fn drop(&mut self) {
		print!("IHDA: Deallocating IHDA driver.\n");

	}
}


impl SchemeMut for IntelHDA {

	

	fn open(&mut self, _path: &[u8], flags: usize, uid: u32, _gid: u32) -> Result<usize> {
		let path: Vec<&str>;
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
		if uid == 0 {
			Ok(0)
		} else {
			Err(Error::new(EACCES))
		}
	}

	fn write(&mut self, _id: usize, buf: &[u8]) -> Result<usize> {
	
		//print!("Int count: {}\n", self.int_counter);
		

		self.write_to_output(0, buf)
	}

	fn seek(&mut self, id: usize, pos: usize, whence: usize) -> Result<usize> {
		let mut handles = self.handles.lock();
		match *handles.get_mut(&id).ok_or(Error::new(EBADF))? {
			Handle::StrBuf(ref mut strbuf, ref mut size) => {
				let len = strbuf.len() as usize;
				*size = match whence {
					SEEK_SET => cmp::min(len, pos),
					SEEK_CUR => cmp::max(0, cmp::min(len as isize, *size as isize + pos as isize)) as usize,
					SEEK_END => cmp::max(0, cmp::min(len as isize,   len as isize + pos as isize)) as usize,
					_ => return Err(Error::new(EINVAL))
				};
				Ok(*size)
			},
	            
			_ => Err(Error::new(EINVAL)),
	        }
    	}


	fn close(&mut self, _id: usize) -> Result<usize> {
		let mut handles = self.handles.lock();
        	handles.remove(&_id).ok_or(Error::new(EBADF)).and(Ok(0))
    	}

    fn fpath(&mut self, id: usize, buf: &mut [u8]) -> Result<usize> {
        //let mut handles = self.handles.lock();
        //let handle = handles.get_mut(&id).ok_or(Error::new(EBADF))?;

        let mut i = 0;
        let scheme_path = b"audio:";
        while i < buf.len() && i < scheme_path.len() {
            buf[i] = scheme_path[i];
            i += 1;
        }
        Ok(i)
    }
}
