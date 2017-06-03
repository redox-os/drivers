use std::{mem, thread, ptr, fmt};
use std::cmp::{max, min};

use syscall::MAP_WRITE;
use syscall::error::{Error, EACCES, EWOULDBLOCK, Result};
use syscall::flag::O_NONBLOCK;
use syscall::io::{Dma, Mmio, Io, ReadOnly};
use syscall::scheme::SchemeMut;
use std::sync::Arc;
use std::cell::RefCell;

extern crate syscall;

use std::ptr::copy_nonoverlapping;


use super::BufferDescriptorListEntry;
use super::common::*;
use super::StreamDescriptorRegs;
use super::StreamBuffer;
use super::BitsPerSample;

use super::HDANode;


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



#[repr(packed)]
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
	base: usize,
	regs: &'static mut Regs,


	corb_rirb_base_phys: usize,

	corb_base: *mut u32,
	rirb_base: *mut u64,

	corb_count: usize,
	rirb_count: usize,

	rirb_rp:    u16,

	codecs: Vec<HDANodeAddr>,

	nodes: Vec<HDANode>,
	

	output_pins: Vec<HDANodeAddr>,

	input_pins: Vec<HDANodeAddr>,

	beep_addr: HDANodeAddr,
	

	buff_desc: &'static mut [BufferDescriptorListEntry; 256],
	buff_desc_phys: usize,
	
	
	output_buff:            usize,
	output_buff_phys:       usize,
	output_buff_length:     usize,
	output_buff_wp:         usize,
	output_current_block:   usize,

	buffs: Vec<Vec<StreamBuffer>>,

	int_counter: usize,
}


impl IntelHDA {
	pub unsafe fn new(base: usize) -> Result<Self> {
	
		let regs = &mut *(base as *mut Regs);
		
		let buff_desc_phys = unsafe {
			syscall::physalloc(0x1000)
				.expect("Could not allocate physical memory for buffer descriptor list.")
		};

		
		let buff_desc_virt = unsafe { 
			syscall::physmap(buff_desc_phys, 0x1000, MAP_WRITE)
				.expect("ihdad: failed to map address for buffer descriptor list.") 
		};
		

		let buff_desc = &mut *(buff_desc_virt as *mut [BufferDescriptorListEntry;256]);
		
		
		let output_length = 0x8000;

		let output_phys = unsafe {
			syscall::physalloc(output_length)
				.expect("Could not allocate physical memory for buffer descriptor list.")
		};

		
		let output_virt = unsafe { 
			syscall::physmap(output_phys, output_length, MAP_WRITE)
				.expect("ihdad: failed to map address for buffer descriptor list.") 
		};

	
		let mut module = IntelHDA {
			base: base,
			regs: regs,
			corb_base: ptr::null_mut(),
			rirb_base: ptr::null_mut(),
			corb_rirb_base_phys: 0,			

			corb_count: 0,
			rirb_count: 0,
			rirb_rp:    0,

			beep_addr: 0,
			
			codecs: Vec::<HDANodeAddr>::new(),
			nodes: Vec::<HDANode>::new(),
			
			output_pins: Vec::<HDANodeAddr>::new(),
			input_pins: Vec::<HDANodeAddr>::new(),

			buff_desc: buff_desc,
			buff_desc_phys: buff_desc_phys,

			output_buff: output_virt,
			output_buff_phys: output_phys,
			output_buff_length: output_length,
			output_buff_wp: 0,
			output_current_block: 0,

			buffs: Vec::<Vec<StreamBuffer>>::new(),


			int_counter: 0,
		};
		module.init();
		//module.info();
		module.enumerate();
		module.vbox_speaker_test();
		print!("IHDA: Initialization finished.\n");
		Ok(module)

	}

	pub fn init(&mut self) -> bool {
	
		self.reset_controller();
		self.init_corb_and_rirb();

		self.init_interrupts();
		//print!("Command 0xF0000: {:016X}\n", self.read_response());
		//print!("Command 0xF0004: {:016X}\n", self.send_immediate_command(0xF0004));

		
		true
		
	}

