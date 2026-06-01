// Graphical text editor for the desktop.
// Simple but fast: text buffer, cursor control, file I/O.

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use crate::ansi::{AnsiParser, Key};
use crate::fb::Framebuffer;
use crate::vfs;

const TAB_WIDTH: usize = 4;
const LINE_H: u32 = 14;       // pixels per line
const CHAR_W: u32 = 8;         // pixels per character
const MARGIN_L: u32 = 8;       // left margin
const MARGIN_T: u32 = 8;       // top margin

/// Format a u32 into a stack buffer (no heap). Returns the &str slice.
fn fmt_u32(buf: &mut [u8; 12], mut n: u32) -> &str {
    if n == 0 { buf[0] = b'0'; return core::str::from_utf8(&buf[..1]).unwrap_or(""); }
    let mut tmp = [0u8; 12]; let mut i = 0;
    while n > 0 { tmp[i] = b'0' + (n % 10) as u8; n /= 10; i += 1; }
    for j in 0..i { buf[j] = tmp[i - 1 - j]; }
    core::str::from_utf8(&buf[..i]).unwrap_or("")
}

/// Width of the line-number gutter for `count` lines (min 3 digits + 1 pad).
fn gutter_width(count: usize) -> u32 {
    let mut digits = 1u32; let mut n = count;
    while n >= 10 { n /= 10; digits += 1; }
    (digits.max(3) + 1) * CHAR_W
}

pub struct TextEditor {
    lines: Vec<String>,         // one String per line
    cursor_line: usize,         // current line (0-indexed)
    cursor_col: usize,          // current column (0-indexed)
    scroll_line: usize,         // first visible line
    filename: String,
    dirty: bool,                // unsaved changes
    pub wants_close: bool,
    ansi: AnsiParser,
}

impl TextEditor {
    pub fn new(filename: &str) -> Self {
        let content = vfs::vfs().read(filename).unwrap_or(&[]);
        let text = core::str::from_utf8(content).unwrap_or("");
        let lines: Vec<String> = text.lines().map(|l| l.to_string()).collect();

        TextEditor {
            lines: if lines.is_empty() { alloc::vec![String::new()] } else { lines },
            cursor_line: 0,
            cursor_col: 0,
            scroll_line: 0,
            filename: filename.to_string(),
            dirty: false,
            wants_close: false,
            ansi: AnsiParser::new(),
        }
    }

    pub fn open(&mut self, filename: &str) {
        *self = TextEditor::new(filename);
    }

    pub fn new_file(&mut self, filename: &str) {
        *self = TextEditor {
            lines: alloc::vec![String::new()],
            cursor_line: 0,
            cursor_col: 0,
            scroll_line: 0,
            filename: filename.to_string(),
            dirty: false,
            wants_close: false,
            ansi: AnsiParser::new(),
        };
    }

    pub fn save(&mut self) {
        let content = self.lines.join("\n");
        vfs::vfs().write(&self.filename, content.as_bytes());
        self.dirty = false;
    }

