// This code is informed by the QEMU implementation found here:
// https://github.com/qemu/qemu/blob/master/hw/input/vmmouse.c
//
// As well as the Linux implementation here:
// http://elixir.free-electrons.com/linux/v4.1/source/drivers/input/mouse/vmmouse.c

use core::arch::global_asm;

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

#[cfg(target_arch = "x86_64")]
global_asm!("
    .globl cmd_inner
cmd_inner:
    mov r8, rdi

    // 2nd argument `cmd` as per sysv64.
    mov ecx, esi
    // 3rd argument `arg` as per sysv64.
    mov ebx, edx

    mov eax, {MAGIC}
    mov dx, {PORT}

    in eax, dx

    xchg rdi, r8

    mov DWORD PTR [rdi + 0x00], eax
    mov DWORD PTR [rdi + 0x04], ebx
    mov DWORD PTR [rdi + 0x08], ecx
    mov DWORD PTR [rdi + 0x0C], edx
    mov DWORD PTR [rdi + 0x10], esi
    mov DWORD PTR [rdi + 0x14], r8d

    ret
",
    MAGIC = const MAGIC,
    PORT = const PORT,
);

#[cfg(target_arch = "x86_64")]
pub unsafe fn cmd(cmd: u32, arg: u32) -> (u32, u32, u32, u32, u32, u32) {
    extern "sysv64" {
        fn cmd_inner(array_ptr: *mut u32, cmd: u32, arg: u32);
    }

    let mut array = [0_u32; 6];

    cmd_inner(array.as_mut_ptr(), cmd, arg);

    let [a, b, c, d, e, f] = array;
    (a, b, c, d, e, f)
}

//TODO: is it possible to enable this on non-x86_64?
#[cfg(not(target_arch = "x86_64"))]
pub unsafe fn cmd(_cmd: u32, _arg: u32) -> (u32, u32, u32, u32, u32, u32) {
    (0, 0, 0, 0, 0, 0)
}

#[cfg(target_arch = "x86_64")]
pub fn enable(relative: bool) -> bool {
    eprintln!("ps2d: Enable vmmouse");

    unsafe {
        let (eax, ebx, _, _, _, _) = cmd(GETVERSION, 0);
        if ebx != MAGIC || eax == 0xFFFFFFFF {
            eprintln!("ps2d: No vmmouse support");
            return false;
        }

        let _ = cmd(ABSPOINTER_COMMAND, CMD_ENABLE);

        let (status, _, _, _, _, _) = cmd(ABSPOINTER_STATUS, 0);
    	if (status & 0x0000ffff) == 0 {
        	eprintln!("ps2d: No vmmouse");
    		return false;
    	}

        let (version, _, _, _, _, _) = cmd(ABSPOINTER_DATA, 1);
        if version != VERSION {
            eprintln!("ps2d: Invalid vmmouse version: {} instead of {}", version, VERSION);
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
