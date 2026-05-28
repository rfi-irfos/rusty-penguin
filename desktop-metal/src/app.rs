// Application framework for Rusty Penguin
// Apps implement this trait to be launchable desktop applications.

use crate::fb::Framebuffer;
use alloc::string::String;

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

/// File manager application
pub struct FileManager {
    cwd: String,
    pub dirty: bool,
    pub wants_close: bool,
}

impl FileManager {
    pub fn new() -> Self {
        FileManager {
            cwd: String::from("/"),
            dirty: true,
            wants_close: false,
        }
    }
}

impl App for FileManager {
    fn render(&mut self, fb: &mut Framebuffer, x: u32, y: u32, w: u32, h: u32) {
        // Draw file list header
        fb.fill_rect(x, y, w, 20, 0x2C2C38);
        fb.draw_str(x + 8, y + 6, "Name", 0xB8B8B8, 0x2C2C38);
        fb.draw_str(x + 200, y + 6, "Size", 0xB8B8B8, 0x2C2C38);

        // Draw divider
        fb.fill_rect(x, y + 20, w, 1, 0x3C3C48);

        // Draw file entries (placeholder)
        let files = ["readme.txt", "motd.txt", "QUICKSTART.txt", "demo.psh"];
        for (i, filename) in files.iter().enumerate() {
            let file_y = y + 24 + (i as u32 * 18);
            if file_y + 18 > y + h { break; }

            // Alternate row colors for readability
            if i % 2 == 0 {
                fb.fill_rect(x, file_y, w, 18, 0x1A1A24);
            }

            fb.draw_str(x + 8, file_y + 5, filename, 0xB8B8B8, 0x1A1A24);
        }

        self.dirty = false;
    }

    fn on_key(&mut self, _key: u8) {
        // File manager keyboard handling
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
