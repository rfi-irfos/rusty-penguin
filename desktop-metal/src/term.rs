// Terminal emulator with built-in shell. No PTY, no fork — commands run inline.
// VT100 parser kept intentionally minimal (newline, CR, backspace, CSI m).

use alloc::vec::Vec;
use alloc::string::String;
use alloc::format;
use crate::fb::Framebuffer;
use crate::trit::{Trit, linear_layer, seed_trits};

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

const HISTORY_CAP: usize = 32;

pub struct Terminal {
    pub cells:       Vec<Cell>,
    pub cur_col:     usize,
    pub cur_row:     usize,
    cur_fg:          u32,
    cur_bg:          u32,
    esc:             EscState,
    pub dirty:       bool,
    pub wants_close: bool,
    // Input line
    line_buf:        [u8; 256],
    line_len:        usize,
    line_cursor:     usize,   // insertion point within line_buf, 0..=line_len
    // Command history
    history:         Vec<Vec<u8>>,
    hist_pos:        usize,   // 0 = browsing oldest, history.len() = live input
    saved_line:      Vec<u8>, // line saved when user starts browsing
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
            cells:       alloc::vec![blank; COLS * ROWS],
            cur_col:     0, cur_row: 0,
            cur_fg:      DEFAULT_FG, cur_bg: DEFAULT_BG,
            esc:         EscState::Normal,
            dirty:       true,
            wants_close: false,
            line_buf:    [0u8; 256],
            line_len:    0,
            line_cursor: 0,
            history:     Vec::new(),
            hist_pos:    0,
            saved_line:  Vec::new(),
        };
        t.write_output(b"\x1b[32mRusty Penguin\x1b[0m psh 1.0\r\n");
        t.write_output(b"\x1b[90mtype 'help' for commands\x1b[0m\r\n\x1b[32m>\x1b[0m ");
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
                if p.is_empty() {
                    self.cur_fg = DEFAULT_FG; self.cur_bg = DEFAULT_BG;
                } else {
                    for part in p.split(';') {
                        match part.parse::<u32>().unwrap_or(0) {
                            0  => { self.cur_fg = DEFAULT_FG; self.cur_bg = DEFAULT_BG; }
                            1  => {} // bold — ignored
                            30 => self.cur_fg = 0x1E293B,
                            31 => self.cur_fg = 0xEF4444,
                            32 => self.cur_fg = 0x4ADE80,
                            33 => self.cur_fg = 0xFBBF24,
                            34 => self.cur_fg = 0x60A5FA,
                            35 => self.cur_fg = 0xC084FC,
                            36 => self.cur_fg = 0x22D3EE,
                            37 => self.cur_fg = 0xF8FAFC,
                            90 => self.cur_fg = 0x64748B,
                            91 => self.cur_fg = 0xF87171,
                            92 => self.cur_fg = 0x86EFAC,
                            93 => self.cur_fg = 0xFDE68A,
                            94 => self.cur_fg = 0x93C5FD,
                            95 => self.cur_fg = 0xD8B4FE,
                            96 => self.cur_fg = 0x67E8F9,
                            97 => self.cur_fg = 0xFFFFFF,
                            _  => {}
                        }
                    }
                }
            }
            b'J' if p == "2" || p == "3" => {
                let blank = self.blank();
                for c in self.cells.iter_mut() { *c = blank; }
                self.cur_col = 0; self.cur_row = 0;
            }
            // Absolute cursor position (with params) — used by shell output, not line editing
            b'H' | b'f' if !p.is_empty() && p != ";" => {
                let mut parts = p.splitn(2, ';');
                let r = parts.next().and_then(|s| s.parse::<usize>().ok()).unwrap_or(1).max(1) - 1;
                let c = parts.next().and_then(|s| s.parse::<usize>().ok()).unwrap_or(1).max(1) - 1;
                self.cur_row = r.min(ROWS - 1); self.cur_col = c.min(COLS - 1);
            }
            // Up/Down with no params → history navigation
            b'A' if p.is_empty() => {
                if !self.history.is_empty() {
                    if self.hist_pos == self.history.len() {
                        self.saved_line = self.line_buf[..self.line_len].to_vec();
                    }
                    if self.hist_pos > 0 {
                        self.hist_pos -= 1;
                        let entry = self.history[self.hist_pos].clone();
                        self.load_history_line(&entry);
                    }
                }
            }
            b'B' if p.is_empty() => {
                if self.hist_pos < self.history.len() {
                    self.hist_pos += 1;
                    let line = if self.hist_pos == self.history.len() {
                        self.saved_line.clone()
                    } else {
                        self.history[self.hist_pos].clone()
                    };
                    self.load_history_line(&line);
                }
            }
            // Left arrow — move cursor back in input line
            b'D' if p.is_empty() => {
                if self.line_cursor > 0 {
                    self.line_cursor -= 1;
                    self.cur_col = self.cur_col.saturating_sub(1);
                }
            }
            // Right arrow — move cursor forward in input line
            b'C' if p.is_empty() => {
                if self.line_cursor < self.line_len {
                    self.line_cursor += 1;
                    self.cur_col = (self.cur_col + 1).min(COLS - 1);
                }
            }
            // Home — jump to start of input line
            b'H' if p.is_empty() => {
                let back = self.line_cursor;
                self.line_cursor = 0;
                self.cur_col = self.cur_col.saturating_sub(back);
            }
            // End — jump to end of input line
            b'F' if p.is_empty() => {
                let fwd = self.line_len - self.line_cursor;
                self.line_cursor = self.line_len;
                self.cur_col = (self.cur_col + fwd).min(COLS - 1);
            }
            // Delete key (ESC [ 3 ~) — forward delete at cursor
            b'~' if p == "3" => {
                if self.line_cursor < self.line_len {
                    for i in self.line_cursor..self.line_len - 1 {
                        self.line_buf[i] = self.line_buf[i + 1];
                    }
                    self.line_len -= 1;
                    self.redraw_from(self.line_cursor);
                }
            }
            // Parameterised cursor movement (used by output sequences, not line editing)
            b'A' => { self.cur_row = self.cur_row.saturating_sub(n1()); }
            b'B' => { self.cur_row = (self.cur_row + n1()).min(ROWS - 1); }
            b'C' => { self.cur_col = (self.cur_col + n1()).min(COLS - 1); }
            b'D' => { self.cur_col = self.cur_col.saturating_sub(n1()); }
            _ => {}
        }
        self.dirty = true;
    }

    /// Redraw line_buf[from..line_len] starting at the current terminal cursor column,
    /// append one blank to erase any leftover char (needed after deletion),
    /// then reposition the terminal cursor to match line_cursor.
    /// Invariant: call only when cur_col == prompt_col + from.
    fn redraw_from(&mut self, from: usize) {
        let start_col = self.cur_col;
        for i in from..self.line_len {
            if self.cur_col < COLS {
                let ch = self.line_buf[i];
                self.cells[self.cur_row * COLS + self.cur_col] =
                    Cell { ch, fg: self.cur_fg, bg: self.cur_bg };
                self.cur_col += 1;
            }
        }
        // Erase one trailing cell (accounts for deletion making the line shorter)
        if self.cur_col < COLS {
            self.cells[self.cur_row * COLS + self.cur_col] = self.blank();
            self.cur_col += 1;
        }
        // Reposition: we drew (line_len - from + 1) chars forward from start_col;
        // desired position is start_col + (line_cursor - from).
        let desired = start_col + self.line_cursor.saturating_sub(from);
        if desired <= self.cur_col { self.cur_col = desired; }
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

    fn erase_line_input(&mut self) {
        // Advance visual cursor to end of line before erasing backwards
        while self.line_cursor < self.line_len {
            self.cur_col = (self.cur_col + 1).min(COLS - 1);
            self.line_cursor += 1;
        }
        while self.line_len > 0 {
            self.line_len -= 1;
            self.line_cursor = self.line_len;
            self.process_byte(0x08);
        }
    }

    fn load_history_line(&mut self, src: &[u8]) {
        self.erase_line_input();
        let n = src.len().min(255);
        self.line_buf[..n].copy_from_slice(&src[..n]);
        self.line_len = n;
        self.line_cursor = n;
        for &b in &src[..n] { self.process_byte(b); }
    }

    pub fn send_key(&mut self, b: u8) {
        match b {
            b'\n' | b'\r' => {
                self.process_byte(b'\n');
                let line_len = self.line_len;
                let line: [u8; 256] = self.line_buf;
                self.line_len = 0;
                self.line_cursor = 0;
                self.hist_pos = self.history.len();
                self.saved_line.clear();
                if line_len > 0 {
                    let entry: Vec<u8> = line[..line_len].to_vec();
                    if self.history.last().map(|e| e.as_slice()) != Some(&line[..line_len]) {
                        if self.history.len() >= HISTORY_CAP { self.history.remove(0); }
                        self.history.push(entry);
                    }
                    self.hist_pos = self.history.len();
                }
                self.exec_command(&line[..line_len]);
                self.write_output(b"\x1b[32m>\x1b[0m ");
            }
            0x08 | 0x7F => {
                if self.line_cursor > 0 {
                    if self.line_cursor == self.line_len {
                        // Cursor at end: fast path
                        self.line_len -= 1;
                        self.line_cursor -= 1;
                        self.process_byte(0x08);
                    } else {
                        // Cursor in middle: shift left, redraw suffix
                        let from = self.line_cursor - 1;
                        for i in from..self.line_len - 1 {
                            self.line_buf[i] = self.line_buf[i + 1];
                        }
                        self.line_len -= 1;
                        self.line_cursor -= 1;
                        self.cur_col = self.cur_col.saturating_sub(1);
                        self.redraw_from(self.line_cursor);
                    }
                }
            }
            0x03 => {
                // Ctrl+C — cancel current line
                self.write_output(b"^C\r\n");
                self.line_len = 0;
                self.line_cursor = 0;
                self.hist_pos = self.history.len();
                self.saved_line.clear();
                self.write_output(b"\x1b[32m>\x1b[0m ");
            }
            0x0C => {
                // Ctrl+L — clear screen, redraw prompt and current line content
                let blank = Cell::default();
                for c in self.cells.iter_mut() { *c = blank; }
                self.cur_col = 0; self.cur_row = 0;
                self.write_output(b"\x1b[32m>\x1b[0m ");
                // Replay current line up to line_len
                for i in 0..self.line_len {
                    let ch = self.line_buf[i];
                    self.process_byte(ch);
                }
                // Reposition cursor to line_cursor
                let back = self.line_len - self.line_cursor;
                self.cur_col = self.cur_col.saturating_sub(back);
                self.dirty = true;
            }
            0x01 => {
                // Ctrl+A — go to start of line
                let back = self.line_cursor;
                self.line_cursor = 0;
                self.cur_col = self.cur_col.saturating_sub(back);
                self.dirty = true;
            }
            0x05 => {
                // Ctrl+E — go to end of line
                let fwd = self.line_len - self.line_cursor;
                self.line_cursor = self.line_len;
                self.cur_col = (self.cur_col + fwd).min(COLS - 1);
                self.dirty = true;
            }
            b if b >= 0x20 && self.line_len < 255 => {
                if self.line_cursor == self.line_len {
                    // Append at end: fast path
                    self.line_buf[self.line_len] = b;
                    self.line_len += 1;
                    self.line_cursor += 1;
                    self.process_byte(b);
                } else {
                    // Insert in middle: shift right, redraw suffix
                    let from = self.line_cursor;
                    let mut i = self.line_len;
                    while i > from { self.line_buf[i] = self.line_buf[i - 1]; i -= 1; }
                    self.line_buf[from] = b;
                    self.line_len += 1;
                    self.line_cursor += 1;
                    self.redraw_from(from);
                }
            }
            _ => {
                // ESC sequences (arrows, etc.) pass through to the VT100 parser
                self.process_byte(b);
            }
        }
    }

    fn exec_command(&mut self, line: &[u8]) {
        // strip trailing whitespace
        let mut end = line.len();
        while end > 0 && line[end - 1] == b' ' { end -= 1; }
        let line = &line[..end];

        if line.is_empty() { return; }

        if line == b"help" {
            self.write_output(b"commands:\r\n");
            self.write_output(b"  trit <n>           balanced ternary of n\r\n");
            self.write_output(b"  trit add|sub|mul <a> <b>\r\n");
            self.write_output(b"  ai [n]             sparse ternary inference (default 32)\r\n");
            self.write_output(b"  ls                 list files\r\n");
            self.write_output(b"  ps                 process table\r\n");
            self.write_output(b"  pwd  date  sysinfo  uname  whoami\r\n");
            self.write_output(b"  uptime  mem  echo  clear  exit\r\n");
            self.write_output(b"  \x1b[90mUp/Down=history  Left/Right=cursor  Home/End\x1b[0m\r\n");
            self.write_output(b"  \x1b[90mCtrl+C=cancel  Ctrl+L=clear  Ctrl+A/E=line start/end\x1b[0m\r\n");
            self.write_output(b"  \x1b[90mCtrl+T=new term  Ctrl+W=close term\x1b[0m\r\n");
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
            self.write_output(b"\x1b[36mPID  ST  NAME\x1b[0m\r\n");
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
        } else if line == b"ls" || line == b"ls /" || line == b"ls /bin" {
            self.write_output(b"bin/\r\n");
            self.write_output(b"  psh\r\n");
            self.write_output(b"  desktop\r\n");
        } else if line == b"echo" {
            self.write_output(b"\r\n");
        } else if line.starts_with(b"echo ") {
            self.write_output(&line[5..]);
            self.write_output(b"\r\n");
        } else if line == b"ai" || line.starts_with(b"ai ") {
            let arg = if line.starts_with(b"ai ") { &line[3..] } else { b"32" as &[u8] };
            let n_tokens: usize = {
                let mut v: usize = 0;
                for &b in arg { if b >= b'0' && b <= b'9' { v = v.wrapping_mul(10).wrapping_add((b - b'0') as usize); } }
                v.max(1).min(256)
            };
            self.run_ai_inference(n_tokens);
        } else if line == b"trit" {
            self.write_output(b"usage: trit add|sub|mul|neg|cns <a> [b]\r\n");
        } else if line.starts_with(b"trit ") {
            self.exec_trit(&line[5..]);
        } else if line == b"pwd" {
            self.write_output(b"/home/ring3\r\n");
        } else if line == b"date" {
            let ticks = sys_ticks();
            let secs = ticks / 100;
            let out = format!("uptime {:02}h {:02}m {:02}s\r\n",
                secs / 3600, (secs % 3600) / 60, secs % 60);
            self.write_output(out.as_bytes());
        } else if line == b"sysinfo" || line == b"neofetch" {
            let (free, total) = sys_meminfo();
            let used = total.saturating_sub(free);
            let ticks = sys_ticks();
            let secs = ticks / 100;
            self.write_output(b"\x1b[32m  RustyPenguin 1.0.0\x1b[0m\r\n");
            self.write_output(b"\x1b[90m  --------------------\x1b[0m\r\n");
            self.write_output(b"  \x1b[36mOS    \x1b[0m : RustyPenguin bare metal x86_64\r\n");
            self.write_output(b"  \x1b[36mKernel\x1b[0m : Rust + ternary STE\r\n");
            self.write_output(b"  \x1b[36mShell \x1b[0m : psh 1.0\r\n");
            let mem = format!("  \x1b[36mMemory\x1b[0m : {}/{} MiB\r\n", used, total);
            self.write_output(mem.as_bytes());
            let up = format!("  \x1b[36mUptime\x1b[0m : {:02}:{:02}:{:02}\r\n",
                secs / 3600, (secs % 3600) / 60, secs % 60);
            self.write_output(up.as_bytes());
            self.write_output(b"  \x1b[35mModel \x1b[0m : Binary hardware. Ternary mind.\r\n");
        } else if line == b"exit" {
            self.write_output(b"bye\r\n");
            self.wants_close = true;
        } else {
            let out = format!("\x1b[31mpsh: command not found:\x1b[0m '{}'\r\n",
                core::str::from_utf8(line).unwrap_or("?"));
            self.write_output(out.as_bytes());
        }
    }

    fn run_ai_inference(&mut self, n_tokens: usize) {
        const DIM: usize = 8;
        const LAYERS: usize = 4;

        self.write_output(b"albert. [bare metal]\r\n");
        let s_line = format!("sparse ternary inference -- {} layers x {} tokens\r\n", LAYERS, n_tokens);
        self.write_output(s_line.as_bytes());
        self.write_output(b"\r\n");

        let seed = sys_ticks().wrapping_add(n_tokens as u64 * 7);

        // Weight matrices for all layers (stack-allocated: 4 * 8 * 8 = 256 trits)
        let mut w_all = [Trit::Zero; LAYERS * DIM * DIM];
        seed_trits(&mut w_all, seed ^ 0xCAFE_BABE_DEAD_BEEF);

        // Initial activation vector, seeded from n_tokens + ticks
        let mut act = [Trit::Zero; DIM];
        seed_trits(&mut act, seed);

        // Show input
        self.write_output(b"  input  [");
        for t in &act { self.process_byte(t.to_byte()); }
        self.write_output(b"]\r\n");

        let mut total_total = 0usize;
        let mut total_skip  = 0usize;

        for l in 0..LAYERS {
            let in_act = act;
            let w = &w_all[l * DIM * DIM..(l + 1) * DIM * DIM];
            let mut out_act = [Trit::Zero; DIM];
            let (t, sk) = linear_layer(w, DIM, DIM, &in_act, &mut out_act);
            total_total += t;
            total_skip  += sk;
            let dorm = if t > 0 { sk * 100 / t } else { 0 };

            let prefix = format!("  L{}     [", l);
            self.write_output(prefix.as_bytes());
            for t in &in_act  { self.process_byte(t.to_byte()); }
            self.write_output(b"] -> [");
            for t in &out_act { self.process_byte(t.to_byte()); }
            let suffix = format!("]  dormancy {}%\r\n", dorm);
            self.write_output(suffix.as_bytes());

            act = out_act;
        }

        let avg_dorm = if total_total > 0 { total_skip * 100 / total_total } else { 0 };
        self.write_output(b"\r\n");
        let summary = format!("{} tokens  avg dormancy {}%  skipped {}/{} ops\r\n",
            n_tokens, avg_dorm, total_skip, total_total);
        self.write_output(summary.as_bytes());
        self.write_output(b"ACTIVE -- Binary hardware. Ternary mind.\r\n");
    }

    fn to_tern(n: i64) -> String {
        if n == 0 { return String::from("0"); }
        let flip = n < 0;
        let mut v = if n < 0 { -n } else { n };
        let mut digits = [0i8; 40];
        let mut len = 0;
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
    }

    fn parse_i64(s: &[u8]) -> Option<i64> {
        if s.is_empty() { return None; }
        let (neg, d) = if s[0] == b'-' { (true, &s[1..]) } else { (false, s) };
        if d.is_empty() { return None; }
        let mut n: i64 = 0;
        for &b in d {
            if b < b'0' || b > b'9' { return None; }
            n = n.wrapping_mul(10).wrapping_add((b - b'0') as i64);
        }
        Some(if neg { -n } else { n })
    }

    fn exec_trit(&mut self, args: &[u8]) {
        let mut parts = [b"" as &[u8]; 3];
        let mut count = 0usize;
        let mut start = 0usize;
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

        // Plain number: "trit 42" → show balanced ternary representation
        let is_keyword = matches!(op, b"add" | b"sub" | b"mul" | b"neg" | b"cns");
        if !is_keyword && count == 1 {
            match Self::parse_i64(op) {
                Some(n) => self.write_output(format!("{}  ({})\r\n", n, Self::to_tern(n)).as_bytes()),
                None    => self.write_output(b"usage: trit add|sub|mul|neg|cns <a> [b]\r\n"),
            }
            return;
        }

        if op == b"neg" {
            if count < 2 { self.write_output(b"usage: trit neg <a>\r\n"); return; }
            if let Some(a) = Self::parse_i64(parts[1]) {
                let r = -a;
                self.write_output(format!("{}  ({})\r\n", r, Self::to_tern(r)).as_bytes());
            }
            return;
        }
        if count < 3 { self.write_output(b"usage: trit <op> <a> <b>\r\n"); return; }
        let (a, bv) = match (Self::parse_i64(parts[1]), Self::parse_i64(parts[2])) {
            (Some(a), Some(b)) => (a, b),
            _ => { self.write_output(b"bad number\r\n"); return; }
        };
        let r: i64 = if op == b"add" { a + bv }
            else if op == b"sub" { a - bv }
            else if op == b"mul" { a * bv }
            else if op == b"cns" { if a > 0 && bv > 0 { 1 } else if a < 0 && bv < 0 { -1 } else { 0 } }
            else { self.write_output(b"unknown op\r\n"); return; };
        self.write_output(format!("{}  ({})\r\n", r, Self::to_tern(r)).as_bytes());
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
