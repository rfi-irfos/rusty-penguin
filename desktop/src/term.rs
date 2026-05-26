// term.rs — PTY terminal emulator
// Spawns psh in a PTY, processes VT100 output, renders to framebuffer.

use std::os::unix::io::{IntoRawFd, RawFd};
use std::os::unix::process::CommandExt;
use std::process::Child;

use crate::fb::Framebuffer;

pub const COLS: usize = 80;
pub const ROWS: usize = 24;

pub const TERM_PIX_W: u32 = (COLS * 8) as u32; // 640
pub const TERM_PIX_H: u32 = (ROWS * 8) as u32; // 192

const DEFAULT_FG: u32 = 0x4ADE80;
const DEFAULT_BG: u32 = 0x0F172A;

#[derive(Clone, Copy)]
pub struct Cell {
    pub ch: u8,
    pub fg: u32,
    pub bg: u32,
}

impl Default for Cell {
    fn default() -> Self {
        Cell { ch: b' ', fg: DEFAULT_FG, bg: DEFAULT_BG }
    }
}

enum EscState {
    Normal,
    Esc,
    Csi(String),
}

pub struct Terminal {
    pub cells: Vec<Cell>,
    pub cur_col: usize,
    pub cur_row: usize,
    cur_fg: u32,
    cur_bg: u32,
    pub master_fd: RawFd,
    pub child: Child,
    esc: EscState,
    pub dirty: bool,
}

