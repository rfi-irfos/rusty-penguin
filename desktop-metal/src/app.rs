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

/// Kernel Manager — "bring your own kernel" installer.
///
/// The whole point (Linus's old challenge: make swapping the kernel stupidly
/// easy): scan the filesystem for kernel ELF candidates, let you pick one and
/// stage it as the next boot kernel with a single click, and show the running
/// kernel + ABI. Staging writes `boot.manifest` into the VFS, which the ISO
/// build tooling (and the recovery shell's `kinstall`) pick up.
pub struct KernelManager {
    candidates: Vec<String>,    // *.elf files found in /
    selected:   usize,
    staged:     Option<String>, // kernel currently staged for next boot
    status:     String,
    ansi:       crate::ansi::AnsiParser,
    pub dirty:       bool,
    pub wants_close: bool,
}

const KM_LIST_TOP: i32 = 84;   // content-relative y where the kernel list starts
const KM_ROW_H:    i32 = 24;
const KM_BTN_H:    i32 = 32;

impl KernelManager {
    pub fn new() -> Self {
        let mut km = KernelManager {
            candidates:  Vec::new(),
            selected:    0,
            staged:      None,
            status:      String::from("Pick a kernel ELF, then click Install."),
            ansi:        crate::ansi::AnsiParser::new(),
            dirty:       true,
            wants_close: false,
        };
        km.scan();
        // Reflect any kernel staged in a previous session.
        if let Some(data) = vfs::vfs().read("boot.manifest") {
            if let Some(name) = Self::parse_manifest(data) {
                if let Some(i) = km.candidates.iter().position(|c| *c == name) {
                    km.selected = i;
                }
                km.status = String::from("Staged kernel found — reboot to apply.");
                km.staged = Some(name);
            }
        }
        km
    }

    /// Extract the `kernel_elf=<name>` value from a boot.manifest blob.
    fn parse_manifest(data: &[u8]) -> Option<String> {
        let text = core::str::from_utf8(data).ok()?;
        for line in text.lines() {
            if let Some(v) = line.strip_prefix("kernel_elf=") {
                let v = v.trim();
                if !v.is_empty() { return Some(String::from(v)); }
            }
        }
        None
    }

    /// Scan the VFS root for files ending in `.elf`.
    fn scan(&mut self) {
        self.candidates.clear();
        let mut buf = [0u8; 4096];
        let count = unsafe { sys_listdir(b"/", &mut buf) };
        let mut off = 0usize;
        for _ in 0..count {
            if off >= buf.len() { break; }
            let name_len = buf[off] as usize; off += 1;
            if off + name_len > buf.len() { break; }
            let name = String::from_utf8_lossy(&buf[off..off + name_len]).into_owned();
            off += name_len;
            if off + 8 > buf.len() { break; }
            off += 8; // skip the 8-byte size field
            if name.ends_with(".elf") { self.candidates.push(name); }
        }
        if self.selected >= self.candidates.len() { self.selected = 0; }
    }

    fn install_selected(&mut self) {
        if self.selected < self.candidates.len() {
            let name = self.candidates[self.selected].clone();
            let mut m = String::from("kernel_elf=");
            m.push_str(&name);
            m.push('\n');
            m.push_str("staged_by=Kernel Manager\n");
            vfs::vfs().write("boot.manifest", m.as_bytes());
            self.status = alloc::format!("Staged '{}'. Reboot to boot it.", name);
            self.staged = Some(name);
        } else {
            self.status = String::from("No kernel selected — copy a .elf into / first.");
        }
        self.dirty = true;
    }
}

impl App for KernelManager {
    fn render(&mut self, fb: &mut Framebuffer, x: u32, y: u32, w: u32, h: u32) {
        // Header
        fb.fill_rect(x, y, w, 28, 0x2C2C38);
        fb.draw_str(x + 12, y + 9, "Kernel Manager", 0xF5F5F7, 0x2C2C38);
        fb.draw_str(x + 168, y + 9, "bring your own kernel", 0x8CC6E5, 0x2C2C38);

        // Running kernel + ABI
        fb.fill_rect(x, y + 28, w, 26, 0x232333);
        fb.draw_str(x + 12, y + 36, "Running: RustyPenguin 1.0.0 x86_64  (psh syscall ABI v1)",
                    0x6FE18B, 0x232333);

        // Section label
        fb.draw_str(x + 12, y + 62, "Available kernels (.elf in /):", 0xB8B8B8, 0x1A1A24);

        // Candidate list
        if self.candidates.is_empty() {
            fb.draw_str(x + 12, y + KM_LIST_TOP as u32,
                        "None found. Copy a multiboot2 kernel.elf into / (or psh: kinstall).",
                        0xB8B8B8, 0x1A1A24);
        } else {
            for (i, name) in self.candidates.iter().enumerate() {
                let row_y = y + KM_LIST_TOP as u32 + (i as u32 * KM_ROW_H as u32);
                if row_y + KM_ROW_H as u32 > y + h - (KM_BTN_H as u32 + 24) { break; }
                let is_sel = i == self.selected;
                let is_staged = self.staged.as_deref() == Some(name.as_str());
                let bg = if is_sel { 0x4A5568 } else if i % 2 == 0 { 0x1A1A24 } else { 0x232333 };
                fb.fill_rect(x, row_y, w, KM_ROW_H as u32, bg);
                if is_staged { fb.fill_rect(x, row_y, 4, KM_ROW_H as u32, 0x6FE18B); }
                let fg = if is_sel { 0xF5F5F7 } else { 0xCFCFD6 };
                fb.draw_str(x + 14, row_y + 6, name, fg, bg);
                if is_staged {
                    fb.draw_str(x + w - 80, row_y + 6, "staged", 0x6FE18B, bg);
                }
            }
        }

        // Status line (above the buttons)
        let status_y = y + h - (KM_BTN_H as u32 + 22);
        fb.fill_rect(x, status_y, w, 18, 0x1A1A24);
        fb.draw_str(x + 12, status_y + 2, &self.status, 0xF5C451, 0x1A1A24);

        // Buttons: Install (green) + Rescan (blue)
        let btn_y = y + h - KM_BTN_H as u32 - 2;
        fb.fill_rect(x + 12, btn_y, 220, KM_BTN_H as u32, 0x2E7D4F);
        fb.draw_str(x + 40, btn_y + 9, "Install Selected Kernel", 0xF5F5F7, 0x2E7D4F);
        fb.fill_rect(x + 244, btn_y, 110, KM_BTN_H as u32, 0x355B8C);
        fb.draw_str(x + 280, btn_y + 9, "Rescan", 0xF5F5F7, 0x355B8C);

        self.dirty = false;
    }

    fn on_key(&mut self, key: u8) {
        use crate::ansi::Key as AK;
        match self.ansi.feed(key) {
            AK::Up   => { if self.selected > 0 { self.selected -= 1; self.dirty = true; } }
            AK::Down => { if self.selected + 1 < self.candidates.len() { self.selected += 1; self.dirty = true; } }
            AK::Char(b'\n') | AK::Char(b'\r') => self.install_selected(),
            AK::Char(b'r') | AK::Char(b'R')   => { self.scan(); self.status = String::from("Rescanned /."); self.dirty = true; }
            _ => {}
        }
    }

