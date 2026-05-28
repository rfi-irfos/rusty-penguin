#![no_std]
#![no_main]

extern crate alloc;

mod allocator;
mod app;
mod editor;
mod fb;
mod font;
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

// ---- Palette ────────────────────────────────────────────────────────────────
// Ubuntu-inspired color scheme: clean, modern, accessible
const BG:       u32 = 0x13131B;  // Deep charcoal (polished dark)
const TOPBAR:   u32 = 0x1A1A24;  // Slightly lighter charcoal topbar
const TASKBAR:  u32 = 0x0F0F17;  // Deep taskbar with subtle depth
const TOPBAR_H: u32 = 32;        // Slightly taller topbar for better proportions
const BORDER:   u32 = 0x2C2C38;  // Ubuntu-like medium contrast
const GREEN:    u32 = 0x5FDD8F;  // Warmer Ubuntu green
const DIM:      u32 = 0x6B7280;  // Better readable dim text
const DIMMER:   u32 = 0x2C2C38;  // Match border for consistency
const WHITE:    u32 = 0xF5F5F7;  // Warmer white (not pure white)
const AMBER:    u32 = 0xFFA500;  // Ubuntu-like warm accent
const BLUE:     u32 = 0x4A9EFF;  // Brighter, more vibrant blue
const CURSOR:   u32 = 0xF5F5F7;  // Match white
const TEAL:     u32 = 0x00D4AA;  // More vibrant teal

// Fill area (hot-spot at (0,0)).  Save/restore adds 1px border on all four sides.
const CURSOR_W:  u32 = 13;
const CURSOR_H:  u32 = 21;
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

fn cursor_mask(col: i32, row: i32) -> bool {
    if row < 0 || col < 0 || row >= CURSOR_H as i32 || col >= CURSOR_W as i32 { return false; }
    if row <= 10 { col <= row }   // expanding triangle head (11 rows)
    else         { col <= 1 }     // 2px shaft
}