    pub fn send_key(&mut self, key: u8) {
        match self.ansi.feed(key) {
            Key::None => {}
            Key::Up => {
                if self.cursor_line > 0 { self.cursor_line -= 1; self.clamp_cursor(); }
            }
            Key::Down => {
                if self.cursor_line + 1 < self.lines.len() {
                    self.cursor_line += 1; self.clamp_cursor();
                }
            }
            Key::Right => {
                let line_len = self.lines[self.cursor_line].len();
                if self.cursor_col < line_len {
                    self.cursor_col += 1;
                } else if self.cursor_line + 1 < self.lines.len() {
                    self.cursor_line += 1;
                    self.cursor_col = 0;
                }
            }
            Key::Left => {
                if self.cursor_col > 0 {
                    self.cursor_col -= 1;
                } else if self.cursor_line > 0 {
                    self.cursor_line -= 1;
                    self.cursor_col = self.lines[self.cursor_line].len();
                }
            }
            Key::Home => self.cursor_col = 0,
            Key::End  => self.cursor_col = self.lines[self.cursor_line].len(),
            Key::Delete => {
                let line_len = self.lines[self.cursor_line].len();
                if self.cursor_col < line_len {
                    self.lines[self.cursor_line].remove(self.cursor_col);
                    self.dirty = true;
                } else if self.cursor_line + 1 < self.lines.len() {
                    let next = self.lines.remove(self.cursor_line + 1);
                    self.lines[self.cursor_line].push_str(&next);
                    self.dirty = true;
                }
            }
            Key::Char(0x03) => {
                // Ctrl+C — copy current line to clipboard.
                let line = self.lines[self.cursor_line].clone();
                crate::clipboard::set(&line);
            }
            Key::Char(0x16) => {
                // Ctrl+V — paste clipboard at cursor.
                if let Some(text) = crate::clipboard::get() {
                    for ch in text.chars() {
                        if ch == '\n' {
                            let line = &self.lines[self.cursor_line];
                            let tail = line[self.cursor_col..].to_string();
                            self.lines[self.cursor_line].truncate(self.cursor_col);
                            self.lines.insert(self.cursor_line + 1, tail);
                            self.cursor_line += 1;
                            self.cursor_col = 0;
                        } else if (ch as u32) >= 0x20 && (ch as u32) < 0x7F {
                            self.lines[self.cursor_line].insert(self.cursor_col, ch);
                            self.cursor_col += 1;
                        }
                    }
                    self.dirty = true;
                    self.clamp_cursor();
                }
            }
            Key::Char(b'\n') | Key::Char(b'\r') => {
                let line = &self.lines[self.cursor_line];
                let tail = line[self.cursor_col..].to_string();
                self.lines[self.cursor_line].truncate(self.cursor_col);
                self.lines.insert(self.cursor_line + 1, tail);
                self.cursor_line += 1;
                self.cursor_col = 0;
                self.dirty = true;
            }
            Key::Char(0x08) | Key::Char(0x7F) => {
                if self.cursor_col > 0 {
                    self.lines[self.cursor_line].remove(self.cursor_col - 1);
                    self.cursor_col -= 1;
                    self.dirty = true;
                } else if self.cursor_line > 0 {
                    let line = self.lines.remove(self.cursor_line);
                    self.cursor_line -= 1;
                    self.cursor_col = self.lines[self.cursor_line].len();
                    self.lines[self.cursor_line].push_str(&line);
                    self.dirty = true;
                }
            }
            Key::Char(9) => {
                for _ in 0..TAB_WIDTH {
                    self.lines[self.cursor_line].insert(self.cursor_col, ' ');
                    self.cursor_col += 1;
                }
                self.dirty = true;
            }
            Key::Char(c) if (32..=126).contains(&c) => {
                self.lines[self.cursor_line].insert(self.cursor_col, c as char);
                self.cursor_col += 1;
                self.dirty = true;
            }
            Key::Char(_) => {}
        }
        self.clamp_cursor();
    }

    fn clamp_cursor(&mut self) {
        if self.cursor_line >= self.lines.len() {
            self.cursor_line = self.lines.len() - 1;
        }
        let max_col = self.lines[self.cursor_line].len();
        if self.cursor_col > max_col {
            self.cursor_col = max_col;
        }
    }

    /// Position the cursor at the click point. `(x, y)` are content-relative.
    pub fn on_mouse(&mut self, x: i32, y: i32, buttons: u8) {
        if buttons & 0x01 == 0 { return; }
        let lx = x - (MARGIN_L + gutter_width(self.lines.len())) as i32;
        let ly = y - MARGIN_T as i32;
        if ly < 0 { return; }
        let row = (ly as u32 / LINE_H) as usize;
        let line_idx = self.scroll_line + row;
        if line_idx >= self.lines.len() { return; }
        let col_pixels = lx.max(0) as u32;
        let mut col = (col_pixels / CHAR_W) as usize;
        col = col.min(self.lines[line_idx].len());
        self.cursor_line = line_idx;
        self.cursor_col = col;
    }