	pub fn init_interrupts(&mut self) {
		// TODO: provide a function to enable certain interrupts
		// This just enables the first output stream interupt and the global interrupt

		// TODO: No magic numbers! Bad Schemm.
		self.regs.intctl.write((1 << 31) | (1 << 30) | (1 << 4));
		
		

	}


	

	pub fn irq(&mut self) -> bool {		
		self.int_counter += 1;
		
		self.handle_interrupts();

		true
	}

	pub fn int_count(&self) -> usize {
		self.int_counter
	}

	pub fn read_node(&mut self, addr: HDANodeAddr) -> HDANode {
		let mut node = HDANode::new();
		let mut temp:u64;

		node.addr = addr;

		self.send_command(Self::node_command(0x00, addr, 0xF00, 0x04));
		temp = self.read_response();

		node.subnode_count = (temp & 0xff) as u16;
		node.subnode_start = ((temp >> 16) & 0xff) as u16;
		
		self.send_command(Self::node_command(0x00, addr, 0xF00, 0x05));
		temp = self.read_response();

		node.function_group_type = (temp & 0xff) as u8;

		self.send_command(Self::node_command(0x00, addr, 0xF00, 0x09));
		temp = self.read_response();

		node.capabilities = temp as u32;


		self.send_command(Self::node_command(0x00, addr, 0xF00, 0x09));
		temp = self.read_response();

		node.capabilities = temp as u32;

		self.send_command(Self::node_command(0x00, addr, 0xF00, 0x0E));
		temp = self.read_response();
		
		node.conn_list_len = (temp & 0xFF) as u8;

		node.connections = self.node_get_connection_list(&node);

		self.send_command(Self::node_command(0x00, addr, 0xF1C, 0x00));
		node.config_default = self.read_response() as u32;
		
		node

	}

	pub fn node_get_connection_list(&mut self, node: &HDANode) -> Vec<HDANodeAddr> {
		// get list length
		self.send_command(Self::node_command(0x00, node.addr, 0xF00, 0x0E));
		let len_field: u8 = (self.read_response() & 0xFF) as u8;
		
		// Highest bit is if addresses are represented in longer notation
		// lower 7 is actual count
			
		let count:u8 = len_field & 0x7F;
		let use_long_addr: bool = (len_field >> 7) & 0x1 == 1;
		
		let mut current: u8 = 0;
		
		let mut list = Vec::<HDANodeAddr>::new();

		while current < count {
			self.send_command(Self::node_command(0x00, node.addr, 0xF02, current as u32));
			let response: u32 = (self.read_response() & 0xFFFFFFFF) as u32;
			
			if use_long_addr {
				for i in 0..2 {
					let addr_field = ((response >> (16 * i)) & 0xFFFF) as u16;
					let addr = (addr_field & 0x7FFF);

					if addr == 0 { break; }
					
					if (addr_field >> 15) & 0x1 == 0x1 {
						for i in (list.pop().unwrap() .. (addr + 1)) {
							list.push(i);
						}
					} else {
						list.push(addr);
					}
				}

			} else {

				for i in 0..4 {
					let addr_field = ((response >> (8 * i)) & 0xff) as u16;
					let addr = (addr_field & 0x7F);
					
					if addr == 0 { break; }				

					if (addr_field >> 7) & 0x1 == 0x1 {
						for i in (list.pop().unwrap() .. (addr + 1)) {
							list.push(i);
						}
					} else {
						list.push(addr);
					}
				}

			}
			
			current = list.len() as u8;

		}
		list
		
	}
	pub fn enumerate(&mut self) {

		self.nodes.clear();
		self.output_pins.clear();
		self.input_pins.clear();

		
		let root = self.read_node(0);
		
		// print!("{}\n", root);

		let root_count = root.subnode_count;
		let root_start = root.subnode_start;
		self.nodes.push(root);
		

		//FIXME: So basically the way this is set up is to only support one codec and hopes the first one is an audio
		for i in 0..root_count { 
			let afg = self.read_node((root_start + i) as HDANodeAddr);
			// print!("{}\n", afg);
			let afg_count = afg.subnode_count;
			let afg_start = afg.subnode_start;
			self.nodes.push(afg);
			
			for j in 0..afg_count {
				let mut widget = self.read_node((afg_start + j) as HDANodeAddr);
				widget.is_widget = true;
				match widget.widget_type() {
					HDAWidgetType::AudioOutput => {self.output_pins.push(widget.addr)},
					HDAWidgetType::AudioInput  => {self.input_pins.push(widget.addr)},
					HDAWidgetType::BeepGenerator => {self.beep_addr = widget.addr },
					_ => {},
				}
				// print!("{}\n", widget);
				self.nodes.push(widget);
			}	
		}
	}