    fn on_mouse(&mut self, x: i32, y: i32, _w: u32, h: u32, buttons: u8) {
        if buttons & 0x01 == 0 { return; }
        let btn_y = h as i32 - KM_BTN_H - 2;
        // Install button
        if x >= 12 && x < 232 && y >= btn_y && y < btn_y + KM_BTN_H {
            self.install_selected();
            return;
        }
        // Rescan button
        if x >= 244 && x < 354 && y >= btn_y && y < btn_y + KM_BTN_H {
            self.scan();
            self.status = String::from("Rescanned /.");
            self.dirty = true;
            return;
        }
        // List row selection
        if y >= KM_LIST_TOP {
            let row = ((y - KM_LIST_TOP) / KM_ROW_H) as usize;
            if row < self.candidates.len() && row != self.selected {
                self.selected = row;
                self.dirty = true;
            }
        }
    }

    fn title(&self) -> &str { "Kernel Manager" }
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
// Browser — a NATIVE web browser shell rendered by our own engine (no Chromium,
// no WebKit). Real chrome: back/forward/reload + an address bar, and pages laid
// out by the desktop's text/CSS primitives. v1 renders built-in local pages
// offline — live networking is the next kernel brick (NIC + TCP/IP), and a real
// remote-web engine rides the Linux ABI layer. Honest about that on the page.
// ─────────────────────────────────────────────────────────────────────────────

const BR_TOOLBAR_H: u32 = 34;

// Line kinds for the tiny built-in page layout engine.
const K_SPACE: u8 = 0;
const K_H1:    u8 = 1;
const K_H2:    u8 = 2;
const K_P:     u8 = 3;
const K_LINK:  u8 = 4;
const K_NOTE:  u8 = 5;

struct BrLine { kind: u8, text: &'static str, link: i32 }
struct BrPage { url: &'static str, title: &'static str, lines: &'static [BrLine] }

const PAGE_HOME: &[BrLine] = &[
    BrLine { kind: K_H1,   text: "Rusty Penguin Web",                                  link: -1 },
    BrLine { kind: K_NOTE, text: "Native browser . rendered by the ternary engine",   link: -1 },
    BrLine { kind: K_SPACE,text: "",                                                   link: -1 },
    BrLine { kind: K_P,    text: "No Chromium, no WebKit underneath. This window, its",link: -1 },
    BrLine { kind: K_P,    text: "chrome and every page are drawn by Rusty Penguin's", link: -1 },
    BrLine { kind: K_P,    text: "own framebuffer + CSS engine.",                      link: -1 },
    BrLine { kind: K_SPACE,text: "",                                                   link: -1 },
    BrLine { kind: K_H2,   text: "Bookmarks",                                          link: -1 },
    BrLine { kind: K_LINK, text: "About this OS",                                      link: 1 },
    BrLine { kind: K_LINK, text: "The ternary case",                                   link: 2 },
    BrLine { kind: K_LINK, text: "When does the live web work?",                       link: 3 },
];

const PAGE_ABOUT: &[BrLine] = &[
    BrLine { kind: K_H1,   text: "About Rusty Penguin",                                link: -1 },
    BrLine { kind: K_NOTE, text: "rustypenguin://about",                               link: -1 },
    BrLine { kind: K_SPACE,text: "",                                                   link: -1 },
    BrLine { kind: K_P,    text: "A complete operating system written from scratch in",link: -1 },
    BrLine { kind: K_P,    text: "pure Rust: its own bootloader, kernel, drivers,",    link: -1 },
    BrLine { kind: K_P,    text: "window manager and apps. No Linux kernel, no libc.", link: -1 },
    BrLine { kind: K_SPACE,text: "",                                                   link: -1 },
    BrLine { kind: K_P,    text: "Ternary logic (-1 / 0 / +1) is a first-class",       link: -1 },
    BrLine { kind: K_P,    text: "primitive at every layer. Built by RFI-IRFOS.",      link: -1 },
    BrLine { kind: K_SPACE,text: "",                                                   link: -1 },
    BrLine { kind: K_LINK, text: "< Back to start",                                    link: 0 },
];

const PAGE_TERNARY: &[BrLine] = &[
    BrLine { kind: K_H1,   text: "The ternary case",                                   link: -1 },
    BrLine { kind: K_NOTE, text: "rustypenguin://ternary",                             link: -1 },
    BrLine { kind: K_SPACE,text: "",                                                   link: -1 },
    BrLine { kind: K_P,    text: "Binary has two states. We treat a third as real:",   link: -1 },
    BrLine { kind: K_P,    text: "dormant. Not running, not stopped - resting.",       link: -1 },
    BrLine { kind: K_SPACE,text: "",                                                   link: -1 },
    BrLine { kind: K_P,    text: "Zero-weight work is skipped, not computed. The",     link: -1 },
    BrLine { kind: K_P,    text: "renderer, scheduler and AI runtime all do this.",    link: -1 },
    BrLine { kind: K_SPACE,text: "",                                                   link: -1 },
    BrLine { kind: K_LINK, text: "< Back to start",                                    link: 0 },
];

const PAGE_WEB: &[BrLine] = &[
    BrLine { kind: K_H1,   text: "When does the live web work?",                       link: -1 },
    BrLine { kind: K_NOTE, text: "rustypenguin://roadmap",                             link: -1 },
    BrLine { kind: K_SPACE,text: "",                                                   link: -1 },
    BrLine { kind: K_P,    text: "Honestly: not yet. Loading real sites needs two",    link: -1 },
    BrLine { kind: K_P,    text: "things this OS is still building from scratch:",     link: -1 },
    BrLine { kind: K_SPACE,text: "",                                                   link: -1 },
    BrLine { kind: K_P,    text: "1. A network stack - a NIC driver + TCP/IP on the",  link: -1 },
    BrLine { kind: K_P,    text: "   bare-metal kernel (planned).",                    link: -1 },
    BrLine { kind: K_P,    text: "2. The Linux ABI layer maturing enough to host a",   link: -1 },
    BrLine { kind: K_P,    text: "   full web engine (bricks 1-5 done).",              link: -1 },
    BrLine { kind: K_SPACE,text: "",                                                   link: -1 },
    BrLine { kind: K_P,    text: "Until then this renders local pages. We don't",      link: -1 },
    BrLine { kind: K_P,    text: "pretend velocity equals completion.",                link: -1 },
    BrLine { kind: K_SPACE,text: "",                                                   link: -1 },
    BrLine { kind: K_LINK, text: "< Back to start",                                    link: 0 },
];

fn br_page(i: usize) -> BrPage {
    match i {
        1 => BrPage { url: "rustypenguin://about",   title: "About Rusty Penguin", lines: PAGE_ABOUT },
        2 => BrPage { url: "rustypenguin://ternary", title: "The ternary case",    lines: PAGE_TERNARY },
        3 => BrPage { url: "rustypenguin://roadmap", title: "Live web roadmap",    lines: PAGE_WEB },
        _ => BrPage { url: "rustypenguin://home",    title: "Start",               lines: PAGE_HOME },
    }
}

fn br_line_advance(kind: u8) -> i32 {
    match kind { K_H1 => 38, K_H2 => 26, K_P => 19, K_LINK => 24, K_NOTE => 19, _ => 11 }
}

/// sys_http_get(host, out) — kernel TCP/IP fetch (syscall #16).
unsafe fn sys_http_get_raw(host: &[u8], out: &mut [u8]) -> usize {
    let n: u64;
    let arg2 = (host.len() as u64 & 0xFFFF) | ((out.len() as u64) << 16);
    core::arch::asm!(
        "syscall",
        inout("rax") 16u64 => n,
        in("rdi") host.as_ptr(),
        in("rsi") arg2,
        in("rdx") out.as_mut_ptr(),
        out("rcx") _, out("r11") _,
        options(nostack),
    );
    n as usize
}

// Max bytes of fetched HTML the browser buffers. Fits in the 24 MiB heap.
const FETCH_BUF: usize = 16_384;
// Rendered lines extracted from the fetched HTML (heap).
const MAX_LIVE_LINES: usize = 200;

#[derive(Copy, Clone, PartialEq)]
enum BrMode {
    Static,     // showing a built-in local page (page index in Browser.page)
    Loading,    // sys_http_get in progress
    Live,       // showing a fetched remote page
    Err,        // last fetch failed
}

pub struct Browser {
    page:         usize,
    // URL bar
    url:          [u8; 128],
    url_len:      usize,
    addr_focused: bool,
    // Live page
    mode:         BrMode,
    resp_buf:     Vec<u8>,              // raw HTTP response bytes
    live_lines:   Vec<String>,          // extracted text lines for rendering
    scroll:       usize,                // first visible live line
    pub dirty:       bool,
    pub wants_close: bool,
    ansi:         crate::ansi::AnsiParser,
}

impl Browser {
    pub fn new() -> Self {
        const HOME: &[u8] = b"rustypenguin://home";
        let mut url = [0u8; 128];
        url[..HOME.len()].copy_from_slice(HOME);
        Browser {
            page: 0,
            url, url_len: HOME.len(),
            addr_focused: false,
            mode: BrMode::Static,
            resp_buf: Vec::new(),
            live_lines: Vec::new(),
            scroll: 0,
            dirty: true,
            wants_close: false,
            ansi: crate::ansi::AnsiParser::new(),
        }
    }

