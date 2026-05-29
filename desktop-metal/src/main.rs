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
// "Rusty Penguin v2" warm-stone-green palette — from Simeon's HTML design mockup
// (rusty-penguin-os.html). Warm dark stone, spring green, gold/cream, ternary triad.
const BG:       u32 = 0x1B211E;  // Warm stone wall (--wall deep)
const TOPBAR:   u32 = 0x252E2A;  // Warm panel topbar (--wall2/panel)
const TASKBAR:  u32 = 0x1B211E;  // Dock backing (warm deep)
const TOPBAR_H: u32 = 32;        // Topbar height
const BORDER:   u32 = 0x2A332F;  // Warm hairline / medium contrast (--panel-solid)
const GREEN:    u32 = 0x6FE18B;  // Spring green (--green / --pos)
const DIM:      u32 = 0xA8B0A6;  // Secondary text (--txt-dim)
const DIMMER:   u32 = 0x2A332F;  // Match border
const WHITE:    u32 = 0xECEDE5;  // Warm off-white (--txt)
const AMBER:    u32 = 0xF5C451;  // Warm amber accent
const BLUE:     u32 = 0x8CC6E5;  // Warm sky (--sky)
const CURSOR:   u32 = 0x14171A;  // arrow fill — near-black (white halo via outline)
const TEAL:     u32 = 0x00D4AA;  // More vibrant teal
const ACCENT_CREAM: u32 = 0xECDAA7;  // dingir gold (--cream)
const TRIT_NEG:  u32 = 0xEF7575;  // ternary -1
const TRIT_ZERO: u32 = 0x909A92;  // ternary 0
const TRIT_POS:  u32 = 0x6FE18B;  // ternary +1

// ── Bottom panel layout (Simeon's v2 mockup form: a single floating bottom dock
// — Menu · favourites · tasks · tray — and NO top bar). ─────────────────────────
const PANEL_MARGIN: i32 = 14;   // inset from screen left/right
const PANEL_BOTTOM: i32 = 12;   // gap below the panel
const PANEL_H:      i32 = 54;   // panel height
const MENU_BTN_W:   i32 = 72;   // "Menu" button width
const FAV_TILE:     i32 = 40;   // favourite icon tile
const FAV_GAP:      i32 = 8;    // gap between favourites
const PANEL_SOLID:  u32 = 0x333D38;  // dock body — warm stone, lighter than the wall so it floats
const PANEL_EDGE:   u32 = 0x55615A;  // panel hairline / top sheen
const PANEL_R:      i32 = 14;        // panel corner radius

fn panel_top(h: u32) -> i32 { h as i32 - PANEL_BOTTOM - PANEL_H }
fn menu_btn_rect(h: u32) -> (i32, i32, i32, i32) { (PANEL_MARGIN + 8, panel_top(h) + 7, MENU_BTN_W, 40) }

// Dock favourites — the mockup pins exactly 5 (Terminal, Files, TIS, Editor,
// Calculator); everything else lives in the start menu. Values index into
// DESKTOP_ICONS so the existing click-handler match (keyed on that index) is
// unchanged. Keeping the dock to 5 also avoids the 10-icon overflow regression.
const N_FAV: usize = 6;
const FAV_IDX: [usize; N_FAV] = [0, 1, 10, 2, 3, 6]; // Term, Files, Web, Edit, Calc, TIS

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

fn draw_scene_static(fb: &mut Framebuffer) { draw_scene_static_v(fb, 0); }

