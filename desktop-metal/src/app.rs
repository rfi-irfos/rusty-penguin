// Application framework for Rusty Penguin
// Apps implement this trait to be launchable desktop applications.

use crate::fb::Framebuffer;
use crate::vfs;
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
    clipboard: Option<String>,
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
        };
        fm.refresh();
        fm
    }

    fn copy_selected(&mut self) {
        if self.selected < self.entries.len() {
            let entry = &self.entries[self.selected];
            let sep = if self.cwd.ends_with('/') { "" } else { "/" };
            self.clipboard = Some(alloc::format!("{}{}{}", self.cwd, sep, entry.name));
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
            0x1C => {                   // Enter - open directory
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
            0x2E => self.copy_selected(),    // C - copy
            0x20 => self.delete_selected(),  // D - delete
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
    theme: bool,           // true = dark, false = light
    window_snap: bool,
    taskbar_bottom: bool,
    auto_save_enabled: bool,
    auto_save_interval: u32, // seconds
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

        // Settings options with dynamic values
        let theme_str = if self.theme { "Dark" } else { "Light" };
        let snap_str = if self.window_snap { "On" } else { "Off" };
        let taskbar_str = if self.taskbar_bottom { "Bottom" } else { "Top" };
        let autosave_str = if self.auto_save_enabled {
            alloc::format!("{}s", self.auto_save_interval)
        } else {
            String::from("Off")
        };

        let settings = [
            alloc::format!("Theme: {}", theme_str),
            alloc::format!("Window Snap: {}", snap_str),
            alloc::format!("Taskbar: {}", taskbar_str),
            alloc::format!("Auto-Save: {}", &autosave_str),
        ];

        for (i, setting) in settings.iter().enumerate() {
            let y_pos = y + 32 + (i as u32 * 20);
            if y_pos + 20 > y + h { break; }

            let bg_color = if i == self.selected { 0x4A5568 } else if i % 2 == 0 { 0x1A1A24 } else { 0x232333 };
            fb.fill_rect(x, y_pos, w, 20, bg_color);

            let text_color = if i == self.selected { 0xF5F5F7 } else { 0xB8B8B8 };
            fb.draw_str(x + 12, y_pos + 5, setting.as_str(), text_color, bg_color);
        }

        // Draw hint at bottom
        let hint = "(UP/DOWN to select, ENTER to toggle)";
        if y + h > 100 {
            fb.draw_str(x + 12, y + h - 24, hint, 0x808080, 0x0A0E27);
        }

        self.dirty = false;
    }

    fn on_key(&mut self, key: u8) {
        match key {
            0x48 => { if self.selected > 0 { self.selected -= 1; self.dirty = true; } }
            0x50 => { if self.selected < 3 { self.selected += 1; self.dirty = true; } }
            0x1C => { self.toggle_selected(); }
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
    pub dirty: bool,
    pub wants_close: bool,
}

impl TisConsole {
    pub fn new() -> Self {
        let mut console = TisConsole {
            input_buffer: String::new(),
            output_lines: Vec::new(),
            dirty: true,
            wants_close: false,
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

        // Draw output (last few lines only)
        let line_h = 13u32;
        let max_lines = ((h - 44) / line_h) as usize;
        let start = if self.output_lines.len() > max_lines {
            self.output_lines.len() - max_lines
        } else { 0 };

        for (i, line) in self.output_lines[start..].iter().enumerate() {
            let y_pos = y + 28 + (i as u32 * line_h);
            if y_pos + line_h > y + h - 20 { break; }
            fb.draw_str(x + 8, y_pos, line, 0x00FF00, 0x0A0E27);
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
        match key {
            0x1C => { self.execute_command(); }
            0x0E => { self.input_buffer.pop(); self.dirty = true; }
            b' '..=b'~' => {
                self.input_buffer.push(key as char);
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
}

impl HelpBrowser {
    pub fn new() -> Self {
        HelpBrowser {
            scroll_offset: 0,
            dirty: true,
            wants_close: false,
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
        match key {
            0x48 => { // Up arrow
                if self.scroll_offset > 0 {
                    self.scroll_offset -= 1;
                    self.dirty = true;
                }
            }
            0x50 => { // Down arrow
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
