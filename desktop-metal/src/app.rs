// Application framework for Rusty Penguin
// Apps implement this trait to be launchable desktop applications.

use crate::fb::Framebuffer;
use crate::vfs;
use alloc::string::String;
use alloc::vec::Vec;

extern crate libm;

/// sys_ticks (#4) — 100 Hz tick counter since boot.
fn sys_ticks() -> u64 {
    let n: u64;
    unsafe {
        core::arch::asm!("syscall", inout("rax") 4u64 => n, in("rdi") 0u64,
            out("rcx") _, out("r11") _, options(nostack));
    }
    n
}
/// sys_rtc (#13) — packed CMOS clock: wday|sec<<8|min<<16|hour<<24|mday<<32|month<<40.
fn sys_rtc() -> u64 {
    let n: u64;
    unsafe {
        core::arch::asm!("syscall", inout("rax") 13u64 => n, in("rdi") 0u64,
            out("rcx") _, out("r11") _, options(nostack));
    }
    n
}
/// sys_meminfo (#5) → (free_mib, total_mib).
fn sys_meminfo() -> (u32, u32) {
    let n: u64;
    unsafe {
        core::arch::asm!("syscall", inout("rax") 5u64 => n, in("rdi") 0u64,
            out("rcx") _, out("r11") _, options(nostack));
    }
    ((n >> 32) as u32, (n & 0xFFFF_FFFF) as u32)
}
/// sys_cpu (#33) → CPU busy permille (0..1000) over the last sampling window.
fn sys_cpu() -> u32 {
    let n: u64;
    unsafe {
        core::arch::asm!("syscall", inout("rax") 33u64 => n, in("rdi") 0u64,
            out("rcx") _, out("r11") _, options(nostack));
    }
    n as u32
}
/// sys_ps (#9) → fills `buf` with up to `max` 32-byte process records
/// ([pid u64][state u8][7 pad][name 16]); returns the record count.
unsafe fn sys_ps(buf: &mut [u8], max: usize) -> usize {
    let n: u64;
    core::arch::asm!("syscall", inout("rax") 9u64 => n, in("rdi") buf.as_mut_ptr(),
        in("rsi") max as u64, out("rcx") _, out("r11") _, options(nostack));
    n as usize
}

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

/// Number of decimal digits in `n` (≥1). For laying text out next to a number.
fn count_digits(mut n: u32) -> u32 { let mut d = 1; while n >= 10 { n /= 10; d += 1; } d }