    /// Update the URL bar text to reflect the given slice.
    fn set_url(&mut self, s: &[u8]) {
        let n = s.len().min(self.url.len());
        self.url[..n].copy_from_slice(&s[..n]);
        self.url_len = n;
    }

    /// Navigate: detect rustypenguin:// schemes vs plain hostnames.
    fn navigate(&mut self) {
        let url = &self.url[..self.url_len];
        // Local scheme — map to built-in pages.
        if url.starts_with(b"rustypenguin://") || url.is_empty() {
            let slug = &url[url.iter().position(|&b| b == b'/').map(|p| p + 2).unwrap_or(0)..];
            let slug = if let Some(p) = slug.iter().position(|&b| b == b'/') { &slug[p+1..] } else { slug };
            self.page = match slug { b"about" => 1, b"ternary" => 2, b"roadmap" => 3, _ => 0 };
            self.mode = BrMode::Static;
            self.dirty = true;
            return;
        }
        // Strip leading http:// if present.
        let host = if url.starts_with(b"http://") { &url[7..] } else { url };
        // Strip trailing path (keep only the host part for the syscall).
        let host = if let Some(p) = host.iter().position(|&b| b == b'/') { &host[..p] } else { host };
        if host.is_empty() { return; }
        // Fetch.
        self.mode = BrMode::Loading;
        self.dirty = true;
        let mut buf = alloc::vec![0u8; FETCH_BUF];
        let n = unsafe { sys_http_get_raw(host, &mut buf) };
        if n == 0 {
            self.mode = BrMode::Err;
            self.dirty = true;
            return;
        }
        buf.truncate(n);
        self.resp_buf = buf;
        self.extract_lines();
        self.scroll = 0;
        self.mode = BrMode::Live;
        self.dirty = true;
    }