	pub fn get_node(&self, addr: HDANodeAddr) -> Option<&HDANode> {
		
		for ref node in &self.nodes {
			if node.addr == addr {
				return Some(node);
			}
			
		}
		None
	}
	pub fn find_shortest_path_to_speaker(&mut self) -> Vec<HDANodeAddr> {
		let mut path = Vec::<HDANodeAddr>::new();
		
		for addr in &self.output_pins {
			//let node = self.get_node().unwrap();
			

		}

		path
	}


	pub fn create_sound_buffers(&mut self) {

		self.buffs.push(Vec::<StreamBuffer>::new());
		// self.buffs[0].push(StreamBuffer::new(0x4000).unwrap());
		// self.buffs[0].push(StreamBuffer::new(0x4000).unwrap());


	}

	/*
	  Here we update the buffers and split them into 128 byte sub chunks
	  because each BufferDescriptorList needs to be 128 byte aligned,
	  this makes it so each of the streams can have up to 128/16 (8) buffer descriptors
	*/
	/*
	  Vec of a Vec was doing something weird and causing the driver to hang.
	  So now we have a set of variables instead.
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
		
		self.buff_desc[0].set_address(self.output_buff_phys);
		self.buff_desc[0].set_length((self.output_buff_length/2) as u32);
		self.buff_desc[0].set_interrupt_on_complete(true);

		self.buff_desc[1].set_address(self.output_buff_phys + self.output_buff_length/2);
		self.buff_desc[1].set_length((self.output_buff_length/2) as u32);
		self.buff_desc[1].set_interrupt_on_complete(true);

		

	}


	/*
	  For testing in VBOX:
	  Create Ramp wave of 400hz to test the output of 
	  the speakers to see sound can be played
	*/

	pub fn test_buff_fill(&mut self) {
		let n_samples = self.output_buff_length / (2 * 2);
		let buf_ptr = unsafe { self.output_buff as * mut u16};

		let freq:u16 = 440;
		
		let period:u16 = 44100 / 440;
		
		let step:u16 = 65535 / period;

		let mut j:u16 = 0;
		let mut val:u16 = 0;

		for i in 0..n_samples {
			unsafe {
				*buf_ptr.offset((2*i) as isize) = val;
				*buf_ptr.offset((2*i+1) as isize) = val;
			}
			val += step;
			j += 1;
			
			if j >= period {
				j = 0;
				val = 0;
			}

		}
		print!("IHDA: Test buffer created.\n");
	}
	

	pub fn vbox_speaker_test(&mut self) {

		


		// Pin enable
		self.send_command(Self::node_command(0x00, 0xC, 0x707, 0x40));
		let mut response: u32 = (self.read_response() & 0xFFFFFFFF) as u32;


		// EAPD enable
		self.send_command(Self::node_command(0x00, 0xC, 0x70C, 2));
		response = (self.read_response() & 0xFFFFFFFF) as u32;


		self.set_stream_channel(0x3, 1, 0);

		// self.create_sound_buffers();
		self.update_sound_buffers();


		print!("Supported Formats: {:08X}\n", self.get_supported_formats(0x1));
		print!("Capabilities: {:08X}\n", self.get_capabilities(0x3));

		let output = self.get_output_stream_descriptor(0).unwrap();
		
		output.set_address(self.buff_desc_phys);

		output.set_pcm_format(&super::SR_44_1, BitsPerSample::Bits16, 2);
		output.set_cyclic_buffer_length(0x8000); // number of samples
		output.set_stream_number(1);
		output.set_last_valid_index(1);
		output.set_interrupt_on_completion(true);


		self.set_power_state(0x3, 0); // Power state 0 is fully on
		self.set_converter_format(0x3, &super::SR_44_1, BitsPerSample::Bits16, 2);
		
		self.send_command(Self::node_command(0x00, 0x3, 0xA00, 0));
		response = (self.read_response() & 0xFFFFFFFF) as u32;
		
		print!("Format: {:04X}\n",response);

		// Unmute and set gain for pin complex and DAC
		self.set_amplifier_gain_mute(0x3, true, true, true, true, 0, false, 0x7f);
		self.set_amplifier_gain_mute(0xC, true, true, true, true, 0, false, 0x7f);


		

		// self.test_buff_fill();

		output.run();

		print!("IHDA: Beep? \n");
		self.beep(1);
		
	}