fn draw_cursor(fb: &mut Framebuffer, x: i32, y: i32) {
    let outline = 0x000000u32;
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

fn draw_scene_static(fb: &mut Framebuffer) {
    let w = fb.width; let h = fb.height;
    let tb_y = h - 28;

    // Desktop gradient — smooth, refined Ubuntu-style background.
    // Subtle shift from charcoal to deep blue, giving depth without distraction.
    let total_rows = tb_y.saturating_sub(TOPBAR_H);
    let mut y = TOPBAR_H;
    while y < tb_y {
        let t = (y - TOPBAR_H) as u64 * 256 / total_rows.max(1) as u64; // 0..255
        // Smooth four-step gradient: charcoal → slate → midnight → deep charcoal
        let (r, g, b) = if t < 85 {
            // top: charcoal → slate
            let s = t * 3 / 85;
            (0x13u64 + s / 4, 0x13u64 + s / 3, 0x1Bu64 + s / 2)
        } else if t < 170 {
            // middle-top: slate → midnight blue
            let s = (t - 85) * 3 / 85;
            (0x16u64 - s / 8, 0x18u64 + s / 8, 0x26u64 + s / 3)
        } else {
            // bottom: midnight → deep charcoal
            let s = (t - 170) * 3 / 85;
            (0x14u64 - s / 16, 0x16u64 - s / 32, 0x24u64 - s / 16)
        };
        let col = ((r as u32) << 16) | ((g as u32) << 8) | b as u32;
        fb.fill_rect(0, y, w, 1, col);
        y += 1;
    }

    // Centered logo widget — 240×140px with modern styling
    const LW: u32 = 240; const LH: u32 = 140;
    let logo_ix = w.saturating_sub(LW) / 2;
    let logo_iy = TOPBAR_H + tb_y.saturating_sub(TOPBAR_H).saturating_sub(LH + 28) / 2;
    let lx = logo_ix as i32; let ly2 = logo_iy as i32;
    // Soft shadow for depth (Ubuntu-style)
    fb.fill_rect_s(lx + 3, ly2 + 3, LW as i32, LH as i32, 0x0A0A10);
    // Card background — refined modern look
    fb.fill_rounded_rect(lx, ly2, LW as i32, LH as i32, 10, 0x1A1A24);
    // Accent bar at top (warm gradient-like effect with green)
    fb.fill_rounded_rect(lx, ly2, LW as i32, 5, 3, GREEN);
    // Subtle border for definition
    fb.fill_rect_s(lx, ly2, LW as i32, 1, 0x3C3C48);
    // Dingir 3× icon (24×24) centered near top
    fb.draw_bitmap_3x(logo_ix + (LW - 24) / 2, logo_iy + 16, &DINGIR, GREEN, 0x1A1A24);
    // "RUSTY" in 2× white with better spacing
    fb.draw_str_2x(logo_ix + (LW - 5 * 16) / 2, logo_iy + 48, "RUSTY", WHITE, 0x1A1A24);
    // "PENGUIN" in 2× green
    fb.draw_str_2x(logo_ix + (LW - 7 * 16) / 2, logo_iy + 70, "PENGUIN", GREEN, 0x1A1A24);
    // Version — subtle
    fb.draw_str(logo_ix + (LW - 10 * 8) / 2, logo_iy + 100, "v1.0.0-bm", DIM, 0x1A1A24);
    // Tagline
    let tag = "Bare-metal Rust OS · Sparse ternary inference";
    let tag_y = logo_iy + LH + 16;
    if tag_y + 8 < tb_y {
        fb.draw_str(w.saturating_sub(tag.len() as u32 * 8) / 2, tag_y, tag, DIM, BG);
    }

    // ── Taskbar — gradient lighter at top, darker at bottom ──
    fb.fill_rect(0, tb_y, w, 28, TASKBAR);
    for dy in 0..28u32 {
        let blend = (7u8).saturating_sub((dy * 7 / 27) as u8);
        let col = (0x0Bu8.saturating_add(blend) as u32) << 16
                | (0x14u8.saturating_add(blend) as u32) << 8
                |  0x1Au8.saturating_add(blend) as u32;
        fb.fill_rect(0, tb_y + dy, w, 1, col);
    }
    // Separator — subtle, clean line
    fb.fill_rect(0, tb_y, w, 1, 0x2C2C38);
    // Menu button — refined rounded style
    fb.fill_rounded_rect(4, tb_y as i32 + 3, 62, 22, 5, 0x2C2C38);
    fb.fill_rect(4, tb_y + 3, 62, 1, GREEN);  // green top accent
    fb.draw_bitmap_2x(8, tb_y + 6, &DINGIR, GREEN, 0x2C2C38);
    fb.draw_str(30, tb_y + 9, "Menu", WHITE, 0x2C2C38);
    // Left icon dock panel — refined appearance
    let dock_h = (tb_y - TOPBAR_H).saturating_sub(8);
    fb.fill_rounded_rect(4, TOPBAR_H as i32 + 4, 62, dock_h as i32, 12, 0x1A1A24);
    fb.fill_rect(4, TOPBAR_H + 4, 2, dock_h, 0x3C3C48);   // left edge highlight
    fb.fill_rect(64, TOPBAR_H + 4, 2, dock_h, 0x0F0F17);  // right edge shadow

    // Separator after menu button
    fb.fill_rect(70, tb_y + 5, 1, 18, 0x2C2C38);
    // "Show desktop" strip — far right of taskbar
    fb.fill_rect(w - 6, tb_y, 6, 28, 0x1A1A24);   // refined backing
    fb.fill_rect(w - 5, tb_y, 1, 28, GREEN);      // green accent stripe

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

fn draw_topbar(fb: &mut Framebuffer, time: &str, s: &SysStats, ticks: u64) {
    let fw = fb.width;
    // Solid topbar with subtle gradient for depth (Ubuntu-like)
    for dy in 0..TOPBAR_H {
        let blend = (dy * 6 / TOPBAR_H) as u8;
        let col = (0x1Au8.saturating_add(blend) as u32) << 16
                | (0x1Au8.saturating_add(blend) as u32) << 8
                |  0x24u8.saturating_add(blend) as u32;
        fb.fill_rect(0, dy, fw, 1, col);
    }
    // Bottom border for definition
    fb.fill_rect(0, TOPBAR_H - 1, fw, 1, 0x3C3C48);
    // Left accent bar (smaller, more refined)
    fb.fill_rect(0, 0, 2, TOPBAR_H, GREEN);
    let ty = (TOPBAR_H / 2).saturating_sub(4);

    // LEFT: brand mark with better spacing
    fb.draw_bitmap_2x(8, ty.saturating_sub(3), &DINGIR, GREEN, TOPBAR);
    fb.draw_str(28, ty, "Rusty Penguin", WHITE, TOPBAR);

    // CENTER: uptime clock
    let cx = (fw - time.len() as u32 * 8) / 2;
    fb.draw_str(cx, ty, time, WHITE, TOPBAR);

    // RIGHT: trit indicator + memory bar + pct label
    let ind = trit_indicator(ticks);
    let ind_str = core::str::from_utf8(&ind).unwrap_or("T[+--+]");
    let mut rx = fw as i32 - 6;

    // Trit indicator
    rx -= ind_str.len() as i32 * 8;
    if rx > 120 { fb.draw_str(rx as u32, ty, ind_str, AMBER, 0x090F1B); }
    rx -= 8;

    // Memory percentage text ("xx%")
    let mut pct_buf = Strbuf::new();
    pct_buf.push_u64(s.mem_pct as u64);
    pct_buf.push(b'%');
    let pct_str = pct_buf.as_str();
    rx -= pct_str.len() as i32 * 8;
    let mem_col = if s.mem_pct > 80 { 0xEF4444u32 } else if s.mem_pct > 60 { AMBER } else { GREEN };
    if rx > 120 { fb.draw_str(rx as u32, ty, pct_str, mem_col, 0x090F1B); }
    rx -= 4;

    // Memory bar (52×8, inside a 1px dark track)
    const BAR_W: i32 = 52;
    rx -= BAR_W + 2;
    let bar_y = ty as i32 - 1;
    if rx > 120 {
        fb.fill_rect_s(rx, bar_y, BAR_W + 2, 10, 0x060C18);      // outer track
        fb.fill_rect_s(rx + 1, bar_y + 1, BAR_W, 8, 0x0E1C2C);   // inner track
        let fill = (BAR_W * s.mem_pct as i32 / 100).max(2);
        fb.fill_rect_s(rx + 1, bar_y + 1, fill, 8, mem_col);
    }
    rx -= 6;

    // "MEM" label
    if rx > 120 { fb.draw_str((rx - 24) as u32, ty, "MEM", DIM, 0x090F1B); }
}

// ---- Launcher buttons ───────────────────────────────────────────────────────

struct Launcher { label: &'static str, cmd: Option<&'static str>, title: &'static str, color: u32 }
const LAUNCHERS: &[Launcher] = &[
    Launcher { label: " psh ", cmd: None,               title: "psh - Terminal",   color: GREEN },
    Launcher { label: "files", cmd: Some("ls -la\n"),   title: "Files",            color: BLUE  },
    Launcher { label: "nano ", cmd: Some("nano\n"),     title: "Text Editor",      color: AMBER },
    Launcher { label: " ps  ", cmd: Some("ps\n"),       title: "ps - Processes",   color: 0xA0D0FF },
    Launcher { label: " ai  ", cmd: Some("ai 32\n"),    title: "ai - Inference",   color: 0xFFD700 },
];


// ---- Desktop icon shortcuts ─────────────────────────────────────────────────
// Fixed 4 icons. Each is 72×64px: 48px image area + 8px label + 8px margin.

struct DesktopIcon {
    label: &'static str,
    bitmap: &'static [u8; 8],
    color: u32,
    launcher_idx: usize,
}

const DESKTOP_ICONS: &[DesktopIcon] = &[
    DesktopIcon { label: "Term",  bitmap: &ICON_TERM,  color: GREEN,   launcher_idx: 0 },
    DesktopIcon { label: "Files", bitmap: &ICON_FILES, color: BLUE,    launcher_idx: 1 },
    DesktopIcon { label: "Edit",  bitmap: &ICON_AI,    color: AMBER,   launcher_idx: 2 },
    DesktopIcon { label: "Procs", bitmap: &ICON_PROC,  color: 0xA0D0FF, launcher_idx: 3 },
    DesktopIcon { label: "Cal",   bitmap: &ICON_TRIT,  color: 0xC4B5FD,  launcher_idx: 4 },
    DesktopIcon { label: "Prefs", bitmap: &ICON_PROC,  color: 0x9CA3AF,  launcher_idx: 5 },
    DesktopIcon { label: "TIS",   bitmap: &ICON_TRIT,  color: 0x4A9EFF,  launcher_idx: 6 },
];

// 56px wide keeps icons safely left of the default window start (x=79)
const DICON_W: u32 = 56;
const DICON_H: u32 = 60;   // 48px image area + 8px label + 4px gap below label
const DICON_X: u32 = 10;   // left margin → right edge at 66px
const DICON_GAP: u32 = 8;  // vertical gap between icons

fn dicon_rect(i: usize) -> (u32, u32, u32, u32) {
    let y = TOPBAR_H + 16 + i as u32 * (DICON_H + DICON_GAP);
    (DICON_X, y, DICON_W, DICON_H)
}

fn draw_desktop_icons(fb: &mut Framebuffer, hover_icon: Option<usize>) {
    for (i, icon) in DESKTOP_ICONS.iter().enumerate() {
        let (x, y, w, h) = dicon_rect(i);
        let img_h: u32 = h - 12;
        let ix = x as i32; let iy = y as i32;
        let iw = w as i32; let ih = img_h as i32;
        let hovered = hover_icon == Some(i);
        // Modern shadow effect (multi-layer for depth)
        let sd = if hovered { 3 } else { 2 };
        fb.fill_rounded_rect(ix + sd, iy + sd, iw, ih, 8, 0x00000040.min(0x0A0A14));
        // Icon background with hover effect
        let inner_bg = if hovered { 0x2C2C38u32 } else { 0x1A1A24u32 };
        let border_col = if hovered { icon.color } else { 0x3C3C48u32 };
        // Rounded card design
        fb.fill_rounded_rect(ix,     iy,     iw,     ih,     8, border_col);
        fb.fill_rounded_rect(ix + 1, iy + 1, iw - 2, ih - 2, 7, inner_bg);
        // Accent top bar
        fb.fill_rect_s(ix + 1, iy + 1, iw - 2, 2, icon.color);
        // 2x bitmap centered
        let bx = x + (w - 16) / 2;
        let by = y + (img_h - 16) / 2;
        let icon_color = if hovered { icon.color } else { 0x6B7280u32 };
        fb.draw_bitmap_2x(bx, by, icon.bitmap, icon_color, inner_bg);
        // Label with refined styling
        let lw = icon.label.len() as u32 * 8;
        let lx = if lw < w { x + (w - lw) / 2 } else { x };
        let label_bg = if hovered { 0x2C2C38u32 } else { 0x1A1A24u32 };
        let label_color = if hovered { icon.color } else { DIM };
        fb.fill_rect(lx.saturating_sub(2), y + img_h + 2, lw + 4, 11, label_bg);
        fb.draw_str(lx, y + img_h + 4, icon.label, label_color, label_bg);
    }
}

fn desktop_icon_hit(mx: i32, my: i32) -> Option<usize> {
    for i in 0..DESKTOP_ICONS.len() {
        let (x, y, w, h) = dicon_rect(i);
        if mx >= x as i32 && mx < (x + w) as i32
            && my >= y as i32 && my < (y + h) as i32
        {
            return Some(i);
        }
    }
    None
}


// ---- Taskbar window buttons ─────────────────────────────────────────────────

fn tbwin_rect(_fw: u32, fh: u32, slot: usize) -> (i32, i32, i32, i32) {
    (160 + slot as i32 * 100, (fh - 22) as i32, 92, 18)
}

fn draw_taskbar_win_btns(fb: &mut Framebuffer, term_wins: &[TermWin]) {
    let fw = fb.width; let fh = fb.height;
    let n = term_wins.len();
    for (slot, tw) in term_wins.iter().enumerate() {
        let (x, y, w, h) = tbwin_rect(fw, fh, slot);
        if x + w >= fw as i32 { break; }
        let is_focused   = slot == n - 1;
        let is_minimized = tw.win.minimized;
        // Modern colors that match system palette
        let bg  = if is_minimized { 0x1A1A24u32 } else { 0x2C2C38u32 };
        let txt = if is_minimized { 0x6B7280u32 } else { WHITE };
        let accent = if is_focused { BLUE } else { 0x3C3C48u32 };
        // Rounded pill with subtle shadow
        fb.fill_rounded_rect(x - 1, y - 1, w + 2, h + 2, 5, 0x0A0A14);
        fb.fill_rounded_rect(x, y, w, h, 5, bg);
        // Accent indicator (bottom stripe for focused)
        if is_focused {
            fb.fill_rect_s(x + 2, y + h - 2, w - 4, 2, accent);
        } else {
            fb.fill_rect_s(x + 2, y + h - 1, w - 4, 1, 0x3C3C48);
        }
        // App indicator dot
        fb.fill_circle(x + 6, y + h / 2, 2, accent);
        // Window title with proper truncation
        let title = tw.win.title.as_str();
        let short = title.find(" - ").map(|i| &title[..i]).unwrap_or(title);
        let max_chars = ((w - 18) / 8).max(0) as usize;
        let lbl = &short[..max_chars.min(short.len())];
        fb.draw_str((x + 14) as u32, (y + 4) as u32, lbl, txt, bg);
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
    fb.draw_str(x, tb_y + 10, hhmm, WHITE, TASKBAR);
}

fn tbwin_hit(fw: u32, fh: u32, wins: &[TermWin], mx: i32, my: i32) -> Option<usize> {
    for (slot, _) in wins.iter().enumerate() {
        let (x, y, w, h) = tbwin_rect(fw, fh, slot);
        if mx >= x && mx < x + w && my >= y && my < y + h { return Some(slot); }
    }
    None
}

// ---- Start menu ─────────────────────────────────────────────────────────────

fn dingir_hit(fh: u32, mx: i32, my: i32) -> bool {
    let tb_y = fh as i32 - 28;
    mx >= 4 && mx < 24 && my >= tb_y + 4 && my < tb_y + 24
}

fn show_desktop_hit(fw: u32, fh: u32, mx: i32, my: i32) -> bool {
    mx >= fw as i32 - 6 && mx < fw as i32
        && my >= fh as i32 - 28 && my < fh as i32
}

fn start_menu_bounds(fh: u32) -> (i32, i32, i32, i32) {
    let h = 14 + LAUNCHERS.len() as i32 * 20 + 4;
    let w = 160i32;
    (2, fh as i32 - 28 - h, w, h)
}

fn draw_start_menu(fb: &mut Framebuffer) {
    let (x, y, w, h) = start_menu_bounds(fb.height);
    // Modern drop shadow (multi-layer)
    fb.fill_rounded_rect(x + 2, y + 2, w, h, 8, 0x00000080.min(0x0A0A14));
    // Panel background — refined
    fb.fill_rounded_rect(x, y, w, h, 8, 0x1A1A24);
    // Header with accent
    fb.fill_rounded_rect(x, y, w, 20, 6, 0x2C2C38);
    fb.fill_rect_s(x, y, w, 3, GREEN);  // top accent stripe
    fb.draw_bitmap_2x((x + 3) as u32, (y + 4) as u32, &DINGIR, GREEN, 0x2C2C38);
    fb.draw_str((x + 23) as u32, (y + 6) as u32, "RUSTY PENGUIN", WHITE, 0x2C2C38);
    fb.fill_rect_s(x, y + 19, w, 1, 0x3C3C48);  // separator
    // Menu items
    for (i, l) in LAUNCHERS.iter().enumerate() {
        let iy = y + 20 + i as i32 * 20;
        let bg = 0x1A1A24u32;  // consistent background
        fb.fill_rect_s(x + 1, iy, w - 2, 20, bg);
        // Colored left accent bar per item
        fb.fill_rect_s(x + 2, iy + 4, 2, 12, l.color);
        fb.draw_str((x + 8) as u32, (iy + 6) as u32, l.label, l.color, bg);
        let desc = l.title.split('-').nth(1).unwrap_or(l.title).trim();
        fb.draw_str((x + 52) as u32, (iy + 6) as u32, desc, DIM, bg);
    }
}

fn start_menu_hit(fh: u32, mx: i32, my: i32) -> Option<usize> {
    let (x, y, w, _) = start_menu_bounds(fh);
    if mx < x || mx >= x + w { return None; }
    for i in 0..LAUNCHERS.len() {
        let iy = y + 16 + i as i32 * 20;
        if my >= iy && my < iy + 20 { return Some(i); }
    }
    None
}

// ---- Right-click context menu ───────────────────────────────────────────────

const CTX_ITEMS: &[(&str, u32)] = &[
    ("New Terminal",      0x4ADE80),
    ("Close All Windows", 0xEF4444),
    ("Refresh Desktop",   0x60A5FA),
];

fn ctx_menu_bounds(mx: i32, my: i32, fw: u32, fh: u32) -> (i32, i32, i32, i32) {
    let w = 148i32;
    let h = 4 + CTX_ITEMS.len() as i32 * 20;
    let x = mx.min(fw as i32 - w - 4).max(0);
    let y = my.min(fh as i32 - h - 4).max(0);
    (x, y, w, h)
}

fn draw_ctx_menu(fb: &mut Framebuffer, mx: i32, my: i32) {
    let (x, y, w, h) = ctx_menu_bounds(mx, my, fb.width, fb.height);
    // Modern shadow effect
    fb.fill_rounded_rect(x + 2, y + 2, w, h, 6, 0x0A0A14);
    // Menu background matching system palette
    fb.fill_rounded_rect(x, y, w, h, 6, 0x1A1A24);
    // Top accent line
    fb.fill_rect_s(x, y, w, 2, GREEN);
    for (i, (label, color)) in CTX_ITEMS.iter().enumerate() {
        let iy = y + 2 + i as i32 * 20;
        let bg = 0x1A1A24u32;  // consistent background
        fb.fill_rect_s(x + 1, iy, w - 2, 20, bg);
        // Color accent bar (smaller, refined)
        fb.fill_rect_s(x + 2, iy + 4, 2, 12, *color);
        fb.draw_str((x + 8) as u32, (iy + 6) as u32, label, *color, bg);
    }
}

fn ctx_menu_item_hit(mx: i32, my: i32, cmx: i32, cmy: i32, fw: u32, fh: u32) -> Option<usize> {
    let (x, y, w, _) = ctx_menu_bounds(cmx, cmy, fw, fh);
    if mx < x || mx >= x + w { return None; }
    for i in 0..CTX_ITEMS.len() {
        let iy = y + 2 + i as i32 * 20;
        if my >= iy && my < iy + 20 { return Some(i); }
    }
    None
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
        Ok(t) => {
            let off = n as i32 * 20;
            let left_margin = 75;
            let wx = ((w - left_margin - wm::WINDOW_W) / 2 + left_margin + off)
                .max(left_margin)
                .min(w - wm::WINDOW_W);
            let wy = ((h - wm::WINDOW_H - 28) / 2 + off).max(TOPBAR_H as i32).min(h - wm::WINDOW_H - 28);
            Some(TermWin {
                win: wm::Window::new(wx, wy, l.title),
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
            Some(TermWin {
                win: wm::Window::new(wx, wy, "Process Monitor"),
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

// ---- Full scene recomposite ─────────────────────────────────────────────────

fn recomposite(fb: &mut Framebuffer, wins: &mut Vec<TermWin>, start_menu: bool, ctx_menu: Option<(i32,i32)>, stats: &SysStats, blink_on: bool, hover_icon: Option<usize>) {
    draw_scene_static(fb);
    draw_desktop_icons(fb, hover_icon);
    draw_taskbar_win_btns(fb, wins);
    let up = rtc_str();
    let n = wins.len();
    for (i, tw) in wins.iter_mut().enumerate() {
        if tw.win.minimized { continue; }
        let focused = i == n - 1;
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
    draw_topbar(fb, up.as_str(), stats, sys_ticks());
}

// ── Entry point ──────────────────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let mut fb = match Framebuffer::open() {
        Ok(f) => f,
        Err(_) => loop { sys_yield(); },
    };

    let w = fb.width as i32; let h = fb.height as i32;
    let mut mouse = MouseState { x: w / 2, y: h / 2, buttons: 0, btn_pressed: 0 };

    let mut stats = sample_stats();
    draw_scene_static(&mut fb);
    draw_desktop_icons(&mut fb, None);
    let up0 = rtc_str();
    draw_topbar(&mut fb, up0.as_str(), &stats, sys_ticks());

    let cbl = (CURSOR_BW * CURSOR_BH) as usize;
    let mut cbuf = vec![BG; cbl];
    let mut cx = mouse.x; let mut cy = mouse.y;
    save_cursor_bg(&fb, cx, cy, &mut cbuf);
    draw_cursor(&mut fb, cx, cy);

    let mut last_topbar_tick: u64 = 0;
    let mut last_blink_tick: u64 = 0;
    let mut blink_on: bool = true;
    let mut drag_tick: u64 = 0;
    let mut wins: Vec<TermWin> = Vec::new();
    let mut scene_dirty = false;
    let mut start_menu_open = false;
    let mut ctx_menu: Option<(i32, i32)> = None;
    let mut hover_icon: Option<usize> = None;

    if let Some(tw) = open_term(w, h, 0, &LAUNCHERS[0]) {
        wins.push(tw);
        scene_dirty = true;
    }

    loop {
        sys_yield();

        let keys = input::poll(&mut mouse, w, h);
        let btn = mouse.buttons;

        // Snapshot old cursor position — the unified render uses this for restore.
        let prev_cx = cx; let prev_cy = cy;

        // Keyboard → global shortcuts first, then focused terminal/editor
        for &k in keys.iter() {
            if k == 0x14 { // Ctrl+T
                if let Some(tw) = open_term(w, h, wins.len(), &LAUNCHERS[0]) {
                    wins.push(tw);
                    scene_dirty = true;
                }
            } else if k == 0x17 { // Ctrl+W
                if !wins.is_empty() {
                    wins.pop();
                    scene_dirty = true;
                }
            } else if k == 0x13 { // Ctrl+S (save editor)
                if let Some(tw) = wins.last_mut() {
                    if let Some(ed) = &mut tw.editor {
                        ed.save();
                    }
                }
            } else if k == 0x11 { // Ctrl+Q (close editor)
                if let Some(tw) = wins.last_mut() {
                    if let Some(ed) = &mut tw.editor {
                        ed.wants_close = true;
                    }
                }
            } else if let Some(tw) = wins.last_mut() {
                if let Some(ed) = &mut tw.editor {
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

        // Close windows that typed `exit`
        if wins.iter().any(|tw| tw.editor.is_some() || tw.term.wants_close) {
            wins.retain(|tw| !(tw.editor.as_ref().map(|ed| ed.wants_close).unwrap_or(false) || tw.term.wants_close));
            scene_dirty = true;
        }

        // Topbar: check if due; defer actual draw to the unified render pass.
        let now_ticks = sys_ticks();
        let topbar_due = now_ticks.wrapping_sub(last_topbar_tick) >= 200;
        if topbar_due {
            last_topbar_tick = now_ticks;
            stats = sample_stats();
        }

        // Text cursor blink: toggle every 50 ticks (~500ms @ 100Hz).
        // Only the focused (topmost) terminal shows a cursor at all.
        if now_ticks.wrapping_sub(last_blink_tick) >= 50 {
            last_blink_tick = now_ticks;
            blink_on = !blink_on;
            // Mark focused terminal dirty so the blink triggers a re-render.
            if let Some(tw) = wins.iter_mut().rev().find(|tw| !tw.win.minimized) {
                tw.term.dirty = true;
            }
        }

        // Update cursor to new position so click/drag handlers see current coords.
        cx = mouse.x; cy = mouse.y;

        // Desktop icon hover — triggers a recomposite when it changes.
        let new_hover = desktop_icon_hit(cx, cy);
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
            let on_win = wins.iter().any(|tw| wm::window_hit(&tw.win, cx, cy));
            if !on_win && cy >= TOPBAR_H as i32 && cy < h - 28 {
                ctx_menu = Some((cx, cy));
            }
            if ctx_menu.is_some() || prev_open { scene_dirty = true; }
        }

        if left_edge {
            if let Some((cmx, cmy)) = ctx_menu.take() {
                if let Some(item) = ctx_menu_item_hit(cx, cy, cmx, cmy, fb.width, fb.height) {
                    match item {
                        0 => {
                            if let Some(tw) = open_term(w, h, wins.len(), &LAUNCHERS[0]) {
                                wins.push(tw);
                            }
                        }
                        1 => { wins.clear(); }
                        _ => {}
                    }
                }
                scene_dirty = true;
            } else if start_menu_open {
                if let Some(li) = start_menu_hit(fb.height, cx, cy) {
                    if let Some(tw) = open_term(w, h, wins.len(), &LAUNCHERS[li]) {
                        wins.push(tw);
                    }
                }
                start_menu_open = false;
                scene_dirty = true;
            } else if dingir_hit(fb.height, cx, cy) {
                start_menu_open = true;
                scene_dirty = true;
            } else {
                let hit = wins.iter().enumerate().rev()
                    .find(|(_, tw)| wm::window_hit(&tw.win, cx, cy))
                    .map(|(i, _)| i);

                if let Some(hi) = hit {
                    if hi != wins.len() - 1 {
                        let tw = wins.remove(hi); wins.push(tw);
                        wins.last_mut().unwrap().win_dirty = true;
                    }
                    let last = wins.len() - 1;
                    let tw = &mut wins[last];

                    if wm::close_btn_hit(&tw.win, cx, cy) {
                        wins.remove(last);
                        scene_dirty = true;
                    } else if wm::min_btn_hit(&tw.win, cx, cy) {
                        tw.win.toggle_maximize(w, h, TOPBAR_H as i32);
                        scene_dirty = true;
                    } else if wm::max_btn_hit(&tw.win, cx, cy) {
                        tw.win.minimized = true;
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
                    }
                } else {
                    if show_desktop_hit(fb.width, fb.height, cx, cy) {
                        for tw in wins.iter_mut() { tw.win.minimized = true; }
                        scene_dirty = true;
                    } else if let Some(mi) = tbwin_hit(fb.width, fb.height, &wins, cx, cy) {
                        wins[mi].win.minimized = false;
                        let tw = wins.remove(mi); wins.push(tw);
                        scene_dirty = true;
                    } else if let Some(di) = desktop_icon_hit(cx, cy) {
                        match di {
                            1 => { // Files icon → FileManager app
                                if let Some(tw) = open_file_manager(w, h, wins.len()) {
                                    wins.push(tw);
                                    scene_dirty = true;
                                }
                            }
                            2 => { // Edit icon → graphical text editor
                                if let Some(tw) = open_editor(w, h, wins.len(), "readme.txt", "Text Editor") {
                                    wins.push(tw);
                                    scene_dirty = true;
                                }
                            }
                            3 => { // Procs icon → Process Monitor
                                if let Some(tw) = open_process_monitor(w, h, wins.len()) {
                                    wins.push(tw);
                                    scene_dirty = true;
                                }
                            }
                            4 => { // Cal icon → Calendar app
                                if let Some(tw) = open_calendar(w, h, wins.len()) {
                                    wins.push(tw);
                                    scene_dirty = true;
                                }
                            }
                            5 => { // Prefs icon → Settings app
                                if let Some(tw) = open_settings(w, h, wins.len()) {
                                    wins.push(tw);
                                    scene_dirty = true;
                                }
                            }
                            6 => { // TIS icon → TIS Console
                                if let Some(tw) = open_tis_console(w, h, wins.len()) {
                                    wins.push(tw);
                                    scene_dirty = true;
                                }
                            }
                            _ => { // Other icons → terminal with launcher command
                                let li = DESKTOP_ICONS[di].launcher_idx;
                                if let Some(tw) = open_term(w, h, wins.len(), &LAUNCHERS[li]) {
                                    wins.push(tw);
                                    scene_dirty = true;
                                }
                            }
                        }
                    }
                }
            }
        }

        // Drag / resize
        if left_down {
            if let Some(tw) = wins.last_mut() {
                if tw.win.dragging {
                    let nx2 = (cx - tw.win.drag_ox).max(75).min(w - tw.win.w);
                    let ny2 = (cy - tw.win.drag_oy).max(TOPBAR_H as i32).min(h - tw.win.h - 28);
                    if nx2 != tw.win.x || ny2 != tw.win.y {
                        tw.win.x = nx2; tw.win.y = ny2;
                        // Rate-limit drag recomposites to ~25Hz (4 ticks @ 100Hz) for smooth motion.
                        // Cursor still updates every frame; only the window position
                        // repaints are throttled to cut framebuffer write pressure.
                        if now_ticks.wrapping_sub(drag_tick) >= 4 {
                            drag_tick = now_ticks;
                            scene_dirty = true;
                        }
                    }
                } else if tw.win.resizing {
                    let nw = (tw.win.resize_ow + cx - tw.win.resize_mx).max(wm::WIN_MIN_W);
                    let nh = (tw.win.resize_oh + cy - tw.win.resize_my).max(wm::WIN_MIN_H);
                    let nw = nw.min(w - tw.win.x);
                    let nh = nh.min(h - tw.win.y - 28);
                    if nw != tw.win.w || nh != tw.win.h {
                        tw.win.w = nw; tw.win.h = nh;
                        if now_ticks.wrapping_sub(drag_tick) >= 4 {
                            drag_tick = now_ticks;
                            scene_dirty = true;
                        }
                    }
                }
            }
        } else {
            // Drag/resize ended — flush final position unconditionally.
            let was_active = wins.iter().any(|tw| tw.win.dragging || tw.win.resizing);
            for tw in wins.iter_mut() {
                tw.win.dragging = false;
                tw.win.resizing = false;
            }
            if was_active { scene_dirty = true; }
        }

        // ── Single unified render pass per frame ──────────────────────────────
        // With multiple windows, always do full recomposite to avoid flickering.
        // Single window can use partial rendering for efficiency.
        let cursor_moved = prev_cx != cx || prev_cy != cy;
        let any_chrome   = scene_dirty || wins.iter().any(|tw| tw.win_dirty);
        let any_content  = wins.iter().any(|tw| (tw.term.dirty || tw.editor.is_some()) && !tw.win.minimized);
        let multi_window = wins.len() > 1;
        let force_full_composite = any_chrome || multi_window;

        if any_chrome || any_content || cursor_moved || topbar_due {
            restore_cursor_bg(&mut fb, prev_cx, prev_cy, &cbuf);

            if force_full_composite {
                recomposite(&mut fb, &mut wins, start_menu_open, ctx_menu, &stats, blink_on, hover_icon);
                scene_dirty = false;
            } else if any_content {
                let n = wins.len();
                for (i, tw) in wins.iter_mut().enumerate() {
                    if tw.win.minimized { continue; }
                    let is_dirty = tw.term.dirty || tw.editor.is_some();
                    if !is_dirty { continue; }
                    let focused = i == n - 1;
                    let (ox, oy) = wm::content_origin(&tw.win);
                    let cw = (tw.win.w - 2).max(0) as u32;
                    let ch = (tw.win.h - 3 - wm::TITLEBAR_H).max(0) as u32;
                    if let Some(ed) = &mut tw.editor {
                        ed.render(&mut fb, ox as u32, oy as u32, cw, ch);
                    } else {
                        tw.term.render(&mut fb, ox as u32, oy as u32, cw, ch, focused && blink_on);
                    }
                    tw.term.dirty = false;
                }
                if start_menu_open { draw_start_menu(&mut fb); }
                if let Some((cmx, cmy)) = ctx_menu { draw_ctx_menu(&mut fb, cmx, cmy); }
            }

            if topbar_due && !any_chrome {
                let up = rtc_str();
                draw_topbar(&mut fb, up.as_str(), &stats, now_ticks);
            }

            // Re-stamp the focused terminal cursor (editors include cursor in render).
            {
                let n = wins.len();
                if let Some((fi, tw)) = wins.iter_mut().enumerate().rev().find(|(_, tw)| !tw.win.minimized && tw.editor.is_none()) {
                    let focused = fi == n - 1;
                    let (ox, oy) = wm::content_origin(&tw.win);
                    let cw = (tw.win.w - 2).max(0) as u32;
                    let ch = (tw.win.h - 3 - wm::TITLEBAR_H).max(0) as u32;
                    tw.term.paint_cursor(&mut fb, ox as u32, oy as u32, cw, ch, focused && blink_on);
                }
            }

            save_cursor_bg(&fb, cx, cy, &mut cbuf);
            draw_cursor(&mut fb, cx, cy);
        }

        // Save button state for next frame's edge detection
        mouse.btn_pressed = btn;
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    sys_serial_debug(b'!'); // '!' = panic in serial log
    loop {}
}
