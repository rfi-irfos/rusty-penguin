#![no_std]
#![no_main]

extern crate alloc;

mod allocator;
mod ansi;
mod app;
mod clipboard;
mod css;
mod editor;
mod fb;
mod font;
mod font_aa;
mod heap;
mod icons;
mod input;
mod term;
mod trit;
mod vfs;
mod wm;

use alloc::vec;
use alloc::vec::Vec;

use fb::Framebuffer;
use input::MouseState;

// ---- Syscall stubs ────────────────────────────────────────────────────────

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

/// sys_battery_pct (#20) → bits 7:0 = battery % (0-100 or 0xFF if no battery),
///                          bit 8   = charging flag (1 = charging).
unsafe fn sys_battery_raw() -> u64 {
    let n: u64;
    core::arch::asm!(
        "syscall",
        inout("rax") 20u64 => n,
        in("rdi") 0u64,
        out("rcx") _, out("r11") _,
        options(nostack),
    );
    n
}
unsafe fn sys_battery_pct() -> u32 { (sys_battery_raw() & 0xFF) as u32 }
unsafe fn sys_battery_charging() -> bool { (sys_battery_raw() >> 8) & 1 != 0 }

/// sys_kbd_layout (#21): op 0 = query, 1 = set EN, 2 = set DE. Returns 0=EN, 1=DE.
unsafe fn sys_kbd_layout(op: u64) -> u64 {
    let n: u64;
    core::arch::asm!(
        "syscall",
        inout("rax") 21u64 => n,
        in("rdi") op,
        out("rcx") _, out("r11") _,
        options(nostack),
    );
    n
}

