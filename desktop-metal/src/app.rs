// Application framework for Rusty Penguin
// Apps implement this trait to be launchable desktop applications.

use crate::fb::Framebuffer;
use alloc::string::String;
use alloc::vec::Vec;

pub trait App {
    /// Render the app's content into the given framebuffer region
    fn render(&mut self, fb: &mut Framebuffer, x: u32, y: u32, w: u32, h: u32);

    /// Handle keyboard input
    fn on_key(&mut self, key: u8);

    /// Handle mouse input (x, y relative to window)
    fn on_mouse(&mut self, x: i32, y: i32, buttons: u8) {}

    /// Check if app wants to close
    fn wants_close(&self) -> bool {
        false
    }

    /// Get app title for window
    fn title(&self) -> &str;
}

struct FileEntry {
    name: String,
    size: u64,
}

/// File manager application
pub struct FileManager {
    cwd: String,
    entries: Vec<FileEntry>,
    selected: usize,
    pub dirty: bool,
    pub wants_close: bool,
}

unsafe fn sys_listdir(path: &[u8], buf: &mut [u8]) -> u64 {
    let mut result: u64;
    core::arch::asm!(
        "syscall",
        in("rax") 14,
        in("rdi") path.as_ptr() as u64,
        in("rsi") path.len() as u64,
        in("rdx") buf.as_mut_ptr() as u64,
        in("rcx") 0u64,
        lateout("rax") result,
        clobber_abi("C"),
    );
    result
}

impl FileManager {
    pub fn new() -> Self {
        let mut fm = FileManager {
            cwd: String::from("/"),
            entries: Vec::new(),
            selected: 0,
            dirty: true,
            wants_close: false,
        };
        fm.refresh();
        fm
    }

    fn refresh(&mut self) {
        self.entries.clear();
        self.selected = 0;

        let mut buf = [0u8; 4096];
        let count = unsafe { sys_listdir(self.cwd.as_bytes(), &mut buf) };

        let mut off = 0usize;
        for _ in 0..count {
            if off >= buf.len() { break; }

            let name_len = buf[off] as usize;
            off += 1;

            if off + name_len > buf.len() { break; }
            let name_bytes = &buf[off..off + name_len];
            let name = String::from_utf8_lossy(name_bytes).into_owned();
            off += name_len;

            if off + 8 > buf.len() { break; }
            let size_bytes = &buf[off..off + 8];
            let size = u64::from_le_bytes([
                size_bytes[0], size_bytes[1], size_bytes[2], size_bytes[3],
                size_bytes[4], size_bytes[5], size_bytes[6], size_bytes[7],
            ]);
            off += 8;

            self.entries.push(FileEntry { name, size });
        }
    }

    fn nav_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
            self.dirty = true;
        }
    }

    fn nav_down(&mut self) {
        if self.selected < self.entries.len().saturating_sub(1) {
            self.selected += 1;
            self.dirty = true;
        }
    }
}

impl App for FileManager {
    fn render(&mut self, fb: &mut Framebuffer, x: u32, y: u32, w: u32, h: u32) {
        // Draw header with current directory
        fb.fill_rect(x, y, w, 24, 0x2C2C38);
        let title = alloc::format!("  {}", self.cwd);
        fb.draw_str(x + 8, y + 7, &title, 0xF5F5F7, 0x2C2C38);

        // Draw column headers
        fb.fill_rect(x, y + 24, w, 18, 0x3C3C48);
        fb.draw_str(x + 8, y + 29, "Name", 0xB8B8B8, 0x3C3C48);
        fb.draw_str(x + 300, y + 29, "Size", 0xB8B8B8, 0x3C3C48);

        // Draw file entries
        for (i, entry) in self.entries.iter().enumerate() {
            let file_y = y + 42 + (i as u32 * 18);
            if file_y + 18 > y + h { break; }

            // Highlight selected entry
            let bg_color = if i == self.selected { 0x4A5568 } else if i % 2 == 0 { 0x1A1A24 } else { 0x232333 };
            fb.fill_rect(x, file_y, w, 18, bg_color);

            let text_color = if i == self.selected { 0xF5F5F7 } else { 0xB8B8B8 };
            fb.draw_str(x + 8, file_y + 4, &entry.name, text_color, bg_color);

            let size_str = alloc::format!("{}", entry.size);
            fb.draw_str(x + 300, file_y + 4, &size_str, text_color, bg_color);
        }

        self.dirty = false;
    }