    pub fn render(&mut self, fb: &mut Framebuffer, ox: u32, oy: u32, w: u32, h: u32) {
        const STATUS_H: u32 = 16;
        let gutter = gutter_width(self.lines.len());        // numbers + 1 char pad
        let text_x = ox + MARGIN_L + gutter;

        let body_h = h.saturating_sub(STATUS_H);
        let max_lines = (body_h / LINE_H) as usize;

        self.scroll_line = self.cursor_line.saturating_sub(max_lines / 2).min(
            self.lines.len().saturating_sub(max_lines)
        );

        // Gutter background + separator.
        fb.fill_rect(ox, oy, MARGIN_L + gutter, body_h, 0x161A23);
        fb.fill_rect(ox + MARGIN_L + gutter - 2, oy, 1, body_h, 0x2A3040);

        for (i, line) in self.lines.iter().skip(self.scroll_line).take(max_lines).enumerate() {
            let lineno = self.scroll_line + i;
            let y = oy + (i as u32 * LINE_H);
            let on_cursor = lineno == self.cursor_line;
            // Line number (right-aligned in the gutter).
            let mut nb = [0u8; 12];
            let s = fmt_u32(&mut nb, (lineno + 1) as u32);
            let nx = ox + MARGIN_L + gutter - CHAR_W - (s.len() as u32 * CHAR_W);
            let ncol = if on_cursor { 0x8CC6E5 } else { 0x55615A };
            fb.draw_str(nx, y + MARGIN_T, s, ncol, 0x161A23);
            // Text.
            let color = if on_cursor { 0xE8E8E8 } else { 0xB8B8B8 };
            fb.draw_str(text_x, y + MARGIN_T, line, color, 0x1A1A24);
        }

        // Cursor.
        let cy = oy + ((self.cursor_line - self.scroll_line) as u32 * LINE_H) + MARGIN_T;
        let cx = text_x + (self.cursor_col as u32 * CHAR_W);
        if cx >= ox && cx < ox + w && cy >= oy && cy + LINE_H <= oy + body_h {
            for row in 0..LINE_H.min(body_h) {
                if cy + row < oy + body_h {
                    fb.set_pixel(cx, cy + row, 0xF5F5F7);
                }
            }
        }

        // Status bar: Ln/Col, line count, filename, modified marker.
        let sy = oy + h - STATUS_H;
        fb.fill_rect(ox, sy, w, STATUS_H, 0x252E2A);
        let mut b = [0u8; 12];
        let mut sx = ox + 8;
        fb.draw_str_t(sx, sy + 4, "Ln", 0x8A938C); sx += 18;
        fb.draw_str_t(sx, sy + 4, fmt_u32(&mut b, (self.cursor_line + 1) as u32), 0xECEDE5);
        sx += 36;
        fb.draw_str_t(sx, sy + 4, "Col", 0x8A938C); sx += 24;
        fb.draw_str_t(sx, sy + 4, fmt_u32(&mut b, (self.cursor_col + 1) as u32), 0xECEDE5);
        sx += 40;
        fb.draw_str_t(sx, sy + 4, fmt_u32(&mut b, self.lines.len() as u32), 0x6FE18B);
        sx += 28; fb.draw_str_t(sx, sy + 4, "lines", 0x8A938C);
        // Right side: filename + modified dot.
        let fnx = ox + w - 8 - (self.filename.len() as u32 * 6) - 12;
        if self.dirty { fb.draw_str_t(fnx - 10, sy + 4, "*", 0xF5C451); }
        fb.draw_str_t(fnx, sy + 4, &self.filename, 0xA8B0A6);
    }
}
