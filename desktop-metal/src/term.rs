// Terminal emulator + psh shell + nano-style text editor.
// No PTY, no fork — everything runs inline in the single ring-3 process.

use alloc::vec::Vec;
use alloc::string::String;
use alloc::format;
use crate::fb::Framebuffer;
use crate::trit::{Trit, linear_layer, seed_trits};
use crate::vfs;

pub const COLS: usize = 80;
pub const ROWS: usize = 24;
pub const TERM_PIX_W: u32 = (COLS * 8) as u32;
pub const TERM_PIX_H: u32 = (ROWS * 8) as u32;

const DEFAULT_FG: u32 = 0x4ADE80;
const DEFAULT_BG: u32 = 0x0F172A;

// ── Cell ────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
pub struct Cell {
    pub ch: u8,
    pub fg: u32,
    pub bg: u32,
}

impl Default for Cell {
    fn default() -> Self { Cell { ch: b' ', fg: DEFAULT_FG, bg: DEFAULT_BG } }
}

// ── Editor state ─────────────────────────────────────────────────────────────

pub struct EditorState {
    pub filename:   String,
    pub lines:      Vec<Vec<u8>>,
    pub cursor_row: usize,
    pub cursor_col: usize,
    pub scroll_top: usize,
    pub modified:   bool,
    esc_state:      u8,   // 0=normal 1=saw-ESC 2=saw-CSI
}

impl EditorState {
    fn new(filename: &str, data: &[u8]) -> Self {
        let mut lines: Vec<Vec<u8>> = Vec::new();
        let mut cur: Vec<u8> = Vec::new();
        for &b in data {
            if b == b'\n' { lines.push(core::mem::replace(&mut cur, Vec::new())); }
            else { cur.push(b); }
        }
        lines.push(cur);
        if lines.is_empty() { lines.push(Vec::new()); }
        EditorState {
            filename: String::from(filename),
            lines,
            cursor_row: 0,
            cursor_col: 0,
            scroll_top: 0,
            modified: false,
            esc_state: 0,
        }
    }

    fn line_len(&self, row: usize) -> usize {
        self.lines.get(row).map(|l| l.len()).unwrap_or(0)
    }

    fn clamp_col(&mut self) {
        let max = self.line_len(self.cursor_row);
        if self.cursor_col > max { self.cursor_col = max; }
    }

    fn clamp_scroll(&mut self) {
        const CONTENT: usize = ROWS - 2;
        if self.cursor_row < self.scroll_top {
            self.scroll_top = self.cursor_row;
        } else if self.cursor_row >= self.scroll_top + CONTENT {
            self.scroll_top = self.cursor_row - CONTENT + 1;
        }
    }

    fn move_up(&mut self) {
        if self.cursor_row > 0 { self.cursor_row -= 1; self.clamp_col(); }
    }
    fn move_down(&mut self) {
        if self.cursor_row + 1 < self.lines.len() { self.cursor_row += 1; self.clamp_col(); }
    }
    fn move_left(&mut self) {
        if self.cursor_col > 0 { self.cursor_col -= 1; }
        else if self.cursor_row > 0 {
            self.cursor_row -= 1;
            self.cursor_col = self.line_len(self.cursor_row);
        }
    }
    fn move_right(&mut self) {
        let ll = self.line_len(self.cursor_row);
        if self.cursor_col < ll { self.cursor_col += 1; }
        else if self.cursor_row + 1 < self.lines.len() { self.cursor_row += 1; self.cursor_col = 0; }
    }

    fn insert_char(&mut self, b: u8) {
        if self.cursor_row < self.lines.len() {
            let col = self.cursor_col;
            self.lines[self.cursor_row].insert(col, b);
            self.cursor_col += 1;
            self.modified = true;
        }
    }

    fn backspace(&mut self) {
        let row = self.cursor_row;
        let col = self.cursor_col;
        if col > 0 {
            self.lines[row].remove(col - 1);
            self.cursor_col -= 1;
            self.modified = true;
        } else if row > 0 {
            let cur_line = self.lines.remove(row);
            self.cursor_row -= 1;
            self.cursor_col = self.lines[self.cursor_row].len();
            self.lines[self.cursor_row].extend_from_slice(&cur_line);
            self.modified = true;
        }
    }

    fn newline(&mut self) {
        let row = self.cursor_row;
        let col = self.cursor_col;
        if row < self.lines.len() {
            let rest = self.lines[row].split_off(col);
            self.lines.insert(row + 1, rest);
            self.cursor_row += 1;
            self.cursor_col = 0;
            self.modified = true;
        }
    }

    fn to_bytes(&self) -> Vec<u8> {
        let mut out: Vec<u8> = Vec::new();
        for (i, line) in self.lines.iter().enumerate() {
            out.extend_from_slice(line);
            if i + 1 < self.lines.len() { out.push(b'\n'); }
        }
        out
    }
}

// ── Kernel Manager state ─────────────────────────────────────────────────────

pub struct KmState {
    pub path:   Vec<u8>,    // kernel ELF path input
    pub status: String,     // last action status line
}

impl KmState {
    fn new() -> Self { KmState { path: Vec::new(), status: String::new() } }
}

// ── Escape-sequence parsing ──────────────────────────────────────────────────

enum EscState { Normal, Esc, Csi(String) }

// ── Pipeline helpers ─────────────────────────────────────────────────────────

fn trim_ws(b: &[u8]) -> &[u8] {
    let s = b.iter().position(|&x| x != b' ' && x != b'\t').unwrap_or(b.len());
    let e = b.iter().rposition(|&x| x != b' ' && x != b'\t').map(|i| i + 1).unwrap_or(0);
    if s < e { &b[s..e] } else { b"" }
}

fn strip_ansi(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        if input[i] == 0x1B {
            i += 1;
            if i < input.len() && input[i] == b'[' {
                i += 1;
                while i < input.len() && !input[i].is_ascii_alphabetic() { i += 1; }
                if i < input.len() { i += 1; }
            }
        } else {
            if input[i] != b'\r' { out.push(input[i]); }
            i += 1;
        }
    }
    out
}

// Returns (cmd_part, Option<(filename, is_append)>)
fn parse_redir<'a>(seg: &'a [u8]) -> (&'a [u8], Option<(String, bool)>) {
    // Search >> first
    let mut i = 0;
    while i + 1 < seg.len() {
        if seg[i] == b'>' && seg[i + 1] == b'>' {
            let cmd = trim_ws(&seg[..i]);
            let fname = core::str::from_utf8(trim_ws(&seg[i + 2..])).unwrap_or("").trim();
            if !fname.is_empty() { return (cmd, Some((String::from(fname), true))); }
        }
        i += 1;
    }
    // Single >
    for i in 0..seg.len() {
        if seg[i] == b'>' && !(i + 1 < seg.len() && seg[i + 1] == b'>') {
            let cmd = trim_ws(&seg[..i]);
            let fname = core::str::from_utf8(trim_ws(&seg[i + 1..])).unwrap_or("").trim();
            if !fname.is_empty() { return (cmd, Some((String::from(fname), false))); }
        }
    }
    (seg, None)
}

// ── Simple arithmetic expression parser (recursive descent) ─────────────────

fn calc_skip(s: &[u8]) -> &[u8] {
    let i = s.iter().position(|&b| b != b' ' && b != b'\t').unwrap_or(s.len());
    &s[i..]
}
fn calc_atom(s: &[u8]) -> Option<(i64, &[u8])> {
    let s = calc_skip(s);
    if s.is_empty() { return None; }
    if s[0] == b'(' {
        let (v, r) = calc_add(calc_skip(&s[1..]))?;
        let r = calc_skip(r);
        if r.first() != Some(&b')') { return None; }
        return Some((v, &r[1..]));
    }
    let neg = s[0] == b'-';
    let d = if neg { &s[1..] } else { s };
    if d.is_empty() || !d[0].is_ascii_digit() { return None; }
    let mut n: i64 = 0;
    let mut i = 0;
    while i < d.len() && d[i].is_ascii_digit() { n = n * 10 + (d[i] - b'0') as i64; i += 1; }
    Some((if neg { -n } else { n }, &d[i..]))
}
fn calc_mul(s: &[u8]) -> Option<(i64, &[u8])> {
    let (mut lhs, mut r) = calc_atom(s)?;
    loop {
        let r2 = calc_skip(r);
        match r2.first().copied() {
            Some(b'*') => { let (rhs, r3) = calc_atom(calc_skip(&r2[1..]))?; lhs = lhs.wrapping_mul(rhs); r = r3; }
            Some(b'/') => { let (rhs, r3) = calc_atom(calc_skip(&r2[1..]))?; if rhs == 0 { return None; } lhs /= rhs; r = r3; }
            Some(b'%') => { let (rhs, r3) = calc_atom(calc_skip(&r2[1..]))?; if rhs == 0 { return None; } lhs %= rhs; r = r3; }
            _ => break,
        }
    }
    Some((lhs, r))
}
fn calc_add(s: &[u8]) -> Option<(i64, &[u8])> {
    let (mut lhs, mut r) = calc_mul(s)?;
    loop {
        let r2 = calc_skip(r);
        match r2.first().copied() {
            Some(b'+') => { let (rhs, r3) = calc_mul(calc_skip(&r2[1..]))?; lhs = lhs.wrapping_add(rhs); r = r3; }
            Some(b'-') => { let (rhs, r3) = calc_mul(calc_skip(&r2[1..]))?; lhs = lhs.wrapping_sub(rhs); r = r3; }
            _ => break,
        }
    }
    Some((lhs, r))
}

// ── History ──────────────────────────────────────────────────────────────────

const HISTORY_CAP: usize = 32;

// ── Terminal ──────────────────────────────────────────────────────────────────

