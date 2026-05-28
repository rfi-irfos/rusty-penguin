// Application framework for Rusty Penguin
// Apps implement this trait to be launchable desktop applications.

use crate::fb::Framebuffer;
use crate::vfs;
use alloc::string::String;
use alloc::vec::Vec;

/// Format a u64 into a stack buffer and return the &str slice that points into
/// it. Avoids the heap allocation that `format!("{}", n)` would do — the bump
/// allocator never frees, so every render-path format! was leaking.
fn u64_into<'a>(buf: &'a mut [u8; 24], n: u64) -> &'a str {
    if n == 0 { buf[0] = b'0'; return core::str::from_utf8(&buf[..1]).unwrap_or(""); }
    let mut tmp = [0u8; 24];
    let mut i = 0;
    let mut n = n;
    while n > 0 && i < 24 { tmp[i] = b'0' + (n % 10) as u8; n /= 10; i += 1; }
    for j in 0..i { buf[j] = tmp[i - 1 - j]; }
    core::str::from_utf8(&buf[..i]).unwrap_or("")
}

pub trait App {
    /// Render the app's content into the given framebuffer region
    fn render(&mut self, fb: &mut Framebuffer, x: u32, y: u32, w: u32, h: u32);

    /// Handle keyboard input
    fn on_key(&mut self, key: u8);

    /// Handle mouse input. `(x, y)` are content-relative (0,0 = top-left of
    /// app's drawable area). `(w, h)` are the current content dimensions so
    /// the app can hit-test against the same coordinates it renders in.
    /// `buttons` is the raw button mask at the moment of the click event.
    fn on_mouse(&mut self, _x: i32, _y: i32, _w: u32, _h: u32, _buttons: u8) {}

    /// Check if app wants to close
    fn wants_close(&self) -> bool {
        false
    }

    /// Periodic update tick (called ~100 Hz with the current PIT tick count).
    /// Return `true` if the app's state advanced and it needs a redraw. Apps
    /// that are purely input-driven (most of them) keep the default no-op.
    /// Used by animated apps like Snake. Sparse by default: no redraw unless
    /// something actually changed.
    fn tick(&mut self, _ticks: u64) -> bool {
        false
    }

    /// Get app title for window
    fn title(&self) -> &str;
}

/// Tiny xorshift PRNG — no_std, no heap. Seeded from the PIT tick at launch.
struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self { Rng(seed ^ 0x9E3779B97F4A7C15 | 1) }
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13; x ^= x >> 7; x ^= x << 17;
        self.0 = x; x
    }
    fn range(&mut self, n: u32) -> u32 { (self.next() % n as u64) as u32 }
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
    clipboard: Option<String>,
    pub dirty: bool,
    pub wants_close: bool,
    ansi: crate::ansi::AnsiParser,
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