    fn on_key(&mut self, key: u8) {
        match key {
            0x48 => self.nav_up(),      // Up arrow
            0x50 => self.nav_down(),    // Down arrow
            0x1C => {                   // Enter
                if self.selected < self.entries.len() {
                    let entry = &self.entries[self.selected];
                    let sep = if self.cwd.ends_with('/') { "" } else { "/" };
                    self.cwd = alloc::format!("{}{}{}", self.cwd, sep, entry.name);
                    self.refresh();
                    self.dirty = true;
                }
            }
            0x0E => {                   // Backspace - go up directory
                if self.cwd != "/" {
                    if let Some(pos) = self.cwd.rfind('/') {
                        if pos == 0 {
                            self.cwd = String::from("/");
                        } else {
                            self.cwd.truncate(pos);
                        }
                        self.refresh();
                        self.dirty = true;
                    }
                }
            }
            _ => {}
        }
    }

    fn title(&self) -> &str {
        "File Manager"
    }
}

/// Calendar application
pub struct Calendar {
    pub dirty: bool,
    pub wants_close: bool,
}

impl Calendar {
    pub fn new() -> Self {
        Calendar {
            dirty: true,
            wants_close: false,
        }
    }
}

impl App for Calendar {
    fn render(&mut self, fb: &mut Framebuffer, x: u32, y: u32, w: u32, h: u32) {
        // Draw calendar header
        fb.fill_rect(x, y, w, 30, 0x2C2C38);
        fb.draw_str(x + 8, y + 10, "May 2026", 0xF5F5F7, 0x2C2C38);

        // Draw day headers
        let days = ["Su", "Mo", "Tu", "We", "Th", "Fr", "Sa"];
        let day_width = (w - 16) / 7;
        for (i, day) in days.iter().enumerate() {
            let dx = x + 8 + (i as u32 * day_width);
            fb.draw_str(dx, y + 35, day, 0xB8B8B8, 0x1A1A24);
        }

        // Draw calendar grid (simplified - just draw a 7x5 grid)
        let mut day = 1;
        for week in 0..5 {
            for dow in 0..7 {
                if day > 31 { break; }
                let cx = x + 8 + (dow as u32 * day_width);
                let cy = y + 55 + (week * 20);
                if day == 28 { // Today
                    fb.fill_rect(cx - 2, cy - 2, 16, 14, 0x4A9EFF);
                }
                let day_str = alloc::format!("{}", day);
                fb.draw_str(cx, cy, &day_str, if day == 28 { 0xF5F5F7 } else { 0xB8B8B8 }, 0x1A1A24);
                day += 1;
            }
        }

        self.dirty = false;
    }

    fn on_key(&mut self, _key: u8) {
        // Calendar keyboard handling
    }

    fn title(&self) -> &str {
        "Calendar"
    }
}

/// Settings application
pub struct Settings {
    selected: usize,
    pub dirty: bool,
    pub wants_close: bool,
}

impl Settings {
    pub fn new() -> Self {
        Settings {
            selected: 0,
            dirty: true,
            wants_close: false,
        }
    }
}

impl App for Settings {
    fn render(&mut self, fb: &mut Framebuffer, x: u32, y: u32, w: u32, h: u32) {
        // Draw header
        fb.fill_rect(x, y, w, 24, 0x2C2C38);
        fb.draw_str(x + 8, y + 7, "System Settings", 0xF5F5F7, 0x2C2C38);
        fb.fill_rect(x, y + 24, w, 1, 0x3C3C48);

        // Settings options
        let settings = [
            "Theme: Dark",
            "Window Snap: On",
            "Taskbar: Bottom",
            "Auto-Save: 30s",
        ];

        for (i, setting) in settings.iter().enumerate() {
            let y_pos = y + 32 + (i as u32 * 20);
            if y_pos + 20 > y + h { break; }

            let bg_color = if i == self.selected { 0x4A5568 } else if i % 2 == 0 { 0x1A1A24 } else { 0x232333 };
            fb.fill_rect(x, y_pos, w, 20, bg_color);

            let text_color = if i == self.selected { 0xF5F5F7 } else { 0xB8B8B8 };
            fb.draw_str(x + 12, y_pos + 5, setting, text_color, bg_color);
        }

        self.dirty = false;
    }

    fn on_key(&mut self, key: u8) {
        match key {
            0x48 => { if self.selected > 0 { self.selected -= 1; self.dirty = true; } }
            0x50 => { if self.selected < 3 { self.selected += 1; self.dirty = true; } }
            0x1C => { self.dirty = true; }
            _ => {}
        }
    }

    fn title(&self) -> &str {
        "Settings"
    }
}
