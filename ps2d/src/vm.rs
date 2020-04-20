// This code is informed by the QEMU implementation found here:
// https://github.com/qemu/qemu/blob/master/hw/input/vmmouse.c
//
// As well as the Linux implementation here:
// http://elixir.free-electrons.com/linux/v4.1/source/drivers/input/mouse/vmmouse.c

const MAGIC: u32 = 0x564D5868;
const PORT: u16 = 0x5658;

pub const ABSPOINTER_DATA: u32 = 39;
pub const ABSPOINTER_STATUS: u32 = 40;
pub const ABSPOINTER_COMMAND: u32 = 41;

pub const CMD_ENABLE: u32 = 0x45414552;
pub const CMD_DISABLE: u32 = 0x000000f5;
pub const CMD_REQUEST_ABSOLUTE: u32 = 0x53424152;
pub const CMD_REQUEST_RELATIVE: u32 = 0x4c455252;

const VERSION: u32 = 0x3442554a;

pub const RELATIVE_PACKET: u32 = 0x00010000;

pub const LEFT_BUTTON: u32 = 0x20;
pub const RIGHT_BUTTON: u32 = 0x10;
pub const MIDDLE_BUTTON: u32 = 0x08;

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
pub unsafe fn cmd(cmd: u32, arg: u32) -> (u32, u32, u32, u32, u32, u32) {
    let a: u32;
    let b: u32;
    let c: u32;
    let d: u32;
    let si: u32;
    let di: u32;

    asm!(
        "in eax, dx"
        :
        "={eax}"(a),
        "={ebx}"(b),
        "={ecx}"(c),
        "={edx}"(d),
        "={esi}"(si),
        "={edi}"(di)
        :
        "{eax}"(MAGIC),
        "{ebx}"(arg),
        "{ecx}"(cmd),
        "{dx}"(PORT)
        :
        "memory"
        :
        "intel", "volatile"
    );

    (a, b, c, d, si, di)
}

pub fn enable(relative: bool) -> bool {
    println!("ps2d: Enable vmmouse");

    unsafe {
        let _ = cmd(ABSPOINTER_COMMAND, CMD_ENABLE);

        let (status, _, _, _, _, _) = cmd(ABSPOINTER_STATUS, 0);
    	if (status & 0x0000ffff) == 0 {
        	println!("ps2d: No vmmouse");
    		return false;
    	}

        let (version, _, _, _, _, _) = cmd(ABSPOINTER_DATA, 1);
        if version != VERSION {
            println!("ps2d: Invalid vmmouse version: {} instead of {}", version, VERSION);
            let _ = cmd(ABSPOINTER_COMMAND, CMD_DISABLE);
            return false;
        }

        if relative {
        	cmd(ABSPOINTER_COMMAND, CMD_REQUEST_RELATIVE);
        } else {
        	cmd(ABSPOINTER_COMMAND, CMD_REQUEST_ABSOLUTE);
        }
    }

    return true;
}