/// Human-readable byte size: "512 B", "4 KB", "12 MB", "3 GB". Integer units,
/// no heap. Writes into `buf` and returns the slice as &str.
fn fmt_size<'a>(buf: &'a mut [u8; 24], n: u64) -> &'a str {
    let (val, unit): (u64, &[u8]) =
        if n < 1024 { (n, b"B") }
        else if n < 1 << 20 { (n >> 10, b"KB") }
        else if n < 1 << 30 { (n >> 20, b"MB") }
        else { (n >> 30, b"GB") };
    let mut tmp = [0u8; 24];
    let ds = u64_into(&mut tmp, val);
    let mut k = 0usize;
    for &b in ds.as_bytes() { if k < 23 { buf[k] = b; k += 1; } }
    if k < 23 { buf[k] = b' '; k += 1; }
    for &b in unit { if k < 24 { buf[k] = b; k += 1; } }
    core::str::from_utf8(&buf[..k]).unwrap_or("")
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
    sort_mode: u8,   // 0 name A-Z, 1 name Z-A, 2 size large→small, 3 size small→large
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
            sort_mode: 0,
            dirty: true,
            wants_close: false,
            ansi: crate::ansi::AnsiParser::new(),
        };
        fm.refresh();
        fm
    }

    /// Sort the current entries by the active sort mode. Stable, no heap churn
    /// beyond the in-place sort.
    fn apply_sort(&mut self) {
        match self.sort_mode {
            0 => self.entries.sort_by(|a, b| a.name.cmp(&b.name)),
            1 => self.entries.sort_by(|a, b| b.name.cmp(&a.name)),
            2 => self.entries.sort_by(|a, b| b.size.cmp(&a.size).then(a.name.cmp(&b.name))),
            _ => self.entries.sort_by(|a, b| a.size.cmp(&b.size).then(a.name.cmp(&b.name))),
        }
    }

    fn sort_label(&self) -> &'static str {
        match self.sort_mode { 0 => "Name A-Z", 1 => "Name Z-A", 2 => "Size big", _ => "Size small" }
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
        self.apply_sort();
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
        let xi = x as i32; let wi = w as i32;
        const TEAL: u32 = 0x2DD4BF; const BLUE: u32 = 0x8CC6E5;
        let line = Framebuffer::aa_line(crate::fb::AA_S);

        // Body
        fb.fill_rect(x, y, w, h, 0x14181F);

        // Header: current path
        fb.fill_rect(x, y, w, 28, 0x1B2230);
        fb.draw_aa(xi + 12, y as i32 + 5, &self.cwd, 0xEAF4F0, crate::fb::AA_S);
        fb.fill_rect(x, y + 28, w, 1, 0x2C3A38);

        // Column header with sort markers.
        fb.fill_rect(x, y + 29, w, 20, 0x171C24);
        let name_active = self.sort_mode < 2;
        let size_active = self.sort_mode >= 2;
        let name_arrow = if self.sort_mode == 0 { " v" } else if self.sort_mode == 1 { " ^" } else { "" };
        let size_arrow = if self.sort_mode == 2 { "v " } else if self.sort_mode == 3 { "^ " } else { "" };
        let chy = y as i32 + 31;
        fb.draw_aa(xi + 44, chy, "Name", if name_active { BLUE } else { 0x8A949E }, crate::fb::AA_T);
        fb.draw_aa(xi + 44 + Framebuffer::aa_w("Name", crate::fb::AA_T), chy, name_arrow, BLUE, crate::fb::AA_T);
        let sz_hdr = "Size";
        let sz_x = xi + wi - Framebuffer::aa_w(sz_hdr, crate::fb::AA_T) - 18;
        fb.draw_aa(sz_x, chy, sz_hdr, if size_active { BLUE } else { 0x8A949E }, crate::fb::AA_T);
        fb.draw_aa(sz_x - 16, chy, size_arrow, BLUE, crate::fb::AA_T);

        // File rows (reserve 22px at the bottom for the status bar).
        let row_h: i32 = 26;
        let list_top = y as i32 + 50;
        let list_bottom = y as i32 + h as i32 - 22;
        let mut sbuf = [0u8; 24];
        for (i, entry) in self.entries.iter().enumerate() {
            let ry = list_top + i as i32 * row_h;
            if ry + row_h > list_bottom { break; }
            let selected = i == self.selected;
            let bg = if selected { 0x243442 } else if i % 2 == 0 { 0x161B22 } else { 0x1A2028 };
            fb.fill_rect(x, ry as u32, w, row_h as u32, bg);
            if selected { fb.fill_rect_s(xi, ry, 3, row_h, TEAL); }

            // File glyph — a small document tile (listdir returns leaf files only).
            let gx = xi + 14; let gy = ry + (row_h - 14) / 2;
            fb.fill_rounded_rect(gx, gy, 11, 14, 2, if selected { 0x3A6E8A } else { 0x2A3B47 });
            fb.fill_rect_s(gx + 2, gy + 4, 7, 1, 0x9FC4D8);
            fb.fill_rect_s(gx + 2, gy + 7, 7, 1, 0x9FC4D8);
            fb.fill_rect_s(gx + 2, gy + 10, 5, 1, 0x9FC4D8);

            let ty = ry + (row_h - line) / 2;
            let tcol = if selected { 0xEAF4F0 } else { 0xC2CCD4 };
            fb.draw_aa(xi + 44, ty, &entry.name, tcol, crate::fb::AA_S);

            // Size, right-aligned, human-readable.
            let size_str = fmt_size(&mut sbuf, entry.size);
            let sw = Framebuffer::aa_w(size_str, crate::fb::AA_S);
            fb.draw_aa(xi + wi - sw - 16, ty, size_str, if selected { 0xBFE8DF } else { 0x90A0AA }, crate::fb::AA_S);
        }

        // Status bar: count · sort · key hints.
        let sby = y + h - 22;
        fb.fill_rect(x, sby, w, 22, 0x171F1B);
        fb.fill_rect_s(xi, sby as i32, wi, 1, 0x2C3A38);
        let mut cb = [0u8; 24];
        let sty = sby as i32 + 4;
        let mut sx = xi + 10;
        let cnt = u64_into(&mut cb, self.entries.len() as u64);
        fb.draw_aa(sx, sty, cnt, 0xECEDE5, crate::fb::AA_T); sx += Framebuffer::aa_w(cnt, crate::fb::AA_T) + 4;
        fb.draw_aa(sx, sty, "items", 0x8A938C, crate::fb::AA_T); sx += Framebuffer::aa_w("items", crate::fb::AA_T) + 14;
        fb.draw_aa(sx, sty, "sort:", 0x8A938C, crate::fb::AA_T); sx += Framebuffer::aa_w("sort:", crate::fb::AA_T) + 6;
        fb.draw_aa(sx, sty, self.sort_label(), 0x6FE18B, crate::fb::AA_T);
        let hint = "s sort   c copy   d del   Enter open   Bksp up";
        fb.draw_aa(xi + wi - Framebuffer::aa_w(hint, crate::fb::AA_T) - 10, sty, hint, 0x6B756D, crate::fb::AA_T);

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
            AK::Char(b's') | AK::Char(b'S') => {
                // Cycle sort mode, keeping the selected file selected.
                let sel_name = self.entries.get(self.selected).map(|e| e.name.clone());
                self.sort_mode = (self.sort_mode + 1) % 4;
                self.apply_sort();
                if let Some(n) = sel_name {
                    if let Some(i) = self.entries.iter().position(|e| e.name == n) { self.selected = i; }
                }
                self.dirty = true;
            }
            _ => {}
        }
    }

    fn on_mouse(&mut self, _x: i32, y: i32, _w: u32, _h: u32, buttons: u8) {
        if buttons & 0x01 == 0 { return; }
        // Rows start at y+50, 26px tall. Mirrors render layout above.
        let row_y = y - 50;
        if row_y < 0 { return; }
        let row = (row_y / 26) as usize;
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
        // Body + header
        fb.fill_rect(x, y, w, h, 0x14181F);
        fb.fill_rect(x, y, w, 30, 0x1B2230);
        fb.draw_aa(x as i32 + 10, y as i32 + 6, "May 2026", 0xEAF4F0, crate::fb::AA_S);

        // Day headers
        let days = ["Su", "Mo", "Tu", "We", "Th", "Fr", "Sa"];
        let day_width = (w - 16) / 7;
        for (i, day) in days.iter().enumerate() {
            let dx = x as i32 + 8 + (i as i32 * day_width as i32);
            fb.draw_aa(dx, y as i32 + 36, day, 0x8A949E, crate::fb::AA_T);
        }

        // Grid (7×5)
        let mut day: u32 = 1;
        let mut sbuf = [0u8; 24];
        for week in 0..5 {
            for dow in 0..7 {
                if day > 31 { break; }
                let cx = x as i32 + 8 + (dow as i32 * day_width as i32);
                let cy = y as i32 + 56 + (week * 22);
                if day == 28 { // Today
                    fb.fill_rounded_rect(cx - 3, cy - 1, 22, 19, 5, 0x2DD4BF);
                }
                let day_str = u64_into(&mut sbuf, day as u64);
                fb.draw_aa(cx, cy, day_str, if day == 28 { 0x0B1413 } else { 0xC2CCD4 }, crate::fb::AA_T);
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
        // Body + header
        fb.fill_rect(x, y, w, h, 0x14181F);
        fb.fill_rect(x, y, w, 28, 0x1B2230);
        fb.draw_aa(x as i32 + 12, y as i32 + 5, "Kernel Manager", 0xEAF4F0, crate::fb::AA_S);
        fb.draw_aa(x as i32 + 168, y as i32 + 7, "bring your own kernel", 0x8CC6E5, crate::fb::AA_T);

        // Running kernel + ABI
        fb.fill_rect(x, y + 28, w, 26, 0x18222B);
        fb.draw_aa(x as i32 + 12, y as i32 + 33, "Running: RustyPenguin 3.3.0 x86_64  (psh syscall ABI v1)",
                    0x6FE18B, crate::fb::AA_T);

        // Section label
        fb.draw_aa(x as i32 + 12, y as i32 + 60, "Available kernels (.elf in /):", 0xB8C2CC, crate::fb::AA_T);

        // Candidate list
        if self.candidates.is_empty() {
            fb.draw_aa(x as i32 + 12, y as i32 + KM_LIST_TOP,
                        "None found. Copy a multiboot2 kernel.elf into / (or psh: kinstall).",
                        0x9CA3AF, crate::fb::AA_T);
        } else {
            for (i, name) in self.candidates.iter().enumerate() {
                let row_y = y + KM_LIST_TOP as u32 + (i as u32 * KM_ROW_H as u32);
                if row_y + KM_ROW_H as u32 > y + h - (KM_BTN_H as u32 + 24) { break; }
                let is_sel = i == self.selected;
                let is_staged = self.staged.as_deref() == Some(name.as_str());
                let bg = if is_sel { 0x243442 } else if i % 2 == 0 { 0x161B22 } else { 0x1A2028 };
                fb.fill_rect(x, row_y, w, KM_ROW_H as u32, bg);
                if is_staged { fb.fill_rect(x, row_y, 4, KM_ROW_H as u32, 0x6FE18B); }
                let fg = if is_sel { 0xF5F5F7 } else { 0xCFCFD6 };
                fb.draw_aa(x as i32 + 14, row_y as i32 + 3, name, fg, crate::fb::AA_S);
                if is_staged {
                    fb.draw_aa(x as i32 + w as i32 - 70, row_y as i32 + 4, "staged", 0x6FE18B, crate::fb::AA_T);
                }
            }
        }

        // Status line (above the buttons)
        let status_y = y + h - (KM_BTN_H as u32 + 22);
        fb.fill_rect(x, status_y, w, 18, 0x18222B);
        fb.draw_aa(x as i32 + 12, status_y as i32 + 1, &self.status, 0xF5C451, crate::fb::AA_T);

        // Buttons: Install (green) + Rescan (blue)
        let btn_y = y + h - KM_BTN_H as u32 - 2;
        fb.fill_rounded_rect(x as i32 + 12, btn_y as i32, 220, KM_BTN_H, 8, 0x2E7D4F);
        fb.draw_aa(x as i32 + 40, btn_y as i32 + 7, "Install Selected Kernel", 0xF5F5F7, crate::fb::AA_S);
        fb.fill_rounded_rect(x as i32 + 244, btn_y as i32, 110, KM_BTN_H, 8, 0x355B8C);
        fb.draw_aa(x as i32 + 280, btn_y as i32 + 7, "Rescan", 0xF5F5F7, crate::fb::AA_S);

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
        let xi = x as i32; let yi = y as i32; let wi = w as i32;
        const TEAL: u32 = 0x2DD4BF;
        let line = Framebuffer::aa_line(crate::fb::AA_S);

        // Body + header band.
        fb.fill_rect(x, y, w, h, 0x14181F);
        fb.fill_rect(x, y, w, 30, 0x1B2230);
        fb.draw_aa(xi + 14, yi + 6, "System Settings", 0xEAF4F0, crate::fb::AA_S);
        fb.fill_rect(x, y + 30, w, 1, 0x2C3A38);

        let labels = ["Theme", "Window Snap", "Taskbar", "Auto-Save"];
        let theme_v   = if self.theme { "Dark" } else { "Light" };
        let snap_v    = if self.window_snap { "On" } else { "Off" };
        let taskbar_v = if self.taskbar_bottom { "Bottom" } else { "Top" };

        let mut sbuf = [0u8; 24];
        let interval_str = u64_into(&mut sbuf, self.auto_save_interval as u64);

        let row_h: i32 = 34;
        let top = yi + 38;
        for i in 0..4 {
            let ry = top + i as i32 * row_h;
            if (ry + row_h) as u32 > y + h { break; }
            let selected = i == self.selected;
            if selected {
                fb.fill_rounded_rect(xi + 6, ry, wi - 12, row_h - 4, 8, 0x243442);
                fb.fill_rect_s(xi + 8, ry + 6, 3, row_h - 16, TEAL);
            }
            let ty = ry + (row_h - 4 - line) / 2;
            let lcol = if selected { 0xEAF4F0 } else { 0xB8C2CC };
            fb.draw_aa(xi + 18, ty, labels[i], lcol, crate::fb::AA_S);

            // Value, right-aligned. Auto-Save composes "<n>s" into a buffer.
            let mut vbuf = [0u8; 16];
            let val: &str = match i {
                0 => theme_v,
                1 => snap_v,
                2 => taskbar_v,
                _ => if self.auto_save_enabled {
                    let mut n = 0usize;
                    for &b in interval_str.as_bytes() { if n < 15 { vbuf[n] = b; n += 1; } }
                    if n < 15 { vbuf[n] = b's'; n += 1; }
                    core::str::from_utf8(&vbuf[..n]).unwrap_or("")
                } else { "Off" },
            };
            let vw = Framebuffer::aa_w(val, crate::fb::AA_S);
            let on = matches!((i, val), (0, "Dark") | (1, "On") | (2, "Bottom")) || (i == 3 && self.auto_save_enabled);
            let vcol = if on { TEAL } else { 0x90A0AA };
            fb.draw_aa(xi + wi - vw - 16, ty, val, vcol, crate::fb::AA_S);
        }

        // Hint at the bottom.
        if h > 110 {
            fb.draw_aa(xi + 14, yi + h as i32 - 24, "UP/DOWN to select   .   ENTER to toggle", 0x6B7B85, crate::fb::AA_T);
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

    fn on_mouse(&mut self, _mx: i32, my: i32, _w: u32, _h: u32, buttons: u8) {
        if buttons & 0x01 == 0 { return; }
        // Rows: top = y+38, 34px each (mirrors render). Click a row → select + toggle.
        if my < 38 { return; }
        let row = (my - 38) / 34;
        if row >= 0 && (row as usize) < 4 {
            self.selected = row as usize;
            self.toggle_selected();
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
        // Body + header
        fb.fill_rect(x, y, w, h, 0x0A0E1A);
        fb.fill_rect(x, y, w, 26, 0x141A2E);
        fb.draw_aa(x as i32 + 10, y as i32 + 4, "TIS Console", 0x4A9EFF, crate::fb::AA_S);
        fb.fill_rect(x, y + 26, w, 1, 0x2C3E50);

        // Output. scroll_offset shifts the visible window up from the latest.
        let line_h = 17u32;
        let max_lines = ((h.saturating_sub(50)) / line_h) as usize;
        let total = self.output_lines.len();
        let max_off = total.saturating_sub(max_lines);
        if self.scroll_offset > max_off { self.scroll_offset = max_off; }
        let end = total.saturating_sub(self.scroll_offset);
        let start = end.saturating_sub(max_lines);

        for (i, line) in self.output_lines[start..end].iter().enumerate() {
            let y_pos = y as i32 + 30 + (i as i32 * line_h as i32);
            if (y_pos + line_h as i32) > (y + h - 24) as i32 { break; }
            fb.draw_aa(x as i32 + 10, y_pos, line, 0x6FE18B, crate::fb::AA_T);
        }

        // Scroll indicator when not at the bottom.
        if self.scroll_offset > 0 && w > 12 {
            fb.draw_aa(x as i32 + w as i32 - 14, y as i32 + 30, "^", 0x6B7280, crate::fb::AA_T);
        }

        // Input box
        fb.fill_rect(x, y + h - 24, w, 24, 0x1E2C3A);
        let disp = if self.input_buffer.len() > 48 {
            &self.input_buffer[self.input_buffer.len()-48..]
        } else { &self.input_buffer };
        fb.draw_aa(x as i32 + 10, y as i32 + h as i32 - 21, disp, 0xA0D0FF, crate::fb::AA_S);

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
const MON_HIST: usize = 200;   // memory-% history samples (~50s at 4 Hz)

/// System Monitor — a real, GNOME-System-Monitor-style tool. Resources tab shows a
/// LIVE scrolling memory-usage graph (sys_meminfo, real data) + readout + uptime;
/// Processes tab lists the kernel's actual processes (sys_ps) with ternary-coloured
/// state. (This kernel doesn't track per-core CPU%, so we don't fake a CPU graph —
/// memory + processes + uptime are all real.) Tabs: 1/2 or Tab.
pub struct ProcessMonitor {
    tab: u8,                       // 0 Resources, 1 Processes
    mem_hist: [u8; MON_HIST],      // memory-used % history (chronological, [len-1]=newest)
    cpu_hist: [u8; MON_HIST],      // CPU busy % history
    hist_len: usize,
    mem_free: u32, mem_total: u32, // MiB
    cpu_pct: u32,                  // CPU busy %
    nprocs: usize,
    sel: usize,                    // selected process row (for kill)
    last_sample: u64,
    pub dirty: bool,
    pub wants_close: bool,
    ansi: crate::ansi::AnsiParser,
}

impl ProcessMonitor {
    pub fn new() -> Self {
        let mut m = ProcessMonitor {
            tab: 0, mem_hist: [0; MON_HIST], cpu_hist: [0; MON_HIST], hist_len: 0,
            mem_free: 0, mem_total: 0, cpu_pct: 0, nprocs: 0, sel: 0, last_sample: 0,
            dirty: true, wants_close: false, ansi: crate::ansi::AnsiParser::new(),
        };
        m.sample();
        m
    }

    fn sample(&mut self) {
        let (free, total) = sys_meminfo();
        self.mem_free = free; self.mem_total = total;
        let mem_pct = if total > 0 { ((total - free) as u64 * 100 / total as u64) as u8 } else { 0 };
        self.cpu_pct = (sys_cpu() / 10).min(100);          // permille → %
        let cpu = self.cpu_pct as u8;
        if self.hist_len < MON_HIST {
            self.mem_hist[self.hist_len] = mem_pct;
            self.cpu_hist[self.hist_len] = cpu;
            self.hist_len += 1;
        } else {
            self.mem_hist.copy_within(1.., 0); self.mem_hist[MON_HIST - 1] = mem_pct;
            self.cpu_hist.copy_within(1.., 0); self.cpu_hist[MON_HIST - 1] = cpu;
        }
        let mut buf = [0u8; 16 * 32];
        self.nprocs = unsafe { sys_ps(&mut buf, 16) };
    }

    /// Draw a scrolling area-chart of `hist[..len]` (newest right) into a box.
    fn draw_graph(fb: &mut Framebuffer, gx: u32, gy: u32, gw: u32, gh: u32,
                  hist: &[u8], len: usize, area: u32, line: u32) {
        fb.fill_rect(gx, gy, gw, gh, 0x0E1311);
        for q in 1..4 { fb.fill_rect(gx, gy + gh * q / 4, gw, 1, 0x1E2A24); }
        for col in 0..gw {
            let back = (gw - 1 - col) as usize;
            if back >= len { continue; }
            let v = (hist[len - 1 - back] as u32).min(100);
            let barh = v * gh / 100;
            let top = gy + gh - barh;
            fb.fill_rect(gx + col, top, 1, barh, area);
            fb.set_pixel(gx + col, top, line);
        }
    }
}

impl App for ProcessMonitor {
    fn tick(&mut self, ticks: u64) -> bool {
        if ticks.wrapping_sub(self.last_sample) >= 25 { // ~4 Hz
            self.last_sample = ticks; self.sample(); return true;
        }
        false
    }

    fn render(&mut self, fb: &mut Framebuffer, x: u32, y: u32, w: u32, h: u32) {
        let xi = x as i32; let wi = w as i32;
        const T: u8 = crate::fb::AA_T; const S: u8 = crate::fb::AA_S;
        fb.fill_rect(x, y, w, h, 0x14171A);
        // Tab bar.
        fb.fill_rect(x, y, w, 28, 0x1B2230);
        for (i, t) in ["Resources", "Processes"].iter().enumerate() {
            let tx = xi + 14 + i as i32 * 110;
            let active = self.tab as usize == i;
            let col = if active { 0x6FE18B } else { 0x8A938C };
            if active { fb.fill_rect_s(tx - 4, y as i32 + 25, Framebuffer::aa_w(t, S) + 8, 2, 0x6FE18B); }
            fb.draw_aa(tx, y as i32 + 5, t, col, S);
        }

        if self.tab == 0 {
            // ── Resources: live CPU + memory graphs ────────────────────────────
            let gx = x + 14; let gw = w.saturating_sub(28); let gh = 86u32;
            let mut pb = [0u8; 24];
            // CPU graph (busy %).
            fb.draw_aa(xi + 14, y as i32 + 32, "CPU", 0xECEDE5, S);
            let ps = u64_into(&mut pb, self.cpu_pct as u64);
            let pw = Framebuffer::aa_w(ps, S) + Framebuffer::aa_w("%", S);
            let mut px = xi + wi - 14 - pw;
            px += fb.draw_aa(px, y as i32 + 32, ps, 0x8CC6E5, S);
            fb.draw_aa(px, y as i32 + 32, "%", 0x8A938C, S);
            Self::draw_graph(fb, gx, y + 52, gw, gh, &self.cpu_hist, self.hist_len, 0x1E4A6B, 0x8CC6E5);
            // Memory graph (used %).
            let used = self.mem_total.saturating_sub(self.mem_free);
            let pct = if self.mem_total > 0 { used * 100 / self.mem_total } else { 0 };
            let my = y + 52 + gh + 20;
            let myl = my as i32 - 19;
            fb.draw_aa(xi + 14, myl, "Memory", 0xECEDE5, S);
            let mut ub = [0u8; 24]; let mut tb = [0u8; 24]; let mut mp = [0u8; 24];
            // "used / total MiB" chained from a left anchor.
            let mut rx = xi + wi - 210;
            rx += fb.draw_aa(rx, myl, u64_into(&mut ub, used as u64), 0x6FE18B, S) + 5;
            rx += fb.draw_aa(rx, myl, "/", 0x8A938C, S) + 5;
            rx += fb.draw_aa(rx, myl, u64_into(&mut tb, self.mem_total as u64), 0xECEDE5, S) + 4;
            rx += fb.draw_aa(rx, myl, "MiB", 0x8A938C, S) + 12;
            rx += fb.draw_aa(rx, myl, u64_into(&mut mp, pct as u64), 0xF5C451, S);
            fb.draw_aa(rx, myl, "%", 0x8A938C, S);
            Self::draw_graph(fb, gx, my, gw, gh, &self.mem_hist, self.hist_len, 0x1E5A3E, 0x6FE18B);
            // uptime + process count
            let secs = sys_ticks() / 100;
            let mut hb = [0u8; 12]; let mut nb = [0u8; 24];
            let fy = my as i32 + gh as i32 + 14;
            let mut ux = xi + 14;
            ux += fb.draw_aa(ux, fy, "Uptime", 0x8A938C, T) + 8;
            let upt = SystemClock::hms(&mut hb, secs / 3600, secs / 60 % 60, secs % 60);
            fb.draw_aa(ux, fy, upt, 0xA8B0A6, T);
            let mut nx = xi + wi - 150;
            nx += fb.draw_aa(nx, fy, "Processes", 0x8A938C, T) + 8;
            fb.draw_aa(nx, fy, u64_into(&mut nb, self.nprocs as u64), 0xA8B0A6, T);
        } else {
            // ── Processes: SYSTEM (kernel, with CPU split) + APPLICATIONS (killable)
            let mut buf = [0u8; 16 * 32];
            let n = unsafe { sys_ps(&mut buf, 16) };
            self.nprocs = n;
            fb.draw_aa(xi + 14, y as i32 + 30, "SYSTEM", 0x8A938C, T);
            fb.fill_rect(x, y + 46, w, 18, 0x1B2230);
            fb.draw_aa(xi + 14, y as i32 + 47, "PID", 0x8A938C, T);
            fb.draw_aa(xi + 70, y as i32 + 47, "NAME", 0x8A938C, T);
            fb.draw_aa(xi + 230, y as i32 + 47, "CPU", 0x8A938C, T);
            fb.draw_aa(xi + 310, y as i32 + 47, "STATE", 0x8A938C, T);
            let mut yy = y as i32 + 66;
            for i in 0..n {
                let rec = &buf[i * 32..i * 32 + 32];
                let pid = u64::from_le_bytes([rec[0],rec[1],rec[2],rec[3],rec[4],rec[5],rec[6],rec[7]]);
                let state = rec[8];
                let name_end = rec[16..32].iter().position(|&c| c == 0).unwrap_or(16);
                let name = core::str::from_utf8(&rec[16..16 + name_end]).unwrap_or("?");
                let cpu = if name == "desktop" { self.cpu_pct } else if name == "idle" { 100u32.saturating_sub(self.cpu_pct) } else { 0 };
                let mut pb = [0u8; 24]; let mut cb = [0u8; 24];
                fb.draw_aa(xi + 14, yy, u64_into(&mut pb, pid), 0xA8B0A6, T);
                fb.draw_aa(xi + 70, yy, name, 0xECEDE5, T);
                let cs = u64_into(&mut cb, cpu as u64);
                let adv = fb.draw_aa(xi + 230, yy, cs, 0x8CC6E5, T);
                fb.draw_aa(xi + 230 + adv, yy, "%", 0x8A938C, T);
                let (lbl, col) = match state { 1 => ("Running", 0x6FE18B), 2 => ("Blocked", 0xEF7575), _ => ("Ready", 0xF5C451) };
                fb.draw_aa(xi + 310, yy, lbl, col, T);
                yy += 20;
            }
            // Applications — open app windows, selectable + killable.
            yy += 8;
            fb.draw_aa(xi + 14, yy, "APPLICATIONS", 0x8A938C, T);
            yy += 20;
            let count = unsafe { crate::APP_COUNT };
            if count > 0 && self.sel >= count { self.sel = count - 1; }
            if count == 0 {
                fb.draw_aa(xi + 18, yy + 2, "(no app windows open)", 0x6B756D, T);
            }
            for i in 0..count {
                let title = unsafe {
                    let t = &crate::APP_TITLES[i];
                    let end = t.iter().position(|&c| c == 0).unwrap_or(20);
                    core::str::from_utf8(&t[..end]).unwrap_or("?")
                };
                let ry = yy + i as i32 * 20;
                if ry + 20 > (y + h) as i32 - 18 { break; }
                let sel = i == self.sel;
                let bg = if sel { 0x2E3C32u32 } else { 0x14171A };
                if sel { fb.fill_rect(x, ry as u32, w, 20, bg); fb.fill_rect_s(xi + 2, ry + 4, 3, 12, 0x6FE18B); }
                fb.draw_aa(xi + 18, ry + 2, title, 0xECEDE5, S);
            }
            // footer hint
            fb.fill_rect(x, y + h - 18, w, 18, 0x1B2230);
            fb.draw_aa(xi + 14, y as i32 + h as i32 - 16, "up/down select   k = kill (force-quit)", 0x6B756D, T);
        }
        self.dirty = false;
    }

    fn on_key(&mut self, key: u8) {
        use crate::ansi::Key as AK;
        match self.ansi.feed(key) {
            AK::Char(b'1') => { self.tab = 0; self.dirty = true; }
            AK::Char(b'2') => { self.tab = 1; self.dirty = true; }
            AK::Char(b'\t') => { self.tab ^= 1; self.dirty = true; }
            AK::Up   => { if self.tab == 1 && self.sel > 0 { self.sel -= 1; self.dirty = true; } }
            AK::Down => { if self.tab == 1 { let c = unsafe { crate::APP_COUNT }; if self.sel + 1 < c { self.sel += 1; self.dirty = true; } } }
            AK::Char(b'k') | AK::Char(b'K') => {
                // Force-quit the selected app window (the loop services KILL_IDX).
                if self.tab == 1 && unsafe { crate::APP_COUNT } > 0 {
                    unsafe { crate::KILL_IDX = self.sel as i32; }
                    self.dirty = true;
                }
            }
            _ => {}
        }
    }

    fn on_mouse(&mut self, mx: i32, my: i32, _w: u32, _h: u32, buttons: u8) {
        if buttons & 0x01 == 0 { return; }
        // Tab bar (content y 0..28): Resources at x=14, Processes at x=124.
        if my < 28 {
            for (i, t) in ["Resources", "Processes"].iter().enumerate() {
                let tx = 14 + i as i32 * 110;
                let tw = Framebuffer::aa_w(t, crate::fb::AA_S);
                if mx >= tx - 4 && mx < tx + tw + 4 {
                    self.tab = i as u8; self.dirty = true; return;
                }
            }
        }
    }

    fn title(&self) -> &str {
        "System Monitor"
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
        // Body + header
        fb.fill_rect(x, y, w, h, 0x14181F);
        fb.fill_rect(x, y, w, 28, 0x1B2230);
        fb.draw_aa(x as i32 + 12, y as i32 + 5, "System Information", 0xEAF4F0, crate::fb::AA_S);
        fb.fill_rect(x, y + 28, w, 1, 0x2C3A38);

        let items = [
            "OS: Rusty Penguin v3.3.0",
            "Kernel: Bare-Metal (Pure Rust)",
            "Arch: x86_64",
            "Memory: 512 MB available",
            "Boot Time: ~2 seconds",
            "Uptime: Active",
            "Desktop: Modern window manager",
            "Shell: psh (penguin shell)",
        ];

        for (i, item) in items.iter().enumerate() {
            let y_pos = y + 34 + (i as u32 * 24);
            if y_pos + 24 > y + h { break; }
            let bg_color = if i % 2 == 0 { 0x161B22 } else { 0x1A2028 };
            fb.fill_rect(x, y_pos, w, 24, bg_color);
            fb.draw_aa(x as i32 + 12, y_pos as i32 + 3, item, 0xC2CCD4, crate::fb::AA_S);
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

/// Scientific calculator with balanced-ternary result panel.
/// Uses f64 arithmetic + libm for transcendentals. No heap beyond String display.
pub struct Calculator {
    display: String,
    accumulator: f64,
    current_op: u8,   // 0=none, b'+'/b'-'/b'*'/b'/' etc.
    new_number: bool,
    mem: f64,
    deg_mode: bool,   // true = degrees, false = radians
    error: bool,
    pub dirty: bool,
    pub wants_close: bool,
}

// Button layout — each entry: (label, key_byte, bg_accent)
// key_byte 0 = handled by label name (unary ops)
const CALC_BTNS: &[(&str, u8, u32)] = &[
    ("sin", 0, 0x2A3A4A), ("cos", 0, 0x2A3A4A), ("tan", 0, 0x2A3A4A), ("sqrt", 0, 0x2A3A4A), ("x^y", 0, 0x2A3A4A),
    ("7",   b'7', 0x22282E), ("8", b'8', 0x22282E), ("9", b'9', 0x22282E), ("/", b'/', 0x1E3040), ("pi",  0, 0x2A3A4A),
    ("4",   b'4', 0x22282E), ("5", b'5', 0x22282E), ("6", b'6', 0x22282E), ("*", b'*', 0x1E3040), ("ln",  0, 0x2A3A4A),
    ("1",   b'1', 0x22282E), ("2", b'2', 0x22282E), ("3", b'3', 0x22282E), ("-", b'-', 0x1E3040), ("log", 0, 0x2A3A4A),
    ("0",   b'0', 0x22282E), (".", b'.', 0x22282E), ("=", b'=', 0x14503A), ("+", b'+', 0x1E3040), ("exp", 0, 0x2A3A4A),
    ("+/-", 0, 0x2A2A3A), ("1/x", 0, 0x2A2A3A), ("C", b'c', 0x3A2A2A), ("M+", 0, 0x2A3A2A), ("MR",  0, 0x2A3A2A),
];
const CALC_COLS: u32 = 5;
const CALC_ROWS: u32 = 6;

fn fmt_f64(v: f64) -> String {
    if v.is_nan()      { return String::from("Error"); }
    if v.is_infinite() { return if v > 0.0 { String::from("Inf") } else { String::from("-Inf") }; }
    // Prefer integer display when the value is whole.
    let i = v as i64;
    if (v - i as f64).abs() < 1e-10 && v.abs() < 1e15 {
        return alloc::format!("{}", i);
    }
    // Up to 10 significant digits, strip trailing zeros.
    let s = alloc::format!("{:.10}", v);
    let s = s.trim_end_matches('0');
    let s = s.trim_end_matches('.');
    String::from(s)
}

fn to_balanced_ternary(mut n: i64) -> String {
    if n == 0 { return String::from("0"); }
    let neg = n < 0;
    if neg { n = -n; }
    let mut digits: [i8; 42] = [0; 42];
    let mut len = 0;
    while n != 0 {
        let rem = (n % 3) as i8;
        n /= 3;
        if rem == 2 { digits[len] = -1; n += 1; }
        else { digits[len] = rem; }
        len += 1;
    }
    let mut out = String::new();
    if neg { out.push('-'); }
    for k in 0..len { let d = digits[len - 1 - k]; out.push(match d { 1 => '+', -1 => '-', _ => '0' }); }
    out
}

impl Calculator {
    pub fn new() -> Self {
        Calculator {
            display: String::from("0"),
            accumulator: 0.0,
            current_op: 0,
            new_number: true,
            mem: 0.0,
            deg_mode: true,
            error: false,
            dirty: true,
            wants_close: false,
        }
    }

    fn display_val(&self) -> f64 {
        self.display.parse::<f64>().unwrap_or(0.0)
    }

    fn set_result(&mut self, v: f64) {
        self.error = v.is_nan();
        self.display = fmt_f64(v);
        self.accumulator = v;
        self.new_number = true;
        self.dirty = true;
    }

    fn commit_op(&mut self) {
        if self.current_op == 0 { self.accumulator = self.display_val(); return; }
        let cur = self.display_val();
        let result = match self.current_op {
            b'+' => self.accumulator + cur,
            b'-' => self.accumulator - cur,
            b'*' => self.accumulator * cur,
            b'/' => if cur.abs() < 1e-300 { f64::NAN } else { self.accumulator / cur },
            b'^' => libm::pow(self.accumulator, cur),
            _    => cur,
        };
        self.display = fmt_f64(result);
        self.error = result.is_nan();
        self.accumulator = result;
        self.new_number = true;
    }

    fn to_rad(&self, v: f64) -> f64 {
        if self.deg_mode { v * core::f64::consts::PI / 180.0 } else { v }
    }

    fn press(&mut self, label: &str, key: u8) {
        if key != 0 {
            match key {
                b'0'..=b'9' | b'.' => {
                    if self.new_number || self.error { self.display.clear(); self.new_number = false; self.error = false; }
                    if key == b'.' && self.display.contains('.') { return; }
                    if self.display.len() < 18 { self.display.push(key as char); }
                    self.dirty = true;
                }
                b'+' | b'-' | b'*' | b'/' => {
                    self.commit_op();
                    self.accumulator = self.display_val();
                    self.current_op = key;
                    self.new_number = true;
                    self.dirty = true;
                }
                b'=' | b'\r' => {
                    self.commit_op();
                    self.current_op = 0;
                    self.dirty = true;
                }
                b'c' | b'C' => {
                    self.display = String::from("0");
                    self.accumulator = 0.0;
                    self.current_op = 0;
                    self.new_number = true;
                    self.error = false;
                    self.dirty = true;
                }
                8 => { // Backspace
                    if !self.new_number && !self.error {
                        self.display.pop();
                        if self.display.is_empty() || self.display == "-" { self.display = String::from("0"); }
                        self.dirty = true;
                    }
                }
                _ => {}
            }
            return;
        }
        // Named ops.
        let v = self.display_val();
        match label {
            "sin"  => { let r = libm::sin(self.to_rad(v)); self.set_result(r); }
            "cos"  => { let r = libm::cos(self.to_rad(v)); self.set_result(r); }
            "tan"  => {
                let rad = self.to_rad(v);
                let r = if (libm::cos(rad)).abs() < 1e-12 { f64::NAN } else { libm::tan(rad) };
                self.set_result(r);
            }
            "sqrt" => { let r = if v < 0.0 { f64::NAN } else { libm::sqrt(v) }; self.set_result(r); }
            "ln"   => { let r = if v <= 0.0 { f64::NAN } else { libm::log(v) }; self.set_result(r); }
            "log"  => { let r = if v <= 0.0 { f64::NAN } else { libm::log10(v) }; self.set_result(r); }
            "exp"  => { self.set_result(libm::exp(v)); }
            "pi"   => { self.display = String::from("3.14159265358979"); self.new_number = false; self.dirty = true; }
            "x^y"  => { self.commit_op(); self.accumulator = self.display_val(); self.current_op = b'^'; self.new_number = true; self.dirty = true; }
            "1/x"  => { let r = if v.abs() < 1e-300 { f64::NAN } else { 1.0 / v }; self.set_result(r); }
            "+/-"  => {
                if self.display.starts_with('-') { self.display.remove(0); }
                else if self.display != "0" { self.display.insert(0, '-'); }
                self.dirty = true;
            }
            "M+"   => { self.mem += v; self.dirty = true; }
            "MC"   => { self.mem = 0.0; self.dirty = true; }
            "MR"   => { self.display = fmt_f64(self.mem); self.new_number = true; self.dirty = true; }
            "DEG"  => { self.deg_mode = true; self.dirty = true; }
            "RAD"  => { self.deg_mode = false; self.dirty = true; }
            _ => {}
        }
    }
}

impl App for Calculator {
    fn render(&mut self, fb: &mut Framebuffer, x: u32, y: u32, w: u32, h: u32) {
        let bg = 0x141720u32;
        fb.fill_rect(x, y, w, h, bg);

        // ── Display panel ────────────────────────────────────────────────────
        let dp_h = 52u32;
        let dp_y = y + 4;
        fb.fill_rect(x + 6, dp_y, w - 12, dp_h, 0x0D1117);
        fb.fill_rect(x + 6, dp_y, w - 12, 1, 0x30364A);
        fb.fill_rect(x + 6, dp_y + dp_h - 1, w - 12, 1, 0x30364A);

        // Decimal result (right-aligned).
        let disp = if self.error { "Error" } else { &self.display };
        let dw = Framebuffer::aa_w(disp, crate::fb::AA_S);
        let dx = x as i32 + w as i32 - dw - 10;
        let disp_col = if self.error { 0xEF4444u32 } else { 0xF0F0F0 };
        fb.draw_aa(dx, dp_y as i32 + 5, disp, disp_col, crate::fb::AA_S);

        // Balanced ternary of the integer part (right-aligned, smaller).
        if !self.error {
            if let Ok(v) = self.display.parse::<f64>() {
                let trit = to_balanced_ternary(v as i64);
                let tw = Framebuffer::aa_w(&trit, crate::fb::AA_T);
                let tx = x as i32 + w as i32 - tw - 10;
                fb.draw_aa(tx, dp_y as i32 + 30, &trit, 0x4A9EFF, crate::fb::AA_T);
                // mode indicator
                let mode = if self.deg_mode { "DEG" } else { "RAD" };
                fb.draw_aa(x as i32 + 10, dp_y as i32 + 30, mode, 0x6B7280, crate::fb::AA_T);
                // mem indicator
                if self.mem != 0.0 { fb.draw_aa(x as i32 + 46, dp_y as i32 + 30, "M", 0x6FE18B, crate::fb::AA_T); }
            }
        }

        // ── Button grid (6 rows × 5 cols) ───────────────────────────────────
        let pad = 4u32;
        let btn_w = (w.saturating_sub(pad * (CALC_COLS + 1))) / CALC_COLS;
        let btn_h = (h.saturating_sub(dp_h + 12 + pad * (CALC_ROWS + 1))) / CALC_ROWS;
        let grid_y = y + dp_h + 12;

        for (i, (label, _key, accent)) in CALC_BTNS.iter().enumerate() {
            let row = (i as u32) / CALC_COLS;
            let col = (i as u32) % CALC_COLS;
            let bx = x + pad + col * (btn_w + pad);
            let by = grid_y + pad + row * (btn_h + pad);
            let is_eq = *label == "=";
            let btn_bg = if is_eq { 0x1A5C42u32 } else { *accent };
            fb.fill_rect(bx, by, btn_w, btn_h, btn_bg);
            fb.fill_rect(bx, by, btn_w, 1, btn_bg.saturating_add(0x181818));
            let lw = Framebuffer::aa_w(label, crate::fb::AA_S);
            let lx = bx as i32 + (btn_w as i32 - lw) / 2;
            let ly = by as i32 + (btn_h as i32 - Framebuffer::aa_line(crate::fb::AA_S)) / 2;
            let lc = if is_eq { 0x6FE18Bu32 } else { 0xCDD6E0 };
            fb.draw_aa(lx, ly, label, lc, crate::fb::AA_S);
        }

        self.dirty = false;
    }

    fn on_key(&mut self, key: u8) {
        // Map readable chars to their button.
        let (label, k): (&str, u8) = match key {
            b'0'..=b'9' | b'.' | b'+' | b'-' | b'*' | b'/' | b'=' | b'\r' | b'c' | b'C' | 8 => ("", key),
            b's' => ("sin",  0), b'o' => ("cos", 0), b't' => ("tan", 0),
            b'q' => ("sqrt", 0), b'l' => ("ln",  0), b'g' => ("log", 0),
            b'e' => ("exp",  0), b'p' => ("pi",  0), b'^' => ("x^y", 0),
            b'i' => ("1/x",  0), b'n' => ("+/-", 0),
            b'm' => ("M+",   0), b'r' => ("MR",  0),
            b'd' => { self.deg_mode = !self.deg_mode; self.dirty = true; return; }
            _ => return,
        };
        self.press(label, k);
    }

    fn on_mouse(&mut self, x: i32, y: i32, w: u32, h: u32, buttons: u8) {
        if buttons & 0x01 == 0 { return; }
        let dp_h = 52i32;
        let pad = 4i32;
        let btn_w = (w as i32 - pad * (CALC_COLS as i32 + 1)) / CALC_COLS as i32;
        let btn_h = (h as i32 - dp_h - 12 - pad * (CALC_ROWS as i32 + 1)) / CALC_ROWS as i32;
        let grid_y = dp_h + 12;
        let lx = x - pad;
        let ly = y - grid_y - pad;
        if lx < 0 || ly < 0 { return; }
        let col = lx / (btn_w + pad);
        let row = ly / (btn_h + pad);
        if col >= CALC_COLS as i32 || row >= CALC_ROWS as i32 { return; }
        if lx % (btn_w + pad) >= btn_w || ly % (btn_h + pad) >= btn_h { return; }
        let idx = (row * CALC_COLS as i32 + col) as usize;
        if idx < CALC_BTNS.len() {
            let (label, key, _) = CALC_BTNS[idx];
            self.press(label, key);
        }
    }

    fn title(&self) -> &str { "Calculator" }
    fn wants_close(&self) -> bool { self.wants_close }
}

/// System Clock and Status Display
/// Clock — live time + date, a stopwatch, a countdown timer, and world clocks.
/// Tabs switch with 1-4 (or Tab). Real RTC + 100 Hz tick counter, no fakes.
pub struct SystemClock {
    tab: u8,                 // 0 Clock, 1 Stopwatch, 2 Timer, 3 World
    sw_running: bool, sw_start: u64, sw_accum: u64,   // stopwatch, in ticks
    tm_set: u64,             // timer total seconds configured
    tm_running: bool, tm_start: u64, tm_left_at_start: u64, // timer
    last_update: u64,
    pub dirty: bool,
    pub wants_close: bool,
    ansi: crate::ansi::AnsiParser,
}

// (city, hours offset from the RTC's local Vienna time). Vienna RTC is the base.
const WORLD: [(&str, i64); 6] = [
    ("Vienna", 0), ("London", -1), ("New York", -6),
    ("Los Angeles", -9), ("Tokyo", 7), ("Sydney", 8),
];

impl SystemClock {
    pub fn new() -> Self {
        SystemClock { tab: 0, sw_running: false, sw_start: 0, sw_accum: 0,
                      tm_set: 60, tm_running: false, tm_start: 0, tm_left_at_start: 60,
                      last_update: 0, dirty: true, wants_close: false,
                      ansi: crate::ansi::AnsiParser::new() }
    }

    fn sw_elapsed(&self) -> u64 { // in ticks (100 Hz)
        self.sw_accum + if self.sw_running { sys_ticks().wrapping_sub(self.sw_start) } else { 0 }
    }
    fn tm_remaining(&self) -> u64 { // seconds, floored at 0
        if !self.tm_running { return self.tm_left_at_start; }
        let elapsed = sys_ticks().wrapping_sub(self.tm_start) / 100;
        self.tm_left_at_start.saturating_sub(elapsed)
    }

    // hh:mm:ss into a stack buffer.
    fn hms(buf: &mut [u8; 12], h: u64, m: u64, s: u64) -> &str {
        buf[0] = b'0' + (h / 10) as u8; buf[1] = b'0' + (h % 10) as u8; buf[2] = b':';
        buf[3] = b'0' + (m / 10) as u8; buf[4] = b'0' + (m % 10) as u8; buf[5] = b':';
        buf[6] = b'0' + (s / 10) as u8; buf[7] = b'0' + (s % 10) as u8;
        core::str::from_utf8(&buf[..8]).unwrap_or("")
    }
}

impl App for SystemClock {
    fn tick(&mut self, ticks: u64) -> bool {
        // Update at ~20 Hz while a stopwatch/timer runs (centiseconds matter),
        // ~2 Hz otherwise (catch the seconds flip promptly).
        let iv = if self.sw_running || self.tm_running { 5 } else { 50 };
        if ticks.wrapping_sub(self.last_update) >= iv { self.last_update = ticks; return true; }
        false
    }

    fn render(&mut self, fb: &mut Framebuffer, x: u32, y: u32, w: u32, h: u32) {
        fb.fill_rect(x, y, w, h, 0x14171A);
        // Tab bar.
        fb.fill_rect(x, y, w, 28, 0x1B2230);
        let tabs = ["Clock", "Stopwatch", "Timer", "World"];
        let mut tx = x as i32 + 12;
        for (i, t) in tabs.iter().enumerate() {
            let active = self.tab as usize == i;
            let col = if active { 0x6FE18B } else { 0x8A938C };
            let tw_px = Framebuffer::aa_w(t, crate::fb::AA_S);
            if active { fb.fill_rect_s(tx - 4, y as i32 + 25, tw_px + 8, 2, 0x6FE18B); }
            fb.draw_aa(tx, y as i32 + 5, t, col, crate::fb::AA_S);
            tx += tw_px + 22;
        }

        let cx = x as i32 + w as i32 / 2;
        let body_y = y as i32 + 34;
        match self.tab {
            0 => { // Clock
                let rtc = sys_rtc();
                let sec = ((rtc >> 8) & 0xFF) as u64;
                let min = ((rtc >> 16) & 0xFF) as u64;
                let hour = ((rtc >> 24) & 0xFF) as u64;
                let mday = ((rtc >> 32) & 0xFF) as u64;
                let month = ((rtc >> 40) & 0xFF) as u64;
                let mut tbuf = [0u8; 12];
                let (hh, mm, ss) = if hour < 24 && min < 60 && sec < 60 {
                    (hour, min, sec)
                } else { let t = sys_ticks() / 100; (t / 3600 % 24, t / 60 % 60, t % 60) };
                let s = Self::hms(&mut tbuf, hh, mm, ss);
                let sw = Framebuffer::aa_w(s, crate::fb::AA_L);
                fb.draw_aa(cx - sw / 2, body_y + 26, s, 0x6FE18B, crate::fb::AA_L);
                // Date line.
                const MON: [&str; 13] = ["", "Jan","Feb","Mar","Apr","May","Jun","Jul","Aug","Sep","Oct","Nov","Dec"];
                if month >= 1 && month <= 12 {
                    let mut db = [0u8; 24];
                    let dstr = u64_into(&mut db, mday);
                    let dy = body_y + 80;
                    let mw = Framebuffer::aa_w(MON[month as usize], crate::fb::AA_S);
                    let dw = Framebuffer::aa_w(dstr, crate::fb::AA_S);
                    let yw = Framebuffer::aa_w("2026", crate::fb::AA_S);
                    let mut px = cx - (mw + 8 + dw + 8 + yw) / 2;
                    px += fb.draw_aa(px, dy, MON[month as usize], 0xA8B0A6, crate::fb::AA_S) + 8;
                    px += fb.draw_aa(px, dy, dstr, 0xECEDE5, crate::fb::AA_S) + 8;
                    fb.draw_aa(px, dy, "2026", 0xA8B0A6, crate::fb::AA_S);
                }
            }
            1 => { // Stopwatch — MM:SS.cs
                let t = self.sw_elapsed();
                let cs = (t % 100) as u64; let total = t / 100;
                let mut tb = [0u8; 12];
                let s = Self::hms(&mut tb, total / 3600, total / 60 % 60, total % 60);
                let sw = Framebuffer::aa_w(s, crate::fb::AA_L);
                let adv = fb.draw_aa(cx - sw / 2, body_y + 24, s, 0xECEDE5, crate::fb::AA_L);
                let mut cb = [0u8; 8];
                cb[0] = b'.'; cb[1] = b'0' + (cs / 10) as u8; cb[2] = b'0' + (cs % 10) as u8;
                fb.draw_aa(cx - sw / 2 + adv + 4, body_y + 38, core::str::from_utf8(&cb[..3]).unwrap_or(""), 0x8CC6E5, crate::fb::AA_S);
                let hint = "SPACE start/stop   R reset";
                fb.draw_aa(cx - Framebuffer::aa_w(hint, crate::fb::AA_T) / 2, body_y + 86, hint, 0x6B756D, crate::fb::AA_T);
            }
            2 => { // Timer — countdown
                let rem = self.tm_remaining();
                let done = self.tm_running && rem == 0;
                let mut tb = [0u8; 12];
                let s = Self::hms(&mut tb, rem / 3600, rem / 60 % 60, rem % 60);
                let col = if done { 0xEF7575 } else { 0xF5C451 };
                let sw = Framebuffer::aa_w(s, crate::fb::AA_L);
                fb.draw_aa(cx - sw / 2, body_y + 24, s, col, crate::fb::AA_L);
                if done {
                    let tu = "TIME UP";
                    fb.draw_aa(cx - Framebuffer::aa_w(tu, crate::fb::AA_S) / 2, body_y + 72, tu, 0xEF7575, crate::fb::AA_S);
                }
                let hint = "+/- adjust   SPACE start/stop   R reset";
                fb.draw_aa(cx - Framebuffer::aa_w(hint, crate::fb::AA_T) / 2, body_y + 94, hint, 0x6B756D, crate::fb::AA_T);
            }
            _ => { // World clocks
                let rtc = sys_rtc();
                let hour = ((rtc >> 24) & 0xFF) as i64;
                let min = ((rtc >> 16) & 0xFF) as i64;
                let sec = ((rtc >> 8) & 0xFF) as i64;
                let (lh, lm, ls) = if hour < 24 && min < 60 { (hour, min, sec.max(0)) }
                    else { let t = (sys_ticks()/100) as i64; (t/3600%24, t/60%60, t%60) };
                let mut yy = body_y + 8;
                for (city, off) in WORLD.iter() {
                    let hh = (((lh + off) % 24) + 24) % 24;
                    let mut tb = [0u8; 12];
                    let s = Self::hms(&mut tb, hh as u64, lm as u64, ls as u64);
                    fb.draw_aa(x as i32 + 16, yy, city, 0xECEDE5, crate::fb::AA_S);
                    let sw = Framebuffer::aa_w(s, crate::fb::AA_S);
                    fb.draw_aa(x as i32 + w as i32 - sw - 16, yy, s, 0x6FE18B, crate::fb::AA_S);
                    yy += 26;
                }
            }
        }
        self.dirty = false;
    }

    fn on_key(&mut self, key: u8) {
        use crate::ansi::Key as AK;
        match self.ansi.feed(key) {
            AK::Char(b'1') => { self.tab = 0; self.dirty = true; }
            AK::Char(b'2') => { self.tab = 1; self.dirty = true; }
            AK::Char(b'3') => { self.tab = 2; self.dirty = true; }
            AK::Char(b'4') => { self.tab = 3; self.dirty = true; }
            AK::Char(b'\t') => { self.tab = (self.tab + 1) % 4; self.dirty = true; }
            AK::Char(b' ') => {
                match self.tab {
                    1 => { // stopwatch start/stop
                        if self.sw_running { self.sw_accum = self.sw_elapsed(); self.sw_running = false; }
                        else { self.sw_start = sys_ticks(); self.sw_running = true; }
                        self.dirty = true;
                    }
                    2 => { // timer start/stop
                        if self.tm_running { self.tm_left_at_start = self.tm_remaining(); self.tm_running = false; }
                        else if self.tm_left_at_start > 0 { self.tm_start = sys_ticks(); self.tm_running = true; }
                        self.dirty = true;
                    }
                    _ => {}
                }
            }
            AK::Char(b'r') | AK::Char(b'R') => {
                match self.tab {
                    1 => { self.sw_running = false; self.sw_accum = 0; self.dirty = true; }
                    2 => { self.tm_running = false; self.tm_left_at_start = self.tm_set; self.dirty = true; }
                    _ => {}
                }
            }
            AK::Char(b'+') | AK::Char(b'=') => {
                if self.tab == 2 && !self.tm_running {
                    self.tm_set = (self.tm_set + 10).min(86399);
                    self.tm_left_at_start = self.tm_set; self.dirty = true;
                }
            }
            AK::Char(b'-') | AK::Char(b'_') => {
                if self.tab == 2 && !self.tm_running {
                    self.tm_set = self.tm_set.saturating_sub(10);
                    self.tm_left_at_start = self.tm_set; self.dirty = true;
                }
            }
            _ => {}
        }
    }

    fn on_mouse(&mut self, mx: i32, my: i32, _w: u32, _h: u32, buttons: u8) {
        if buttons & 0x01 == 0 { return; }
        // Tab bar (content y 0..28): Clock/Stopwatch/Timer/World, x=12 + advancing.
        if my < 28 {
            let mut tx = 12;
            for (i, t) in ["Clock", "Stopwatch", "Timer", "World"].iter().enumerate() {
                let tw = Framebuffer::aa_w(t, crate::fb::AA_S);
                if mx >= tx - 4 && mx < tx + tw + 4 {
                    self.tab = i as u8; self.dirty = true; return;
                }
                tx += tw + 22;
            }
        }
    }

    fn title(&self) -> &str { "Clock" }
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
        // Body + header
        fb.fill_rect(x, y, w, h, 0x0F141B);
        fb.fill_rect(x, y, w, 28, 0x1B2230);
        fb.draw_aa(x as i32 + 12, y as i32 + 5, "Help & Reference", 0xEAF4F0, crate::fb::AA_S);
        fb.fill_rect(x, y + 28, w, 1, 0x2C3A38);

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

        let mut y_pos = y as i32 + 34;
        for (i, line) in help_text.iter().enumerate() {
            if i < self.scroll_offset { continue; }
            if y_pos + 18 > (y + h) as i32 { break; }

            let color = if line.is_empty() {
                0x0F141B
            } else if line.contains("=") {
                0x4A9EFF
            } else if line.starts_with("  ") {
                0x9CA3AF
            } else {
                0xC2CCD4
            };

            fb.draw_aa(x as i32 + 12, y_pos, line, color, crate::fb::AA_T);
            y_pos += 18;
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
    BrLine { kind: K_H1,   text: "PinguBrowser",                                       link: -1 },
    BrLine { kind: K_NOTE, text: "Pure-Rust reader browser . from-scratch TLS . RFI-IRFOS", link: -1 },
    BrLine { kind: K_SPACE,text: "",                                                   link: -1 },
    BrLine { kind: K_P,    text: "Type a hostname to visit it, or type words to search",link: -1 },
    BrLine { kind: K_P,    text: "Google - all over our own TCP/IP + TLS 1.3 stack.",  link: -1 },
    BrLine { kind: K_SPACE,text: "",                                                   link: -1 },
    BrLine { kind: K_H2,   text: "Bookmarks",                                          link: -1 },
    BrLine { kind: K_LINK, text: "About this OS",                                      link: 1 },
    BrLine { kind: K_LINK, text: "The ternary case",                                   link: 2 },
    BrLine { kind: K_LINK, text: "How the live web works",                             link: 3 },
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
    BrLine { kind: K_H1,   text: "The live web works now.",                            link: -1 },
    BrLine { kind: K_NOTE, text: "rustypenguin://roadmap",                             link: -1 },
    BrLine { kind: K_SPACE,text: "",                                                   link: -1 },
    BrLine { kind: K_P,    text: "Type any hostname in the address bar. The OS will:", link: -1 },
    BrLine { kind: K_P,    text: "1. Resolve it via our DNS stack (UDP/53)",           link: -1 },
    BrLine { kind: K_P,    text: "2. Open a TCP connection to port 80",                link: -1 },
    BrLine { kind: K_P,    text: "3. Send HTTP/1.0 GET and receive the page",          link: -1 },
    BrLine { kind: K_P,    text: "4. Strip headers and render the text here.",         link: -1 },
    BrLine { kind: K_SPACE,text: "",                                                   link: -1 },
    BrLine { kind: K_P,    text: "All from scratch. No curl, no libc, no Linux.",      link: -1 },
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

/// sys_fetch_trust (#36) → trust state of the most recent fetch:
/// 1 = verified HTTPS (cert chain validated to a trusted root), 0 = plain HTTP,
/// 2 = none / failed. Drives the address-bar lock indicator.
unsafe fn sys_fetch_trust() -> u64 {
    let n: u64;
    core::arch::asm!(
        "syscall",
        inout("rax") 36u64 => n,
        out("rcx") _, out("r11") _,
        options(nostack),
    );
    n
}

/// What the lock indicator shows for the current page.
#[derive(Copy, Clone, PartialEq)]
enum Security {
    Secure,    // HTTPS, certificate chain validated
    Insecure,  // plain HTTP — no transport security
    None,      // local page / no network fetch
}

// Max bytes of fetched HTML the browser buffers. Fits in the 24 MiB heap.
const FETCH_BUF: usize = 32_768;

#[derive(Copy, Clone, PartialEq)]
enum BrMode {
    Static,     // showing a built-in local page (page index in Browser.page)
    Loading,    // sys_http_get in progress
    Live,       // showing a fetched remote page
    Err,        // last fetch failed
}

// ── Visual HTML node types ────────────────────────────────────────────────────
#[derive(Clone)]
enum HNode {
    H1(String),
    H2(String),
    H3(String),
    Para(String),
    Link { text: String, href: String },
    Input { placeholder: String },
    Li(String),
    Hr,
    Blank,
}

impl HNode {
    fn base_h(&self) -> i32 {
        match self {
            HNode::H1(_)    => 44,
            HNode::H2(_)    => 32,
            HNode::H3(_)    => 26,
            HNode::Para(_)  => 20,
            HNode::Link{..} => 22,
            HNode::Input{..}=> 32,
            HNode::Li(_)    => 20,
            HNode::Hr       => 14,
            HNode::Blank    => 10,
        }
    }
}

/// Extract an attribute value from a raw HTML tag byte slice, e.g. href="..." or href='...'
fn html_attr<'a>(tag: &'a [u8], attr: &[u8]) -> &'a [u8] {
    let mut i = 0;
    while i + attr.len() < tag.len() {
        if tag[i..].starts_with(attr) {
            let after = i + attr.len();
            if after < tag.len() && (tag[after] == b'=' || tag[after..].starts_with(b" =")) {
                let eq = tag[after..].iter().position(|&b| b == b'=').unwrap_or(0) + after;
                if eq + 1 < tag.len() {
                    let rest = &tag[eq + 1..];
                    let (q, close) = if rest[0] == b'"' { (b'"', 1) }
                                     else if rest[0] == b'\'' { (b'\'', 1) }
                                     else { (b' ', 0) };
                    let s = &rest[close..];
                    let end = s.iter().position(|&b| b == q || (q == b' ' && (b == b'>' || b == b' '))).unwrap_or(s.len());
                    return &s[..end];
                }
            }
        }
        i += 1;
    }
    &[]
}

/// Concatenate the text of every <style>…</style> block in an HTML body
/// (case-insensitive) so the CSS engine can parse the page's rules.
fn extract_style(html: &[u8]) -> String {
    let lower: Vec<u8> = html.iter().map(|b| b.to_ascii_lowercase()).collect();
    let mut out = String::new();
    let mut i = 0usize;
    while i < lower.len() {
        let rel = match lower[i..].windows(6).position(|w| w == b"<style") { Some(p) => p, None => break };
        let open = i + rel;
        let gt = match lower[open..].iter().position(|&b| b == b'>') { Some(g) => open + g + 1, None => break };
        if gt >= lower.len() { break; }
        let close = match lower[gt..].windows(7).position(|w| w == b"</style") { Some(c) => gt + c, None => break };
        if let Ok(s) = core::str::from_utf8(&html[gt..close]) { out.push_str(s); out.push('\n'); }
        i = close + 7;
    }
    out
}

/// Resolve a page's CSS for one element `tag` over PinguBrowser's reader default
/// (base_color, base_size). Any property the page's CSS doesn't set keeps the
/// reader value, so unstyled pages still look clean.
fn style_tag(sheet: &crate::tcss::Stylesheet, tag: &str, color: u32, size: u8) -> crate::tcss::ComputedStyle {
    let mut base = crate::tcss::ComputedStyle::default();
    base.color = color; base.font_size = size;
    sheet.resolve_with_base(&crate::tcss::Element::new(tag), "", base)
}

/// Pull the text of the first <title>…</title> out of an HTML body (case-insensitive).
fn extract_title(html: &[u8]) -> String {
    let lower: Vec<u8> = html.iter().map(|b| b.to_ascii_lowercase()).collect();
    if let Some(open) = lower.windows(6).position(|w| w == b"<title") {
        // skip to the '>' that closes the opening tag
        if let Some(gt) = html[open..].iter().position(|&b| b == b'>') {
            let start = open + gt + 1;
            if let Some(close) = lower[start..].windows(7).position(|w| w == b"</title") {
                let raw = core::str::from_utf8(&html[start..start + close]).unwrap_or("").trim();
                return collapse_ws(raw);
            }
        }
    }
    String::new()
}

/// Draw a small padlock at (lx, ly) in `col`. `closed` = both shackle posts down
/// (verified HTTPS); open = right post raised (plain HTTP / not secure).
fn draw_lock(fb: &mut Framebuffer, lx: i32, ly: i32, col: u32, closed: bool) {
    let sx = lx + 2;
    fb.fill_rect(sx as u32, ly as u32, 6, 2, col);          // top of the shackle arch
    fb.fill_rect(sx as u32, (ly + 1) as u32, 2, 5, col);    // left post
    if closed {
        fb.fill_rect((sx + 4) as u32, (ly + 1) as u32, 2, 5, col); // right post down = locked
    } else {
        fb.fill_rect((sx + 4) as u32, (ly - 1) as u32, 2, 4, col); // right post raised = open
    }
    fb.fill_rounded_rect(lx, ly + 5, 10, 8, 2, col);        // lock body
}

/// Parse HTML bytes into a list of visual nodes.
fn parse_html(html: &[u8]) -> Vec<HNode> {
    let mut nodes: Vec<HNode> = Vec::new();
    let mut buf:   Vec<u8>    = Vec::new();
    let mut tag_buf: Vec<u8>  = Vec::new();
    let mut in_tag   = false;
    let mut skip_depth: u32 = 0;  // depth of <script>/<style>/<head>
    let mut ctx_h: u8 = 0;        // 1=h1, 2=h2, 3=h3
    let mut in_link  = false;
    let mut link_href: Vec<u8> = Vec::new();
    let mut in_li    = false;
    let mut in_pre   = false;

    macro_rules! flush {
        () => {{
            if !buf.is_empty() {
                let raw = core::str::from_utf8(&buf).unwrap_or("").trim();
                let s = collapse_ws(raw);
                if !s.is_empty() {
                    let node = if ctx_h == 1 { HNode::H1(s) }
                               else if ctx_h == 2 { HNode::H2(s) }
                               else if ctx_h == 3 { HNode::H3(s) }
                               else if in_link {
                                   let href = String::from(core::str::from_utf8(&link_href).unwrap_or(""));
                                   HNode::Link { text: s, href }
                               }
                               else if in_li { HNode::Li(s) }
                               else { HNode::Para(s) };
                    nodes.push(node);
                }
                buf.clear();
            }
        }};
    }

    let mut i = 0usize;
    while i < html.len() {
        let b = html[i];
        if b == b'<' && !in_pre {
            if skip_depth == 0 { flush!(); }
            in_tag = true; tag_buf.clear(); i += 1; continue;
        }
        if b == b'>' && in_tag {
            in_tag = false;
            // Parse tag
            let tag = tag_buf.as_slice();
            let is_close = tag.starts_with(b"/");
            let start = if is_close { 1 } else { 0 };
            let nend = tag[start..].iter()
                .position(|&c| c == b' ' || c == b'\t' || c == b'/' || c == b'\n')
                .map(|p| p + start).unwrap_or(tag.len());
            let name_raw = &tag[start..nend];
            let mut name_lc = [0u8; 16];
            let nl = name_raw.len().min(16);
            for (k, &c) in name_raw[..nl].iter().enumerate() {
                name_lc[k] = c.to_ascii_lowercase();
            }
            let name = &name_lc[..nl];

            if skip_depth > 0 {
                if is_close && matches!(name, b"script" | b"style" | b"head" | b"noscript") {
                    skip_depth -= 1;
                }
                i += 1; continue;
            }
            if is_close {
                match name {
                    b"a"   => { flush!(); in_link = false; link_href.clear(); }
                    b"li"  => { flush!(); in_li = false; }
                    b"h1"  => { flush!(); ctx_h = 0; }
                    b"h2"  => { flush!(); ctx_h = 0; }
                    b"h3"  => { flush!(); ctx_h = 0; }
                    b"pre" => { in_pre = false; }
                    b"p"|b"div"|b"section"|b"article"|b"main"|b"header"|b"footer"|b"nav"|b"td"|b"th"|b"tr" => {
                        flush!();
                        if !nodes.last().map(|n| matches!(n, HNode::Blank)).unwrap_or(true) {
                            nodes.push(HNode::Blank);
                        }
                    }
                    _ => {}
                }
            } else {
                match name {
                    b"script"|b"style"|b"head"|b"noscript" => { skip_depth += 1; }
                    b"h1" => { flush!(); ctx_h = 1; buf.clear(); }
                    b"h2" => { flush!(); ctx_h = 2; buf.clear(); }
                    b"h3" => { flush!(); ctx_h = 3; buf.clear(); }
                    b"a"  => {
                        flush!();
                        in_link = true; buf.clear();
                        link_href.clear();
                        let href = html_attr(tag, b"href");
                        link_href.extend_from_slice(href);
                    }
                    b"input" => {
                        let tp = html_attr(tag, b"type");
                        let is_hidden = tp.eq_ignore_ascii_case(b"hidden") || tp.eq_ignore_ascii_case(b"submit");
                        if !is_hidden {
                            let ph = html_attr(tag, b"placeholder");
                            let ph = String::from(core::str::from_utf8(ph).unwrap_or("Search"));
                            nodes.push(HNode::Input { placeholder: if ph.is_empty() { String::from("Search") } else { ph } });
                        }
                    }
                    b"hr" => { flush!(); nodes.push(HNode::Hr); }
                    b"br" => { flush!(); }
                    b"li" => { flush!(); in_li = true; buf.clear(); }
                    b"pre"=> { in_pre = true; }
                    b"p"|b"div"|b"section"|b"article"|b"main"|b"header"|b"footer"|b"nav" => {
                        flush!();
                    }
                    _ => {}
                }
            }
            i += 1; continue;
        }
        if in_tag { tag_buf.push(b); i += 1; continue; }
        if skip_depth > 0 { i += 1; continue; }

        // Text content
        if b == b'&' {
            let rest = &html[i..];
            let (ch, len) = if rest.starts_with(b"&amp;")  { (b'&', 5) }
                       else if rest.starts_with(b"&lt;")   { (b'<', 4) }
                       else if rest.starts_with(b"&gt;")   { (b'>', 4) }
                       else if rest.starts_with(b"&nbsp;") { (b' ', 6) }
                       else if rest.starts_with(b"&quot;") { (b'"', 6) }
                       else if rest.starts_with(b"&#39;")  { (b'\'',5) }
                       else if rest.get(1) == Some(&b'#') {
                           if let Some(p) = rest.iter().position(|&c| c == b';') { (b' ', p + 1) }
                           else { (b'&', 1) }
                       }
                       else { (b'&', 1) };
            if ch != 0 { buf.push(ch); }
            i += len; continue;
        }
        if b == b'\r' { i += 1; continue; }
        if in_pre {
            buf.push(b);
        } else if b == b'\n' || b == b'\t' {
            if !buf.is_empty() && *buf.last().unwrap() != b' ' { buf.push(b' '); }
        } else {
            buf.push(b);
        }
        i += 1;
    }
    flush!();

    // Remove leading/trailing blanks and collapse runs
    while nodes.first().map(|n| matches!(n, HNode::Blank)).unwrap_or(false) { nodes.remove(0); }
    let mut k = 1;
    while k < nodes.len() {
        if matches!(nodes[k], HNode::Blank) && matches!(nodes[k-1], HNode::Blank) {
            nodes.remove(k);
        } else { k += 1; }
    }
    nodes
}

fn collapse_ws(s: &str) -> String {
    let mut out = String::new();
    let mut sp = true;
    for ch in s.chars() {
        if ch.is_ascii_whitespace() {
            if !sp { out.push(' '); sp = true; }
        } else { out.push(ch); sp = false; }
    }
    if out.ends_with(' ') { out.pop(); }
    out
}

/// Wrap a string into lines of at most `max_px` width using the AA_S font metrics.
fn wrap_text(s: &str, max_px: i32) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut cur_w = 0i32;
    for word in s.split_ascii_whitespace() {
        let ww = Framebuffer::aa_w(word, crate::fb::AA_S);
        let sp = if cur.is_empty() { 0 } else { Framebuffer::aa_w(" ", crate::fb::AA_S) };
        if !cur.is_empty() && cur_w + sp + ww > max_px {
            lines.push(cur.clone()); cur.clear(); cur_w = 0;
        }
        if !cur.is_empty() { cur.push(' '); cur_w += sp; }
        cur.push_str(word); cur_w += ww;
    }
    if !cur.is_empty() { lines.push(cur); }
    if lines.is_empty() { lines.push(String::new()); }
    lines
}

pub struct Browser {
    page:         usize,
    // URL bar
    url:          [u8; 128],
    url_len:      usize,
    addr_focused: bool,
    // Live page
    mode:         BrMode,
    resp_buf:     Vec<u8>,
    nodes:        Vec<HNode>,
    sheet:        crate::tcss::Stylesheet,  // parsed <style> rules for the live page
    scroll_px:    i32,
    // Link hit-test regions populated by the last render call: (y_abs, height, href)
    link_hits:    Vec<(i32, i32, String)>,
    auto_nav:     bool,              // fetch default URL on first tick
    security:     Security,          // lock indicator state for the live page
    title:        String,            // <title> of the live page (window/tab label)
    history:      Vec<String>,       // visited URLs (back stack); current = last
    fwd:          Vec<String>,       // forward stack (popped from history on Back)
    pub dirty:       bool,
    pub wants_close: bool,
    ansi:         crate::ansi::AnsiParser,
}

/// Decode %XX percent-encoding in a URL string.
fn url_decode(s: &str) -> String {
    let mut out = String::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let h = b[i+1]; let l = b[i+2];
            let hv = if h.is_ascii_digit() { h - b'0' } else { (h|32) - b'a' + 10 };
            let lv = if l.is_ascii_digit() { l - b'0' } else { (l|32) - b'a' + 10 };
            if hv < 16 && lv < 16 {
                out.push((hv * 16 + lv) as char); i += 3; continue;
            }
        }
        out.push(b[i] as char); i += 1;
    }
    out
}

impl Browser {
    pub fn new() -> Self {
        const HOME: &[u8] = b"google.com/search?q=rusty+penguin+os";
        let mut url = [0u8; 128];
        url[..HOME.len()].copy_from_slice(HOME);
        Browser {
            page: 0,
            url, url_len: HOME.len(),
            addr_focused: false,
            mode: BrMode::Static,
            resp_buf: Vec::new(),
            nodes: Vec::new(),
            sheet: crate::tcss::Stylesheet::parse(""),
            scroll_px: 0,
            link_hits: Vec::new(),
            auto_nav: true,
            security: Security::None,
            title: String::new(),
            history: Vec::new(),
            fwd: Vec::new(),
            dirty: true,
            wants_close: false,
            ansi: crate::ansi::AnsiParser::new(),
        }
    }

    fn set_url(&mut self, s: &[u8]) {
        let n = s.len().min(self.url.len());
        self.url[..n].copy_from_slice(&s[..n]);
        self.url_len = n;
    }

    /// Navigate to the URL currently in the address bar as a *new* history entry:
    /// records it on the back stack and clears the forward stack, then fetches.
    /// Used for typed URLs and link clicks. Reload and Back/Forward bypass this.
    fn navigate(&mut self) {
        let cur = String::from(core::str::from_utf8(&self.url[..self.url_len]).unwrap_or(""));
        if !cur.is_empty() && self.history.last() != Some(&cur) {
            self.history.push(cur);
            self.fwd.clear();
        }
        self.do_fetch();
    }

    /// Fetch and render whatever is in the address bar — no history bookkeeping.
    fn do_fetch(&mut self) {
        let url = &self.url[..self.url_len];
        if url.starts_with(b"rustypenguin://") || url.is_empty() {
            let slug = &url[url.iter().position(|&b| b == b'/').map(|p| p + 2).unwrap_or(0)..];
            let slug = if let Some(p) = slug.iter().position(|&b| b == b'/') { &slug[p+1..] } else { slug };
            self.page = match slug { b"about" => 1, b"ternary" => 2, b"roadmap" => 3, _ => 0 };
            self.mode = BrMode::Static;
            self.security = Security::None;
            self.title = String::new();
            self.dirty = true;
            return;
        }
        self.mode = BrMode::Loading;
        self.dirty = true;
        // Type-to-search: if the address bar holds words rather than a host
        // (has a space, or has no dot and no scheme), search Google over TLS.
        let is_scheme = url.starts_with(b"http://") || url.starts_with(b"https://");
        let has_space = url.contains(&b' ');
        let has_dot   = url.contains(&b'.');
        let target: alloc::vec::Vec<u8> = if !is_scheme && (has_space || !has_dot) {
            let mut q: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
            q.extend_from_slice(b"https://www.google.com/search?q=");
            for &c in url {
                match c {
                    b' ' => q.push(b'+'),
                    b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => q.push(c),
                    _ => q.extend_from_slice(alloc::format!("%{:02X}", c).as_bytes()),
                }
            }
            q
        } else {
            url.to_vec()
        };
        let mut buf = alloc::vec![0u8; FETCH_BUF];
        let n = unsafe { sys_http_get_raw(&target, &mut buf) };
        if n == 0 {
            self.mode = BrMode::Err;
            self.security = Security::None;
            self.dirty = true;
            return;
        }
        buf.truncate(n);
        self.resp_buf = buf;
        self.parse_page();
        // Trust indicator: ask the kernel how the last fetch was secured.
        self.security = match unsafe { sys_fetch_trust() } {
            1 => Security::Secure,
            0 => Security::Insecure,
            _ => Security::None,
        };
        self.scroll_px = 0;
        self.mode = BrMode::Live;
        self.dirty = true;
    }

    /// Parse the raw HTTP response in resp_buf into visual HNodes + the <title>.
    fn parse_page(&mut self) {
        self.nodes.clear();
        let data = &self.resp_buf;
        let body_start = data.windows(4).position(|w| w == b"\r\n\r\n")
            .map(|p| p + 4)
            .or_else(|| data.windows(2).position(|w| w == b"\n\n").map(|p| p + 2))
            .unwrap_or(0);
        let body = &data[body_start..];
        self.title = extract_title(body);
        // Parse the page's <style> blocks through the from-scratch CSS engine.
        self.sheet = crate::tcss::Stylesheet::parse(&extract_style(body));
        self.nodes = parse_html(body);
    }
}

impl App for Browser {
    fn tick(&mut self, _ticks: u64) -> bool {
        if self.auto_nav {
            self.auto_nav = false;
            self.navigate();
            return true;
        }
        false
    }

    fn render(&mut self, fb: &mut Framebuffer, x: u32, y: u32, w: u32, h: u32) {
        // ── Toolbar ────────────────────────────────────────────────────────────
        fb.fill_rect(x, y, w, BR_TOOLBAR_H, 0x1A2230);
        fb.fill_rect(x, y + BR_TOOLBAR_H, w, 1, 0x2A3A50);
        let by = y as i32 + 6;
        // Back button
        let can_back = self.can_back();
        let fg_back = if can_back { 0x6FE18B } else { 0x4A5A6A };
        fb.fill_rounded_rect(x as i32 + 6, by, 22, 22, 6,
            if can_back { 0x263040 } else { 0x1A2030 });
        fb.draw_aa(x as i32 + 12, by + 3, "<", fg_back, crate::fb::AA_S);
        // Forward button
        let can_fwd = self.can_fwd();
        let fg_fwd = if can_fwd { 0x6FE18B } else { 0x4A5A6A };
        fb.fill_rounded_rect(x as i32 + 30, by, 22, 22, 6,
            if can_fwd { 0x263040 } else { 0x1A2030 });
        fb.draw_aa(x as i32 + 36, by + 3, ">", fg_fwd, crate::fb::AA_S);
        // Reload
        fb.fill_rounded_rect(x as i32 + 54, by, 22, 22, 6, 0x1E2A3C);
        fb.draw_aa(x as i32 + 60, by + 3, "r", 0x4A9EFF, crate::fb::AA_S);
        // Address bar
        let ax = (x as i32 + 82) as u32;
        let aw = (x + w).saturating_sub(ax + 8);
        fb.fill_rounded_rect(ax as i32, by, aw as i32, 22, 10,
            if self.addr_focused { 0x243550 } else { 0x141E2C });
        fb.fill_rounded_rect(ax as i32 - 1, by - 1, aw as i32 + 2, 24, 10,
            if self.addr_focused { 0x4A9EFF } else { 0 });
        fb.fill_rounded_rect(ax as i32, by, aw as i32, 22, 10,
            if self.addr_focused { 0x243550 } else { 0x141E2C });
        // Security indicator: a real padlock when the page came over a validated
        // HTTPS chain; an amber "not secure" mark for plain HTTP; nothing local.
        let mut text_x = ax as i32 + 10;
        match (self.mode, self.security) {
            (BrMode::Live, Security::Secure)   => { draw_lock(fb, ax as i32 + 8, by + 5, 0x35C46A, true);  text_x = ax as i32 + 24; }
            (BrMode::Live, Security::Insecure) => { draw_lock(fb, ax as i32 + 8, by + 5, 0xE5A23C, false); text_x = ax as i32 + 24; }
            (BrMode::Loading, _) => { fb.fill_circle(ax as i32 + 12, by + 11, 3, 0xF5C451); text_x = ax as i32 + 22; }
            (BrMode::Err, _)     => { fb.fill_circle(ax as i32 + 12, by + 11, 3, 0xEF4444); text_x = ax as i32 + 22; }
            _ => {}
        }
        let url_str = core::str::from_utf8(&self.url[..self.url_len]).unwrap_or("");
        let cursor = if self.addr_focused { "\u{258c}" } else { "" };
        let mut ubuf = String::from(url_str); ubuf.push_str(cursor);
        fb.draw_aa(text_x, by + 4, &ubuf, 0xB8CCE0, crate::fb::AA_S);

        // ── Page area ─────────────────────────────────────────────────────────
        let py0  = y + BR_TOOLBAR_H + 1;
        let ph   = h.saturating_sub(BR_TOOLBAR_H + 1);
        let pw   = w;
        fb.fill_rect(x, py0, pw, ph, 0xF7F5F0);  // off-white paper
        // Readable centered column — on a full-HD canvas, full-width lines are
        // unreadable, so cap the text measure (~900px) and center it on the page.
        let rw   = (pw as i32 - 56).min(900);
        let lx   = x as i32 + (pw as i32 - rw) / 2;

        match self.mode {
            BrMode::Static => {
                let page = br_page(self.page);
                let mut cy = py0 as i32 + 16;
                for ln in page.lines {
                    if cy as u32 + 30 > py0 + ph { break; }
                    match ln.kind {
                        K_H1   => { fb.draw_aa(lx, cy, ln.text, 0x1A4A80, crate::fb::AA_L); }
                        K_H2   => { fb.draw_aa(lx, cy, ln.text, 0xB4502A, crate::fb::AA_S); }
                        K_P    => { fb.draw_aa(lx, cy, ln.text, 0x2A2A24, crate::fb::AA_S); }
                        K_NOTE => { fb.draw_aa(lx, cy, ln.text, 0x888880, crate::fb::AA_T); }
                        K_LINK => {
                            let lw = Framebuffer::aa_w(ln.text, crate::fb::AA_S);
                            fb.draw_aa(lx, cy, ln.text, 0x1A5FBE, crate::fb::AA_S);
                            fb.fill_rect(lx as u32, (cy + 17) as u32, lw as u32, 1, 0x1A5FBE);
                        }
                        _ => {}
                    }
                    cy += br_line_advance(ln.kind);
                }
            }
            BrMode::Loading => {
                fb.draw_aa(lx, py0 as i32 + 44, "Connecting...", 0x8A8A82, crate::fb::AA_S);
            }
            BrMode::Err => {
                fb.draw_aa(lx, py0 as i32 + 44, "Could not load page.", 0xC0392B, crate::fb::AA_S);
                fb.draw_aa(lx, py0 as i32 + 66, "Check hostname or network.", 0x888880, crate::fb::AA_T);
            }
            BrMode::Live => {
                self.link_hits.clear();
                let scroll = self.scroll_px;
                let bottom = py0 as i32 + ph as i32;
                // Lay out all nodes, clipping to viewport
                let mut doc_y = 0i32;  // y in document space
                let line_h_body: i32 = 20;
                // The page's parsed CSS (tag-level cascade over reader defaults).
                let sheet = &self.sheet;

                for node in self.nodes.iter() {
                    let node_h = match node {
                        HNode::Para(s) | HNode::Li(s) => {
                            let lines = wrap_text(s, rw);
                            lines.len() as i32 * line_h_body + 4
                        }
                        HNode::Link { text, .. } => {
                            let lines = wrap_text(text, rw);
                            lines.len() as i32 * line_h_body + 2
                        }
                        n => n.base_h(),
                    };

                    let screen_y = py0 as i32 + doc_y - scroll;
                    if screen_y + node_h >= py0 as i32 && screen_y < bottom {
                        // Draw this node
                        let sy = screen_y;
                        match node {
                            HNode::H1(s) => {
                                let st = style_tag(sheet, "h1", 0x1A2040, crate::fb::AA_L);
                                fb.draw_aa(lx, sy + 4, s, st.color, st.font_size);
                                fb.fill_rect(lx as u32, (sy + node_h - 3) as u32, rw as u32, 1, 0xCCCCC4);
                            }
                            HNode::H2(s) => {
                                let st = style_tag(sheet, "h2", 0x1A3A70, crate::fb::AA_S);
                                fb.draw_aa(lx, sy + 4, s, st.color, st.font_size);
                            }
                            HNode::H3(s) => {
                                let st = style_tag(sheet, "h3", 0x2A4A80, crate::fb::AA_S);
                                fb.draw_aa(lx, sy + 4, s, st.color, st.font_size);
                            }
                            HNode::Para(s) => {
                                let st = style_tag(sheet, "p", 0x2A2A24, crate::fb::AA_S);
                                let mut ly = sy + 2;
                                for line in wrap_text(s, rw) {
                                    fb.draw_aa(lx, ly, &line, st.color, st.font_size);
                                    ly += line_h_body;
                                }
                            }
                            HNode::Li(s) => {
                                let st = style_tag(sheet, "li", 0x2A2A24, crate::fb::AA_S);
                                fb.draw_aa(lx, sy + 2, "\u{2022}", 0x1A5FBE, crate::fb::AA_S);
                                let mut ly = sy + 2;
                                for line in wrap_text(s, rw - 18) {
                                    fb.draw_aa(lx + 18, ly, &line, st.color, st.font_size);
                                    ly += line_h_body;
                                }
                            }
                            HNode::Link { text, href } => {
                                let st = style_tag(sheet, "a", 0x1A5FBE, crate::fb::AA_S);
                                let mut ly = sy + 2;
                                let lines = wrap_text(text, rw);
                                let total_h = lines.len() as i32 * line_h_body;
                                for line in &lines {
                                    let lw = Framebuffer::aa_w(line, crate::fb::AA_S);
                                    fb.draw_aa(lx, ly, line, st.color, crate::fb::AA_S);
                                    fb.fill_rect(lx as u32, (ly + 17) as u32, lw as u32, 1, st.color);
                                    ly += line_h_body;
                                }
                                // Resolve relative hrefs: prepend current host if path-only
                                let href_s = if href.starts_with('/') || href.starts_with("http") {
                                    href.clone()
                                } else { href.clone() };
                                self.link_hits.push((sy, sy + total_h + 4, href_s));
                            }
                            HNode::Input { placeholder } => {
                                fb.fill_rounded_rect(lx, sy + 4, rw.min(320), 24, 8, 0xFFFFFF);
                                fb.fill_rounded_rect(lx - 1, sy + 3, rw.min(320) + 2, 26, 8, 0xAAAA99);
                                fb.fill_rounded_rect(lx, sy + 4, rw.min(320), 24, 8, 0xFFFFFF);
                                fb.draw_aa(lx + 10, sy + 8, placeholder, 0xAAAAAA, crate::fb::AA_T);
                            }
                            HNode::Hr => {
                                fb.fill_rect(lx as u32, (sy + 6) as u32, rw as u32, 1, 0xCCCCC4);
                            }
                            HNode::Blank => {}
                        }
                    }
                    doc_y += node_h;
                }

                // Scroll bar
                let total_h = doc_y.max(1);
                if total_h > ph as i32 {
                    let bar_h = ((ph as i32 * ph as i32) / total_h).max(20) as u32;
                    let bar_y = py0 + (scroll * ph as i32 / total_h).max(0) as u32;
                    let bar_y = bar_y.min(py0 + ph - bar_h);
                    fb.fill_rounded_rect(x as i32 + pw as i32 - 8, bar_y as i32, 5, bar_h as i32, 2, 0xAAAAAA);
                }
            }
        }
        self.dirty = false;
    }

    fn on_mouse(&mut self, mx: i32, my: i32, w: u32, h: u32, buttons: u8) {
        let pressed = (buttons & 1) != 0;
        if !pressed { return; }
        let by = 6i32;
        let ax = 82i32;
        let aw = (w as i32).saturating_sub(ax + 8);
        // Address bar
        if mx >= ax && mx < ax + aw && my >= by && my < by + 22 {
            self.addr_focused = true; self.dirty = true; return;
        }
        self.addr_focused = false;
        // Back button
        if mx >= 6 && mx < 28 && my >= by && my < by + 22 {
            self.go_back(); return;
        }
        // Forward button
        if mx >= 30 && mx < 52 && my >= by && my < by + 22 {
            self.go_forward(); return;
        }
        // Reload (re-fetch current page; no new history entry)
        if mx >= 54 && mx < 76 && my >= by && my < by + 22 {
            self.do_fetch(); return;
        }
        // Static page links
        if self.mode == BrMode::Static {
            let page = br_page(self.page);
            let mut cy = BR_TOOLBAR_H as i32 + 1 + 16;
            for ln in page.lines {
                let adv = br_line_advance(ln.kind);
                if ln.kind == K_LINK && ln.link >= 0 {
                    let lw = Framebuffer::aa_w(ln.text, crate::fb::AA_S);
                    if mx >= 28 && mx < 28 + lw && my >= cy && my < cy + adv {
                        self.page = ln.link as usize;
                        self.update_url_from_page();
                        self.dirty = true; return;
                    }
                }
                cy += adv;
            }
        }
        // Live page: check link hit-test regions first, then scroll
        if self.mode == BrMode::Live {
            let content_y0 = BR_TOOLBAR_H as i32 + 1;
            if my > content_y0 {
                for (y0, y1, href) in self.link_hits.clone() {
                    if my >= y0 && my < y1 && !href.is_empty() {
                        let resolved = self.resolve_href(&href);
                        self.set_url(resolved.as_bytes());
                        self.navigate();
                        return;
                    }
                }
                // Scroll: up half = scroll up, bottom half = scroll down
                let ph = h as i32 - content_y0;
                if my < content_y0 + ph / 2 {
                    self.scroll_px = (self.scroll_px - 60).max(0);
                } else {
                    self.scroll_px += 60;
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
                    self.addr_focused = false; self.navigate();
                }
                AK::Char(0x08) | AK::Char(0x7F) => {
                    if self.url_len > 0 { self.url_len -= 1; self.dirty = true; }
                }
                AK::Char(ch) if ch >= 0x20 && ch < 0x7F => {
                    if self.url_len < self.url.len() - 1 {
                        self.url[self.url_len] = ch; self.url_len += 1; self.dirty = true;
                    }
                }
                AK::Up   => { self.scroll_px = (self.scroll_px - 30).max(0); self.dirty = true; }
                AK::Down => { self.scroll_px += 30; self.dirty = true; }
                _ => {}
            }
        } else {
            match self.ansi.feed(key) {
                AK::Up   => { self.scroll_px = (self.scroll_px - 60).max(0); self.dirty = true; }
                AK::Down => { self.scroll_px += 60; self.dirty = true; }
                _ => {}
            }
        }
    }

    fn wants_close(&self) -> bool { self.wants_close }
    fn title(&self) -> &str {
        // Show the live page's <title> in the window chrome when we have one.
        if self.mode == BrMode::Live && !self.title.is_empty() { &self.title } else { "PinguBrowser" }
    }
}

impl Browser {
    /// Resolve a (possibly relative) href against the current URL.
    /// Handles Google's /url?q=https://... redirect format.
    fn resolve_href(&self, href: &str) -> String {
        // Already absolute
        if href.starts_with("http://") || href.starts_with("https://") {
            return String::from(href);
        }
        // Google redirect: /url?q=https://...&...  → extract target
        if href.contains("/url?") || href.contains("url?q=") {
            if let Some(q) = href.find("q=") {
                let val = &href[q+2..];
                let end = val.find('&').unwrap_or(val.len());
                let target = url_decode(&val[..end]);
                if target.starts_with("http") { return target; }
            }
        }
        // Protocol-relative: //example.com/...
        if href.starts_with("//") {
            return String::from("https:") + href;
        }
        // Relative path: prepend current host
        let url = core::str::from_utf8(&self.url[..self.url_len]).unwrap_or("");
        let host_end = url.find('/').unwrap_or(url.len());
        let host = &url[..host_end];
        if href.starts_with('/') {
            String::from(host) + href
        } else {
            String::from(host) + "/" + href
        }
    }

    fn can_back(&self) -> bool { self.history.len() >= 2 }
    fn can_fwd(&self)  -> bool { !self.fwd.is_empty() }

    /// Back: pop the current page onto the forward stack and re-fetch the previous.
    fn go_back(&mut self) {
        if self.history.len() >= 2 {
            let cur = self.history.pop().unwrap();
            self.fwd.push(cur);
            let prev = self.history.last().unwrap().clone();
            self.set_url(prev.as_bytes());
            self.do_fetch();
        } else {
            // No history depth — fall back to the local home page.
            self.mode = BrMode::Static; self.page = 0;
            self.update_url_from_page();
            self.security = Security::None;
            self.dirty = true;
        }
    }

    /// Forward: replay a page we backed out of.
    fn go_forward(&mut self) {
        if let Some(next) = self.fwd.pop() {
            self.history.push(next.clone());
            self.set_url(next.as_bytes());
            self.do_fetch();
        }
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
        fb.fill_rect(x, y, w, 28, 0x1B2230);
        fb.draw_aa(x as i32 + 12, y as i32 + 5, "Sound", 0xEAF4F0, crate::fb::AA_S);
        fb.draw_aa(x as i32 + 80, y as i32 + 7, "Intel HDA  44.1 kHz  stereo  16-bit", 0x6B756D, crate::fb::AA_T);

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
            let st = "STOPPED";
            fb.draw_aa(x as i32 + 24 + (bar_w - Framebuffer::aa_w(st, crate::fb::AA_S)) / 2,
                         oy + bar_h / 2 - 9, st, 0x4A5260, crate::fb::AA_S);
        }
        oy += bar_h + 14;

        // Note selector row
        fb.draw_aa(cx + 12, oy + 2, "Note:", 0xB8B8B8, crate::fb::AA_S);
        let nx = cx + 60;
        for (i, name) in SCALE_NAMES.iter().enumerate() {
            let bx = nx + i as i32 * 34;
            let sel = i == self.note_idx;
            let bg = if sel { 0x2E7D4F } else { 0x232B38 };
            let fg = if sel { 0xF5F5F7 } else { 0x9CA3AF };
            fb.fill_rounded_rect(bx, oy, 30, 22, 5, bg);
            let lw = Framebuffer::aa_w(name, crate::fb::AA_S);
            fb.draw_aa(bx + (30 - lw) / 2, oy + 2, name, fg, crate::fb::AA_S);
        }
        oy += 30;

        // Volume slider
        fb.draw_aa(cx + 12, oy + 2, "Vol:", 0xB8B8B8, crate::fb::AA_S);
        let vx = cx + 55;
        let vw = 200i32;
        fb.fill_rect(vx as u32, oy as u32 + 8, vw as u32, 8, 0x232B38);
        let filled = (self.vol as i32 * vw / 127).max(0).min(vw) as u32;
        fb.fill_rect(vx as u32, oy as u32 + 8, filled, 8, 0x4A9EFF);
        let mut vbuf = [0u8; 24];
        let vs = u64_into(&mut vbuf, self.vol as u64);
        fb.draw_aa(vx + vw + 8, oy + 2, vs, 0xB8B8B8, crate::fb::AA_S);
        // Vol –/+ buttons
        fb.fill_rounded_rect(vx + vw + 36, oy, 22, 22, 5, 0x232B38);
        fb.draw_aa(vx + vw + 44, oy + 2, "-", 0xF5C451, crate::fb::AA_S);
        fb.fill_rounded_rect(vx + vw + 62, oy, 22, 22, 5, 0x232B38);
        fb.draw_aa(vx + vw + 69, oy + 2, "+", 0x6FE18B, crate::fb::AA_S);
        oy += 36;

        // Play/Stop button
        let (btn_label, btn_bg) = if self.playing { ("Stop", 0x7D2E2E) } else { ("Play", 0x2E7D4F) };
        fb.fill_rounded_rect(cx + 12, oy, 80, 30, 7, btn_bg);
        let lw = Framebuffer::aa_w(btn_label, crate::fb::AA_S);
        fb.draw_aa(cx + 12 + (80 - lw) / 2, oy + 6, btn_label, 0xF5F5F7, crate::fb::AA_S);

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
// Media — the founding clip (Linus Torvalds, OSS EU 2024) plays IN a window on
// the OS it triggered. The kernel owns the decoder + the HDA ring + the 58 MiB
// .rpv (syscalls 30/31/32); this app draws the chrome, paces the frames, and
// asks the kernel to scale each frame straight into the backbuffer. Press V for
// the Windows-Media-Player "Plasma" visualizer — a from-scratch easter egg that
// pulses to the audio amplitude the kernel reports each frame.
// ─────────────────────────────────────────────────────────────────────────────

/// sys_video_open (#30) → packed dims (w<<48)|(h<<32)|(fps<<24)|nframes, or 0.
unsafe fn sys_video_open() -> u64 {
    let n: u64;
    core::arch::asm!("syscall", inout("rax") 30u64 => n, in("rdi") 0u64,
        out("rcx") _, out("r11") _, options(nostack));
    n
}
/// sys_video_advance (#31) → (audio_level<<32)|frame_index.
unsafe fn sys_video_advance() -> u64 {
    let n: u64;
    core::arch::asm!("syscall", inout("rax") 31u64 => n, in("rdi") 0u64,
        out("rcx") _, out("r11") _, options(nostack));
    n
}
/// sys_video_blit (#32): scale the current frame into the backbuffer at `base`.
/// `rect` packs (dx<<48)|(dy<<32)|(dw<<16)|dh, each 16 bits.
unsafe fn sys_video_blit(base: u64, rect: u64) {
    core::arch::asm!("syscall", in("rax") 32u64, in("rdi") base, in("rsi") rect,
        out("rcx") _, out("r11") _, options(nostack));
}

/// Integer sine over a 256-step circle, returning roughly -1024..=1024. Reuses
/// the quarter-sine QSINE table (no float / no libm in the render path).
fn vsin(phase: i32) -> i32 {
    let p = ((phase % 256) + 256) % 256;     // 0..255
    let quad = p / 64;
    let t = ((p % 64) * 25 / 64) as usize;    // 0..24 into QSINE (rising quarter)
    let rise = QSINE[t.min(25)] as i32;
    let fall = QSINE[(25 - t).min(25)] as i32;
    let v = match quad { 0 => rise, 1 => fall, 2 => -rise, _ => -fall };
    v >> 5
}

pub struct MediaPlayer {
    avail: bool,        // bin/meta.rpv present in initrd
    w: u32, h: u32,     // native video dims
    fps: u32,
    nframes: u32,
    frame: u32,         // current frame index
    playing: bool,
    last_step: u64,     // tick of last frame advance
    last_viz: u64,      // tick of last visualizer animation step
    interval: u64,      // ticks between frames (100 Hz / fps)
    viz: bool,          // WMP visualizer easter egg active
    level: u32,         // last reported audio amplitude 0..255
    t: i32,             // visualizer animation time
    pub dirty: bool,
    pub wants_close: bool,
    ansi: crate::ansi::AnsiParser,
}

impl MediaPlayer {
    pub fn new() -> Self {
        let packed = unsafe { sys_video_open() };
        let w = ((packed >> 48) & 0xFFFF) as u32;
        let h = ((packed >> 32) & 0xFFFF) as u32;
        let fps = ((packed >> 24) & 0xFF) as u32;
        let nframes = (packed & 0xFF_FFFF) as u32;
        let avail = packed != 0 && w > 0 && h > 0;
        let interval = if fps > 0 { (100 / fps as u64).max(1) } else { 4 };
        MediaPlayer {
            avail, w, h, fps, nframes, frame: 0,
            playing: avail,              // autoplay the founding clip
            last_step: 0, last_viz: 0, interval,
            viz: false, level: 0, t: 0,
            dirty: true, wants_close: false,
            ansi: crate::ansi::AnsiParser::new(),
        }
    }

    /// Draw the Windows-Media-Player "Plasma" visualizer into the content area.
    /// Rotating rainbow plasma + an audio-reactive oscilloscope sweep. Cell-based
    /// (8 px) so the cost stays bounded even at large window sizes.
    fn draw_visualizer(&self, fb: &mut Framebuffer, x: u32, y: u32, w: u32, h: u32) {
        let cell = 8u32;
        let lvl = self.level as i32;
        let cols = w / cell;
        let rows = h / cell;
        for ry in 0..rows {
            let cy = (ry * cell) as i32;
            for rx in 0..cols {
                let cx = (rx * cell) as i32;
                // Three interfering plasma waves + audio drive → hue rotation.
                let v = vsin(cx / 3 + self.t)
                      + vsin(cy / 3 - self.t / 2)
                      + vsin((cx + cy) / 4 + self.t * 2)
                      + lvl * 6;
                let phase = v / 6 + self.t + lvl * 3;
                let r = ((vsin(phase) + 1024) * 255 / 2048) as u32;
                let g = ((vsin(phase + 85) + 1024) * 255 / 2048) as u32;
                let b = ((vsin(phase + 170) + 1024) * 255 / 2048) as u32;
                fb.fill_rect(x + rx * cell, y + ry * cell, cell, cell, (r << 16) | (g << 8) | b);
            }
        }
        // Audio-reactive oscilloscope sweeping the mid-line — amplitude tracks
        // the kernel-reported level so it visibly jumps to Linus's voice.
        let mid = y as i32 + h as i32 / 2;
        let amp = (lvl + 12) * (h as i32 / 3) / 255;
        let mut px = -1i32;
        let mut py = mid;
        let mut i = 0u32;
        while i < w {
            let xi = x as i32 + i as i32;
            let yi = mid - vsin(i as i32 * 3 + self.t * 4) * amp / 1024
                         + vsin(i as i32 * 7 - self.t * 3) * amp / 2048;
            if px >= 0 {
                let (ya, yb) = if py <= yi { (py, yi) } else { (yi, py) };
                let mut yy = ya;
                while yy <= yb {
                    if yy >= y as i32 && yy < (y + h) as i32 {
                        fb.set_pixel(xi as u32, yy as u32, 0xF5F5F7);
                    }
                    yy += 1;
                }
            }
            px = xi; py = yi;
            i += 2;
        }
    }

    fn draw_transport(&self, fb: &mut Framebuffer, x: u32, y: u32, w: u32, bar_h: u32) {
        fb.fill_rect(x, y, w, bar_h, 0x14171A);
        // Play/pause glyph (pure SVG-style rects/triangle — no emoji).
        let gx = x + 10; let gy = y + bar_h / 2;
        if self.playing {
            fb.fill_rect(gx, gy - 5, 3, 10, 0x6FE18B);
            fb.fill_rect(gx + 5, gy - 5, 3, 10, 0x6FE18B);
        } else {
            for r in 0..10 {
                let half = (10 - r) / 2;
                fb.fill_rect(gx, gy - 5 + r, half.max(1), 1, 0x6FE18B);
            }
        }
        // Progress bar.
        let bx = x + 28; let bw = w.saturating_sub(150);
        fb.fill_rect(bx, gy - 2, bw, 4, 0x2A332F);
        if self.nframes > 0 {
            let fill = (self.frame.min(self.nframes) as u64 * bw as u64 / self.nframes as u64) as u32;
            fb.fill_rect(bx, gy - 2, fill, 4, 0x6FE18B);
        }
        // Frame counter + hint.
        let mut fb_buf = [0u8; 24];
        let mut nf_buf = [0u8; 24];
        let tx = (x + 32 + bw) as i32;
        let mut px = tx;
        px += fb.draw_aa(px, y as i32 + 3, u64_into(&mut fb_buf, self.frame as u64), 0xA8B0A6, crate::fb::AA_T) + 8;
        px += fb.draw_aa(px, y as i32 + 3, "/", 0x6B756D, crate::fb::AA_T) + 8;
        fb.draw_aa(px, y as i32 + 3, u64_into(&mut nf_buf, self.nframes as u64), 0x6B756D, crate::fb::AA_T);
        let hint = if self.viz { "V: video" } else { "V: visualizer" };
        fb.draw_aa(x as i32 + 10, (y + bar_h) as i32 - 14, hint, 0x6B756D, crate::fb::AA_T);
    }
}

impl App for MediaPlayer {
    fn tick(&mut self, ticks: u64) -> bool {
        if !self.avail { return false; }
        let mut changed = false;
        if self.playing && ticks.wrapping_sub(self.last_step) >= self.interval {
            self.last_step = ticks;
            let r = unsafe { sys_video_advance() };
            self.level = ((r >> 32) & 0xFF) as u32;
            self.frame = (r & 0xFFFF_FFFF) as u32;
            changed = true;
        }
        // Animate the visualizer at ~25 fps independently of the video cadence.
        if self.viz && ticks.wrapping_sub(self.last_viz) >= 4 {
            self.last_viz = ticks;
            self.t = self.t.wrapping_add(6);
            changed = true;
        }
        changed
    }

    fn render(&mut self, fb: &mut Framebuffer, x: u32, y: u32, w: u32, h: u32) {
        let bar_h = 26u32;
        let vh = h.saturating_sub(bar_h);

        if !self.avail {
            fb.fill_rect(x, y, w, h, 0x14171A);
            fb.draw_aa(x as i32 + 16, y as i32 + 16, "Media Player", 0xECEDE5, crate::fb::AA_S);
            fb.draw_aa(x as i32 + 16, y as i32 + 42, "bin/meta.rpv not found in this build.", 0xA8B0A6, crate::fb::AA_T);
            self.dirty = false;
            return;
        }

        if self.viz {
            self.draw_visualizer(fb, x, y, w, vh);
        } else {
            // Letterbox backing, then the kernel scales the frame into the rect.
            fb.fill_rect(x, y, w, vh, 0x000000);
            let rect = ((x as u64 & 0xFFFF) << 48)
                     | ((y as u64 & 0xFFFF) << 32)
                     | ((w as u64 & 0xFFFF) << 16)
                     | (vh as u64 & 0xFFFF);
            unsafe { sys_video_blit(fb.data as u64, rect); }
        }
        self.draw_transport(fb, x, y + vh, w, bar_h);
        self.dirty = false;
    }

    fn on_key(&mut self, key: u8) {
        use crate::ansi::Key as AK;
        match self.ansi.feed(key) {
            AK::Char(b' ') => { self.playing = !self.playing; self.dirty = true; }
            AK::Char(b'v') | AK::Char(b'V') => { self.viz = !self.viz; self.dirty = true; }
            AK::Char(b'r') | AK::Char(b'R') => {
                // Restart from the top.
                unsafe { sys_video_open(); }
                self.frame = 0; self.playing = true; self.dirty = true;
            }
            _ => {}
        }
    }

    fn on_mouse(&mut self, mx: i32, my: i32, _w: u32, h: u32, buttons: u8) {
        // Click the lower transport strip toggles play/pause.
        if buttons & 1 != 0 && my >= (h as i32 - 26) && mx >= 0 {
            self.playing = !self.playing;
            self.dirty = true;
        }
    }

    fn title(&self) -> &str { "Media Player" }
}

// ─────────────────────────────────────────────────────────────────────────────
// Screenshot — capture the composited screen to a PPM in the VFS, with a live
// preview. The capture buffer is a reused static (the desktop heap is a bump
// allocator that never frees, so a per-capture Vec would leak). Full-screen
// capture only in v1; window/region need WM cooperation the app can't see yet.
// ─────────────────────────────────────────────────────────────────────────────

const SHOT_W: usize = 384;             // saved + preview thumbnail width (16:9)
const SHOT_H: usize = 216;
static mut SHOT_BUF: [u32; SHOT_W * SHOT_H] = [0; SHOT_W * SHOT_H];
static mut SHOT_PPM: [u8; SHOT_W * SHOT_H * 3 + 32] = [0; SHOT_W * SHOT_H * 3 + 32];
static mut SHOT_COUNTER: u32 = 0;      // shared filename counter (app + right-click)

/// Sample the full composited framebuffer into SHOT_BUF, encode a PPM into
/// SHOT_PPM, and write it to the VFS as screenshots/shot-N.ppm. Returns N.
/// Shared by the Screenshot app and the desktop's right-click "Take Screenshot".
pub fn capture_fullscreen(fb: &mut Framebuffer) -> u32 {
    let fw = fb.width as usize;
    let fh = fb.height as usize;
    if fw == 0 || fh == 0 { return unsafe { SHOT_COUNTER }; }
    unsafe {
        for ty in 0..SHOT_H {
            let sy = (ty * fh / SHOT_H) as u32;
            for tx in 0..SHOT_W {
                let sx = (tx * fw / SHOT_W) as u32;
                SHOT_BUF[ty * SHOT_W + tx] = fb.get_pixel(sx, sy);
            }
        }
        let hdr = b"P6\n384 216\n255\n";
        let mut p = 0usize;
        for &b in hdr { SHOT_PPM[p] = b; p += 1; }
        for i in 0..SHOT_W * SHOT_H {
            let rgb = SHOT_BUF[i];
            SHOT_PPM[p] = ((rgb >> 16) & 0xFF) as u8; p += 1;
            SHOT_PPM[p] = ((rgb >> 8) & 0xFF) as u8;  p += 1;
            SHOT_PPM[p] = (rgb & 0xFF) as u8;         p += 1;
        }
        SHOT_COUNTER += 1;
        let mut name = alloc::string::String::from("screenshots/shot-");
        let mut nb = [0u8; 24];
        name.push_str(u64_into(&mut nb, SHOT_COUNTER as u64));
        name.push_str(".ppm");
        vfs::vfs().write(&name, &SHOT_PPM[..p]);
        SHOT_COUNTER
    }
}

/// Most recent capture's thumbnail (SHOT_BUF) — used by the Image Viewer to show
/// the latest shot, and by the Screenshot app preview.
pub fn last_shot_buf() -> (&'static [u32], usize, usize) {
    unsafe { (&SHOT_BUF[..], SHOT_W, SHOT_H) }
}
pub fn shot_count() -> u32 { unsafe { SHOT_COUNTER } }

pub struct Screenshot {
    have_shot: bool,
    pending: bool,        // capture requested (executed in render where fb is live)
    saved_n: u32,         // index of the last saved file (for the status line)
    pub dirty: bool,
    pub wants_close: bool,
    ansi: crate::ansi::AnsiParser,
}

impl Screenshot {
    pub fn new() -> Self {
        Screenshot { have_shot: false, pending: false, saved_n: 0,
                     dirty: true, wants_close: false, ansi: crate::ansi::AnsiParser::new() }
    }

    /// Capture via the shared full-screen function. Runs inside render() so `fb`
    /// holds the live scene. The screenshot window itself appears in the shot (an
    /// in-desktop tool capturing itself) — acceptable for v1.
    fn capture(&mut self, fb: &mut Framebuffer) {
        self.saved_n = capture_fullscreen(fb);
        self.have_shot = true;
    }
}

impl App for Screenshot {
    fn render(&mut self, fb: &mut Framebuffer, x: u32, y: u32, w: u32, h: u32) {
        if self.pending {
            self.pending = false;
            self.capture(fb);
        }
        fb.fill_rect(x, y, w, h, 0x14181F);
        // Header.
        fb.fill_rect(x, y, w, 28, 0x1B2230);
        fb.draw_aa(x as i32 + 12, y as i32 + 5, "Screenshot", 0xECEDE5, crate::fb::AA_S);
        fb.draw_aa(x as i32 + 120, y as i32 + 7, "F or click Capture = full screen", 0x6B756D, crate::fb::AA_T);

        // Capture button (geometry unchanged — on_mouse hit-tests these coords).
        let bx = x + 12; let by = y + 38; let bw = 150u32; let bh = 26u32;
        fb.fill_rounded_rect(bx as i32, by as i32, bw as i32, bh as i32, 6, 0x335C3F);
        fb.fill_rect_s(bx as i32, by as i32, bw as i32, 1, 0x6FE18B);
        let cl = "Capture full screen";
        fb.draw_aa(bx as i32 + (bw as i32 - Framebuffer::aa_w(cl, crate::fb::AA_T)) / 2, by as i32 + 5, cl, 0xECEDE5, crate::fb::AA_T);

        // Preview area.
        let pvx = x + 12; let pvy = y + 74;
        let pvw = w.saturating_sub(24);
        let pvh = h.saturating_sub(74 + 28);
        fb.fill_rect(pvx, pvy, pvw, pvh, 0x0E1311);
        if self.have_shot && pvw > 16 && pvh > 16 {
            // Fit SHOT_BUF (384x216) into the preview, aspect-preserved.
            let s = ((pvw as usize * 1024 / SHOT_W).min(pvh as usize * 1024 / SHOT_H)).max(1);
            let outw = SHOT_W * s / 1024; let outh = SHOT_H * s / 1024;
            let ox = pvx + (pvw - outw as u32) / 2;
            let oy = pvy + (pvh - outh as u32) / 2;
            unsafe {
                for yy in 0..outh {
                    let sy = yy * SHOT_H / outh;
                    for xx in 0..outw {
                        let sx = xx * SHOT_W / outw;
                        fb.set_pixel(ox + xx as u32, oy + yy as u32, SHOT_BUF[sy * SHOT_W + sx]);
                    }
                }
            }
        } else {
            fb.draw_aa(pvx as i32 + 12, pvy as i32 + 12, "No capture yet — press F.", 0x6B756D, crate::fb::AA_S);
        }

        // Status line.
        let sy = y + h - 22;
        fb.fill_rect(x, sy, w, 22, 0x171F1B);
        if self.have_shot {
            let mut nb = [0u8; 24];
            let nstr = u64_into(&mut nb, self.saved_n as u64);
            let mut px = x as i32 + 12;
            px += fb.draw_aa(px, sy as i32 + 4, "saved  screenshots/shot-", 0xA8B0A6, crate::fb::AA_T);
            px += fb.draw_aa(px, sy as i32 + 4, nstr, 0x6FE18B, crate::fb::AA_T);
            fb.draw_aa(px, sy as i32 + 4, ".ppm  (384x216)", 0xA8B0A6, crate::fb::AA_T);
        } else {
            fb.draw_aa(x as i32 + 12, sy as i32 + 4, "ready", 0x6B756D, crate::fb::AA_T);
        }
        self.dirty = false;
    }

    fn on_key(&mut self, key: u8) {
        use crate::ansi::Key as AK;
        match self.ansi.feed(key) {
            AK::Char(b'f') | AK::Char(b'F') | AK::Char(b' ') => { self.pending = true; self.dirty = true; }
            _ => {}
        }
    }

    fn on_mouse(&mut self, mx: i32, my: i32, _w: u32, _h: u32, buttons: u8) {
        // The Capture button sits at content (12,38) size 150x26.
        if buttons & 1 != 0 && mx >= 12 && mx < 162 && my >= 38 && my < 64 {
            self.pending = true;
            self.dirty = true;
        }
    }

    fn title(&self) -> &str { "Screenshot" }
}

// ─────────────────────────────────────────────────────────────────────────────
// Image Viewer — decode + display a binary PPM (P6) from the VFS, scaled to fit.
// Pairs with the Screenshot tool (opens screenshots/shot-N.ppm), but works on any
// P6 PPM. N/P cycle through the saved screenshots; no heap (decodes in place).
// ─────────────────────────────────────────────────────────────────────────────

pub struct ImageViewer {
    cur: u32,             // current screenshot index (1..shot_count)
    pub dirty: bool,
    pub wants_close: bool,
    ansi: crate::ansi::AnsiParser,
}

impl ImageViewer {
    pub fn new() -> Self {
        ImageViewer { cur: shot_count().max(1), dirty: true, wants_close: false,
                      ansi: crate::ansi::AnsiParser::new() }
    }

    /// Parse a binary PPM (P6) header. Returns (width, height, pixel_data_offset).
    /// Tolerates a single comment line and arbitrary whitespace, per the spec.
    fn parse_ppm(data: &[u8]) -> Option<(usize, usize, usize)> {
        if data.len() < 2 || &data[0..2] != b"P6" { return None; }
        let mut i = 2usize;
        let mut nums = [0usize; 3]; // width, height, maxval
        let mut ni = 0usize;
        while ni < 3 && i < data.len() {
            // skip whitespace + comments
            while i < data.len() {
                let c = data[i];
                if c == b'#' { while i < data.len() && data[i] != b'\n' { i += 1; } }
                else if c == b' ' || c == b'\t' || c == b'\n' || c == b'\r' { i += 1; }
                else { break; }
            }
            let mut v = 0usize; let mut any = false;
            while i < data.len() && data[i].is_ascii_digit() {
                v = v * 10 + (data[i] - b'0') as usize; any = true; i += 1;
            }
            if !any { return None; }
            nums[ni] = v; ni += 1;
        }
        if ni < 3 { return None; }
        // exactly one whitespace byte separates the header from the pixel data
        if i < data.len() { i += 1; }
        Some((nums[0], nums[1], i))
    }

    /// Build "screenshots/shot-N.ppm" into a stack buffer — no heap. (render()
    /// runs whenever any other window animates, so a per-render String would
    /// steadily leak the never-freeing bump heap.)
    fn filename<'a>(&self, buf: &'a mut [u8; 40]) -> &'a str {
        let prefix = b"screenshots/shot-";
        let mut i = 0;
        for &b in prefix { buf[i] = b; i += 1; }
        let mut tmp = [0u8; 12]; let mut j = 0; let mut v = self.cur;
        if v == 0 { tmp[0] = b'0'; j = 1; } else { while v > 0 { tmp[j] = b'0' + (v % 10) as u8; v /= 10; j += 1; } }
        while j > 0 { j -= 1; buf[i] = tmp[j]; i += 1; }
        for &b in b".ppm" { buf[i] = b; i += 1; }
        core::str::from_utf8(&buf[..i]).unwrap_or("")
    }
}

impl App for ImageViewer {
    fn render(&mut self, fb: &mut Framebuffer, x: u32, y: u32, w: u32, h: u32) {
        fb.fill_rect(x, y, w, h, 0x14171A);
        // Header.
        fb.fill_rect(x, y, w, 28, 0x1B2230);
        fb.draw_aa(x as i32 + 12, y as i32 + 5, "Image Viewer", 0xECEDE5, crate::fb::AA_S);
        let mut nb = [0u8; 24]; let mut tb = [0u8; 24];
        let mut px = x as i32 + 130;
        px += fb.draw_aa(px, y as i32 + 7, "shot ", 0x6B756D, crate::fb::AA_T);
        px += fb.draw_aa(px, y as i32 + 7, u64_into(&mut nb, self.cur as u64), 0x8CC6E5, crate::fb::AA_T);
        px += fb.draw_aa(px, y as i32 + 7, " / ", 0x6B756D, crate::fb::AA_T);
        fb.draw_aa(px, y as i32 + 7, u64_into(&mut tb, shot_count() as u64), 0x6B756D, crate::fb::AA_T);
        let hint = "N/P cycle   W set wallpaper";
        fb.draw_aa(x as i32 + w as i32 - Framebuffer::aa_w(hint, crate::fb::AA_T) - 12, y as i32 + 7, hint, 0x6B756D, crate::fb::AA_T);

        let vx = x; let vy = y + 28; let vw = w; let vh = h.saturating_sub(28);
        fb.fill_rect(vx, vy, vw, vh, 0x0A0D0B);

        let mut namebuf = [0u8; 40];
        let name = self.filename(&mut namebuf);
        let img = vfs::vfs().read(name);
        match img.and_then(|d| Self::parse_ppm(d).map(|(iw, ih, off)| (d, iw, ih, off))) {
            Some((data, iw, ih, off)) if iw > 0 && ih > 0 => {
                // Fit the image into the view, aspect-preserved.
                let s = ((vw as usize * 1024 / iw).min(vh as usize * 1024 / ih)).max(1);
                let outw = (iw * s / 1024).max(1); let outh = (ih * s / 1024).max(1);
                let ox = vx + (vw - outw.min(vw as usize) as u32) / 2;
                let oy = vy + (vh - outh.min(vh as usize) as u32) / 2;
                for yy in 0..outh {
                    if oy as usize + yy >= (vy + vh) as usize { break; }
                    let sy = yy * ih / outh;
                    for xx in 0..outw {
                        let sx = xx * iw / outw;
                        let p = off + (sy * iw + sx) * 3;
                        if p + 2 < data.len() {
                            let rgb = ((data[p] as u32) << 16) | ((data[p+1] as u32) << 8) | data[p+2] as u32;
                            fb.set_pixel(ox + xx as u32, oy + yy as u32, rgb);
                        }
                    }
                }
            }
            _ => {
                if shot_count() == 0 {
                    fb.draw_aa(vx as i32 + 16, vy as i32 + 18, "No images. Use the Screenshot tool", 0xA8B0A6, crate::fb::AA_S);
                    fb.draw_aa(vx as i32 + 16, vy as i32 + 40, "or right-click the desktop -> Take Screenshot.", 0xA8B0A6, crate::fb::AA_S);
                } else {
                    fb.draw_aa(vx as i32 + 16, vy as i32 + 18, "Image not found.", 0xEF7575, crate::fb::AA_S);
                }
            }
        }
        self.dirty = false;
    }

    fn on_key(&mut self, key: u8) {
        use crate::ansi::Key as AK;
        match self.ansi.feed(key) {
            AK::Char(b'n') | AK::Char(b'N') | AK::Right => {
                if self.cur < shot_count() { self.cur += 1; self.dirty = true; }
            }
            AK::Char(b'p') | AK::Char(b'P') | AK::Left => {
                if self.cur > 1 { self.cur -= 1; self.dirty = true; }
            }
            AK::Char(b'w') | AK::Char(b'W') => {
                // Set the current image as the desktop wallpaper: copy it to
                // VFS "wallpaper.ppm" and signal the desktop (same process).
                let mut namebuf = [0u8; 40];
                let name = self.filename(&mut namebuf);
                let copy = vfs::vfs().read(name).map(|d| d.to_vec());
                if let Some(c) = copy {
                    vfs::vfs().write("wallpaper.ppm", &c);
                    unsafe { crate::WALLPAPER_SET_REQUEST = true; }
                }
            }
            _ => {}
        }
    }

    fn title(&self) -> &str { "Image Viewer" }
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
        fb.fill_rect(x, y, w, 26, 0x14281E);
        fb.draw_aa(x as i32 + 10, y as i32 + 4, "SNAKE", 0x4ADE80, crate::fb::AA_S);
        let mut sbuf = [0u8; 24];
        let mut px = x as i32 + 84;
        px += fb.draw_aa(px, y as i32 + 6, "score ", 0x9CA3AF, crate::fb::AA_T);
        fb.draw_aa(px, y as i32 + 6, u64_into(&mut sbuf, self.score as u64), 0xF5F5F7, crate::fb::AA_T);

        // Board area below header.
        let board_y = y + 28;
        let board_h = h.saturating_sub(28);
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
            let by = oy + gh / 2 - 22;
            fb.fill_rect(ox, by, gw, 46, 0x000000);
            fb.draw_aa(ox as i32 + 10, by as i32 + 2, "GAME OVER", 0xEF4444, crate::fb::AA_S);
            fb.draw_aa(ox as i32 + 10, by as i32 + 24, "press SPACE to restart", 0xF5F5F7, crate::fb::AA_T);
        } else if !self.started {
            let by = oy + gh / 2 - 11;
            fb.fill_rect(ox, by, gw, 22, 0x000000);
            fb.draw_aa(ox as i32 + 10, by as i32 + 3, "arrow keys / WASD to start", 0x86EFAC, crate::fb::AA_T);
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
        // Header: mines remaining + status (header height stays 24 — the mouse
        // hit-test in on_mouse keys off the same 24px board offset).
        fb.fill_rect(x, y, w, 24, 0x1B2230);
        fb.draw_aa(x as i32 + 8, y as i32 + 3, "MINES", 0xFCD34D, crate::fb::AA_S);
        let mut sbuf = [0u8; 24];
        let remaining = MS_MINES.saturating_sub(self.flags) as u64;
        fb.draw_aa(x as i32 + 74, y as i32 + 4, u64_into(&mut sbuf, remaining), 0xF5F5F7, crate::fb::AA_T);
        if self.dead { fb.draw_aa(x as i32 + 116, y as i32 + 4, "BOOM!", 0xEF4444, crate::fb::AA_T); }
        else if self.won { fb.draw_aa(x as i32 + 116, y as i32 + 4, "YOU WIN!", 0x4ADE80, crate::fb::AA_T); }

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
        fb.fill_rect(x, y, w, 20, 0x101418);
        fb.draw_aa(x as i32 + 6, y as i32 + 2, "DOOM (pure-Rust raycaster)", 0xEF4444, crate::fb::AA_T);
        let mut kb = [0u8; 24];
        let ks = u64_into(&mut kb, self.kills as u64);
        let kw = Framebuffer::aa_w("kills ", crate::fb::AA_T) + Framebuffer::aa_w(ks, crate::fb::AA_T);
        let mut px = x as i32 + w as i32 - kw - 10;
        px += fb.draw_aa(px, y as i32 + 2, "kills ", 0x9CA3AF, crate::fb::AA_T);
        fb.draw_aa(px, y as i32 + 2, ks, 0xF5F5F7, crate::fb::AA_T);

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

// ─────────────────────────────────────────────────────────────────────────────
// WadDoom — DOOM rendered from the real doom1.wad (E1M1 geometry, PLAYPAL
// palette). Reads the WAD from the kernel initrd via sys_initrd_read (#29).
// Falls back gracefully if doom1.wad is not in the initrd.
// ─────────────────────────────────────────────────────────────────────────────

fn fcos(x: f32) -> f32 { libm::cosf(x) }
fn fsin(x: f32) -> f32 { libm::sinf(x) }
fn wad_fabs(x: f32) -> f32 { libm::fabsf(x) }
const FRAC_PI_2: f32 = 1.5707963268f32;

/// sys_initrd_read(path, path_len, out_ptr|(out_len<<32)) → bytes or u64::MAX
unsafe fn sys_initrd_read(path: &[u8], buf: &mut [u8]) -> usize {
    let packed: u64 = (buf.as_mut_ptr() as u64 & 0xFFFF_FFFF)
                    | ((buf.len() as u64) << 32);
    let n: u64;
    core::arch::asm!(
        "syscall",
        inout("rax") 29u64 => n,
        in("rdi") path.as_ptr() as u64,
        in("rsi") path.len() as u64,
        in("rdx") packed,
        out("rcx") _, out("r11") _,
        options(nostack),
    );
    if n == u64::MAX { 0 } else { n as usize }
}

// ── WAD structures ────────────────────────────────────────────────────────────
#[repr(C, packed)] #[derive(Copy, Clone)]
struct WadEntry { ofs: u32, size: u32, name: [u8; 8] }

fn lump_name_eq(a: &[u8; 8], b: &[u8]) -> bool {
    let n = b.len().min(8);
    for i in 0..8 {
        let ac = if i < 8 { a[i].to_ascii_uppercase() } else { 0 };
        let bc = if i < n { b[i].to_ascii_uppercase() } else { 0 };
        if ac != bc { return false; }
    }
    true
}

fn read_i16_le(b: &[u8], i: usize) -> i16 {
    i16::from_le_bytes([b[i], b[i+1]])
}
fn read_u16_le(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i+1]])
}
fn read_u32_le(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i+1], b[i+2], b[i+3]])
}

