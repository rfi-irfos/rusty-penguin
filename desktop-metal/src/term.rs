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

// ── Escape-sequence parsing ──────────────────────────────────────────────────

enum EscState { Normal, Esc, Csi(String) }

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
        };
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
        if self.editor.is_some() {
            self.send_key_editor(b);
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
                self.exec_command(&line[..line_len]);
                if self.editor.is_none() {
                    self.write_output(b"\x1b[32mring3\x1b[0m@\x1b[36mrusty-penguin\x1b[90m:~$\x1b[0m ");
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

        if line == b"help" {
            self.write_output(b"\x1b[36mFile system:\x1b[0m\r\n");
            self.write_output(b"  ls           list files\r\n");
            self.write_output(b"  cat <f>      print file\r\n");
            self.write_output(b"  nano <f>     edit file  (^S save  ^X exit)\r\n");
            self.write_output(b"  touch <f>    create empty file\r\n");
            self.write_output(b"  rm <f>       delete file\r\n");
            self.write_output(b"  cp <s> <d>   copy file\r\n");
            self.write_output(b"  mv <s> <d>   rename file\r\n");
            self.write_output(b"  wc <f>       word/line/byte count\r\n");
            self.write_output(b"  head <f>     first 10 lines\r\n");
            self.write_output(b"  tail <f>     last 10 lines\r\n");
            self.write_output(b"  mkdir <d>    create directory\r\n");
            self.write_output(b"\x1b[36mSystem:\x1b[0m\r\n");
            self.write_output(b"  ps  mem  sysinfo  uname  whoami  uptime  date\r\n");
            self.write_output(b"  trit <n|op a b>    balanced ternary\r\n");
            self.write_output(b"  ai [n]             sparse ternary inference\r\n");
            self.write_output(b"  echo  clear  pwd  exit\r\n");
            self.write_output(b"\x1b[36mKernel:\x1b[0m\r\n");
            self.write_output(b"  kver               show kernel version + ABI\r\n");
            self.write_output(b"  kinstall <f>       stage custom kernel ELF\r\n");
            self.write_output(b"\x1b[90m  Ctrl+T new term  Ctrl+W close  Ctrl+L clear\x1b[0m\r\n");
            self.write_output(b"\x1b[90m  Up/Down history  Ctrl+A/E line start/end\x1b[0m\r\n");

        } else if line == b"ls" || line.starts_with(b"ls ") {
            let entries = vfs::vfs().list();
            if entries.is_empty() {
                self.write_output(b"\x1b[90m(empty)\x1b[0m\r\n");
            } else {
                for e in entries {
                    if e.is_dir {
                        let s = format!("\x1b[34m{}/\x1b[0m\r\n", e.name);
                        self.write_output(s.as_bytes());
                    } else {
                        let s = format!("{}\x1b[90m  {} B\x1b[0m\r\n", e.name, e.data.len());
                        self.write_output(s.as_bytes());
                    }
                }
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

        } else if line.starts_with(b"wc ") {
            let fname = arg(3);
            match vfs::vfs().read(fname) {
                Some(data) => {
                    let bytes = data.len();
                    let lines = data.iter().filter(|&&b| b == b'\n').count() + 1;
                    let words = data.split(|b| *b == b' ' || *b == b'\n' || *b == b'\t')
                        .filter(|w| !w.is_empty()).count();
                    let s = format!("  {} lines  {} words  {} bytes  {}\r\n", lines, words, bytes, fname);
                    self.write_output(s.as_bytes());
                }
                None => {
                    let s = format!("\x1b[31mwc: {}: no such file\x1b[0m\r\n", fname);
                    self.write_output(s.as_bytes());
                }
            }

        } else if line.starts_with(b"head ") {
            let fname = arg(5);
            match vfs::vfs().read(fname) {
                Some(data) => {
                    let data = data.to_vec();
                    for chunk in data.split(|&b| b == b'\n').take(10) {
                        self.write_output(chunk);
                        self.write_output(b"\r\n");
                    }
                }
                None => {
                    let s = format!("\x1b[31mhead: {}: no such file\x1b[0m\r\n", fname);
                    self.write_output(s.as_bytes());
                }
            }

        } else if line.starts_with(b"tail ") {
            let fname = arg(5);
            match vfs::vfs().read(fname) {
                Some(data) => {
                    let data = data.to_vec();
                    let all_lines: Vec<&[u8]> = data.split(|&b| b == b'\n').collect();
                    let start = all_lines.len().saturating_sub(10);
                    for chunk in &all_lines[start..] {
                        self.write_output(chunk);
                        self.write_output(b"\r\n");
                    }
                }
                None => {
                    let s = format!("\x1b[31mtail: {}: no such file\x1b[0m\r\n", fname);
                    self.write_output(s.as_bytes());
                }
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
        } else if line == b"ai" || line.starts_with(b"ai ") {
            let a = if line.starts_with(b"ai ") { &line[3..] } else { b"32" as &[u8] };
            let n: usize = { let mut v = 0usize; for &b in a { if b >= b'0' && b <= b'9' { v = v.wrapping_mul(10).wrapping_add((b - b'0') as usize); } } v.max(1).min(256) };
            self.run_ai_inference(n);
        } else if line == b"trit" {
            self.write_output(b"usage: trit add|sub|mul|neg|cns <a> [b]\r\n");
        } else if line.starts_with(b"trit ") {
            self.exec_trit(&line[5..]);
        } else if line == b"pwd" {
            self.write_output(b"/\r\n");
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

    pub fn render(&self, fb: &mut Framebuffer, x: u32, y: u32, show_cursor: bool) {
        for row in 0..ROWS {
            for col in 0..COLS {
                let cell = &self.cells[row * COLS + col];
                fb.draw_char(x + col as u32 * 8, y + row as u32 * 8, cell.ch as char, cell.fg, cell.bg);
            }
        }
        self.paint_cursor(fb, x, y, show_cursor);
    }

    pub fn paint_cursor(&self, fb: &mut Framebuffer, x: u32, y: u32, show: bool) {
        let cx = x + self.cur_col as u32 * 8;
        let cy = y + self.cur_row as u32 * 8;
        if cx + 8 > fb.width || cy + 8 > fb.height { return; }
        let cell = &self.cells[self.cur_row * COLS + self.cur_col];
        if show {
            fb.fill_rect(cx, cy, 8, 8, DEFAULT_FG);
            if cell.ch > b' ' { fb.draw_char(cx, cy, cell.ch as char, DEFAULT_BG, DEFAULT_FG); }
        } else {
            fb.draw_char(cx, cy, cell.ch as char, cell.fg, cell.bg);
        }
    }
}
