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

fn sys_ticks() -> u64 {
    let n: u64;
    unsafe {
        core::arch::asm!(
            "syscall",
            inout("rax") 4u64 => n,
            in("rdi") 0u64,
            out("rcx") _,
            out("r11") _,
            options(nostack),
        );
    }
    n
}

fn sys_meminfo() -> (u32, u32) {
    let n: u64;
    unsafe {
        core::arch::asm!(
            "syscall",
            inout("rax") 5u64 => n,
            in("rdi") 0u64,
            out("rcx") _,
            out("r11") _,
            options(nostack),
        );
    }
    ((n >> 32) as u32, (n & 0xFFFF_FFFF) as u32)
}

fn sys_getpid() -> u64 {
    let n: u64;
    unsafe {
        core::arch::asm!(
            "syscall",
            inout("rax") 39u64 => n,
            in("rdi") 0u64,
            out("rcx") _,
            out("r11") _,
            options(nostack),
        );
    }
    n
}

fn sys_ps(buf: *mut u8, max: usize) -> usize {
    let n: u64;
    unsafe {
        core::arch::asm!(
            "syscall",
            inout("rax") 9u64 => n,
            in("rdi") buf,
            in("rsi") max,
            in("rdx") 0u64,
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

// ── Integer formatting ────────────────────────────────────────────────────────

fn write_u64(mut n: u64) {
    if n == 0 { sys_write(b"0"); return; }
    let mut buf = [0u8; 20];
    let mut i = 0;
    while n > 0 {
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
        i += 1;
    }
    let mut out = [0u8; 20];
    for j in 0..i { out[j] = buf[i - 1 - j]; }
    sys_write(&out[..i]);
}

fn write_i64(n: i64) {
    if n < 0 { sys_write(b"-"); write_u64((-n) as u64); }
    else { write_u64(n as u64); }
}

// ── Balanced ternary display ─────────────────────────────────────────────────
// Each digit is +, 0, or - (i.e. +1, 0, -1 times the place value 3^k).

fn write_ternary(mut n: i64) {
    if n == 0 { sys_write(b"0"); return; }
    let flip = n < 0;
    if n < 0 { n = -n; }
    let mut digits = [0i8; 40];
    let mut len = 0;
    let mut v = n;
    while v != 0 {
        let rem = (v % 3) as i8;
        v /= 3;
        if rem == 2 { digits[len] = -1; v += 1; }
        else { digits[len] = rem; }
        len += 1;
    }
    let mut buf = [0u8; 40];
    for k in 0..len {
        let d = if flip { -digits[len - 1 - k] } else { digits[len - 1 - k] };
        buf[k] = match d { 1 => b'+', -1 => b'-', _ => b'0' };
    }
    sys_write(&buf[..len]);
}

// ── Integer parser ────────────────────────────────────────────────────────────

fn parse_i64(s: &[u8]) -> Option<i64> {
    if s.is_empty() { return None; }
    let (neg, digits) = if s[0] == b'-' { (true, &s[1..]) } else { (false, s) };
    if digits.is_empty() { return None; }
    let mut n: i64 = 0;
    for &b in digits {
        if b < b'0' || b > b'9' { return None; }
        n = n.wrapping_mul(10).wrapping_add((b - b'0') as i64);
    }
    Some(if neg { -n } else { n })
}

// Split a byte slice on spaces, returning up to 4 tokens.
fn split_args(line: &[u8]) -> ([&[u8]; 4], usize) {
    let mut parts: [&[u8]; 4] = [b""; 4];
    let mut count = 0;
    let mut start = 0;
    let mut in_word = false;
    for i in 0..=line.len() {
        let at_space = i == line.len() || line[i] == b' ';
        if at_space && in_word {
            if count < 4 { parts[count] = &line[start..i]; count += 1; }
            in_word = false;
        } else if !at_space && !in_word {
            start = i;
            in_word = true;
        }
    }
    (parts, count)
}

// ── trit command ─────────────────────────────────────────────────────────────

fn cmd_trit(args: &[u8]) {
    let (parts, count) = split_args(args);

    let usage = b"usage: trit add|sub|mul|neg|cns <a> [b]\n";

    if count == 0 { write(usage); return; }
    let op = parts[0];

    if op == b"neg" {
        if count < 2 { write(usage); return; }
        let a = match parse_i64(parts[1]) { Some(v) => v, None => { write(b"bad number\n"); return; } };
        let r = -a;
        write_i64(r); sys_write(b"  ("); write_ternary(r); sys_write(b")\n");
        return;
    }

    if count < 3 { write(usage); return; }
    let a = match parse_i64(parts[1]) { Some(v) => v, None => { write(b"bad number\n"); return; } };
    let b = match parse_i64(parts[2]) { Some(v) => v, None => { write(b"bad number\n"); return; } };

    let r: i64 = if op == b"add" { a + b }
        else if op == b"sub" { a - b }
        else if op == b"mul" { a * b }
        else if op == b"cns" {
            // consensus: 0 if signs differ, keep sign if same
            if (a > 0 && b > 0) { 1 } else if (a < 0 && b < 0) { -1 } else { 0 }
        }
        else { write(usage); return; };

    write_i64(r); sys_write(b"  ("); write_ternary(r); sys_write(b")\n");
}

// ── ps command ───────────────────────────────────────────────────────────────
// Record: [u64 pid][u8 state][7 pad][16 name]  (32 bytes each)

fn cmd_ps() {
    let mut buf = [0u8; 32 * 16];
    let count = sys_ps(buf.as_mut_ptr(), 16);
    write(b"PID  ST  NAME\n");
    for i in 0..count {
        let off = i * 32;
        let mut pid: u64 = 0;
        for j in 0..8usize { pid |= (buf[off + j] as u64) << (j * 8); }
        let state_ch = match buf[off + 8] { 1 => b'+', 2 => b'-', _ => b'0' };
        let name_slice = &buf[off + 16..off + 32];
        let nlen = name_slice.iter().position(|&b| b == 0).unwrap_or(16);
        write_u64(pid);
        write(b"    ");
        sys_write(&[state_ch]);
        write(b"   ");
        write(&name_slice[..nlen]);
        write(b"\n");
    }
}

// ── Shell ─────────────────────────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn _start() -> ! {
    sys_clear();
    write(b"Rusty Penguin v1.0.0\n");
    write(b"Binary hardware. Ternary mind. No Linux.\n");
    write(b"---\n");
    write(b"psh 1.0  type 'help' for commands\n\n");

    let mut buf = [0u8; 256];

    loop {
        write(b"> ");
        let n = sys_read(&mut buf);
        if n == 0 { continue; }

        let raw = &buf[..n];
        let mut end = n;
        if end > 0 && raw[end - 1] == b'\n' { end -= 1; }
        if end > 0 && raw[end - 1] == b'\r' { end -= 1; }
        let line = &raw[..end];

        if line == b"exit" || line == b"quit" {
            write(b"bye\n");
            sys_exit(0);
        } else if line == b"help" {
            write(b"commands:\n");
            write(b"  echo <text>       print text\n");
            write(b"  uname             kernel info\n");
            write(b"  whoami            current context + pid\n");
            write(b"  ps                process table (ternary states)\n");
            write(b"  clear             clear screen\n");
            write(b"  reboot            reboot machine\n");
            write(b"  uptime            seconds since boot\n");
            write(b"  mem               memory usage\n");
            write(b"  trit <op> <a> [b] ternary arithmetic\n");
            write(b"  exit              exit shell\n");
        } else if line == b"uname" {
            write(b"Rusty Penguin 1.0.0 x86_64 ternary-kernel\n");
            write(b"Binary hardware. Ternary mind. No Linux.\n");
        } else if line == b"uname -a" {
            write(b"RustyPenguin 1.0.0 psh x86_64 GNU/Trit\n");
        } else if line == b"whoami" {
            write(b"ring3  pid=");
            write_u64(sys_getpid());
            write(b"\n");
        } else if line == b"ps" {
            cmd_ps();
        } else if line == b"version" {
            write(b"Rusty Penguin v1.0.0 -- Binary hardware. Ternary mind.\n");
        } else if line == b"uptime" {
            let ticks = sys_ticks();
            let secs  = ticks / 100;
            let mins  = secs / 60;
            let hrs   = mins / 60;
            write(b"up ");
            if hrs > 0 { write_u64(hrs); write(b"h "); }
            if mins % 60 > 0 || hrs > 0 { write_u64(mins % 60); write(b"m "); }
            write_u64(secs % 60); write(b"s  (");
            write_u64(ticks); write(b" ticks at 100 Hz)\n");
        } else if line == b"mem" {
            let (free_mib, total_mib) = sys_meminfo();
            let used_mib = total_mib.saturating_sub(free_mib);
            write(b"mem  total: "); write_u64(total_mib as u64); write(b" MiB");
            write(b"  used: ");  write_u64(used_mib as u64); write(b" MiB");
            write(b"  free: ");  write_u64(free_mib as u64); write(b" MiB\n");
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
        } else if line == b"trit" {
            write(b"usage: trit add|sub|mul|neg|cns <a> [b]\n");
            write(b"  balanced ternary: digits +  0  -  (weights +1  0  -1)\n");
        } else if line.starts_with(b"trit ") {
            cmd_trit(&line[5..]);
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