// ── Raycaster on real DOOM linedefs ──────────────────────────────────────────
const WAD_DOOM_W: usize = 320;
const WAD_DOOM_H: usize = 200;

pub struct WadDoom {
    // Map data loaded from WAD
    verts:   Vec<(f32, f32)>,  // VERTEXES
    walls:   Vec<WadWall>,     // LINEDEFS (solid ones only)
    // PLAYPAL: 14 palettes × 256 colors × 3 bytes. We only use palette 0.
    pal:     Vec<u32>,         // 256 RGB values
    // Player state
    px: f32, py: f32, angle: f32,
    // Input
    fwd: i8, rot: i8, strafe: i8,
    // Render framebuffer — sized to the actual window at first render.
    pixels:  Vec<u32>,
    rw: usize, rh: usize,      // current render dimensions
    loaded:  bool,
    pub dirty: bool,
    pub wants_close: bool,
    ansi: crate::ansi::AnsiParser,
}

#[derive(Clone)]
struct WadWall {
    x1: f32, y1: f32,
    x2: f32, y2: f32,
    color: u32,  // wall shading color from sector light + texture class
}

impl WadDoom {
    pub fn new(seed: u64) -> Self {
        let _ = seed;
        let mut d = WadDoom {
            verts: Vec::new(), walls: Vec::new(), pal: Vec::new(),
            px: 0.0, py: 0.0, angle: 0.0,
            fwd: 0, rot: 0, strafe: 0,
            pixels: Vec::new(), rw: 0, rh: 0,
            loaded: false,
            dirty: true, wants_close: false,
            ansi: crate::ansi::AnsiParser::new(),
        };
        d.load_wad();
        d
    }