fn draw_scene_static_v(fb: &mut Framebuffer, variant: u8) {
    let w = fb.width; let h = fb.height;
    let ptop = panel_top(h);

    // Wallpaper — warm-stone gradient, 4 variants for "Change Background".
    // v0: warm stone green (default), v1: cool slate, v2: deep night, v3: sunset amber.
    let mut y = 0u32;
    while y < h {
        let t = y as u64 * 256 / h as u64;
        let (r, g, b) = match variant {
            1 => { // Cool slate blue-grey
                let r = 0x1Cu64.saturating_sub(0x06 * t / 255);
                let g = 0x22u64.saturating_sub(0x08 * t / 255);
                let b = 0x2Eu64.saturating_sub(0x0A * t / 255);
                (r, g, b)
            }
            2 => { // Deep forest night
                let r = 0x10u64.saturating_sub(0x04 * t / 255);
                let g = 0x1Au64.saturating_sub(0x08 * t / 255);
                let b = 0x12u64.saturating_sub(0x05 * t / 255);
                (r, g, b)
            }
            3 => { // Warm amber dusk
                let r = 0x2Eu64.saturating_sub(0x0A * t / 255);
                let g = 0x24u64.saturating_sub(0x0A * t / 255);
                let b = 0x18u64.saturating_sub(0x08 * t / 255);
                (r, g, b)
            }
            _ => { // v0: default warm stone green
                let mut r = 0x25u64.saturating_sub(0x0A * t / 255);
                let mut g = 0x2Du64.saturating_sub(0x0C * t / 255);
                let b = 0x29u64.saturating_sub(0x0B * t / 255);
                if t < 140 { let glow = (140 - t) * 10 / 140; g += glow; r += glow / 3; }
                (r, g, b)
            }
        };
        fb.fill_rect(0, y, w, 1, ((r as u32) << 16) | ((g as u32) << 8) | b as u32);
        y += 1;
    }

    // Warm atmospheric glows + a faint dingir constellation — the depth the
    // mockup gets from radial-gradients and blurred color pools. Only on the
    // default variant (the others stay clean tinted gradients).
    if variant == 0 {
        let wi = w as i32; let hi = h as i32;
        fb.glow(wi * 76 / 100, hi * 22 / 100, (w as i32) * 30 / 100, 0x5F9476, 70); // sage, top-right
        fb.glow(wi * 16 / 100, hi * 84 / 100, (w as i32) * 26 / 100, 0xB47850, 46); // warm amber, bottom-left
        fb.glow(wi / 2,        hi / 2,        (w as i32) * 20 / 100, 0x506E5F, 40); // center sage
        // faint cream dingir stars in the dimmer regions (away from the glows),
        // a hair lighter than the wall so they read as soft highlights, not dirt.
        let stars: [(i32, i32, i32); 4] = [
            (wi * 11 / 100, hi * 30 / 100, 11),
            (wi * 90 / 100, hi * 82 / 100, 14),
            (wi * 33 / 100, hi * 70 / 100,  9),
            (wi * 60 / 100, hi * 12 / 100,  8),
        ];
        for (sx, sy, sr) in stars { fb.draw_star8(sx, sy, sr, 0x3B4239); }
    }

    // Centered hero — floating text on the wallpaper, NO card box (matches the
    // mockup #hero: transparent, pointer-events:none). Big dingir, then
    // "Rusty Penguin" with Penguin in green, then two tagline lines.
    let cx = w as i32 / 2;
    let hero_cy = ptop.max(0) / 2;
    fb.draw_star8(cx, hero_cy - 56, 26, ACCENT_CREAM);
    // Title: 3× glyphs (24px). "Rusty " white + "Penguin" green, kerned together.
    let title_w = (6 + 7) as i32 * 8 * 3; // "Rusty " + "Penguin" at scale 3
    let tx = (cx - title_w / 2) as u32;
    let ty = (hero_cy - 18) as u32;
    fb.draw_str_scaled_t(tx, ty, "Rusty ", WHITE, 3);
    fb.draw_str_scaled_t(tx + 6 * 8 * 3, ty, "Penguin", GREEN, 3);
    let tag1 = "Bare-metal Rust OS . Sparse ternary inference . Zero binary";
    let tag2 = "RFI-IRFOS . Ternary Intelligence Stack";
    fb.draw_str_t((cx - tag1.len() as i32 * 4) as u32, (hero_cy + 30) as u32, tag1, DIM);
    fb.draw_str_t((cx - tag2.len() as i32 * 4) as u32, (hero_cy + 48) as u32, tag2, TRIT_ZERO);

    // ── Bottom panel — frosted-glass floating dock.
    // Drawn as translucent glass over the wallpaper (the warm glows show
    // through) plus a crisp hairline border + top sheen so it reads as a single
    // illuminated physical object hovering above the desktop, not a flush bar.
    let px = PANEL_MARGIN; let pw = w as i32 - 2 * PANEL_MARGIN;
    // soft drop shadow beneath the dock
    fb.fill_rounded_rect(px + 3, ptop + 6, pw, PANEL_H, PANEL_R + 1, 0x0C100E);
    fb.fill_rounded_rect(px + 1, ptop + 3, pw, PANEL_H, PANEL_R,     0x0F140F);
    // opaque body (uniform with the per-frame tray/tasks repaints) — kept light
    // and warm so it stands clearly above the wall.
    fb.fill_rounded_rect(px, ptop, pw, PANEL_H, PANEL_R, PANEL_SOLID);
    // hairline border + top light-catch
    draw_round_border(fb, px, ptop, pw, PANEL_H, PANEL_R, PANEL_EDGE);
    fb.fill_rect_s(px + PANEL_R, ptop + 1, pw - 2 * PANEL_R, 1, 0x66726A);

    // Menu button (dingir + "Menu")
    let (mbx, mby, mbw, _mbh) = menu_btn_rect(h);
    fb.fill_rounded_rect(mbx, mby, mbw, 40, 10, 0x323C37);
    fb.fill_rect_s(mbx + 8, mby, mbw - 16, 2, GREEN);  // green top accent
    fb.draw_star8(mbx + 15, mby + 20, 9, ACCENT_CREAM);
    fb.draw_str(mbx as u32 + 30, mby as u32 + 16, "Menu", GREEN, 0x323C37);
    // separator
    fb.fill_rect_s(mbx + mbw + 7, ptop + 14, 1, PANEL_H - 28, PANEL_EDGE);

    // Favourites (horizontal app icons)
    draw_desktop_icons(fb, None);
}

