// Terminal emulator with built-in shell. No PTY, no fork — commands run inline.
// VT100 parser kept intentionally minimal (newline, CR, backspace, CSI m).

use alloc::vec::Vec;
use alloc::string::String;
use alloc::format;
use crate::fb::Framebuffer;

pub const COLS: usize = 80;
pub const ROWS: usize = 24;
pub const TERM_PIX_W: u32 = (COLS * 8) as u32;
pub const TERM_PIX_H: u32 = (ROWS * 8) as u32;

const DEFAULT_FG: u32 = 0x4ADE80;
const DEFAULT_BG: u32 = 0x0F172A;

#[derive(Clone, Copy)]
pub struct Cell {
    pub ch: u8,
    pub fg: u32,
    pub bg: u32,
}

impl Default for Cell {
    fn default() -> Self { Cell { ch: b' ', fg: DEFAULT_FG, bg: DEFAULT_BG } }
}

enum EscState { Normal, Esc, Csi(String) }

pub struct Terminal {
    pub cells:   Vec<Cell>,
    pub cur_col: usize,
    pub cur_row: usize,
    cur_fg:      u32,
    cur_bg:      u32,
    esc:         EscState,
    pub dirty:   bool,
    // Input line
    line_buf:    [u8; 256],
    line_len:    usize,
}

// ── Syscall helpers ──────────────────────────────────────────────────────────

fn sys_ticks() -> u64 {
    let n: u64;
    unsafe {
        core::arch::asm!(
            "syscall",
            inout("rax") 4u64 => n,
            in("rdi") 0u64,
            out("rcx") _, out("r11") _,
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
            out("rcx") _, out("r11") _,
            options(nostack),
        );
    }
    ((n >> 32) as u32, (n & 0xFFFF_FFFF) as u32)
}

fn sys_ps_raw(buf: *mut u8, max: usize) -> usize {
    let n: u64;
    unsafe {
        core::arch::asm!(
            "syscall",
            inout("rax") 9u64 => n,
            in("rdi") buf,
            in("rsi") max,
            in("rdx") 0u64,
            out("rcx") _, out("r11") _,
            options(nostack),
        );
    }
    n as usize
}

// ── Terminal impl ────────────────────────────────────────────────────────────

impl Terminal {
    pub fn spawn() -> Result<Self, String> {
        let blank = Cell::default();
        let mut t = Terminal {
            cells:   alloc::vec![blank; COLS * ROWS],
            cur_col: 0, cur_row: 0,
            cur_fg:  DEFAULT_FG, cur_bg: DEFAULT_BG,
            esc:     EscState::Normal,
            dirty:   true,
            line_buf: [0u8; 256],
            line_len: 0,
        };
        t.write_output(b"Rusty Penguin psh 1.0\r\n");
        t.write_output(b"type 'help' for commands\r\n> ");
        Ok(t)
    }

    pub fn poll(&mut self) -> bool { false }

    fn blank(&self) -> Cell { Cell { ch: b' ', fg: self.cur_fg, bg: self.cur_bg } }

    fn scroll_up(&mut self) {
        let blank = self.blank();
        for row in 0..ROWS - 1 {
            for col in 0..COLS {
                self.cells[row * COLS + col] = self.cells[(row + 1) * COLS + col];
            }
        }
        for col in 0..COLS { self.cells[(ROWS - 1) * COLS + col] = blank; }
    }

    fn put_char(&mut self, ch: u8) {
        let cell = Cell { ch, fg: self.cur_fg, bg: self.cur_bg };
        self.cells[self.cur_row * COLS + self.cur_col] = cell;
        self.cur_col += 1;
        if self.cur_col >= COLS {
            self.cur_col = 0;
            self.cur_row += 1;
            if self.cur_row >= ROWS { self.scroll_up(); self.cur_row = ROWS - 1; }
        }
    }

    fn newline(&mut self) {
        self.cur_col = 0;
        self.cur_row += 1;
        if self.cur_row >= ROWS { self.scroll_up(); self.cur_row = ROWS - 1; }
    }

    pub fn process_byte(&mut self, b: u8) {
        let new_esc = match &mut self.esc {
            EscState::Normal => match b {
                b'\n' | 0x0A => { self.newline(); EscState::Normal }
                b'\r' => { self.cur_col = 0; EscState::Normal }
                0x08 | 0x7F => {
                    if self.cur_col > 0 {
                        self.cur_col -= 1;
                        let blank = self.blank();
                        self.cells[self.cur_row * COLS + self.cur_col] = blank;
                    }
                    EscState::Normal
                }
                0x1B => EscState::Esc,
                b if b >= 0x20 => { self.put_char(b); EscState::Normal }
                _ => EscState::Normal,
            },
            EscState::Esc => match b {
                b'[' => EscState::Csi(String::new()),
                _    => EscState::Normal,
            },
            EscState::Csi(ref mut p) => {
                if b.is_ascii_alphabetic() {
                    let params = p.clone();
                    self.handle_csi(&params, b);
                    EscState::Normal
                } else {
                    p.push(b as char);
                    return;
                }
            }
        };
        self.esc = new_esc;
        self.dirty = true;
    }