    /// Strip HTTP headers + HTML tags from resp_buf, produce text lines.
    fn extract_lines(&mut self) {
        self.live_lines.clear();
        let data = &self.resp_buf;
        // Skip HTTP headers: find \r\n\r\n or \n\n.
        let body_start = data.windows(4).position(|w| w == b"\r\n\r\n")
            .map(|p| p + 4)
            .or_else(|| data.windows(2).position(|w| w == b"\n\n").map(|p| p + 2))
            .unwrap_or(0);
        let html = &data[body_start..];
        // Simple tag stripper: walk, skip <...>, collect text.
        let mut text: Vec<u8> = Vec::with_capacity(html.len());
        let mut in_tag = false;
        let mut in_script = false;
        let mut i = 0;
        while i < html.len() {
            match html[i] {
                b'<' => {
                    // Detect <script / <style — skip until closing tag.
                    let rest = &html[i..];
                    if rest.len() > 7 && (rest[1..7].eq_ignore_ascii_case(b"script") || rest[1..6].eq_ignore_ascii_case(b"style")) {
                        in_script = true;
                    }
                    if in_script {
                        // scan for </script> or </style>
                        if let Some(p) = rest.windows(9).position(|w| {
                            w[..2] == *b"</" && w[2..8].eq_ignore_ascii_case(b"script")
                        }).or_else(|| rest.windows(8).position(|w| {
                            w[..2] == *b"</" && w[2..7].eq_ignore_ascii_case(b"style")
                        })) {
                            i += p;
                            in_script = false;
                            in_tag = true;
                        } else {
                            break; // malformed, bail
                        }
                    } else {
                        in_tag = true;
                        // Block-level tags → emit a newline so lines break.
                        let tag = if html.len() > i + 1 { html[i+1] } else { 0 };
                        if matches!(tag | 0x20, b'p'|b'h'|b'd'|b'l'|b't'|b'b') {
                            if !text.is_empty() && *text.last().unwrap() != b'\n' {
                                text.push(b'\n');
                            }
                        }
                    }
                    i += 1;
                }
                b'>' if in_tag => { in_tag = false; i += 1; }
                _ if in_tag => { i += 1; }
                b'&' => {
                    // Basic entity decode.
                    let rest = &html[i..];
                    if rest.starts_with(b"&amp;")       { text.push(b'&'); i += 5; }
                    else if rest.starts_with(b"&lt;")   { text.push(b'<'); i += 4; }
                    else if rest.starts_with(b"&gt;")   { text.push(b'>'); i += 4; }
                    else if rest.starts_with(b"&nbsp;") { text.push(b' '); i += 6; }
                    else { text.push(b'&'); i += 1; }
                }
                b'\r' => { i += 1; } // collapse \r\n → \n
                ch => { text.push(ch); i += 1; }
            }
        }
        // Word-wrap each text line at ~72 chars.
        const WRAP: usize = 72;
        for raw_line in text.split(|&b| b == b'\n') {
            let s = core::str::from_utf8(raw_line).unwrap_or("").trim();
            if s.is_empty() {
                if !self.live_lines.last().map(|l: &String| l.is_empty()).unwrap_or(true) {
                    self.live_lines.push(String::new());
                }
                continue;
            }
            if s.len() <= WRAP {
                self.live_lines.push(String::from(s));
            } else {
                let bytes = s.as_bytes();
                let mut start = 0;
                while start < bytes.len() {
                    let end = (start + WRAP).min(bytes.len());
                    let end = if end < bytes.len() {
                        // break at last space before end
                        bytes[start..end].iter().rposition(|&b| b == b' ')
                            .map(|p| start + p + 1)
                            .unwrap_or(end)
                    } else { end };
                    if let Ok(chunk) = core::str::from_utf8(&bytes[start..end]) {
                        let trimmed = chunk.trim();
                        if !trimmed.is_empty() { self.live_lines.push(String::from(trimmed)); }
                    }
                    if end <= start { break; }
                    start = end;
                }
            }
        }
        if self.live_lines.len() > MAX_LIVE_LINES {
            self.live_lines.truncate(MAX_LIVE_LINES);
        }
    }
}

impl App for Browser {
    fn render(&mut self, fb: &mut Framebuffer, x: u32, y: u32, w: u32, h: u32) {
        // ── Toolbar ───────────────────────────────────────────────────────────
        fb.fill_rect(x, y, w, BR_TOOLBAR_H, 0x2A332F);
        fb.fill_rect(x, y + BR_TOOLBAR_H, w, 1, 0x55615A);
        let by = y as i32 + 5;
        // Back button
        let can_back = self.mode == BrMode::Static && self.page != 0
                    || self.mode == BrMode::Live || self.mode == BrMode::Err;
        let bg_back = if can_back { 0x39443E } else { 0x232C28 };
        let fg_back = if can_back { 0x6FE18B } else { 0x6B756D };
        fb.fill_rounded_rect(x as i32 + 6, by, 22, 22, 6, bg_back);
        fb.draw_str(x + 14, y + 12, "<", fg_back, bg_back);
        // Address bar
        let ax = x + 6 + 3 * 26 + 6;
        let aw = (x + w).saturating_sub(ax + 8);
        let bar_bg = if self.addr_focused { 0x2A3A28 } else { 0x1A211C };
        fb.fill_rounded_rect(ax as i32, by, aw as i32, 22, 8, bar_bg);
        // lock dot (green = rustypenguin, blue = http, orange = err)
        let dot_col = match self.mode {
            BrMode::Static  => 0x6FE18B,
            BrMode::Live    => 0x4A9EFF,
            BrMode::Loading => 0xF5C451,
            BrMode::Err     => 0xEF4444,
        };
        fb.fill_circle((ax + 12) as i32, (by + 11), 3, dot_col);
        let url_str = core::str::from_utf8(&self.url[..self.url_len]).unwrap_or("");
        let cursor = if self.addr_focused { "_" } else { "" };
        let mut ubuf = String::from(url_str);
        ubuf.push_str(cursor);
        fb.draw_aa((ax + 22) as i32, by + 4, &ubuf, 0xCFE6D6, crate::fb::AA_S);

        // ── Page area ─────────────────────────────────────────────────────────
        let py0 = y + BR_TOOLBAR_H + 1;
        let ph  = h.saturating_sub(BR_TOOLBAR_H + 1);
        fb.fill_rect(x, py0, w, ph, 0xF2F0E8);
        let lx = x as i32 + 24;

        match self.mode {
            BrMode::Static => {
                let page = br_page(self.page);
                let mut cy = py0 as i32 + 14;
                for ln in page.lines {
                    if cy as u32 + 30 > py0 + ph { break; }
                    match ln.kind {
                        K_H1   => { fb.draw_aa(lx, cy, ln.text, 0x1B6B3A, crate::fb::AA_L); }
                        K_H2   => { fb.draw_aa(lx, cy, ln.text, 0xB4502A, crate::fb::AA_S); }
                        K_P    => { fb.draw_aa(lx, cy, ln.text, 0x3A3A36, crate::fb::AA_S); }
                        K_NOTE => { fb.draw_aa(lx, cy, ln.text, 0x8A8A82, crate::fb::AA_T); }
                        K_LINK => {
                            let lw = Framebuffer::aa_w(ln.text, crate::fb::AA_S);
                            fb.draw_aa(lx, cy, ln.text, 0x2A6FB0, crate::fb::AA_S);
                            fb.fill_rect(lx as u32, cy as u32 + 17, lw as u32, 1, 0x2A6FB0);
                        }
                        _ => {}
                    }
                    cy += br_line_advance(ln.kind);
                }
            }
            BrMode::Loading => {
                fb.draw_aa(lx, py0 as i32 + 40, "Fetching page...", 0x8A8A82, crate::fb::AA_S);
            }
            BrMode::Err => {
                fb.draw_aa(lx, py0 as i32 + 40, "Could not load page.", 0xEF4444, crate::fb::AA_S);
                fb.draw_aa(lx, py0 as i32 + 62, "Check hostname or network.", 0x8A8A82, crate::fb::AA_S);
            }
            BrMode::Live => {
                let line_h: i32 = 19;
                let visible = (ph as i32 / line_h).max(1) as usize;
                let start = self.scroll.min(self.live_lines.len().saturating_sub(1));
                let mut cy = py0 as i32 + 10;
                for line in self.live_lines.iter().skip(start).take(visible) {
                    if cy as u32 + 20 > py0 + ph { break; }
                    if line.is_empty() { cy += line_h / 2; continue; }
                    fb.draw_aa(lx, cy, line.as_str(), 0x3A3A36, crate::fb::AA_S);
                    cy += line_h;
                }
                // Scroll indicator
                if self.live_lines.len() > visible {
                    let bar_h = (ph * visible as u32 / self.live_lines.len() as u32).max(6);
                    let bar_y = py0 + start as u32 * ph / self.live_lines.len() as u32;
                    fb.fill_rect(x + w - 5, bar_y, 3, bar_h, 0xB8C8B0);
                }
            }
        }
        self.dirty = false;
    }