// Crisp 1px rounded border (no fill) — outline for the floating dock/menu.
fn draw_round_border(fb: &mut Framebuffer, x: i32, y: i32, w: i32, h: i32, r: i32, color: u32) {
    fb.fill_rect_s(x + r, y,         w - 2 * r, 1, color);
    fb.fill_rect_s(x + r, y + h - 1, w - 2 * r, 1, color);
    fb.fill_rect_s(x,         y + r, 1, h - 2 * r, color);
    fb.fill_rect_s(x + w - 1, y + r, 1, h - 2 * r, color);
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
    // v2 form: this draws the bottom-panel TRAY (right side) each frame over the
    // solid panel — ternary {-1,0,+1} bus + clock + memory. (Name kept so the
    // existing per-frame call sites don't change.)
    let w = fb.width; let h = fb.height;
    let ptop = panel_top(h);
    let pr = PANEL_MARGIN + (w as i32 - 2 * PANEL_MARGIN); // panel right edge
    let ty = ptop + 7;
    // Clear the tray region with the solid panel color (no cumulative glass).
    let tray_w = 340;
    let trx = (pr - tray_w - 6).max(PANEL_MARGIN + 8);
    fb.fill_rect_s(trx, ty, pr - trx - 6, 40, PANEL_SOLID);

    // Ternary {-1,0,+1} bus — 5 cells cycling neg/zero/pos.
    let phase = ticks / 24;
    let mut cx = trx + 6;
    let cyy = ptop + (PANEL_H - 12) / 2;
    for i in 0..5i64 {
        let col = match (phase as i64 + i).rem_euclid(3) { 0 => TRIT_NEG, 1 => TRIT_ZERO, _ => TRIT_POS };
        fb.fill_rounded_rect(cx, cyy, 11, 12, 3, col);
        cx += 15;
    }

    // Clock — right-aligned wall-clock string.
    let clk_y = (ty + 14) as u32;
    let clk_x = (pr - 10 - time.len() as i32 * 8).max(trx + 90) as u32;
    fb.draw_str(clk_x, clk_y, time, WHITE, PANEL_SOLID);

    // Memory % to the left of the clock.
    let mem_col = if s.mem_pct > 80 { TRIT_NEG } else if s.mem_pct > 60 { AMBER } else { GREEN };
    let mut mp = Strbuf::new(); mp.push_u64(s.mem_pct as u64); mp.push(b'%');
    let mp_s = mp.as_str();
    let lbl_x = (clk_x as i32 - (4 + mp_s.len() as i32) * 8 - 16).max(trx as i32 + 90) as u32;
    fb.draw_str(lbl_x, clk_y, "MEM", DIM, PANEL_SOLID);
    fb.draw_str(lbl_x + 32, clk_y, mp_s, mem_col, PANEL_SOLID);
    let _ = ticks;
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
    DesktopIcon { label: "Term",  bitmap: &ICON_TERM,  color: GREEN,    launcher_idx: 0 },
    DesktopIcon { label: "Files", bitmap: &ICON_FILES, color: BLUE,     launcher_idx: 1 },
    DesktopIcon { label: "Edit",  bitmap: &ICON_AI,    color: AMBER,    launcher_idx: 2 },
    DesktopIcon { label: "Calc",  bitmap: &ICON_PROC,  color: 0xFFD700, launcher_idx: 7 },
    DesktopIcon { label: "Help",  bitmap: &ICON_FILES, color: 0x90EE90, launcher_idx: 9 },
    DesktopIcon { label: "Prefs", bitmap: &ICON_PROC,  color: 0x9CA3AF, launcher_idx: 5 },
    DesktopIcon { label: "TIS",   bitmap: &ICON_TRIT,  color: 0x4A9EFF, launcher_idx: 6 },
    DesktopIcon { label: "Snake", bitmap: &ICON_SNAKE, color: 0x4ADE80, launcher_idx: 10 },
    DesktopIcon { label: "Mines", bitmap: &ICON_MINE,  color: 0xFCD34D, launcher_idx: 11 },
    DesktopIcon { label: "Doom",  bitmap: &ICON_DOOM,  color: 0xEF4444, launcher_idx: 12 },
    DesktopIcon { label: "Web",   bitmap: &ICON_WEB,   color: 0x8CC6E5, launcher_idx: 13 }, // index 10
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
        // accent-tinted tile background (~16% accent over the glass)
        let tile = tint(icon.color, if hovered { 64 } else { 34 });
        fb.fill_rounded_rect(x, y, tw, th, 10, tile);
        if hovered { fb.fill_rect_s(x + 10, y + th - 3, tw - 20, 2, icon.color); }
        let bx = (x + (tw - 16) / 2) as u32;
        let by = (y + (th - 16) / 2) as u32;
        fb.draw_bitmap_2x(bx, by, icon.bitmap, icon.color, tile);
    }
}