/// sys_exec_linux (#28): kernel suspends desktop, executes a Linux ELF from
/// the initrd by path, then restarts the desktop when that process exits.
/// Returns only if the binary was not found — otherwise diverges in the kernel.
fn sys_exec_linux(path: &[u8]) -> u64 {
    let n: u64;
    unsafe {
        core::arch::asm!(
            "syscall",
            inout("rax") 28u64 => n,
            in("rdi") path.as_ptr() as u64,
            in("rsi") path.len() as u64,
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

fn sys_rtc() -> u64 {
    let n: u64;
    unsafe {
        core::arch::asm!(
            "syscall",
            inout("rax") 13u64 => n,
            in("rdi") 0u64,
            out("rcx") _, out("r11") _,
            options(nostack),
        );
    }
    n
}

fn sys_yield() {
    unsafe {
        core::arch::asm!(
            "syscall",
            in("rax") 24u64,
            in("rdi") 0u64,
            out("rcx") _, out("r11") _,
            options(nostack),
        );
    }
}

/// sys_autostart (#22): app index from `autostart=N` on the kernel cmdline, or
/// u64::MAX if none. Used to open a specific app on boot for headless screendumps.
fn sys_autostart() -> u64 {
    let n: u64;
    unsafe {
        core::arch::asm!(
            "syscall",
            inout("rax") 22u64 => n,
            in("rdi") 0u64,
            out("rcx") _, out("r11") _,
            options(nostack),
        );
    }
    n
}

/// sys_boot_brightness (#40): startup software-brightness % from `brightness=N`,
/// or u64::MAX if unset. Lets the desktop boot pre-dimmed (default/kiosk knob, and
/// the headless way to verify the present-time dimming since the slider needs a mouse).
fn sys_boot_brightness() -> u64 {
    let n: u64;
    unsafe {
        core::arch::asm!(
            "syscall",
            inout("rax") 40u64 => n,
            in("rdi") 0u64,
            out("rcx") _, out("r11") _,
            options(nostack),
        );
    }
    n
}

/// sys_app_surface (#41): VA where a second app's live 32×32 surface is mapped
/// into this (desktop) process, or 0 if none (schedesktop2 windowed-app mode).
fn sys_app_surface() -> u64 {
    let n: u64;
    unsafe {
        core::arch::asm!(
            "syscall",
            inout("rax") 41u64 => n,
            in("rdi") 0u64,
            out("rcx") _, out("r11") _,
            options(nostack),
        );
    }
    n
}

/// sys_app_surface_dims (#42): (w<<16)|h of the app surface, or 0 for the legacy
/// 32x32 synthetic app.
fn sys_app_surface_dims() -> u64 {
    let n: u64;
    unsafe {
        core::arch::asm!(
            "syscall",
            inout("rax") 42u64 => n,
            in("rdi") 0u64,
            out("rcx") _, out("r11") _,
            options(nostack),
        );
    }
    n
}

/// sys_initrd_read (#29): read a file from the kernel CPIO initramfs into a
/// caller-provided buffer. Returns bytes written or u64::MAX on failure.
/// arg3 packs: low 32 bits = out_ptr, high 32 bits = out_len.
fn sys_initrd_read(path: &[u8], out: &mut [u8]) -> usize {
    let out_cap = out.len().min(0xFFFF_FFFF);
    let packed_out = (out.as_mut_ptr() as u64) | ((out_cap as u64) << 32);
    let n: u64;
    unsafe {
        core::arch::asm!(
            "syscall",
            inout("rax") 29u64 => n,
            in("rdi") path.as_ptr(),
            in("rsi") path.len(),
            in("rdx") packed_out,
            out("rcx") _, out("r11") _,
            options(nostack),
        );
    }
    if n == u64::MAX { 0 } else { n as usize }
}

/// Read a kernel CPIO file into a freshly heap-allocated Vec. Returns None if
/// missing or too large.
fn load_cpio_file(path: &str) -> Option<alloc::vec::Vec<u8>> {
    // First pass: measure size (read into a 1-byte probe returns 0, not ideal).
    // Instead allocate a generous cap and shrink.
    let mut buf = alloc::vec![0u8; 512 * 1024]; // 512 KiB — covers all icons + the 320×304 watermark
    let n = sys_initrd_read(path.as_bytes(), &mut buf);
    if n == 0 { return None; }
    buf.truncate(n);
    Some(buf)
}

/// sys_wallpaper (#23): system-background index from `wallpaper=N`, or u64::MAX.
fn sys_wallpaper() -> u64 {
    let n: u64;
    unsafe {
        core::arch::asm!("syscall", inout("rax") 23u64 => n, in("rdi") 0u64,
            out("rcx") _, out("r11") _, options(nostack));
    }
    n
}

/// sys_showmenu (#35): true if `showmenu` was on the cmdline (open the start
/// menu on launch — a screenshot aid, since mouse clicks can't be driven headless).
fn sys_showmenu() -> bool {
    let n: u64;
    unsafe {
        core::arch::asm!("syscall", inout("rax") 35u64 => n, in("rdi") 0u64,
            out("rcx") _, out("r11") _, options(nostack));
    }
    n != 0
}

/// Open a desktop app by its menu index (the MenuLaunch::App(n) tags). Returns
/// None for unknown indices. Shared by autostart and the right-click menu.
fn open_app_by_index(idx: u64, w: i32, h: i32, n: usize) -> Option<TermWin> {
    match idx {
        0  => open_file_manager(w, h, n),
        1  => open_editor(w, h, n, "readme.txt", "Text Editor"),
        2  => open_calculator(w, h, n),
        3  => open_help_browser(w, h, n),
        4  => open_settings(w, h, n),
        5  => open_tis_console(w, h, n),
        6  => open_snake(w, h, n),
        7  => open_minesweeper(w, h, n),
        8  => open_doom_raycaster(w, h, n),
        9  => open_browser(w, h, n),
        10 => open_kernel_manager(w, h, n),
        11 => open_sound(w, h, n),
        12 => open_media_player(w, h, n),
        13 => open_screenshot(w, h, n),
        14 => open_image_viewer(w, h, n),
        15 => open_system_clock(w, h, n),
        16 => open_process_monitor(w, h, n),
        100 => open_term(w, h, n, &LAUNCHERS[0]),
        _  => None,
    }
}

fn sys_serial_debug(b: u8) {
    unsafe {
        core::arch::asm!(
            "syscall",
            in("rax") 12u64,
            in("rdi") b as u64,
            out("rcx") _,
            out("r11") _,
            options(nostack),
        );
    }
}

fn serial_write_str(s: &str) {
    for &b in s.as_bytes() { sys_serial_debug(b); }
}

fn serial_write_u64(mut n: u64) {
    if n == 0 { sys_serial_debug(b'0'); return; }
    let mut buf = [0u8; 20];
    let mut i = 0;
    while n > 0 { buf[i] = b'0' + (n % 10) as u8; n /= 10; i += 1; }
    while i > 0 { i -= 1; sys_serial_debug(buf[i]); }
}

// ---- Palette ────────────────────────────────────────────────────────────────
// "Deep Azure" — midnight blue-black base, sky-blue primary, rose/pink accent.
// Zabih's direction: ubuntu-style deep gradients, no more olive/stone green.
// The stone-green palette is still available as a preset in Settings.
const BG:       u32 = 0x0B0F19;  // Midnight blue-black
const TOPBAR:   u32 = 0x131928;  // Deep navy panel
const TASKBAR:  u32 = 0x0B0F19;  // Dock backing
const TOPBAR_H: u32 = 32;        // Topbar height
const BORDER:   u32 = 0x1E2A40;  // Deep navy hairline
const GREEN:    u32 = 0x38BDF8;  // Azure / sky blue (primary accent)
const DIM:      u32 = 0x8B9EC4;  // Blue-grey secondary text
const DIMMER:   u32 = 0x1E2A40;  // Match border
const WHITE:    u32 = 0xE2E8F0;  // Cool off-white
const AMBER:    u32 = 0xFBBF24;  // Warm gold accent
const BLUE:     u32 = 0x818CF8;  // Indigo/violet (ternary bus)
const CURSOR:   u32 = 0x060912;  // arrow fill — near-black
const TEAL:     u32 = 0x2DD4BF;  // Cyan/turquoise
const ACCENT_CREAM: u32 = 0xF472B6;  // Rose/pink — dingir star + highlights
const TRIT_NEG:  u32 = 0xFB7185;  // ternary -1 (rose)
const TRIT_ZERO: u32 = 0x64748B;  // ternary 0  (slate)
const TRIT_POS:  u32 = 0x38BDF8;  // ternary +1 (azure)

// ── Bottom panel layout (Simeon's v2 mockup form: a single floating bottom dock
// — Menu · favourites · tasks · tray — and NO top bar). ─────────────────────────
const PANEL_MARGIN: i32 = 14;   // inset from screen left/right
const PANEL_BOTTOM: i32 = 12;   // gap below the panel
const PANEL_H:      i32 = 54;   // panel height
const MENU_BTN_W:   i32 = 100;  // "Menu" button width (wider to fit 36px dingir + label)
const FAV_TILE:     i32 = 40;   // favourite icon tile
const FAV_GAP:      i32 = 8;    // gap between favourites
const PANEL_SOLID:  u32 = 0x0E1828;  // dock body: deep navy glass
const DOCK_ALPHA:   u32 = 130;       // semi-transparent
const PANEL_EDGE:   u32 = 0x2A4A6A;  // panel hairline / azure sheen
const PANEL_R:      i32 = 16;        // panel corner radius

fn panel_top(h: u32) -> i32 { h as i32 - PANEL_BOTTOM - PANEL_H }

// ---- Window snap ────────────────────────────────────────────────────────────
// Drag a window's titlebar to within SNAP_ZONE px of a screen edge and release
// to snap it to the left half, right half, or full maximized.
const SNAP_ZONE: i32 = 20;

#[derive(Copy, Clone, PartialEq)]
enum SnapSide { Left, Right, Top }
fn menu_btn_rect(h: u32) -> (i32, i32, i32, i32) { (PANEL_MARGIN + 8, panel_top(h) + 7, MENU_BTN_W, 40) }

// Dock favourites — the mockup pins exactly 5 (Terminal, Files, TIS, Editor,
// Calculator); everything else lives in the start menu. Values index into
// DESKTOP_ICONS so the existing click-handler match (keyed on that index) is
// unchanged. Keeping the dock to 5 also avoids the 10-icon overflow regression.
const N_FAV: usize = 7;
const FAV_IDX: [usize; N_FAV] = [0, 1, 10, 2, 11, 12, 9]; // Term, Files, Web, Edit, Phone, Notes, Doom

// favourites_row: CSS FlexRow for the icon strip inside the dock.
// Left edge starts after Menu button + separator gap.
fn favourites_row(h: u32) -> css::FlexRow {
    let ptop = panel_top(h);
    let x0 = PANEL_MARGIN + 8 + MENU_BTN_W + 16;  // after menu button
    css::FlexRow::new(x0, ptop + 7, N_FAV as i32 * (FAV_TILE + FAV_GAP), PANEL_H - 14, FAV_GAP)
}

// fav_rect takes a dock SLOT (0..N_FAV), not a DESKTOP_ICONS index.
fn fav_rect(slot: usize, h: u32) -> (i32, i32, i32, i32) {
    let row = favourites_row(h);
    let (x, y) = row.item_rect_centered(N_FAV, slot, FAV_TILE, FAV_TILE);
    (x, y, FAV_TILE, FAV_TILE)
}

// Fill area (hot-spot at (0,0)).  Save/restore adds 1px border on all four sides.
const CURSOR_W:  u32 = 12;
const CURSOR_H:  u32 = 18;
const CURSOR_BW: u32 = CURSOR_W + 2;   // buffer width
const CURSOR_BH: u32 = CURSOR_H + 2;   // buffer height

// ---- Desktop icon bitmaps ───────────────────────────────────────────────────
// 8×8 bitmaps for desktop icon graphics
#[rustfmt::skip]
const ICON_TERM: [u8; 8] = [0x7E, 0x42, 0x5A, 0x4E, 0x42, 0x42, 0x42, 0x7E];  // terminal box
#[rustfmt::skip]
const ICON_PROC: [u8; 8] = [0x00, 0x3C, 0x42, 0x5A, 0x5A, 0x42, 0x3C, 0x00];  // process ring
#[rustfmt::skip]
const ICON_AI:   [u8; 8] = [0x18, 0x24, 0x42, 0xFF, 0x42, 0x24, 0x18, 0x00];  // diamond/AI
#[rustfmt::skip]
const ICON_TRIT: [u8; 8] = [0x08, 0x1C, 0x36, 0x63, 0x63, 0x36, 0x1C, 0x08];  // ternary ring

// Kernel Manager icon: gear/cog shape
#[rustfmt::skip]
const ICON_KM: [u8; 8] = [0x18, 0x7E, 0x3C, 0xFF, 0xFF, 0x3C, 0x7E, 0x18];

// File manager icon: folder shape
#[rustfmt::skip]
const ICON_FILES: [u8; 8] = [0x3E, 0x22, 0xE2, 0xA2, 0xA2, 0xA2, 0xA2, 0xFE];

// Snake icon: coiled body
#[rustfmt::skip]
const ICON_SNAKE: [u8; 8] = [0x7C, 0x04, 0x04, 0x7C, 0x40, 0x40, 0x7C, 0x02];
// Minesweeper icon: bomb with fuse
#[rustfmt::skip]
const ICON_MINE:  [u8; 8] = [0x08, 0x2A, 0x1C, 0x3E, 0x7F, 0x7F, 0x3E, 0x1C];
// Doom icon: a stylized demon/skull face
#[rustfmt::skip]
const ICON_DOOM:  [u8; 8] = [0x3C, 0x7E, 0xDB, 0xFF, 0xBD, 0xFF, 0x66, 0x24];

// Web browser icon: globe with meridians
#[rustfmt::skip]
const ICON_WEB: [u8; 8] = [0x3C, 0x52, 0x9D, 0xBD, 0xBD, 0x9D, 0x52, 0x3C];

// Dingir — cuneiform divine determinative (8-pointed star with wedges)
#[rustfmt::skip]
const DINGIR: [u8; 8] = [
    0x18, 0x7E, 0x7E, 0x3C, 0x3C, 0x7E, 0x7E, 0x18,
];

// ---- Cursor helpers ─────────────────────────────────────────────────────────
// Arrow cursor. Callers pass the hotspot in screen coordinates; the hotspot is
// the pointer tip, so hit testing and drawing use the same point.

fn save_cursor_bg(fb: &Framebuffer, x: i32, y: i32, buf: &mut [u32]) {
    for row in 0..CURSOR_BH as i32 {
        for col in 0..CURSOR_BW as i32 {
            let px = x - 1 + col; let py = y - 1 + row;
            let idx = (row * CURSOR_BW as i32 + col) as usize;
            buf[idx] = if px >= 0 && py >= 0 && (px as u32) < fb.width && (py as u32) < fb.height {
                fb.get_pixel(px as u32, py as u32)
            } else { BG };
        }
    }
}

fn restore_cursor_bg(fb: &mut Framebuffer, x: i32, y: i32, buf: &[u32]) {
    for row in 0..CURSOR_BH as i32 {
        for col in 0..CURSOR_BW as i32 {
            let px = x - 1 + col; let py = y - 1 + row;
            if px >= 0 && py >= 0 && (px as u32) < fb.width && (py as u32) < fb.height {
                fb.set_pixel(px as u32, py as u32, buf[(row * CURSOR_BW as i32 + col) as usize]);
            }
        }
    }
}

// Classic left-pointing arrow pointer (hotspot = top-left tip at col0,row0):
// a slim arrowhead with a diagonal tail, not the old flag-on-a-pole shape.
// Each row is a 12-bit mask, MSB = leftmost column.
#[rustfmt::skip]
const ARROW: [u16; CURSOR_H as usize] = [
    0b100000000000,
    0b110000000000,
    0b111000000000,
    0b111100000000,
    0b111110000000,
    0b111111000000,
    0b111111100000,
    0b111111110000,
    0b111111111000,
    0b111111111100,
    0b111111100000,
    0b111011100000,
    0b110011100000,
    0b100001110000,
    0b000001110000,
    0b000000111000,
    0b000000111000,
    0b000000010000,
];

fn cursor_mask(col: i32, row: i32) -> bool {
    if row < 0 || col < 0 || row >= CURSOR_H as i32 || col >= CURSOR_W as i32 { return false; }
    (ARROW[row as usize] >> (11 - col)) & 1 != 0
}

fn draw_cursor(fb: &mut Framebuffer, x: i32, y: i32) {
    // Black arrow with a light halo (matches the classic pointer Simeon wants);
    // the white outline keeps it legible on the dark wallpaper.
    let outline = 0xF2F0E8u32;
    let put = |fb: &mut Framebuffer, px: i32, py: i32, c: u32| {
        if px >= 0 && py >= 0 && (px as u32) < fb.width && (py as u32) < fb.height {
            fb.set_pixel(px as u32, py as u32, c);
        }
    };

    // Outline — iterate the full extended bounding box (including -1 offsets)
    // so the top and left edges of the cursor get a black border too.
    // 8-neighbor check produces a clean 1px outline around all edges including
    // the diagonal hypotenuse.
    for row in -1..CURSOR_H as i32 + 1 {
        for col in -1..CURSOR_W as i32 + 1 {
            if !cursor_mask(col, row) {
                let near =
                    cursor_mask(col-1, row-1) || cursor_mask(col, row-1) || cursor_mask(col+1, row-1) ||
                    cursor_mask(col-1, row  ) ||                            cursor_mask(col+1, row  ) ||
                    cursor_mask(col-1, row+1) || cursor_mask(col, row+1) || cursor_mask(col+1, row+1);
                if near { put(fb, x + col, y + row, outline); }
            }
        }
    }

    // White fill drawn on top of outline
    for row in 0..CURSOR_H as i32 {
        for col in 0..CURSOR_W as i32 {
            if cursor_mask(col, row) { put(fb, x + col, y + row, CURSOR); }
        }
    }
}

// ---- Stats ──────────────────────────────────────────────────────────────────

struct SysStats { mem_pct: u8, used_mib: u32, total_mib: u32 }

fn sample_stats() -> SysStats {
    let (free, total) = sys_meminfo();
    let used = total.saturating_sub(free);
    let mem_pct = if total > 0 { ((used as u64 * 100 / total as u64) as u8).min(100) } else { 0 };
    SysStats { mem_pct, used_mib: used, total_mib: total }
}

// Stack-allocated string builder — format! uses alloc::fmt::format_inner which
// crashes in this bare-metal env (1-byte-aligned heap + SSE movaps = #GP).
struct Strbuf { buf: [u8; 48], len: usize }
impl Strbuf {
    fn new() -> Self { Strbuf { buf: [0; 48], len: 0 } }
    fn push(&mut self, b: u8) { if self.len < 48 { self.buf[self.len] = b; self.len += 1; } }
    fn push_bytes(&mut self, s: &[u8]) { for &b in s { self.push(b); } }
    fn push_u64(&mut self, mut n: u64) {
        if n == 0 { self.push(b'0'); return; }
        let mut tmp = [0u8; 20]; let mut i = 0;
        while n > 0 { tmp[i] = b'0' + (n % 10) as u8; n /= 10; i += 1; }
        for j in (0..i).rev() { self.push(tmp[j]); }
    }
    fn push_d2(&mut self, n: u64) { if n < 10 { self.push(b'0'); } self.push_u64(n); }
    fn as_str(&self) -> &str { core::str::from_utf8(&self.buf[..self.len]).unwrap_or("") }
}

fn month_abbr(m: u8) -> [u8; 3] {
    match m {
        1  => *b"Jan",  2  => *b"Feb",  3  => *b"Mar",  4  => *b"Apr",
        5  => *b"May",  6  => *b"Jun",  7  => *b"Jul",  8  => *b"Aug",
        9  => *b"Sep",  10 => *b"Oct",  11 => *b"Nov",  12 => *b"Dec",
        _  => *b"???",
    }
}
fn day_abbr(d: u8) -> [u8; 3] {
    match d {
        1 => *b"Sun", 2 => *b"Mon", 3 => *b"Tue", 4 => *b"Wed",
        5 => *b"Thu", 6 => *b"Fri", 7 => *b"Sat", _ => *b"???",
    }
}

fn rtc_str() -> Strbuf {
    let rtc = sys_rtc();
    let wday  = (rtc        & 0xFF) as u8;
    let sec   = ((rtc >>  8) & 0xFF) as u8;
    let min   = ((rtc >> 16) & 0xFF) as u8;
    let hour  = ((rtc >> 24) & 0xFF) as u8;
    let mday  = ((rtc >> 32) & 0xFF) as u8;
    let month = ((rtc >> 40) & 0xFF) as u8;
    let mut s = Strbuf::new();
    if month == 0 || month > 12 || mday == 0 || mday > 31 || hour > 23 || min > 59 || sec > 59 {
        let ticks = sys_ticks();
        let secs = ticks / 100;
        let h = secs / 3600; let m = (secs % 3600) / 60; let sc = secs % 60;
        s.push_d2(h); s.push(b':'); s.push_d2(m); s.push(b':'); s.push_d2(sc);
        return s;
    }
    let d = day_abbr(wday);
    let mo = month_abbr(month);
    s.push_bytes(&d); s.push(b' ');
    s.push_bytes(&mo); s.push(b' ');
    s.push_u64(mday as u64);
    s.push(b' '); s.push(b' ');
    s.push_d2(hour as u64); s.push(b':');
    s.push_d2(min as u64); s.push(b':');
    s.push_d2(sec as u64);
    s
}

// ---- Scene drawing ──────────────────────────────────────────────────────────

// ── Wallpaper gallery ─────────────────────────────────────────────────────────
// Eight procedural "system backgrounds", each with real depth (gradients, glows,
// layered silhouettes, atmospheric perspective) — never flat. Cycle through them
// with right-click desktop → Change Background. All drawn from scratch, no image
// assets (every pixel is ours). These are STATIC (cached) so per-pixel cost is a
// one-time draw.
pub const WALLPAPER_COUNT: u8 = 8;
/// Index of the custom-image wallpaper (loaded from VFS "wallpaper.ppm"); only in
/// the cycle when that file exists. Set via the Image Viewer's "W" key.
pub const WP_CUSTOM: u8 = 8;
/// The Image Viewer sets this (same process) when the user presses W; the desktop
/// loop switches to the custom wallpaper. (apps run in-process, so a shared static
/// is the simplest signal.)
pub static mut WALLPAPER_SET_REQUEST: bool = false;

/// Open-window titles + count, published each frame for the System Monitor's
/// Applications list. KILL_IDX, set by the monitor, asks the loop to force-quit
/// (close) that window. (Apps run in-process, so shared statics are the channel.)
pub static mut APP_TITLES: [[u8; 20]; 12] = [[0; 20]; 12];
pub static mut APP_COUNT: usize = 0;
pub static mut KILL_IDX: i32 = -1;

/// Parse a binary PPM (P6) header → (width, height, pixel_data_offset).
fn parse_ppm_dims(data: &[u8]) -> Option<(usize, usize, usize)> {
    if data.len() < 2 || &data[0..2] != b"P6" { return None; }
    let mut i = 2usize; let mut nums = [0usize; 3]; let mut ni = 0usize;
    while ni < 3 && i < data.len() {
        while i < data.len() {
            let c = data[i];
            if c == b'#' { while i < data.len() && data[i] != b'\n' { i += 1; } }
            else if c == b' ' || c == b'\t' || c == b'\n' || c == b'\r' { i += 1; }
            else { break; }
        }
        let mut v = 0usize; let mut any = false;
        while i < data.len() && data[i].is_ascii_digit() { v = v * 10 + (data[i] - b'0') as usize; any = true; i += 1; }
        if !any { return None; }
        nums[ni] = v; ni += 1;
    }
    if ni < 3 { return None; }
    if i < data.len() { i += 1; }
    Some((nums[0], nums[1], i))
}

/// Dingir logo — baked into the binary at compile time.
static DINGIR_PPM: &[u8] = include_bytes!("../../iso/data/dingir.ppm");

/// Render the baked-in dingir (or any path — path is now ignored, always uses baked data).
fn draw_ppm_icon_centered(fb: &mut Framebuffer, _path: &str, cx: i32, cy: i32, size: u32) -> bool {
    let data: &[u8] = DINGIR_PPM;
    let (iw, ih, off) = match parse_ppm_dims(data) { Some(v) => v, None => return false };
    if iw == 0 || ih == 0 || off + iw * ih * 3 > data.len() { return false; }
    let sx = cx - size as i32 / 2;
    let sy = cy - size as i32 / 2;
    for py in 0..size {
        let src_y = (py as usize * ih / size as usize).min(ih - 1);
        for px in 0..size {
            let src_x = (px as usize * iw / size as usize).min(iw - 1);
            let p = off + (src_y * iw + src_x) * 3;
            let (r, g, b) = (data[p] as u32, data[p+1] as u32, data[p+2] as u32);
            // Skip pixels that are near the bg color (transparent area from JPEG bg removal).
            if r < 0x18 && g < 0x20 && b < 0x2C { continue; }
            let col = (r << 16) | (g << 8) | b;
            fb.set_pixel((sx + px as i32) as u32, (sy + py as i32) as u32, col);
        }
    }
    true
}

/// Rusty Penguin logo — baked into the binary at compile time.
/// No CPIO loading, no heap, always renders on every background.
static RP_LOGO_PPM: &[u8] = include_bytes!("../../iso/data/rp_watermark.ppm");

/// Render the baked-in RP logo as a ghost watermark at `(x, y)`.
/// Dark pixels (logo background) are skipped so only the gear+penguin shows.
fn draw_ppm_watermark(fb: &mut Framebuffer, _path: &str, x: u32, y: u32, size: u32) {
    let data: &[u8] = RP_LOGO_PPM;
    let (iw, ih, off) = match parse_ppm_dims(data) { Some(v) => v, None => return };
    if iw == 0 || ih == 0 || off + iw * ih * 3 > data.len() { return; }
    let fw = fb.width; let fh = fb.height;
    for py in 0..size {
        let ty = y + py; if ty >= fh { break; }
        let src_y = (py as usize * ih / size as usize).min(ih - 1);
        for px in 0..size {
            let tx = x + px; if tx >= fw { break; }
            let src_x = (px as usize * iw / size as usize).min(iw - 1);
            let p = off + (src_y * iw + src_x) * 3;
            if p + 2 >= data.len() { continue; }
            let (r, g, b) = (data[p] as u32, data[p+1] as u32, data[p+2] as u32);
            // Skip dark background pixels (the brownish-black surround of the PNG).
            // Threshold 50 cuts the ~luma-38 brownish bg while keeping gear teeth.
            let luma = (r * 77 + g * 150 + b * 29) >> 8;
            if luma < 50 { continue; }
            // Blend: 28% source, 72% background.
            let bg = fb.get_pixel(tx, ty);
            let br = (bg >> 16) & 0xFF;
            let bg_ = (bg >> 8) & 0xFF;
            let bb = bg & 0xFF;
            let nr = (r * 28 + br * 72) / 100;
            let ng = (g * 28 + bg_ * 72) / 100;
            let nb = (b * 28 + bb * 72) / 100;
            fb.set_pixel(tx, ty, (nr << 16) | (ng << 8) | nb);
        }
    }
}

/// Custom wallpaper — decode VFS "wallpaper.ppm" (any P6) and scale it to FILL the
/// screen (cover + centre-crop). Falls back to the default if absent/invalid.
fn draw_bg_custom(fb: &mut Framebuffer) {
    let data = match vfs::vfs().read("wallpaper.ppm") { Some(d) => d, None => { draw_bg_stone(fb); return; } };
    let (iw, ih, off) = match parse_ppm_dims(data) { Some(v) => v, None => { draw_bg_stone(fb); return; } };
    if iw == 0 || ih == 0 { draw_bg_stone(fb); return; }
    let w = fb.width as usize; let h = fb.height as usize;
    let s = ((w * 1000 / iw).max(h * 1000 / ih)).max(1);   // cover scale ×1000
    let outw = (iw * s / 1000).max(w); let outh = (ih * s / 1000).max(h);
    let ox = (outw - w) / 2; let oy = (outh - h) / 2;       // centre-crop offset
    for py in 0..h {
        let sy = ((py + oy) * ih / outh).min(ih - 1);
        for px in 0..w {
            let sx = ((px + ox) * iw / outw).min(iw - 1);
            let p = off + (sy * iw + sx) * 3;
            if p + 2 < data.len() {
                fb.set_pixel(px as u32, py as u32,
                    ((data[p] as u32) << 16) | ((data[p+1] as u32) << 8) | data[p+2] as u32);
            }
        }
    }
}

#[inline] fn lerp8(a: u32, b: u32, t: u64) -> u32 {
    ((a as i64) + ((b as i64) - (a as i64)) * (t as i64) / 255) as u32
}
#[inline] fn rgb(r: u32, g: u32, b: u32) -> u32 { ((r & 0xFF) << 16) | ((g & 0xFF) << 8) | (b & 0xFF) }
/// Cheap per-coordinate hash → pseudo-random u32 (for stars, bubbles, grass…).
#[inline] fn hash2(x: i32, y: i32) -> u32 {
    let mut h = (x as u32).wrapping_mul(374_761_393).wrapping_add((y as u32).wrapping_mul(668_265_263));
    h = (h ^ (h >> 13)).wrapping_mul(1_274_126_177);
    h ^ (h >> 16)
}
/// Vertical 3-stop gradient fill across [y0,y1): top→mid at the midpoint→bot.
fn vgrad3(fb: &mut Framebuffer, y0: u32, y1: u32, top: (u32,u32,u32), mid: (u32,u32,u32), bot: (u32,u32,u32)) {
    let w = fb.width; if y1 <= y0 { return; }
    let span = (y1 - y0) as u64;
    for y in y0..y1 {
        let t = (y - y0) as u64 * 255 / span.max(1);
        let (r, g, b) = if t < 128 {
            let u = t * 255 / 128;
            (lerp8(top.0, mid.0, u), lerp8(top.1, mid.1, u), lerp8(top.2, mid.2, u))
        } else {
            let u = (t - 128) * 255 / 127;
            (lerp8(mid.0, bot.0, u), lerp8(mid.1, bot.1, u), lerp8(mid.2, bot.2, u))
        };
        fb.fill_rect(0, y, w, 1, rgb(r, g, b));
    }
}

/// Default background — Deep Azure: midnight blue base with azure/teal nebula
/// glows, a rose dingir star, and deep-space vignette. Ubuntu energy.
fn draw_bg_stone(fb: &mut Framebuffer) {
    let w = fb.width; let h = fb.height; let wi = w as i32; let hi = h as i32;
    // Vertical gradient: deep midnight blue top → near-black bottom.
    let mut y = 0u32;
    while y < h {
        let t = y as u64 * 255 / h as u64;
        let r = (0x08u64 + t * 2 / 255) as u32;
        let g = (0x0Cu64 + t * 3 / 255) as u32;
        let b = (0x1Eu64.saturating_sub(t * 8 / 255)) as u32 + 3;
        fb.fill_rect(0, y, w, 1, rgb(r, g, b));
        y += 1;
    }
    // Large azure nebula bloom top-right (ubuntu-style background glow).
    fb.glow(wi*78/100, hi*18/100, wi*32/100, 0x0A2A5A, 70);
    fb.glow(wi*78/100, hi*18/100, wi*14/100, 0x0E4080, 50);
    // Teal/cyan bloom bottom-left.
    fb.glow(wi*12/100, hi*82/100, wi*28/100, 0x0A2D3A, 60);
    fb.glow(wi*12/100, hi*82/100, wi*10/100, 0x0A3A3A, 40);
    // Rose/pink subtle bloom centre-bottom (the accent colour).
    fb.glow(wi*52/100, hi*88/100, wi*18/100, 0x3A0A1A, 45);
    // Dingir constellation stars — now with azure tint.
    for (sx, sy, sr) in [(wi*11/100, hi*28/100, 10), (wi*88/100, hi*80/100, 13),
                         (wi*35/100, hi*68/100, 8), (wi*62/100, hi*11/100, 9)] {
        fb.glow(sx, sy, sr*5, 0x0E2A4A, 55);
        fb.draw_star8(sx, sy, sr, 0x1A4A7A);
    }
    // Corner vignette for depth.
    let vr = wi * 55 / 100;
    for (vx, vy) in [(0,0),(wi,0),(0,hi),(wi,hi)] { fb.glow(vx, vy, vr, 0x03050C, 65); }
}

/// Scatter stars on a coarse grid using hash2 (used by several night scenes).
fn scatter_stars(fb: &mut Framebuffer, y0: u32, y1: u32, density: u32, cell: i32) {
    let w = fb.width as i32;
    let mut gy = y0 as i32;
    while gy < y1 as i32 {
        let mut gx = 0;
        while gx < w {
            let hsh = hash2(gx, gy);
            if hsh % 100 < density {
                let px = gx + (hash2(gx, gy + 1) % cell as u32) as i32;
                let py = gy + (hash2(gx + 1, gy) % cell as u32) as i32;
                let bright = 0x60 + (hsh >> 8) % 0xA0;           // 0x60..0xFF
                let c = rgb(bright, bright, (bright + 0x10).min(0xFF));
                if hsh % 23 == 0 { fb.draw_star8(px, py, 3, c); } // occasional sparkle
                else { fb.set_pixel(px as u32, py as u32, c); if hsh % 5 == 0 { fb.set_pixel(px as u32+1, py as u32, c); } }
            }
            gx += cell;
        }
        gy += cell;
    }
}

/// v1 — Aurora: deep polar night, scattered stars, and flowing green/teal
/// aurora ribbons (soft vertical falloff around sine baselines).
fn draw_bg_aurora(fb: &mut Framebuffer) {
    let w = fb.width; let h = fb.height; let wi = w as i32; let hi = h as i32;
    vgrad3(fb, 0, h, (0x04,0x06,0x10), (0x06,0x0A,0x1A), (0x09,0x12,0x20));
    scatter_stars(fb, 0, h*70/100, 5, 22);
    // Two ribbons: (baseline frac, amplitude frac, thickness, color).
    let ribbons = [(0.34f32, 0.07f32, hi*16/100, 0x2BE08Au32), (0.46f32, 0.05f32, hi*12/100, 0x35B8C8u32)];
    for (basef, ampf, thick, color) in ribbons {
        let sr = (color >> 16) & 0xFF; let sg = (color >> 8) & 0xFF; let sb = color & 0xFF;
        for x in 0..w {
            let phase = x as f32 / wi as f32 * 6.2831 * 1.5;
            let base = hi as f32 * basef + libm::sinf(phase) * hi as f32 * ampf
                     + libm::sinf(phase * 2.3 + 1.0) * hi as f32 * (ampf * 0.4);
            let by = base as i32;
            for dy in -thick..thick {
                let yy = by + dy;
                if yy < 0 || yy >= hi { continue; }
                // brightest at the baseline, fading up/down; brighter at the bottom edge.
                let f = (thick - dy.abs()) as i64 * 200 / thick as i64;
                let a = (f.max(0).min(200)) as u32;
                if a == 0 { continue; }
                let d = fb.get_pixel(x, yy as u32);
                let dr = (d>>16)&0xFF; let dg = (d>>8)&0xFF; let db = d&0xFF;
                fb.set_pixel(x, yy as u32, rgb((sr*a+dr*(255-a))/255, (sg*a+dg*(255-a))/255, (sb*a+db*(255-a))/255));
            }
        }
    }
    fb.glow(wi*30/100, hi*40/100, wi*30/100, 0x103A2A, 30); // faint ground glow
}

/// v2 — Sunset Dusk: indigo→orange→gold sky, a low sun, layered warm hills.
fn draw_bg_sunset(fb: &mut Framebuffer) {
    let w = fb.width; let h = fb.height; let wi = w as i32; let hi = h as i32;
    let horizon = h*66/100;
    vgrad3(fb, 0, horizon, (0x2A,0x1A,0x46), (0xC8,0x55,0x44), (0xF5,0xB0,0x55));
    // Sun — bright disk + halo low on the horizon, right of centre.
    let sx = wi*62/100; let sy = horizon as i32 - hi*4/100;
    fb.glow(sx, sy, wi*22/100, 0xF5D06A, 90);
    fb.glow(sx, sy, wi*9/100, 0xFFF0C0, 120);
    fb.fill_circle(sx, sy, hi*6/100, 0xFFE8A0);
    // Three hill ranges, front darkest (parallax + warm silhouettes).
    let ranges = [(0.70f32, hi*5/100, 0x6A2E3Au32), (0.78f32, hi*7/100, 0x3E1A28u32), (0.86f32, hi*9/100, 0x180A12u32)];
    for (basef, amp, color) in ranges {
        for x in 0..w {
            let phase = x as f32 / wi as f32 * 6.2831;
            let ridge = (hi as f32 * basef + libm::sinf(phase*1.3 + basef*9.0) * amp as f32
                       + libm::sinf(phase*0.6) * (amp as f32 * 0.6)) as u32;
            let top = ridge.min(h);
            if top < h { fb.fill_rect(x, top, 1, h - top, color); }
        }
    }
}

/// v3 — Deep Ocean: aqua→navy depth, angled god-rays, drifting bubbles.
fn draw_bg_ocean(fb: &mut Framebuffer) {
    let w = fb.width; let h = fb.height; let wi = w as i32; let hi = h as i32;
    vgrad3(fb, 0, h, (0x2E,0x9A,0xB8), (0x12,0x52,0x76), (0x04,0x12,0x22));
    // God-rays — slanted bright wedges from the surface.
    for k in 0..5 {
        let ox = wi*(12 + k*20)/100;
        let width = wi*4/100;
        for y in 0..hi*70/100 {
            let slant = y*30/100;
            let cxr = ox + slant;
            let a = (70 - y*70/(hi*70/100)).max(0) as u32; // fade with depth
            if a == 0 { continue; }
            for dx in -width..width {
                let px = cxr + dx;
                if px < 0 || px >= wi { continue; }
                let fa = a * (width - dx.abs()) as u32 / width as u32;
                if fa == 0 { continue; }
                let d = fb.get_pixel(px as u32, y as u32);
                let dr=(d>>16)&0xFF; let dg=(d>>8)&0xFF; let db=d&0xFF;
                let (sr,sg,sb)=(0xD0u32,0xF0,0xFF);
                fb.set_pixel(px as u32, y as u32, rgb((sr*fa+dr*(255-fa))/255,(sg*fa+dg*(255-fa))/255,(sb*fa+db*(255-fa))/255));
            }
        }
    }
    // Bubbles — soft light circles, larger/brighter toward the surface.
    let mut gy = 0; while gy < hi {
        let mut gx = 0; while gx < wi {
            let hsh = hash2(gx*3, gy*3);
            if hsh % 100 < 4 {
                let px = gx + (hsh % 40) as i32; let py = gy + ((hsh>>8)%40) as i32;
                let r = 2 + (hsh>>16)%4;
                fb.glow(px, py, r as i32*3, 0xCFEFFF, 40);
            }
            gx += 46;
        }
        gy += 46;
    }
}

/// v4 — XP "Bliss" homage: rolling green hill, azure cloudy sky, grass texture +
/// dandelions. Drawn from scratch (no copyrighted photo). The nostalgia card. XDXD
fn draw_bg_bliss(fb: &mut Framebuffer) {
    let w = fb.width; let h = fb.height; let wi = w as i32; let hi = h as i32;
    let horizon = (h*60/100).max(1);
    let mut y = 0u32;
    while y < horizon {
        let t = y as u64 * 255 / horizon as u64;
        fb.fill_rect(0, y, w, 1, rgb(lerp8(0x33,0xCF,t), lerp8(0x73,0xE6,t), lerp8(0xBE,0xF6,t)));
        y += 1;
    }
    fb.glow(wi*22/100, hi*15/100, wi*20/100, 0xEFF5FF, 80);
    fb.glow(wi*30/100, hi*19/100, wi*13/100, 0xFFFFFF, 70);
    fb.glow(wi*17/100, hi*23/100, wi*10/100, 0xFFFFFF, 55);
    fb.glow(wi*70/100, hi*12/100, wi*18/100, 0xEAF2FF, 60);
    fb.glow(wi*79/100, hi*20/100, wi*12/100, 0xFFFFFF, 50);
    fb.glow(wi*52/100, hi*27/100, wi*16/100, 0xF3F8FF, 38);
    let cxh = wi*36/100; let half2 = ((wi*66/100) as i64).pow(2).max(1); let amp = hi*14/100;
    for x in 0..w {
        let dx = (x as i32 - cxh) as i64;
        let hump = (1000 - (dx*dx*1000/half2)).max(0) as i32;
        let crest = (horizon as i32 - amp*hump/1000).max(0) as u32;
        let span = (h - crest).max(1) as u64;
        let mut yy = crest;
        while yy < h {
            let d = (yy - crest) as u64;
            let (mut r, mut g, mut b) = if d < 2 { (0xBCu32,0xE0,0x74) }
                else { let tt = d*255/span; (lerp8(0x86,0x2C,tt), lerp8(0xC0,0x60,tt), lerp8(0x3A,0x18,tt)) };
            // Grass texture: fine vertical-blade noise, stronger in the foreground.
            if d >= 2 {
                let n = hash2(x as i32, (yy/3) as i32) % 24;
                let shade = (n as i32) - 12;                 // -12..+11
                let fg = (d as i32 * 100 / span as i32).min(100);   // foreground weight
                let s = shade * fg / 100;
                r = (r as i32 + s).clamp(0,255) as u32;
                g = (g as i32 + s + s/2).clamp(0,255) as u32;
            }
            fb.set_pixel(x, yy, rgb(r, g, b));
            yy += 1;
        }
    }
    // Dandelions — tiny yellow clusters, denser/larger toward the foreground.
    let mut gy = (horizon as i32); while gy < hi {
        let mut gx = 0; while gx < wi {
            let hsh = hash2(gx*7, gy*7);
            let fg = (gy - horizon as i32) * 100 / (hi - horizon as i32).max(1);
            if (hsh % 100) < (3 + fg as u32 / 12) {
                let px = gx + (hsh%30) as i32; let py = gy + ((hsh>>8)%30) as i32;
                let yellow = 0xE8D24A;
                fb.set_pixel(px as u32, py as u32, yellow);
                if fg > 50 { // bigger blooms up close
                    fb.set_pixel((px+1) as u32, py as u32, yellow);
                    fb.set_pixel(px as u32, (py+1) as u32, yellow);
                    fb.set_pixel((px+1) as u32, (py+1) as u32, 0xF5E070);
                }
            }
            gx += 34;
        }
        gy += 34;
    }
}

/// v5 — Nebula Cosmos: deep space, overlapping colourful nebula clouds, a dense
/// starfield of varied brightness.
fn draw_bg_nebula(fb: &mut Framebuffer) {
    let w = fb.width; let h = fb.height; let wi = w as i32; let hi = h as i32;
    let mut y = 0u32;
    while y < h { // near-black with a faint violet wash toward the middle band
        let t = y as u64*255/h as u64;
        fb.fill_rect(0, y, w, 1, rgb(lerp8(0x05,0x0A,t), lerp8(0x04,0x06,t), lerp8(0x0C,0x14,t)));
        y += 1;
    }
    // Nebula pools — overlapping glows in a diagonal cloud band.
    fb.glow(wi*30/100, hi*42/100, wi*30/100, 0x6A2EA0, 55);
    fb.glow(wi*42/100, hi*52/100, wi*24/100, 0xB0408A, 45);
    fb.glow(wi*55/100, hi*38/100, wi*26/100, 0x2E7AC0, 50);
    fb.glow(wi*64/100, hi*55/100, wi*20/100, 0x9A3AD0, 40);
    fb.glow(wi*48/100, hi*46/100, wi*14/100, 0xE070B0, 35);
    fb.glow(wi*20/100, hi*30/100, wi*16/100, 0x203A8A, 30);
    scatter_stars(fb, 0, h, 9, 16);
}

/// v6 — Mountain Dawn: dawn sky, a low sun, four mountain ranges with
/// atmospheric perspective (hazier/lighter as they recede) and mist bands.
fn draw_bg_mountains(fb: &mut Framebuffer) {
    let w = fb.width; let h = fb.height; let wi = w as i32; let hi = h as i32;
    let horizon = h*72/100;
    vgrad3(fb, 0, horizon, (0x8A,0xA8,0xD0), (0xE0,0xA8,0xB0), (0xF7,0xD8,0xB0));
    let sx = wi*40/100; let sy = horizon as i32 - hi*6/100;
    fb.glow(sx, sy, wi*20/100, 0xFFE0B0, 80);
    fb.glow(sx, sy, wi*7/100, 0xFFF4D8, 110);
    // Ranges back→front: lighter/hazier (sky-tinted) in back, darker in front.
    let ranges = [
        (0.50f32, hi*8/100,  0xB9C2D6u32, 4.0f32),
        (0.58f32, hi*11/100, 0x97A2BEu32, 3.0f32),
        (0.66f32, hi*13/100, 0x6E7796u32, 2.2f32),
        (0.74f32, hi*16/100, 0x434C68u32, 1.6f32),
    ];
    for (basef, amp, color, freq) in ranges {
        for x in 0..w {
            let ph = x as f32 / wi as f32 * 6.2831 * freq;
            let jag = libm::sinf(ph) * 0.6 + libm::sinf(ph*2.7+basef*5.0)*0.3
                    + ((hash2(x as i32/8, (basef*100.0) as i32) % 100) as f32/100.0 - 0.5)*0.3;
            let ridge = (hi as f32 * basef + jag * amp as f32) as u32;
            let top = ridge.min(h);
            if top < h { fb.fill_rect(x, top, 1, h - top, color); }
        }
        // mist band hugging the base of each range
        fb.glow(wi/2, (hi as f32*basef) as i32 + amp, wi*60/100, 0xE8E0E0, 14);
    }
}

/// v7 — Sakura Mist: a calm lavender→pink→cream wash with soft bokeh petals and
/// a gentle vignette. The serene one.
fn draw_bg_sakura(fb: &mut Framebuffer) {
    let w = fb.width; let h = fb.height; let wi = w as i32; let hi = h as i32;
    vgrad3(fb, 0, h, (0x5E,0x52,0x80), (0xC8,0x88,0xA8), (0xF2,0xDD,0xE4));
    // Bokeh petals — soft circles of varied size (depth via size + alpha).
    let mut gy = 0; while gy < hi {
        let mut gx = 0; while gx < wi {
            let hsh = hash2(gx*5, gy*5);
            if hsh % 100 < 7 {
                let px = gx + (hsh%60) as i32; let py = gy + ((hsh>>8)%60) as i32;
                let r = 4 + (hsh>>16)%18;
                let pink = if hsh % 3 == 0 { 0xFFFFFF } else { 0xF8C8DC };
                fb.glow(px, py, r as i32, pink, 30 + (hsh>>20)%40);
            }
            gx += 64;
        }
        gy += 64;
    }
    let vr = wi*60/100;
    for (vx, vy) in [(0,0),(wi,0),(0,hi),(wi,hi)] { fb.glow(vx, vy, vr, 0x3A2E40, 35); }
}

fn draw_scene_static_v(fb: &mut Framebuffer, variant: u8) {
    let w = fb.width; let h = fb.height;
    let ptop = panel_top(h);

    // Wallpaper gallery — 8 procedural system backgrounds with depth. Cycle via
    // right-click desktop → Change Background. v4 = the XP "Bliss" easter egg.
    let _ = (w, h);
    match variant {
        1 => draw_bg_aurora(fb),
        2 => draw_bg_sunset(fb),
        3 => draw_bg_ocean(fb),
        4 => draw_bg_bliss(fb),
        5 => draw_bg_nebula(fb),
        6 => draw_bg_mountains(fb),
        7 => draw_bg_sakura(fb),
        8 => draw_bg_custom(fb),     // user image from VFS "wallpaper.ppm"
        _ => draw_bg_stone(fb),
    }

    // Centered hero — floating text on the wallpaper, NO card box (matches the
    // mockup #hero: transparent, pointer-events:none). Big dingir, then
    // "Rusty Penguin" with Penguin in green, then two tagline lines.
    let cx = w as i32 / 2;
    let hero_cy = ptop.max(0) / 2;
    // Ambient depth: a soft warm light pool lifts the hero off the wall, and a
    // cream halo makes the dingir luminous. (Also improves hero legibility over
    // any wallpaper, including the Bliss easter egg.)
    // Rusty Penguin gear+penguin logo — centered in the upper portion of the hero.
    // Text goes BELOW the logo, not overlapping it.
    let logo_size = ((w.min(h) * 36 / 100) as u32).min(320);
    let logo_top = (hero_cy as u32).saturating_sub(logo_size / 2 + logo_size / 8);
    let lx = (cx as u32).saturating_sub(logo_size / 2);
    draw_ppm_watermark(fb, "bin/rp_watermark.ppm", lx, logo_top, logo_size);

    // Text block sits below the logo with comfortable breathing room.
    let text_y = (logo_top + logo_size) as i32 + 18;
    fb.fill_rect_s(cx - 92, text_y - 4, 184, 1, 0x2A3548);
    let w1 = Framebuffer::aa_w("Rusty ", crate::fb::AA_L);
    let w2 = Framebuffer::aa_w("Penguin", crate::fb::AA_L);
    let tx = cx - (w1 + w2) / 2;
    fb.draw_aa(tx + 1, text_y + 1, "Rusty ", 0x080B0D, crate::fb::AA_L);
    fb.draw_aa(tx + w1 + 1, text_y + 1, "Penguin", 0x080B0D, crate::fb::AA_L);
    fb.draw_aa(tx, text_y, "Rusty ", WHITE, crate::fb::AA_L);
    fb.draw_aa(tx + w1, text_y, "Penguin", 0x63C7AD, crate::fb::AA_L);

    // ── Bottom panel — frosted-glass floating dock.
    // Drawn as translucent glass over the wallpaper (the warm glows show
    // through) plus a crisp hairline border + top sheen so it reads as a single
    // illuminated physical object hovering above the desktop, not a flush bar.
    let px = PANEL_MARGIN; let pw = w as i32 - 2 * PANEL_MARGIN;
    // TRANSLUCENT GLASS BAR: the wallpaper shows through the dock itself, while the
    // buttons/icons drawn on top stay fully opaque. A subtle shadow sliver below
    // keeps the floating feel without darkening the glass (nothing opaque under it).
    fb.fill_rect_s(px + PANEL_R, ptop + PANEL_H, pw - 2 * PANEL_R, 2, 0x0A0D10);
    fb.fill_rounded_rect_glass(px, ptop, pw, PANEL_H, PANEL_R, PANEL_SOLID, DOCK_ALPHA);
    // hairline border + top light-catch
    draw_round_border(fb, px, ptop, pw, PANEL_H, PANEL_R, PANEL_EDGE);
    fb.fill_rect_s(px + PANEL_R, ptop + 1, pw - 2 * PANEL_R, 1, 0x7A878E);
    fb.fill_rect_s(px + PANEL_R + 4, ptop + PANEL_H - 2, pw - 2 * PANEL_R - 8, 1, 0x151A1E);

    // Menu button — clean layout: large dingir left, "Menu" right, no permanent underline.
    // The green active-indicator underline is drawn per-frame only when start_menu_open=true.
    let (mbx, mby, mbw, _mbh) = menu_btn_rect(h);
    // Subtle button body — shadow inset by 2px so it doesn't bleed past the border.
    fb.fill_rounded_rect(mbx + 2, mby + 2, mbw - 2, PANEL_H - 16, 11, 0x0A0D10);
    fb.fill_rounded_rect_glass(mbx, mby, mbw, PANEL_H - 14, 12, 0x303940, 202);
    draw_round_border(fb, mbx, mby, mbw, PANEL_H - 14, 12, 0x58656C);
    fb.fill_rect_s(mbx + 12, mby + 1, mbw - 24, 1, 0x7B878E);
    // Dingir icon — full quality, 36px render of the 64x64 source.
    let star_cy = mby + (PANEL_H - 14) / 2;
    let icon_drawn = draw_ppm_icon_centered(fb, "bin/dingir.ppm", mbx + 23, star_cy, 40);
    if !icon_drawn { fb.draw_star8(mbx + 23, star_cy, 16, TEAL); }
    // "Menu" label
    let label_y = star_cy - 6;
    fb.draw_aa(mbx + 48, label_y, "Menu", 0xE3ECEE, crate::fb::AA_S);
    // separator
    fb.fill_rect_s(mbx + mbw + 8, ptop + 14, 1, PANEL_H - 28, 0x435059);

    // Favourites (horizontal app icons)
    draw_desktop_icons(fb, None);
}

// Crisp 1px rounded border (no fill) — all four sides plus corner quarter-arcs.
fn draw_round_border(fb: &mut Framebuffer, x: i32, y: i32, w: i32, h: i32, r: i32, color: u32) {
    let r = r.min(w / 2).min(h / 2).max(0);
    // Straight edges (skip the corner arc spans)
    fb.fill_rect_s(x + r, y,         w - 2 * r, 1, color);  // top
    fb.fill_rect_s(x + r, y + h - 1, w - 2 * r, 1, color);  // bottom
    fb.fill_rect_s(x,         y + r, 1, h - 2 * r, color);  // left
    fb.fill_rect_s(x + w - 1, y + r, 1, h - 2 * r, color);  // right
    // Corner arcs — midpoint circle, one pixel per column in the arc quadrant.
    // For each dx from 0..r, plot the outermost pixel on the circle arc.
    let cx_l = x + r;         let cy_t = y + r;
    let cx_r = x + w - 1 - r; let cy_b = y + h - 1 - r;
    let mut dx = r; let mut dy = 0i32;
    let mut err = 1 - r;
    while dx >= dy {
        let px = |cx: i32, ox: i32| (cx + ox) as u32;
        let py = |cy: i32, oy: i32| (cy + oy) as u32;
        // Top-left quadrant
        fb.set_pixel(px(cx_l, -dx), py(cy_t, -dy), color);
        fb.set_pixel(px(cx_l, -dy), py(cy_t, -dx), color);
        // Top-right
        fb.set_pixel(px(cx_r,  dx), py(cy_t, -dy), color);
        fb.set_pixel(px(cx_r,  dy), py(cy_t, -dx), color);
        // Bottom-left
        fb.set_pixel(px(cx_l, -dx), py(cy_b,  dy), color);
        fb.set_pixel(px(cx_l, -dy), py(cy_b,  dx), color);
        // Bottom-right
        fb.set_pixel(px(cx_r,  dx), py(cy_b,  dy), color);
        fb.set_pixel(px(cx_r,  dy), py(cy_b,  dx), color);
        dy += 1;
        if err < 0 { err += 2 * dy + 1; }
        else { dx -= 1; err += 2 * (dy - dx) + 1; }
    }
}

fn trit_indicator(ticks: u64) -> [u8; 7] {
    // 4-trit cycling indicator: T[+--+], changes ~every 300ms (30 ticks)
    let phase = ticks / 30;
    let mut out = *b"T[+--+]";
    for i in 0..4u64 {
        out[2 + i as usize] = match (phase + i) % 3 { 0 => b'-', 1 => b'0', _ => b'+' };
    }
    out
}

// Clickable bounds of the keyboard-layout pill, recorded each draw so the click
// handler can toggle EN/DE without opening the quick-settings panel.
static mut KBD_PILL: (i32, i32, i32, i32) = (0, 0, 0, 0);
fn kbd_pill_rect() -> (i32, i32, i32, i32) { unsafe { KBD_PILL } }
fn kbd_pill_hit(cx: i32, cy: i32) -> bool {
    let (x, y, w, hh) = kbd_pill_rect();
    w > 0 && cx >= x && cx < x + w && cy >= y && cy < y + hh
}

// Virtual desktop workspace dot click targets (full-panel-height hit zones for
// easy clicking). Recorded each draw by draw_topbar.
const N_DESKTOPS: usize = 4;
static mut WS_DOT_RECTS: [(i32,i32,i32,i32); N_DESKTOPS] = [(0,0,0,0); N_DESKTOPS];
fn ws_dot_hit(cx: i32, cy: i32) -> Option<usize> {
    for (d, &(x, y, w, hh)) in unsafe { WS_DOT_RECTS }.iter().enumerate() {
        if w > 0 && cx >= x && cx < x + w && cy >= y && cy < y + hh { return Some(d); }
    }
    None
}

// Battery glyph: outline body + nub + a fill bar proportional to charge. When on
// AC (no battery) we draw a small bolt instead of a fill — a real "how full" symbol.
fn draw_battery_glyph(fb: &mut Framebuffer, x: i32, y: i32, pct: u32, has_batt: bool, fill: u32) {
    let bw = 20; let bh = 11; let out = 0x7A848B;  // graphite outline
    fb.fill_rect_s(x, y, bw, 1, out);
    fb.fill_rect_s(x, y + bh - 1, bw, 1, out);
    fb.fill_rect_s(x, y, 1, bh, out);
    fb.fill_rect_s(x + bw - 1, y, 1, bh, out);
    fb.fill_rect_s(x + bw, y + 3, 2, 5, out);              // positive terminal nub
    if has_batt {
        let fw = ((bw - 4) * pct as i32 / 100).max(0).min(bw - 4);
        fb.fill_rect_s(x + 2, y + 2, fw, bh - 4, fill);
    } else {
        // lightning bolt for AC power (teal)
        fb.fill_rect_s(x + 9, y + 2, 2, 4, wm::ACCENT_GREEN);
        fb.fill_rect_s(x + 7, y + 5, 6, 1, wm::ACCENT_GREEN);
        fb.fill_rect_s(x + 9, y + 5, 2, 4, wm::ACCENT_GREEN);
    }
}

// Memory glyph: a little RAM-stick chip with contact pins.
fn draw_mem_glyph(fb: &mut Framebuffer, x: i32, y: i32, col: u32) {
    fb.fill_rect_s(x, y, 14, 9, 0x39443E);                 // chip body
    fb.fill_rect_s(x + 2, y + 2, 10, 1, col);              // top trace
    fb.fill_rect_s(x + 2, y + 4, 10, 1, 0x6F796F);
    for i in 0..4 { fb.fill_rect_s(x + 2 + i * 3, y + 9, 2, 2, col); } // pins
}

fn draw_topbar(fb: &mut Framebuffer, time: &str, s: &SysStats, ticks: u64, current_desktop: usize) {
    // Bottom-panel TRAY (right side): keyboard layout · memory · battery · clock.
    // Real system stats with real symbols — no animated cells (the old ternary
    // bus repainted every frame and made the desktop feel laggy). Static content
    // means the tray only repaints when a value or the clock actually changes.
    let w = fb.width; let h = fb.height;
    let ptop = panel_top(h);
    let pr = PANEL_MARGIN + (w as i32 - 2 * PANEL_MARGIN); // panel right edge
    let ty = ptop + 7;
    let tray_w = 440;  // wider to accommodate workspace dots
    let trx = (pr - tray_w - 6).max(PANEL_MARGIN + 8);
    // Restore the cached glass dock (not an opaque fill) so the bar stays
    // translucent; the tray text/icons are then drawn on top.
    fb.restore_bg_rect(trx.max(0) as u32, ty.max(0) as u32, (pr - trx - 6).max(0) as u32, 40);

    let cyy = ptop + (PANEL_H - 12) / 2;          // vertical centre of the 12px row
    let txt_top = cyy - 1;                         // AA_T baseline sits on that row
    const GAP: i32 = 14;

    // Clock — right-aligned, owns the edge.
    let clk_w = Framebuffer::aa_w(time, crate::fb::AA_T);
    let clk_x = (pr - 12 - clk_w).max(trx + 4);
    fb.draw_aa(clk_x, txt_top, time, WHITE, crate::fb::AA_T);

    // Battery — icon with proportional fill + percentage (or AC bolt + "AC").
    // Charging adds a "+" prefix. No-battery (QEMU, desktops) shows "AC" correctly.
    let batt = unsafe { sys_battery_pct() };
    let charging = unsafe { sys_battery_charging() };
    let has_batt = batt <= 100;
    let bat_col = if !has_batt { wm::ACCENT_GREEN }
                  else if batt < 15 { wm::TRIT_NEG }
                  else if batt < 30 { AMBER }
                  else if charging  { 0x63D4C8 }   // charging: lighter teal
                  else { wm::TRIT_POS };
    let mut bb = Strbuf::new();
    if has_batt {
        if charging { bb.push(b'+'); }
        bb.push_u64(batt as u64); bb.push(b'%');
    } else {
        for c in b"AC" { bb.push(*c); }
    }
    let bs = bb.as_str();
    let bval_w = Framebuffer::aa_w(bs, crate::fb::AA_T);
    let bat_group = 22 + 5 + bval_w;
    let bat_x = (clk_x - GAP - bat_group).max(trx + 4);
    draw_battery_glyph(fb, bat_x, cyy + 1, batt.min(100), has_batt, bat_col);
    fb.draw_aa(bat_x + 22 + 5, txt_top, bs, bat_col, crate::fb::AA_T);

    // Memory — chip icon + percentage.
    let mem_col = if s.mem_pct > 80 { TRIT_NEG } else if s.mem_pct > 60 { AMBER } else { GREEN };
    let mut mp = Strbuf::new(); mp.push_u64(s.mem_pct as u64); mp.push(b'%');
    let mp_s = mp.as_str();
    let mval_w = Framebuffer::aa_w(mp_s, crate::fb::AA_T);
    let mem_group = 14 + 5 + mval_w;
    let mem_x = (bat_x - GAP - mem_group).max(trx + 4);
    draw_mem_glyph(fb, mem_x, cyy + 1, mem_col);
    fb.draw_aa(mem_x + 14 + 5, txt_top, mp_s, mem_col, crate::fb::AA_T);

    // Keyboard layout pill (EN / DE) — clickable to toggle.
    let de = unsafe { sys_kbd_layout(0) } == 1;
    let kbl = if de { "DE" } else { "EN" };
    let kb_w = Framebuffer::aa_w(kbl, crate::fb::AA_T);
    let pill_w = kb_w + 16;

    // Workspace dots — 4 small rounded squares, one per virtual desktop.
    // Drawn to the left of the keyboard pill. Current desktop = lit green; others = dim.
    const DOT_SZ: i32 = 10;
    const DOT_GAP: i32 = 5;
    let ws_group_w = N_DESKTOPS as i32 * DOT_SZ + (N_DESKTOPS as i32 - 1) * DOT_GAP;
    // keyboard pill sits left of memory; ws dots sit left of keyboard pill
    let pill_x = (mem_x - GAP - pill_w).max(trx + 4);
    let ws_x = (pill_x - GAP - ws_group_w).max(trx + 4);
    for d in 0..N_DESKTOPS {
        let dx = ws_x + d as i32 * (DOT_SZ + DOT_GAP);
        let active = d == current_desktop;
        let dot_col = if active { tint(GREEN, 130) } else { 0x2A3540 };
        fb.fill_rounded_rect(dx, cyy - 1, DOT_SZ, DOT_SZ, 3, dot_col);
        if active { draw_round_border(fb, dx, cyy - 1, DOT_SZ, DOT_SZ, 3, GREEN); }
        // Full panel-height hit zone so it's easy to click.
        unsafe { WS_DOT_RECTS[d] = (dx, ptop, DOT_SZ, PANEL_H); }
    }

    fb.fill_rounded_rect(pill_x, cyy - 2, pill_w, 16, 6, 0x2B343A);
    fb.fill_rect_s(pill_x + 6, cyy - 1, pill_w - 12, 1, 0x6B7A82);
    fb.fill_rect_s(pill_x + 6, cyy + 13, pill_w - 12, 1, 0x171C21);
    fb.draw_aa(pill_x + 8, txt_top, kbl, 0xDDE7EA, crate::fb::AA_T);
    unsafe { KBD_PILL = (pill_x, ptop, pill_w, PANEL_H); }

    let _ = ticks;
}

// ---- Launcher buttons ───────────────────────────────────────────────────────

struct Launcher { label: &'static str, cmd: Option<&'static str>, title: &'static str, color: u32 }
const LAUNCHERS: &[Launcher] = &[
    Launcher { label: " psh ", cmd: None,                title: "Terminal",         color: GREEN },
    Launcher { label: "files", cmd: Some("ls -la\n"),    title: "Files",            color: BLUE  },
    Launcher { label: "edit ", cmd: Some("nano\n"),      title: "Text Editor",      color: AMBER },
    Launcher { label: " mem ", cmd: Some("mem\n"),       title: "Memory",           color: TEAL  },
    Launcher { label: " ps  ", cmd: Some("ps\n"),        title: "Processes",        color: 0xA0D0FF },
    Launcher { label: " ai  ", cmd: Some("ai 32\n"),     title: "TIS Inference",    color: 0xFFD700 },
    Launcher { label: "phone", cmd: Some("wine\n"),      title: "RustyPhone",       color: 0x22C55E },
];


// ---- Desktop icon shortcuts ─────────────────────────────────────────────────
// Fixed 4 icons. Each is 72×64px: 48px image area + 8px label + 8px margin.

struct DesktopIcon {
    label: &'static str,
    icon: usize,      // index into the HD icon atlas (icons.rs)
    color: u32,
    launcher_idx: usize,
}

const DESKTOP_ICONS: &[DesktopIcon] = &[
    DesktopIcon { label: "Terminal",    icon: icons::IC_TERM,     color: GREEN,    launcher_idx: 0 },
    DesktopIcon { label: "Files",       icon: icons::IC_FILES,    color: BLUE,     launcher_idx: 1 },
    DesktopIcon { label: "Text Editor", icon: icons::IC_EDIT,     color: AMBER,    launcher_idx: 2 },
    DesktopIcon { label: "Calculator",  icon: icons::IC_CALC,     color: 0xFFD700, launcher_idx: 7 },
    DesktopIcon { label: "Help",        icon: icons::IC_FILES,    color: 0x90EE90, launcher_idx: 9 },
    DesktopIcon { label: "Settings",    icon: icons::IC_SETTINGS, color: 0x9CA3AF, launcher_idx: 5 },
    DesktopIcon { label: "TIS Console", icon: icons::IC_TIS,      color: 0x4A9EFF, launcher_idx: 6 },
    DesktopIcon { label: "Snake",       icon: icons::IC_SNAKE,    color: 0x4ADE80, launcher_idx: 10 },
    DesktopIcon { label: "Minesweeper", icon: icons::IC_MINES,    color: 0xFCD34D, launcher_idx: 11 },
    DesktopIcon { label: "Doom",        icon: icons::IC_DOOM,     color: 0xEF4444, launcher_idx: 12 },
    DesktopIcon { label: "Web",         icon: icons::IC_WEB,      color: 0x8CC6E5, launcher_idx: 13 }, // index 10
    DesktopIcon { label: "RustyPhone",  icon: icons::IC_PHONE,    color: 0x22C55E, launcher_idx: 99 }, // index 11
    DesktopIcon { label: "Notes",       icon: icons::IC_NOTES,    color: AMBER,    launcher_idx: 98 }, // index 12
];

// Favourites are horizontal 40×40 tiles inside the bottom panel (see fav_rect).
// Each tile is an accent-tinted rounded square with the app icon in its accent
// color, like the mockup's .favbtn .fi. `hover_icon` carries a DESKTOP_ICONS
// index (what desktop_icon_hit returns).
fn draw_desktop_icons(fb: &mut Framebuffer, hover_icon: Option<usize>) {
    let h = fb.height;
    for slot in 0..N_FAV {
        let icon = &DESKTOP_ICONS[FAV_IDX[slot]];
        let (x, y, tw, th) = fav_rect(slot, h);
        let hovered = hover_icon == Some(FAV_IDX[slot]);
        // Clean solid colored tile — tint at 110 gives a medium-dark colored
        // background (≈43% accent + dark base) matching the original icon style.
        // No glass layers, no sheen lines, no borders — those read as noise at 40px.
        let bg = tint(icon.color, if hovered { 150 } else { 110 });
        fb.fill_rounded_rect(x, y, tw, th, 12, bg);
        let isz = icons::ICON_PX as i32;
        fb.draw_icon(x + (tw - isz) / 2, y + (th - isz) / 2, icon.icon, icon.color);
    }
}

// tint: blend an accent color toward the dark glass body at the given alpha
// (0..255 = how much accent), producing the mockup's color-mix(accent 16%) look.
fn tint(accent: u32, alpha: u32) -> u32 {
    let base = 0x2B3237u32;
    let a = alpha.min(255); let ia = 255 - a;
    let mix = |sh: u32| (((accent >> sh) & 0xFF) * a + ((base >> sh) & 0xFF) * ia) / 255;
    (mix(16) << 16) | (mix(8) << 8) | mix(0)
}

fn desktop_icon_hit(mx: i32, my: i32, h: u32) -> Option<usize> {
    for slot in 0..N_FAV {
        let (x, y, tw, th) = fav_rect(slot, h);
        if mx >= x && mx < x + tw && my >= y && my < y + th { return Some(FAV_IDX[slot]); }
    }
    None
}


// ---- Taskbar window buttons ─────────────────────────────────────────────────

// Running-window task buttons live INSIDE the bottom dock, in the tasks area
// after the favourites (and before the right tray), at full panel-row height.
fn tasks_start_x(fh: u32) -> i32 {
    let (lx, _, _, _) = fav_rect(N_FAV - 1, fh); // last favourite slot
    lx + FAV_TILE + 18  // + separator gap
}
fn tbwin_rect(_fw: u32, fh: u32, slot: usize) -> (i32, i32, i32, i32) {
    (tasks_start_x(fh) + slot as i32 * 116, panel_top(fh) + 7, 108, 40)
}

fn draw_taskbar_win_btns(fb: &mut Framebuffer, term_wins: &[TermWin], current_desktop: usize) {
    let fw = fb.width; let fh = fb.height;
    let ptop = panel_top(fh);
    let pr = PANEL_MARGIN + (fw as i32 - 2 * PANEL_MARGIN);
    let tray_left = pr - 426;            // wider tray (ws dots added)
    let tx0 = tasks_start_x(fh);
    // Clear the tasks strip each frame (so closed windows leave no ghost) by
    // restoring the cached glass dock — keeps the bar translucent.
    if tray_left > tx0 { fb.restore_bg_rect(tx0.max(0) as u32, (ptop + 7).max(0) as u32, (tray_left - tx0).max(0) as u32, 40); }

    // Only show windows on the current virtual desktop.
    // The focused window is the last one in `wins` that is on this desktop.
    let focused_orig_idx = term_wins.iter().enumerate().rev()
        .find(|(_, tw)| tw.win.desktop == current_desktop)
        .map(|(i, _)| i);

    let mut slot = 0usize;
    for (orig_idx, tw) in term_wins.iter().enumerate() {
        if tw.win.desktop != current_desktop { continue; }
        let (x, y, w, h) = tbwin_rect(fw, fh, slot);
        if x + w > tray_left { break; }  // out of room → stop
        let is_focused   = Some(orig_idx) == focused_orig_idx;
        let is_minimized = tw.win.minimized;
        let bg     = if is_minimized { 0x262D32u32 } else { 0x343D44u32 };
        let txt    = if is_minimized { 0xAAB3B8u32 } else { WHITE };
        let accent = if is_focused { GREEN } else { TRIT_ZERO };
        fb.fill_rounded_rect(x + 1, y + 1, w, h, 12, 0x0A0D10);
        fb.fill_rounded_rect(x, y, w, h, 12, bg);
        fb.fill_rect_s(x + 10, y + 1, w - 20, 1, 0x72808A);
        fb.fill_rect_s(x + 10, y + h - 2, w - 20, 1, 0x1A1F24);
        // running/focus dot
        fb.fill_circle(x + 11, y + h / 2, 3, accent);
        // window title (truncated)
        let title = tw.win.title.as_str();
        let short = title.find(" - ").map(|i| &title[..i]).unwrap_or(title);
        let max_chars = ((w - 28) / 8).max(0) as usize;
        let lbl = &short[..max_chars.min(short.len())];
        fb.draw_aa(x + 22, y + 12, lbl, txt, crate::fb::AA_T);
        // focus underline
        if is_focused { fb.fill_rect_s(x + 9, y + h - 5, w - 18, 2, TEAL); }
        slot += 1;
    }
}

fn draw_taskbar_clock(fb: &mut Framebuffer, time_str: &str) {
    let fw = fb.width; let tb_y = fb.height - 28;
    // Extract HH:MM from time_str (find first ':' then take 5 chars back 2)
    let hhmm = if let Some(pos) = time_str.find(':') {
        let start = pos.saturating_sub(2);
        let end = (start + 5).min(time_str.len());
        &time_str[start..end]
    } else { "" };
    if hhmm.is_empty() { return; }
    let tw = hhmm.len() as u32 * 8;
    let x = fw - tw - 10;
    // Clear the clock area and redraw
    fb.fill_rect(x - 3, tb_y + 5, tw + 6, 18, TASKBAR);
    fb.fill_rounded_rect(x as i32 - 8, tb_y as i32 + 3, tw as i32 + 16, 24, 10, 0x0A0E12);
    fb.fill_rounded_rect(x as i32 - 7, tb_y as i32 + 4, tw as i32 + 14, 22, 9, 0x293137);
    fb.fill_rect_s(x as i32 - 2, tb_y as i32 + 4, tw as i32 + 4, 1, 0x6F7B83);
    fb.fill_rect_s(x as i32 - 2, tb_y as i32 + 24, tw as i32 + 4, 1, 0x181D22);
    fb.draw_str(x, tb_y + 10, hhmm, WHITE, 0x293137);
}

fn tbwin_hit(fw: u32, fh: u32, wins: &[TermWin], mx: i32, my: i32, current_desktop: usize) -> Option<usize> {
    let mut slot = 0usize;
    for (orig_idx, tw) in wins.iter().enumerate() {
        if tw.win.desktop != current_desktop { continue; }
        let (x, y, w, h) = tbwin_rect(fw, fh, slot);
        if mx >= x && mx < x + w && my >= y && my < y + h { return Some(orig_idx); }
        slot += 1;
    }
    None
}

// ---- Start menu ─────────────────────────────────────────────────────────────

fn dingir_hit(fh: u32, mx: i32, my: i32) -> bool {
    // Whole "Menu" button (bottom panel, left) is clickable.
    let (x, y, w, ht) = menu_btn_rect(fh);
    mx >= x && mx < x + w && my >= y && my < y + ht
}

fn show_desktop_hit(fw: u32, fh: u32, mx: i32, my: i32) -> bool {
    mx >= fw as i32 - 6 && mx < fw as i32
        && my >= fh as i32 - 28 && my < fh as i32
}

// ── Quick Settings panel (GNOME-style connections / toggles) ─────────────────
// A frosted-glass dropdown anchored above the tray. Click the tray to open it.
// Volume and Dark Style are wired to real state (audio syscall / wallpaper); the
// Wi-Fi / Bluetooth / DND / Airplane / Night Light tiles hold state and are the
// surface for the per-chip drivers as they land (bare-metal radio is a separate
// hardware brick — see project memory). Honest, not faked.
struct QuickSettings {
    wifi: bool, bluetooth: bool, dark: bool, dnd: bool, airplane: bool, nightlight: bool,
    volume: u8,
    brightness: u8,
}
static mut QS: QuickSettings = QuickSettings {
    wifi: true, bluetooth: false, dark: false, dnd: false, airplane: false, nightlight: false,
    volume: 80,
    brightness: 100,
};
static mut QS_OPEN: bool = false;

const QS_W: i32 = 320;
const QS_H: i32 = 332;
const QS_TILE_H: i32 = 50;
const QS_PAD: i32 = 14;

const QS_LABELS: [&str; 6] = ["Wi-Fi", "Bluetooth", "Dark Style", "Do Not Disturb", "Airplane", "Night Light"];

fn qs_rect(fh: u32, fw: u32) -> (i32, i32, i32, i32) {
    let pr = PANEL_MARGIN + (fw as i32 - 2 * PANEL_MARGIN);
    let x = pr - QS_W - 6;
    let y = panel_top(fh) - QS_H - 10;
    (x, y, QS_W, QS_H)
}

fn qs_tile_rect(px: i32, py: i32, idx: usize) -> (i32, i32, i32, i32) {
    let tile_w = (QS_W - QS_PAD * 3) / 2;
    let col = (idx % 2) as i32;
    let row = (idx / 2) as i32;
    let x = px + QS_PAD + col * (tile_w + QS_PAD);
    let y = py + 50 + row * (QS_TILE_H + 12);
    (x, y, tile_w, QS_TILE_H)
}

fn qs_slider_rect(px: i32, py: i32) -> (i32, i32, i32, i32) {
    // below the 3 rows of tiles
    let y = py + 50 + 3 * (QS_TILE_H + 12) + 12;
    (px + QS_PAD + 30, y, QS_W - 2 * QS_PAD - 30, 8)
}

fn qs_bright_slider_rect(px: i32, py: i32) -> (i32, i32, i32, i32) {
    // one row below the volume slider
    let (x, y, w, h) = qs_slider_rect(px, py);
    (x, y + 30, w, h)
}

fn qs_bright_slider_hit(px: i32, py: i32, cx: i32, cy: i32) -> Option<u8> {
    let (x, y, w, h) = qs_bright_slider_rect(px, py);
    if cx >= x - 6 && cx < x + w + 6 && cy >= y - 10 && cy < y + h + 10 {
        let v = ((cx - x).max(0).min(w) * 100 / w) as u8;
        return Some(v);
    }
    None
}

fn qs_tile_hit(px: i32, py: i32, cx: i32, cy: i32) -> Option<usize> {
    for i in 0..6 {
        let (x, y, w, h) = qs_tile_rect(px, py, i);
        if cx >= x && cx < x + w && cy >= y && cy < y + h { return Some(i); }
    }
    None
}

fn qs_slider_hit(px: i32, py: i32, cx: i32, cy: i32) -> Option<u8> {
    let (x, y, w, h) = qs_slider_rect(px, py);
    if cx >= x - 6 && cx < x + w + 6 && cy >= y - 10 && cy < y + h + 10 {
        let v = ((cx - x).max(0).min(w) * 100 / w) as u8;
        return Some(v);
    }
    None
}

unsafe fn qs_toggle(idx: usize) {
    match idx {
        0 => QS.wifi = !QS.wifi,
        1 => QS.bluetooth = !QS.bluetooth,
        2 => QS.dark = !QS.dark,
        3 => QS.dnd = !QS.dnd,
        4 => QS.airplane = !QS.airplane,
        5 => QS.nightlight = !QS.nightlight,
        _ => {}
    }
}

unsafe fn set_master_volume(vol: u8) {
    core::arch::asm!(
        "syscall",
        in("rax") 18u64,
        in("rdi") vol as u64,
        out("rcx") _, out("r11") _,
        options(nostack),
    );
}

fn tray_hit(fw: u32, fh: u32, mx: i32, my: i32) -> bool {
    let pr = PANEL_MARGIN + (fw as i32 - 2 * PANEL_MARGIN);
    let ptop = panel_top(fh);
    mx >= pr - 346 && mx < pr - 4 && my >= ptop && my < ptop + PANEL_H
}

// ── icon primitives (minimalist, drawn from shapes — no glyph font) ──────────
fn qs_icon(fb: &mut Framebuffer, idx: usize, cx: i32, cy: i32, col: u32) {
    match idx {
        0 => { // Wi-Fi — four ascending signal bars
            for b in 0..4i32 {
                let bh = 4 + b * 4;
                fb.fill_rect_s(cx - 9 + b * 6, cy + 8 - bh, 4, bh, col);
            }
        }
        1 => { // Bluetooth — a slim diamond rune
            for dy in -8..=8i32 {
                let span = 8 - dy.abs();
                if span > 0 { fb.fill_rect_s(cx - span / 2, cy + dy, span.max(1), 1, col); }
            }
            fb.fill_rect_s(cx, cy - 8, 1, 17, col);
        }
        2 => { // Dark Style — crescent moon (carve an offset disc out of a disc)
            fb.fill_circle(cx, cy, 9, col);
            fb.fill_circle(cx + 4, cy - 3, 8, QS_TILE_BG);
        }
        3 => { // Do Not Disturb — ring with a diagonal slash
            fb.fill_circle(cx, cy, 9, col);
            fb.fill_circle(cx, cy, 6, QS_TILE_BG);
            for t in -8..=8i32 { fb.fill_rect_s(cx + t, cy - t, 2, 2, col); }
        }
        4 => { // Airplane — a simple plus-wing silhouette
            fb.fill_rect_s(cx - 1, cy - 9, 3, 18, col);  // fuselage
            fb.fill_rect_s(cx - 9, cy - 1, 18, 3, col);  // wings
            fb.fill_rect_s(cx - 4, cy + 6, 9, 2, col);   // tailplane
        }
        5 => { // Night Light — sun-warm dot with short rays
            fb.fill_circle(cx, cy, 5, col);
            for (dx, dy) in [(-10,0),(10,0),(0,-10),(0,10),(-7,-7),(7,7),(-7,7),(7,-7)] {
                fb.fill_rect_s(cx + dx, cy + dy, 2, 2, col);
            }
        }
        _ => {}
    }
}

const QS_TILE_BG: u32 = 0x1E2620;   // off — warm-stone dark
const QS_TILE_ON: u32 = 0x2E7D4F;   // on  — spring-green

fn draw_quick_settings(fb: &mut Framebuffer) {
    let (px, py, pw, ph) = qs_rect(fb.height, fb.width);
    // Aero shadow + frosted body
    fb.fill_rounded_rect(px + 4, py + 7, pw, ph, 16, 0x070A08);
    fb.fill_rounded_rect(px + 1, py + 3, pw, ph, 15, 0x0C100E);
    fb.fill_rounded_rect_glass(px, py, pw, ph, 14, 0x222A24, 236);
    // top light-catch
    fb.fill_rect_s(px + 14, py + 1, pw - 28, 1, 0x3C5040);

    // Title row + power status
    fb.draw_aa(px + QS_PAD, py + 14, "Quick Settings", WHITE, crate::fb::AA_S);
    let batt = unsafe { sys_battery_pct() };
    let pw_lbl: &str = if batt <= 100 { "" } else { "AC" };
    if batt <= 100 {
        let mut bb = Strbuf::new(); bb.push_u64(batt as u64); bb.push(b'%');
        let bs = bb.as_str();
        let bwid = Framebuffer::aa_w(bs, crate::fb::AA_S);
        fb.draw_aa(px + pw - QS_PAD - bwid, py + 14, bs, 0x6FE18B, crate::fb::AA_S);
    } else {
        let bwid = Framebuffer::aa_w(pw_lbl, crate::fb::AA_S);
        fb.draw_aa(px + pw - QS_PAD - bwid, py + 14, pw_lbl, 0x6FE18B, crate::fb::AA_S);
    }

    // Toggle tiles
    let states = unsafe { [QS.wifi, QS.bluetooth, QS.dark, QS.dnd, QS.airplane, QS.nightlight] };
    for i in 0..6 {
        let (x, y, w, h) = qs_tile_rect(px, py, i);
        let on = states[i];
        let bg = if on { QS_TILE_ON } else { QS_TILE_BG };
        fb.fill_rounded_rect(x, y, w, h, 10, bg);
        if on { fb.fill_rect_s(x + 10, y + 1, w - 20, 1, 0x8FE6B0); }
        let icol = if on { 0xEAF6EE } else { 0x8A948C };
        qs_icon(fb, i, x + 22, y + h / 2, icol);
        let tcol = if on { WHITE } else { 0xB8C2BA };
        fb.draw_aa(x + 40, y + h / 2 - 8, QS_LABELS[i], tcol, crate::fb::AA_T);
        let scol = if on { 0x9CE9BC } else { 0x6B756D };
        fb.draw_aa(x + 40, y + h / 2 + 4, if on { "On" } else { "Off" }, scol, crate::fb::AA_T);
    }

    // Volume slider
    let (sx, sy, sw, _sh) = qs_slider_rect(px, py);
    qs_icon_speaker(fb, sx - 22, sy + 4);
    fb.fill_rounded_rect(sx, sy, sw, 6, 3, 0x141A16);
    let v = unsafe { QS.volume } as i32;
    let fillw = (sw * v / 100).max(0);
    fb.fill_rounded_rect(sx, sy, fillw, 6, 3, QS_TILE_ON);
    fb.fill_circle(sx + fillw, sy + 3, 7, 0xEAF6EE);
    fb.fill_circle(sx + fillw, sy + 3, 5, QS_TILE_ON);

    // Brightness slider
    let (bx, by, bw, _bh) = qs_bright_slider_rect(px, py);
    qs_icon_sun(fb, bx - 22, by + 3);
    fb.fill_rounded_rect(bx, by, bw, 6, 3, 0x141A16);
    let bv = unsafe { QS.brightness } as i32;
    let bfillw = (bw * bv / 100).max(0);
    fb.fill_rounded_rect(bx, by, bfillw, 6, 3, QS_TILE_ON);
    fb.fill_circle(bx + bfillw, by + 3, 7, 0xEAF6EE);
    fb.fill_circle(bx + bfillw, by + 3, 5, QS_TILE_ON);
}

fn qs_icon_sun(fb: &mut Framebuffer, cx: i32, cy: i32) {
    fb.fill_circle(cx, cy, 4, 0xECDAA7);                          // sun core
    for k in 0..8i32 {                                           // 8 rays
        let (dx, dy) = match k {
            0 => (0, -7), 1 => (5, -5), 2 => (7, 0), 3 => (5, 5),
            4 => (0, 7),  5 => (-5, 5), 6 => (-7, 0), _ => (-5, -5),
        };
        fb.fill_rect_s(cx + dx, cy + dy, 1, 1, 0xECDAA7);
    }
}

fn qs_icon_speaker(fb: &mut Framebuffer, cx: i32, cy: i32) {
    fb.fill_rect_s(cx - 6, cy - 3, 3, 6, 0xB8C2BA);          // box
    for dy in -6..=6i32 {                                    // cone
        let span = 6 - dy.abs();
        if span > 0 && dy.abs() <= 6 { fb.fill_rect_s(cx - 3, cy + dy, (span / 2 + 1).min(5), 1, 0xB8C2BA); }
    }
    fb.fill_rect_s(cx + 4, cy - 4, 1, 9, 0x6FE18B);          // wave
}

// ── Start menu ───────────────────────────────────────────────────────────────
// Matches the HTML mockup form: user header · app list with icons+desc ·
// games section · footer. Width 280px, items 34px tall.
#[derive(Copy, Clone)]
enum MenuLaunch { App(u8), Term(usize) }

struct MenuItem {
    label: &'static str,
    desc:  &'static str,
    color: u32,
    icon:  usize,
    kind:  MenuLaunch,
}

// Real apps only — no raw shell commands.
const MENU_ITEMS: &[MenuItem] = &[
    MenuItem { label: "Web",          desc: "Native browser (local pages)", color: 0x8CC6E5, icon: icons::IC_WEB,      kind: MenuLaunch::App(9) },
    MenuItem { label: "Files",        desc: "Browse & manage files",      color: 0x8CC6E5, icon: icons::IC_FILES,    kind: MenuLaunch::App(0) },
    MenuItem { label: "Text Editor",  desc: "Write & edit documents",     color: 0xF5C451, icon: icons::IC_EDIT,     kind: MenuLaunch::App(1) },
    MenuItem { label: "Calculator",   desc: "Ternary arithmetic",         color: 0xFFD700, icon: icons::IC_CALC,     kind: MenuLaunch::App(2) },
    MenuItem { label: "Terminal",     desc: "psh — bare-metal shell",     color: 0x6FE18B, icon: icons::IC_TERM,     kind: MenuLaunch::Term(0) },
    MenuItem { label: "Settings",     desc: "System preferences",         color: 0x9CA3AF, icon: icons::IC_SETTINGS, kind: MenuLaunch::App(4) },
    MenuItem { label: "TIS Console",  desc: "Sparse ternary AI runtime",  color: 0x4A9EFF, icon: icons::IC_TIS,      kind: MenuLaunch::App(5) },
    MenuItem { label: "Kernel Manager", desc: "Bring & boot your own kernel", color: 0x6FE18B, icon: icons::IC_SETTINGS, kind: MenuLaunch::App(10) },
    MenuItem { label: "Sound",          desc: "Audio mixer + tone player",    color: 0x4A9EFF, icon: icons::IC_CALC,     kind: MenuLaunch::App(11) },
    MenuItem { label: "Media Player",   desc: "Video + audio — the founding clip", color: 0xF5C451, icon: icons::IC_MEDIA, kind: MenuLaunch::App(12) },
    MenuItem { label: "Screenshot",     desc: "Capture the screen to a file",  color: 0x8CC6E5, icon: icons::IC_SHOT,  kind: MenuLaunch::App(13) },
    MenuItem { label: "Image Viewer",   desc: "View PPM images & screenshots",  color: 0x8CC6E5, icon: icons::IC_IMAGE, kind: MenuLaunch::App(14) },
    MenuItem { label: "Clock",          desc: "Time, stopwatch, timer, world",  color: 0x6FE18B, icon: icons::IC_CLOCK, kind: MenuLaunch::App(15) },
    MenuItem { label: "Task Manager",   desc: "Processes, CPU & memory",        color: 0x8CC6E5, icon: icons::IC_TASKS, kind: MenuLaunch::App(16) },
    MenuItem { label: "Notes",          desc: "Quick notes — Ctrl+S to save",   color: 0xF5C451, icon: icons::IC_EDIT,  kind: MenuLaunch::App(17) },
    MenuItem { label: "RustyPhone",    desc: "SIP dialer + phone verification", color: 0x22C55E, icon: icons::IC_TERM,  kind: MenuLaunch::App(18) },
    // games start at index 14
    MenuItem { label: "Snake",        desc: "Classic arcade",             color: 0x4ADE80, icon: icons::IC_SNAKE,    kind: MenuLaunch::App(6) },
    MenuItem { label: "Minesweeper",  desc: "Find the mines",             color: 0xFCD34D, icon: icons::IC_MINES,    kind: MenuLaunch::App(7) },
    MenuItem { label: "Doom",         desc: "E1M1 — shareware DOOM",      color: 0xEF4444, icon: icons::IC_DOOM,     kind: MenuLaunch::App(8) },
];
const MENU_APPS_END: usize = 14;  // items 0..14 = apps, 14..17 = games

const MENU_W:       i32 = 280;
const MENU_HDR_H:   i32 = 54;   // dingir avatar + title + subtitle
const MENU_SECT_H:  i32 = 20;   // "APPLICATIONS" / "GAMES" label
const MENU_ITEM_H:  i32 = 34;   // icon + name + description per row
const MENU_SEP_H:   i32 = 9;    // separator between sections
const MENU_FOOT_H:  i32 = 38;   // footer: Settings + Shut down

fn menu_total_h() -> i32 {
    MENU_HDR_H + MENU_SECT_H
    + MENU_APPS_END as i32 * MENU_ITEM_H
    + MENU_SEP_H + MENU_SECT_H
    + (MENU_ITEMS.len() - MENU_APPS_END) as i32 * MENU_ITEM_H
    + MENU_FOOT_H
}

fn start_menu_bounds(fh: u32) -> (i32, i32, i32, i32) {
    let h = menu_total_h();
    (PANEL_MARGIN, panel_top(fh) - h - 8, MENU_W, h)
}

fn draw_menu_item(fb: &mut Framebuffer, x: i32, y: i32, w: i32, item: &MenuItem, hovered: bool) {
    // Row highlight on hover — no shadow/underline clutter when idle.
    if hovered {
        fb.fill_rounded_rect(x + 4, y, w - 8, MENU_ITEM_H, 8, 0x29343A);
    }
    // Icon tile: accent-tinted dark glass (tint=60 ≈ 24% color, like dock tiles).
    // No border, no sheen — at 28×28 those details read as noise.
    let icon_x = x + 10; let icon_y = y + (MENU_ITEM_H - 28) / 2;
    fb.fill_rounded_rect(icon_x, icon_y, 28, 28, 7, tint(item.color, 60));
    let isz = icons::ICON_PX as i32;
    fb.draw_icon(icon_x + (28 - isz) / 2, icon_y + (28 - isz) / 2, item.icon, item.color);
    // Label + description
    let tx = x + 46;
    fb.draw_aa(tx, y + 2, item.label, WHITE, crate::fb::AA_S);
    fb.draw_aa(tx, y + 20, item.desc, 0x7E8C85, crate::fb::AA_T);
}

fn draw_start_menu(fb: &mut Framebuffer) {
    let (x, y, w, h) = start_menu_bounds(fb.height);
    let bg = 0x1B2126u32;

    // Shadow + glass panel
    fb.fill_rounded_rect(x + 3, y + 5, w + 2, h, 16, 0x0A0D10);
    fb.fill_rounded_rect(x + 1, y + 3, w,     h, 14, 0x0C1114);
    fb.fill_rounded_rect_glass(x, y, w, h, 14, bg, 236);
    fb.fill_rect_s(x + 12, y, w - 24, 2, TEAL);  // azure accent top strip
    fb.fill_rect_s(x + 12, y + h - 2, w - 24, 1, 0x151A1F);

    // ── Header: dingir avatar circle + "Rusty Penguin" + "OS v2.0.0" ────────
    let av_x = x + 14; let av_y = y + 10;
    fb.fill_circle(av_x + 18, av_y + 18, 18, 0x34424A);
    fb.fill_circle(av_x + 18, av_y + 18, 16, 0x232C31);
    fb.draw_star8(av_x + 18, av_y + 18, 10, TEAL);
    fb.draw_aa(av_x + 40, av_y + 1, "Rusty Penguin", WHITE, crate::fb::AA_S);
    fb.draw_aa(av_x + 40, av_y + 21, "OS v2.0.0  .  Ternary", 0x6FE18B, crate::fb::AA_T);

    // Hairline below header
    fb.fill_rect_s(x + 8, y + MENU_HDR_H - 1, w - 16, 1, 0x46525A);

    // ── Applications section ─────────────────────────────────────────────────
    let apps_y = y + MENU_HDR_H;
    fb.draw_aa(x + 14, apps_y + 3, "APPLICATIONS", 0x98A3AA, crate::fb::AA_T);
    for i in 0..MENU_APPS_END {
        let iy = apps_y + MENU_SECT_H + i as i32 * MENU_ITEM_H;
        draw_menu_item(fb, x, iy, w, &MENU_ITEMS[i], false);
    }

    // Separator
    let sep_y = apps_y + MENU_SECT_H + MENU_APPS_END as i32 * MENU_ITEM_H + MENU_SEP_H / 2;
    fb.fill_rect_s(x + 12, sep_y, w - 24, 1, 0x46525A);

    // ── Games section ────────────────────────────────────────────────────────
    let games_y = apps_y + MENU_SECT_H + MENU_APPS_END as i32 * MENU_ITEM_H + MENU_SEP_H;
    fb.draw_aa(x + 14, games_y + 3, "GAMES", 0x98A3AA, crate::fb::AA_T);
    for i in MENU_APPS_END..MENU_ITEMS.len() {
        let iy = games_y + MENU_SECT_H + (i - MENU_APPS_END) as i32 * MENU_ITEM_H;
        draw_menu_item(fb, x, iy, w, &MENU_ITEMS[i], false);
    }

    // ── Footer: Settings · Shut Down ─────────────────────────────────────────
    let foot_y = y + h - MENU_FOOT_H;
    fb.fill_rect_s(x + 8, foot_y, w - 16, 1, 0x46525A);
    let bw = (w / 2) - 14;
    let bh = 28;
    let by = foot_y + 5;
    // Settings button — visible neutral tile
    let sbx = x + 10;
    fb.fill_rounded_rect(sbx, by, bw, bh, 9, 0x3A4850);
    draw_round_border(fb, sbx, by, bw, bh, 9, 0x68787F);
    fb.draw_aa(sbx + 14, by + 8, "Settings", WHITE, crate::fb::AA_S);
    // Shut Down button — visible red tile
    let dbx = x + w / 2 + 4;
    fb.fill_rounded_rect(dbx, by, bw, bh, 9, 0x4A2020);
    draw_round_border(fb, dbx, by, bw, bh, 9, 0x8A4040);
    fb.draw_aa(dbx + 10, by + 8, "Shut Down", TRIT_NEG, crate::fb::AA_S);
}

fn start_menu_hit(fh: u32, mx: i32, my: i32) -> Option<usize> {
    let (x, y, w, _) = start_menu_bounds(fh);
    if mx < x || mx >= x + w { return None; }
    let apps_y = y + MENU_HDR_H + MENU_SECT_H;
    for i in 0..MENU_APPS_END {
        let iy = apps_y + i as i32 * MENU_ITEM_H;
        if my >= iy && my < iy + MENU_ITEM_H { return Some(i); }
    }
    let games_y = apps_y + MENU_APPS_END as i32 * MENU_ITEM_H + MENU_SEP_H + MENU_SECT_H;
    for i in MENU_APPS_END..MENU_ITEMS.len() {
        let iy = games_y + (i - MENU_APPS_END) as i32 * MENU_ITEM_H;
        if my >= iy && my < iy + MENU_ITEM_H { return Some(i); }
    }
    // Footer: Settings (index 4 = existing Settings app), Shut Down
    let foot_y = y + menu_total_h() - MENU_FOOT_H;
    if my >= foot_y + 6 && my < foot_y + 30 {
        if mx < x + w / 2 { return Some(4); }  // Settings
        // Shut Down — handled by special value
        return Some(99);
    }
    None
}

// ---- Right-click context menu ───────────────────────────────────────────────
// Items: (label, color) — empty label = separator.
// Action indices (non-separator items, in order):
//   0  Open Terminal     1  Open Files        2  Show Desktop
//   3  Take Screenshot   4  Change Background  5  Display Settings
//   6  Close All Windows

const CTX_ITEMS: &[(&str, u32)] = &[
    ("Open Terminal",      0x6FE18B),  // GREEN
    ("Open Files",         0x8CC6E5),  // BLUE
    ("",                   0),          // separator
    ("Show Desktop",       0x909A92),  // DIM
    ("Take Screenshot",    0x8CC6E5),  // BLUE — sleek one-click capture
    ("",                   0),          // separator
    ("Change Background",  0xECDAA7),  // CREAM
    ("Display Settings",   0xF5C451),  // AMBER
    ("",                   0),          // separator
    ("Close All Windows",  0xEF7575),  // NEG red
];

const CTX_ITEM_H:  i32 = 26;  // height of one item row
const CTX_SEP_H:   i32 = 9;   // height of a separator
const CTX_W:       i32 = 210;  // menu width
const CTX_PAD_Y:   i32 = 6;   // top/bottom padding inside menu
const CTX_BG:      u32 = 0x1E2620;  // warm-stone panel dark

fn ctx_menu_total_h() -> i32 {
    CTX_PAD_Y * 2 + CTX_ITEMS.iter().map(|(lbl, _)| {
        if lbl.is_empty() { CTX_SEP_H } else { CTX_ITEM_H }
    }).sum::<i32>()
}

fn ctx_menu_bounds(mx: i32, my: i32, fw: u32, fh: u32) -> (i32, i32, i32, i32) {
    let w = CTX_W;
    let h = ctx_menu_total_h();
    let x = mx.min(fw as i32 - w - 6).max(0);
    let y = my.min(fh as i32 - h - 6).max(0);
    (x, y, w, h)
}

fn draw_ctx_menu(fb: &mut Framebuffer, mx: i32, my: i32) {
    draw_ctx_menu_hover(fb, mx, my, None);
}

fn draw_ctx_menu_hover(fb: &mut Framebuffer, mx: i32, my: i32, hover: Option<usize>) {
    let (x, y, w, h) = ctx_menu_bounds(mx, my, fb.width, fb.height);
    // Aero-style shadow + frosted glass body
    fb.fill_rounded_rect(x + 3, y + 5, w + 2, h, 13, 0x0A0D10);
    fb.fill_rounded_rect(x + 1, y + 3, w,     h, 12, 0x0C1114);
    fb.fill_rounded_rect_glass(x, y, w, h, 12, CTX_BG, 230);
    // Top accent edge
    fb.fill_rect_s(x + 10, y + 1, w - 20, 1, 0x5E6A72);
    fb.fill_rect_s(x + 10, y, w - 20, 2, TEAL);
    fb.fill_rect_s(x + 10, y + h - 2, w - 20, 1, 0x151A1F);

    let mut cy = y + CTX_PAD_Y;
    for (i, (label, color)) in CTX_ITEMS.iter().enumerate() {
        if label.is_empty() {
            // Separator line
            let sep_y = cy + CTX_SEP_H / 2;
            fb.fill_rect_s(x + 12, sep_y, w - 24, 1, 0x3C4641);
            cy += CTX_SEP_H;
        } else {
            // Item background — highlight on hover
            let hovered = hover == Some(i);
            let bg = if hovered { 0x2B343A } else { CTX_BG };
            if hovered {
                fb.fill_rounded_rect_glass(x + 2, cy, w - 4, CTX_ITEM_H, 8, bg, 200);
                // Left accent bar on hover
                fb.fill_rect_s(x + 4, cy + 5, 3, CTX_ITEM_H - 10, *color);
            }
            // Label text
            fb.draw_aa(x + 14, cy + (CTX_ITEM_H - Framebuffer::aa_line(crate::fb::AA_S)) / 2, label, *color, crate::fb::AA_S);
            cy += CTX_ITEM_H;
        }
    }
}

fn ctx_menu_item_hit(mx: i32, my: i32, cmx: i32, cmy: i32, fw: u32, fh: u32) -> Option<usize> {
    let (x, y, w, _) = ctx_menu_bounds(cmx, cmy, fw, fh);
    if mx < x || mx >= x + w { return None; }
    let mut cy = y + CTX_PAD_Y;
    for (i, (label, _)) in CTX_ITEMS.iter().enumerate() {
        if label.is_empty() { cy += CTX_SEP_H; continue; }
        if my >= cy && my < cy + CTX_ITEM_H { return Some(i); }
        cy += CTX_ITEM_H;
    }
    None
}

// Map CTX_ITEMS index → sequential action index (skipping separators).
fn ctx_action(item_idx: usize) -> usize {
    CTX_ITEMS[..item_idx].iter().filter(|(l, _)| !l.is_empty()).count()
}

// ---- Window + terminal wrapper ──────────────────────────────────────────────

struct TermWin {
    win:         wm::Window,
    term:        term::Terminal,
    editor:      Option<editor::TextEditor>,
    app:         Option<alloc::boxed::Box<dyn app::App>>,
    win_dirty:   bool,
    initial_cmd: Option<&'static [u8]>,
}

fn open_term(w: i32, h: i32, n: usize, l: &Launcher) -> Option<TermWin> {
    match term::Terminal::spawn() {
        Ok(mut t) => {
            let off = n as i32 * 20;
            let left_margin = 75;
            // Open at ~60% of screen width and ~65% of usable height by default.
            let tw = ((w * 3 / 5) & !7).max(wm::WINDOW_W).min(w - left_margin - 20);
            let th = ((h * 13 / 20) & !7).max(wm::WINDOW_H).min(h - 28 - TOPBAR_H as i32 - 20);
            let wx = ((w - left_margin - tw) / 2 + left_margin + off)
                .max(left_margin)
                .min(w - tw);
            let wy = ((h - th - 28) / 2 + off).max(TOPBAR_H as i32).min(h - th - 28);
            // Reflow terminal to match the initial window size.
            let nc = ((tw - 2) as usize / term::CHAR_W).max(8);
            let nr = ((th - 2 - wm::TITLEBAR_H) as usize / term::CHAR_H).max(4);
            t.resize_term(nc, nr);
            let mut win = wm::Window::new(wx, wy, l.title);
            win.w = tw; win.h = th;
            win.restore_w = tw; win.restore_h = th;
            Some(TermWin {
                win,
                term: t,
                editor: None,
                app: None,
                win_dirty: true,
                initial_cmd: l.cmd.map(|s| s.as_bytes()),
            })
        }
        Err(_) => None,
    }
}

fn open_editor(w: i32, h: i32, n: usize, filename: &str, title: &str) -> Option<TermWin> {
    match term::Terminal::spawn() {
        Ok(t) => {
            let ed = editor::TextEditor::new(filename);
            let off = n as i32 * 20;
            let left_margin = 75;
            let wx = ((w - left_margin - wm::WINDOW_W) / 2 + left_margin + off)
                .max(left_margin)
                .min(w - wm::WINDOW_W);
            let wy = ((h - wm::WINDOW_H - 28) / 2 + off).max(TOPBAR_H as i32).min(h - wm::WINDOW_H - 28);
            Some(TermWin {
                win: wm::Window::new(wx, wy, title),
                term: t,
                editor: Some(ed),
                app: None,
                win_dirty: true,
                initial_cmd: None,
            })
        }
        Err(_) => None,
    }
}

fn open_file_manager(w: i32, h: i32, n: usize) -> Option<TermWin> {
    match term::Terminal::spawn() {
        Ok(t) => {
            let fm = alloc::boxed::Box::new(app::FileManager::new());
            let off = n as i32 * 20;
            let left_margin = 75;
            let wx = ((w - left_margin - wm::WINDOW_W) / 2 + left_margin + off)
                .max(left_margin)
                .min(w - wm::WINDOW_W);
            let wy = ((h - wm::WINDOW_H - 28) / 2 + off).max(TOPBAR_H as i32).min(h - wm::WINDOW_H - 28);
            Some(TermWin {
                win: wm::Window::new(wx, wy, "File Manager"),
                term: t,
                editor: None,
                app: Some(fm),
                win_dirty: true,
                initial_cmd: None,
            })
        }
        Err(_) => None,
    }
}

fn open_calendar(w: i32, h: i32, n: usize) -> Option<TermWin> {
    match term::Terminal::spawn() {
        Ok(t) => {
            let cal = alloc::boxed::Box::new(app::Calendar::new());
            let off = n as i32 * 20;
            let left_margin = 75;
            let wx = ((w - left_margin - wm::WINDOW_W) / 2 + left_margin + off)
                .max(left_margin)
                .min(w - wm::WINDOW_W);
            let wy = ((h - wm::WINDOW_H - 28) / 2 + off).max(TOPBAR_H as i32).min(h - wm::WINDOW_H - 28);
            Some(TermWin {
                win: wm::Window::new(wx, wy, "Calendar"),
                term: t,
                editor: None,
                app: Some(cal),
                win_dirty: true,
                initial_cmd: None,
            })
        }
        Err(_) => None,
    }
}

fn open_settings(w: i32, h: i32, n: usize) -> Option<TermWin> {
    match term::Terminal::spawn() {
        Ok(t) => {
            let settings = alloc::boxed::Box::new(app::Settings::new());
            let off = n as i32 * 20;
            let left_margin = 75;
            let wx = ((w - left_margin - wm::WINDOW_W) / 2 + left_margin + off)
                .max(left_margin)
                .min(w - wm::WINDOW_W);
            let wy = ((h - wm::WINDOW_H - 28) / 2 + off).max(TOPBAR_H as i32).min(h - wm::WINDOW_H - 28);
            Some(TermWin {
                win: wm::Window::new(wx, wy, "Settings"),
                term: t,
                editor: None,
                app: Some(settings),
                win_dirty: true,
                initial_cmd: None,
            })
        }
        Err(_) => None,
    }
}

fn open_sound(w: i32, h: i32, n: usize) -> Option<TermWin> {
    match term::Terminal::spawn() {
        Ok(t) => {
            let snd = alloc::boxed::Box::new(app::Sound::new());
            let off = n as i32 * 20;
            let left_margin = 75;
            let wx = ((w - left_margin - wm::WINDOW_W) / 2 + left_margin + off)
                .max(left_margin).min(w - wm::WINDOW_W);
            let wy = ((h - wm::WINDOW_H - 28) / 2 + off).max(TOPBAR_H as i32).min(h - wm::WINDOW_H - 28);
            Some(TermWin { win: wm::Window::new(wx, wy, "Sound"), term: t,
                           editor: None, app: Some(snd), win_dirty: true, initial_cmd: None })
        }
        Err(_) => None,
    }
}

fn open_media_player(w: i32, h: i32, n: usize) -> Option<TermWin> {
    match term::Terminal::spawn() {
        Ok(t) => {
            let mp = alloc::boxed::Box::new(app::MediaPlayer::new());
            let off = n as i32 * 20;
            let left_margin = 75;
            let wx = ((w - left_margin - wm::WINDOW_W) / 2 + left_margin + off)
                .max(left_margin).min(w - wm::WINDOW_W);
            let wy = ((h - wm::WINDOW_H - 28) / 2 + off).max(TOPBAR_H as i32).min(h - wm::WINDOW_H - 28);
            let mut win = wm::Window::new(wx, wy, "Media Player");
            // The clip is 16:9 — give it a roomier landscape window than the
            // terminal default so the founding shot isn't cramped.
            let mw = 560.min(w - 90); let mh = 360.min(h - 28 - TOPBAR_H as i32);
            win.w = mw; win.h = mh;
            win.restore_w = mw; win.restore_h = mh;
            win.x = win.x.min(w - mw - 8).max(75);
            Some(TermWin { win, term: t, editor: None, app: Some(mp),
                           win_dirty: true, initial_cmd: None })
        }
        Err(_) => None,
    }
}

fn open_screenshot(w: i32, h: i32, n: usize) -> Option<TermWin> {
    match term::Terminal::spawn() {
        Ok(t) => {
            let ss = alloc::boxed::Box::new(app::Screenshot::new());
            let off = n as i32 * 20;
            let left_margin = 75;
            let wx = ((w - left_margin - wm::WINDOW_W) / 2 + left_margin + off)
                .max(left_margin).min(w - wm::WINDOW_W);
            let wy = ((h - wm::WINDOW_H - 28) / 2 + off).max(TOPBAR_H as i32).min(h - wm::WINDOW_H - 28);
            Some(TermWin { win: wm::Window::new(wx, wy, "Screenshot"), term: t,
                           editor: None, app: Some(ss), win_dirty: true, initial_cmd: None })
        }
        Err(_) => None,
    }
}

fn open_image_viewer(w: i32, h: i32, n: usize) -> Option<TermWin> {
    match term::Terminal::spawn() {
        Ok(t) => {
            let iv = alloc::boxed::Box::new(app::ImageViewer::new());
            let off = n as i32 * 20;
            let left_margin = 75;
            let wx = ((w - left_margin - wm::WINDOW_W) / 2 + left_margin + off)
                .max(left_margin).min(w - wm::WINDOW_W);
            let wy = ((h - wm::WINDOW_H - 28) / 2 + off).max(TOPBAR_H as i32).min(h - wm::WINDOW_H - 28);
            let mut win = wm::Window::new(wx, wy, "Image Viewer");
            let iw = 540.min(w - 90); let ih = 400.min(h - 28 - TOPBAR_H as i32);
            win.w = iw; win.h = ih; win.restore_w = iw; win.restore_h = ih;
            win.x = win.x.min(w - iw - 8).max(75);
            Some(TermWin { win, term: t, editor: None, app: Some(iv),
                           win_dirty: true, initial_cmd: None })
        }
        Err(_) => None,
    }
}

fn open_kernel_manager(w: i32, h: i32, n: usize) -> Option<TermWin> {
    match term::Terminal::spawn() {
        Ok(t) => {
            let km = alloc::boxed::Box::new(app::KernelManager::new());
            let off = n as i32 * 20;
            let left_margin = 75;
            let wx = ((w - left_margin - wm::WINDOW_W) / 2 + left_margin + off)
                .max(left_margin)
                .min(w - wm::WINDOW_W);
            let wy = ((h - wm::WINDOW_H - 28) / 2 + off).max(TOPBAR_H as i32).min(h - wm::WINDOW_H - 28);
            Some(TermWin {
                win: wm::Window::new(wx, wy, "Kernel Manager"),
                term: t,
                editor: None,
                app: Some(km),
                win_dirty: true,
                initial_cmd: None,
            })
        }
        Err(_) => None,
    }
}

fn open_tis_console(w: i32, h: i32, n: usize) -> Option<TermWin> {
    match term::Terminal::spawn() {
        Ok(t) => {
            let tis = alloc::boxed::Box::new(app::TisConsole::new());
            let off = n as i32 * 20;
            let left_margin = 75;
            let wx = ((w - left_margin - wm::WINDOW_W) / 2 + left_margin + off)
                .max(left_margin)
                .min(w - wm::WINDOW_W);
            let wy = ((h - wm::WINDOW_H - 28) / 2 + off).max(TOPBAR_H as i32).min(h - wm::WINDOW_H - 28);
            Some(TermWin {
                win: wm::Window::new(wx, wy, "TIS Console"),
                term: t,
                editor: None,
                app: Some(tis),
                win_dirty: true,
                initial_cmd: None,
            })
        }
        Err(_) => None,
    }
}

fn open_process_monitor(w: i32, h: i32, n: usize) -> Option<TermWin> {
    match term::Terminal::spawn() {
        Ok(t) => {
            let proc_mon = alloc::boxed::Box::new(app::ProcessMonitor::new());
            let off = n as i32 * 20;
            let left_margin = 75;
            let wx = ((w - left_margin - wm::WINDOW_W) / 2 + left_margin + off)
                .max(left_margin)
                .min(w - wm::WINDOW_W);
            let wy = ((h - wm::WINDOW_H - 28) / 2 + off).max(TOPBAR_H as i32).min(h - wm::WINDOW_H - 28);
            let mut win = wm::Window::new(wx, wy, "System Monitor");
            // Roomier than the terminal default so the graph + readout breathe.
            let mw = 580.min(w - 90); let mh = 440.min(h - 28 - TOPBAR_H as i32);
            win.w = mw; win.h = mh; win.restore_w = mw; win.restore_h = mh;
            win.x = win.x.min(w - mw - 8).max(75);
            Some(TermWin {
                win,
                term: t,
                editor: None,
                app: Some(proc_mon),
                win_dirty: true,
                initial_cmd: None,
            })
        }
        Err(_) => None,
    }
}

fn open_rusty_phone(w: i32, h: i32, n: usize) -> Option<TermWin> {
    match term::Terminal::spawn() {
        Ok(t) => {
            let phone = alloc::boxed::Box::new(app::RustyPhone::new());
            let off = n as i32 * 20;
            // Default to portrait-ish size — shows full dialer.
            let pw = 340.min(w - 90);
            let ph = 580.min(h - 28 - TOPBAR_H as i32);
            let wx = ((w - pw) / 2 + off).max(75).min(w - pw);
            let wy = ((h - ph - 28) / 2 + off).max(TOPBAR_H as i32).min(h - ph - 28);
            let mut win = wm::Window::new(wx, wy, "RustyPhone");
            win.w = pw; win.h = ph; win.restore_w = pw; win.restore_h = ph;
            Some(TermWin { win, term: t, editor: None, app: Some(phone), win_dirty: true, initial_cmd: None })
        }
        Err(_) => None,
    }
}

fn open_notes(w: i32, h: i32, n: usize) -> Option<TermWin> {
    match term::Terminal::spawn() {
        Ok(t) => {
            let notes = alloc::boxed::Box::new(app::Notes::new());
            let off = n as i32 * 20;
            let left_margin = 75;
            let nw = 480.min(w - 90);
            let nh = 360.min(h - 28 - TOPBAR_H as i32);
            let wx = ((w - left_margin - nw) / 2 + left_margin + off).max(left_margin).min(w - nw);
            let wy = ((h - nh - 28) / 2 + off).max(TOPBAR_H as i32).min(h - nh - 28);
            let mut win = wm::Window::new(wx, wy, "Notes");
            win.w = nw; win.h = nh; win.restore_w = nw; win.restore_h = nh;
            win.x = win.x.min(w - nw - 8).max(75);
            Some(TermWin { win, term: t, editor: None, app: Some(notes), win_dirty: true, initial_cmd: None })
        }
        Err(_) => None,
    }
}

fn open_calculator(w: i32, h: i32, n: usize) -> Option<TermWin> {
    match term::Terminal::spawn() {
        Ok(t) => {
            let calc = alloc::boxed::Box::new(app::Calculator::new());
            let off = n as i32 * 20;
            let left_margin = 75;
            let wx = ((w - left_margin - wm::WINDOW_W) / 2 + left_margin + off)
                .max(left_margin)
                .min(w - wm::WINDOW_W);
            let wy = ((h - wm::WINDOW_H - 28) / 2 + off).max(TOPBAR_H as i32).min(h - wm::WINDOW_H - 28);
            Some(TermWin {
                win: wm::Window::new(wx, wy, "Calculator"),
                term: t,
                editor: None,
                app: Some(calc),
                win_dirty: true,
                initial_cmd: None,
            })
        }
        Err(_) => None,
    }
}

fn open_system_clock(w: i32, h: i32, n: usize) -> Option<TermWin> {
    match term::Terminal::spawn() {
        Ok(t) => {
            let clock = alloc::boxed::Box::new(app::SystemClock::new());
            let off = n as i32 * 20;
            let left_margin = 75;
            let wx = ((w - left_margin - wm::WINDOW_W) / 2 + left_margin + off)
                .max(left_margin)
                .min(w - wm::WINDOW_W);
            let wy = ((h - wm::WINDOW_H - 28) / 2 + off).max(TOPBAR_H as i32).min(h - wm::WINDOW_H - 28);
            Some(TermWin {
                win: wm::Window::new(wx, wy, "Clock"),
                term: t,
                editor: None,
                app: Some(clock),
                win_dirty: true,
                initial_cmd: None,
            })
        }
        Err(_) => None,
    }
}

fn open_browser(w: i32, h: i32, n: usize) -> Option<TermWin> {
    match term::Terminal::spawn() {
        Ok(t) => {
            let br = alloc::boxed::Box::new(app::Browser::new());
            let off = n as i32 * 20;
            let left_margin = 75;
            let wx = ((w - left_margin - wm::WINDOW_W) / 2 + left_margin + off)
                .max(left_margin)
                .min(w - wm::WINDOW_W);
            let wy = ((h - wm::WINDOW_H - 28) / 2 + off).max(TOPBAR_H as i32).min(h - wm::WINDOW_H - 28);
            let mut win = wm::Window::new(wx, wy, "Web");
            // Browser wants a roomier page than the default terminal-sized window.
            let bw = 620.min(w - 90); let bh = 460.min(h - 28 - TOPBAR_H as i32);
            win.w = bw; win.h = bh;
            win.restore_w = bw; win.restore_h = bh;
            win.x = win.x.min(w - bw - 8).max(75);
            Some(TermWin {
                win,
                term: t,
                editor: None,
                app: Some(br),
                win_dirty: true,
                initial_cmd: None,
            })
        }
        Err(_) => None,
    }
}

fn open_help_browser(w: i32, h: i32, n: usize) -> Option<TermWin> {
    match term::Terminal::spawn() {
        Ok(t) => {
            let help = alloc::boxed::Box::new(app::HelpBrowser::new());
            let off = n as i32 * 20;
            let left_margin = 75;
            let wx = ((w - left_margin - wm::WINDOW_W) / 2 + left_margin + off)
                .max(left_margin)
                .min(w - wm::WINDOW_W);
            let wy = ((h - wm::WINDOW_H - 28) / 2 + off).max(TOPBAR_H as i32).min(h - wm::WINDOW_H - 28);
            Some(TermWin {
                win: wm::Window::new(wx, wy, "Help Browser"),
                term: t,
                editor: None,
                app: Some(help),
                win_dirty: true,
                initial_cmd: None,
            })
        }
        Err(_) => None,
    }
}

fn open_snake(w: i32, h: i32, n: usize) -> Option<TermWin> {
    match term::Terminal::spawn() {
        Ok(t) => {
            let game = alloc::boxed::Box::new(app::Snake::new(sys_ticks()));
            let off = n as i32 * 20;
            let left_margin = 75;
            let wx = ((w - left_margin - wm::WINDOW_W) / 2 + left_margin + off)
                .max(left_margin)
                .min(w - wm::WINDOW_W);
            let wy = ((h - wm::WINDOW_H - 28) / 2 + off).max(TOPBAR_H as i32).min(h - wm::WINDOW_H - 28);
            Some(TermWin {
                win: wm::Window::new(wx, wy, "Snake"),
                term: t,
                editor: None,
                app: Some(game),
                win_dirty: true,
                initial_cmd: None,
            })
        }
        Err(_) => None,
    }
}

fn open_doom_raycaster(w: i32, h: i32, n: usize) -> Option<TermWin> {
    match term::Terminal::spawn() {
        Ok(t) => {
            // Use WAD-based DOOM renderer (real E1M1 geometry from doom1.wad)
            // when available; fall back to pure-Rust raycaster otherwise.
            let wad = alloc::boxed::Box::new(app::WadDoom::new(sys_ticks()));
            let game: alloc::boxed::Box<dyn app::App> = if wad.loaded() {
                wad
            } else {
                alloc::boxed::Box::new(app::Doom::new(sys_ticks()))
            };
            let off = n as i32 * 20;
            let left_margin = 75;
            let wx = ((w - left_margin - wm::WINDOW_W) / 2 + left_margin + off)
                .max(left_margin)
                .min(w - wm::WINDOW_W);
            let wy = ((h - wm::WINDOW_H - 28) / 2 + off).max(TOPBAR_H as i32).min(h - wm::WINDOW_H - 28);
            Some(TermWin {
                win: wm::Window::new(wx, wy, "Doom"),
                term: t,
                editor: None,
                app: Some(game),
                win_dirty: true,
                initial_cmd: None,
            })
        }
        Err(_) => None,
    }
}

fn open_minesweeper(w: i32, h: i32, n: usize) -> Option<TermWin> {
    match term::Terminal::spawn() {
        Ok(t) => {
            let game = alloc::boxed::Box::new(app::Minesweeper::new(sys_ticks()));
            let off = n as i32 * 20;
            let left_margin = 75;
            let wx = ((w - left_margin - wm::WINDOW_W) / 2 + left_margin + off)
                .max(left_margin)
                .min(w - wm::WINDOW_W);
            let wy = ((h - wm::WINDOW_H - 28) / 2 + off).max(TOPBAR_H as i32).min(h - wm::WINDOW_H - 28);
            Some(TermWin {
                win: wm::Window::new(wx, wy, "Minesweeper"),
                term: t,
                editor: None,
                app: Some(game),
                win_dirty: true,
                initial_cmd: None,
            })
        }
        Err(_) => None,
    }
}

// ---- Snap preview overlay ───────────────────────────────────────────────────
// A semi-transparent green half-screen (or full-screen) rectangle drawn while
// the user is dragging near a snap zone, showing where the window will land.
fn draw_snap_preview(fb: &mut Framebuffer, side: SnapSide, topbar_h: i32) {
    let sw = fb.width as i32; let sh = fb.height as i32;
    let usable_y = topbar_h;
    let usable_h = sh - 28 - topbar_h;  // 28 = dock area
    let (x, y, pw, ph) = match side {
        SnapSide::Left  => (0,      usable_y, sw / 2,       usable_h),
        SnapSide::Right => (sw / 2, usable_y, sw - sw / 2,  usable_h),
        SnapSide::Top   => (0,      usable_y, sw,            usable_h),
    };
    let pad = 6;
    // Soft glow behind the preview rectangle
    fb.fill_rounded_rect_glass(x + pad, y + pad, pw - 2*pad, ph - 2*pad, 14, 0x1A4A38, 110);
    // Crisp green outline
    draw_round_border(fb, x + pad, y + pad, pw - 2*pad, ph - 2*pad, 14, GREEN);
    // Subtle inner fill so the zone is clearly visible over any wallpaper
    fb.fill_rounded_rect_glass(x + pad + 1, y + pad + 1, pw - 2*pad - 2, ph - 2*pad - 2, 13, 0x1C3428, 60);
}

// ---- Full scene recomposite ─────────────────────────────────────────────────

fn recomposite(fb: &mut Framebuffer, wins: &mut Vec<TermWin>, start_menu: bool, ctx_menu: Option<(i32,i32)>, stats: &SysStats, blink_on: bool, hover_icon: Option<usize>, wallpaper_v: u8, snap_side: Option<SnapSide>, current_desktop: usize) {
    // Static background (gradient + logo + icon dock) is cached: blitting it is
    // far cheaper than recomputing the 1080-row gradient every frame, which is
    // what made dragging a window at 1080p janky. The cache is rebuilt only
    // when the static scene changes (icon hover, wallpaper change).
    if fb.bg_cached() {
        fb.restore_bg();
    } else {
        draw_scene_static_v(fb, wallpaper_v);
        fb.snapshot_bg();
    }
    draw_desktop_icons(fb, hover_icon);
    draw_taskbar_win_btns(fb, wins, current_desktop);
    // Snap preview — draw BEFORE windows so the semi-transparent overlay sits
    // underneath the dragged window and gives clear visual feedback.
    if let Some(side) = snap_side {
        draw_snap_preview(fb, side, TOPBAR_H as i32);
    }
    // Menu button active indicator: green underline + brightened bg when open.
    // Drawn per-frame (not in the cached bg) so it only appears on state change.
    if start_menu {
        let (mbx, mby, mbw, _) = menu_btn_rect(fb.height);
        let bh = PANEL_H - 14;
        fb.fill_rounded_rect(mbx, mby, mbw, bh, 10, 0x4A5A50);
        let star_cy = mby + bh / 2;
        fb.draw_star8(mbx + 19, star_cy, 13, TEAL);
        fb.draw_aa(mbx + 38, star_cy - 6, "Menu", GREEN, crate::fb::AA_S);
        // Active indicator: solid green line at very bottom of button
        fb.fill_rect_s(mbx + 14, mby + bh - 3, mbw - 28, 3, GREEN);
    }
    let up = rtc_str();
    // Find the focused index on the current desktop (last non-minimized window there).
    let focused_idx = wins.iter().enumerate().rev()
        .find(|(_, tw)| !tw.win.minimized && tw.win.desktop == current_desktop)
        .map(|(i, _)| i);
    for (i, tw) in wins.iter_mut().enumerate() {
        if tw.win.minimized { continue; }
        if tw.win.desktop != current_desktop { continue; }
        let focused = Some(i) == focused_idx;
        wm::draw_window(fb, &tw.win, focused);
        let (ox, oy) = wm::content_origin(&tw.win);
        let cw = (tw.win.w - 2).max(0) as u32;
        let ch = (tw.win.h - 3 - wm::TITLEBAR_H).max(0) as u32;
        if let Some(app) = &mut tw.app {
            app.render(fb, ox as u32, oy as u32, cw, ch);
        } else if let Some(ed) = &mut tw.editor {
            ed.render(fb, ox as u32, oy as u32, cw, ch);
        } else {
            tw.term.render(fb, ox as u32, oy as u32, cw, ch, focused && blink_on);
        }
        wm::draw_resize_grip(fb, &tw.win, focused);
        tw.term.dirty = false;
        tw.win_dirty  = false;
    }
    if start_menu { draw_start_menu(fb); }
    if let Some((cmx, cmy)) = ctx_menu { draw_ctx_menu(fb, cmx, cmy); }
    draw_topbar(fb, up.as_str(), stats, sys_ticks(), current_desktop);
    if unsafe { QS_OPEN } { draw_quick_settings(fb); }
    // Dock hover tooltip (drawn last, as an overlay; the cached bg restore on the
    // next recomposite wipes it cleanly). Suppressed while a menu is open.
    if !start_menu && ctx_menu.is_none() {
        if let Some(hi) = hover_icon { draw_dock_tooltip(fb, hi); }
    }
}

// schedesktop2 windowed-app compositing: blit a second real process's live 32×32
// surface (mapped read-only into our AS by the kernel) into an on-screen window,
// scaled 4× so it's clearly visible. This is the desktop — itself a scheduled,
// isolated process — hosting another isolated process's output in a window.
fn draw_mp_app_window(fb: &mut Framebuffer, surf_va: u64) {
    const TBH: u32 = 22; // titlebar height
    // Surface dimensions: a real WxH (e.g. a Linux app's 640x400 framebuffer) or
    // 0 = the legacy 32x32 synthetic app (drawn scaled 4x).
    let dims = sys_app_surface_dims();
    let (sw, sh, scale): (u32, u32, u32) = if dims == 0 {
        (32, 32, 4)
    } else {
        (((dims >> 16) & 0xFFFF) as u32, (dims & 0xFFFF) as u32, 1)
    };
    let cw = sw * scale; // on-screen content width
    let ch = sh * scale;
    // Centre the window in the upper-middle of the screen, clamped on-screen.
    let wx = ((fb.width.saturating_sub(cw)) / 2).min(fb.width.saturating_sub(cw + 4)).max(4);
    let wy: u32 = 90;
    // Soft shadow + frame + titlebar (matches the desktop's window chrome palette).
    fb.fill_rounded_rect(wx as i32 - 3, wy as i32 - 1, (cw + 6) as i32, (ch + TBH + 6) as i32, 9, 0x0C100E);
    fb.fill_rect(wx - 1, wy - 1, cw + 2, ch + TBH + 2, 0x2A332F);
    fb.fill_rect(wx, wy, cw, TBH, 0x333D38);
    fb.fill_rect_s(wx as i32 + 8, wy as i32 + 1, (cw as i32 - 16).max(1), 1, 0x4C5A50); // top light-catch
    let title = if dims == 0 { "App (process 2)" } else { "Linux app (scheduled)" };
    fb.draw_aa(wx as i32 + 8, wy as i32 + 6, title, WHITE, crate::fb::AA_T);
    // Blit the app's surface into the window body (1:1, or 4x for the synthetic app).
    let src = surf_va as *const u32;
    for cy in 0..ch {
        for cx in 0..cw {
            let sp = unsafe { core::ptr::read_volatile(src.add((((cy / scale) * sw) + (cx / scale)) as usize)) };
            fb.set_pixel(wx + cx, wy + TBH + cy, sp & 0x00FF_FFFF);
        }
    }
}

// A small floating label above a hovered dock favourite.
fn draw_dock_tooltip(fb: &mut Framebuffer, hi: usize) {
    let slot = match FAV_IDX.iter().position(|&x| x == hi) { Some(s) => s, None => return };
    let (x, y, tw, _) = fav_rect(slot, fb.height);
    let label = DESKTOP_ICONS[hi].label;
    let lw = Framebuffer::aa_w(label, crate::fb::AA_S);
    let pad = 11; let bw = lw + pad * 2; let bh = 28;
    let bx = x + tw / 2 - bw / 2;
    let by = y - bh - 8;
    fb.fill_rounded_rect(bx + 1, by + 1, bw, bh, 8, 0x0A0D10);
    fb.fill_rounded_rect(bx, by, bw, bh, 8, 0x2A332F);
    draw_round_border(fb, bx, by, bw, bh, 8, PANEL_EDGE);
    fb.draw_aa(bx + pad, by + 6, label, WHITE, crate::fb::AA_S);
}

// ── Entry point ──────────────────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let mut fb = match Framebuffer::open() {
        Ok(f) => f,
        Err(_) => loop { sys_yield(); },
    };

    // `brightness=N` on the kernel cmdline → boot pre-dimmed. Applied before the
    // first present so the whole UI comes up at that software-brightness level.
    let bb = sys_boot_brightness();
    if bb != u64::MAX && bb <= 100 {
        unsafe { QS.brightness = bb as u8; }
        fb.set_brightness(bb as u8);
    }

    // schedesktop2 windowed-app mode: if the kernel mapped a second real app's
    // live surface into our address space, composite it into an on-screen window
    // each frame — the desktop (itself a scheduled process) hosting another
    // isolated process's output. 0 = normal boot (no app surface).
    let mp_surf_va = sys_app_surface();

    let w = fb.width as i32; let h = fb.height as i32;
    let mut mouse = MouseState { x: w / 2, y: h / 2, buttons: 0, btn_pressed: 0 };

    let mut wallpaper_variant: u8 = {   // cycles on "Change Background"
        let wp = sys_wallpaper();       // `wallpaper=N` boot override (test + feature)
        if wp != u64::MAX { (wp as u8) % WALLPAPER_COUNT } else { 5 } // default = Nebula
    };

    let mut stats = sample_stats();
    draw_scene_static_v(&mut fb, wallpaper_variant);
    let up0 = rtc_str();
    draw_topbar(&mut fb, up0.as_str(), &stats, sys_ticks(), 0);

    let cbl = (CURSOR_BW * CURSOR_BH) as usize;
    let mut cbuf = vec![BG; cbl];
    let mut cx = mouse.x; let mut cy = mouse.y;
    save_cursor_bg(&fb, cx, cy, &mut cbuf);
    draw_cursor(&mut fb, cx, cy);
    fb.present();  // flush initial paint to screen

    let mut last_topbar_tick: u64 = 0;
    let mut last_blink_tick: u64 = 0;
    let mut last_stat_tick: u64 = 0;
    let mut frames_since_stat: u32 = 0;
    let mut blink_on: bool = true;
    let mut wins: Vec<TermWin> = Vec::new();
    // Boot to a clean desktop — the welcome card + dock + gradient are the
    // first impression, not a terminal covering them. Open apps from the dock
    // or the Menu.
    let mut scene_dirty = true;
    let mut start_menu_open = false;
    let mut ctx_menu: Option<(i32, i32)> = None;
    let mut hover_icon: Option<usize> = None;
    let mut pending_screenshot = false; // right-click "Take Screenshot" → capture next clean frame
    let mut current_desktop: usize = 0;  // active virtual desktop (0-3)
    let mut snap_preview: Option<SnapSide> = None;  // snap zone preview during drag

    // autostart=N on the kernel cmdline opens app N immediately (headless
    // screendump verification — no mouse needed).
    let autostart = sys_autostart();
    if autostart != u64::MAX {
        if let Some(mut tw) = open_app_by_index(autostart, w, h, wins.len()) {
            tw.win.desktop = current_desktop;
            wins.push(tw);
        }
    }
    // showmenu cmdline flag → open the start menu on launch (screenshot aid).
    if sys_showmenu() {
        start_menu_open = true;
        scene_dirty = true;
    }

    let mut last_loop_tick: u64 = sys_ticks();

    loop {
        // Cap the loop to one iteration per PIT tick (~100 Hz). Without this,
        // sys_yield on a single-process kernel returns immediately and the
        // loop spins as fast as the CPU allows — ~100k+ frames/sec with no
        // visible benefit, since the display can't show that and the user
        // can't react that fast. Doctrine: sparse execution, no busy loops.
        loop {
            sys_yield();
            let now = sys_ticks();
            if now != last_loop_tick { last_loop_tick = now; break; }
        }

        // Sparse-rendering / "ternary" damage tracking for this frame: when only
        // a window is being dragged, the rest of the screen is DORMANT — we
        // recomposite the (correct) backbuffer but present ONLY the changed band
        // to VRAM, skipping the dominant full-screen MMIO copy. Set by the drag
        // handler below to the union of the window's old+new vertical span.
        let mut drag_band: Option<(i32, i32)> = None;

        let keys = input::poll(&mut mouse, w, h);
        let btn = mouse.buttons;

        // The Image Viewer (same process) requested "set as wallpaper" — switch
        // to the custom image and repaint the desktop.
        if unsafe { WALLPAPER_SET_REQUEST } {
            unsafe { WALLPAPER_SET_REQUEST = false; }
            wallpaper_variant = WP_CUSTOM;
            fb.invalidate_bg();
            scene_dirty = true;
        }

        // Publish the open-window list for the System Monitor, and service a
        // force-quit request (close the window the monitor selected).
        unsafe {
            let n = wins.len().min(12);
            for (i, tw) in wins.iter().take(12).enumerate() {
                let t = tw.win.title.as_bytes();
                let m = t.len().min(19);
                APP_TITLES[i][..m].copy_from_slice(&t[..m]);
                for j in m..20 { APP_TITLES[i][j] = 0; }
            }
            APP_COUNT = n;
            if KILL_IDX >= 0 {
                let idx = KILL_IDX as usize;
                KILL_IDX = -1;
                if idx < wins.len() { wins.remove(idx); scene_dirty = true; }
            }
        }

        // Snapshot old cursor position — the unified render uses this for restore.
        let prev_cx = cx; let prev_cy = cy;

        // Keyboard → global shortcuts first, then focused terminal/editor
        for &k in keys.iter() {
            if k == 0x14 { // Ctrl+T
                if let Some(mut tw) = open_term(w, h, wins.len(), &LAUNCHERS[0]) {
                    tw.win.desktop = current_desktop;
                    wins.push(tw);
                    scene_dirty = true;
                }
            } else if k == 0x02 { // Ctrl+B — open browser
                if let Some(mut tw) = open_browser(w, h, wins.len()) {
                    tw.win.desktop = current_desktop;
                    wins.push(tw);
                    scene_dirty = true;
                }
            } else if k == 0x07 { // Ctrl+G — open Image Viewer (gallery)
                if let Some(mut tw) = open_image_viewer(w, h, wins.len()) {
                    tw.win.desktop = current_desktop;
                    wins.push(tw);
                    scene_dirty = true;
                }
            } else if k == 0x10 { // Ctrl+P — take a screenshot (capture next clean frame)
                pending_screenshot = true;
            } else if k == 0x06 { // Ctrl+F — exec real fbDOOM (Linux ABI test)
                sys_exec_linux(b"bin/fbdoom");
            } else if k == 0x04 { // Ctrl+D — open DOOM raycaster window
                if let Some(mut tw) = open_doom_raycaster(w, h, wins.len()) {
                    tw.win.desktop = current_desktop;
                    wins.push(tw);
                    scene_dirty = true;
                }
            } else if k == 0x17 { // Ctrl+W — close the focused window on this desktop
                // Close only the top window on the current desktop (not across desktops).
                let idx = wins.iter().enumerate().rev()
                    .find(|(_, tw)| tw.win.desktop == current_desktop)
                    .map(|(i, _)| i);
                if let Some(i) = idx { wins.remove(i); scene_dirty = true; }
            } else if k == 0x13 { // Ctrl+S (save editor)
                if let Some(tw) = wins.iter_mut().rev()
                    .find(|tw| tw.win.desktop == current_desktop) {
                    if let Some(ed) = &mut tw.editor { ed.save(); }
                }
            } else if k == 0x11 { // Ctrl+Q (close editor)
                if let Some(tw) = wins.iter_mut().rev()
                    .find(|tw| tw.win.desktop == current_desktop) {
                    if let Some(ed) = &mut tw.editor { ed.wants_close = true; }
                }
            } else if let Some(tw) = wins.iter_mut().rev()
                .find(|tw| tw.win.desktop == current_desktop) {
                if let Some(app) = &mut tw.app {
                    app.on_key(k);
                    tw.win_dirty = true;
                } else if let Some(ed) = &mut tw.editor {
                    ed.send_key(k);
                    tw.win_dirty = true;
                } else {
                    tw.term.send_key(k);
                    tw.term.dirty = true;
                }
            }
        }

        // Pump initial commands into newly opened terminals
        for tw in wins.iter_mut() {
            if let Some(cmd) = tw.initial_cmd.take() {
                for &b in cmd { tw.term.send_key(b); }
                tw.term.dirty = true;
            }
        }

        // Close windows that requested close
        let needs_retain = wins.iter().any(|tw| {
            tw.term.wants_close
                || tw.editor.as_ref().map(|e| e.wants_close).unwrap_or(false)
                || tw.app.as_ref().map(|a| a.wants_close()).unwrap_or(false)
        });
        if needs_retain {
            wins.retain(|tw| {
                !(tw.term.wants_close
                    || tw.editor.as_ref().map(|e| e.wants_close).unwrap_or(false)
                    || tw.app.as_ref().map(|a| a.wants_close()).unwrap_or(false))
            });
            scene_dirty = true;
        }

        // Topbar: check if due; defer actual draw to the unified render pass.
        let now_ticks = sys_ticks();
        let topbar_due = now_ticks.wrapping_sub(last_topbar_tick) >= 200;
        if topbar_due {
            last_topbar_tick = now_ticks;
            stats = sample_stats();
        }

        // Periodic stat line to serial (every 500 ticks = 5s). Host-side
        // log inspection can read these to see heap drift, frame rate,
        // and window count without opening QEMU. Doctrine: observable.
        if now_ticks.wrapping_sub(last_stat_tick) >= 500 {
            let elapsed = now_ticks.wrapping_sub(last_stat_tick).max(1);
            last_stat_tick = now_ticks;
            let heap_pct = {
                let u = allocator::used_bytes() as u64;
                let t = allocator::total_bytes() as u64;
                if t > 0 { (u * 100 / t).min(100) } else { 0 }
            };
            serial_write_str("[stat] heap=");
            serial_write_u64(heap_pct);
            serial_write_str("% wins=");
            serial_write_u64(wins.len() as u64);
            serial_write_str(" fps=");
            // frames per second over the elapsed window
            serial_write_u64((frames_since_stat as u64) * 100 / elapsed);
            serial_write_str("\n");
            frames_since_stat = 0;
        }

        // Text cursor blink: toggle every 50 ticks (~500ms @ 100Hz).
        // Only the focused (topmost) terminal on the current desktop blinks.
        if now_ticks.wrapping_sub(last_blink_tick) >= 50 {
            last_blink_tick = now_ticks;
            blink_on = !blink_on;
            if let Some(tw) = wins.iter_mut().rev()
                .find(|tw| !tw.win.minimized && tw.win.desktop == current_desktop) {
                tw.term.dirty = true;
            }
        }

        // Update cursor to new position so click/drag handlers see current coords.
        cx = mouse.x; cy = mouse.y;

        // Desktop icon hover — triggers a recomposite when it changes.
        let new_hover = desktop_icon_hit(cx, cy, fb.height);
        if new_hover != hover_icon {
            hover_icon = new_hover;
            scene_dirty = true;
        }

        let left_down  = (btn & 0x01) != 0;
        let left_edge  = (mouse.btn_pressed & 0x01) != 0;
        let right_edge = (mouse.btn_pressed & 0x02) != 0;

        // Right-click: open context menu on empty desktop area
        if right_edge {
            let prev_open = ctx_menu.is_some() || start_menu_open;
            ctx_menu = None;
            start_menu_open = false;
            let on_win = wins.iter().any(|tw|
                tw.win.desktop == current_desktop && wm::window_hit(&tw.win, cx, cy));
            if !on_win && cy >= TOPBAR_H as i32 && cy < h - 28 {
                ctx_menu = Some((cx, cy));
            }
            if ctx_menu.is_some() || prev_open { scene_dirty = true; }
        }

        if left_edge {
            let qs_open = unsafe { QS_OPEN };
            if let Some(d) = ws_dot_hit(cx, cy) {
                // Workspace dot click — switch virtual desktop.
                if d != current_desktop {
                    current_desktop = d;
                    snap_preview = None;
                    scene_dirty = true;
                }
            } else if kbd_pill_hit(cx, cy) {
                // Toggle keyboard layout EN <-> DE via the kernel (syscall #21).
                let cur = unsafe { sys_kbd_layout(0) };
                unsafe { sys_kbd_layout(if cur == 1 { 1 } else { 2 }); }
                scene_dirty = true;
            } else if tray_hit(w as u32, h as u32, cx, cy) {
                // Toggle the quick-settings panel from the tray.
                unsafe { QS_OPEN = !qs_open; }
                ctx_menu = None; start_menu_open = false;
                scene_dirty = true;
            } else if qs_open {
                let (px, py, pw0, ph0) = qs_rect(h as u32, w as u32);
                if cx >= px && cx < px + pw0 && cy >= py && cy < py + ph0 {
                    if let Some(t) = qs_tile_hit(px, py, cx, cy) {
                        unsafe { qs_toggle(t); }
                        if t == 2 { // Dark Style → swap wallpaper to the dark/light variant
                            wallpaper_variant = if unsafe { QS.dark } { 3 } else { 0 };
                            fb.invalidate_bg();
                        }
                        scene_dirty = true;
                    } else if let Some(v) = qs_slider_hit(px, py, cx, cy) {
                        unsafe { QS.volume = v; set_master_volume(v); }
                        scene_dirty = true;
                    } else if let Some(v) = qs_bright_slider_hit(px, py, cx, cy) {
                        // Software brightness: dim the whole display via the
                        // present-time LUT. Repaint so the new level is visible now.
                        unsafe { QS.brightness = v; }
                        fb.set_brightness(v);
                        scene_dirty = true;
                    }
                    // click on panel chrome → consume, keep panel open
                } else {
                    // click outside the panel just dismisses it
                    unsafe { QS_OPEN = false; }
                    scene_dirty = true;
                }
            } else if let Some((cmx, cmy)) = ctx_menu.take() {
                if let Some(item) = ctx_menu_item_hit(cx, cy, cmx, cmy, fb.width, fb.height) {
                    match ctx_action(item) {
                        0 => { // Open Terminal
                            if let Some(mut tw) = open_term(w, h, wins.len(), &LAUNCHERS[0]) {
                                tw.win.desktop = current_desktop; wins.push(tw);
                            }
                        }
                        1 => { // Open Files
                            if let Some(mut tw) = open_file_manager(w, h, wins.len()) {
                                tw.win.desktop = current_desktop; wins.push(tw);
                            }
                        }
                        2 => { // Show Desktop — minimize windows on current desktop
                            for tw in wins.iter_mut() {
                                if tw.win.desktop == current_desktop { tw.win.minimized = true; }
                            }
                        }
                        3 => { // Take Screenshot — capture on the NEXT clean frame
                               // (after this menu is gone), so the shot is unobstructed.
                            pending_screenshot = true;
                        }
                        4 => { // Change Background — cycle wallpaper variant
                            // 8 system backgrounds + the custom image when one is set.
                            let count = if vfs::vfs().read("wallpaper.ppm").is_some() { WALLPAPER_COUNT + 1 } else { WALLPAPER_COUNT };
                            wallpaper_variant = (wallpaper_variant + 1) % count;
                            fb.invalidate_bg();
                        }
                        5 => { // Display Settings
                            if let Some(mut tw) = open_settings(w, h, wins.len()) {
                                tw.win.desktop = current_desktop; wins.push(tw);
                            }
                        }
                        6 => { // Close All Windows on current desktop
                            wins.retain(|tw| tw.win.desktop != current_desktop);
                        }
                        _ => {}
                    }
                }
                scene_dirty = true;
            } else if start_menu_open {
                if let Some(mi) = start_menu_hit(fb.height, cx, cy) {
                    if mi == 99 {
                        // Shut Down: real ACPI S5 soft-off (syscall 37). The kernel
                        // drives the PM1 control register; the machine actually
                        // powers off. Falls through to halt only if ACPI is absent.
                        unsafe { core::arch::asm!("syscall", in("rax") 37u64, out("rcx") _, out("r11") _, options(nostack)); }
                        loop { unsafe { core::arch::asm!("hlt", options(nostack)); } }
                    }
                    if mi < MENU_ITEMS.len() {
                        let opened = match MENU_ITEMS[mi].kind {
                            MenuLaunch::Term(li) => open_term(w, h, wins.len(), &LAUNCHERS[li]),
                            MenuLaunch::App(0)   => open_file_manager(w, h, wins.len()),
                            MenuLaunch::App(1)   => open_editor(w, h, wins.len(), "readme.txt", "Text Editor"),
                            MenuLaunch::App(2)   => open_calculator(w, h, wins.len()),
                            MenuLaunch::App(3)   => open_help_browser(w, h, wins.len()),
                            MenuLaunch::App(4)   => open_settings(w, h, wins.len()),
                            MenuLaunch::App(5)   => open_tis_console(w, h, wins.len()),
                            MenuLaunch::App(6)   => open_snake(w, h, wins.len()),
                            MenuLaunch::App(7)   => open_minesweeper(w, h, wins.len()),
                            MenuLaunch::App(8)   => open_doom_raycaster(w, h, wins.len()),
                            MenuLaunch::App(9)   => open_browser(w, h, wins.len()),
                            MenuLaunch::App(10)  => open_kernel_manager(w, h, wins.len()),
                            MenuLaunch::App(11)  => open_sound(w, h, wins.len()),
                            MenuLaunch::App(12)  => open_media_player(w, h, wins.len()),
                            MenuLaunch::App(13)  => open_screenshot(w, h, wins.len()),
                            MenuLaunch::App(14)  => open_image_viewer(w, h, wins.len()),
                            MenuLaunch::App(15)  => open_system_clock(w, h, wins.len()),
                            MenuLaunch::App(16)  => open_process_monitor(w, h, wins.len()),
                            MenuLaunch::App(17)  => open_notes(w, h, wins.len()),
                            MenuLaunch::App(18)  => open_rusty_phone(w, h, wins.len()),
                            MenuLaunch::App(_)   => None,
                        };
                        if let Some(mut tw) = opened {
                            tw.win.desktop = current_desktop;
                            wins.push(tw);
                        }
                    }
                }
                start_menu_open = false;
                scene_dirty = true;
            } else if dingir_hit(fb.height, cx, cy) {
                start_menu_open = true;
                scene_dirty = true;
            } else {
                // Only hit-test windows on the current virtual desktop.
                let hit = wins.iter().enumerate().rev()
                    .find(|(_, tw)| tw.win.desktop == current_desktop && wm::window_hit(&tw.win, cx, cy))
                    .map(|(i, _)| i);

                if let Some(hi) = hit {
                    // Bring to front if not already — preserve desktop assignment.
                    let last_on_desktop = wins.iter().enumerate().rev()
                        .find(|(_, tw)| tw.win.desktop == current_desktop)
                        .map(|(i, _)| i)
                        .unwrap_or(wins.len().saturating_sub(1));
                    if hi != last_on_desktop {
                        let tw = wins.remove(hi); wins.push(tw);
                        wins.last_mut().unwrap().win_dirty = true;
                    }
                    let last = wins.len() - 1;
                    let tw = &mut wins[last];

                    if wm::close_btn_hit(&tw.win, cx, cy) {
                        wins.remove(last);
                        scene_dirty = true;
                    } else if wm::min_btn_hit(&tw.win, cx, cy) {
                        // Yellow button: minimize.
                        tw.win.minimized = true;
                        scene_dirty = true;
                    } else if wm::max_btn_hit(&tw.win, cx, cy) {
                        // Green button: toggle maximize.
                        tw.win.toggle_maximize(w, h, TOPBAR_H as i32);
                        scene_dirty = true;
                    } else if wm::resize_corner_hit(&tw.win, cx, cy) {
                        tw.win.resizing  = true;
                        tw.win.resize_mx = cx;
                        tw.win.resize_my = cy;
                        tw.win.resize_ow = tw.win.w;
                        tw.win.resize_oh = tw.win.h;
                    } else if wm::titlebar_hit(&tw.win, cx, cy) {
                        tw.win.dragging = true;
                        tw.win.drag_ox  = cx - tw.win.x;
                        tw.win.drag_oy  = cy - tw.win.y;
                    } else if let Some(app) = &mut tw.app {
                        // Click inside an app's content area — route to the app.
                        let (ox, oy) = wm::content_origin(&tw.win);
                        let cw = (tw.win.w - 2).max(0) as u32;
                        let ch = (tw.win.h - 3 - wm::TITLEBAR_H).max(0) as u32;
                        let lx = cx - ox;
                        let ly = cy - oy;
                        if lx >= 0 && ly >= 0 && (lx as u32) < cw && (ly as u32) < ch {
                            app.on_mouse(lx, ly, cw, ch, btn);
                            tw.win_dirty = true;
                        }
                    } else if let Some(ed) = &mut tw.editor {
                        // Click inside editor content — position the cursor.
                        let (ox, oy) = wm::content_origin(&tw.win);
                        let cw = (tw.win.w - 2).max(0) as u32;
                        let ch = (tw.win.h - 3 - wm::TITLEBAR_H).max(0) as u32;
                        let lx = cx - ox;
                        let ly = cy - oy;
                        if lx >= 0 && ly >= 0 && (lx as u32) < cw && (ly as u32) < ch {
                            ed.on_mouse(lx, ly, btn);
                            tw.win_dirty = true;
                        }
                    }
                } else {
                    if show_desktop_hit(fb.width, fb.height, cx, cy) {
                        // Minimize only windows on the current desktop.
                        for tw in wins.iter_mut() {
                            if tw.win.desktop == current_desktop { tw.win.minimized = true; }
                        }
                        scene_dirty = true;
                    } else if let Some(mi) = tbwin_hit(fb.width, fb.height, &wins, cx, cy, current_desktop) {
                        wins[mi].win.minimized = false;
                        let tw = wins.remove(mi); wins.push(tw);
                        scene_dirty = true;
                    } else if let Some(di) = desktop_icon_hit(cx, cy, fb.height) {
                        // Icon order: Term, Files, Edit, Calc, Help, Prefs, TIS, Snake, Mines
                        let opened = match di {
                            0 => open_term(w, h, wins.len(), &LAUNCHERS[0]),
                            1 => open_file_manager(w, h, wins.len()),
                            2 => open_editor(w, h, wins.len(), "readme.txt", "Text Editor"),
                            3 => open_calculator(w, h, wins.len()),
                            4 => open_help_browser(w, h, wins.len()),
                            5 => open_settings(w, h, wins.len()),
                            6 => open_tis_console(w, h, wins.len()),
                            7 => open_snake(w, h, wins.len()),
                            8 => open_minesweeper(w, h, wins.len()),
                            9 => open_doom_raycaster(w, h, wins.len()),
                            10 => open_browser(w, h, wins.len()),
                            11 => open_rusty_phone(w, h, wins.len()),
                            12 => open_notes(w, h, wins.len()),
                            _ => None,
                        };
                        if let Some(mut tw) = opened {
                            tw.win.desktop = current_desktop;
                            wins.push(tw);
                            scene_dirty = true;
                        }
                    }
                }
            }
        }

        // Drag / resize. No rate limit — if the position changed we want
        // the recomposite this frame, otherwise dragging looks like the
        // window is stuck while the cursor moves smoothly past it.
        if left_down {
            // Operate on the topmost DRAGGING/RESIZING window (which is always last
            // in wins since it was brought to front on click).
            let dragging_idx = wins.iter().enumerate().rev()
                .find(|(_, tw)| tw.win.dragging || tw.win.resizing)
                .map(|(i, _)| i);
            if let Some(di) = dragging_idx {
                let tw = &mut wins[di];
                if tw.win.dragging {
                    let oy = tw.win.y; let oh = tw.win.h;
                    let nx2 = (cx - tw.win.drag_ox).max(75).min(w - tw.win.w);
                    let ny2 = (cy - tw.win.drag_oy).max(TOPBAR_H as i32).min(h - tw.win.h - 28);

                    // Snap zone detection: show preview when cursor is near an edge.
                    let new_snap = if cx < SNAP_ZONE {
                        Some(SnapSide::Left)
                    } else if cx > w - SNAP_ZONE {
                        Some(SnapSide::Right)
                    } else if cy < TOPBAR_H as i32 + SNAP_ZONE {
                        Some(SnapSide::Top)
                    } else {
                        None
                    };
                    if new_snap != snap_preview {
                        snap_preview = new_snap;
                        scene_dirty = true;
                    }

                    if nx2 != tw.win.x || ny2 != tw.win.y {
                        // Damage band = union of old + new vertical span (plus a
                        // little slack for the cursor + shadow).
                        let y0 = oy.min(ny2) - 8;
                        let y1 = (oy + oh).max(ny2 + oh) + 10;
                        drag_band = Some((y0.max(0), y1.min(h)));
                        tw.win.x = nx2; tw.win.y = ny2;
                        scene_dirty = true;
                    }
                } else if tw.win.resizing {
                    let nw = (tw.win.resize_ow + cx - tw.win.resize_mx).max(wm::WIN_MIN_W);
                    let nh = (tw.win.resize_oh + cy - tw.win.resize_my).max(wm::WIN_MIN_H);
                    let nw = nw.min(w - tw.win.x);
                    let nh = nh.min(h - tw.win.y - 28);
                    if nw != tw.win.w || nh != tw.win.h {
                        tw.win.w = nw; tw.win.h = nh;
                        // Reflow the terminal to fill the new window content area.
                        let cw = (nw - 2).max(0) as usize;
                        let ch = (nh - 2 - wm::TITLEBAR_H).max(0) as usize;
                        let new_cols = (cw / term::CHAR_W).max(8);
                        let new_rows = (ch / term::CHAR_H).max(4);
                        tw.term.resize_term(new_cols, new_rows);
                        scene_dirty = true;
                    }
                }
            }
        } else {
            // Mouse button released — apply snap if a drag just ended.
            let was_dragging = wins.iter().any(|tw| tw.win.dragging);
            let was_active   = was_dragging || wins.iter().any(|tw| tw.win.resizing);

            if was_dragging {
                if let Some(side) = snap_preview.take() {
                    // Find the dragging window and apply the snap geometry.
                    if let Some(tw) = wins.iter_mut().find(|tw| tw.win.dragging) {
                        let usable_h = h - 28 - TOPBAR_H as i32;
                        // Save pre-snap restore position so the user can unsnap later.
                        tw.win.restore_x = tw.win.x;
                        tw.win.restore_y = tw.win.y;
                        tw.win.restore_w = tw.win.w;
                        tw.win.restore_h = tw.win.h;
                        match side {
                            SnapSide::Left => {
                                tw.win.x = 0; tw.win.y = TOPBAR_H as i32;
                                tw.win.w = w / 2; tw.win.h = usable_h;
                            }
                            SnapSide::Right => {
                                tw.win.x = w / 2; tw.win.y = TOPBAR_H as i32;
                                tw.win.w = w - w / 2; tw.win.h = usable_h;
                            }
                            SnapSide::Top => {
                                tw.win.toggle_maximize(w, h, TOPBAR_H as i32);
                            }
                        }
                    }
                } else {
                    snap_preview = None;
                }
            }

            for tw in wins.iter_mut() { tw.win.dragging = false; tw.win.resizing = false; }
            if was_active { scene_dirty = true; }
        }

        // Pump per-tick updates into animated apps (e.g. Snake). Throttled to
        // ~33 Hz (every 3rd 100 Hz tick) so a running game does not force a
        // full recomposite + 8 MB present at 100 Hz — 3x reduction in worst-case
        // MMIO bandwidth without any perceptible change in smoothness.
        let game_tick = now_ticks % 3 == 0;
        for tw in wins.iter_mut() {
            if tw.win.minimized || !game_tick { continue; }
            if let Some(app) = &mut tw.app {
                if app.tick(now_ticks) { tw.win_dirty = true; }
            }
        }

        // ── Single unified render pass per frame ──────────────────────────────
        // Render only when something visible actually changed. With multiple
        // windows we still only recompose on real state change — not every frame —
        // so the screen stops flickering.
        let cursor_moved = prev_cx != cx || prev_cy != cy;
        let any_chrome   = scene_dirty || wins.iter().any(|tw| tw.win_dirty && tw.win.desktop == current_desktop);
        let any_term     = wins.iter().any(|tw| tw.term.dirty && !tw.win.minimized && tw.win.desktop == current_desktop);

        if any_chrome || any_term || cursor_moved || topbar_due {
            restore_cursor_bg(&mut fb, prev_cx, prev_cy, &cbuf);

            if any_chrome {
                recomposite(&mut fb, &mut wins, start_menu_open, ctx_menu, &stats, blink_on, hover_icon, wallpaper_variant, snap_preview, current_desktop);
                scene_dirty = false;
            } else if any_term {
                // Partial: re-render only the focused terminal content on this desktop.
                let focused_idx = wins.iter().enumerate().rev()
                    .find(|(_, tw)| !tw.win.minimized && tw.win.desktop == current_desktop)
                    .map(|(i, _)| i);
                for (i, tw) in wins.iter_mut().enumerate() {
                    if tw.win.minimized || !tw.term.dirty { continue; }
                    if tw.win.desktop != current_desktop { continue; }
                    if tw.app.is_some() || tw.editor.is_some() { continue; }
                    let focused = Some(i) == focused_idx;
                    let (ox, oy) = wm::content_origin(&tw.win);
                    let cw = (tw.win.w - 2).max(0) as u32;
                    let ch = (tw.win.h - 3 - wm::TITLEBAR_H).max(0) as u32;
                    tw.term.render(&mut fb, ox as u32, oy as u32, cw, ch, focused && blink_on);
                    tw.term.dirty = false;
                }
                if start_menu_open { draw_start_menu(&mut fb); }
                if let Some((cmx, cmy)) = ctx_menu { draw_ctx_menu(&mut fb, cmx, cmy); }
                if unsafe { QS_OPEN } { draw_quick_settings(&mut fb); }
            }

            if topbar_due && !any_chrome {
                let up = rtc_str();
                draw_topbar(&mut fb, up.as_str(), &stats, now_ticks, current_desktop);
            }

            // Re-stamp the cursor on the focused terminal window on this desktop.
            if let Some(tw) = wins.iter_mut().rev()
                .find(|tw| !tw.win.minimized && tw.win.desktop == current_desktop
                           && tw.editor.is_none() && tw.app.is_none()) {
                let (ox, oy) = wm::content_origin(&tw.win);
                let cw = (tw.win.w - 2).max(0) as u32;
                let ch = (tw.win.h - 3 - wm::TITLEBAR_H).max(0) as u32;
                tw.term.paint_cursor(&mut fb, ox as u32, oy as u32, cw, ch, blink_on);
            }

            // schedesktop2: composite the second app's live surface into a window.
            // Drawn after the scene compose, before the cursor, so the cursor sits
            // on top and the window survives the next recomposite (re-blit each frame).
            if mp_surf_va != 0 {
                draw_mp_app_window(&mut fb, mp_surf_va);
            }

            save_cursor_bg(&fb, cx, cy, &mut cbuf);
            draw_cursor(&mut fb, cx, cy);

            // Flush the backbuffer to the real framebuffer. Sparse path: on a
            // pure drag frame (no terminal output, no topbar tick) only the
            // window's damage band changed, so present just those rows — the
            // rest of the screen is dormant. This skips the dominant full-screen
            // MMIO copy and is what makes 1080p dragging smooth. The backbuffer
            // is always fully correct, so a missed source just self-corrects on
            // the next full present. In windowed-app mode we full-present so the
            // app window is always flushed.
            match drag_band {
                Some((y0, y1)) if !any_term && !topbar_due && mp_surf_va == 0 =>
                    fb.present_rows(y0.max(0) as u32, y1.max(0) as u32),
                _ if !any_chrome && !any_term && !topbar_due && mp_surf_va == 0 => {
                    // Cursor-only frame: flush just the band the cursor swept through.
                    // Avoids the full 8 MB MMIO write (~40x cheaper) on every mouse tick.
                    let y_top = (prev_cy.min(cy) as u32).saturating_sub(2);
                    let y_bot = ((prev_cy.max(cy) as u32) + CURSOR_H + 4).min(fb.height);
                    fb.present_rows(y_top, y_bot);
                }
                _ => fb.present(),
            }
            // Right-click "Take Screenshot": now that a clean frame (menu gone) is
            // on screen, capture the backbuffer to screenshots/shot-N.ppm.
            if pending_screenshot {
                pending_screenshot = false;
                let _ = app::capture_fullscreen(&mut fb);
            }
            frames_since_stat = frames_since_stat.saturating_add(1);
        }
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    // Marker, then file:line if available — written byte-by-byte so we don't
    // need an allocator at panic time (the bump allocator may itself be the
    // cause of the panic).
    sys_serial_debug(b'\n');
    sys_serial_debug(b'!');
    if let Some(loc) = info.location() {
        for &b in loc.file().as_bytes() { sys_serial_debug(b); }
        sys_serial_debug(b':');
        let mut line = loc.line();
        if line == 0 {
            sys_serial_debug(b'0');
        } else {
            let mut buf = [0u8; 10];
            let mut i = 0;
            while line > 0 { buf[i] = b'0' + (line % 10) as u8; line /= 10; i += 1; }
            while i > 0 { i -= 1; sys_serial_debug(buf[i]); }
        }
    }
    sys_serial_debug(b'\n');
    loop {}
}