    pub fn loaded(&self) -> bool { self.loaded }

    fn load_wad(&mut self) {
        // Allocate WAD buffer (doom1.wad is ~4.5 MiB)
        const WAD_MAX: usize = 5 * 1024 * 1024;
        let mut wad = alloc::vec![0u8; WAD_MAX];
        let n = unsafe { sys_initrd_read(b"doom1.wad", &mut wad) };
        if n < 12 { return; }  // not found or truncated
        wad.truncate(n);

        // WAD header
        if &wad[0..4] != b"IWAD" { return; }
        let numlumps = read_u32_le(&wad, 4) as usize;
        let dirofs   = read_u32_le(&wad, 8) as usize;
        if dirofs + numlumps * 16 > n { return; }

        // Build lump index
        let dir_bytes = &wad[dirofs..dirofs + numlumps * 16];
        let lumps: Vec<(u32, u32, [u8;8])> = (0..numlumps).map(|i| {
            let b = &dir_bytes[i*16..i*16+16];
            let mut name = [0u8; 8];
            name.copy_from_slice(&b[8..16]);
            (read_u32_le(b, 0), read_u32_le(b, 4), name)
        }).collect();

        // Load PLAYPAL (first palette)
        for (ofs, size, name) in &lumps {
            if lump_name_eq(name, b"PLAYPAL") && *size >= 768
                && (*ofs as usize).saturating_add(768) <= wad.len() {
                let pal_bytes = &wad[*ofs as usize..*ofs as usize + 768];
                self.pal = (0..256).map(|i| {
                    let r = pal_bytes[i*3] as u32;
                    let g = pal_bytes[i*3+1] as u32;
                    let b = pal_bytes[i*3+2] as u32;
                    (r << 16) | (g << 8) | b
                }).collect();
                break;
            }
        }
        if self.pal.is_empty() {
            // Fallback grey palette
            self.pal = (0..256).map(|i| { let v = i as u32; (v<<16)|(v<<8)|v }).collect();
        }

        // Find E1M1 marker and load map lumps
        let e1m1_idx = lumps.iter().position(|(_, _, n)| lump_name_eq(n, b"E1M1"));
        let e1m1_idx = match e1m1_idx { Some(i) => i, None => return };

        // After E1M1 marker: THINGS(+1) LINEDEFS(+2) SIDEDEFS(+3) VERTEXES(+4)
        // SEGS(+5) SSECTORS(+6) NODES(+7) SECTORS(+8) REJECT(+9) BLOCKMAP(+10)
        let get_lump = |name: &[u8]| -> Option<(usize, usize)> {
            for i in e1m1_idx+1..e1m1_idx+15 {
                if i >= lumps.len() { break; }
                if lump_name_eq(&lumps[i].2, name) {
                    let ofs = lumps[i].0 as usize; let size = lumps[i].1 as usize;
                    // Defensive: a corrupt WAD could point a lump past the buffer,
                    // which would panic the later &wad[ofs..ofs+size] slices.
                    if ofs.checked_add(size).map_or(true, |end| end > n) { return None; }
                    return Some((ofs, size));
                }
            }
            None
        };

        // Load VERTEXES (each 4 bytes: i16 x, i16 y)
        if let Some((ofs, size)) = get_lump(b"VERTEXES") {
            let cnt = size / 4;
            let vb = &wad[ofs..ofs + size];
            self.verts = (0..cnt).map(|i| {
                let x = read_i16_le(vb, i*4) as f32;
                let y = read_i16_le(vb, i*4+2) as f32;
                (x, y)  // DOOM Y is flipped vs screen Y
            }).collect();
        }

        // Load SECTORS (for light level and floor heights)
        let mut sector_light: Vec<u8> = Vec::new();
        if let Some((ofs, size)) = get_lump(b"SECTORS") {
            let cnt = size / 26;
            let sb = &wad[ofs..ofs+size];
            sector_light = (0..cnt).map(|i| sb[i*26+20]).collect(); // light at offset 20
        }

        // Load SIDEDEFS (each 30 bytes; we want sector reference)
        let mut sidedef_sector: Vec<u16> = Vec::new();
        if let Some((ofs, size)) = get_lump(b"SIDEDEFS") {
            let cnt = size / 30;
            let sb = &wad[ofs..ofs+size];
            sidedef_sector = (0..cnt).map(|i| read_u16_le(sb, i*30+28)).collect();
        }

        // Load LINEDEFS (each 14 bytes: v1, v2, flags, special, tag, right, left)
        if let Some((ofs, size)) = get_lump(b"LINEDEFS") {
            let cnt = size / 14;
            let lb = &wad[ofs..ofs+size];
            for i in 0..cnt {
                let v1  = read_u16_le(lb, i*14)     as usize;
                let v2  = read_u16_le(lb, i*14+2)   as usize;
                let right_sd = read_u16_le(lb, i*14+10) as usize;
                if v1 >= self.verts.len() || v2 >= self.verts.len() { continue; }
                let (x1, y1) = self.verts[v1];
                let (x2, y2) = self.verts[v2];
                // Get sector light
                let light = if right_sd < sidedef_sector.len() {
                    let sec = sidedef_sector[right_sd] as usize;
                    if sec < sector_light.len() { sector_light[sec] } else { 160 }
                } else { 160 };
                // Map light 0-255 to a grey-brown color in DOOM palette feel
                let lf = (light as u32).min(255);
                // Stone-grey walls with slight warm tint
                let r = (lf * 180 / 255).min(255);
                let g = (lf * 160 / 255).min(255);
                let b = (lf * 140 / 255).min(255);
                let color = (r << 16) | (g << 8) | b;
                self.walls.push(WadWall { x1, y1: -y1, x2, y2: -y2, color });
            }
        }

        // Find player 1 start (THING type 1)
        if let Some((ofs, size)) = get_lump(b"THINGS") {
            let cnt = size / 10;
            let tb = &wad[ofs..ofs+size];
            for i in 0..cnt {
                let ty = read_u16_le(tb, i*10+6);
                if ty == 1 {
                    self.px    = read_i16_le(tb, i*10) as f32;
                    self.py    = -(read_i16_le(tb, i*10+2) as f32);
                    // We flip the world Y axis (py and wall Y are negated), so the
                    // facing angle must negate too — otherwise the player looks the
                    // opposite way (at the E1M1 start that's straight into the wall
                    // behind them, which fills the view with one flat color).
                    let ang_deg = read_u16_le(tb, i*10+4) as f32;
                    self.angle = -ang_deg * (3.14159265358979f32 / 180.0f32);
                    break;
                }
            }
        }

        self.loaded = true;
    }

