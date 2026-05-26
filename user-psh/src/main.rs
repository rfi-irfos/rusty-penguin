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

fn sys_clear() {
    unsafe {
        core::arch::asm!(
            "syscall",
            inout("rax") 2u64 => _,
            in("rdi") 0u64,
            out("rcx") _,
            out("r11") _,
            options(nostack),
        );
    }
}

fn sys_reboot() -> ! {
    unsafe {
        core::arch::asm!(
            "syscall",
            in("rax") 3u64,
            in("rdi") 0u64,
            options(noreturn, nostack),
        );
    }
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
    sys_clear();
    write(b"Rusty Penguin v1.0.0\n");
    write(b"Binary hardware. Ternary mind.\n");
    write(b"---\n");
    write(b"psh 1.0  type 'help' for commands\n\n");

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
            write(b"commands:\n");
            write(b"  echo <text>   print text\n");
            write(b"  uname         kernel info\n");
            write(b"  whoami        current context\n");
            write(b"  clear         clear screen\n");
            write(b"  reboot        reboot machine\n");
            write(b"  exit          exit shell\n");
        } else if line == b"uname" {
            write(b"Rusty Penguin 1.0.0 x86_64 ternary-kernel\n");
            write(b"Binary hardware. Ternary mind. No Linux.\n");
        } else if line == b"uname -a" {
            write(b"RustyPenguin 1.0.0 psh x86_64 GNU/Trit\n");
        } else if line == b"whoami" {
            write(b"ring3\n");
        } else if line == b"version" {
            write(b"Rusty Penguin v1.0.0 -- Binary hardware. Ternary mind.\n");
        } else if line == b"clear" {
            sys_clear();
        } else if line == b"reboot" {
            write(b"rebooting...\n");
            sys_reboot();
        } else if line == b"echo" {
            write(b"\n");
        } else if line.starts_with(b"echo ") {
            write(&line[5..]);
            write(b"\n");
        } else if line.is_empty() {
            // nothing
        } else {
            write(b"psh: command not found: '");
            write(line);
            write(b"'\n");
        }
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
