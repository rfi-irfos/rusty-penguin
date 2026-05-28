// Graphical text editor for the desktop.
// Simple but fast: text buffer, cursor control, file I/O.

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use crate::fb::Framebuffer;
use crate::vfs;

const TAB_WIDTH: usize = 4;
const LINE_H: u32 = 14;       // pixels per line
const CHAR_W: u32 = 8;         // pixels per character
const MARGIN_L: u32 = 8;       // left margin
const MARGIN_T: u32 = 8;       // top margin

pub struct TextEditor {
    lines: Vec<String>,         // one String per line
    cursor_line: usize,         // current line (0-indexed)
    cursor_col: usize,          // current column (0-indexed)
    scroll_line: usize,         // first visible line
    filename: String,
    dirty: bool,                // unsaved changes
    pub wants_close: bool,
    // ANSI escape state for arrow / Home / End / Delete keys, which the
    // kernel translates to ESC [ A/B/C/D, ESC [ H, ESC [ F, ESC [ 3 ~.
    // 0 = normal, 1 = saw ESC, 2 = saw ESC [, 3 = saw ESC [ 3
    esc_state: u8,
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
            esc_state: 0,
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
            esc_state: 0,
        };
    }

    pub fn save(&mut self) {
        let content = self.lines.join("\n");
        vfs::vfs().write(&self.filename, content.as_bytes());
        self.dirty = false;
    }

    pub fn send_key(&mut self, key: u8) {
        // ANSI escape state machine: arrows / Home / End / Delete arrive as
        // multi-byte sequences from the kernel keyboard handler.
        match self.esc_state {
            1 => {
                self.esc_state = if key == b'[' { 2 } else { 0 };
                return;
            }
            2 => {
                self.esc_state = 0;
                match key {
                    b'A' => { // Up
                        if self.cursor_line > 0 { self.cursor_line -= 1; self.clamp_cursor(); }
                    }
                    b'B' => { // Down
                        if self.cursor_line + 1 < self.lines.len() {
                            self.cursor_line += 1; self.clamp_cursor();
                        }
                    }
                    b'C' => { // Right
                        let line_len = self.lines[self.cursor_line].len();
                        if self.cursor_col < line_len {
                            self.cursor_col += 1;
                        } else if self.cursor_line + 1 < self.lines.len() {
                            self.cursor_line += 1;
                            self.cursor_col = 0;
                        }
                    }
                    b'D' => { // Left
                        if self.cursor_col > 0 {
                            self.cursor_col -= 1;
                        } else if self.cursor_line > 0 {
                            self.cursor_line -= 1;
                            self.cursor_col = self.lines[self.cursor_line].len();
                        }
                    }
                    b'H' => self.cursor_col = 0,                                        // Home
                    b'F' => self.cursor_col = self.lines[self.cursor_line].len(),       // End
                    b'3' => self.esc_state = 3,                                         // Delete: expect '~'
                    _ => {}
                }
                return;
            }
            3 => {
                self.esc_state = 0;
                if key == b'~' {
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
                return;
            }
            _ => {}
        }
        if key == 0x1B {
            self.esc_state = 1;
            return;
        }

        match key {
            b'\n' | b'\r' => {
                let line = &self.lines[self.cursor_line];
                let tail = line[self.cursor_col..].to_string();
                self.lines[self.cursor_line].truncate(self.cursor_col);
                self.lines.insert(self.cursor_line + 1, tail);
                self.cursor_line += 1;
                self.cursor_col = 0;
                self.dirty = true;
            }
            8 | 127 => {
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
            9 => {
                for _ in 0..TAB_WIDTH {
                    self.lines[self.cursor_line].insert(self.cursor_col, ' ');
                    self.cursor_col += 1;
                }
                self.dirty = true;
            }
            32..=126 => {
                self.lines[self.cursor_line].insert(self.cursor_col, key as char);
                self.cursor_col += 1;
                self.dirty = true;
            }
            _ => {}
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
        let lx = x - MARGIN_L as i32;
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
        let max_lines = (h / LINE_H) as usize;

        self.scroll_line = self.cursor_line.saturating_sub(max_lines / 2).min(
            self.lines.len().saturating_sub(max_lines)
        );

        for (i, line) in self.lines.iter().skip(self.scroll_line).take(max_lines).enumerate() {
            let y = oy + (i as u32 * LINE_H);
            let color = if self.scroll_line + i == self.cursor_line { 0xE8E8E8 } else { 0xB8B8B8 };
            fb.draw_str(ox + MARGIN_L, y + MARGIN_T, line, color, 0x1A1A24);
        }

        // Draw cursor with bounds checking
        let cy = oy + ((self.cursor_line - self.scroll_line) as u32 * LINE_H) + MARGIN_T;
        let cx = ox + MARGIN_L + (self.cursor_col as u32 * CHAR_W);

        // Only draw cursor if it's within window bounds
        if cx >= ox && cx < ox + w && cy >= oy && cy + LINE_H <= oy + h {
            for row in 0..LINE_H.min(h) {
                if cy + row < oy + h {
                    fb.set_pixel(cx, cy + row, 0xF5F5F7);
                }
            }
        }
    }
}