    fn render_frame(&mut self) {
        if !self.loaded || self.rw == 0 || self.rh == 0 { return; }
        let w = self.rw as i32;
        let h = self.rh as i32;
        let hh = h / 2;
        // Sky gradient
        for row in 0..hh as usize {
            let sky_t = row * 0x0D / (hh as usize).max(1);
            let color = ((sky_t as u32 + 0x10) << 8) | (sky_t as u32 + 0x18);
            for col in 0..self.rw { self.pixels[row * self.rw + col] = color; }
        }
        // Floor
        for row in hh as usize..self.rh {
            for col in 0..self.rw { self.pixels[row * self.rw + col] = 0x281E14; }
        }

        // DDA raycast against DOOM linedefs
        let fov: f32 = 1.15191731f32;
        for col in 0..self.rw {
            let ray_ang = self.angle + fov * (col as f32 / self.rw as f32 - 0.5);
            let rdx = fcos(ray_ang);
            let rdy = fsin(ray_ang);

            let mut best_t = f32::MAX;
            let mut best_col = 0u32;

            for wall in &self.walls {
                let wx = wall.x2 - wall.x1;
                let wy = wall.y2 - wall.y1;
                let denom = rdx * wy - rdy * wx;
                if wad_fabs(denom) < 0.001 { continue; }
                let tx = wall.x1 - self.px;
                let ty = wall.y1 - self.py;
                let t  = (tx * wy - ty * wx) / denom;
                let s  = (tx * rdy - ty * rdx) / denom;
                if t > 0.1 && s >= 0.0 && s <= 1.0 && t < best_t {
                    best_t = t;
                    let fog = 1.0 - (best_t / 1400.0).min(1.0);
                    let r = (((wall.color >> 16) & 0xFF) as f32 * fog) as u32;
                    let g = (((wall.color >>  8) & 0xFF) as f32 * fog) as u32;
                    let b = ((wall.color & 0xFF) as f32 * fog) as u32;
                    best_col = (r << 16) | (g << 8) | b;
                }
            }

            if best_t < f32::MAX {
                let perp_dist = best_t * fcos(ray_ang - self.angle);
                let wall_h = ((h as f32 * 200.0) / perp_dist.max(1.0)) as i32;
                let top = (hh - wall_h / 2).max(0);
                let bot = (hh + wall_h / 2).min(h);
                for row in top..bot {
                    let v = (row - top) as f32 / (bot - top).max(1) as f32;
                    let shade = if v < 0.5 { 1.1 } else { 0.85 };
                    let r = (((best_col >> 16) & 0xFF) as f32 * shade).min(255.0) as u32;
                    let g = (((best_col >>  8) & 0xFF) as f32 * shade).min(255.0) as u32;
                    let b = ((best_col & 0xFF) as f32 * shade).min(255.0) as u32;
                    self.pixels[row as usize * self.rw + col] = (r<<16)|(g<<8)|b;
                }
            }
        }
    }
}