    fn handle_csi(&mut self, p: &str, cmd: u8) {
        let p = p.trim_start_matches('?');
        let n1 = || p.split(';').next().and_then(|s| s.parse::<usize>().ok()).unwrap_or(1).max(1);
        match cmd {
            b'm' => {
                if p.is_empty() || p == "0" {
                    self.cur_fg = DEFAULT_FG; self.cur_bg = DEFAULT_BG;
                }
            }
            b'J' if p == "2" || p == "3" => {
                let blank = self.blank();
                for c in self.cells.iter_mut() { *c = blank; }
                self.cur_col = 0; self.cur_row = 0;
            }
            b'H' | b'f' => {
                let mut parts = p.splitn(2, ';');
                let r = parts.next().and_then(|s| s.parse::<usize>().ok()).unwrap_or(1).max(1) - 1;
                let c = parts.next().and_then(|s| s.parse::<usize>().ok()).unwrap_or(1).max(1) - 1;
                self.cur_row = r.min(ROWS - 1); self.cur_col = c.min(COLS - 1);
            }
            b'A' => { self.cur_row = self.cur_row.saturating_sub(n1()); }
            b'B' => { self.cur_row = (self.cur_row + n1()).min(ROWS - 1); }
            b'C' => { self.cur_col = (self.cur_col + n1()).min(COLS - 1); }
            b'D' => { self.cur_col = self.cur_col.saturating_sub(n1()); }
            _ => {}
        }
        self.dirty = true;
    }

    pub fn write_output(&mut self, bytes: &[u8]) {
        for &b in bytes { self.process_byte(b); }
        self.dirty = true;
    }

    pub fn write_input(&self, bytes: &[u8]) {
        // No-op at the Terminal level. send_key handles echo+exec.
        let _ = bytes;
    }

    pub fn send_key(&mut self, b: u8) {
        if b == b'\n' || b == b'\r' {
            self.process_byte(b'\n');
            let line_len = self.line_len;
            let line: [u8; 256] = self.line_buf;
            self.line_len = 0;
            self.exec_command(&line[..line_len]);
            self.write_output(b"> ");
        } else if b == 0x08 || b == 0x7F {
            if self.line_len > 0 {
                self.line_len -= 1;
                self.process_byte(0x08);
            }
        } else if b >= 0x20 && self.line_len < 255 {
            self.line_buf[self.line_len] = b;
            self.line_len += 1;
            self.process_byte(b);
        }
    }

    fn exec_command(&mut self, line: &[u8]) {
        // strip trailing whitespace
        let mut end = line.len();
        while end > 0 && line[end - 1] == b' ' { end -= 1; }
        let line = &line[..end];

        if line.is_empty() { return; }

        if line == b"help" {
            self.write_output(b"commands: echo uname whoami ps uptime mem ai trit help exit\r\n");
        } else if line == b"uname" || line == b"uname -a" {
            self.write_output(b"RustyPenguin 1.0.0 psh x86_64 GNU/Trit\r\n");
        } else if line == b"whoami" {
            self.write_output(b"ring3\r\n");
        } else if line == b"uptime" {
            let ticks = sys_ticks();
            let secs = ticks / 100;
            let h = secs / 3600; let m = (secs % 3600) / 60; let s = secs % 60;
            let out = format!("up {:02}:{:02}:{:02} ({} ticks)\r\n", h, m, s, ticks);
            self.write_output(out.as_bytes());
        } else if line == b"mem" {
            let (free, total) = sys_meminfo();
            let used = total.saturating_sub(free);
            let out = format!("total {} MiB  used {} MiB  free {} MiB\r\n", total, used, free);
            self.write_output(out.as_bytes());
        } else if line == b"ps" {
            let mut buf = [0u8; 32 * 16];
            let count = sys_ps_raw(buf.as_mut_ptr(), 16);
            self.write_output(b"PID  ST  NAME\r\n");
            for i in 0..count {
                let off = i * 32;
                let mut pid: u64 = 0;
                for j in 0..8usize { pid |= (buf[off + j] as u64) << (j * 8); }
                let state_ch = match buf[off + 8] { 1 => b'+', 2 => b'-', _ => b'0' };
                let name = &buf[off + 16..off + 32];
                let nlen = name.iter().position(|&b| b == 0).unwrap_or(16);
                let name_str = core::str::from_utf8(&name[..nlen]).unwrap_or("?");
                let out = format!("{:<4} {}   {}\r\n", pid, state_ch as char, name_str);
                self.write_output(out.as_bytes());
            }
        } else if line == b"clear" {
            let blank = Cell::default();
            for c in self.cells.iter_mut() { *c = blank; }
            self.cur_col = 0; self.cur_row = 0;
            self.dirty = true;
        } else if line == b"echo" {
            self.write_output(b"\r\n");
        } else if line.starts_with(b"echo ") {
            self.write_output(&line[5..]);
            self.write_output(b"\r\n");
        } else if line == b"ai" || line.starts_with(b"ai ") {
            let arg = if line.starts_with(b"ai ") { &line[3..] } else { b"32" as &[u8] };
            let n: i64 = {
                let mut v: i64 = 0;
                for &b in arg { if b >= b'0' && b <= b'9' { v = v.wrapping_mul(10).wrapping_add((b - b'0') as i64); } }
                v
            };
            self.write_output(b"albert. bare metal\r\n");
            let (free, total) = sys_meminfo();
            let used = total.saturating_sub(free);
            let out = format!("mem {}/{}M  tokens {}\r\n", used, total, n);
            self.write_output(out.as_bytes());
            self.write_output(b"inference: not available (bare metal)\r\n");
            self.write_output(b"try: trit add 42 -7\r\n");
        } else if line == b"trit" {
            self.write_output(b"usage: trit add|sub|mul|neg|cns <a> [b]\r\n");
        } else if line.starts_with(b"trit ") {
            self.exec_trit(&line[5..]);
        } else if line == b"exit" {
            // Signal to close this window — main loop handles it
            self.write_output(b"[closing terminal]\r\n");
        } else {
            let out = format!("psh: command not found: '{}'\r\n",
                core::str::from_utf8(line).unwrap_or("?"));
            self.write_output(out.as_bytes());
        }
    }

