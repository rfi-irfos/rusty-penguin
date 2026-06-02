// PSH (Penguin Shell) ls implementation.
fn main() {
    // Native syscall: enumerate directory items via VFS
    // For now, listing root files.
    println!("README.md  kernel/  drivers/  docs/  native-apps/");
}
fn println(s: &str) {
    unsafe {
        core::arch::asm!(
            "syscall",
            in("rax") 0x01, // sys_write
            in("rdi") 1,    // stdout
            in("rsi") s.as_ptr(),
            in("rdx") s.len() as u64,
            out("rax") _,
        );
    }
}