impl Terminal {
    pub fn spawn() -> Result<Self, String> {
        let psh_path = ["/bin/psh", "/usr/local/bin/psh"]
            .iter()
            .find(|p| std::path::Path::new(p).exists())
            .copied()
            .ok_or_else(|| "psh not found".to_string())?;

        let pty = nix::pty::openpty(None, None)
            .map_err(|e| format!("openpty: {}", e))?;

        let master_fd = pty.master.into_raw_fd();
        let slave_fd  = pty.slave.into_raw_fd();

        // Tell psh the terminal size
        unsafe {
            let ws = libc::winsize {
                ws_row:    ROWS as libc::c_ushort,
                ws_col:    COLS as libc::c_ushort,
                ws_xpixel: 0,
                ws_ypixel: 0,
            };
            libc::ioctl(slave_fd, libc::TIOCSWINSZ, &ws);
        }

        let mut cmd = std::process::Command::new(psh_path);
        cmd.env("TERM", "vt100")
           .env("COLUMNS", COLS.to_string())
           .env("LINES", ROWS.to_string());

        // Run in child between fork and exec: become session leader, acquire
        // controlling terminal, wire slave PTY as stdin/stdout/stderr.
        unsafe {
            cmd.pre_exec(move || {
                libc::setsid();
                libc::ioctl(slave_fd, libc::TIOCSCTTY, 0i32);
                libc::dup2(slave_fd, 0);
                libc::dup2(slave_fd, 1);
                libc::dup2(slave_fd, 2);
                if slave_fd > 2 { libc::close(slave_fd); }
                libc::close(master_fd);
                Ok(())
            });
        }

        let child = cmd.spawn().map_err(|e| format!("spawn {}: {}", psh_path, e))?;

        // Parent: close slave, keep master
        unsafe { libc::close(slave_fd); }

        // Non-blocking reads on master so poll() never blocks the render loop
        unsafe {
            let flags = libc::fcntl(master_fd, libc::F_GETFL);
            libc::fcntl(master_fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
        }

        Ok(Terminal {
            cells: vec![Cell::default(); COLS * ROWS],
            cur_col: 0, cur_row: 0,
            cur_fg: DEFAULT_FG, cur_bg: DEFAULT_BG,
            master_fd, child,
            esc: EscState::Normal,
            dirty: true,
        })
    }

    // Drain master fd; returns true if any bytes arrived.
    pub fn poll(&mut self) -> bool {
        let mut buf = [0u8; 512];
        let mut got = false;
        loop {
            let n = unsafe {
                libc::read(self.master_fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len())
            };
            if n <= 0 { break; }
            for i in 0..n as usize { self.process_byte(buf[i]); }
            got = true;
        }
        got
    }

    pub fn write_input(&self, data: &[u8]) {
        if data.is_empty() { return; }
        unsafe {
            libc::write(
                self.master_fd,
                data.as_ptr() as *const libc::c_void,
                data.len(),
            );
        }
    }

    // --- internal helpers ---

    fn scroll_up(&mut self) {
        self.cells.copy_within(COLS..COLS * ROWS, 0);
        let last = ROWS - 1;
        for c in 0..COLS {
            self.cells[last * COLS + c] =
                Cell { ch: b' ', fg: self.cur_fg, bg: self.cur_bg };
        }
    }

    fn put_char(&mut self, ch: u8) {
        if self.cur_col >= COLS {
            self.cur_col = 0;
            if self.cur_row + 1 < ROWS { self.cur_row += 1; }
            else { self.scroll_up(); }
        }
        let idx = self.cur_row * COLS + self.cur_col;
        self.cells[idx] = Cell { ch, fg: self.cur_fg, bg: self.cur_bg };
        self.cur_col += 1;
    }

    fn blank(&self) -> Cell {
        Cell { ch: b' ', fg: self.cur_fg, bg: self.cur_bg }
    }

    fn process_byte(&mut self, b: u8) {
        let state = std::mem::replace(&mut self.esc, EscState::Normal);
        match state {
            EscState::Normal => match b {
                0x07 => {}
                0x08 => { if self.cur_col > 0 { self.cur_col -= 1; } }
                0x09 => {
                    let next = ((self.cur_col / 8) + 1) * 8;
                    while self.cur_col < next.min(COLS) { self.put_char(b' '); }
                }
                0x0A | 0x0B | 0x0C => {
                    if self.cur_row + 1 < ROWS { self.cur_row += 1; }
                    else { self.scroll_up(); }
                }
                0x0D => { self.cur_col = 0; }
                0x1B => { self.esc = EscState::Esc; }
                0x20..=0x7E => { self.put_char(b); }
                _ => {}
            },

            EscState::Esc => match b {
                b'[' => { self.esc = EscState::Csi(String::new()); }
                b'c' => {
                    self.cells = vec![Cell::default(); COLS * ROWS];
                    self.cur_col = 0; self.cur_row = 0;
                    self.cur_fg = DEFAULT_FG; self.cur_bg = DEFAULT_BG;
                }
                _ => {}
            },

            EscState::Csi(mut params) => {
                if b.is_ascii_alphabetic() || b == b'~' {
                    self.handle_csi(&params, b);
                } else {
                    params.push(b as char);
                    self.esc = EscState::Csi(params);
                }
            }
        }
        self.dirty = true;
    }

    fn handle_csi(&mut self, params: &str, cmd: u8) {
        let p = params.trim_start_matches('?');

        let n1 = || p.split(';').next()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(1).max(1);
        let n1_0 = || p.parse::<usize>().unwrap_or(0);

        match cmd {
            b'A' => { self.cur_row = self.cur_row.saturating_sub(n1()); }
            b'B' => { self.cur_row = (self.cur_row + n1()).min(ROWS - 1); }
            b'C' => { self.cur_col = (self.cur_col + n1()).min(COLS - 1); }
            b'D' => { self.cur_col = self.cur_col.saturating_sub(n1()); }
            b'G' => { self.cur_col = (n1().saturating_sub(1)).min(COLS - 1); }

            b'H' | b'f' => {
                let mut parts = p.splitn(2, ';');
                let r = parts.next().and_then(|s| s.parse::<usize>().ok()).unwrap_or(1).max(1) - 1;
                let c = parts.next().and_then(|s| s.parse::<usize>().ok()).unwrap_or(1).max(1) - 1;
                self.cur_row = r.min(ROWS - 1);
                self.cur_col = c.min(COLS - 1);
            }

            b'J' => match n1_0() {
                0 => {
                    let start = self.cur_row * COLS + self.cur_col;
                    let blank = self.blank();
                    for i in start..COLS * ROWS { self.cells[i] = blank; }
                }
                1 => {
                    let end = (self.cur_row * COLS + self.cur_col).min(COLS * ROWS - 1);
                    let blank = self.blank();
                    for i in 0..=end { self.cells[i] = blank; }
                }
                2 | 3 => {
                    let blank = self.blank();
                    self.cells = vec![blank; COLS * ROWS];
                    self.cur_col = 0; self.cur_row = 0;
                }
                _ => {}
            },

            b'K' => {
                let row = self.cur_row;
                let blank = self.blank();
                match n1_0() {
                    0 => for c in self.cur_col..COLS { self.cells[row * COLS + c] = blank; },
                    1 => for c in 0..=self.cur_col.min(COLS-1) { self.cells[row * COLS + c] = blank; },
                    2 => for c in 0..COLS { self.cells[row * COLS + c] = blank; },
                    _ => {}
                }
            }

            b'L' => {
                let n = n1().min(ROWS - self.cur_row);
                for _ in 0..n {
                    for row in (self.cur_row..ROWS-1).rev() {
                        for c in 0..COLS {
                            self.cells[(row+1)*COLS+c] = self.cells[row*COLS+c];
                        }
                    }
                    let blank = self.blank();
                    for c in 0..COLS { self.cells[self.cur_row*COLS+c] = blank; }
                }
            }
            b'M' => {
                let n = n1().min(ROWS - self.cur_row);
                for _ in 0..n {
                    for row in self.cur_row..ROWS-1 {
                        for c in 0..COLS {
                            self.cells[row*COLS+c] = self.cells[(row+1)*COLS+c];
                        }
                    }
                    let blank = self.blank();
                    for c in 0..COLS { self.cells[(ROWS-1)*COLS+c] = blank; }
                }
            }
            b'P' => {
                let n = n1();
                let row = self.cur_row;
                let col = self.cur_col;
                let blank = self.blank();
                for c in col..COLS {
                    self.cells[row*COLS+c] = if c + n < COLS {
                        self.cells[row*COLS+c+n]
                    } else { blank };
                }
            }

            b'm' => {
                if p.is_empty() || p == "0" {
                    self.cur_fg = DEFAULT_FG; self.cur_bg = DEFAULT_BG;
                } else {
                    for seg in p.split(';') {
                        match seg.parse::<u16>().unwrap_or(0) {
                            0  => { self.cur_fg = DEFAULT_FG; self.cur_bg = DEFAULT_BG; }
                            30 => self.cur_fg = 0x1E293B,
                            31 => self.cur_fg = 0xF87171,
                            32 => self.cur_fg = 0x4ADE80,
                            33 => self.cur_fg = 0xFBBF24,
                            34 => self.cur_fg = 0x60A5FA,
                            35 => self.cur_fg = 0xF0ABFC,
                            36 => self.cur_fg = 0x67E8F9,
                            37 => self.cur_fg = 0xF8FAFC,
                            39 => self.cur_fg = DEFAULT_FG,
                            40 => self.cur_bg = 0x000000,
                            41 => self.cur_bg = 0x7F1D1D,
                            42 => self.cur_bg = 0x14532D,
                            43 => self.cur_bg = 0x78350F,
                            44 => self.cur_bg = 0x1E3A5F,
                            45 => self.cur_bg = 0x4A044E,
                            46 => self.cur_bg = 0x164E63,
                            47 => self.cur_bg = 0xF1F5F9,
                            49 => self.cur_bg = DEFAULT_BG,
                            90 => self.cur_fg = 0x475569,
                            91 => self.cur_fg = 0xFF7070,
                            92 => self.cur_fg = 0x86EFAC,
                            93 => self.cur_fg = 0xFDE68A,
                            94 => self.cur_fg = 0x93C5FD,
                            95 => self.cur_fg = 0xF0ABFC,
                            96 => self.cur_fg = 0xA5F3FC,
                            97 => self.cur_fg = 0xFFFFFF,
                            _  => {}
                        }
                    }
                }
            }

            // mode set/reset, scrolling region, device status — ignore
            b'h' | b'l' | b'r' | b'n' | b'c' | b's' | b'u' => {}
            _ => {}
        }
    }

    pub fn render(&self, fb: &mut Framebuffer, x: u32, y: u32) {
        for row in 0..ROWS {
            for col in 0..COLS {
                let cell = &self.cells[row * COLS + col];
                fb.draw_char(
                    x + col as u32 * 8,
                    y + row as u32 * 8,
                    cell.ch as char,
                    cell.fg,
                    cell.bg,
                );
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

impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        unsafe { libc::close(self.master_fd); }
    }
}
