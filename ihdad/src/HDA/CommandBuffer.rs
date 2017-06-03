use std::{mem, thread, ptr, fmt};
use syscall::io::{Dma, Mmio, Io, ReadOnly};


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


const CORB_OFFSET: usize = 0x00;
const RIRB_OFFSET: usize = 0x10;


const CORB_BUFF_MAX_SIZE: usize = 1024;

struct CommandBufferRegs {
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
}


struct CorbRegs {
	corblbase:  Mmio<u32>,
	corbubase:  Mmio<u32>,
	corbwp:     Mmio<u16>,
	corbrp:     Mmio<u16>,
	corbctl:    Mmio<u8>,
	corbsts:    Mmio<u8>,
	corbsize:   Mmio<u8>,
	rsvd5:      Mmio<u8>,
}

struct Corb {
	regs: &'static mut CorbRegs,
	corb_base: *mut u32,
	corb_base_phys: usize,
}

impl Corb {

	pub fn new(regs_addr:usize, corb_buff_phys:usize, corb_buff_virt:usize) -> Corb {
		

		Corb {
			regs: &mut *(regs_addr as *mut CorbRegs);
			corb_base: (corb_buff_virt) as *mut u64,
			
		}
	}

}

struct RirbRegs {
	rirblbase:  Mmio<u32>,
	rirbubase:  Mmio<u32>,
	rirbwp:     Mmio<u16>,
	rintcnt:    Mmio<u16>,
	rirbctl:    Mmio<u8>,
	rirbsts:    Mmio<u8>,
	rirbsize:   Mmio<u8>,
	rsvd6:      Mmio<u8>,
}

struct Rirb {
	regs: &'static mut RirbRegs,
	rirb_base: *mut u64,
	rirb_base_phys: usize,
	rirb_rp: usize,
}

impl Rirb {

	pub fn new(regs_addr:usize, rirb_buff_phys:usize, rirb_buff_virt:usize) -> Rirb {
		

		Rirb {
			regs:           &mut *(regs_addr as *mut RirbRegs);
			rirb_base:      (rirb_buff_virt) as *mut u64,
			rirb_rp:        0,
			rirb_base_phys: rirb_buff_phys, 
		}
	}

}


struct CommandBuffer {

	// regs: &'static mut CommandBufferRegs,

	corb: Corb,
	rirb: Rirb,

	
	corb_rirb_base_phys: usize,



}

impl CommandBuffer {
	pub fn new(regs_addr:usize, cmd_buff_frame_phys:usize, cmd_buff_frame:usize ) -> CommandBuffer {
		
		let corb = Corb::new(regs_addr + CORB_OFFSET, cmd_buff_frame_phys, cmd_buff_frame);
		let rirb = Rirb::new(regs_addr + RIRB_OFFSET,
					 cmd_buff_frame_phys + CORB_BUFF_MAX_SIZE, 
					 cmd_buff_frame + CORB_BUFF_MAX_SIZE);

		CommandBuffer {
			corb: corb,
			rirb: rirb,
			corb_rirb_base_phys: cmd_buff_frame_phys,
		}
	}

}