// tint: blend an accent color toward the dark glass body at the given alpha
// (0..255 = how much accent), producing the mockup's color-mix(accent 16%) look.
fn tint(accent: u32, alpha: u32) -> u32 {
    let base = 0x39443Eu32;
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

fn draw_taskbar_win_btns(fb: &mut Framebuffer, term_wins: &[TermWin]) {
    let fw = fb.width; let fh = fb.height;
    let ptop = panel_top(fh);
    let pr = PANEL_MARGIN + (fw as i32 - 2 * PANEL_MARGIN);
    let tray_left = pr - 346;            // keep clear of the right tray
    let tx0 = tasks_start_x(fh);
    // Clear the whole tasks strip each frame (so closed windows leave no ghost).
    if tray_left > tx0 { fb.fill_rect_s(tx0, ptop + 7, tray_left - tx0, 40, PANEL_SOLID); }

    let n = term_wins.len();
    for (slot, tw) in term_wins.iter().enumerate() {
        let (x, y, w, h) = tbwin_rect(fw, fh, slot);
        if x + w > tray_left { break; }  // out of room → stop
        let is_focused   = slot == n - 1;
        let is_minimized = tw.win.minimized;
        let bg     = if is_minimized { 0x232C28u32 } else { 0x37423Cu32 }; // warm stone tile
        let txt    = if is_minimized { 0xA8B0A6u32 } else { WHITE };
        let accent = if is_focused { GREEN } else { TRIT_ZERO };
        fb.fill_rounded_rect(x, y, w, h, 9, bg);
        // running/focus dot
        fb.fill_circle(x + 11, y + h / 2, 3, accent);
        // window title (truncated)
        let title = tw.win.title.as_str();
        let short = title.find(" - ").map(|i| &title[..i]).unwrap_or(title);
        let max_chars = ((w - 28) / 8).max(0) as usize;
        let lbl = &short[..max_chars.min(short.len())];
        fb.draw_str((x + 22) as u32, (y + 14) as u32, lbl, txt, bg);
        // focus underline
        if is_focused { fb.fill_rect_s(x + 9, y + h - 5, w - 18, 2, GREEN); }
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
    // Whole "Menu" button (bottom panel, left) is clickable.
    let (x, y, w, ht) = menu_btn_rect(fh);
    mx >= x && mx < x + w && my >= y && my < y + ht
}

fn show_desktop_hit(fw: u32, fh: u32, mx: i32, my: i32) -> bool {
    mx >= fw as i32 - 6 && mx < fw as i32
        && my >= fh as i32 - 28 && my < fh as i32
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
    kind:  MenuLaunch,
}

// Real apps only — no raw shell commands.
const MENU_ITEMS: &[MenuItem] = &[
    MenuItem { label: "Web",          desc: "Native browser (local pages)", color: 0x8CC6E5, kind: MenuLaunch::App(9) },
    MenuItem { label: "Files",        desc: "Browse & manage files",      color: 0x8CC6E5, kind: MenuLaunch::App(0) },
    MenuItem { label: "Text Editor",  desc: "Write & edit documents",     color: 0xF5C451, kind: MenuLaunch::App(1) },
    MenuItem { label: "Calculator",   desc: "Ternary arithmetic",         color: 0xFFD700, kind: MenuLaunch::App(2) },
    MenuItem { label: "Terminal",     desc: "psh — bare-metal shell",     color: 0x6FE18B, kind: MenuLaunch::Term(0) },
    MenuItem { label: "Settings",     desc: "System preferences",         color: 0x9CA3AF, kind: MenuLaunch::App(4) },
    MenuItem { label: "TIS Console",  desc: "Sparse ternary AI runtime",  color: 0x4A9EFF, kind: MenuLaunch::App(5) },
    // games start at index 7
    MenuItem { label: "Snake",        desc: "Classic arcade",             color: 0x4ADE80, kind: MenuLaunch::App(6) },
    MenuItem { label: "Minesweeper",  desc: "Find the mines",             color: 0xFCD34D, kind: MenuLaunch::App(7) },
    MenuItem { label: "Doom",         desc: "E1M1 — shareware DOOM",      color: 0xEF4444, kind: MenuLaunch::App(8) },
];
const MENU_APPS_END: usize = 7;  // items 0..7 = apps, 7..10 = games

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
    let bg = if hovered { 0x2A3830u32 } else { 0x1E2620u32 };
    fb.fill_rounded_rect_glass(x + 4, y, w - 8, MENU_ITEM_H, 8, bg, if hovered { 200 } else { 0 });
    // Colored icon square (28×28, radius 6)
    let icon_x = x + 10; let icon_y = y + (MENU_ITEM_H - 28) / 2;
    let ic_bg = {
        let r = ((item.color >> 16) & 0xFF) / 3;
        let g = ((item.color >>  8) & 0xFF) / 3;
        let b = (item.color         & 0xFF) / 3;
        (r << 16) | (g << 8) | b
    };
    fb.fill_rounded_rect(icon_x, icon_y, 28, 28, 6, ic_bg);
    // Icon: first letter of label as a simple 8x8 glyph, centered
    let first = item.label.as_bytes().first().copied().unwrap_or(b' ');
    fb.draw_char((icon_x + 10) as u32, (icon_y + 10) as u32, first as char, item.color, ic_bg);
    // Name (bold appearance: two slightly offset draws)
    let tx = (x + 46) as u32; let ty = (y + 6) as u32;
    fb.draw_str(tx, ty, item.label, WHITE, bg);
    // Description
    fb.draw_str(tx, ty + 10, item.desc, 0x909A92, bg);
}

fn draw_start_menu(fb: &mut Framebuffer) {
    let (x, y, w, h) = start_menu_bounds(fb.height);
    let bg = 0x1E2620u32;

    // Shadow + glass panel
    fb.fill_rounded_rect(x + 3, y + 6, w + 2, h, 14, 0x07090A);
    fb.fill_rounded_rect(x + 1, y + 3, w,     h, 12, 0x0C110E);
    fb.fill_rounded_rect_glass(x, y, w, h, 12, bg, 235);
    fb.fill_rect_s(x + 12, y, w - 24, 2, GREEN);  // green top accent

    // ── Header: dingir avatar circle + "Rusty Penguin" + "OS v2.0.0" ────────
    let av_x = x + 14; let av_y = y + 10;
    fb.fill_circle(av_x + 18, av_y + 18, 18, 0x3A4A3E);
    fb.fill_circle(av_x + 18, av_y + 18, 16, 0x2A3830);
    fb.draw_star8(av_x + 18, av_y + 18, 10, ACCENT_CREAM);
    fb.draw_str((av_x + 40) as u32, (av_y + 5) as u32, "Rusty Penguin", WHITE, bg);
    fb.draw_str((av_x + 40) as u32, (av_y + 17) as u32, "OS v2.0.0  Ternary", 0x6FE18B, bg);

    // Hairline below header
    fb.fill_rect_s(x + 8, y + MENU_HDR_H - 1, w - 16, 1, 0x3C4641);

    // ── Applications section ─────────────────────────────────────────────────
    let apps_y = y + MENU_HDR_H;
    fb.draw_str((x + 14) as u32, (apps_y + 6) as u32, "APPLICATIONS", 0x909A92, bg);
    for i in 0..MENU_APPS_END {
        let iy = apps_y + MENU_SECT_H + i as i32 * MENU_ITEM_H;
        draw_menu_item(fb, x, iy, w, &MENU_ITEMS[i], false);
    }

    // Separator
    let sep_y = apps_y + MENU_SECT_H + MENU_APPS_END as i32 * MENU_ITEM_H + MENU_SEP_H / 2;
    fb.fill_rect_s(x + 12, sep_y, w - 24, 1, 0x3C4641);

    // ── Games section ────────────────────────────────────────────────────────
    let games_y = apps_y + MENU_SECT_H + MENU_APPS_END as i32 * MENU_ITEM_H + MENU_SEP_H;
    fb.draw_str((x + 14) as u32, (games_y + 6) as u32, "GAMES", 0x909A92, bg);
    for i in MENU_APPS_END..MENU_ITEMS.len() {
        let iy = games_y + MENU_SECT_H + (i - MENU_APPS_END) as i32 * MENU_ITEM_H;
        draw_menu_item(fb, x, iy, w, &MENU_ITEMS[i], false);
    }

    // ── Footer: Settings · Shut Down ─────────────────────────────────────────
    let foot_y = y + h - MENU_FOOT_H;
    fb.fill_rect_s(x + 8, foot_y, w - 16, 1, 0x3C4641);
    let btn_bg = 0x252E2Au32;
    // Settings button (left)
    fb.fill_rounded_rect(x + 10, foot_y + 6, (w / 2) - 14, 24, 8, btn_bg);
    fb.draw_str((x + 20) as u32, (foot_y + 13) as u32, "Settings", 0x909A92, btn_bg);
    // Shut Down button (right)
    fb.fill_rounded_rect(x + w / 2 + 4, foot_y + 6, (w / 2) - 14, 24, 8, btn_bg);
    fb.draw_str((x + w / 2 + 10) as u32, (foot_y + 13) as u32, "Shut Down", TRIT_NEG, btn_bg);
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
//   3  Change Background 4  Display Settings  5  Close All Windows

const CTX_ITEMS: &[(&str, u32)] = &[
    ("Open Terminal",      0x6FE18B),  // GREEN
    ("Open Files",         0x8CC6E5),  // BLUE
    ("",                   0),          // separator
    ("Show Desktop",       0x909A92),  // DIM
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
    fb.fill_rounded_rect(x + 3, y + 6, w + 2, h, 12, 0x070A08);
    fb.fill_rounded_rect(x + 1, y + 3, w,     h, 11, 0x0C110E);
    fb.fill_rounded_rect_glass(x, y, w, h, 10, CTX_BG, 230);
    // Top accent edge (spring green hairline)
    fb.fill_rect_s(x + 10, y + 1, w - 20, 1, 0x3C5040);
    // Green top accent strip
    fb.fill_rect_s(x + 10, y, w - 20, 2, GREEN);

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
            let bg = if hovered { 0x2E3C32u32 } else { CTX_BG };
            if hovered {
                fb.fill_rounded_rect_glass(x + 2, cy, w - 4, CTX_ITEM_H, 6, bg, 200);
                // Left accent bar on hover
                fb.fill_rect_s(x + 4, cy + 5, 3, CTX_ITEM_H - 10, *color);
            }
            // Label text
            let ty = (cy + (CTX_ITEM_H - 8) / 2) as u32;
            fb.draw_str((x + 14) as u32, ty, label, *color, bg);
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
                win: wm::Window::new(wx, wy, "System Clock"),
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

fn open_doom(w: i32, h: i32, n: usize) -> Option<TermWin> {
    match term::Terminal::spawn() {
        Ok(t) => {
            let game = alloc::boxed::Box::new(app::Doom::new(sys_ticks()));
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

// ---- Full scene recomposite ─────────────────────────────────────────────────

fn recomposite(fb: &mut Framebuffer, wins: &mut Vec<TermWin>, start_menu: bool, ctx_menu: Option<(i32,i32)>, stats: &SysStats, blink_on: bool, hover_icon: Option<usize>, wallpaper_v: u8) {
    // Static background (gradient + logo + icon dock) is cached: blitting it is
    // far cheaper than recomputing the 1080-row gradient every frame, which is
    // what made dragging a window at 1080p janky. The cache is rebuilt only
    // when the static scene changes (icon hover, wallpaper change).
    if fb.bg_cached() {
        fb.restore_bg();
    } else {
        draw_scene_static_v(fb, wallpaper_v);
        draw_desktop_icons(fb, hover_icon);
        fb.snapshot_bg();
    }
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
    fb.present();  // flush initial paint to screen

    let mut last_topbar_tick: u64 = 0;
    let mut last_blink_tick: u64 = 0;
    let mut last_stat_tick: u64 = 0;
    let mut frames_since_stat: u32 = 0;
    let mut blink_on: bool = true;
    let mut wins: Vec<TermWin> = Vec::new();
    let mut scene_dirty = false;
    let mut start_menu_open = false;
    let mut ctx_menu: Option<(i32, i32)> = None;
    let mut hover_icon: Option<usize> = None;
    let mut wallpaper_variant: u8 = 0;  // cycles on "Change Background"

    // Boot to a clean desktop — the Apple-style welcome card + dock + gradient
    // are the first impression, not a terminal covering them. Open apps from
    // the dock or the Menu.
    scene_dirty = true;

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
        let new_hover = desktop_icon_hit(cx, cy, fb.height);
        if new_hover != hover_icon {
            hover_icon = new_hover;
            fb.invalidate_bg();   // icon dock changes → rebuild the cached background
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
                    match ctx_action(item) {
                        0 => { // Open Terminal
                            if let Some(tw) = open_term(w, h, wins.len(), &LAUNCHERS[0]) {
                                wins.push(tw);
                            }
                        }
                        1 => { // Open Files
                            if let Some(tw) = open_file_manager(w, h, wins.len()) {
                                wins.push(tw);
                            }
                        }
                        2 => { // Show Desktop — minimize all windows
                            for tw in wins.iter_mut() { tw.win.minimized = true; }
                        }
                        3 => { // Change Background — cycle wallpaper variant
                            wallpaper_variant = (wallpaper_variant + 1) % 4;
                            fb.invalidate_bg();
                        }
                        4 => { // Display Settings
                            if let Some(tw) = open_settings(w, h, wins.len()) {
                                wins.push(tw);
                            }
                        }
                        5 => { // Close All Windows
                            wins.clear();
                        }
                        _ => {}
                    }
                }
                scene_dirty = true;
            } else if start_menu_open {
                if let Some(mi) = start_menu_hit(fb.height, cx, cy) {
                    if mi == 99 {
                        // Shut Down: call sys_reboot (native syscall 11)
                        unsafe { core::arch::asm!("syscall", in("rax") 11u64, out("rcx") _, out("r11") _, options(nostack)); }
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
                            MenuLaunch::App(8)   => open_doom(w, h, wins.len()),
                            MenuLaunch::App(9)   => open_browser(w, h, wins.len()),
                            MenuLaunch::App(_)   => None,
                        };
                        if let Some(tw) = opened { wins.push(tw); }
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
                        for tw in wins.iter_mut() { tw.win.minimized = true; }
                        scene_dirty = true;
                    } else if let Some(mi) = tbwin_hit(fb.width, fb.height, &wins, cx, cy) {
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
                            9 => open_doom(w, h, wins.len()),
                            10 => open_browser(w, h, wins.len()),
                            _ => None,
                        };
                        if let Some(tw) = opened {
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
            if let Some(tw) = wins.last_mut() {
                if tw.win.dragging {
                    let oy = tw.win.y; let oh = tw.win.h;
                    let nx2 = (cx - tw.win.drag_ox).max(75).min(w - tw.win.w);
                    let ny2 = (cy - tw.win.drag_oy).max(TOPBAR_H as i32).min(h - tw.win.h - 28);
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
                        scene_dirty = true;
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

        // Pump per-tick updates into animated apps (e.g. Snake). Only the
        // non-minimized windows advance; an app that actually changed state
        // marks its window dirty so the recomposite path below redraws it.
        // Sparse: apps that don't animate return false and cost nothing.
        for tw in wins.iter_mut() {
            if tw.win.minimized { continue; }
            if let Some(app) = &mut tw.app {
                if app.tick(now_ticks) { tw.win_dirty = true; }
            }
        }

        // ── Single unified render pass per frame ──────────────────────────────
        // Render only when something visible actually changed. With multiple
        // windows we still only recompose on real state change — not every frame —
        // so the screen stops flickering.
        let cursor_moved = prev_cx != cx || prev_cy != cy;
        let any_chrome   = scene_dirty || wins.iter().any(|tw| tw.win_dirty);
        let any_term     = wins.iter().any(|tw| tw.term.dirty && !tw.win.minimized);

        if any_chrome || any_term || cursor_moved || topbar_due {
            restore_cursor_bg(&mut fb, prev_cx, prev_cy, &cbuf);

            if any_chrome {
                recomposite(&mut fb, &mut wins, start_menu_open, ctx_menu, &stats, blink_on, hover_icon, wallpaper_variant);
                scene_dirty = false;
            } else if any_term {
                // Partial: re-render only the focused terminal content. App and editor
                // windows mark themselves dirty via win_dirty and go through the full
                // recomposite path above instead.
                let n = wins.len();
                for (i, tw) in wins.iter_mut().enumerate() {
                    if tw.win.minimized || !tw.term.dirty { continue; }
                    if tw.app.is_some() || tw.editor.is_some() { continue; }
                    let focused = i == n - 1;
                    let (ox, oy) = wm::content_origin(&tw.win);
                    let cw = (tw.win.w - 2).max(0) as u32;
                    let ch = (tw.win.h - 3 - wm::TITLEBAR_H).max(0) as u32;
                    tw.term.render(&mut fb, ox as u32, oy as u32, cw, ch, focused && blink_on);
                    tw.term.dirty = false;
                }
                if start_menu_open { draw_start_menu(&mut fb); }
                if let Some((cmx, cmy)) = ctx_menu { draw_ctx_menu(&mut fb, cmx, cmy); }
            }

            if topbar_due && !any_chrome {
                let up = rtc_str();
                draw_topbar(&mut fb, up.as_str(), &stats, now_ticks);
            }

            // Re-stamp the cursor on the topmost window only — and only if
            // it's a plain terminal. Previously we walked the stack looking
            // for the first non-app/non-editor window, which would paint a
            // terminal cursor block onto a window UNDERNEATH an app/editor.
            // That pixel was then visible (bled through) when the topmost
            // window didn't repaint over it.
            if let Some(tw) = wins.last_mut() {
                if !tw.win.minimized && tw.editor.is_none() && tw.app.is_none() {
                    let (ox, oy) = wm::content_origin(&tw.win);
                    let cw = (tw.win.w - 2).max(0) as u32;
                    let ch = (tw.win.h - 3 - wm::TITLEBAR_H).max(0) as u32;
                    tw.term.paint_cursor(&mut fb, ox as u32, oy as u32, cw, ch, blink_on);
                }
            }

            save_cursor_bg(&fb, cx, cy, &mut cbuf);
            draw_cursor(&mut fb, cx, cy);

            // Flush the backbuffer to the real framebuffer. Sparse path: on a
            // pure drag frame (no terminal output, no topbar tick) only the
            // window's damage band changed, so present just those rows — the
            // rest of the screen is dormant. This skips the dominant full-screen
            // MMIO copy and is what makes 1080p dragging smooth. The backbuffer
            // is always fully correct, so a missed source just self-corrects on
            // the next full present.
            match drag_band {
                Some((y0, y1)) if !any_term && !topbar_due =>
                    fb.present_rows(y0.max(0) as u32, y1.max(0) as u32),
                _ => fb.present(),
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