    fn on_mouse(&mut self, mx: i32, my: i32, w: u32, h: u32, buttons: u8) {
        if buttons & 1 == 0 { return; }
        let by = 5i32;
        let ax = (6 + 3 * 26 + 6) as i32;
        let aw = (w as i32).saturating_sub(ax + 8);
        // Click address bar → focus it
        if mx >= ax && mx < ax + aw && my >= by && my < by + 22 {
            self.addr_focused = true;
            self.dirty = true;
            return;
        }
        self.addr_focused = false;
        // Back button
        if mx >= 6 && mx < 28 && my >= 5 && my < 27 {
            self.go_back();
            return;
        }
        // Static page link clicks
        if self.mode == BrMode::Static {
            let page = br_page(self.page);
            let mut cy = (BR_TOOLBAR_H as i32) + 1 + 14;
            for ln in page.lines {
                let adv = br_line_advance(ln.kind);
                if ln.kind == K_LINK && ln.link >= 0 {
                    let lw = Framebuffer::aa_w(ln.text, crate::fb::AA_S);
                    if mx >= 24 && mx < 24 + lw && my >= cy && my < cy + adv {
                        self.page = ln.link as usize;
                        self.update_url_from_page();
                        self.dirty = true;
                        return;
                    }
                }
                cy += adv;
            }
        }
        // Live page scroll (click in page area = scroll toward click position)
        if self.mode == BrMode::Live {
            let page_h = h as i32 - BR_TOOLBAR_H as i32 - 1;
            if my > BR_TOOLBAR_H as i32 {
                if my < BR_TOOLBAR_H as i32 + page_h / 2 {
                    self.scroll = self.scroll.saturating_sub(3);
                } else {
                    self.scroll = (self.scroll + 3).min(self.live_lines.len().saturating_sub(1));
                }
                self.dirty = true;
            }
        }
    }

    fn on_key(&mut self, key: u8) {
        use crate::ansi::Key as AK;
        if self.addr_focused {
            match self.ansi.feed(key) {
                AK::Char(b'\n') | AK::Char(b'\r') => {
                    self.addr_focused = false;
                    self.navigate();
                }
                AK::Char(0x08) | AK::Char(0x7F) => {
                    if self.url_len > 0 { self.url_len -= 1; self.dirty = true; }
                }
                AK::Char(ch) if ch >= 0x20 && ch < 0x7F => {
                    if self.url_len < self.url.len() - 1 {
                        self.url[self.url_len] = ch;
                        self.url_len += 1;
                        self.dirty = true;
                    }
                }
                AK::Up   => { self.scroll = self.scroll.saturating_sub(1); self.dirty = true; }
                AK::Down => { self.scroll = (self.scroll + 1).min(self.live_lines.len().saturating_sub(1)); self.dirty = true; }
                _ => {}
            }
        } else {
            match self.ansi.feed(key) {
                AK::Up   => { self.scroll = self.scroll.saturating_sub(1); self.dirty = true; }
                AK::Down => { self.scroll = (self.scroll + 1).min(self.live_lines.len().saturating_sub(1)); self.dirty = true; }
                _ => {}
            }
        }
    }