impl App for WadDoom {
    fn tick(&mut self, _ticks: u64) -> bool {
        if !self.loaded { return false; }
        let speed = 24.0f32;
        let rot_speed = 0.06f32;
        if self.rot  != 0 { self.angle += self.rot as f32 * rot_speed; }
        if self.fwd  != 0 {
            self.px += fcos(self.angle) * self.fwd as f32 * speed;
            self.py += fsin(self.angle) * self.fwd as f32 * speed;
        }
        if self.strafe != 0 {
            let sa = self.angle + FRAC_PI_2;
            self.px += fcos(sa) * self.strafe as f32 * speed;
            self.py += fsin(sa) * self.strafe as f32 * speed;
        }
        if self.fwd != 0 || self.rot != 0 || self.strafe != 0 {
            self.render_frame();
            self.dirty = true;
            return true;
        }
        false
    }

    fn render(&mut self, fb: &mut Framebuffer, x: u32, y: u32, w: u32, h: u32) {
        if !self.loaded {
            fb.fill_rect(x, y, w, h, 0x0A0A0A);
            fb.draw_aa(x as i32 + 8, y as i32 + 8,
                "doom1.wad not found in initrd", 0xEF4444, crate::fb::AA_T);
            self.dirty = false; return;
        }
        let sw = w as usize;
        let sh = h as usize;
        let game_h = (sh * 84 / 100).max(1);
        let bar_h  = sh - game_h;

        // Resize pixel buffer if window changed — then re-render at new native size.
        if self.rw != sw || self.rh != game_h {
            self.rw = sw; self.rh = game_h;
            self.pixels.resize(sw * game_h, 0);
            self.dirty = true;
        }
        if self.dirty { self.render_frame(); }

        // Direct blit — no scale needed, pixels are already window-resolution.
        for dy in 0..game_h {
            for dx in 0..sw {
                fb.set_pixel(x + dx as u32, y + dy as u32, self.pixels[dy * sw + dx]);
            }
        }

        // Status bar — label row at +4, value row at +18 (two distinct lines).
        let bar_y = y + game_h as u32;
        fb.fill_rect(x, bar_y, w, bar_h as u32, 0x181008);
        // Health
        fb.draw_aa(x as i32 + 10, bar_y as i32 + 4,  "HLTH", 0x888880, crate::fb::AA_T);
        fb.draw_aa(x as i32 + 10, bar_y as i32 + 18, "100%", 0xEF7575, crate::fb::AA_T);
        // Ammo (right-aligned)
        fb.draw_aa(x as i32 + w as i32 - 56, bar_y as i32 + 4,  "AMMO", 0x888880, crate::fb::AA_T);
        fb.draw_aa(x as i32 + w as i32 - 46, bar_y as i32 + 18, "50",   0xF5C451, crate::fb::AA_T);
        // Title centered
        fb.draw_aa(x as i32 + w as i32 / 2 - 30, bar_y as i32 + 11,
            "E1M1 DOOM", 0xEF4444, crate::fb::AA_T);

        self.dirty = false;
    }

