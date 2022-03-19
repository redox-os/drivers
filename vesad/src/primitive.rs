use core::arch::asm;

#[cfg(target_arch = "x86_64")]
#[inline(always)]
pub unsafe fn fast_copy(dst: *mut u8, src: *const u8, len: usize) {
    // direction flag must always be cleared, as per the System V ABI
    asm!("rep movsb",
        inout("rdi") dst as usize => _, inout("rsi") src as usize => _, inout("rcx") len => _,
        options(nostack, preserves_flags),
    );
}

#[cfg(target_arch = "x86_64")]
#[inline(always)]
pub unsafe fn fast_copy64(dst: *mut u64, src: *const u64, len: usize) {
    asm!("rep movsq",
        inout("rdi") dst as usize => _, inout("rsi") src as usize => _, inout("rcx") len => _,
        options(nostack, preserves_flags),
    );
}

#[cfg(target_arch = "x86_64")]
#[inline(always)]
pub unsafe fn fast_set32(dst: *mut u32, src: u32, len: usize) {
    asm!("rep stosd",
        inout("rdi") dst as usize => _, in("eax") src, inout("rcx") len => _,
        options(nostack, preserves_flags),
    );
}

#[cfg(target_arch = "x86_64")]
#[inline(always)]
pub unsafe fn fast_set64(dst: *mut u64, src: u64, len: usize) {
    asm!("rep stosq",
        inout("rdi") dst as usize => _, in("rax") src, inout("rcx") len => _,
        options(nostack, preserves_flags),
    );
}