    fn wants_close(&self) -> bool { self.wants_close }
    fn title(&self) -> &str { "Web" }
}

impl Browser {
    fn go_back(&mut self) {
        match self.mode {
            BrMode::Live | BrMode::Err => {
                self.mode = BrMode::Static;
                self.page = 0;
                self.update_url_from_page();
            }
            BrMode::Static if self.page != 0 => {
                self.page = 0;
                self.update_url_from_page();
            }
            _ => {}
        }
        self.dirty = true;
    }
    fn update_url_from_page(&mut self) {
        let url = br_page(self.page).url.as_bytes();
        let n = url.len().min(self.url.len());
        self.url[..n].copy_from_slice(&url[..n]);
        self.url_len = n;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Sound — audio mixer/player. Uses sys_audio_write (#17) and sys_audio_vol (#18)
// to control the Intel HDA DMA stream. Generates tones via ternary-encoded
// sine tables and lets the user play scales, set volume, and see a live bar meter.
// ─────────────────────────────────────────────────────────────────────────────

const AUDIO_BYTES: usize = 0x0002_0000; // 128 KiB ring buffer (matches kernel)
const AUDIO_SR:    usize = 44_100;      // samples/sec
const AUDIO_CH:    usize = 2;           // stereo
const FRAME_SZ:    usize = 4;           // 16-bit stereo = 4 bytes/frame

unsafe fn sys_audio_write(pcm: &[u8], offset: usize) -> usize {
    let n: u64;
    core::arch::asm!(
        "syscall",
        inout("rax") 17u64 => n,
        in("rdi") pcm.as_ptr(),
        in("rsi") pcm.len() as u64,
        in("rdx") offset as u64,
        out("rcx") _, out("r11") _,
        options(nostack),
    );
    n as usize
}
unsafe fn sys_audio_vol(vol: u8) {
    core::arch::asm!(
        "syscall",
        in("rax") 18u64,
        in("rdi") vol as u64,
        out("rcx") _, out("r11") _,
        options(nostack),
    );
}

// Quarter-sine table (25 entries, 0..π/2 at amplitude 32767).
const QSINE: [i16; 26] = [0,2057,4106,6140,8149,10126,12062,13951,
    15785,17557,19260,20886,22430,23886,25247,26509,27666,28714,29648,
    30465,31160,31730,32168,32473,32642,32767];

fn sine_sample(phase: usize, period: usize) -> i16 {
    let p = phase % period;
    let idx = p * 100 / period;
    let rem = ((idx % 25) * 25 / 25).min(25);
    if idx < 25      { QSINE[rem] }
    else if idx < 50 { QSINE[25 - (idx - 25).min(25)] }
    else if idx < 75 { -(QSINE[(idx - 50).min(25)]) }
    else             { -(QSINE[25 - (idx - 75).min(25)]) }
}

// 12-tone equal-tempered scale starting at A4=440 Hz. Period in samples @ 44100.
const SCALE_NAMES: [&str; 13] = ["A4","A#4","B4","C5","C#5","D5","D#5","E5","F5","F#5","G5","G#5","A5"];
const SCALE_HZ: [u32; 13] = [440,466,494,523,554,587,622,659,698,740,784,831,880];

pub struct Sound {
    vol:         u8,           // 0–127
    playing:     bool,
    note_idx:    usize,        // index into SCALE_NAMES
    phase:       usize,        // DMA write phase (frame count)
    pub dirty:   bool,
    pub wants_close: bool,
    ansi:        crate::ansi::AnsiParser,
}

impl Sound {
    pub fn new() -> Self {
        Sound { vol: 64, playing: false, note_idx: 0, phase: 0,
                dirty: true, wants_close: false, ansi: crate::ansi::AnsiParser::new() }
    }

    fn write_tone(&mut self) {
        // Fill the DMA buffer with one period of the current note.
        let hz = SCALE_HZ[self.note_idx] as usize;
        let period = AUDIO_SR / hz;
        let n_frames = AUDIO_BYTES / FRAME_SZ;
        let mut pcm = alloc::vec![0u8; AUDIO_BYTES];
        for i in 0..n_frames {
            let s = sine_sample(i, period);
            let lo = (s & 0xFF) as u8;
            let hi = (s >> 8) as u8;
            // Stereo: L then R
            pcm[i * 4]     = lo; pcm[i * 4 + 1] = hi;  // L
            pcm[i * 4 + 2] = lo; pcm[i * 4 + 3] = hi;  // R
        }
        unsafe { sys_audio_write(&pcm, 0); }
    }

    fn set_vol(&mut self, v: u8) {
        self.vol = v;
        unsafe { sys_audio_vol(v); }
        self.dirty = true;
    }
}

impl App for Sound {
    fn render(&mut self, fb: &mut Framebuffer, x: u32, y: u32, w: u32, h: u32) {
        fb.fill_rect(x, y, w, h, 0x1A1A24);

        // Title
        fb.fill_rect(x, y, w, 28, 0x2C2C38);
        fb.draw_str(x + 12, y + 9, "Sound", 0xF5F5F7, 0x2C2C38);
        fb.draw_str(x + 80, y + 9, "Intel HDA  44.1 kHz  stereo  16-bit", 0x6B756D, 0x2C2C38);

        let cx = x as i32;
        let mut oy = y as i32 + 40;

        // Waveform preview bar (visualize current note as a simple sine curve).
        let bar_w = w.saturating_sub(48) as i32;
        let bar_h = 48i32;
        fb.fill_rect(x + 24, oy as u32, bar_w as u32, bar_h as u32, 0x111118);
        if self.playing {
            let hz = SCALE_HZ[self.note_idx] as usize;
            let period = (AUDIO_SR / hz).max(1);
            let n = bar_w as usize;
            for xi in 0..n.saturating_sub(1) {
                let s0 = sine_sample(xi * period / n, period);
                let s1 = sine_sample((xi + 1) * period / n, period);
                let y0 = oy + bar_h / 2 - (s0 as i32 * (bar_h / 2 - 4) / 32767);
                let y1 = oy + bar_h / 2 - (s1 as i32 * (bar_h / 2 - 4) / 32767);
                let x0 = x as i32 + 24 + xi as i32;
                // draw a vertical line between y0 and y1 (1px wide waveform)
                let (ya, yb) = if y0 <= y1 { (y0, y1) } else { (y1, y0) };
                for py in ya..=yb {
                    if py >= oy && py < oy + bar_h {
                        fb.fill_rect(x0 as u32, py as u32, 1, 1, 0x4A9EFF);
                    }
                }
            }
        } else {
            fb.draw_str(x + 24 + (bar_w as u32 / 2).saturating_sub(24),
                         (oy + bar_h / 2 - 4) as u32, "STOPPED", 0x3A3A48, 0x111118);
        }
        oy += bar_h + 14;

        // Note selector row
        fb.draw_str(cx as u32 + 12, oy as u32, "Note:", 0xB8B8B8, 0x1A1A24);
        let nx = cx + 60;
        for (i, name) in SCALE_NAMES.iter().enumerate() {
            let bx = nx + i as i32 * 34;
            let sel = i == self.note_idx;
            let bg = if sel { 0x2E7D4F } else { 0x2C2C38 };
            let fg = if sel { 0xF5F5F7 } else { 0x9CA3AF };
            fb.fill_rect(bx as u32, oy as u32, 30, 22, bg);
            fb.draw_str(bx as u32 + 3, oy as u32 + 7, name, fg, bg);
        }
        oy += 30;

        // Volume slider
        fb.draw_str(cx as u32 + 12, oy as u32 + 4, "Vol:", 0xB8B8B8, 0x1A1A24);
        let vx = cx + 55;
        let vw = 200i32;
        fb.fill_rect(vx as u32, oy as u32 + 8, vw as u32, 8, 0x2C2C38);
        let filled = (self.vol as i32 * vw / 127).max(0).min(vw) as u32;
        fb.fill_rect(vx as u32, oy as u32 + 8, filled, 8, 0x4A9EFF);
        let mut vbuf = [0u8; 24];
        let vs = u64_into(&mut vbuf, self.vol as u64);
        fb.draw_str(vx as u32 + vw as u32 + 8, oy as u32 + 4, vs, 0xB8B8B8, 0x1A1A24);
        // Vol –/+ buttons
        fb.fill_rect(vx as u32 + vw as u32 + 36, oy as u32, 22, 22, 0x2C2C38);
        fb.draw_str(vx as u32 + vw as u32 + 43, oy as u32 + 7, "-", 0xF5C451, 0x2C2C38);
        fb.fill_rect(vx as u32 + vw as u32 + 62, oy as u32, 22, 22, 0x2C2C38);
        fb.draw_str(vx as u32 + vw as u32 + 69, oy as u32 + 7, "+", 0x6FE18B, 0x2C2C38);
        oy += 36;

        // Play/Stop button
        let (btn_label, btn_bg) = if self.playing { ("Stop ", 0x7D2E2E) } else { ("Play ", 0x2E7D4F) };
        fb.fill_rect(cx as u32 + 12, oy as u32, 80, 30, btn_bg);
        fb.draw_str(cx as u32 + 24, oy as u32 + 10, btn_label, 0xF5F5F7, btn_bg);

        self.dirty = false;
    }

    fn on_mouse(&mut self, mx: i32, my: i32, _w: u32, _h: u32, buttons: u8) {
        if buttons & 1 == 0 { return; }
        let oy_notes = 40 + 48 + 14;
        let oy_vol   = oy_notes + 30;
        let oy_btn   = oy_vol + 36;

        // Note buttons (row at oy_notes)
        if my >= oy_notes && my < oy_notes + 22 {
            let xi = (mx - 60) / 34;
            if xi >= 0 && (xi as usize) < SCALE_NAMES.len() {
                self.note_idx = xi as usize;
                if self.playing { self.write_tone(); }
                self.dirty = true;
                return;
            }
        }
        // Volume –
        let vx = 55; let vw = 200i32;
        if my >= oy_vol && my < oy_vol + 22 {
            let vbx = vx + vw + 36;
            if mx >= vbx && mx < vbx + 22 {
                self.set_vol(self.vol.saturating_sub(8));
                return;
            }
            let pbx = vx + vw + 62;
            if mx >= pbx && mx < pbx + 22 {
                self.set_vol((self.vol as u16 + 8).min(127) as u8);
                return;
            }
            // Click on the slider track → set volume proportionally
            if mx >= vx && mx < vx + vw {
                let v = ((mx - vx) * 127 / vw) as u8;
                self.set_vol(v);
                return;
            }
        }
        // Play/Stop button
        if my >= oy_btn && my < oy_btn + 30 && mx >= 12 && mx < 92 {
            self.playing = !self.playing;
            if self.playing { self.write_tone(); }
            self.dirty = true;
        }
    }

    fn on_key(&mut self, key: u8) {
        use crate::ansi::Key as AK;
        match self.ansi.feed(key) {
            AK::Char(b' ') => {
                self.playing = !self.playing;
                if self.playing { self.write_tone(); }
                self.dirty = true;
            }
            AK::Left  => { if self.note_idx > 0 { self.note_idx -= 1; if self.playing { self.write_tone(); } self.dirty = true; } }
            AK::Right => { if self.note_idx + 1 < SCALE_NAMES.len() { self.note_idx += 1; if self.playing { self.write_tone(); } self.dirty = true; } }
            AK::Up    => self.set_vol((self.vol as u16 + 8).min(127) as u8),
            AK::Down  => self.set_vol(self.vol.saturating_sub(8)),
            _ => {}
        }
    }

    fn title(&self) -> &str { "Sound" }
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

// ─────────────────────────────────────────────────────────────────────────────
// Doom — a pure-Rust raycaster FPS (Lode-style DDA). NOT id Software's DOOM
// (that C engine runs on the Linux track via the ISO's DOOM boot entry); this
// is a from-scratch tribute that fits the no_std bare-metal desktop: only f32
// +,-,*,/ and casts — no trig, no sqrt at runtime (turning uses a constant
// rotation matrix), no per-frame heap allocation (zbuffer is on the stack).
// ─────────────────────────────────────────────────────────────────────────────

const DM_W: usize = 16;
const DM_H: usize = 16;
#[rustfmt::skip]
const DOOM_MAP: [u8; DM_W * DM_H] = [
    1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,
    1,0,0,0,0,0,2,0,0,0,0,3,0,0,0,1,
    1,0,1,1,0,0,2,0,0,0,0,3,0,1,0,1,
    1,0,1,0,0,0,0,0,1,1,0,0,0,1,0,1,
    1,0,1,0,0,0,0,0,1,0,0,0,0,0,0,1,
    1,0,0,0,0,4,4,4,1,0,0,2,2,2,0,1,
    1,0,0,0,0,4,0,0,0,0,0,0,0,2,0,1,
    1,0,0,0,0,4,0,1,1,1,0,0,0,2,0,1,
    1,0,3,3,0,0,0,1,0,1,0,0,0,0,0,1,
    1,0,3,0,0,0,0,1,0,1,1,1,0,0,0,1,
    1,0,3,0,0,0,0,0,0,0,0,4,0,0,0,1,
    1,0,0,0,1,1,0,0,0,0,0,4,0,2,2,1,
    1,0,0,0,1,0,0,0,3,3,0,0,0,0,0,1,
    1,0,1,0,0,0,0,0,3,0,0,0,1,1,0,1,
    1,0,1,0,0,0,0,0,0,0,0,0,0,0,0,1,
    1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,
];

// Constant rotation matrix for one turn step (~6.9°): cos/sin precomputed so we
// never call a trig function at runtime.
const ROT_C: f32 = 0.99272;
const ROT_S: f32 = 0.12050;
const MOVE_STEP: f32 = 0.18;

fn fabs(x: f32) -> f32 { if x < 0.0 { -x } else { x } }

pub struct Doom {
    px: f32, py: f32,        // player position
    dx: f32, dy: f32,        // direction vector
    plx: f32, ply: f32,      // camera plane
    enemies: Vec<(f32, f32, bool)>, // x, y, alive
    kills: u32,
    flash: u8,               // muzzle-flash countdown
    pub dirty: bool,
    pub wants_close: bool,
    ansi: crate::ansi::AnsiParser,
}

impl Doom {
    pub fn new(_seed: u64) -> Self {
        let mut enemies = Vec::with_capacity(8);
        enemies.push((6.5, 2.5, true));
        enemies.push((11.5, 11.5, true));
        enemies.push((2.5, 8.5, true));
        enemies.push((13.5, 6.5, true));
        enemies.push((8.5, 13.5, true));
        Doom {
            px: 4.5, py: 4.5,
            dx: -1.0, dy: 0.0,
            plx: 0.0, ply: 0.66,
            enemies,
            kills: 0,
            flash: 0,
            dirty: true,
            wants_close: false,
            ansi: crate::ansi::AnsiParser::new(),
        }
    }

    fn cell(x: f32, y: f32) -> u8 {
        let xi = x as i32; let yi = y as i32;
        if xi < 0 || yi < 0 || xi >= DM_W as i32 || yi >= DM_H as i32 { return 1; }
        DOOM_MAP[yi as usize * DM_W + xi as usize]
    }

    fn try_move(&mut self, nx: f32, ny: f32) {
        // Slide along walls: test each axis independently.
        if Self::cell(nx, self.py) == 0 { self.px = nx; }
        if Self::cell(self.px, ny) == 0 { self.py = ny; }
        self.dirty = true;
    }

    fn rotate(&mut self, c: f32, s: f32) {
        let odx = self.dx;
        self.dx = self.dx * c - self.dy * s;
        self.dy = odx * s + self.dy * c;
        let oplx = self.plx;
        self.plx = self.plx * c - self.ply * s;
        self.ply = oplx * s + self.ply * c;
        self.dirty = true;
    }

    fn fire(&mut self) {
        self.flash = 4;
        // Hit the nearest alive enemy roughly in front of the crosshair.
        let mut best: Option<usize> = None;
        let mut best_d = 1e30f32;
        for (i, &(ex, ey, alive)) in self.enemies.iter().enumerate() {
            if !alive { continue; }
            let rx = ex - self.px; let ry = ey - self.py;
            let dist = rx * self.dx + ry * self.dy;       // forward distance
            if dist <= 0.2 { continue; }
            let side = rx * self.plx + ry * self.ply;     // lateral offset
            // Within a narrow cone that tightens with distance ≈ crosshair.
            if fabs(side) < 0.35 * dist && dist < best_d {
                best_d = dist; best = Some(i);
            }
        }
        if let Some(i) = best { self.enemies[i].2 = false; self.kills += 1; }
        self.dirty = true;
    }
}

impl App for Doom {
    fn tick(&mut self, _t: u64) -> bool {
        if self.flash > 0 { self.flash -= 1; return true; }
        false
    }

    fn render(&mut self, fb: &mut Framebuffer, x: u32, y: u32, w: u32, h: u32) {
        if w == 0 || h == 0 { return; }
        let cols = (w as usize).min(1024);
        let mut zbuf = [1e30f32; 1024];

        // Ceiling (dark) and floor (gray).
        fb.fill_rect(x, y, w, h / 2, 0x202830);
        fb.fill_rect(x, y + h / 2, w, h - h / 2, 0x383838);

        // Cast one ray per column.
        for col in 0..cols {
            let camera = 2.0 * (col as f32) / (w as f32) - 1.0;
            let rdx = self.dx + self.plx * camera;
            let rdy = self.dy + self.ply * camera;

            let mut map_x = self.px as i32;
            let mut map_y = self.py as i32;
            let ddx = if rdx == 0.0 { 1e30 } else { fabs(1.0 / rdx) };
            let ddy = if rdy == 0.0 { 1e30 } else { fabs(1.0 / rdy) };

            let (step_x, mut sdx) = if rdx < 0.0 {
                (-1i32, (self.px - map_x as f32) * ddx)
            } else {
                (1i32, (map_x as f32 + 1.0 - self.px) * ddx)
            };
            let (step_y, mut sdy) = if rdy < 0.0 {
                (-1i32, (self.py - map_y as f32) * ddy)
            } else {
                (1i32, (map_y as f32 + 1.0 - self.py) * ddy)
            };

            let mut side = 0;
            let mut hit = 0u8;
            for _ in 0..64 {
                if sdx < sdy { sdx += ddx; map_x += step_x; side = 0; }
                else         { sdy += ddy; map_y += step_y; side = 1; }
                if map_x < 0 || map_y < 0 || map_x >= DM_W as i32 || map_y >= DM_H as i32 { hit = 1; break; }
                let c = DOOM_MAP[map_y as usize * DM_W + map_x as usize];
                if c != 0 { hit = c; break; }
            }

            let perp = if side == 0 {
                (map_x as f32 - self.px + (1 - step_x) as f32 / 2.0) / rdx
            } else {
                (map_y as f32 - self.py + (1 - step_y) as f32 / 2.0) / rdy
            };
            let perp = if perp < 0.01 { 0.01 } else { perp };
            zbuf[col] = perp;

            let line_h = (h as f32 / perp) as i32;
            let mut draw_start = -line_h / 2 + h as i32 / 2;
            let mut draw_end = line_h / 2 + h as i32 / 2;
            if draw_start < 0 { draw_start = 0; }
            if draw_end > h as i32 { draw_end = h as i32; }

            // Base color per wall type, darkened on y-sides and with distance.
            let base = match hit {
                2 => 0xB04040u32, // red brick
                3 => 0x40A060u32, // green
                4 => 0x4060B0u32, // blue
                _ => 0xA0A0A0u32, // gray stone
            };
            let mut col_rgb = if side == 1 { (base >> 1) & 0x7F7F7F } else { base };
            // Distance shade.
            let shade = if perp > 6.0 { 3 } else if perp > 3.5 { 2 } else if perp > 1.8 { 1 } else { 0 };
            col_rgb = (col_rgb >> shade) & (0xFFFFFF >> shade);

            let sh = (draw_end - draw_start).max(0) as u32;
            if sh > 0 {
                fb.fill_rect(x + col as u32, y + draw_start as u32, 1, sh, col_rgb);
            }
        }

        // Sprites (enemies): sort far→near, billboard, z-test per column.
        // Stack-only ordering — no per-frame heap allocation (bump allocator
        // never frees, so a render-path Vec would leak every frame).
        let mut order = [0usize; 16];
        let mut ocount = 0usize;
        for i in 0..self.enemies.len().min(16) {
            if self.enemies[i].2 { order[ocount] = i; ocount += 1; }
        }
        // insertion sort by distance descending
        for a in 1..ocount {
            let key = order[a];
            let kd = sqdist(self.px, self.py, self.enemies[key].0, self.enemies[key].1);
            let mut b = a;
            while b > 0 && sqdist(self.px, self.py, self.enemies[order[b-1]].0, self.enemies[order[b-1]].1) < kd {
                order[b] = order[b-1]; b -= 1;
            }
            order[b] = key;
        }
        for &i in &order[..ocount] {
            let (ex, ey, _) = self.enemies[i];
            let rx = ex - self.px; let ry = ey - self.py;
            // Inverse camera transform.
            let inv = 1.0 / (self.plx * self.dy - self.dx * self.ply);
            let tx = inv * (self.dy * rx - self.dx * ry);
            let ty = inv * (-self.ply * rx + self.plx * ry); // depth
            if ty <= 0.1 { continue; }
            let screen_x = ((w as f32 / 2.0) * (1.0 + tx / ty)) as i32;
            let sprite_h = (h as f32 / ty) as i32;
            let sp_top = (-sprite_h / 2 + h as i32 / 2).max(0);
            let sp_bot = (sprite_h / 2 + h as i32 / 2).min(h as i32);
            let sprite_w = sprite_h / 2;
            let col_start = (screen_x - sprite_w / 2).max(0);
            let col_end = (screen_x + sprite_w / 2).min(w as i32);
            for sc in col_start..col_end {
                if sc < 0 || sc as usize >= cols { continue; }
                if ty >= zbuf[sc as usize] { continue; } // behind wall
                let sh = (sp_bot - sp_top).max(0) as u32;
                if sh > 0 {
                    // crude imp: body + darker head band
                    fb.fill_rect(x + sc as u32, y + sp_top as u32, 1, sh, 0x7A3010);
                }
            }
            // head accent
            if sp_bot > sp_top {
                let hx = (screen_x).max(0).min(w as i32 - 1);
                let head_h = ((sp_bot - sp_top) / 4).max(1) as u32;
                fb.fill_rect(x + hx as u32, y + sp_top as u32, 2, head_h, 0xC05020);
            }
        }

        // Gun (bottom center) + muzzle flash.
        let gun_w = w / 6;
        let gun_x = x + w / 2 - gun_w / 2;
        let gun_h = h / 5;
        fb.fill_rect(gun_x, y + h - gun_h, gun_w, gun_h, 0x303030);
        fb.fill_rect(gun_x + gun_w / 3, y + h - gun_h - gun_h / 2, gun_w / 3, gun_h / 2, 0x202020);
        if self.flash > 0 {
            let fx = x + w / 2 - 6;
            let fy = y + h - gun_h - gun_h / 2 - 10;
            fb.fill_rect(fx, fy, 12, 10, 0xFFE060);
        }

        // Crosshair.
        let cx = x + w / 2; let cy = y + h / 2;
        fb.fill_rect(cx - 5, cy, 11, 1, 0xE0E0E0);
        fb.fill_rect(cx, cy - 5, 1, 11, 0xE0E0E0);

        // HUD.
        fb.fill_rect(x, y, w, 16, 0x101418);
        fb.draw_str(x + 6, y + 4, "DOOM (pure-Rust raycaster)", 0xEF4444, 0x101418);
        let mut kb = [0u8; 24];
        fb.draw_str(x + w - 90, y + 4, "kills:", 0x9CA3AF, 0x101418);
        fb.draw_str(x + w - 42, y + 4, u64_into(&mut kb, self.kills as u64), 0xF5F5F7, 0x101418);

        self.dirty = false;
    }

    fn on_key(&mut self, key: u8) {
        use crate::ansi::Key as AK;
        match self.ansi.feed(key) {
            AK::Up    | AK::Char(b'w') | AK::Char(b'W') => {
                self.try_move(self.px + self.dx * MOVE_STEP, self.py + self.dy * MOVE_STEP);
            }
            AK::Down  | AK::Char(b's') | AK::Char(b'S') => {
                self.try_move(self.px - self.dx * MOVE_STEP, self.py - self.dy * MOVE_STEP);
            }
            AK::Left  | AK::Char(b'a') | AK::Char(b'A') => self.rotate(ROT_C, ROT_S),
            AK::Right | AK::Char(b'd') | AK::Char(b'D') => self.rotate(ROT_C, -ROT_S),
            AK::Char(b'q') | AK::Char(b'Q') => {
                // strafe left (perpendicular to dir)
                self.try_move(self.px + self.dy * MOVE_STEP, self.py - self.dx * MOVE_STEP);
            }
            AK::Char(b'e') | AK::Char(b'E') => {
                self.try_move(self.px - self.dy * MOVE_STEP, self.py + self.dx * MOVE_STEP);
            }
            AK::Char(b' ') | AK::Char(b'\n') | AK::Char(b'\r') => self.fire(),
            _ => {}
        }
    }

    fn title(&self) -> &str { "Doom" }
}

fn sqdist(ax: f32, ay: f32, bx: f32, by: f32) -> f32 {
    let dx = ax - bx; let dy = ay - by; dx * dx + dy * dy
}