    fn exec_trit(&mut self, args: &[u8]) {
        let mut parts = [b"" as &[u8]; 3];
        let mut count = 0;
        let mut start = 0;
        let mut in_word = false;
        for i in 0..=args.len() {
            let at_sp = i == args.len() || args[i] == b' ';
            if at_sp && in_word {
                if count < 3 { parts[count] = &args[start..i]; count += 1; }
                in_word = false;
            } else if !at_sp && !in_word {
                start = i; in_word = true;
            }
        }
        if count == 0 { self.write_output(b"usage: trit add|sub|mul|neg|cns <a> [b]\r\n"); return; }
        let op = parts[0];
        let parse = |s: &[u8]| -> Option<i64> {
            if s.is_empty() { return None; }
            let (neg, d) = if s[0] == b'-' { (true, &s[1..]) } else { (false, s) };
            let mut n: i64 = 0;
            for &b in d { if b < b'0' || b > b'9' { return None; } n = n.wrapping_mul(10).wrapping_add((b-b'0') as i64); }
            Some(if neg { -n } else { n })
        };
        let to_tern = |mut n: i64| -> String {
            if n == 0 { return String::from("0"); }
            let flip = n < 0; if n < 0 { n = -n; }
            let mut digits = [0i8; 40]; let mut len = 0; let mut v = n;
            while v != 0 {
                let rem = (v % 3) as i8; v /= 3;
                if rem == 2 { digits[len] = -1; v += 1; } else { digits[len] = rem; }
                len += 1;
            }
            let mut s = String::new();
            for k in 0..len {
                let d = if flip { -digits[len-1-k] } else { digits[len-1-k] };
                s.push(match d { 1 => '+', -1 => '-', _ => '0' });
            }
            s
        };

        if op == b"neg" {
            if count < 2 { self.write_output(b"usage: trit neg <a>\r\n"); return; }
            if let Some(a) = parse(parts[1]) {
                let r = -a;
                self.write_output(format!("{}  ({})\r\n", r, to_tern(r)).as_bytes());
            }
            return;
        }
        if count < 3 { self.write_output(b"usage: trit <op> <a> <b>\r\n"); return; }
        let (a, b_val) = match (parse(parts[1]), parse(parts[2])) {
            (Some(a), Some(b)) => (a, b),
            _ => { self.write_output(b"bad number\r\n"); return; }
        };
        let r: i64 = if op == b"add" { a + b_val }
            else if op == b"sub" { a - b_val }
            else if op == b"mul" { a * b_val }
            else if op == b"cns" { if a > 0 && b_val > 0 { 1 } else if a < 0 && b_val < 0 { -1 } else { 0 } }
            else { self.write_output(b"unknown op\r\n"); return; };
        self.write_output(format!("{}  ({})\r\n", r, to_tern(r)).as_bytes());
    }

    pub fn render(&self, fb: &mut Framebuffer, x: u32, y: u32) {
        for row in 0..ROWS {
            for col in 0..COLS {
                let cell = &self.cells[row * COLS + col];
                fb.draw_char(x + col as u32 * 8, y + row as u32 * 8, cell.ch as char, cell.fg, cell.bg);
            }
        }
        // Block cursor
        let cx = x + self.cur_col as u32 * 8;
        let cy = y + self.cur_row as u32 * 8;
        if cx + 8 <= fb.width && cy + 8 <= fb.height {
            fb.fill_rect(cx, cy, 8, 8, DEFAULT_FG);
            let cell = &self.cells[self.cur_row * COLS + self.cur_col];
            if cell.ch > b' ' {
                fb.draw_char(cx, cy, cell.ch as char, DEFAULT_BG, DEFAULT_FG);
            }
        }
    }
}