pub struct Terminal {
    pub cells:       Vec<Cell>,
    pub cur_col:     usize,
    pub cur_row:     usize,
    cur_fg:          u32,
    cur_bg:          u32,
    esc:             EscState,
    pub dirty:       bool,
    pub wants_close: bool,
    line_buf:        [u8; 256],
    line_len:        usize,
    line_cursor:     usize,
    history:         Vec<Vec<u8>>,
    hist_pos:        usize,
    saved_line:      Vec<u8>,
    pub editor:      Option<EditorState>,
    pub km:          Option<KmState>,   // kernel manager TUI
    capture:         Option<Vec<u8>>,   // when Some, write_output feeds this instead of cells
    stdin_data:      Vec<u8>,           // pipe input for the current command
    vars:            Vec<(String, String)>, // shell variables
    aliases:         Vec<(String, String)>, // command aliases (alias_name, expanded_cmd)
    cwd:             String,            // current working directory
    last_exit:       u8,               // exit code of last command
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

fn sys_yield() {
    unsafe {
        core::arch::asm!(
            "syscall",
            in("rax") 24u64,
            in("rdi") 0u64,
            out("rcx") _, out("r11") _,
            options(nostack),
        );
    }
}

// ── Terminal impl ─────────────────────────────────────────────────────────────

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
            editor:      None,
            km:          None,
            capture:     None,
            stdin_data:  Vec::new(),
            vars:        Vec::new(),
            aliases:     Vec::new(),
            cwd:         String::from("/"),
            last_exit:   0,
        };
        // Initialize default environment variables
        t.set_var("HOME", "/home/ring3");
        t.set_var("USER", "ring3");
        t.set_var("SHELL", "/bin/psh");
        t.set_var("TERM", "xterm-256color");
        t.set_var("PATH", "/bin:/usr/bin:/usr/local/bin");
        t.set_var("OS", "RustyPenguin");
        t.set_var("ARCH", "x86_64");
        t.write_output(b"\x1b[32m  Rusty Penguin\x1b[0m \x1b[90mv1.0.0 \xB7 psh 1.0\x1b[0m\r\n");
        t.write_output(b"\x1b[36m  Binary hardware. Ternary mind.\x1b[0m\r\n");
        t.write_output(b"\x1b[90m  Type 'help' for commands.\x1b[0m\r\n\r\n");
        t.write_output(b"\x1b[32mring3\x1b[0m@\x1b[36mrusty-penguin\x1b[90m:~$\x1b[0m ");
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
                b'\r'        => { self.cur_col = 0; EscState::Normal }
                0x08 | 0x7F  => {
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
                            1  => {}
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
            b'H' | b'f' if !p.is_empty() && p != ";" => {
                let mut parts = p.splitn(2, ';');
                let r = parts.next().and_then(|s| s.parse::<usize>().ok()).unwrap_or(1).max(1) - 1;
                let c = parts.next().and_then(|s| s.parse::<usize>().ok()).unwrap_or(1).max(1) - 1;
                self.cur_row = r.min(ROWS - 1); self.cur_col = c.min(COLS - 1);
            }
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
            b'D' if p.is_empty() => {
                if self.line_cursor > 0 {
                    self.line_cursor -= 1;
                    self.cur_col = self.cur_col.saturating_sub(1);
                }
            }
            b'C' if p.is_empty() => {
                if self.line_cursor < self.line_len {
                    self.line_cursor += 1;
                    self.cur_col = (self.cur_col + 1).min(COLS - 1);
                }
            }
            b'H' if p.is_empty() => {
                let back = self.line_cursor;
                self.line_cursor = 0;
                self.cur_col = self.cur_col.saturating_sub(back);
            }
            b'F' if p.is_empty() => {
                let fwd = self.line_len - self.line_cursor;
                self.line_cursor = self.line_len;
                self.cur_col = (self.cur_col + fwd).min(COLS - 1);
            }
            b'~' if p == "3" => {
                if self.line_cursor < self.line_len {
                    for i in self.line_cursor..self.line_len - 1 {
                        self.line_buf[i] = self.line_buf[i + 1];
                    }
                    self.line_len -= 1;
                    self.redraw_from(self.line_cursor);
                }
            }
            b'A' => { self.cur_row = self.cur_row.saturating_sub(n1()); }
            b'B' => { self.cur_row = (self.cur_row + n1()).min(ROWS - 1); }
            b'C' => { self.cur_col = (self.cur_col + n1()).min(COLS - 1); }
            b'D' => { self.cur_col = self.cur_col.saturating_sub(n1()); }
            _ => {}
        }
        self.dirty = true;
    }

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
        if self.cur_col < COLS {
            self.cells[self.cur_row * COLS + self.cur_col] = self.blank();
            self.cur_col += 1;
        }
        let desired = start_col + self.line_cursor.saturating_sub(from);
        if desired <= self.cur_col { self.cur_col = desired; }
        self.dirty = true;
    }

    pub fn write_output(&mut self, bytes: &[u8]) {
        if let Some(cap) = &mut self.capture {
            cap.extend_from_slice(bytes);
            return;
        }
        for &b in bytes { self.process_byte(b); }
        self.dirty = true;
    }

    pub fn write_input(&self, bytes: &[u8]) { let _ = bytes; }

    fn erase_line_input(&mut self) {
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

    // ── send_key: route to editor or shell ─────────────────────────────────

    pub fn send_key(&mut self, b: u8) {
        if self.editor.is_some() { self.send_key_editor(b); return; }
        if self.km.is_some()     { self.send_key_km(b);     return; }
        // Ctrl+V (0x16): paste clipboard contents as if typed. Strip the
        // terminating newline so the user can review before pressing Enter —
        // shells generally don't expect a trailing \n in pasted input.
        if b == 0x16 {
            if let Some(text) = crate::clipboard::get() {
                let bytes = text.as_bytes();
                let end = if bytes.last() == Some(&b'\n') { bytes.len() - 1 } else { bytes.len() };
                for &byte in &bytes[..end] {
                    // Filter out control bytes that would re-trigger paste or
                    // close the line; pass through printable plus '\t'.
                    if byte == b'\t' || (byte >= 0x20 && byte < 0x7F) {
                        self.send_key(byte);
                    }
                }
            }
            return;
        }
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
                self.parse_and_exec(&line[..line_len]);
                if self.editor.is_none() {
                    let s = format!("\x1b[32mring3\x1b[0m@\x1b[36mrusty-penguin\x1b[90m:{}\x1b[0m$ ", self.cwd);
                    self.write_output(s.as_bytes());
                }
            }
            0x08 | 0x7F => {
                if self.line_cursor > 0 {
                    if self.line_cursor == self.line_len {
                        self.line_len -= 1;
                        self.line_cursor -= 1;
                        self.process_byte(0x08);
                    } else {
                        let from = self.line_cursor - 1;
                        for i in from..self.line_len - 1 { self.line_buf[i] = self.line_buf[i + 1]; }
                        self.line_len -= 1;
                        self.line_cursor -= 1;
                        self.cur_col = self.cur_col.saturating_sub(1);
                        self.redraw_from(self.line_cursor);
                    }
                }
            }
            0x03 => {
                self.write_output(b"^C\r\n");
                self.line_len = 0; self.line_cursor = 0;
                self.hist_pos = self.history.len(); self.saved_line.clear();
                self.write_output(b"\x1b[32mring3\x1b[0m@\x1b[36mrusty-penguin\x1b[90m:~$\x1b[0m ");
            }
            0x0C => {
                let blank = Cell::default();
                for c in self.cells.iter_mut() { *c = blank; }
                self.cur_col = 0; self.cur_row = 0;
                self.write_output(b"\x1b[32mring3\x1b[0m@\x1b[36mrusty-penguin\x1b[90m:~$\x1b[0m ");
                for i in 0..self.line_len { let ch = self.line_buf[i]; self.process_byte(ch); }
                let back = self.line_len - self.line_cursor;
                self.cur_col = self.cur_col.saturating_sub(back);
                self.dirty = true;
            }
            0x01 => {
                let back = self.line_cursor;
                self.line_cursor = 0;
                self.cur_col = self.cur_col.saturating_sub(back);
                self.dirty = true;
            }
            0x05 => {
                let fwd = self.line_len - self.line_cursor;
                self.line_cursor = self.line_len;
                self.cur_col = (self.cur_col + fwd).min(COLS - 1);
                self.dirty = true;
            }
            0x09 => { self.complete_tab(); } // Tab
            b if b >= 0x20 && self.line_len < 255 => {
                if self.line_cursor == self.line_len {
                    self.line_buf[self.line_len] = b;
                    self.line_len += 1;
                    self.line_cursor += 1;
                    self.process_byte(b);
                } else {
                    let from = self.line_cursor;
                    let mut i = self.line_len;
                    while i > from { self.line_buf[i] = self.line_buf[i - 1]; i -= 1; }
                    self.line_buf[from] = b;
                    self.line_len += 1;
                    self.line_cursor += 1;
                    self.redraw_from(from);
                }
            }
            _ => { self.process_byte(b); }
        }
    }

    // ── Editor input handling ────────────────────────────────────────────────

    fn send_key_editor(&mut self, b: u8) {
        let mut ed = match self.editor.take() {
            Some(e) => e,
            None    => return,
        };

        // CSI sequence state machine
        match ed.esc_state {
            1 => {
                ed.esc_state = if b == b'[' { 2 } else { 0 };
                self.editor = Some(ed);
                return;
            }
            2 => {
                ed.esc_state = 0;
                match b {
                    b'A' => ed.move_up(),
                    b'B' => ed.move_down(),
                    b'C' => ed.move_right(),
                    b'D' => ed.move_left(),
                    b'H' => { ed.cursor_col = 0; }
                    b'F' => { ed.cursor_col = ed.line_len(ed.cursor_row); }
                    _    => {}
                }
                ed.clamp_scroll();
                self.editor = Some(ed);
                self.render_editor();
                return;
            }
            _ => {}
        }

        let mut do_save = false;
        let mut do_exit = false;

        match b {
            0x1B => { ed.esc_state = 1; }
            0x13 => { do_save = true; }  // Ctrl+S
            0x18 => { do_exit = true; }  // Ctrl+X
            0x11 => { do_exit = true; }  // Ctrl+Q (force)
            b'\r' | b'\n' => { ed.newline(); }
            0x08 | 0x7F   => { ed.backspace(); }
            b if b >= 0x20 => { ed.insert_char(b); }
            _ => {}
        }

        ed.clamp_scroll();

        if do_save {
            let bytes = ed.to_bytes();
            vfs::vfs().write(&ed.filename, &bytes);
            ed.modified = false;
            self.editor = Some(ed);
            self.render_editor();
        } else if do_exit {
            // Return to shell: clear screen, reprint prompt
            self.editor = None;
            let blank = Cell::default();
            for c in self.cells.iter_mut() { *c = blank; }
            self.cur_col = 0; self.cur_row = 0;
            self.write_output(b"\x1b[32mring3\x1b[0m@\x1b[36mrusty-penguin\x1b[90m:~$\x1b[0m ");
        } else {
            self.editor = Some(ed);
            self.render_editor();
        }
        self.dirty = true;
    }

    // ── Kernel Manager input / render ────────────────────────────────────────

    fn send_key_km(&mut self, b: u8) {
        match b {
            0x18 | 0x03 => {
                // Ctrl+X or Ctrl+C — exit
                self.km = None;
                let blank = Cell::default();
                for c in self.cells.iter_mut() { *c = blank; }
                self.cur_col = 0; self.cur_row = 0;
                self.write_output(b"\x1b[32mring3\x1b[0m@\x1b[36mrusty-penguin\x1b[90m:~$\x1b[0m ");
            }
            0x08 | 0x7F => {
                if let Some(km) = &mut self.km { km.path.pop(); }
                self.render_km();
            }
            b'\r' | b'\n' | 0x13 => {
                // Enter or Ctrl+S — stage kernel
                let path = if let Some(km) = &self.km {
                    String::from(core::str::from_utf8(&km.path).unwrap_or("").trim())
                } else { String::new() };
                if path.is_empty() {
                    if let Some(km) = &mut self.km { km.status = String::from("No path entered."); }
                } else {
                    let manifest = {
                        let mut m = String::from("kernel_elf=");
                        m.push_str(&path);
                        m.push('\n');
                        m.push_str("staged_by=kmanager\n");
                        m
                    };
                    vfs::vfs().write("boot.manifest", manifest.as_bytes());
                    if let Some(km) = &mut self.km {
                        km.status = format!("Staged: {}  ->  iso/boot/kernel.elf", path);
                    }
                }
                self.render_km();
            }
            b if b >= 0x20 => {
                if let Some(km) = &mut self.km {
                    if km.path.len() < 60 { km.path.push(b); }
                }
                self.render_km();
            }
            _ => {}
        }
        self.dirty = true;
    }

    pub fn render_km(&mut self) {
        let km = match &self.km { Some(k) => k, None => return };
        const H_BG: u32 = 0x1D4ED8; const H_FG: u32 = 0xF8FAFC;
        const C_BG: u32 = 0x0F172A; const C_FG: u32 = 0x4ADE80;
        const D_FG: u32 = 0x94A3B8; const Y_FG: u32 = 0xFBBF24;
        const I_FG: u32 = 0x60A5FA; const W_FG: u32 = 0xF8FAFC;

        let fill = |cells: &mut Vec<Cell>, row: usize, fg: u32, bg: u32, text: &[u8]| {
            for col in 0..COLS { cells[row * COLS + col] = Cell { ch: b' ', fg, bg }; }
            for (col, &ch) in text.iter().enumerate().take(COLS) {
                cells[row * COLS + col] = Cell { ch, fg, bg };
            }
        };

        // Row 0 — header
        let hdr = b"  KERNEL MANAGER   RustyPenguin 1.0.0 bare-metal Rust  x86_64";
        fill(&mut self.cells, 0, H_FG, H_BG, hdr);

        // Rows 1-2 — current kernel
        fill(&mut self.cells, 1, C_FG, C_BG, b"");
        fill(&mut self.cells, 2, C_FG, C_BG, b"  CURRENT KERNEL: RustyPenguin v1.0.0  (built-in, cannot be evicted)");

        // Row 3 — blank
        fill(&mut self.cells, 3, C_FG, C_BG, b"");

        // Row 4 — syscall header
        fill(&mut self.cells, 4, Y_FG, C_BG, b"  SYSCALL ABI  (stable: 4 5 7 9 12 13 24   extensions: >=64)");

        // Rows 5-10 — ABI table (two columns)
        const ABI: &[(&[u8], &[u8])] = &[
            (b"  sys  4  ticks       u64 monotonic",    b"  sys  5  meminfo     (free<<32|total) MiB"),
            (b"  sys  7  input_poll  event u64",        b"  sys  9  ps          buf*u8, max_ent"),
            (b"  sys 12  serial      byte u8",          b"  sys 13  rtc         packed BCD u64"),
            (b"  sys 24  yield       sleep->IRQ",       b"  sys >=64            extension range"),
            (b"  ring-3: ELF64 static  e_entry=start", b"  framebuf: 0xFD000000  800x600x24bpp"),
        ];
        for (i, (left, right)) in ABI.iter().enumerate() {
            let row = 5 + i;
            for col in 0..COLS { self.cells[row * COLS + col] = Cell { ch: b' ', fg: D_FG, bg: C_BG }; }
            for (col, &ch) in left.iter().enumerate().take(40) {
                self.cells[row * COLS + col] = Cell { ch, fg: D_FG, bg: C_BG };
            }
            for (col, &ch) in right.iter().enumerate().take(40) {
                self.cells[row * COLS + 40 + col] = Cell { ch, fg: D_FG, bg: C_BG };
            }
        }
        fill(&mut self.cells, 10, C_FG, C_BG, b"");

        // Row 11 — boot manifest
        fill(&mut self.cells, 11, Y_FG, C_BG, b"  BOOT MANIFEST:");
        let manifest_line = match vfs::vfs().read("boot.manifest") {
            Some(d) => {
                let s = core::str::from_utf8(d).unwrap_or("").trim();
                let first_line = s.lines().next().unwrap_or(s);
                format!("  {}", first_line)
            }
            None => String::from("  (none — built-in kernel running)"),
        };
        fill(&mut self.cells, 12, I_FG, C_BG, manifest_line.as_bytes());
        fill(&mut self.cells, 13, C_FG, C_BG, b"");

        // Row 14 — install steps
        fill(&mut self.cells, 14, Y_FG, C_BG, b"  HOW TO SWAP THE KERNEL:");
        fill(&mut self.cells, 15, D_FG, C_BG, b"  1. Build multiboot2 ELF64 (Rust or C - anything goes)");
        fill(&mut self.cells, 16, D_FG, C_BG, b"  2. Enter path below and press Enter (stages boot.manifest)");
        fill(&mut self.cells, 17, D_FG, C_BG, b"  3. On host: bash iso/build.sh  (repacks initramfs + GRUB)");
        fill(&mut self.cells, 18, D_FG, C_BG, b"  4. Boot the new rusty-penguin.iso - your kernel runs ring-0");
        fill(&mut self.cells, 19, C_FG, C_BG, b"");

        // Row 20 — path input
        let mut path_row: Vec<u8> = b"  Path: [".to_vec();
        let km_path = km.path.clone();
        path_row.extend_from_slice(&km_path);
        path_row.push(b'_'); // cursor
        while path_row.len() < 79 { path_row.push(b' '); }
        path_row.push(b']');
        fill(&mut self.cells, 20, W_FG, 0x0B2040, &path_row);

        // Rows 21-22 — blank
        fill(&mut self.cells, 21, C_FG, C_BG, b"");
        fill(&mut self.cells, 22, D_FG, C_BG,
            b"  See docs/syscall-abi.txt  |  github.com/rusty-penguin/kernel-abi");

        // Row 23 — status bar
        let sb_bg = 0x1E3A5Fu32;
        let status_text = if km.status.is_empty() {
            String::from("  Enter to stage   ^X to exit   ^S to stage")
        } else {
            format!("  {}", km.status)
        };
        fill(&mut self.cells, 23, 0xCBD5E1, sb_bg, status_text.as_bytes());

        // Position cursor in path input field
        let path_len = km.path.len();
        self.cur_row = 20;
        self.cur_col = (9 + path_len).min(COLS - 2);
        self.dirty = true;
    }

    // ── Editor renderer (writes directly into self.cells) ───────────────────

    pub fn render_editor(&mut self) {
        let ed = match &self.editor { Some(e) => e, None => return };
        const CONTENT: usize = ROWS - 2;
        const HDR_BG:  u32 = 0x1D4ED8;  // blue header
        const HDR_FG:  u32 = 0xF8FAFC;
        const SB_BG:   u32 = 0x1E3A5F;  // dark status bar
        const SB_FG:   u32 = 0xCBD5E1;

        // Header row (row 0)
        for col in 0..COLS {
            self.cells[col] = Cell { ch: b' ', fg: HDR_FG, bg: HDR_BG };
        }
        let hdr = {
            let mut s = String::from("  nano: ");
            s.push_str(&ed.filename);
            if ed.modified { s.push_str("  [modified]"); }
            s
        };
        for (col, &ch) in hdr.as_bytes().iter().enumerate().take(COLS) {
            self.cells[col] = Cell { ch, fg: HDR_FG, bg: HDR_BG };
        }

        // Content rows (1..ROWS-1)
        for vis in 0..CONTENT {
            let doc_row = ed.scroll_top + vis;
            let term_row = vis + 1;
            // Line number gutter (4 chars wide)
            let gutter_bg = 0x111827u32;
            let gutter_fg = 0x374151u32;
            for col in 0..4usize {
                self.cells[term_row * COLS + col] = Cell { ch: b' ', fg: gutter_fg, bg: gutter_bg };
            }
            if doc_row < ed.lines.len() {
                // Print line number (right-aligned in 3 chars + space)
                let lnum = doc_row + 1;
                let d2 = (lnum % 100 / 10) as u8 + b'0';
                let d3 = (lnum % 10) as u8 + b'0';
                if lnum < 10 {
                    self.cells[term_row * COLS + 2] = Cell { ch: d3, fg: gutter_fg, bg: gutter_bg };
                } else {
                    self.cells[term_row * COLS + 1] = Cell { ch: d2, fg: gutter_fg, bg: gutter_bg };
                    self.cells[term_row * COLS + 2] = Cell { ch: d3, fg: gutter_fg, bg: gutter_bg };
                }
            }
            // Content area (cols 4..COLS)
            for col in 4..COLS {
                self.cells[term_row * COLS + col] = Cell { ch: b' ', fg: DEFAULT_FG, bg: DEFAULT_BG };
            }
            if let Some(line) = ed.lines.get(doc_row) {
                for (i, &ch) in line.iter().enumerate() {
                    let col = i + 4;
                    if col >= COLS { break; }
                    let is_cur = doc_row == ed.cursor_row && i == ed.cursor_col;
                    let (fg, bg) = if is_cur { (DEFAULT_BG, DEFAULT_FG) } else { (DEFAULT_FG, DEFAULT_BG) };
                    self.cells[term_row * COLS + col] = Cell { ch, fg, bg };
                }
                // Block cursor at end of line
                if doc_row == ed.cursor_row && ed.cursor_col == line.len() {
                    let col = line.len() + 4;
                    if col < COLS {
                        self.cells[term_row * COLS + col] = Cell { ch: b' ', fg: DEFAULT_BG, bg: DEFAULT_FG };
                    }
                }
            }
        }

        // Status bar (last row)
        for col in 0..COLS {
            self.cells[(ROWS - 1) * COLS + col] = Cell { ch: b' ', fg: SB_FG, bg: SB_BG };
        }
        let sb = format!("  ^S Save  ^X Exit    Ln {}  Col {}  {}",
            ed.cursor_row + 1, ed.cursor_col + 1,
            if ed.modified { "modified" } else { "        " });
        for (col, &ch) in sb.as_bytes().iter().enumerate().take(COLS) {
            self.cells[(ROWS - 1) * COLS + col] = Cell { ch, fg: SB_FG, bg: SB_BG };
        }

        // Position terminal cursor
        let vis_row = ed.cursor_row.saturating_sub(ed.scroll_top);
        if vis_row < CONTENT {
            self.cur_row = vis_row + 1;
            self.cur_col = (ed.cursor_col + 4).min(COLS - 1);
        }
        self.dirty = true;
    }

    // ── Shell variables ──────────────────────────────────────────────────────

    fn get_var(&self, name: &str) -> String {
        for (k, v) in &self.vars { if k == name { return v.clone(); } }
        match name {
            "SHELL" => String::from("/bin/psh"),
            "HOME"  => String::from("/"),
            "PATH"  => String::from("/bin:/usr/local/bin"),
            "USER"  => String::from("ring3"),
            "OS"    => String::from("RustyPenguin"),
            "ARCH"  => String::from("x86_64"),
            _ => String::new(),
        }
    }

    fn set_var(&mut self, name: &str, value: &str) {
        for (k, v) in &mut self.vars { if k == name { *v = String::from(value); return; } }
        self.vars.push((String::from(name), String::from(value)));
    }

    fn unset_var(&mut self, name: &str) {
        self.vars.retain(|(k, _)| k != name);
    }

    // Expand $VAR, ${VAR}, $?, $(...) in raw bytes
    fn expand_vars(&mut self, raw: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < raw.len() {
            if raw[i] != b'$' { out.push(raw[i]); i += 1; continue; }
            i += 1;
            if i >= raw.len() { out.push(b'$'); continue; }
            // $(...)  — command substitution
            if raw[i] == b'(' {
                i += 1;
                let start = i;
                let mut depth = 1usize;
                while i < raw.len() {
                    if raw[i] == b'(' { depth += 1; }
                    else if raw[i] == b')' { depth -= 1; if depth == 0 { break; } }
                    i += 1;
                }
                let inner: Vec<u8> = raw[start..i].to_vec();
                if i < raw.len() { i += 1; }
                // Run inner command, capture output
                let saved = self.capture.take();
                self.capture = Some(Vec::new());
                self.parse_and_exec(&inner);
                let cap = self.capture.take().unwrap_or_default();
                self.capture = saved;
                let clean = strip_ansi(&cap);
                let end = clean.iter().rposition(|&b| b != b'\n' && b != b'\r' && b != b' ')
                    .map(|j| j + 1).unwrap_or(0);
                out.extend_from_slice(&clean[..end]);
                continue;
            }
            // $?
            if raw[i] == b'?' {
                let s = format!("{}", self.last_exit);
                out.extend_from_slice(s.as_bytes());
                i += 1; continue;
            }
            // $$
            if raw[i] == b'$' { out.extend_from_slice(b"1"); i += 1; continue; }
            // ${VAR}
            if raw[i] == b'{' {
                i += 1;
                let start = i;
                while i < raw.len() && raw[i] != b'}' { i += 1; }
                let name = String::from(core::str::from_utf8(&raw[start..i]).unwrap_or(""));
                let val = self.get_var(&name);
                out.extend_from_slice(val.as_bytes());
                if i < raw.len() { i += 1; }
                continue;
            }
            // $IDENTIFIER
            let start = i;
            while i < raw.len() && (raw[i].is_ascii_alphanumeric() || raw[i] == b'_') { i += 1; }
            if i == start { out.push(b'$'); continue; }
            let name = String::from(core::str::from_utf8(&raw[start..i]).unwrap_or(""));
            let val = self.get_var(&name);
            out.extend_from_slice(val.as_bytes());
        }
        out
    }

    // ── Tab completion ───────────────────────────────────────────────────────

    fn tab_common_prefix(strs: &[String]) -> String {
        if strs.is_empty() { return String::new(); }
        let first = strs[0].as_bytes();
        let mut len = first.len();
        for s in &strs[1..] {
            let n = first.iter().zip(s.as_bytes()).take_while(|(a, b)| a == b).count();
            if n < len { len = n; }
        }
        String::from(core::str::from_utf8(&first[..len]).unwrap_or(""))
    }

    fn complete_tab(&mut self) {
        if self.line_cursor != self.line_len { return; }
        let word_start = self.line_buf[..self.line_len]
            .iter().rposition(|&b| b == b' ').map(|i| i + 1).unwrap_or(0);
        let prefix = core::str::from_utf8(&self.line_buf[word_start..self.line_len]).unwrap_or("");
        let is_cmd = word_start == 0;

        let matches: Vec<String> = if is_cmd {
            const CMDS: &[&str] = &[
                "ai","alias","bc","calc","cat","cd","clear","cp","cut","date","df","du","echo","env","exit",
                "file","find","free","grep","head","help","hexdump","history","hostname","hostid","id",
                "kinstall","kmanager","kver","ls","lscpu","lsb_release","mem","mkdir","mv","nano",
                "neofetch","printf","printenv","ps","psh","pwd","rev","rm","seq","sort","stat","sysinfo",
                "tail","touch","tr","trit","uname","unalias","uniq","uptime","vi","wc","which","whoami","xxd",
            ];
            CMDS.iter().filter(|&&c| c.starts_with(prefix)).map(|&c| String::from(c)).collect()
        } else {
            let ents = vfs::vfs().list();
            ents.iter()
                .filter(|e| e.name.starts_with(prefix))
                .map(|e| if e.is_dir { format!("{}/", e.name) } else { e.name.clone() })
                .collect()
        };

        if matches.is_empty() { return; }

        let common = Self::tab_common_prefix(&matches);
        if common.len() > prefix.len() {
            let addition = common.as_bytes()[prefix.len()..].to_vec();
            for b in addition {
                if self.line_len < 255 {
                    let p = self.line_len;
                    self.line_buf[p] = b;
                    self.line_len += 1;
                    self.line_cursor += 1;
                    self.process_byte(b);
                }
            }
            if matches.len() == 1 && is_cmd && self.line_len < 255 {
                let p = self.line_len;
                self.line_buf[p] = b' ';
                self.line_len += 1;
                self.line_cursor += 1;
                self.process_byte(b' ');
            }
        } else {
            // Show all candidates then redraw prompt + line
            self.write_output(b"\r\n");
            for (i, m) in matches.iter().enumerate() {
                self.write_output(m.as_bytes());
                if i + 1 < matches.len() { self.write_output(b"  "); }
            }
            self.write_output(b"\r\n");
            self.write_output(b"\x1b[32mring3\x1b[0m@\x1b[36mrusty-penguin\x1b[90m:~$\x1b[0m ");
            let ll = self.line_len;
            for i in 0..ll { let b = self.line_buf[i]; self.process_byte(b); }
        }
        self.dirty = true;
    }

    // ── Script executor (for/while/if/then/else/fi/done) ────────────────────

    fn exec_script(&mut self, lines: &[Vec<u8>]) {
        let mut i = 0;
        while i < lines.len() {
            if self.wants_close { break; }
            let raw = lines[i].clone();
            let trimmed = trim_ws(&raw).to_vec();
            if trimmed.is_empty() || trimmed[0] == b'#' { i += 1; continue; }
            let s = String::from(core::str::from_utf8(&trimmed).unwrap_or(""));

            if s.starts_with("for ") {
                // "for VAR in ITEMS" or "for VAR in ITEMS; do"
                let hdr = if let Some(p) = s.find("; do") { &s[..p] } else { s.as_str() };
                let rest = hdr[4..].trim();
                if let Some(in_pos) = rest.find(" in ") {
                    let var_name = String::from(rest[..in_pos].trim());
                    let items_raw = rest[in_pos + 4..].trim().trim_end_matches(';').trim();
                    i += 1;
                    if i < lines.len() && trim_ws(&lines[i]) == b"do" { i += 1; }
                    // Collect body until matching "done"
                    let mut body: Vec<Vec<u8>> = Vec::new();
                    let mut depth = 1usize;
                    while i < lines.len() {
                        let bl = lines[i].clone();
                        let (is_done, is_for_or_while) = {
                            let bt = trim_ws(&bl);
                            let bs = core::str::from_utf8(bt).unwrap_or("");
                            (bs == "done", bs.starts_with("for ") || bs.starts_with("while "))
                        };
                        if is_done {
                            depth -= 1;
                            if depth == 0 { i += 1; break; }
                            body.push(bl);
                        } else {
                            if is_for_or_while { depth += 1; }
                            body.push(bl);
                        }
                        i += 1;
                    }
                    let items_exp = self.expand_vars(items_raw.as_bytes());
                    let items_s = String::from(core::str::from_utf8(&items_exp).unwrap_or(""));
                    let items: Vec<String> = items_s.split_whitespace().map(String::from).collect();
                    for item in &items {
                        if self.wants_close { break; }
                        self.set_var(&var_name, item);
                        self.exec_script(&body);
                    }
                } else {
                    self.write_output(b"\x1b[31mpsh: syntax error in for\x1b[0m\r\n");
                    i += 1;
                }

            } else if s.starts_with("while ") {
                let cond_part = {
                    let c = s[6..].trim();
                    if let Some(p) = c.find("; do") { String::from(c[..p].trim()) } else { String::from(c) }
                };
                i += 1;
                if i < lines.len() && trim_ws(&lines[i]) == b"do" { i += 1; }
                let mut body: Vec<Vec<u8>> = Vec::new();
                let mut depth = 1usize;
                while i < lines.len() {
                    let bl = lines[i].clone();
                    let (is_done, is_loop) = {
                        let bt = trim_ws(&bl);
                        let bs = core::str::from_utf8(bt).unwrap_or("");
                        (bs == "done", bs.starts_with("for ") || bs.starts_with("while "))
                    };
                    if is_done {
                        depth -= 1;
                        if depth == 0 { i += 1; break; }
                        body.push(bl);
                    } else {
                        if is_loop { depth += 1; }
                        body.push(bl);
                    }
                    i += 1;
                }
                loop {
                    if self.wants_close { break; }
                    let cond_exp = self.expand_vars(cond_part.as_bytes());
                    self.last_exit = 0;
                    let saved = self.capture.take();
                    self.capture = Some(Vec::new());
                    self.parse_and_exec(&cond_exp);
                    let _ = self.capture.take();
                    self.capture = saved;
                    if self.last_exit != 0 { break; }
                    self.exec_script(&body);
                }

            } else if s.starts_with("if ") || s == "if" {
                let cond_part = {
                    let c = if s.starts_with("if ") { &s[3..] } else { "" };
                    let c = c.trim();
                    if let Some(p) = c.find("; then") { String::from(c[..p].trim()) } else { String::from(c) }
                };
                i += 1;
                if i < lines.len() && trim_ws(&lines[i]) == b"then" { i += 1; }
                let mut then_body: Vec<Vec<u8>> = Vec::new();
                let mut else_body: Vec<Vec<u8>> = Vec::new();
                let mut in_else = false;
                let mut depth_if = 1usize;
                while i < lines.len() {
                    let bl = lines[i].clone();
                    let (is_fi, is_else, is_if) = {
                        let bt = trim_ws(&bl);
                        let bs = core::str::from_utf8(bt).unwrap_or("");
                        (bs == "fi", bs == "else" && depth_if == 1, bs.starts_with("if "))
                    };
                    if is_fi {
                        depth_if -= 1;
                        if depth_if == 0 { i += 1; break; }
                        if in_else { else_body.push(bl); } else { then_body.push(bl); }
                    } else if is_else {
                        in_else = true;
                        i += 1; continue;
                    } else {
                        if is_if { depth_if += 1; }
                        if in_else { else_body.push(bl); } else { then_body.push(bl); }
                    }
                    i += 1;
                }
                let cond_exp = self.expand_vars(cond_part.as_bytes());
                self.last_exit = 0;
                let saved = self.capture.take();
                self.capture = Some(Vec::new());
                self.parse_and_exec(&cond_exp);
                let _ = self.capture.take();
                self.capture = saved;
                let ok = self.last_exit == 0;
                if ok { self.exec_script(&then_body); }
                else if !else_body.is_empty() { self.exec_script(&else_body); }

            } else {
                // Normal command
                let echo_s = format!("\x1b[90m+ {}\x1b[0m\r\n", s);
                self.write_output(echo_s.as_bytes());
                self.parse_and_exec(&trimmed);
                i += 1;
            }
        }
    }

    // ── Pipe / redirect parser ───────────────────────────────────────────────

    fn parse_and_exec(&mut self, raw: &[u8]) {
        // Variable expansion before any splitting
        let expanded = self.expand_vars(raw);
        let raw = expanded.as_slice();

        // Split on unquoted |
        let mut segs: Vec<&[u8]> = Vec::new();
        let mut start = 0usize;
        for i in 0..raw.len() {
            if raw[i] == b'|' {
                segs.push(trim_ws(&raw[start..i]));
                start = i + 1;
            }
        }
        segs.push(trim_ws(&raw[start..]));
        segs.retain(|s| !s.is_empty());
        if segs.is_empty() { return; }

        self.stdin_data.clear();

        let n = segs.len();
        for idx in 0..n {
            let seg: &[u8] = segs[idx];
            let is_last = idx + 1 == n;

            let (cmd, redir) = if is_last { parse_redir(seg) } else { (seg, None) };
            if cmd.is_empty() { continue; }

            let capturing = !is_last || redir.is_some();
            if capturing { self.capture = Some(Vec::new()); }

            self.exec_command(cmd);

            if !is_last {
                let raw_cap = self.capture.take().unwrap_or_default();
                self.stdin_data = strip_ansi(&raw_cap);
            } else if let Some((fname, append)) = redir {
                let raw_cap = self.capture.take().unwrap_or_default();
                let clean = strip_ansi(&raw_cap);
                if append {
                    let mut existing = vfs::vfs().read(&fname).map(|d| d.to_vec()).unwrap_or_default();
                    existing.extend_from_slice(&clean);
                    vfs::vfs().write(&fname, &existing);
                } else {
                    vfs::vfs().write(&fname, &clean);
                }
                let s = format!("\x1b[90mwrote {} bytes to {}\x1b[0m\r\n", clean.len(), fname);
                self.write_output(s.as_bytes());
            } else {
                self.capture = None;
            }
        }

        self.stdin_data.clear();
    }

    // ── Shell command dispatch ───────────────────────────────────────────────

    fn exec_command(&mut self, line: &[u8]) {
        let mut end = line.len();
        while end > 0 && line[end - 1] == b' ' { end -= 1; }
        let line = &line[..end];
        if line.is_empty() { return; }

        // Helper to extract the argument after the first space
        let arg = |prefix: usize| {
            core::str::from_utf8(&line[prefix..]).unwrap_or("").trim()
        };

        // VAR=value assignment (must have no spaces in VAR part, start with alpha/_)
        if let Some(eq) = line.iter().position(|&b| b == b'=') {
            let lhs = &line[..eq];
            if !lhs.is_empty()
                && (lhs[0].is_ascii_alphabetic() || lhs[0] == b'_')
                && lhs.iter().all(|&b| b.is_ascii_alphanumeric() || b == b'_')
                && !lhs.contains(&b' ')
            {
                let name = core::str::from_utf8(lhs).unwrap_or("");
                let val  = core::str::from_utf8(&line[eq + 1..]).unwrap_or("").trim();
                self.set_var(name, val);
                return;
            }
        }

        self.last_exit = 0; // default: success

        if line == b"help" {
            self.write_output(b"\x1b[36mFiles:\x1b[0m\r\n");
            self.write_output(b"  ls  cat  nano  touch  rm  cp  mv  mkdir  cd  pwd\r\n");
            self.write_output(b"  wc  head  tail  find  xxd  rev  stat  file  du\r\n");
            self.write_output(b"\x1b[36mText filters (pipe-aware):\x1b[0m\r\n");
            self.write_output(b"  grep <pat> [f]   sort [f]   uniq [f]\r\n");
            self.write_output(b"  cut <-f N>  tr <from> <to>\r\n");
            self.write_output(b"\x1b[36mPipes & redirect:\x1b[0m\r\n");
            self.write_output(b"  cmd | cmd     cmd > file     cmd >> file\r\n");
            self.write_output(b"\x1b[36mControl & logic:\x1b[0m\r\n");
            self.write_output(b"  true  false  test <expr>  [<expr>]\r\n");
            self.write_output(b"  if...then...else...fi  while...do...done\r\n");
            self.write_output(b"  for...in...do...done   break  continue\r\n");
            self.write_output(b"\x1b[36mScripting:\x1b[0m\r\n");
            self.write_output(b"  psh <script>       run VFS file as script\r\n");
            self.write_output(b"  seq [s] e [step]   generate number sequence\r\n");
            self.write_output(b"  calc <expr>        arithmetic (+ - * / % ())\r\n");
            self.write_output(b"  sleep <secs>       pause execution\r\n");
            self.write_output(b"\x1b[36mSystem:\x1b[0m\r\n");
            self.write_output(b"  ps  mem  free  df  du  lscpu  sysinfo\r\n");
            self.write_output(b"  uname  whoami  uptime  date  hostname  hostid  id\r\n");
            self.write_output(b"  env  history  which <cmd>  lsb_release\r\n");
            self.write_output(b"  alias [name[=val]]   list or set command aliases\r\n");
            self.write_output(b"  printf fmt [args]    formatted output\r\n");
            self.write_output(b"  trit <n|op a b>      balanced ternary\r\n");
            self.write_output(b"  ai [n]             sparse ternary inference\r\n");
            self.write_output(b"  echo  clear  exit\r\n");
            self.write_output(b"\x1b[36mKernel & Desktop:\x1b[0m\r\n");
            self.write_output(b"  kver               show kernel version + ABI\r\n");
            self.write_output(b"  kinstall <f>       stage custom kernel ELF\r\n");
            self.write_output(b"  kmanager           kernel manager TUI\r\n");
            self.write_output(b"\x1b[90m  Tab completion  Up/Down history  Shift+Ctrl+T new window\x1b[0m\r\n");
            self.write_output(b"\x1b[90m  Ctrl+W close  Ctrl+L clear terminal\x1b[0m\r\n");

        } else if line == b"ls" || line.starts_with(b"ls ") {
            let args = if line == b"ls" { b"" } else { &line[3..] };
            let long_fmt = args.contains(&b'l');
            let show_hidden = args.contains(&b'a');
            let entries = vfs::vfs().list();
            if entries.is_empty() {
                self.write_output(b"\x1b[90m(empty)\x1b[0m\r\n");
            } else {
                let mut count = 0;
                let mut total_size = 0u64;
                for e in entries {
                    if !show_hidden && e.name.starts_with('.') { continue; }
                    if long_fmt {
                        let perms = if e.is_dir { "drwxr-xr-x" } else { "-rw-r--r--" };
                        let size = e.data.len();
                        total_size += size as u64;
                        let s = format!("{} 1 root root {:6} May 28 10:22 {}{}\r\n",
                            perms, size, e.name, if e.is_dir { "/" } else { "" });
                        self.write_output(s.as_bytes());
                    } else {
                        if e.is_dir {
                            let s = format!("\x1b[34m{}/\x1b[0m\r\n", e.name);
                            self.write_output(s.as_bytes());
                        } else {
                            let s = format!("{}\x1b[90m  {} B\x1b[0m\r\n", e.name, e.data.len());
                            self.write_output(s.as_bytes());
                        }
                    }
                    count += 1;
                }
                if long_fmt && count > 0 {
                    let s = format!("total {}\r\n", total_size);
                    self.write_output(s.as_bytes());
                }
            }

        } else if line == b"cat" {
            let data = self.stdin_data.clone();
            for chunk in data.split(|&b| b == b'\n') {
                self.write_output(chunk);
                self.write_output(b"\r\n");
            }

        } else if line.starts_with(b"cat ") {
            let fname = arg(4);
            match vfs::vfs().read(fname) {
                Some(data) => {
                    let data = data.to_vec(); // clone before potential re-borrow
                    for chunk in data.split(|&b| b == b'\n') {
                        self.write_output(chunk);
                        self.write_output(b"\r\n");
                    }
                }
                None => {
                    let s = format!("\x1b[31mcat: {}: no such file\x1b[0m\r\n", fname);
                    self.write_output(s.as_bytes());
                }
            }

        } else if line.starts_with(b"nano ") || line.starts_with(b"vi ") || line.starts_with(b"edit ") {
            let prefix = if line.starts_with(b"nano ") { 5 } else if line.starts_with(b"vi ") { 3 } else { 5 };
            let fname = arg(prefix);
            if fname.is_empty() {
                self.write_output(b"usage: nano <filename>\r\n");
            } else {
                let data = vfs::vfs().read(fname).map(|d| d.to_vec()).unwrap_or_default();
                let ed = EditorState::new(fname, &data);
                self.editor = Some(ed);
                self.render_editor();
            }

        } else if line.starts_with(b"touch ") {
            let fname = arg(6);
            if fname.is_empty() {
                self.write_output(b"usage: touch <filename>\r\n");
            } else {
                if !vfs::vfs().exists(fname) {
                    vfs::vfs().write(fname, b"");
                }
            }

        } else if line.starts_with(b"rm ") {
            let fname = arg(3);
            if !vfs::vfs().delete(fname) {
                let s = format!("\x1b[31mrm: {}: no such file\x1b[0m\r\n", fname);
                self.write_output(s.as_bytes());
            }

        } else if line.starts_with(b"mkdir ") {
            let dname = arg(6);
            if dname.is_empty() {
                self.write_output(b"usage: mkdir <dirname>\r\n");
            } else {
                vfs::vfs().mkdir(dname);
            }

        } else if line.starts_with(b"cp ") {
            let rest = arg(3);
            let mut sp = rest.splitn(2, ' ');
            let src = sp.next().unwrap_or("").trim();
            let dst = sp.next().unwrap_or("").trim();
            if src.is_empty() || dst.is_empty() {
                self.write_output(b"usage: cp <src> <dst>\r\n");
            } else {
                match vfs::vfs().read(src) {
                    Some(d) => { let d = d.to_vec(); vfs::vfs().write(dst, &d); }
                    None    => {
                        let s = format!("\x1b[31mcp: {}: no such file\x1b[0m\r\n", src);
                        self.write_output(s.as_bytes());
                    }
                }
            }

        } else if line.starts_with(b"mv ") {
            let rest = arg(3);
            let mut sp = rest.splitn(2, ' ');
            let src = sp.next().unwrap_or("").trim();
            let dst = sp.next().unwrap_or("").trim();
            if src.is_empty() || dst.is_empty() {
                self.write_output(b"usage: mv <src> <dst>\r\n");
            } else if !vfs::vfs().rename(src, dst) {
                let s = format!("\x1b[31mmv: {}: no such file\x1b[0m\r\n", src);
                self.write_output(s.as_bytes());
            }

        } else if line.starts_with(b"stat ") {
            let fname = arg(5);
            if fname.is_empty() {
                self.write_output(b"usage: stat <filename>\r\n");
            } else {
                let entries = vfs::vfs().list();
                if let Some(e) = entries.iter().find(|ent| ent.name == fname) {
                    let s = format!("  File: {}\r\n", e.name);
                    self.write_output(s.as_bytes());
                    let s = format!("  Size: {} bytes\r\n", e.data.len());
                    self.write_output(s.as_bytes());
                    let s = format!("  Type: {}\r\n", if e.is_dir { "directory" } else { "regular file" });
                    self.write_output(s.as_bytes());
                } else {
                    let s = format!("\x1b[31mstat: {}: no such file\x1b[0m\r\n", fname);
                    self.write_output(s.as_bytes());
                }
            }

        } else if line.starts_with(b"file ") {
            let fname = arg(5);
            if fname.is_empty() {
                self.write_output(b"usage: file <filename>\r\n");
            } else {
                let entries = vfs::vfs().list();
                if let Some(e) = entries.iter().find(|ent| ent.name == fname) {
                    let ftype = if e.is_dir {
                        String::from("directory")
                    } else if fname.ends_with(".txt") {
                        String::from("ASCII text")
                    } else if fname.ends_with(".psh") {
                        String::from("psh script")
                    } else {
                        String::from("data")
                    };
                    let s = format!("{}: {}\r\n", fname, ftype);
                    self.write_output(s.as_bytes());
                } else {
                    let s = format!("\x1b[31mfile: {}: no such file\x1b[0m\r\n", fname);
                    self.write_output(s.as_bytes());
                }
            }

        } else if line.starts_with(b"du ") {
            let fname = arg(3);
            if fname.is_empty() {
                let entries = vfs::vfs().list();
                let mut total = 0u64;
                for e in entries {
                    total += e.data.len() as u64;
                }
                let s = format!("{}K\t.\r\n", (total + 1023) / 1024);
                self.write_output(s.as_bytes());
            } else {
                let entries = vfs::vfs().list();
                if let Some(e) = entries.iter().find(|ent| ent.name == fname) {
                    let kb = (e.data.len() as u64 + 1023) / 1024;
                    let s = format!("{}K\t{}\r\n", kb, fname);
                    self.write_output(s.as_bytes());
                } else {
                    let s = format!("\x1b[31mdu: {}: no such file\x1b[0m\r\n", fname);
                    self.write_output(s.as_bytes());
                }
            }

        } else if line == b"wc" || line.starts_with(b"wc ") {
            let (data, label) = if line.starts_with(b"wc ") {
                let fname = arg(3);
                match vfs::vfs().read(fname) {
                    Some(d) => (d.to_vec(), String::from(fname)),
                    None => {
                        let s = format!("\x1b[31mwc: {}: no such file\x1b[0m\r\n", fname);
                        self.write_output(s.as_bytes());
                        return;
                    }
                }
            } else {
                (self.stdin_data.clone(), String::from("stdin"))
            };
            let bytes = data.len();
            let lines_count = data.iter().filter(|&&b| b == b'\n').count() + 1;
            let words = data.split(|b| *b == b' ' || *b == b'\n' || *b == b'\t')
                .filter(|w| !w.is_empty()).count();
            let s = format!("  {} lines  {} words  {} bytes  {}\r\n", lines_count, words, bytes, label);
            self.write_output(s.as_bytes());

        } else if line == b"head" || line.starts_with(b"head ") {
            let data: Vec<u8> = if line.starts_with(b"head ") {
                let fname = arg(5);
                match vfs::vfs().read(fname) {
                    Some(d) => d.to_vec(),
                    None => {
                        let s = format!("\x1b[31mhead: {}: no such file\x1b[0m\r\n", fname);
                        self.write_output(s.as_bytes());
                        return;
                    }
                }
            } else { self.stdin_data.clone() };
            for chunk in data.split(|&b| b == b'\n').take(10) {
                self.write_output(chunk);
                self.write_output(b"\r\n");
            }

        } else if line == b"tail" || line.starts_with(b"tail ") {
            let data: Vec<u8> = if line.starts_with(b"tail ") {
                let fname = arg(5);
                match vfs::vfs().read(fname) {
                    Some(d) => d.to_vec(),
                    None => {
                        let s = format!("\x1b[31mtail: {}: no such file\x1b[0m\r\n", fname);
                        self.write_output(s.as_bytes());
                        return;
                    }
                }
            } else { self.stdin_data.clone() };
            let all_lines: Vec<&[u8]> = data.split(|&b| b == b'\n').collect();
            let start = all_lines.len().saturating_sub(10);
            for chunk in &all_lines[start..] {
                self.write_output(chunk);
                self.write_output(b"\r\n");
            }

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
        } else if line == b"echo" {
            self.write_output(b"\r\n");
        } else if line.starts_with(b"echo ") {
            self.write_output(&line[5..]);
            self.write_output(b"\r\n");

        } else if line == b"printf" {
            self.write_output(b"");
        } else if line.starts_with(b"printf ") {
            let rest = &line[7..];
            if let Some(first_space) = rest.iter().position(|&b| b == b' ') {
                let fmt = &rest[..first_space];
                let args_part = &rest[first_space + 1..];
                let mut i = 0;
                let mut arg_idx = 0;
                let mut args = Vec::new();
                for chunk in args_part.split(|&b| b == b' ') {
                    if !chunk.is_empty() { args.push(chunk); }
                }
                while i < fmt.len() {
                    if fmt[i] == b'%' && i + 1 < fmt.len() {
                        match fmt[i + 1] {
                            b's' | b'd' if arg_idx < args.len() => {
                                self.write_output(args[arg_idx]);
                                arg_idx += 1;
                                i += 2;
                            }
                            b'%' => {
                                self.write_output(b"%");
                                i += 2;
                            }
                            _ => {
                                self.write_output(&fmt[i..i + 1]);
                                i += 1;
                            }
                        }
                    } else if fmt[i] == b'\\' && i + 1 < fmt.len() {
                        match fmt[i + 1] {
                            b'n' => { self.write_output(b"\n"); i += 2; }
                            b't' => { self.write_output(b"\t"); i += 2; }
                            b'\\' => { self.write_output(b"\\"); i += 2; }
                            _ => { self.write_output(&fmt[i..i + 1]); i += 1; }
                        }
                    } else {
                        self.write_output(&fmt[i..i + 1]);
                        i += 1;
                    }
                }
            }

        } else if line == b"ai" || line.starts_with(b"ai ") {
            let a = if line.starts_with(b"ai ") { &line[3..] } else { b"32" as &[u8] };
            let n: usize = { let mut v = 0usize; for &b in a { if b >= b'0' && b <= b'9' { v = v.wrapping_mul(10).wrapping_add((b - b'0') as usize); } } v.max(1).min(256) };
            self.run_ai_inference(n);
        } else if line == b"trit" {
            self.write_output(b"usage: trit add|sub|mul|neg|cns <a> [b]\r\n");
        } else if line.starts_with(b"trit ") {
            self.exec_trit(&line[5..]);
        } else if line == b"pwd" {
            let s = format!("{}\r\n", self.cwd);
            self.write_output(s.as_bytes());

        } else if line == b"cd" || line == b"cd /" {
            self.cwd = String::from("/");

        } else if line.starts_with(b"cd ") {
            let path = arg(3);
            if path.is_empty() || path == "/" {
                self.cwd = String::from("/");
            } else if vfs::vfs().exists(path) {
                self.cwd = String::from(path);
            } else {
                let s = format!("\x1b[31mcd: {}: no such directory\x1b[0m\r\n", path);
                self.write_output(s.as_bytes());
            }

        } else if line == b"date" {
            let ticks = sys_ticks();
            let secs = ticks / 100;
            let out = format!("uptime {:02}h {:02}m {:02}s\r\n", secs / 3600, (secs % 3600) / 60, secs % 60);
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
            self.write_output(format!("  \x1b[36mMemory\x1b[0m : {}/{} MiB\r\n", used, total).as_bytes());
            self.write_output(format!("  \x1b[36mUptime\x1b[0m : {:02}:{:02}:{:02}\r\n", secs / 3600, (secs % 3600) / 60, secs % 60).as_bytes());
            self.write_output(b"  \x1b[35mModel \x1b[0m : Binary hardware. Ternary mind.\r\n");
        } else if line == b"lsb_release" || line == b"lsb_release -a" {
            self.write_output(b"LSB Version:\t1.0.0\r\n");
            self.write_output(b"Distributor ID:\tRustyPenguin\r\n");
            self.write_output(b"Release:\t1.0.0\r\n");
            self.write_output(b"Codename:\tbinary-hardware\r\n");
            self.write_output(b"Description:\tRustyPenguin 1.0.0 - Bare metal Rust OS\r\n");

        } else if line == b"hostname" {
            self.write_output(b"rusty-penguin\r\n");

        } else if line == b"hostid" {
            self.write_output(b"0x7f000001\r\n");

        } else if line == b"id" {
            self.write_output(b"uid=0(ring3) gid=0(ring3) groups=0(ring3)\r\n");

        } else if line == b"whoami" {
            self.write_output(b"ring3\r\n");

        } else if line == b"kver" || line == b"uname -r" {
            // Show current kernel identity baked into this build
            self.write_output(b"\x1b[32mRustyPenguin\x1b[0m 1.0.0 x86_64\r\n");
            self.write_output(b"kernel: rusty-penguin-kernel (bare metal, Rust)\r\n");
            self.write_output(b"ABI: psh syscall table v1\r\n");
            self.write_output(b"\r\n");
            self.write_output(b"\x1b[90mTo boot a custom kernel:\x1b[0m\r\n");
            self.write_output(b"  1. Build your kernel ELF (multiboot2 header required)\r\n");
            self.write_output(b"  2. Drop it into iso/boot/kernel.elf\r\n");
            self.write_output(b"  3. Run  bash iso/build.sh  to repack\r\n");
            self.write_output(b"  Or use 'kinstall <path>' to stage it via the shell.\r\n");

        } else if line.starts_with(b"kinstall ") {
            // Stage a kernel ELF for next boot. Writes a manifest note into VFS.
            // On bare-metal with no real disk, the VFS entry documents the intent;
            // rebuild the ISO to make it permanent.
            let fname = arg(9);
            if fname.is_empty() {
                self.write_output(b"usage: kinstall <kernel.elf>\r\n");
                self.write_output(b"  Stages a custom kernel for next boot.\r\n");
                self.write_output(b"  Rebuild the ISO after this to make it permanent.\r\n");
            } else {
                // Write a boot manifest into VFS so build tooling can pick it up
                let manifest = {
                    let mut m = String::from("kernel_elf=");
                    m.push_str(fname);
                    m.push('\n');
                    m.push_str("staged_by=psh kinstall\n");
                    m
                };
                vfs::vfs().write("boot.manifest", manifest.as_bytes());
                self.write_output(b"\x1b[32m[kinstall]\x1b[0m Kernel staged:\r\n");
                let s = format!("  source : {}\r\n", fname);
                self.write_output(s.as_bytes());
                self.write_output(b"  target : iso/boot/kernel.elf\r\n");
                self.write_output(b"\r\n");
                self.write_output(b"Next steps to make it permanent:\r\n");
                self.write_output(b"  cp <your-kernel.elf> iso/boot/kernel.elf\r\n");
                self.write_output(b"  bash iso/build.sh\r\n");
                self.write_output(b"\r\n");
                self.write_output(b"\x1b[90mMultiboot2 header required. Syscall ABI:\x1b[0m\r\n");
                self.write_output(b"  sys 4  -> ticks u64\r\n");
                self.write_output(b"  sys 5  -> meminfo (free<<32|total) MiB\r\n");
                self.write_output(b"  sys 7  -> input_poll event u64\r\n");
                self.write_output(b"  sys 12 -> serial_write byte\r\n");
                self.write_output(b"  sys 13 -> rtc packed BCD\r\n");
                self.write_output(b"  sys 24 -> yield (sleep until next IRQ)\r\n");
                self.write_output(b"See docs/syscall-abi.txt for full ABI.\r\n");
            }

        } else if line.starts_with(b"grep ") || line == b"grep" {
            let (pat_vec, data) = if line.starts_with(b"grep ") {
                let rest = core::str::from_utf8(&line[5..]).unwrap_or("").trim();
                let mut sp = rest.splitn(2, ' ');
                let pat = sp.next().unwrap_or("").trim();
                let fname = sp.next().map(|s| s.trim()).unwrap_or("");
                let dat: Vec<u8> = if !fname.is_empty() {
                    match vfs::vfs().read(fname) {
                        Some(d) => d.to_vec(),
                        None => {
                            let s = format!("\x1b[31mgrep: {}: no such file\x1b[0m\r\n", fname);
                            self.write_output(s.as_bytes());
                            return;
                        }
                    }
                } else { self.stdin_data.clone() };
                (pat.as_bytes().to_vec(), dat)
            } else { (Vec::new(), self.stdin_data.clone()) };
            if pat_vec.is_empty() {
                self.write_output(b"usage: grep <pattern> [file]\r\n");
            } else {
                let mut matched = false;
                for chunk in data.split(|&b| b == b'\n') {
                    if !chunk.is_empty() && chunk.windows(pat_vec.len()).any(|w| w == pat_vec.as_slice()) {
                        self.write_output(chunk);
                        self.write_output(b"\r\n");
                        matched = true;
                    }
                }
                if !matched {
                    self.last_exit = 1;
                    if data.is_empty() {
                        self.write_output(b"\x1b[90m(no input - pipe something first)\x1b[0m\r\n");
                    }
                }
            }

        } else if line == b"sort" || line.starts_with(b"sort ") {
            let data: Vec<u8> = if line.starts_with(b"sort ") {
                let fname = arg(5);
                match vfs::vfs().read(fname) {
                    Some(d) => d.to_vec(),
                    None => {
                        let s = format!("\x1b[31msort: {}: no such file\x1b[0m\r\n", fname);
                        self.write_output(s.as_bytes());
                        return;
                    }
                }
            } else { self.stdin_data.clone() };
            let mut lines_v: Vec<Vec<u8>> = data.split(|&b| b == b'\n')
                .filter(|l| !l.is_empty()).map(|l| l.to_vec()).collect();
            lines_v.sort_unstable();
            for l in &lines_v { self.write_output(l); self.write_output(b"\r\n"); }

        } else if line == b"uniq" || line.starts_with(b"uniq ") {
            let data: Vec<u8> = if line.starts_with(b"uniq ") {
                let fname = arg(5);
                match vfs::vfs().read(fname) {
                    Some(d) => d.to_vec(),
                    None => {
                        let s = format!("\x1b[31muniq: {}: no such file\x1b[0m\r\n", fname);
                        self.write_output(s.as_bytes());
                        return;
                    }
                }
            } else { self.stdin_data.clone() };
            let mut prev: Option<Vec<u8>> = None;
            for chunk in data.split(|&b| b == b'\n') {
                if chunk.is_empty() { continue; }
                if prev.as_deref() != Some(chunk) {
                    self.write_output(chunk);
                    self.write_output(b"\r\n");
                    prev = Some(chunk.to_vec());
                }
            }

        } else if line.starts_with(b"cut ") {
            let rest = arg(4);
            let data = if rest.contains('-') {
                self.stdin_data.clone()
            } else {
                let fname = rest;
                match vfs::vfs().read(fname) {
                    Some(d) => d.to_vec(),
                    None => {
                        let s = format!("\x1b[31mcut: {}: no such file\x1b[0m\r\n", fname);
                        self.write_output(s.as_bytes());
                        return;
                    }
                }
            };
            for line in data.split(|&b| b == b'\n') {
                if line.is_empty() { continue; }
                let mut field_num = 1;
                for chunk in line.split(|&b| b == b'\t') {
                    if field_num == 1 {
                        self.write_output(chunk);
                    }
                    field_num += 1;
                }
                self.write_output(b"\r\n");
            }

        } else if line.starts_with(b"tr ") {
            let rest = arg(3);
            let parts: Vec<&str> = rest.split(' ').collect();
            if parts.len() < 2 {
                self.write_output(b"usage: tr <from> <to>\r\n");
                return;
            }
            let from = parts[0];
            let to = parts[1];
            let data = self.stdin_data.clone();
            for &b in &data {
                if let Some(idx) = from.as_bytes().iter().position(|&c| c == b) {
                    if idx < to.len() { self.write_output(&[to.as_bytes()[idx]]); continue; }
                }
                self.write_output(&[b]);
            }

        } else if line == b"find" || line.starts_with(b"find ") {
            let pat = if line.starts_with(b"find ") { arg(5) } else { "" };
            let entries = vfs::vfs().list();
            let mut any = false;
            for e in entries {
                let matches = pat.is_empty()
                    || e.name.as_bytes().windows(pat.len()).any(|w| w == pat.as_bytes());
                if matches {
                    let prefix = if e.is_dir { "\x1b[34m./" } else { "./" };
                    let suffix = if e.is_dir { "/\x1b[0m" } else { "" };
                    let s = format!("{}{}{}\r\n", prefix, e.name, suffix);
                    self.write_output(s.as_bytes());
                    any = true;
                }
            }
            if !any { self.write_output(b"\x1b[90m(no match)\x1b[0m\r\n"); }

        } else if line.starts_with(b"xxd ") || line.starts_with(b"hexdump ") {
            let prefix = if line.starts_with(b"xxd ") { 4usize } else { 8 };
            let fname = arg(prefix);
            match vfs::vfs().read(fname) {
                None => {
                    let s = format!("\x1b[31mxxd: {}: no such file\x1b[0m\r\n", fname);
                    self.write_output(s.as_bytes());
                }
                Some(raw_data) => {
                    let raw_data = raw_data.to_vec();
                    let mut offset = 0usize;
                    for chunk in raw_data.chunks(16) {
                        let mut row = format!("{:08x}: ", offset);
                        for (i, &b) in chunk.iter().enumerate() {
                            if i == 8 { row.push(' '); }
                            let hi = b >> 4; let lo = b & 0xF;
                            row.push(if hi < 10 { (b'0' + hi) as char } else { (b'a' + hi - 10) as char });
                            row.push(if lo < 10 { (b'0' + lo) as char } else { (b'a' + lo - 10) as char });
                            row.push(' ');
                        }
                        // pad short rows
                        for i in 0..(16 - chunk.len()) {
                            if chunk.len() + i == 8 { row.push(' '); }
                            row.push_str("   ");
                        }
                        row.push(' ');
                        for &b in chunk { row.push(if b >= 0x20 && b < 0x7F { b as char } else { '.' }); }
                        row.push_str("\r\n");
                        self.write_output(row.as_bytes());
                        offset += chunk.len();
                    }
                }
            }

        } else if line == b"history" {
            let hist = self.history.clone();
            if hist.is_empty() {
                self.write_output(b"\x1b[90m(no history)\x1b[0m\r\n");
            } else {
                for (i, entry) in hist.iter().enumerate() {
                    let s = format!("  {:3}  {}\r\n", i + 1,
                        core::str::from_utf8(entry).unwrap_or("?"));
                    self.write_output(s.as_bytes());
                }
            }

        } else if line == b"env" || line == b"printenv" {
            self.write_output(b"SHELL=/bin/psh\r\n");
            self.write_output(b"PATH=/bin:/usr/local/bin\r\n");
            self.write_output(b"HOME=/\r\n");
            self.write_output(b"USER=ring3\r\n");
            self.write_output(b"OS=RustyPenguin\r\n");
            self.write_output(b"ARCH=x86_64\r\n");
            self.write_output(b"KERNEL=bare-metal-rust\r\n");
            let vars_clone = self.vars.clone();
            for (k, v) in &vars_clone {
                let s = format!("{}={}\r\n", k, v);
                self.write_output(s.as_bytes());
            }

        } else if line.starts_with(b"export ") {
            let rest = arg(7);
            if let Some(eq) = rest.find('=') {
                let name = rest[..eq].trim();
                let val  = rest[eq + 1..].trim();
                self.set_var(name, val);
            } else if !rest.is_empty() {
                // export existing var (no-op, already global)
                let s = format!("declare -x {}\r\n", rest);
                self.write_output(s.as_bytes());
            } else {
                self.write_output(b"usage: export VAR=value\r\n");
            }

        } else if line.starts_with(b"unset ") {
            let name = arg(6);
            self.unset_var(name);

        } else if line == b"set" {
            // Show all variables
            let vars_clone = self.vars.clone();
            if vars_clone.is_empty() {
                self.write_output(b"\x1b[90m(no variables set)\x1b[0m\r\n");
            } else {
                for (k, v) in &vars_clone {
                    let s = format!("{}={}\r\n", k, v);
                    self.write_output(s.as_bytes());
                }
            }

        } else if line == b"true"  { self.last_exit = 0;
        } else if line == b"false" { self.last_exit = 1;
        } else if line.starts_with(b"test ") || (line.starts_with(b"[") && line.ends_with(b"]")) {
            // Minimal test / [ ... ] implementation
            let args_bytes: &[u8] = if line.starts_with(b"test ") { &line[5..] }
                else { let s = trim_ws(&line[1..]); if s.ends_with(&[b']']) { &s[..s.len()-1] } else { s } };
            let args_str = core::str::from_utf8(args_bytes).unwrap_or("").trim();
            let args: Vec<&str> = args_str.split_whitespace().collect();
            let result: bool = match args.as_slice() {
                ["-f", f]           => vfs::vfs().read(f).is_some(),
                ["-d", f]           => vfs::vfs().list().iter().any(|e| e.is_dir && e.name == *f),
                ["-e", f]           => vfs::vfs().exists(f),
                ["-z", s]           => s.is_empty(),
                ["-n", s]           => !s.is_empty(),
                [s]                 => !s.is_empty(),
                [a, "=",  b2]       => *a == *b2,
                [a, "!=", b2]       => *a != *b2,
                [a, "-eq", b2]      => a.parse::<i64>().ok() == b2.parse::<i64>().ok(),
                [a, "-ne", b2]      => a.parse::<i64>().ok() != b2.parse::<i64>().ok(),
                [a, "-lt", b2]      => a.parse::<i64>().unwrap_or(0) <  b2.parse::<i64>().unwrap_or(0),
                [a, "-gt", b2]      => a.parse::<i64>().unwrap_or(0) >  b2.parse::<i64>().unwrap_or(0),
                [a, "-le", b2]      => a.parse::<i64>().unwrap_or(0) <= b2.parse::<i64>().unwrap_or(0),
                [a, "-ge", b2]      => a.parse::<i64>().unwrap_or(0) >= b2.parse::<i64>().unwrap_or(0),
                _                   => false,
            };
            self.last_exit = if result { 0 } else { 1 };

        } else if line.starts_with(b"for ") {
            // Single-line: for VAR in ITEMS; do CMD; done
            let s = core::str::from_utf8(line).unwrap_or("").trim();
            if let Some(in_pos) = s[4..].find(" in ") {
                let var_name = s[4..4 + in_pos].trim();
                let after_in = s[4 + in_pos + 4..].trim();
                let (items_str, cmd_str) = if let Some(dp) = after_in.find("; do ") {
                    (&after_in[..dp], after_in[dp + 5..].trim_end_matches("; done").trim_end_matches(" done").trim())
                } else { (after_in, "") };
                if !cmd_str.is_empty() {
                    let items: Vec<String> = items_str.split_whitespace().map(String::from).collect();
                    let cmd_owned = cmd_str.as_bytes().to_vec();
                    for item in &items {
                        self.set_var(var_name, item);
                        self.parse_and_exec(&cmd_owned);
                        if self.wants_close { break; }
                    }
                } else {
                    self.write_output(b"usage: for VAR in ITEMS; do CMD; done\r\n");
                }
            } else {
                self.write_output(b"usage: for VAR in ITEMS; do CMD; done\r\n");
            }

        } else if line.starts_with(b"psh ") {
            let fname = arg(4);
            match vfs::vfs().read(fname) {
                None => {
                    let s = format!("\x1b[31mpsh: {}: no such file\x1b[0m\r\n", fname);
                    self.write_output(s.as_bytes());
                    self.last_exit = 1;
                }
                Some(script_raw) => {
                    let script = script_raw.to_vec();
                    let lines: Vec<Vec<u8>> = script.split(|&b| b == b'\n').map(|l| l.to_vec()).collect();
                    self.exec_script(&lines);
                }
            }

        } else if line == b"seq" || line.starts_with(b"seq ") {
            let rest = if line.starts_with(b"seq ") { arg(4) } else { "" };
            let mut parts = rest.split_ascii_whitespace();
            let a = parts.next().and_then(|s| s.parse::<i64>().ok());
            let b2 = parts.next().and_then(|s| s.parse::<i64>().ok());
            let c2 = parts.next().and_then(|s| s.parse::<i64>().ok());
            let (start, end, step) = match (a, b2, c2) {
                (Some(e), None, _)        => (1i64, e, 1i64),
                (Some(s), Some(e), None)  => (s, e, 1),
                (Some(s), Some(e), Some(st)) => (s, e, st),
                _ => { self.write_output(b"usage: seq [start] end [step]\r\n"); return; }
            };
            if step == 0 { self.write_output(b"seq: step cannot be 0\r\n"); return; }
            let mut n = start;
            let mut count = 0usize;
            while (step > 0 && n <= end) || (step < 0 && n >= end) {
                if count >= 10000 { self.write_output(b"...(truncated)\r\n"); break; }
                let s = format!("{}\r\n", n);
                self.write_output(s.as_bytes());
                n = n.wrapping_add(step);
                count += 1;
            }

        } else if line.starts_with(b"bc ") || line.starts_with(b"calc ") || line == b"bc" || line == b"calc" {
            let expr_start = if line.starts_with(b"calc ") { 5usize }
                             else if line.starts_with(b"bc ") { 3 } else { 0 };
            let expr = if expr_start > 0 { &line[expr_start..] } else { b"" as &[u8] };
            if expr.is_empty() {
                self.write_output(b"usage: calc <expression>\r\n");
                self.write_output(b"  supports: + - * / % ( )\r\n");
            } else {
                match calc_add(expr) {
                    Some((val, rest)) if trim_ws(rest).is_empty() => {
                        let s = format!("{}\r\n", val);
                        self.write_output(s.as_bytes());
                    }
                    _ => { self.write_output(b"\x1b[31mcalc: syntax error\x1b[0m\r\n"); }
                }
            }

        } else if line == b"rev" || line.starts_with(b"rev ") {
            let data: Vec<u8> = if line.starts_with(b"rev ") {
                let fname = arg(4);
                match vfs::vfs().read(fname) {
                    Some(d) => d.to_vec(),
                    None => {
                        let s = format!("\x1b[31mrev: {}: no such file\x1b[0m\r\n", fname);
                        self.write_output(s.as_bytes()); return;
                    }
                }
            } else { self.stdin_data.clone() };
            for chunk in data.split(|&b| b == b'\n') {
                if chunk.is_empty() { continue; }
                let mut rev = chunk.to_vec(); rev.reverse();
                self.write_output(&rev); self.write_output(b"\r\n");
            }

        } else if line == b"df" {
            let (free, total) = sys_meminfo();
            self.write_output(b"\x1b[36mFilesystem      Size  Used  Avail  Use%  Mounted on\x1b[0m\r\n");
            let s = format!("vfs (memory)  {:4}M  {:4}M  {:4}M   {}%  /\r\n",
                total, total.saturating_sub(free), free,
                if total > 0 { (total.saturating_sub(free)) * 100 / total } else { 0 });
            self.write_output(s.as_bytes());

        } else if line == b"free" {
            let (free, total) = sys_meminfo();
            let used = total.saturating_sub(free);
            self.write_output(b"\x1b[36m              total        used        free\x1b[0m\r\n");
            let s = format!("Mem:       {:8}M  {:8}M  {:8}M\r\n", total, used, free);
            self.write_output(s.as_bytes());

        } else if line == b"lscpu" {
            self.write_output(b"Architecture:   x86_64\r\n");
            self.write_output(b"CPU op-mode:    64-bit\r\n");
            self.write_output(b"Byte order:     Little Endian\r\n");
            self.write_output(b"Model name:     RustyPenguin vCPU (QEMU/VirtualBox)\r\n");
            self.write_output(b"Scheduler:      bare-metal cooperative\r\n");
            self.write_output(b"Cache:          none (ring-3 only)\r\n");

        } else if line.starts_with(b"which ") {
            let cmd_name = arg(6);
            const BUILTINS: &[&str] = &[
                "ls","cat","nano","vi","edit","touch","rm","mkdir","cp","mv",
                "wc","head","tail","grep","sort","uniq","find","xxd","hexdump",
                "cut","tr","history","env","printenv","which","set","export","unset",
                "ps","mem","free","df","du","lscpu","sysinfo","neofetch","lsb_release",
                "uname","whoami","uptime","date","trit","ai","stat","file","hostname","hostid","id",
                "echo","clear","pwd","cd","exit","kver","kinstall","kmanager",
                "psh","seq","bc","calc","rev","alias","printf","unalias",
                "true","false","sleep","test","[",
            ];
            if BUILTINS.iter().any(|&b| b == cmd_name) {
                let s = format!("psh builtin: {}\r\n", cmd_name);
                self.write_output(s.as_bytes());
            } else {
                let s = format!("\x1b[31m{}: not found\x1b[0m\r\n", cmd_name);
                self.write_output(s.as_bytes());
            }

        } else if line == b"kmanager" {
            self.km = Some(KmState::new());
            self.render_km();

        } else if line == b"alias" {
            let aliases = self.aliases.clone();
            for (name, val) in &aliases {
                let s = format!("{}='{}'\r\n", name, val);
                self.write_output(s.as_bytes());
            }

        } else if line.starts_with(b"alias ") {
            let rest = &line[6..];
            if let Some(eq) = rest.iter().position(|&b| b == b'=') {
                let name = core::str::from_utf8(&rest[..eq]).unwrap_or("");
                let val = core::str::from_utf8(&rest[eq + 1..]).unwrap_or("");
                if !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_') {
                    if let Some(pos) = self.aliases.iter().position(|(n, _)| n == name) {
                        self.aliases[pos].1 = String::from(val);
                    } else {
                        self.aliases.push((String::from(name), String::from(val)));
                    }
                }
            } else {
                let name = core::str::from_utf8(rest).unwrap_or("");
                if let Some((_, val)) = self.aliases.iter().find(|(n, _)| n == name) {
                    let s = format!("{}='{}'\r\n", name, val);
                    self.write_output(s.as_bytes());
                }
            }

        } else if line.starts_with(b"unalias ") {
            let name = core::str::from_utf8(&line[8..]).unwrap_or("");
            self.aliases.retain(|(n, _)| n != name);

        } else if line == b"true" {
            self.last_exit = 0;

        } else if line == b"false" {
            self.last_exit = 1;

        } else if line.starts_with(b"sleep ") {
            let secs_str = arg(6);
            if let Ok(n) = secs_str.parse::<u64>() {
                let target_ticks = sys_ticks() + n * 100;
                while sys_ticks() < target_ticks { sys_yield(); }
            } else {
                self.write_output(b"sleep: invalid argument\r\n");
                self.last_exit = 1;
            }

        } else if line == b"test" || line.starts_with(b"test ") || line == b"[" || line.starts_with(b"[ ") {
            let test_arg = if line == b"test" || line == b"[" {
                ""
            } else if line.starts_with(b"test ") {
                arg(5)
            } else {
                arg(2)
            };
            let mut result = true;
            let parts: Vec<&str> = test_arg.split_whitespace().collect();
            if parts.is_empty() {
                result = false;
            } else if parts.len() == 1 {
                if parts[0] == "-z" { result = true; }
                else if parts[0] == "-n" { result = true; }
                else { result = !parts[0].is_empty(); }
            } else if parts.len() == 2 {
                let flag = parts[0];
                let arg_val = parts[1];
                if flag == "-z" { result = arg_val.is_empty(); }
                else if flag == "-n" { result = !arg_val.is_empty(); }
                else if flag == "-f" { result = vfs::vfs().exists(arg_val); }
                else if flag == "-d" { result = vfs::vfs().exists(arg_val); }
                else if flag == "-e" { result = vfs::vfs().exists(arg_val); }
                else { result = false; }
            } else if parts.len() == 3 {
                let left = parts[0]; let op = parts[1]; let right = parts[2];
                result = match op {
                    "=" | "==" => left == right,
                    "!=" => left != right,
                    "-eq" => left.parse::<i64>().ok().zip(right.parse::<i64>().ok()).map(|(a,b)| a == b).unwrap_or(false),
                    "-ne" => left.parse::<i64>().ok().zip(right.parse::<i64>().ok()).map(|(a,b)| a != b).unwrap_or(false),
                    "-lt" => left.parse::<i64>().ok().zip(right.parse::<i64>().ok()).map(|(a,b)| a < b).unwrap_or(false),
                    "-le" => left.parse::<i64>().ok().zip(right.parse::<i64>().ok()).map(|(a,b)| a <= b).unwrap_or(false),
                    "-gt" => left.parse::<i64>().ok().zip(right.parse::<i64>().ok()).map(|(a,b)| a > b).unwrap_or(false),
                    "-ge" => left.parse::<i64>().ok().zip(right.parse::<i64>().ok()).map(|(a,b)| a >= b).unwrap_or(false),
                    _ => false,
                };
            }
            self.last_exit = if result { 0 } else { 1 };

        } else if line == b"exit" {
            self.write_output(b"bye\r\n");
            self.wants_close = true;
        } else {
            let out = format!("\x1b[31mpsh: command not found:\x1b[0m '{}'\r\n",
                core::str::from_utf8(line).unwrap_or("?"));
            self.write_output(out.as_bytes());
        }
    }

    // ── AI inference ─────────────────────────────────────────────────────────

    fn run_ai_inference(&mut self, n_tokens: usize) {
        const DIM: usize = 8;
        const LAYERS: usize = 4;
        self.write_output(b"albert. [bare metal]\r\n");
        let s_line = format!("sparse ternary inference -- {} layers x {} tokens\r\n", LAYERS, n_tokens);
        self.write_output(s_line.as_bytes());
        self.write_output(b"\r\n");
        let seed = sys_ticks().wrapping_add(n_tokens as u64 * 7);
        let mut w_all = [Trit::Zero; LAYERS * DIM * DIM];
        seed_trits(&mut w_all, seed ^ 0xCAFE_BABE_DEAD_BEEF);
        let mut act = [Trit::Zero; DIM];
        seed_trits(&mut act, seed);
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
            total_total += t; total_skip += sk;
            let dorm = if t > 0 { sk * 100 / t } else { 0 };
            self.write_output(format!("  L{}     [", l).as_bytes());
            for t in &in_act  { self.process_byte(t.to_byte()); }
            self.write_output(b"] -> [");
            for t in &out_act { self.process_byte(t.to_byte()); }
            self.write_output(format!("]  dormancy {}%\r\n", dorm).as_bytes());
            act = out_act;
        }
        let avg_dorm = if total_total > 0 { total_skip * 100 / total_total } else { 0 };
        self.write_output(b"\r\n");
        self.write_output(format!("{} tokens  avg dormancy {}%  skipped {}/{} ops\r\n",
            n_tokens, avg_dorm, total_skip, total_total).as_bytes());
        self.write_output(b"ACTIVE -- Binary hardware. Ternary mind.\r\n");
    }

    // ── Ternary helpers ──────────────────────────────────────────────────────

    fn to_tern(n: i64) -> String {
        if n == 0 { return String::from("0"); }
        let flip = n < 0;
        let mut v = if n < 0 { -n } else { n };
        let mut digits = [0i8; 40]; let mut len = 0;
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
        let mut start = 0usize; let mut in_word = false;
        for i in 0..=args.len() {
            let at_sp = i == args.len() || args[i] == b' ';
            if at_sp && in_word { if count < 3 { parts[count] = &args[start..i]; count += 1; } in_word = false; }
            else if !at_sp && !in_word { start = i; in_word = true; }
        }
        if count == 0 { self.write_output(b"usage: trit add|sub|mul|neg|cns <a> [b]\r\n"); return; }
        let op = parts[0];
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
                self.write_output(format!("{}  ({})\r\n", -a, Self::to_tern(-a)).as_bytes());
            }
            return;
        }
        if count < 3 { self.write_output(b"usage: trit <op> <a> <b>\r\n"); return; }
        let (a, bv) = match (Self::parse_i64(parts[1]), Self::parse_i64(parts[2])) {
            (Some(a), Some(b)) => (a, b),
            _ => { self.write_output(b"bad number\r\n"); return; }
        };
        let r: i64 = if op == b"add" { a + bv } else if op == b"sub" { a - bv }
            else if op == b"mul" { a * bv }
            else if op == b"cns" { if a > 0 && bv > 0 { 1 } else if a < 0 && bv < 0 { -1 } else { 0 } }
            else { self.write_output(b"unknown op\r\n"); return; };
        self.write_output(format!("{}  ({})\r\n", r, Self::to_tern(r)).as_bytes());
    }

    // ── Render ───────────────────────────────────────────────────────────────

    pub fn render(&self, fb: &mut Framebuffer, x: u32, y: u32, w: u32, h: u32, show_cursor: bool) {
        let max_col = (w / 8).min(COLS as u32) as usize;
        let max_row = (h / 8).min(ROWS as u32) as usize;
        for row in 0..max_row {
            for col in 0..max_col {
                let px = x + col as u32 * 8;
                let py = y + row as u32 * 8;
                if px + 8 > fb.width || py + 8 > fb.height { continue; }
                let cell = &self.cells[row * COLS + col];
                fb.draw_char(px, py, cell.ch as char, cell.fg, cell.bg);
            }
        }
        self.paint_cursor(fb, x, y, w, h, show_cursor);
    }

    pub fn paint_cursor(&self, fb: &mut Framebuffer, x: u32, y: u32, w: u32, h: u32, show: bool) {
        let cx = x + self.cur_col as u32 * 8;
        let cy = y + self.cur_row as u32 * 8;
        if cx + 8 > fb.width || cy + 8 > fb.height { return; }
        if cx >= x + w || cy >= y + h { return; }
        let cell = &self.cells[self.cur_row * COLS + self.cur_col];
        if show {
            fb.fill_rect(cx, cy, 8, 8, DEFAULT_FG);
            if cell.ch > b' ' { fb.draw_char(cx, cy, cell.ch as char, DEFAULT_BG, DEFAULT_FG); }
        } else {
            fb.draw_char(cx, cy, cell.ch as char, cell.fg, cell.bg);
        }
    }
}