    fn on_key(&mut self, key: u8) {
        use crate::ansi::Key as AK;
        match self.ansi.feed(key) {
            AK::Char(b'w') | AK::Up    => self.fwd    = 1,
            AK::Char(b's') | AK::Down  => self.fwd    = -1,
            AK::Char(b'a')             => self.rot    = -1,
            AK::Char(b'd')             => self.rot    = 1,
            AK::Left                   => self.rot    = -1,
            AK::Right                  => self.rot    = 1,
            AK::Char(b'q')             => self.strafe = -1,
            AK::Char(b'e')             => self.strafe = 1,
            AK::Char(0x1B)             => { self.wants_close = true; }
            AK::Char(ch) => {
                match ch {
                    b'w' => self.fwd = 0, b's' => self.fwd = 0,
                    b'a' | b'd' => self.rot = 0,
                    _ => { self.fwd = 0; self.rot = 0; self.strafe = 0; }
                }
            }
            _ => { self.fwd = 0; self.rot = 0; self.strafe = 0; }
        }
    }

    fn wants_close(&self) -> bool { self.wants_close }
    fn title(&self) -> &str { "DOOM" }
}

// ── Notes ────────────────────────────────────────────────────────────────────
// Sticky-note app: keyboard text input, VFS save (Ctrl+S), VFS load on open.

const NOTES_CAP: usize = 4096;
const NOTES_FILE: &str = "notes.txt";

pub struct Notes {
    buf: [u8; NOTES_CAP],
    len: usize,
    cursor: usize,
    scroll: usize,       // first visible line index
    modified: bool,
    pub dirty: bool,
    pub wants_close: bool,
    ansi: crate::ansi::AnsiParser,
}

impl Notes {
    pub fn new() -> Self {
        let mut n = Notes {
            buf: [0u8; NOTES_CAP],
            len: 0,
            cursor: 0,
            scroll: 0,
            modified: false,
            dirty: true,
            wants_close: false,
            ansi: crate::ansi::AnsiParser::new(),
        };
        // Try to load existing notes from VFS.
        if let Some(data) = vfs::vfs().read(NOTES_FILE) {
            let copy = data.len().min(NOTES_CAP - 1);
            n.buf[..copy].copy_from_slice(&data[..copy]);
            n.len = copy;
            n.cursor = copy;
        }
        n
    }

    fn save(&mut self) {
        vfs::vfs().write(NOTES_FILE, &self.buf[..self.len]);
        self.modified = false;
        self.dirty = true;
    }

    fn current_line(&self) -> usize {
        self.buf[..self.cursor].iter().filter(|&&b| b == b'\n').count()
    }

    fn line_count(&self) -> usize {
        self.buf[..self.len].iter().filter(|&&b| b == b'\n').count() + 1
    }

    // Returns (start, end) byte range of line `idx` in buf.
    fn line_range(&self, idx: usize) -> (usize, usize) {
        let mut line = 0;
        let mut start = 0;
        for (i, &b) in self.buf[..self.len].iter().enumerate() {
            if line == idx { start = i; break; }
            if b == b'\n' { line += 1; }
        }
        if line < idx { return (self.len, self.len); } // past end
        let end = self.buf[start..self.len].iter().position(|&b| b == b'\n')
            .map(|p| start + p).unwrap_or(self.len);
        (start, end)
    }
}

impl App for Notes {
    fn tick(&mut self, _ticks: u64) -> bool { false }

    fn render(&mut self, fb: &mut Framebuffer, x: u32, y: u32, w: u32, h: u32) {
        let bg = 0x1A1E26u32;
        fb.fill_rect(x, y, w, h, bg);

        // Status bar at top (AA chrome; the editor body stays monospace so the
        // per-character cursor block stays column-aligned).
        let sb_h = 22u32;
        fb.fill_rect(x, y, w, sb_h, 0x1B2230);
        let lines = self.line_count();
        let cur_line = self.current_line() + 1;
        let mut lbuf = [0u8; 24]; let mut l2 = [0u8; 24];
        let s = fmt_u64_into(&mut lbuf, cur_line as u64);
        let s2 = fmt_u64_into(&mut l2, lines as u64);
        let mut px = x as i32 + 8;
        px += fb.draw_aa(px, y as i32 + 4, s, 0x8A9AB0, crate::fb::AA_T) + 2;
        px += fb.draw_aa(px, y as i32 + 4, "/", 0x4A5568, crate::fb::AA_T) + 2;
        px += fb.draw_aa(px, y as i32 + 4, s2, 0x8A9AB0, crate::fb::AA_T) + 10;
        if self.modified { fb.draw_aa(px, y as i32 + 4, "[modified]", 0xF5C451, crate::fb::AA_T); }
        let save = "Ctrl+S save";
        fb.draw_aa(x as i32 + w as i32 - Framebuffer::aa_w(save, crate::fb::AA_T) - 10, y as i32 + 4, save, 0x6B7686, crate::fb::AA_T);

        // Text area.
        let text_y = y + sb_h + 4;
        let line_h = 10u32;
        let visible_lines = ((h - sb_h - 8) / line_h) as usize;

        // Clamp scroll so cursor line is visible.
        let cur_line0 = self.current_line();
        if cur_line0 < self.scroll { self.scroll = cur_line0; }
        if cur_line0 >= self.scroll + visible_lines { self.scroll = cur_line0 + 1 - visible_lines; }

        // Render each visible line.
        let mut line_idx = 0usize;
        let mut byte_pos = 0usize;
        while byte_pos <= self.len && line_idx < self.scroll + visible_lines {
            let line_end = self.buf[byte_pos..self.len].iter().position(|&b| b == b'\n')
                .map(|p| byte_pos + p).unwrap_or(self.len);
            if line_idx >= self.scroll {
                let vis_row = (line_idx - self.scroll) as u32;
                let ly = text_y + vis_row * line_h;
                let line_bytes = &self.buf[byte_pos..line_end];
                let safe_len = line_bytes.len().min(128);
                // Render character by character; cursor is a block.
                for (ci, &ch) in line_bytes[..safe_len].iter().enumerate() {
                    let cx = x + 6 + ci as u32 * 7;
                    if cx + 7 > x + w { break; }
                    let is_cursor = byte_pos + ci == self.cursor;
                    let char_bg = if is_cursor { 0x4A9EFF } else { bg };
                    let char_fg = if is_cursor { 0x000000 } else { 0xCDD6E0 };
                    if is_cursor { fb.fill_rect(cx, ly, 7, 9, 0x4A9EFF); }
                    if ch >= 0x20 { fb.draw_char(cx, ly, ch as char, char_fg, char_bg); }
                }
                // Cursor at end of line.
                if self.cursor == line_end && cur_line0 == line_idx {
                    let cx = x + 6 + safe_len as u32 * 7;
                    fb.fill_rect(cx, ly, 3, 9, 0x4A9EFF);
                }
            }
            if byte_pos == self.len { break; }
            byte_pos = line_end + 1;
            line_idx += 1;
        }

        self.dirty = false;
    }

    fn on_key(&mut self, key: u8) {
        use crate::ansi::Key as AK;
        match self.ansi.feed(key) {
            AK::Char(0x13) => { self.save(); return; }        // Ctrl+S
            AK::Char(0x1B) => { self.wants_close = true; return; }
            AK::Char(0x08) => {               // Backspace
                if self.cursor > 0 {
                    self.buf.copy_within(self.cursor..self.len, self.cursor - 1);
                    self.len -= 1;
                    self.cursor -= 1;
                    self.modified = true;
                    self.dirty = true;
                }
            }
            AK::Char(0x7F) => {                               // Delete
                if self.cursor < self.len {
                    self.buf.copy_within(self.cursor + 1..self.len, self.cursor);
                    self.len -= 1;
                    self.modified = true;
                    self.dirty = true;
                }
            }
            AK::Left => {
                if self.cursor > 0 { self.cursor -= 1; self.dirty = true; }
            }
            AK::Right => {
                if self.cursor < self.len { self.cursor += 1; self.dirty = true; }
            }
            AK::Up => {
                let cl = self.current_line();
                if cl > 0 {
                    let (ls, _) = self.line_range(cl);
                    let col = self.cursor - ls;
                    let (ps, pe) = self.line_range(cl - 1);
                    self.cursor = (ps + col).min(pe);
                    self.dirty = true;
                }
            }
            AK::Down => {
                let cl = self.current_line();
                let (ls, _) = self.line_range(cl);
                let col = self.cursor - ls;
                let (ns, ne) = self.line_range(cl + 1);
                if ns < self.len || ns == ne {
                    self.cursor = (ns + col).min(ne);
                    self.dirty = true;
                }
            }
            AK::Char(b'\r') | AK::Char(b'\n') => {
                if self.len < NOTES_CAP - 1 {
                    self.buf.copy_within(self.cursor..self.len, self.cursor + 1);
                    self.buf[self.cursor] = b'\n';
                    self.len += 1;
                    self.cursor += 1;
                    self.modified = true;
                    self.dirty = true;
                }
            }
            AK::Char(ch) if ch >= 0x20 && ch < 0x7F => {
                if self.len < NOTES_CAP - 1 {
                    self.buf.copy_within(self.cursor..self.len, self.cursor + 1);
                    self.buf[self.cursor] = ch;
                    self.len += 1;
                    self.cursor += 1;
                    self.modified = true;
                    self.dirty = true;
                }
            }
            _ => {}
        }
    }

    fn wants_close(&self) -> bool { self.wants_close }
    fn title(&self) -> &str { "Notes" }
}

// ── RustyPhone ───────────────────────────────────────────────────────────────
// Zabih's vision: Rusty Penguin runs on any device — phone, tablet, laptop, PC.
// This app: SIP soft-phone dialer + phone-number verification via SMS.
// The UI adapts to the window width: ≤320px = phone portrait, wider = desktop.

#[derive(Clone, Copy, PartialEq)]
enum PhoneTab { Dialer, Recent, Messages, Account }

#[derive(Clone, Copy)]
struct Msg { text: [u8; 96], len: usize, sent: bool }
impl Msg {
    const EMPTY: Msg = Msg { text: [0u8; 96], len: 0, sent: false };
    fn new(s: &[u8], sent: bool) -> Msg {
        let mut m = Msg::EMPTY; let n = s.len().min(96);
        m.text[..n].copy_from_slice(&s[..n]); m.len = n; m.sent = sent; m
    }
    fn as_str(&self) -> &str { core::str::from_utf8(&self.text[..self.len]).unwrap_or("") }
}

#[derive(Clone, Copy, PartialEq)]
enum CallState { Idle, Dialing, Ringing, Connected, Ended }

#[derive(Clone, Copy, PartialEq)]
enum VerifyState { Idle, EnterNumber, WaitingCode, EnterCode, Verified, Failed }

pub struct RustyPhone {
    tab: PhoneTab,
    // Dialer
    number: [u8; 32],
    number_len: usize,
    call_state: CallState,
    call_secs: u64,
    call_start: u64,
    muted: bool,
    speaker: bool,
    // Recent calls (ring buffer of 8)
    recent: [[u8; 20]; 8],
    recent_len: [usize; 8],
    recent_count: usize,
    // Account / SIP
    sip_server: [u8; 64],
    sip_server_len: usize,
    sip_user: [u8; 32],
    sip_user_len: usize,
    sip_registered: bool,
    // Phone number verification
    verify_state: VerifyState,
    verify_number: [u8; 16],
    verify_number_len: usize,
    verify_code_input: [u8; 8],
    verify_code_len: usize,
    verify_code_sent: [u8; 6],  // the code we sent
    focus_field: u8,            // 0 = number field, 1 = code field, 2 = sip fields
    last_tick: u64,
    // Messages (SMS / RCS chat)
    msgs: [Msg; 24],
    msg_count: usize,
    compose: [u8; 96],
    compose_len: usize,
    is_rcs: bool,               // true = RCS rich chat, false = plain SMS
    peer_typing: bool,
    reply_at: u64,              // tick to deliver the canned reply (0 = none)
    pub dirty: bool,
    pub wants_close: bool,
    ansi: crate::ansi::AnsiParser,
}

impl RustyPhone {
    pub fn new() -> Self {
        let mut r = RustyPhone {
            tab: PhoneTab::Dialer,
            number: [0u8; 32], number_len: 0,
            call_state: CallState::Idle,
            call_secs: 0, call_start: 0,
            muted: false, speaker: false,
            recent: [[0u8; 20]; 8], recent_len: [0usize; 8], recent_count: 0,
            sip_server: [0u8; 64], sip_server_len: 0,
            sip_user: [0u8; 32], sip_user_len: 0,
            sip_registered: false,
            verify_state: VerifyState::Idle,
            verify_number: [0u8; 16], verify_number_len: 0,
            verify_code_input: [0u8; 8], verify_code_len: 0,
            verify_code_sent: [0u8; 6],
            focus_field: 0,
            last_tick: 0,
            msgs: [Msg::EMPTY; 24], msg_count: 0,
            compose: [0u8; 96], compose_len: 0,
            is_rcs: true, peer_typing: false, reply_at: 0,
            dirty: true, wants_close: false,
            ansi: crate::ansi::AnsiParser::new(),
        };
        // Default SIP server hint.
        let srv = b"sip.linphone.org";
        r.sip_server[..srv.len()].copy_from_slice(srv);
        r.sip_server_len = srv.len();
        // Seed a short demo thread so Messages isn't empty on first open.
        r.msgs[0] = Msg::new(b"hey! is rusty penguin really running this chat?", false);
        r.msgs[1] = Msg::new(b"yep - native, no Linux underneath", true);
        r.msgs[2] = Msg::new(b"insane. SMS or RCS?", false);
        r.msg_count = 3;
        r
    }

    fn push_digit(&mut self, d: u8) {
        if self.number_len < 31 {
            self.number[self.number_len] = d;
            self.number_len += 1;
            self.dirty = true;
        }
    }

    fn dial(&mut self, ticks: u64) {
        if self.number_len == 0 { return; }
        self.call_state = CallState::Dialing;
        self.call_start = ticks;
        // Push to recent.
        let slot = self.recent_count % 8;
        let n = self.number_len.min(20);
        self.recent[slot][..n].copy_from_slice(&self.number[..n]);
        self.recent_len[slot] = n;
        self.recent_count += 1;
        self.dirty = true;
    }

    fn end_call(&mut self) {
        self.call_state = CallState::Idle;
        self.dirty = true;
    }

    // Generate a pseudo-random 6-digit code from ticks.
    fn gen_code(ticks: u64) -> [u8; 6] {
        let mut v = ticks.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let mut code = [0u8; 6];
        for i in 0..6 {
            v = v.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            code[i] = b'0' + (v >> 58) as u8 % 10;
        }
        code
    }

    fn draw_dialer(&mut self, fb: &mut Framebuffer, x: u32, y: u32, w: u32, h: u32, portrait: bool) {
        let bg = 0x0D1117u32;
        let accent = 0x22C55Eu32;   // green call button
        let danger = 0xEF4444u32;   // red end button
        let key_bg = 0x1C2128u32;
        let key_fg = 0xF0F0F0u32;

        // Number display
        let disp_h = if portrait { 64u32 } else { 48u32 };
        fb.fill_rect(x, y, w, disp_h, 0x0A0E14);
        let num_str = core::str::from_utf8(&self.number[..self.number_len]).unwrap_or("");
        let nw = Framebuffer::aa_w(num_str, crate::fb::AA_L);
        let nx = x as i32 + w as i32 - nw - 12;
        fb.draw_aa(nx, y as i32 + 10, num_str, 0xF8F8F2, crate::fb::AA_L);

        // Call state badge
        let (state_label, state_col) = match self.call_state {
            CallState::Idle      => ("", 0u32),
            CallState::Dialing   => ("Dialing…", 0xF5C451),
            CallState::Ringing   => ("Ringing…", 0x4A9EFF),
            CallState::Connected => ("Connected", accent),
            CallState::Ended     => ("Call ended", 0x8A938C),
        };
        if !state_label.is_empty() {
            fb.draw_aa(x as i32 + 8, (y + disp_h) as i32 - 17, state_label, state_col, crate::fb::AA_T);
        }
        if self.call_state == CallState::Connected {
            let mut tb = [0u8; 24];
            let mins = self.call_secs / 60;
            let secs = self.call_secs % 60;
            let ts = fmt_duration(&mut tb, mins, secs);
            let tw = Framebuffer::aa_w(ts, crate::fb::AA_T);
            fb.draw_aa(x as i32 + w as i32 - tw - 10, (y + disp_h) as i32 - 17, ts, 0x6FE18B, crate::fb::AA_T);
        }

        // Keypad — 3 columns × 4 rows + 1 action row
        let pad = if portrait { 6u32 } else { 4u32 };
        let grid_y = y + disp_h + pad;
        let key_rows = 4u32;
        let key_cols = 3u32;
        let avail_w = w.saturating_sub(pad * (key_cols + 1));
        let avail_h = h.saturating_sub(disp_h + pad * (key_rows + 3) + 40);
        let kw = avail_w / key_cols;
        let kh = (avail_h / (key_rows + 1)).min(if portrait { 56 } else { 42 });

        let keys = [
            b'1', b'2', b'3',
            b'4', b'5', b'6',
            b'7', b'8', b'9',
            b'*', b'0', b'#',
        ];
        let labels = ["1", "2", "3", "4", "5", "6", "7", "8", "9", "*", "0", "#"];
        let sub    = ["", "ABC", "DEF", "GHI", "JKL", "MNO", "PQRS", "TUV", "WXYZ", "", "+", ""];

        // Classic round-key dialer: each grid cell holds a circular key with the
        // big digit and the letter cluster beneath it. (Cells stay the hit-test
        // grid; we just draw circles inside them.)
        let _ = (key_bg, key_fg);
        for i in 0..12usize {
            let row = (i / 3) as i32;
            let col = (i % 3) as i32;
            let kx = x as i32 + pad as i32 + col * (kw as i32 + pad as i32);
            let ky = grid_y as i32 + row * (kh as i32 + pad as i32);
            let ccx = kx + kw as i32 / 2;
            let ccy = ky + kh as i32 / 2;
            let r = (kw as i32).min(kh as i32) / 2 - 1;
            // Round key: grey disc + a slightly lighter top catch for depth.
            fb.fill_circle(ccx, ccy, r, 0x33383F);
            fb.fill_circle(ccx, ccy - 1, r - 1, 0x3A4049);
            let has_sub = !sub[i].is_empty();
            // Big digit (sits a touch high when letters follow).
            let lw = Framebuffer::aa_w(labels[i], crate::fb::AA_L);
            let ny = if has_sub { ccy - 22 } else { ccy - Framebuffer::aa_line(crate::fb::AA_L) / 2 };
            fb.draw_aa(ccx - lw / 2, ny, labels[i], 0xF4F6FA, crate::fb::AA_L);
            // Letter cluster beneath.
            if has_sub {
                let sw = Framebuffer::aa_w(sub[i], crate::fb::AA_T);
                fb.draw_aa(ccx - sw / 2, ccy + 6, sub[i], 0x9AA3AE, crate::fb::AA_T);
            }
            let _ = keys[i];
        }

        // Action row: backspace (left), round green CALL (center), mute (right).
        let action_y = grid_y + key_rows * (kh + pad) + pad;
        let acy = action_y as i32 + kh as i32 / 2;
        // Backspace — plain glyph, no chrome.
        let bcx = x as i32 + pad as i32 + kw as i32 / 2;
        fb.draw_aa(bcx - Framebuffer::aa_w("<x", crate::fb::AA_S) / 2,
                   acy - Framebuffer::aa_line(crate::fb::AA_S) / 2, "<x", 0x8A93A0, crate::fb::AA_S);
        // Call / End — round button, green to dial, red to hang up.
        let ccx = x as i32 + pad as i32 + kw as i32 + pad as i32 + kw as i32 / 2;
        let cr = (kw as i32).min(kh as i32) / 2;
        let (call_bg, call_label) = if self.call_state == CallState::Idle || self.call_state == CallState::Ended {
            (accent, "CALL")
        } else {
            (danger, "END")
        };
        fb.fill_circle(ccx, acy, cr, call_bg);
        fb.fill_circle(ccx, acy - 1, cr - 1, call_bg.saturating_add(0x0A140A));
        let clw = Framebuffer::aa_w(call_label, crate::fb::AA_S);
        fb.draw_aa(ccx - clw / 2, acy - Framebuffer::aa_line(crate::fb::AA_S) / 2, call_label, 0xFFFFFF, crate::fb::AA_S);
        // Mute — plain glyph, tinted when active.
        let mcx = x as i32 + pad as i32 + (kw as i32 + pad as i32) * 2 + kw as i32 / 2;
        let ml = if self.muted { "unmute" } else { "mute" };
        let mcol = if self.muted { 0xEF7575u32 } else { 0x8A93A0 };
        fb.draw_aa(mcx - Framebuffer::aa_w(ml, crate::fb::AA_T) / 2,
                   acy - Framebuffer::aa_line(crate::fb::AA_T) / 2, ml, mcol, crate::fb::AA_T);
    }

    fn draw_recent(&self, fb: &mut Framebuffer, x: u32, y: u32, w: u32, _h: u32) {
        fb.draw_aa(x as i32 + 10, y as i32 + 8, "Recent Calls", 0xF0F0F0, crate::fb::AA_S);
        if self.recent_count == 0 {
            fb.draw_aa(x as i32 + 10, y as i32 + 34, "No calls yet.", 0x4A5568, crate::fb::AA_S);
            return;
        }
        let count = self.recent_count.min(8);
        for i in 0..count {
            let slot = (self.recent_count - 1 - i) % 8;
            let num = core::str::from_utf8(&self.recent[slot][..self.recent_len[slot]]).unwrap_or("?");
            let ry = y + 34 + i as u32 * 28;
            fb.fill_rounded_rect(x as i32 + 4, ry as i32, w as i32 - 8, 24, 6, 0x161B22);
            fb.draw_aa(x as i32 + 12, ry as i32 + 4, num, 0xCDD6E0, crate::fb::AA_S);
            fb.draw_aa(x as i32 + w as i32 - 60, ry as i32 + 4, "Recall", 0x4A9EFF, crate::fb::AA_T);
        }
    }

