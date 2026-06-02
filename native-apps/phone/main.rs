// RustyPhone: The native E.T. Phone Home App
// Bridges dial-out to the UDI cellular HAL.

fn main() {
    let phone_number = b"123-456-7890";
    unsafe {
        core::arch::asm!(
            "syscall",
            in("rax") 0x4e, // sys_et_phone_home
            in("rdi") phone_number.as_ptr(),
            in("rsi") phone_number.len() as u64,
            out("rax") _,
        );
    }
}