	// BEEP!!
	pub fn beep(&mut self, div:u8) {
		let addr = self.beep_addr;
		if addr != 0 {
			
			self.send_command(Self::node_command(0x00, addr, 0xF0A, div as u32));
			let response = (self.read_response() & 0xFFFFFFFF) as u32;
		}

	}
	
	pub fn read_beep(&mut self) -> u8 {
		let addr = self.beep_addr;
		if addr != 0 {
			self.send_command(Self::node_command(0x00, addr, 0x70A, 0));
			(self.read_response() & 0xFF) as u8
		}else{
			0
		}

	}

	pub fn enable_pin(&self, node: &HDANode) {


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


		for i in 0..15 {
			if (statests >> i) & 0x1 == 1 {
				self.codecs.push(i as HDANodeAddr);
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

	pub fn node_command(codec_address: u32, node_index: HDANodeAddr, command: u32, data: u32) -> u32{
		let mut ncmd: u32 = 0;
		let node_addr = node_index as u32;

		ncmd |= (codec_address & 0x00F) << 28;
		ncmd |= (node_addr     & 0x0FF) << 20;
		ncmd |= (command       & 0xFFF) <<  8;
		ncmd |= (data          & 0x0FF) <<  0;
		ncmd
	}

	
	pub fn corb_start(&mut self) {
		self.regs.corbctl.writef(CORBRUN,true);
	}

	pub fn rirb_start(&mut self) {
		self.regs.rirbctl.writef(RIRBDMAEN | RINTCTL,true);
	}

	pub fn corb_stop(&mut self) {
		
		while self.regs.corbctl.readf(CORBRUN) { self.regs.corbctl.write(0); }
	}

	pub fn rirb_stop(&mut self) {
		let mut val = self.regs.rirbctl.read();
		val &= !(RIRBDMAEN);
		self.regs.rirbctl.write(val);
	}

	

	//Intel 4.4.1.3
	
	pub fn init_corb_and_rirb(&mut self) -> Result<()> {


		self.corb_stop();
		self.rirb_stop();


		//Determine CORB and RIRB size and allocate buffer
			

		//3.3.24
		let corbsize_reg = self.regs.corbsize.read();
		let corbszcap = (corbsize_reg >> 4) & 0xF;

		let mut corbsize_bytes: usize = 0;
		let mut corbsize: u8 = 0;
		
		if (corbszcap & 4) == 4 {
			corbsize = 2;
			corbsize_bytes = 1024;

			self.corb_count = 256;
		} else if (corbszcap & 2) == 2 {
			corbsize = 1;
			corbsize_bytes = 64;

			self.corb_count = 16;
		} else if (corbszcap & 1) == 1 {
			corbsize = 0;
			corbsize_bytes = 8;

			self.corb_count = 2;
		} else {
			//TODO: Error!
		}
		
		//3.3.31

		let rirbsize_reg = self.regs.rirbsize.read();
		let rirbszcap = (rirbsize_reg >> 4) & 0xF;

		let mut rirbsize_bytes: usize = 0;
		let mut rirbsize: u8 = 0;
		
		if (rirbszcap & 4) == 4 {
			rirbsize = 2;
			rirbsize_bytes = 2048;

			self.rirb_count = 256;
		} else if (rirbszcap & 2) == 2 {
			rirbsize = 1;
			rirbsize_bytes = 128;

			self.rirb_count = 8;
		} else if (rirbszcap & 1) == 1 {
			rirbsize = 0;
			rirbsize_bytes = 16;

			self.rirb_count = 2;
		} else {
			//TODO: Error!
		}

		//print!("CORB size: {} RIRB size: {}\n", corbsize_bytes, rirbsize_bytes);
		
		
		// Allocate the physical memory, keeping in mind 
		// that the buffers need to be 128-byte aligned
		
		let buff_address = unsafe {
			syscall::physalloc(max(rirbsize_bytes, 128) + max(corbsize_bytes, 128))
				.expect("Could not allocate physical memory for CORB and RIRB.")
		};

		
		let virt_address = unsafe { syscall::physmap(buff_address, 0x1000, MAP_WRITE).expect("ihdad: failed to map address for CORB/RIRB buff") };
		

		self.corb_rirb_base_phys = buff_address;

		// Set the sizes and addresses of the buffers
		self.regs.corbsize.write(corbsize);

		self.corb_set_address(buff_address);
		
		// make sure that the RIRB buffer is at least aligned to 128 bytes.
		self.regs.rirbsize.write(rirbsize);
		self.rirb_set_address(buff_address + max(128, corbsize_bytes));
		

		// set virtual addresses for the buffer so we can access it

		self.corb_base = (virt_address) as *mut u32;
		self.rirb_base = (virt_address + max(128, corbsize_bytes)) as *mut u64;


		//print!("IHDA State: {:04X}\n",self.regs.statests.read());		
		self.regs.corbwp.write(0);
		self.corb_reset_read_pointer();
		self.rirb_reset_write_pointer();
		self.rirb_rp = 0;

		self.regs.rintcnt.write(1);


		self.corb_start();
		self.rirb_start();

		Ok(())
	}

	fn corb_set_address(&mut self, addr: usize) {

		

		self.regs.corblbase.write((addr & 0xFFFFFFFF) as u32);
		self.regs.corbubase.write((addr >> 32) as u32);
	}

	fn rirb_set_address(&mut self, addr: usize) {
		self.regs.rirblbase.write((addr & 0xFFFFFFFF) as u32);
		self.regs.rirbubase.write((addr >> 32) as u32);
	}

	fn rirb_reset_write_pointer(&mut self) {
		self.regs.rirbwp.writef(RIRBWPRST, true);


	}

	fn corb_reset_read_pointer(&mut self){
		

		/*
		 * FIRST ISSUE/PATCH
		 * This will loop forever in virtualbox 
		 * So maybe just resetting the read pointer
		 * and leaving for the specific model?
		 */
		if true {
			self.regs.corbrp.writef(CORBRPRST, true);
		
		}
		else
		{
			// 3.3.21	
	
			self.corb_stop();	
			// Set CORBRPRST to 1
			print!("CORBRP {:X}\n",self.regs.corbrp.read());
			self.regs.corbrp.writef(CORBRPRST, true);
			print!("CORBRP {:X}\n",self.regs.corbrp.read());
			print!("Here!\n");
	
			// Wait for it to become 1
			while ! self.regs.corbrp.readf(CORBRPRST) {
				self.regs.corbrp.writef(CORBRPRST, true);
			}
			print!("Here!!\n");
			// Clear the bit again
			self.regs.corbrp.write(0);
		
			// Read back the bit until zero to verify that it is cleared.
	
			loop {
	
				if !self.regs.corbrp.readf(CORBRPRST) {
					break;
				}
				self.regs.corbrp.write(0);
			}
		}			
	}


	fn send_command(&mut self, cmd: u32) {
		
		// wait for the commands to finish
		while (self.regs.corbwp.read() & 0xff) != (self.regs.corbrp.read() & 0xff) {}

		let mut write_pos: usize = ( (self.regs.corbwp.read() as usize & 0xFF) + 1) % self.corb_count;
		
		unsafe {
			*self.corb_base.offset(write_pos as isize) = cmd;
		}
	
		self.regs.corbwp.write(write_pos as u16);
		 

		
	}

	fn read_response(&mut self) -> u64 {

		// wait for response
		while (self.regs.rirbwp.read() & 0xff) == (self.rirb_rp & 0xff) {}

		let mut read_pos: u16 = (self.rirb_rp + 1) % self.rirb_count as u16;
		
		let mut res: u64;
		unsafe {
			res = *self.rirb_base.offset(read_pos as isize);
		}
		
		self.rirb_rp = read_pos;

		res

	}


	// FIXME: Apparently vbox is picky about sending immediate commands. 
	// Hopefully this can be disregarded if the DMA works.



	fn send_immediate_command(&mut self, cmd: u32) -> u64 {
		print!("Status: {:04X}\n",self.regs.ics.read());
		
		// wait for ready
		while self.regs.ics.readf(ICB) {}

		// write command
		self.regs.icoi.write(cmd);
		

		// set ICB bit to send command
		self.regs.ics.writef(ICB, true);

		print!("Status: {:04X}\n",self.regs.ics.read());

		// wait for IRV bit to be set to indicate a response is latched
		while !self.regs.ics.readf(IRV) {}

		// read the result register twice, total of 8 bytes
		// highest 4 will most likely be zeros (so I've heard)
		let mut res:u64 = self.regs.irii.read() as u64;
		res |= (self.regs.irii.read() as u64) << 32;


		// clear the bit so we know when the next response comes
		self.regs.ics.writef(IRV, false);

		res

	}

	fn get_input_descriptor(&self, index: usize) -> Option<&'static mut StreamDescriptorRegs> {
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


	fn set_stream_channel(&mut self, addr: HDANodeAddr, stream: u8, channel:u8) {
		let val = ((stream & 0xF) << 4) | (channel & 0xF);
		self.send_command(Self::node_command(0x00, addr, 0x706, val as u32));
		let temp = self.read_response();
	}

	fn set_power_state(&mut self, addr:HDANodeAddr, state:u8) {
		self.send_command(Self::node_command(0x00, addr, 0x705, state as u32 & 0xF));
		let temp = self.read_response();
	}

	fn get_supported_formats(&mut self, addr: HDANodeAddr) -> u32 {
		self.send_command(Self::node_command(0x00, addr, 0xF00, 0x0A));
		self.read_response() as u32
	}

	fn get_capabilities(&mut self, addr: HDANodeAddr) -> u32 {
		self.send_command(Self::node_command(0x00, addr, 0xF00, 0x09));
		self.read_response() as u32
	}

	fn set_converter_format(&mut self, addr:HDANodeAddr, sr: &super::SampleRate, bps: BitsPerSample, channels:u8) {
		let fmt = super::format_to_u16(sr, bps, channels);
		let fmt_hi = (fmt >> 8) as u32;
		let fmt_lo = (fmt & 0xFF) as u32;
		print!("Format: {:04X}\n",fmt);
		self.send_command(Self::node_command(0x00, addr, 0x200 | fmt_hi, fmt_lo));
		let temp = self.read_response();

	}

	fn set_amplifier_gain_mute(&mut self, addr:HDANodeAddr, output:bool, input:bool, left:bool, right:bool, index:u8, mute:bool, gain: u8) {

		let mut payload: u16 = 0;
		
		if output { payload |= (1 << 15); }
		if input  { payload |= (1 << 14); }
		if left   { payload |= (1 << 13); }
		if right  { payload |= (1 << 12); }
		if mute   { payload |= (1 <<  7); }
		payload |= ((index as u16) & 0x0F) << 8;
		payload |= ((gain  as u16) & 0x7F);
		
		let payload_hi = (payload >> 8);
		let payload_lo = (payload & 0xFF) as u8;

		self.send_command(Self::node_command(0x00, addr, 0x300 | payload_hi as u32, payload_lo as u32));
		let temp = self.read_response();
	}

	fn write_to_output(&mut self, out_index: usize, buf: &[u8]) -> Result<usize> {
		
		
		// TODO: Better way of writing than just writing from the write pointer to the link position in buffer

		
		

		let mut output = self.get_output_stream_descriptor(0).unwrap();

		let sample_size:usize = output.sample_size();
		let sample_count:usize = buf.len() / sample_size;
		let buff_len = output.cyclic_buffer_length() as usize;

		let mut samples_copied: usize = 0;
		while samples_copied < sample_count {

			let samples_left = sample_count - samples_copied;

			// modular arithmetic to get the number of samples that we can write to
			
			let mut can_write = (output.link_position() as usize + buff_len) - self.output_buff_wp;

			if can_write >= buff_len {
				can_write -= buff_len;
			}
			
			let samples_to_write = min(can_write, samples_left);
			let samples_until_end = buff_len - self.output_buff_wp;
			
			if samples_to_write > 0 {

				if samples_until_end >= samples_to_write {
					unsafe {
						copy_nonoverlapping(buf.as_ptr(), (self.output_buff + self.output_buff_wp * sample_size) as * mut u8, samples_to_write * sample_size);
					}

				} else {
					unsafe {
						copy_nonoverlapping(buf.as_ptr(), (self.output_buff + self.output_buff_wp * sample_size) as * mut u8, samples_until_end * sample_size);
						copy_nonoverlapping((buf.as_ptr() as usize + (samples_to_write * sample_size)) as * const u8, 
							(self.output_buff + self.output_buff_wp * sample_size) as * mut u8, 
							(samples_to_write - samples_until_end) * sample_size);
					}
				}
					
				self.output_buff_wp += samples_to_write;
	
				if self.output_buff_wp >= buff_len {
					self.output_buff_wp -= buff_len;
				}
				
				samples_copied += samples_to_write;
			}
			thread::yield_now();
		}
				
		Ok(samples_copied * sample_size)

	}



	pub fn write_to_output2(&mut self, index:u8, buf: &[u8]) -> Result<usize> {
		
		
		let mut output = self.get_output_stream_descriptor(0).unwrap();
		let sample_size:usize = output.sample_size();
		let mut open_block = (output.link_position() as usize) / 0x4000;
				
		if open_block == 0 {
			open_block = 1;
		} else {
			open_block = open_block - 1;
		}


		while open_block == self.output_current_block {

			open_block = (output.link_position() as usize) / 0x4000;
				
			if open_block == 0 {
				open_block = 1;
			} else {
				open_block = open_block - 1;
			}

			thread::yield_now();
		}
		
		self.output_current_block = open_block;
		let len = min(0x4000, buf.len());

		unsafe {
			copy_nonoverlapping(buf.as_ptr(), (self.output_buff + self.output_current_block * 0x4000) as * mut u8, len);
		}
		
		Ok(len)
	}

	pub fn handle_interrupts(&mut self) {

		let intsts = self.regs.intsts.read();
		let sis = intsts & 0x3FFFFFFF;      
		print!("IHDA INTSTS: {:08X}\n", intsts);     
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
		

		let sample_size = 4; // TODO: create method to get sample size

		for i in 0..iss {
			if ((sis >> i) & 1 ) == 1 {


			}
		}

		for i in 0..oss {
			if ((sis >> (i + iss)) & 1 ) == 1 {
				
				let mut output = self.get_output_stream_descriptor(i).unwrap();	
				// TODO: No magic numbers!
				let mut temp = output.link_position() as usize / 0x4000;
							
				if temp == 0 {
					temp = self.output_buff_length  - 0x4000;
				} else {
					temp = temp - 0x4000;
				}
				self.output_current_block = temp;
				output.clear_interrupts();
			}
		}

		for i in 0..bss {
			if ((sis >> (i + iss + oss)) & 1 ) == 1 {
				

			}
		}
	}
	
}


impl Drop for IntelHDA {
	fn drop(&mut self) {
		let _ = unsafe {syscall::physfree(self.buff_desc_phys, 0x1000)};
		if self.output_buff_phys != 0 { 
			unsafe {
				let _ = syscall::physfree(self.output_buff_phys, self.output_buff_length);
			}
		}
		print!("IHDA: Deallocating IHDA driver.\n");

	}


}


impl SchemeMut for IntelHDA {
	fn open(&mut self, _path: &[u8], flags: usize, uid: u32, _gid: u32) -> Result<usize> {
		
		// TODO:
	
		if uid == 0 {
			Ok(flags)
		} else {
			Err(Error::new(EACCES))
		}
	}

	fn write(&mut self, _id: usize, buf: &[u8]) -> Result<usize> {
	
		//print!("Int count: {}\n", self.int_counter);
		

		self.write_to_output2(0, buf)
	}

	fn close(&mut self, _id: usize) -> Result<usize> {
		// TODO:
        	Ok(0)
    	}

}