    fn draw_account(&mut self, fb: &mut Framebuffer, x: u32, y: u32, w: u32, _h: u32) {
        let bg = 0x0D1117u32;
        let xi = x as i32; let wi = w as i32;
        // ── Phone number verification section ────────────────────────────────
        fb.draw_aa(xi + 8, y as i32 + 6, "Link Phone Number", 0xF0F0F0, crate::fb::AA_S);
        fb.draw_aa(xi + 8, y as i32 + 26, "Verify your number via SMS to make real calls.", 0x6B7280, crate::fb::AA_T);

        match self.verify_state {
            VerifyState::Idle => {
                fb.fill_rounded_rect(xi + 8, y as i32 + 44, wi - 16, 26, 6, 0x161B22);
                fb.draw_aa(xi + 12, y as i32 + 48, "+  Enter your phone number", 0x4A5568, crate::fb::AA_S);
                fb.fill_rounded_rect(xi + 8, y as i32 + 78, 120, 24, 6, 0x1A3A2A);
                fb.draw_aa(xi + 18, y as i32 + 82, "Send Code", 0x22C55E, crate::fb::AA_S);
            }
            VerifyState::EnterNumber => {
                fb.fill_rounded_rect(xi + 8, y as i32 + 44, wi - 16, 26, 6, 0x1C2A1C);
                fb.fill_rect_s(xi + 8, y as i32 + 44, wi - 16, 1, 0x22C55E);
                let num = core::str::from_utf8(&self.verify_number[..self.verify_number_len]).unwrap_or("");
                let mut px = xi + 12;
                px += fb.draw_aa(px, y as i32 + 48, "+ ", 0x22C55E, crate::fb::AA_S);
                px += fb.draw_aa(px, y as i32 + 48, num, 0xF0F0F0, crate::fb::AA_S);
                fb.fill_rect_s(px, y as i32 + 48, 2, 14, 0x22C55E); // cursor
                fb.fill_rounded_rect(xi + 8, y as i32 + 80, 130, 24, 6, 0x1A3A2A);
                fb.draw_aa(xi + 18, y as i32 + 84, "Send Code ->", 0x22C55E, crate::fb::AA_S);
                fb.draw_aa(xi + 8, y as i32 + 110, "Enter full number incl. country code, e.g. +436...", 0x4A5568, crate::fb::AA_T);
            }
            VerifyState::WaitingCode => {
                fb.draw_aa(xi + 8, y as i32 + 44, "Sending SMS...", 0xF5C451, crate::fb::AA_S);
            }
            VerifyState::EnterCode => {
                fb.draw_aa(xi + 8, y as i32 + 44, "Code sent! Enter the 6-digit code:", 0x22C55E, crate::fb::AA_S);
                fb.fill_rounded_rect(xi + 8, y as i32 + 64, 200, 28, 6, 0x1C2A1C);
                fb.fill_rect_s(xi + 8, y as i32 + 64, 200, 1, 0x4A9EFF);
                let code = core::str::from_utf8(&self.verify_code_input[..self.verify_code_len]).unwrap_or("");
                let cw = fb.draw_aa(xi + 14, y as i32 + 68, code, 0xF0F0F0, crate::fb::AA_S);
                fb.fill_rect_s(xi + 14 + cw, y as i32 + 68, 2, 16, 0x4A9EFF);
                fb.fill_rounded_rect(xi + 8, y as i32 + 100, 100, 24, 6, 0x1A3040);
                fb.draw_aa(xi + 18, y as i32 + 104, "Verify ->", 0x4A9EFF, crate::fb::AA_S);
            }
            VerifyState::Verified => {
                fb.draw_aa(xi + 8, y as i32 + 44, "Phone number verified!", 0x22C55E, crate::fb::AA_S);
                let num = core::str::from_utf8(&self.verify_number[..self.verify_number_len]).unwrap_or("?");
                fb.draw_aa(xi + 8, y as i32 + 64, num, 0xF0F0F0, crate::fb::AA_S);
            }
            VerifyState::Failed => {
                fb.draw_aa(xi + 8, y as i32 + 44, "Verification failed. Wrong code?", 0xEF4444, crate::fb::AA_S);
                fb.fill_rounded_rect(xi + 8, y as i32 + 66, 90, 24, 6, 0x3A1A1A);
                fb.draw_aa(xi + 18, y as i32 + 70, "Try again", 0xEF4444, crate::fb::AA_S);
            }
        }

        // ── SIP configuration ────────────────────────────────────────────────
        let sip_y = y as i32 + 144;
        fb.fill_rect_s(xi, sip_y - 8, wi, 1, 0x252A35);
        fb.draw_aa(xi + 8, sip_y, "SIP Account", 0xF0F0F0, crate::fb::AA_S);
        let reg_col = if self.sip_registered { 0x22C55Eu32 } else { 0x6B7280 };
        let reg = if self.sip_registered { "Registered" } else { "Unregistered" };
        fb.draw_aa(xi + wi - Framebuffer::aa_w(reg, crate::fb::AA_T) - 8, sip_y + 2, reg, reg_col, crate::fb::AA_T);

        fb.draw_aa(xi + 8, sip_y + 22, "Server", 0x6B7280, crate::fb::AA_T);
        fb.fill_rounded_rect(xi + 8, sip_y + 38, wi - 16, 22, 5, 0x161B22);
        let srv = core::str::from_utf8(&self.sip_server[..self.sip_server_len]).unwrap_or("");
        let srv_col = if self.focus_field == 1 { 0xF0F0F0u32 } else { 0x8A9AB0 };
        fb.draw_aa(xi + 12, sip_y + 41, srv, srv_col, crate::fb::AA_S);

        fb.draw_aa(xi + 8, sip_y + 64, "Username", 0x6B7280, crate::fb::AA_T);
        fb.fill_rounded_rect(xi + 8, sip_y + 80, wi - 16, 22, 5, 0x161B22);
        let user = core::str::from_utf8(&self.sip_user[..self.sip_user_len]).unwrap_or("");
        let user_col = if self.focus_field == 2 { 0xF0F0F0u32 } else { 0x8A9AB0 };
        fb.draw_aa(xi + 12, sip_y + 83, user, user_col, crate::fb::AA_S);

        fb.fill_rounded_rect(xi + 8, sip_y + 110, 100, 24, 6, 0x1A3040);
        fb.draw_aa(xi + 18, sip_y + 114, "Register", 0x4A9EFF, crate::fb::AA_S);
    }

    fn send_message(&mut self, now: u64) {
        if self.compose_len == 0 || self.msg_count >= 24 { return; }
        let m = Msg::new(&self.compose[..self.compose_len], true);
        self.msgs[self.msg_count] = m; self.msg_count += 1;
        self.compose_len = 0;
        // Canned auto-reply ~2s later (RCS shows a typing indicator first).
        self.peer_typing = self.is_rcs;
        self.reply_at = now + 200;
        self.dirty = true;
    }

    fn draw_messages(&mut self, fb: &mut Framebuffer, x: u32, y: u32, w: u32, h: u32) {
        let xi = x as i32; let wi = w as i32;
        const RCS: u32 = 0x2D6CDF;   // RCS rich-chat blue
        const SMS: u32 = 0x22A45A;   // plain SMS green
        const RECV: u32 = 0x2A2F37;  // received grey
        fb.fill_rect(x, y, w, h, 0x0D1117);

        // Header: peer + protocol badge (tap badge to toggle SMS/RCS).
        let hdr_h = 28;
        fb.fill_rect(x, y, w, hdr_h, 0x161B22);
        fb.draw_aa(xi + 10, y as i32 + 5, "RFI-IRFOS", 0xF0F0F0, crate::fb::AA_S);
        let (badge, bcol) = if self.is_rcs { ("RCS - encrypted", RCS) } else { ("SMS", SMS) };
        let bw = Framebuffer::aa_w(badge, crate::fb::AA_T) + 16;
        let bx = xi + wi - bw - 8;
        fb.fill_rounded_rect(bx, y as i32 + 5, bw, 18, 9, bcol);
        fb.draw_aa(bx + 8, y as i32 + 6, badge, 0xFFFFFF, crate::fb::AA_T);

        // Composer at the bottom.
        let comp_h = 34;
        let comp_y = y as i32 + h as i32 - comp_h;
        fb.fill_rect(x, comp_y as u32, w, comp_h as u32, 0x12161D);
        let send_r = 13;
        let send_cx = xi + wi - 8 - send_r;
        fb.fill_rounded_rect(xi + 8, comp_y + 5, wi - 16 - 2 * send_r - 12, 24, 12, 0x1C222B);
        let ctext = core::str::from_utf8(&self.compose[..self.compose_len]).unwrap_or("");
        if self.compose_len == 0 {
            fb.draw_aa(xi + 16, comp_y + 9, "Text message", 0x5A6470, crate::fb::AA_S);
        } else {
            let cw = fb.draw_aa(xi + 16, comp_y + 9, ctext, 0xF0F0F0, crate::fb::AA_S);
            fb.fill_rect_s(xi + 16 + cw, comp_y + 9, 2, 16, bcol);
        }
        fb.fill_circle(send_cx, comp_y + comp_h / 2, send_r, if self.compose_len > 0 { bcol } else { 0x2A2F37 });
        fb.draw_aa(send_cx - Framebuffer::aa_w(">", crate::fb::AA_S) / 2, comp_y + comp_h / 2 - 9, ">", 0xFFFFFF, crate::fb::AA_S);

        // Message bubbles — show the most recent that fit, top→bottom.
        let area_top = y as i32 + hdr_h as i32 + 4;
        let area_bot = comp_y - 4;
        let row_h = 26;
        let fit = ((area_bot - area_top) / row_h).max(1) as usize;
        let extra = if self.peer_typing { 1 } else { 0 };
        let total = self.msg_count + extra;
        let start = total.saturating_sub(fit);
        let mut my = area_top;
        for idx in start..total {
            if idx < self.msg_count {
                let m = self.msgs[idx];
                let txt = m.as_str();
                let tw = Framebuffer::aa_w(txt, crate::fb::AA_S).min(wi * 7 / 10);
                let bub_w = tw + 20;
                let (bx, col) = if m.sent {
                    (xi + wi - bub_w - 10, if self.is_rcs { RCS } else { SMS })
                } else { (xi + 10, RECV) };
                fb.fill_rounded_rect(bx, my, bub_w, 22, 10, col);
                fb.draw_aa(bx + 10, my + 3, txt, 0xF4F6FA, crate::fb::AA_S);
            } else {
                // Typing indicator (RCS only).
                fb.fill_rounded_rect(xi + 10, my, 44, 22, 10, RECV);
                fb.draw_aa(xi + 20, my + 1, "...", 0xAAB2BC, crate::fb::AA_S);
            }
            my += row_h;
        }
    }
}

fn fmt_duration<'a>(buf: &'a mut [u8; 24], mins: u64, secs: u64) -> &'a str {
    buf[0] = b'0' + (mins / 10) as u8;
    buf[1] = b'0' + (mins % 10) as u8;
    buf[2] = b':';
    buf[3] = b'0' + (secs / 10) as u8;
    buf[4] = b'0' + (secs % 10) as u8;
    core::str::from_utf8(&buf[..5]).unwrap_or("00:00")
}

impl App for RustyPhone {
    fn tick(&mut self, ticks: u64) -> bool {
        if ticks.wrapping_sub(self.last_tick) < 100 { return false; }
        self.last_tick = ticks;
        // Advance call state machine (simulated: Dialing→Ringing after 1s, Ringing→Connected after 3s).
        match self.call_state {
            CallState::Dialing => {
                if ticks.wrapping_sub(self.call_start) > 100 {
                    self.call_state = CallState::Ringing;
                    self.dirty = true;
                }
            }
            CallState::Ringing => {
                if ticks.wrapping_sub(self.call_start) > 350 {
                    self.call_state = CallState::Connected;
                    self.call_start = ticks;
                    self.dirty = true;
                }
            }
            CallState::Connected => {
                let new_secs = ticks.wrapping_sub(self.call_start) / 100;
                if new_secs != self.call_secs {
                    self.call_secs = new_secs;
                    self.dirty = true;
                }
            }
            _ => {}
        }
        // Deliver the canned chat reply when its time comes.
        if self.reply_at != 0 && ticks >= self.reply_at {
            self.reply_at = 0;
            self.peer_typing = false;
            if self.msg_count < 24 {
                let reply: &[u8] = if self.is_rcs { b"RCS - delivered + read. love it" } else { b"got your SMS!" };
                self.msgs[self.msg_count] = Msg::new(reply, false);
                self.msg_count += 1;
            }
            self.dirty = true;
        }
        self.dirty
    }

    fn render(&mut self, fb: &mut Framebuffer, x: u32, y: u32, w: u32, h: u32) {
        let bg = 0x0D1117u32;
        fb.fill_rect(x, y, w, h, bg);

        // Portrait mode: window width ≤ 320 or when height >> width.
        let portrait = w <= 340;

        // Tab bar (4 tabs).
        let tab_h = 28u32;
        fb.fill_rect(x, y, w, tab_h, 0x161B22);
        let tab_labels = ["Dialer", "Recent", "Messages", "Account"];
        let tab_w = w / 4;
        for (i, label) in tab_labels.iter().enumerate() {
            let tx = x + i as u32 * tab_w;
            let active = match (i, self.tab) {
                (0, PhoneTab::Dialer) | (1, PhoneTab::Recent)
                | (2, PhoneTab::Messages) | (3, PhoneTab::Account) => true,
                _ => false,
            };
            let tc = if active { 0x22C55Eu32 } else { 0x4A5568 };
            if active { fb.fill_rect(tx, y + tab_h - 2, tab_w, 2, 0x22C55E); }
            let lw = Framebuffer::aa_w(label, crate::fb::AA_S);
            fb.draw_aa(tx as i32 + (tab_w as i32 - lw) / 2, y as i32 + 5, label, tc, crate::fb::AA_S);
        }

        let content_y = y + tab_h + 2;
        let content_h = h.saturating_sub(tab_h + 2);

        match self.tab {
            PhoneTab::Dialer   => self.draw_dialer(fb, x, content_y, w, content_h, portrait),
            PhoneTab::Recent   => self.draw_recent(fb, x, content_y, w, content_h),
            PhoneTab::Messages => self.draw_messages(fb, x, content_y, w, content_h),
            PhoneTab::Account  => self.draw_account(fb, x, content_y, w, content_h),
        }

        self.dirty = false;
    }

    fn on_key(&mut self, key: u8) {
        use crate::ansi::Key as AK;
        match self.ansi.feed(key) {
            AK::Char(0x1B) => { self.wants_close = true; }
            AK::Char(b'\t') => {
                self.tab = match self.tab {
                    PhoneTab::Dialer   => PhoneTab::Recent,
                    PhoneTab::Recent   => PhoneTab::Messages,
                    PhoneTab::Messages => PhoneTab::Account,
                    PhoneTab::Account  => PhoneTab::Dialer,
                };
                self.dirty = true;
            }
            AK::Char(b'\r') | AK::Char(b'\n') => {
                match self.tab {
                    PhoneTab::Messages => { let t = self.last_tick; self.send_message(t); }
                    PhoneTab::Dialer => {
                        if self.call_state == CallState::Idle || self.call_state == CallState::Ended {
                            self.dial(self.last_tick);
                        } else {
                            self.end_call();
                        }
                    }
                    PhoneTab::Account => {
                        match self.verify_state {
                            VerifyState::Idle | VerifyState::Failed => {
                                self.verify_state = VerifyState::EnterNumber;
                                self.verify_number_len = 0;
                            }
                            VerifyState::EnterNumber => {
                                if self.verify_number_len > 4 {
                                    // Generate code and (conceptually) trigger SMS via Twilio API.
                                    let code = Self::gen_code(self.last_tick);
                                    self.verify_code_sent = code;
                                    self.verify_state = VerifyState::EnterCode;
                                    // NOTE: on real hardware with internet, call sys_https_post here
                                    // to send the code via Twilio SMS API to verify_number.
                                }
                            }
                            VerifyState::EnterCode => {
                                // Compare entered code to sent code.
                                if self.verify_code_len == 6
                                    && &self.verify_code_input[..6] == &self.verify_code_sent[..] {
                                    self.verify_state = VerifyState::Verified;
                                    self.sip_registered = true;
                                } else {
                                    self.verify_state = VerifyState::Failed;
                                }
                            }
                            _ => {}
                        }
                    }
                    _ => {}
                }
                self.dirty = true;
            }
            AK::Char(0x08) => { // Backspace
                match self.tab {
                    PhoneTab::Messages => {
                        if self.compose_len > 0 { self.compose_len -= 1; self.dirty = true; }
                    }
                    PhoneTab::Dialer => {
                        if self.number_len > 0 { self.number_len -= 1; self.dirty = true; }
                    }
                    PhoneTab::Account => {
                        match self.verify_state {
                            VerifyState::EnterNumber => {
                                if self.verify_number_len > 0 { self.verify_number_len -= 1; }
                            }
                            VerifyState::EnterCode => {
                                if self.verify_code_len > 0 { self.verify_code_len -= 1; }
                            }
                            _ => {}
                        }
                        self.dirty = true;
                    }
                    _ => {}
                }
            }
            AK::Char(ch) if self.tab == PhoneTab::Messages && (0x20..=0x7E).contains(&ch) => {
                if self.compose_len < 96 {
                    self.compose[self.compose_len] = ch;
                    self.compose_len += 1;
                    self.dirty = true;
                }
            }
            AK::Char(ch @ b'0'..=b'9') | AK::Char(ch @ b'*') | AK::Char(ch @ b'#') | AK::Char(ch @ b'+') => {
                match self.tab {
                    PhoneTab::Dialer => { self.push_digit(ch); }
                    PhoneTab::Account => {
                        match self.verify_state {
                            VerifyState::EnterNumber if ch != b'*' && ch != b'#' => {
                                if self.verify_number_len < 15 {
                                    self.verify_number[self.verify_number_len] = ch;
                                    self.verify_number_len += 1;
                                    self.dirty = true;
                                }
                            }
                            VerifyState::EnterCode if ch >= b'0' && ch <= b'9' => {
                                if self.verify_code_len < 6 {
                                    self.verify_code_input[self.verify_code_len] = ch;
                                    self.verify_code_len += 1;
                                    self.dirty = true;
                                }
                            }
                            _ => { self.push_digit(ch); }
                        }
                    }
                    _ => {}
                }
            }
            AK::Char(b'c') | AK::Char(b'C') => {
                if self.tab == PhoneTab::Dialer { self.number_len = 0; self.dirty = true; }
            }
            _ => {}
        }
    }

    fn on_mouse(&mut self, x: i32, y: i32, w: u32, h: u32, buttons: u8) {
        if buttons & 0x01 == 0 { return; }
        let tab_h = 28i32;

        // Tab bar click.
        if y < tab_h {
            let tab_w = w as i32 / 4;
            let col = (x / tab_w).clamp(0, 3);
            self.tab = match col {
                0 => PhoneTab::Dialer, 1 => PhoneTab::Recent,
                2 => PhoneTab::Messages, _ => PhoneTab::Account,
            };
            self.dirty = true;
            return;
        }

        // Content-area click (y relative to content start).
        let cy = y - tab_h - 2;
        let content_h = (h as i32 - tab_h - 2).max(1) as u32;

        // Messages: toggle the SMS/RCS badge (top-right), or hit the send button.
        if self.tab == PhoneTab::Messages {
            if cy >= 0 && cy < 28 && x > w as i32 - 120 {
                self.is_rcs = !self.is_rcs; self.dirty = true; return;
            }
            let comp_y = content_h as i32 - 34;
            if cy >= comp_y && x > w as i32 - 40 {
                let t = self.last_tick; self.send_message(t); return;
            }
            return;
        }

        match self.tab {
            PhoneTab::Dialer => {
                let portrait = w <= 340;
                let pad = if portrait { 6i32 } else { 4i32 };
                let disp_h = if portrait { 64i32 } else { 52i32 };
                let key_rows = 4i32;
                let key_cols = 3i32;
                let avail_w = w as i32 - pad * (key_cols + 1);
                let avail_h = content_h as i32 - disp_h - pad * (key_rows + 3) - 40;
                let kw = avail_w / key_cols;
                let kh = (avail_h / (key_rows + 1)).min(if portrait { 56 } else { 42 });
                let grid_y = disp_h + pad;

                // Key grid (rows 0-3)
                let local_x = x - pad;
                let local_y = cy - grid_y - pad;
                if local_x >= 0 && local_y >= 0 {
                    let col = local_x / (kw + pad);
                    let row = local_y / (kh + pad);
                    if col < key_cols && row < key_rows {
                        if local_x % (kw + pad) < kw && local_y % (kh + pad) < kh {
                            let keys: [u8; 12] = [b'1',b'2',b'3',b'4',b'5',b'6',b'7',b'8',b'9',b'*',b'0',b'#'];
                            let idx = (row * key_cols + col) as usize;
                            self.push_digit(keys[idx]);
                            return;
                        }
                    }
                }

                // Action row
                let action_y = grid_y + key_rows * (kh + pad) + pad;
                if cy >= action_y && cy < action_y + kh {
                    let lx = x - pad;
                    let col = lx / (kw + pad);
                    match col {
                        0 => { if self.number_len > 0 { self.number_len -= 1; self.dirty = true; } }
                        1 => {
                            if self.call_state == CallState::Idle || self.call_state == CallState::Ended {
                                self.dial(self.last_tick);
                            } else {
                                self.end_call();
                            }
                        }
                        2 => { self.muted = !self.muted; self.dirty = true; }
                        _ => {}
                    }
                }
            }
            PhoneTab::Recent => {
                // Recall: click a recent-call row.
                if self.recent_count > 0 {
                    let row = (cy - 32) / 28;
                    let count = self.recent_count.min(8) as i32;
                    if row >= 0 && row < count {
                        let slot = (self.recent_count - 1 - row as usize) % 8;
                        let n = self.recent_len[slot];
                        self.number[..n].copy_from_slice(&self.recent[slot][..n]);
                        self.number_len = n;
                        self.tab = PhoneTab::Dialer;
                        self.dirty = true;
                    }
                }
            }
            PhoneTab::Account => {
                // Verify flow buttons.
                match self.verify_state {
                    VerifyState::Idle | VerifyState::Failed => {
                        // "Send Code" button area ~y=74..96
                        if cy >= 42 && cy < 100 {
                            self.verify_state = VerifyState::EnterNumber;
                            self.verify_number_len = 0;
                            self.dirty = true;
                        }
                    }
                    VerifyState::EnterNumber => {
                        if cy >= 74 && cy < 96 {
                            if self.verify_number_len > 4 {
                                let code = Self::gen_code(self.last_tick);
                                self.verify_code_sent = code;
                                self.verify_state = VerifyState::EnterCode;
                                self.verify_code_len = 0;
                                self.dirty = true;
                            }
                        }
                    }
                    VerifyState::EnterCode => {
                        if cy >= 94 && cy < 116 {
                            if self.verify_code_len == 6
                                && &self.verify_code_input[..6] == &self.verify_code_sent[..] {
                                self.verify_state = VerifyState::Verified;
                                self.sip_registered = true;
                            } else {
                                self.verify_state = VerifyState::Failed;
                            }
                            self.dirty = true;
                        }
                    }
                    VerifyState::Failed => {
                        if cy >= 62 && cy < 84 {
                            self.verify_state = VerifyState::EnterNumber;
                            self.verify_number_len = 0;
                            self.verify_code_len = 0;
                            self.dirty = true;
                        }
                    }
                    _ => {}
                }
                // SIP Register button ~y=sip_y+100..122 (sip_y=140)
                let sip_btn_y = 140 + 100;
                if cy >= sip_btn_y && cy < sip_btn_y + 22 {
                    self.sip_registered = !self.sip_registered;
                    self.dirty = true;
                }
            }
            PhoneTab::Messages => {} // handled by the early return above
        }
    }

    fn wants_close(&self) -> bool { self.wants_close }
    fn title(&self) -> &str { "RustyPhone" }
}

fn fmt_u64_into<'a>(buf: &'a mut [u8; 24], mut n: u64) -> &'a str {
    if n == 0 { buf[0] = b'0'; return core::str::from_utf8(&buf[..1]).unwrap_or("0"); }
    let mut i = 0;
    while n > 0 { buf[i] = b'0' + (n % 10) as u8; n /= 10; i += 1; }
    buf[..i].reverse();
    core::str::from_utf8(&buf[..i]).unwrap_or("0")
}
