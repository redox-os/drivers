// This code is informed by the QEMU implementation found here:
// https://github.com/qemu/qemu/blob/master/hw/input/vmmouse.c
//
// As well as the Linux implementation here:
// http://elixir.free-electrons.com/linux/v4.1/source/drivers/input/mouse/vmmouse.c

use core::arch::asm;

use log::{error, info, trace};

const MAGIC: u32 = 0x564D5868;
const PORT: u16 = 0x5658;

pub const GETVERSION: u32 = 10;
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

pub unsafe fn cmd(cmd: u32, arg: u32) -> (u32, u32, u32, u32) {
    let a: u32;
    let b: u32;
    let c: u32;
    let d: u32;

    // ebx can't be used as input or output constraint in rust as LLVM reserves it.
    // Use xchg to pass it through r9 instead while restoring the original value in
    // rbx when leaving the inline asm block. si and di are clobbered too.
    #[cfg(not(target_arch = "x86"))]
    asm!(
        "xchg r9, rbx; in eax, dx; xchg r9, rbx",
        inout("eax") MAGIC => a,
        inout("r9") arg => b,
        inout("ecx") cmd => c,
        inout("edx") PORT as u32 => d,
        out("rsi") _,
        out("rdi") _,
    );

    // On x86 we don't have a spare register, so push ebx to the stack instead.
    #[cfg(target_arch = "x86")]
    asm!(
        "push ebx; mov ebx, edi; in eax, dx; mov edi, ebx; pop ebx",
        inout("eax") MAGIC => a,
        inout("edi") arg => b,
        inout("ecx") cmd => c,
        inout("edx") PORT as u32 => d,
    );

    (a, b, c, d)
}

pub fn enable(relative: bool) -> bool {
    trace!("ps2d: Enable vmmouse");

    unsafe {
        let (eax, ebx, _, _) = cmd(GETVERSION, 0);
        if ebx != MAGIC || eax == 0xFFFFFFFF {
            info!("ps2d: No vmmouse support");
            return false;
        }

        let _ = cmd(ABSPOINTER_COMMAND, CMD_ENABLE);

        let (status, _, _, _) = cmd(ABSPOINTER_STATUS, 0);
        if (status & 0x0000ffff) == 0 {
            info!("ps2d: No vmmouse");
            return false;
        }

        let (version, _, _, _) = cmd(ABSPOINTER_DATA, 1);
        if version != VERSION {
            error!(
                "ps2d: Invalid vmmouse version: {} instead of {}",
                version, VERSION
            );
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
