#![no_std]
#![feature(naked_functions)]

use core::panic::PanicInfo;

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {}
}

#[no_mangle]
pub unsafe extern "system" fn NtTerminateProcess(
    _process_handle: *mut core::ffi::c_void,
    exit_status: i32,
) -> i32 {
    let nr: u64 = 0x2c;
    let status: u64;
    
    core::arch::asm!(
        "syscall",
        in("rax") nr,
        in("r10") _process_handle as u64,
        in("rdx") exit_status as u64,
        lateout("rax") status,
        out("rcx") _,
        out("r11") _,
    );
    
    status as i32
}

#[no_mangle]
pub unsafe extern "system" fn DllMain(
    _h_inst_dll: *mut core::ffi::c_void,
    _fdw_reason: u32,
    _lpv_reserved: *mut core::ffi::c_void,
) -> i32 {
    1 // TRUE
}