unsafe fn sys_delete(path: &[u8]) -> u64 {
    let mut result: u64;
    core::arch::asm!(
        "syscall",
        in("rax") 15,
        in("rdi") path.as_ptr() as u64,
        in("rsi") path.len() as u64,
        in("rdx") 0u64,
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
            clipboard: None,
            dirty: true,
            wants_close: false,
            ansi: crate::ansi::AnsiParser::new(),
        };
        fm.refresh();
        fm
    }

    fn copy_selected(&mut self) {
        if self.selected < self.entries.len() {
            let entry = &self.entries[self.selected];
            let sep = if self.cwd.ends_with('/') { "" } else { "/" };
            let path = alloc::format!("{}{}{}", self.cwd, sep, entry.name);
            crate::clipboard::set(&path);
            self.clipboard = Some(path);
            self.dirty = true;
        }
    }

    fn delete_selected(&mut self) {
        if self.selected < self.entries.len() {
            let entry = &self.entries[self.selected];
            let sep = if self.cwd.ends_with('/') { "" } else { "/" };
            let path = alloc::format!("{}{}{}", self.cwd, sep, entry.name);

            // Call sys_delete syscall
            let result = unsafe { sys_delete(path.as_bytes()) };

            if result == 0 {
                // Success - refresh the directory listing
                self.refresh();
                self.dirty = true;
            }
            // If result != 0, deletion failed (file not found, etc)
        }
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
        // Draw header with current directory — draw directly, no format!()
        fb.fill_rect(x, y, w, 24, 0x2C2C38);
        fb.draw_str(x + 24, y + 7, &self.cwd, 0xF5F5F7, 0x2C2C38);

        // Draw column headers
        fb.fill_rect(x, y + 24, w, 18, 0x3C3C48);
        fb.draw_str(x + 8, y + 29, "Name", 0xB8B8B8, 0x3C3C48);
        fb.draw_str(x + 300, y + 29, "Size", 0xB8B8B8, 0x3C3C48);

        // Draw file entries
        let mut sbuf = [0u8; 24];
        for (i, entry) in self.entries.iter().enumerate() {
            let file_y = y + 42 + (i as u32 * 18);
            if file_y + 18 > y + h { break; }

            // Highlight selected entry
            let bg_color = if i == self.selected { 0x4A5568 } else if i % 2 == 0 { 0x1A1A24 } else { 0x232333 };
            fb.fill_rect(x, file_y, w, 18, bg_color);

            let text_color = if i == self.selected { 0xF5F5F7 } else { 0xB8B8B8 };
            fb.draw_str(x + 8, file_y + 4, &entry.name, text_color, bg_color);

            let size_str = u64_into(&mut sbuf, entry.size);
            fb.draw_str(x + 300, file_y + 4, size_str, text_color, bg_color);
        }

        self.dirty = false;
    }

    fn on_key(&mut self, key: u8) {
        use crate::ansi::Key as AK;
        match self.ansi.feed(key) {
            AK::Up   => self.nav_up(),
            AK::Down => self.nav_down(),
            AK::Char(b'\n') | AK::Char(b'\r') => {
                // Enter — descend into the selected directory.
                if self.selected < self.entries.len() {
                    let entry = &self.entries[self.selected];
                    let sep = if self.cwd.ends_with('/') { "" } else { "/" };
                    self.cwd = alloc::format!("{}{}{}", self.cwd, sep, entry.name);
                    self.refresh();
                    self.dirty = true;
                }
            }
            AK::Char(0x08) | AK::Char(0x7F) => {
                // Backspace — go up a directory.
                if self.cwd != "/" {
                    if let Some(pos) = self.cwd.rfind('/') {
                        if pos == 0 { self.cwd = String::from("/"); }
                        else        { self.cwd.truncate(pos); }
                        self.refresh();
                        self.dirty = true;
                    }
                }
            }
            AK::Char(b'c') | AK::Char(b'C') => self.copy_selected(),
            AK::Char(b'd') | AK::Char(b'D') => self.delete_selected(),
            _ => {}
        }
    }

    fn on_mouse(&mut self, _x: i32, y: i32, _w: u32, _h: u32, buttons: u8) {
        if buttons & 0x01 == 0 { return; }
        // Rows start at y+42, 18px tall. Mirrors render layout above.
        let row_y = y - 42;
        if row_y < 0 { return; }
        let row = (row_y / 18) as usize;
        if row < self.entries.len() && row != self.selected {
            self.selected = row;
            self.dirty = true;
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
        let mut day: u32 = 1;
        let mut sbuf = [0u8; 24];
        for week in 0..5 {
            for dow in 0..7 {
                if day > 31 { break; }
                let cx = x + 8 + (dow as u32 * day_width);
                let cy = y + 55 + (week * 20);
                if day == 28 { // Today
                    fb.fill_rect(cx - 2, cy - 2, 16, 14, 0x4A9EFF);
                }
                let day_str = u64_into(&mut sbuf, day as u64);
                fb.draw_str(cx, cy, day_str, if day == 28 { 0xF5F5F7 } else { 0xB8B8B8 }, 0x1A1A24);
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
    theme: bool,           // true = dark, false = light
    window_snap: bool,
    taskbar_bottom: bool,
    auto_save_enabled: bool,
    auto_save_interval: u32, // seconds
    ansi: crate::ansi::AnsiParser,
}

impl Settings {
    pub fn new() -> Self {
        let mut s = Settings {
            selected: 0,
            dirty: true,
            wants_close: false,
            theme: true,
            window_snap: true,
            taskbar_bottom: true,
            auto_save_enabled: true,
            auto_save_interval: 30,
            ansi: crate::ansi::AnsiParser::new(),
        };
        s.load_from_disk();
        s
    }

    fn load_from_disk(&mut self) {
        // Try to load from VFS (.config/rusty-penguin/settings.ini)
        if let Some(data) = vfs::vfs().read(".config/rusty-penguin/settings.ini") {
            // Parse key=value pairs
            let content = core::str::from_utf8(data).unwrap_or("");
            for line in content.lines() {
                if let Some(eq_pos) = line.find('=') {
                    let key = &line[..eq_pos];
                    let val = &line[eq_pos + 1..];
                    match key {
                        "theme" => self.theme = val == "dark",
                        "window_snap" => self.window_snap = val == "true",
                        "taskbar_bottom" => self.taskbar_bottom = val == "true",
                        "auto_save_enabled" => self.auto_save_enabled = val == "true",
                        "auto_save_interval" => {
                            if let Ok(n) = val.parse::<u32>() {
                                self.auto_save_interval = n;
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    fn save_to_disk(&self) {
        // Save settings to VFS (.config/rusty-penguin/settings.ini)
        // Format: key=value pairs, one per line
        let config = alloc::format!(
            "theme={}\nwindow_snap={}\ntaskbar_bottom={}\nauto_save_enabled={}\nauto_save_interval={}\n",
            if self.theme { "dark" } else { "light" },
            self.window_snap,
            self.taskbar_bottom,
            self.auto_save_enabled,
            self.auto_save_interval
        );

        let vfs = vfs::vfs();

        // Ensure directory exists
        if !vfs.exists(".config") {
            vfs.mkdir(".config");
        }
        if !vfs.exists(".config/rusty-penguin") {
            vfs.mkdir(".config/rusty-penguin");
        }

        // Write settings file
        vfs.write(".config/rusty-penguin/settings.ini", config.as_bytes());
    }

    fn toggle_selected(&mut self) {
        match self.selected {
            0 => { self.theme = !self.theme; }
            1 => { self.window_snap = !self.window_snap; }
            2 => { self.taskbar_bottom = !self.taskbar_bottom; }
            3 => { self.auto_save_enabled = !self.auto_save_enabled; }
            _ => {}
        }
        self.save_to_disk();
        self.dirty = true;
    }
}

impl App for Settings {
    fn render(&mut self, fb: &mut Framebuffer, x: u32, y: u32, w: u32, h: u32) {
        // Draw header
        fb.fill_rect(x, y, w, 24, 0x2C2C38);
        fb.draw_str(x + 8, y + 7, "System Settings", 0xF5F5F7, 0x2C2C38);
        fb.fill_rect(x, y + 24, w, 1, 0x3C3C48);

        // Settings rows — labels + values drawn separately to avoid format!()
        let labels = ["Theme:", "Window Snap:", "Taskbar:", "Auto-Save:"];
        let theme_v   = if self.theme { "Dark" } else { "Light" };
        let snap_v    = if self.window_snap { "On" } else { "Off" };
        let taskbar_v = if self.taskbar_bottom { "Bottom" } else { "Top" };

        let mut sbuf = [0u8; 24];
        let interval_str = u64_into(&mut sbuf, self.auto_save_interval as u64);

        for i in 0..4 {
            let y_pos = y + 32 + (i as u32 * 20);
            if y_pos + 20 > y + h { break; }

            let bg_color = if i == self.selected { 0x4A5568 } else if i % 2 == 0 { 0x1A1A24 } else { 0x232333 };
            fb.fill_rect(x, y_pos, w, 20, bg_color);
            let text_color = if i == self.selected { 0xF5F5F7 } else { 0xB8B8B8 };

            // Label
            fb.draw_str(x + 12, y_pos + 5, labels[i], text_color, bg_color);
            // Value (offset 140px right of label)
            let vx = x + 140;
            match i {
                0 => fb.draw_str(vx, y_pos + 5, theme_v, text_color, bg_color),
                1 => fb.draw_str(vx, y_pos + 5, snap_v, text_color, bg_color),
                2 => fb.draw_str(vx, y_pos + 5, taskbar_v, text_color, bg_color),
                3 => if self.auto_save_enabled {
                    fb.draw_str(vx, y_pos + 5, interval_str, text_color, bg_color);
                    fb.draw_str(vx + (interval_str.len() as u32 * 8), y_pos + 5, "s", text_color, bg_color);
                } else {
                    fb.draw_str(vx, y_pos + 5, "Off", text_color, bg_color);
                },
                _ => {}
            }
        }

        // Draw hint at bottom
        let hint = "(UP/DOWN to select, ENTER to toggle)";
        if y + h > 100 {
            fb.draw_str(x + 12, y + h - 24, hint, 0x808080, 0x0A0E27);
        }

        self.dirty = false;
    }

    fn on_key(&mut self, key: u8) {
        use crate::ansi::Key as AK;
        match self.ansi.feed(key) {
            AK::Up   => { if self.selected > 0 { self.selected -= 1; self.dirty = true; } }
            AK::Down => { if self.selected < 3 { self.selected += 1; self.dirty = true; } }
            AK::Char(b'\n') | AK::Char(b'\r') => { self.toggle_selected(); }
            _ => {}
        }
    }

    fn title(&self) -> &str {
        "Settings"
    }
}

/// TIS Console for ternary inference
pub struct TisConsole {
    input_buffer: String,
    output_lines: Vec<String>,
    // Lines scrolled back from the bottom. 0 = pinned to the latest output.
    scroll_offset: usize,
    pub dirty: bool,
    pub wants_close: bool,
    ansi: crate::ansi::AnsiParser,
}

impl TisConsole {
    pub fn new() -> Self {
        let mut console = TisConsole {
            input_buffer: String::new(),
            output_lines: Vec::new(),
            scroll_offset: 0,
            dirty: true,
            wants_close: false,
            ansi: crate::ansi::AnsiParser::new(),
        };
        console.output_lines.push(String::from("TIS Console v1.5.0"));
        console.output_lines.push(String::from("Commands: trit, mul, div"));
        console.output_lines.push(String::from("> "));
        console
    }

    fn execute_command(&mut self) {
        let cmd = self.input_buffer.trim();
        if cmd.is_empty() { return; }

        self.output_lines.push(alloc::format!("> {}", cmd));

        if cmd == "help" {
            self.output_lines.push(String::from("  Ternary operations:"));
            self.output_lines.push(String::from("    trit <n>       - convert to balanced ternary"));
            self.output_lines.push(String::from("    mul <a> <b>    - ternary multiplication"));
            self.output_lines.push(String::from("    div <a> <b>    - ternary division"));
            self.output_lines.push(String::from("    infer <prompt> - TIS inference (demo)"));
            self.output_lines.push(String::from("    status         - system status"));
        } else if cmd.starts_with("trit ") {
            if let Ok(n) = cmd[5..].parse::<i32>() {
                self.output_lines.push(alloc::format!("  {} → balanced ternary", n));
            }
        } else if cmd.starts_with("mul ") {
            let parts: Vec<&str> = cmd[4..].split_whitespace().collect();
            if parts.len() >= 2 {
                if let (Ok(a), Ok(b)) = (parts[0].parse::<i32>(), parts[1].parse::<i32>()) {
                    let result = a.wrapping_mul(b);
                    self.output_lines.push(alloc::format!("  {} × {} = {}", a, b, result));
                }
            }
        } else if cmd.starts_with("div ") {
            let parts: Vec<&str> = cmd[4..].split_whitespace().collect();
            if parts.len() >= 2 {
                if let (Ok(a), Ok(b)) = (parts[0].parse::<i32>(), parts[1].parse::<i32>()) {
                    if b != 0 {
                        self.output_lines.push(alloc::format!("  {} ÷ {} = {} (r {})", a, b, a/b, a%b));
                    } else {
                        self.output_lines.push(String::from("  Error: division by zero"));
                    }
                }
            }
        } else if cmd.starts_with("infer ") {
            let prompt = &cmd[6..];
            self.output_lines.push(String::from("  Ternary inference engine (TIS v1.5.0)"));
            self.output_lines.push(alloc::format!("  Prompt: \"{}\"", prompt));
            self.output_lines.push(String::from("  [Loading model...] albert-moe-13-26L"));
            self.output_lines.push(String::from("  [Inference] 83.2 tok/s | sparsity: 12.3%"));
            self.output_lines.push(String::from("  [Complete] response ready for display"));
        } else if cmd == "status" {
            self.output_lines.push(String::from("  Rusty Penguin TIS Console v1.5.0"));
            self.output_lines.push(String::from("  Kernel: Rust 64-bit (bare-metal)"));
            self.output_lines.push(String::from("  Memory: ~512 MiB available"));
            self.output_lines.push(String::from("  TIS: Ready (offline demo mode)"));
        } else if cmd == "clear" {
            self.output_lines.clear();
            self.output_lines.push(String::from("TIS Console v1.5.0"));
            self.output_lines.push(String::from("Type 'help' for commands"));
            self.output_lines.push(String::from("> "));
            self.input_buffer.clear();
            self.dirty = true;
            return;
        } else {
            self.output_lines.push(String::from("  Unknown command. Type 'help' for available commands."));
        }

        self.input_buffer.clear();
        self.output_lines.push(String::from("> "));
        self.dirty = true;
    }
}

impl App for TisConsole {
    fn render(&mut self, fb: &mut Framebuffer, x: u32, y: u32, w: u32, h: u32) {
        // Draw header
        fb.fill_rect(x, y, w, 24, 0x1A1A2E);
        fb.draw_str(x + 8, y + 7, "TIS Console", 0x4A9EFF, 0x1A1A2E);
        fb.fill_rect(x, y + 24, w, 1, 0x2C3E50);

        // Draw output. scroll_offset shifts the window of visible lines up
        // from the latest. Clamp to the buffer size so it can't overscroll.
        let line_h = 13u32;
        let max_lines = ((h - 44) / line_h) as usize;
        let total = self.output_lines.len();
        let max_off = total.saturating_sub(max_lines);
        if self.scroll_offset > max_off { self.scroll_offset = max_off; }
        let end = total.saturating_sub(self.scroll_offset);
        let start = end.saturating_sub(max_lines);

        for (i, line) in self.output_lines[start..end].iter().enumerate() {
            let y_pos = y + 28 + (i as u32 * line_h);
            if y_pos + line_h > y + h - 20 { break; }
            fb.draw_str(x + 8, y_pos, line, 0x00FF00, 0x0A0E27);
        }

        // Scroll indicator on the right edge when not at the bottom.
        if self.scroll_offset > 0 && w > 12 {
            let indicator_x = x + w - 8;
            fb.draw_str(indicator_x, y + 28, "^", 0x6B7280, 0x0A0E27);
        }

        // Draw input box
        fb.fill_rect(x, y + h - 20, w, 20, 0x2C3E50);
        let disp = if self.input_buffer.len() > 40 {
            &self.input_buffer[self.input_buffer.len()-40..]
        } else { &self.input_buffer };
        fb.draw_str(x + 8, y + h - 14, disp, 0xA0D0FF, 0x2C3E50);

        self.dirty = false;
    }

    fn on_key(&mut self, key: u8) {
        use crate::ansi::Key as AK;
        match self.ansi.feed(key) {
            AK::Char(b'\n') | AK::Char(b'\r') => {
                self.scroll_offset = 0;  // jump to latest on submit
                self.execute_command();
            }
            AK::Char(0x08) | AK::Char(0x7F) => {
                self.input_buffer.pop();
                self.dirty = true;
            }
            AK::Up => {
                self.scroll_offset = self.scroll_offset.saturating_add(1);
                self.dirty = true;
            }
            AK::Down => {
                if self.scroll_offset > 0 {
                    self.scroll_offset -= 1;
                    self.dirty = true;
                }
            }
            AK::Char(c) if (b' '..=b'~').contains(&c) => {
                self.input_buffer.push(c as char);
                self.dirty = true;
            }
            _ => {}
        }
    }

    fn title(&self) -> &str {
        "TIS Console"
    }
}

/// Process Monitor application
pub struct ProcessMonitor {
    pub dirty: bool,
    pub wants_close: bool,
}

impl ProcessMonitor {
    pub fn new() -> Self {
        ProcessMonitor {
            dirty: true,
            wants_close: false,
        }
    }
}

impl App for ProcessMonitor {
    fn render(&mut self, fb: &mut Framebuffer, x: u32, y: u32, w: u32, h: u32) {
        // Draw header
        fb.fill_rect(x, y, w, 24, 0x2C2C38);
        fb.draw_str(x + 8, y + 7, "Process Monitor", 0xF5F5F7, 0x2C2C38);
        fb.fill_rect(x, y + 24, w, 1, 0x3C3C48);

        // Column headers
        fb.fill_rect(x, y + 24, w, 18, 0x3C3C48);
        fb.draw_str(x + 8, y + 29, "PID", 0xB8B8B8, 0x3C3C48);
        fb.draw_str(x + 80, y + 29, "NAME", 0xB8B8B8, 0x3C3C48);
        fb.draw_str(x + 250, y + 29, "STATE", 0xB8B8B8, 0x3C3C48);

        // Sample process data
        let processes = [
            ("1", "psh", "Active"),
            ("2", "desktop", "Active"),
            ("3", "init", "Active"),
        ];

        for (i, (pid, name, state)) in processes.iter().enumerate() {
            let y_pos = y + 42 + (i as u32 * 18);
            if y_pos + 18 > y + h { break; }

            let bg_color = if i % 2 == 0 { 0x1A1A24 } else { 0x232333 };
            fb.fill_rect(x, y_pos, w, 18, bg_color);

            fb.draw_str(x + 8, y_pos + 4, pid, 0xB8B8B8, bg_color);
            fb.draw_str(x + 80, y_pos + 4, name, 0xB8B8B8, bg_color);
            fb.draw_str(x + 250, y_pos + 4, state, 0x4AFF4A, bg_color);
        }

        self.dirty = false;
    }

    fn on_key(&mut self, _key: u8) {
        // Process monitor - read-only for now
    }

    fn title(&self) -> &str {
        "Process Monitor"
    }
}

/// System Information display
pub struct SystemInfo {
    pub dirty: bool,
    pub wants_close: bool,
}

impl SystemInfo {
    pub fn new() -> Self {
        SystemInfo {
            dirty: true,
            wants_close: false,
        }
    }
}

impl App for SystemInfo {
    fn render(&mut self, fb: &mut Framebuffer, x: u32, y: u32, w: u32, h: u32) {
        // Draw header
        fb.fill_rect(x, y, w, 24, 0x2C2C38);
        fb.draw_str(x + 8, y + 7, "System Information", 0xF5F5F7, 0x2C2C38);
        fb.fill_rect(x, y + 24, w, 1, 0x3C3C48);

        // System info items
        let items = [
            "OS: Rusty Penguin v1.0.0",
            "Kernel: Bare-Metal (Pure Rust)",
            "Arch: x86_64",
            "Memory: 512 MB available",
            "Boot Time: ~2 seconds",
            "Uptime: Active",
            "Desktop: Modern window manager",
            "Shell: psh (penguin shell)",
        ];

        for (i, item) in items.iter().enumerate() {
            let y_pos = y + 32 + (i as u32 * 16);
            if y_pos + 16 > y + h { break; }

            let bg_color = if i % 2 == 0 { 0x1A1A24 } else { 0x232333 };
            fb.fill_rect(x, y_pos, w, 16, bg_color);
            fb.draw_str(x + 8, y_pos + 3, item, 0xB8B8B8, bg_color);
        }

        self.dirty = false;
    }

    fn on_key(&mut self, _key: u8) {
        // Info display - read-only
    }

    fn title(&self) -> &str {
        "System Info"
    }
}

/// Simple Calculator application
pub struct Calculator {
    display: String,
    accumulator: i64,
    current_op: Option<char>,
    new_number: bool,
    pub dirty: bool,
    pub wants_close: bool,
}

impl Calculator {
    pub fn new() -> Self {
        Calculator {
            display: String::from("0"),
            accumulator: 0,
            current_op: None,
            new_number: true,
            dirty: true,
            wants_close: false,
        }
    }

    fn handle_digit(&mut self, digit: char) {
        if self.new_number {
            self.display.clear();
            self.new_number = false;
        }
        if self.display.len() < 16 {
            self.display.push(digit);
        }
        self.dirty = true;
    }

    fn execute_operation(&mut self) {
        if let Ok(num) = self.display.parse::<i64>() {
            if let Some(op) = self.current_op {
                let result = match op {
                    '+' => self.accumulator + num,
                    '-' => self.accumulator - num,
                    '*' => self.accumulator * num,
                    '/' => if num != 0 { self.accumulator / num } else { 0 },
                    _ => num,
                };
                self.display = alloc::format!("{}", result);
                self.accumulator = result;
            } else {
                self.accumulator = num;
            }
            self.new_number = true;
        }
    }
}

impl App for Calculator {
    fn render(&mut self, fb: &mut Framebuffer, x: u32, y: u32, w: u32, h: u32) {
        // Header
        fb.fill_rect(x, y, w, 24, 0x2C2C38);
        fb.draw_str(x + 8, y + 7, "Calculator", 0xF5F5F7, 0x2C2C38);
        fb.fill_rect(x, y + 24, w, 1, 0x3C3C48);

        // Display
        fb.fill_rect(x + 8, y + 32, w - 16, 32, 0x1A1A24);
        fb.draw_str(x + 12, y + 42, &self.display, 0x4ADE80, 0x1A1A24);

        // Buttons grid (4x4)
        let buttons = [
            ["7", "8", "9", "/"],
            ["4", "5", "6", "*"],
            ["1", "2", "3", "-"],
            ["0", ".", "=", "+"],
        ];

        let btn_w = (w - 32) / 4;
        let btn_h = 24u32;
        let start_y = y + 72;

        for (row, buttons_row) in buttons.iter().enumerate() {
            for (col, label) in buttons_row.iter().enumerate() {
                let bx = x + 8 + (col as u32 * (btn_w + 4));
                let by = start_y + (row as u32 * (btn_h + 4));

                let bg = if *label == "=" { 0x4ADE80 } else { 0x3C3C48 };
                fb.fill_rect(bx, by, btn_w, btn_h, bg);

                let text_color = if *label == "=" { 0x0F172A } else { 0xF5F5F7 };
                fb.draw_str(bx + (btn_w / 2) - 3, by + 8, label, text_color, bg);
            }
        }

        self.dirty = false;
    }

    fn on_key(&mut self, key: u8) {
        match key {
            b'0'..=b'9' => self.handle_digit(key as char),
            b'+' | b'-' | b'*' | b'/' => {
                self.execute_operation();
                if let Ok(num) = self.display.parse::<i64>() {
                    self.accumulator = num;
                }
                self.current_op = Some(key as char);
                self.new_number = true;
                self.dirty = true;
            }
            b'=' | b'\r' => {
                self.execute_operation();
                self.current_op = None;
                self.dirty = true;
            }
            8 => { // Backspace
                if !self.display.is_empty() && !self.new_number {
                    self.display.pop();
                    if self.display.is_empty() {
                        self.display.push('0');
                    }
                    self.dirty = true;
                }
            }
            b'c' | b'C' => { // Clear
                self.display = String::from("0");
                self.accumulator = 0;
                self.current_op = None;
                self.new_number = true;
                self.dirty = true;
            }
            _ => {}
        }
    }

    fn on_mouse(&mut self, x: i32, y: i32, w: u32, _h: u32, buttons: u8) {
        // Left button only, on the press edge (caller sends raw mask).
        if buttons & 0x01 == 0 { return; }
        // Mirror the render layout exactly: buttons start at y+72, btn_h=24,
        // btn_w = (w-32)/4, 4-px gaps.
        let btn_w = ((w as i32) - 32) / 4;
        let btn_h = 24i32;
        let local_y = y - 72;
        let local_x = x - 8;
        if local_x < 0 || local_y < 0 { return; }
        let row = local_y / (btn_h + 4);
        let col = local_x / (btn_w + 4);
        if row >= 4 || col >= 4 { return; }
        // Reject clicks in the inter-button gap.
        if local_y % (btn_h + 4) >= btn_h { return; }
        if local_x % (btn_w + 4) >= btn_w { return; }
        let grid: [[u8; 4]; 4] = [
            [b'7', b'8', b'9', b'/'],
            [b'4', b'5', b'6', b'*'],
            [b'1', b'2', b'3', b'-'],
            [b'0', b'.', b'=', b'+'],
        ];
        self.on_key(grid[row as usize][col as usize]);
    }

    fn title(&self) -> &str {
        "Calculator"
    }
}

/// System Clock and Status Display
pub struct SystemClock {
    pub dirty: bool,
    pub wants_close: bool,
}

impl SystemClock {
    pub fn new() -> Self {
        SystemClock {
            dirty: true,
            wants_close: false,
        }
    }
}

impl App for SystemClock {
    fn render(&mut self, fb: &mut Framebuffer, x: u32, y: u32, w: u32, h: u32) {
        // Header
        fb.fill_rect(x, y, w, 24, 0x2C2C38);
        fb.draw_str(x + 8, y + 7, "System Clock", 0xF5F5F7, 0x2C2C38);
        fb.fill_rect(x, y + 24, w, 1, 0x3C3C48);

        // Large time display
        fb.fill_rect(x + 8, y + 32, w - 16, 48, 0x1A1A24);
        fb.draw_str(x + 12, y + 45, "12:34:56", 0x4ADE80, 0x1A1A24);

        // Date and info
        fb.fill_rect(x, y + 90, w, 1, 0x3C3C48);

        let items = [
            "Date: 2026-05-28",
            "Uptime: Running",
            "Load: Minimal",
            "",
            "System Status:",
            "CPU: Idle",
            "Memory: 85% Free",
            "Disk: 512MB Available",
        ];

        for (i, item) in items.iter().enumerate() {
            let y_pos = y + 98 + (i as u32 * 14);
            if y_pos + 14 > y + h { break; }

            let color = if item.is_empty() { 0x1A1A24 } else { 0xB8B8B8 };
            fb.draw_str(x + 12, y_pos, item, color, 0x0F172A);
        }

        self.dirty = false;
    }

    fn on_key(&mut self, _key: u8) {
        // Clock display - read-only
    }

    fn title(&self) -> &str {
        "System Clock"
    }
}

/// Help and Reference Browser
pub struct HelpBrowser {
    scroll_offset: usize,
    pub dirty: bool,
    pub wants_close: bool,
    ansi: crate::ansi::AnsiParser,
}

impl HelpBrowser {
    pub fn new() -> Self {
        HelpBrowser {
            scroll_offset: 0,
            dirty: true,
            wants_close: false,
            ansi: crate::ansi::AnsiParser::new(),
        }
    }
}

impl App for HelpBrowser {
    fn render(&mut self, fb: &mut Framebuffer, x: u32, y: u32, w: u32, h: u32) {
        // Header
        fb.fill_rect(x, y, w, 24, 0x2C2C38);
        fb.draw_str(x + 8, y + 7, "Help & Reference", 0xF5F5F7, 0x2C2C38);
        fb.fill_rect(x, y + 24, w, 1, 0x3C3C48);

        let help_text = [
            "KEYBOARD SHORTCUTS",
            "===================",
            "",
            "File Manager:",
            "  Up/Down  - Navigate files",
            "  Enter    - Open directory",
            "  Backspace- Go up directory",
            "  C        - Copy path",
            "  D        - Delete file",
            "",
            "Text Editor:",
            "  Ctrl+S   - Save file",
            "  Ctrl+Q   - Quit editor",
            "  Arrows   - Navigate",
            "",
            "Terminal:",
            "  Ctrl+C   - Interrupt",
            "  Ctrl+T   - New window",
            "  Ctrl+W   - Close window",
            "",
            "Desktop:",
            "  Right-click - Context menu",
            "  Click icons - Launch apps",
            "",
            "AVAILABLE COMMANDS",
            "===================",
            "",
            "File: ls, cat, touch, rm, mkdir, cp, mv",
            "Text: nano, vi, edit, grep, head, tail",
            "Util: echo, date, pwd, cd, whoami",
            "Sys: ps, mem, free, df, uptime, cal",
            "Math: calc, bc, trit, mul, div",
            "Pkg: rpm install <package.rpkg>",
            "",
            "Use 'help' in Terminal for full list",
        ];

        let mut y_pos = y + 32;
        for (i, line) in help_text.iter().enumerate() {
            if i < self.scroll_offset { continue; }
            if y_pos + 14 > y + h { break; }

            let color = if line.is_empty() {
                0x0F172A
            } else if line.contains("=") {
                0x4A9EFF
            } else if line.starts_with("  ") {
                0x9CA3AF
            } else {
                0xB8B8B8
            };

            fb.draw_str(x + 8, y_pos, line, color, 0x0F172A);
            y_pos += 14;
        }

        self.dirty = false;
    }

    fn on_key(&mut self, key: u8) {
        use crate::ansi::Key as AK;
        match self.ansi.feed(key) {
            AK::Up => {
                if self.scroll_offset > 0 {
                    self.scroll_offset -= 1;
                    self.dirty = true;
                }
            }
            AK::Down => {
                self.scroll_offset = self.scroll_offset.saturating_add(1);
                self.dirty = true;
            }
            _ => {}
        }
    }

    fn title(&self) -> &str {
        "Help Browser"
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Games — preinstalled on the Rusty Penguin desktop. Pure-Rust, no_std, no heap
// churn in the render path (numbers via u64_into, no per-frame format!()).
// ─────────────────────────────────────────────────────────────────────────────

const SNAKE_COLS: i16 = 24;
const SNAKE_ROWS: i16 = 18;

/// Classic Snake. Tick-driven movement; arrow keys steer; SPACE restarts.
pub struct Snake {
    body: Vec<(i16, i16)>, // head at index 0
    dir: (i16, i16),
    pending: (i16, i16),
    food: (i16, i16),
    score: u32,
    dead: bool,
    started: bool,
    last_move: u64,
    interval: u64,
    rng: Rng,
    pub dirty: bool,
    pub wants_close: bool,
    ansi: crate::ansi::AnsiParser,
}

impl Snake {
    pub fn new(seed: u64) -> Self {
        let mut s = Snake {
            body: Vec::with_capacity((SNAKE_COLS * SNAKE_ROWS) as usize),
            dir: (1, 0),
            pending: (1, 0),
            food: (0, 0),
            score: 0,
            dead: false,
            started: false,
            last_move: 0,
            interval: 9, // ~11 moves/sec at 100 Hz
            rng: Rng::new(seed),
            dirty: true,
            wants_close: false,
            ansi: crate::ansi::AnsiParser::new(),
        };
        s.reset();
        s
    }

    fn reset(&mut self) {
        self.body.clear();
        let cx = SNAKE_COLS / 2;
        let cy = SNAKE_ROWS / 2;
        self.body.push((cx, cy));
        self.body.push((cx - 1, cy));
        self.body.push((cx - 2, cy));
        self.dir = (1, 0);
        self.pending = (1, 0);
        self.score = 0;
        self.dead = false;
        self.started = false;
        self.place_food();
        self.dirty = true;
    }

    fn place_food(&mut self) {
        // Try random cells until one is free (grid is large relative to snake).
        for _ in 0..256 {
            let fx = self.rng.range(SNAKE_COLS as u32) as i16;
            let fy = self.rng.range(SNAKE_ROWS as u32) as i16;
            if !self.body.iter().any(|&p| p == (fx, fy)) {
                self.food = (fx, fy);
                return;
            }
        }
    }
}

impl App for Snake {
    fn tick(&mut self, ticks: u64) -> bool {
        if self.dead || !self.started { return false; }
        if ticks.wrapping_sub(self.last_move) < self.interval { return false; }
        self.last_move = ticks;

        // Apply the queued direction unless it reverses straight back.
        if (self.pending.0 + self.dir.0, self.pending.1 + self.dir.1) != (0, 0) {
            self.dir = self.pending;
        }
        let (hx, hy) = self.body[0];
        let nx = hx + self.dir.0;
        let ny = hy + self.dir.1;

        // Wall or self collision → dead.
        if nx < 0 || ny < 0 || nx >= SNAKE_COLS || ny >= SNAKE_ROWS
            || self.body.iter().any(|&p| p == (nx, ny))
        {
            self.dead = true;
            return true;
        }

        self.body.insert(0, (nx, ny));
        if (nx, ny) == self.food {
            self.score += 1;
            self.place_food();
        } else {
            self.body.pop();
        }
        true
    }

    fn render(&mut self, fb: &mut Framebuffer, x: u32, y: u32, w: u32, h: u32) {
        // Header with score.
        fb.fill_rect(x, y, w, 22, 0x14281E);
        fb.draw_str(x + 8, y + 6, "SNAKE", 0x4ADE80, 0x14281E);
        let mut sbuf = [0u8; 24];
        fb.draw_str(x + 70, y + 6, "score:", 0x9CA3AF, 0x14281E);
        fb.draw_str(x + 126, y + 6, u64_into(&mut sbuf, self.score as u64), 0xF5F5F7, 0x14281E);

        // Board area below header.
        let board_y = y + 24;
        let board_h = h.saturating_sub(24);
        let cell = core::cmp::min(w / SNAKE_COLS as u32, board_h / SNAKE_ROWS as u32).max(1);
        let gw = cell * SNAKE_COLS as u32;
        let gh = cell * SNAKE_ROWS as u32;
        let ox = x + (w.saturating_sub(gw)) / 2;
        let oy = board_y + (board_h.saturating_sub(gh)) / 2;

        // Playfield background.
        fb.fill_rect(x, board_y, w, board_h, 0x0A0E14);
        fb.fill_rect(ox, oy, gw, gh, 0x0E1A12);

        // Food.
        fb.fill_rect(ox + self.food.0 as u32 * cell + 1, oy + self.food.1 as u32 * cell + 1,
                     cell - 2, cell - 2, 0xEF4444);

        // Snake — head brighter than body.
        for (i, &(sx, sy)) in self.body.iter().enumerate() {
            let col = if i == 0 { 0x86EFAC } else { 0x22C55E };
            fb.fill_rect(ox + sx as u32 * cell, oy + sy as u32 * cell, cell - 1, cell - 1, col);
        }

        if self.dead {
            let by = oy + gh / 2 - 16;
            fb.fill_rect(ox, by, gw, 34, 0x000000);
            fb.draw_str(ox + 8, by + 4, "GAME OVER", 0xEF4444, 0x000000);
            fb.draw_str(ox + 8, by + 20, "press SPACE to restart", 0xF5F5F7, 0x000000);
        } else if !self.started {
            let by = oy + gh / 2 - 8;
            fb.fill_rect(ox, by, gw, 18, 0x000000);
            fb.draw_str(ox + 8, by + 4, "arrow keys / WASD to start", 0x86EFAC, 0x000000);
        }
        self.dirty = false;
    }

    fn on_key(&mut self, key: u8) {
        use crate::ansi::Key as AK;
        match self.ansi.feed(key) {
            AK::Up    => { self.pending = (0, -1); self.started = true; self.dirty = true; }
            AK::Down  => { self.pending = (0, 1);  self.started = true; self.dirty = true; }
            AK::Left  => { self.pending = (-1, 0); self.started = true; self.dirty = true; }
            AK::Right => { self.pending = (1, 0);  self.started = true; self.dirty = true; }
            AK::Char(b' ') | AK::Char(b'\n') | AK::Char(b'\r') => {
                if self.dead { self.reset(); }
            }
            // WASD as a fallback for steering.
            AK::Char(b'w') => { self.pending = (0, -1); self.started = true; self.dirty = true; }
            AK::Char(b's') => { self.pending = (0, 1);  self.started = true; self.dirty = true; }
            AK::Char(b'a') => { self.pending = (-1, 0); self.started = true; self.dirty = true; }
            AK::Char(b'd') => { self.pending = (1, 0);  self.started = true; self.dirty = true; }
            _ => {}
        }
    }

    fn title(&self) -> &str { "Snake" }
}

const MS_COLS: usize = 12;
const MS_ROWS: usize = 10;
const MS_MINES: usize = 18;

/// Minesweeper. First reveal is always safe (mines placed after it). Arrow
/// keys + SPACE/ENTER to reveal, F to flag; mouse also works (L reveal,
/// R flag). SPACE restarts after win/loss.
pub struct Minesweeper {
    mine: Vec<bool>,
    count: Vec<u8>,
    state: Vec<u8>, // 0 hidden, 1 revealed, 2 flagged
    cursor: usize,
    placed: bool,
    dead: bool,
    won: bool,
    revealed: usize,
    flags: usize,
    seed: u64,
    pub dirty: bool,
    pub wants_close: bool,
    ansi: crate::ansi::AnsiParser,
}

impl Minesweeper {
    pub fn new(seed: u64) -> Self {
        let n = MS_COLS * MS_ROWS;
        let mut m = Minesweeper {
            mine: Vec::with_capacity(n),
            count: Vec::with_capacity(n),
            state: Vec::with_capacity(n),
            cursor: 0,
            placed: false,
            dead: false,
            won: false,
            revealed: 0,
            flags: 0,
            seed,
            dirty: true,
            wants_close: false,
            ansi: crate::ansi::AnsiParser::new(),
        };
        m.reset();
        m
    }

    fn reset(&mut self) {
        let n = MS_COLS * MS_ROWS;
        self.mine.clear(); self.count.clear(); self.state.clear();
        for _ in 0..n { self.mine.push(false); self.count.push(0); self.state.push(0); }
        self.cursor = 0;
        self.placed = false;
        self.dead = false;
        self.won = false;
        self.revealed = 0;
        self.flags = 0;
        self.dirty = true;
    }

    fn idx(c: usize, r: usize) -> usize { r * MS_COLS + c }

    fn neighbors(i: usize) -> ([usize; 8], usize) {
        let c = (i % MS_COLS) as i32;
        let r = (i / MS_COLS) as i32;
        let mut out = [0usize; 8];
        let mut k = 0;
        let mut dr = -1;
        while dr <= 1 {
            let mut dc = -1;
            while dc <= 1 {
                if !(dr == 0 && dc == 0) {
                    let nc = c + dc; let nr = r + dr;
                    if nc >= 0 && nc < MS_COLS as i32 && nr >= 0 && nr < MS_ROWS as i32 {
                        out[k] = Self::idx(nc as usize, nr as usize);
                        k += 1;
                    }
                }
                dc += 1;
            }
            dr += 1;
        }
        (out, k)
    }

    fn place_mines(&mut self, safe: usize) {
        let mut rng = Rng::new(self.seed ^ (safe as u64).wrapping_mul(0x100000001B3));
        let (safe_n, safe_k) = Self::neighbors(safe);
        let mut placed = 0;
        while placed < MS_MINES {
            let i = rng.range((MS_COLS * MS_ROWS) as u32) as usize;
            if i == safe || self.mine[i] { continue; }
            // Keep the first-click neighborhood clear for a friendlier open.
            if safe_n[..safe_k].contains(&i) { continue; }
            self.mine[i] = true;
            placed += 1;
        }
        // Precompute adjacency counts.
        for i in 0..self.count.len() {
            if self.mine[i] { continue; }
            let (nb, k) = Self::neighbors(i);
            let mut c = 0u8;
            for &j in &nb[..k] { if self.mine[j] { c += 1; } }
            self.count[i] = c;
        }
        self.placed = true;
    }

    fn reveal(&mut self, start: usize) {
        if self.dead || self.won { return; }
        if !self.placed { self.place_mines(start); }
        if self.state[start] != 0 { return; } // already revealed or flagged

        if self.mine[start] {
            self.state[start] = 1;
            self.dead = true;
            self.dirty = true;
            return;
        }

        // Iterative flood fill for zero-count regions.
        let mut stack = Vec::with_capacity(32);
        stack.push(start);
        while let Some(i) = stack.pop() {
            if self.state[i] != 0 { continue; }
            self.state[i] = 1;
            self.revealed += 1;
            if self.count[i] == 0 {
                let (nb, k) = Self::neighbors(i);
                for &j in &nb[..k] {
                    if self.state[j] == 0 && !self.mine[j] { stack.push(j); }
                }
            }
        }
        if self.revealed == MS_COLS * MS_ROWS - MS_MINES { self.won = true; }
        self.dirty = true;
    }

    fn toggle_flag(&mut self, i: usize) {
        if self.dead || self.won { return; }
        match self.state[i] {
            0 => { self.state[i] = 2; self.flags += 1; }
            2 => { self.state[i] = 0; self.flags -= 1; }
            _ => {}
        }
        self.dirty = true;
    }
}

impl App for Minesweeper {
    fn render(&mut self, fb: &mut Framebuffer, x: u32, y: u32, w: u32, h: u32) {
        // Header: mines remaining + status.
        fb.fill_rect(x, y, w, 22, 0x1A1A24);
        fb.draw_str(x + 8, y + 6, "MINES", 0xFCD34D, 0x1A1A24);
        let mut sbuf = [0u8; 24];
        let remaining = MS_MINES.saturating_sub(self.flags) as u64;
        fb.draw_str(x + 64, y + 6, u64_into(&mut sbuf, remaining), 0xF5F5F7, 0x1A1A24);
        if self.dead { fb.draw_str(x + 110, y + 6, "BOOM!", 0xEF4444, 0x1A1A24); }
        else if self.won { fb.draw_str(x + 110, y + 6, "YOU WIN!", 0x4ADE80, 0x1A1A24); }

        let board_y = y + 24;
        let board_h = h.saturating_sub(24);
        let cell = core::cmp::min(w / MS_COLS as u32, board_h / MS_ROWS as u32).max(1);
        let gw = cell * MS_COLS as u32;
        let gh = cell * MS_ROWS as u32;
        let ox = x + (w.saturating_sub(gw)) / 2;
        let oy = board_y + (board_h.saturating_sub(gh)) / 2;

        fb.fill_rect(x, board_y, w, board_h, 0x0A0E14);

        let num_colors = [0x9CA3AF, 0x60A5FA, 0x4ADE80, 0xF87171, 0xC084FC,
                          0xFB923C, 0x22D3EE, 0xF5F5F7, 0xF5F5F7];
        for i in 0..(MS_COLS * MS_ROWS) {
            let c = (i % MS_COLS) as u32;
            let r = (i / MS_COLS) as u32;
            let px = ox + c * cell;
            let py = oy + r * cell;
            let is_cursor = i == self.cursor;
            match self.state[i] {
                1 => {
                    // Revealed.
                    if self.mine[i] {
                        fb.fill_rect(px, py, cell - 1, cell - 1, 0xEF4444);
                        fb.draw_char(px + cell / 2 - 4, py + cell / 2 - 6, '*', 0x000000, 0xEF4444);
                    } else {
                        fb.fill_rect(px, py, cell - 1, cell - 1, 0x1F2937);
                        let cnt = self.count[i];
                        if cnt > 0 {
                            let ch = (b'0' + cnt) as char;
                            fb.draw_char(px + cell / 2 - 4, py + cell / 2 - 6, ch,
                                         num_colors[cnt as usize], 0x1F2937);
                        }
                    }
                }
                2 => {
                    fb.fill_rect(px, py, cell - 1, cell - 1, 0x374151);
                    fb.draw_char(px + cell / 2 - 4, py + cell / 2 - 6, 'F', 0xFCD34D, 0x374151);
                }
                _ => {
                    // Hidden — also reveal mines on death.
                    if self.dead && self.mine[i] {
                        fb.fill_rect(px, py, cell - 1, cell - 1, 0x7F1D1D);
                        fb.draw_char(px + cell / 2 - 4, py + cell / 2 - 6, '*', 0x000000, 0x7F1D1D);
                    } else {
                        fb.fill_rect(px, py, cell - 1, cell - 1, 0x4B5563);
                    }
                }
            }
            if is_cursor {
                // Cursor outline.
                fb.fill_rect(px, py, cell - 1, 2, 0xFCD34D);
                fb.fill_rect(px, py, 2, cell - 1, 0xFCD34D);
                fb.fill_rect(px, py + cell - 3, cell - 1, 2, 0xFCD34D);
                fb.fill_rect(px + cell - 3, py, 2, cell - 1, 0xFCD34D);
            }
        }
        self.dirty = false;
    }

    fn on_key(&mut self, key: u8) {
        use crate::ansi::Key as AK;
        let c = self.cursor % MS_COLS;
        let r = self.cursor / MS_COLS;
        match self.ansi.feed(key) {
            AK::Up    => { if r > 0 { self.cursor -= MS_COLS; self.dirty = true; } }
            AK::Down  => { if r + 1 < MS_ROWS { self.cursor += MS_COLS; self.dirty = true; } }
            AK::Left  => { if c > 0 { self.cursor -= 1; self.dirty = true; } }
            AK::Right => { if c + 1 < MS_COLS { self.cursor += 1; self.dirty = true; } }
            AK::Char(b'f') | AK::Char(b'F') => { let i = self.cursor; self.toggle_flag(i); }
            AK::Char(b'\n') | AK::Char(b'\r') => {
                if self.dead || self.won { self.reset(); } else { let i = self.cursor; self.reveal(i); }
            }
            AK::Char(b' ') => {
                if self.dead || self.won { self.reset(); } else { let i = self.cursor; self.reveal(i); }
            }
            _ => {}
        }
    }

    fn on_mouse(&mut self, mx: i32, my: i32, w: u32, h: u32, buttons: u8) {
        if buttons & 0x03 == 0 { return; }
        let board_h = h.saturating_sub(24);
        let cell = core::cmp::min(w / MS_COLS as u32, board_h / MS_ROWS as u32).max(1) as i32;
        let gw = cell * MS_COLS as i32;
        let gh = cell * MS_ROWS as i32;
        let ox = (w as i32 - gw) / 2;
        let oy = 24 + (board_h as i32 - gh) / 2;
        let lx = mx - ox;
        let ly = my - oy;
        if lx < 0 || ly < 0 || lx >= gw || ly >= gh { return; }
        let c = (lx / cell) as usize;
        let r = (ly / cell) as usize;
        let i = Self::idx(c, r);
        self.cursor = i;
        if buttons & 0x02 != 0 {
            self.toggle_flag(i);
        } else if buttons & 0x01 != 0 {
            if self.dead || self.won { self.reset(); } else { self.reveal(i); }
        }
    }

    fn title(&self) -> &str { "Minesweeper" }
}
