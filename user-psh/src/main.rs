#![no_std]
#![no_main]

fn sys_write(buf: &[u8]) {
    unsafe {
        core::arch::asm!(
            "syscall",
            inout("rax") 1u64 => _,
            in("rdi") 1u64,
            in("rsi") buf.as_ptr(),
            in("rdx") buf.len(),
            out("rcx") _,
            out("r11") _,
            options(nostack),
        );
    }
}

fn sys_read(buf: &mut [u8]) -> usize {
    let n: u64;
    unsafe {
        core::arch::asm!(
            "syscall",
            inout("rax") 0u64 => n,
            in("rdi") 0u64,
            in("rsi") buf.as_mut_ptr(),
            in("rdx") buf.len(),
            out("rcx") _,
            out("r11") _,
            options(nostack),
        );
    }
    n as usize
}

fn sys_exit(code: u64) -> ! {
    unsafe {
        core::arch::asm!(
            "syscall",
            in("rax") 60u64,
            in("rdi") code,
            options(noreturn, nostack),
        );
    }
}

fn write(s: &[u8]) {
    let mut off = 0;
    while off < s.len() {
        let chunk = (s.len() - off).min(256);
        sys_write(&s[off..off + chunk]);
        off += chunk;
    }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    write(b"psh 1.0 -- Rusty Penguin Shell\n");
    write(b"type 'help' for commands\n\n");

    let mut buf = [0u8; 256];

    loop {
        write(b"> ");
        let n = sys_read(&mut buf);
        if n == 0 { continue; }

        let line = if buf[n - 1] == b'\n' { &buf[..n - 1] } else { &buf[..n] };

        if line == b"exit" || line == b"quit" {
            write(b"bye\n");
            sys_exit(0);
        } else if line == b"help" {
            write(b"commands: echo <text>, help, version, exit\n");
        } else if line == b"version" {
            write(b"Rusty Penguin v1.0.0 -- Binary hardware. Ternary mind.\n");
        } else if line.starts_with(b"echo ") {
            write(&line[5..]);
            write(b"\n");
        } else if line.is_empty() {
            // nothing
        } else {
            write(b"unknown: '");
            write(line);
            write(b"'\n");
        }
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
