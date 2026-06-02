//! Windows Driver Model (WDM) Memory and Synchronization support.

#[repr(C)]
pub struct Mdl {
    pub next: *mut Mdl,
    pub size: i16,
    pub mdl_flags: i16,
    pub process: *mut core::ffi::c_void,
    pub mapped_system_va: *mut core::ffi::c_void,
    pub start_va: *mut core::ffi::c_void,
    pub byte_count: u32,
    pub byte_offset: u32,
}

pub fn mm_probe_and_lock_pages(mdl: *mut Mdl) {
    crate::serial::write_str("  [wdm] MmProbeAndLockPages\n");
}

pub fn ke_acquire_spin_lock(lock: *mut u64) -> u64 {
    // Basic spinlock implementation using atomic exchange
    let mut old_irql = 0;
    unsafe {
        core::arch::asm!(
            "lock xchg [{0}], {1}",
            in(reg) lock,
            inout(reg) 1 => old_irql,
            options(nostack)
        );
    }
    old_irql
}
