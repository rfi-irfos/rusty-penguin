#![no_std]
#![no_main]

extern crate alloc;

mod allocator;
mod fb;
mod font;
mod input;
mod term;
mod trit;
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

const BG:       u32 = 0x0B1220;
const TOPBAR:   u32 = 0x080F1C;
const TASKBAR:  u32 = 0x0B141A;
const TOPBAR_H: u32 = 28;
const BORDER:   u32 = 0x1E293B;
const GREEN:    u32 = 0x4ADE80;
const DIM:      u32 = 0x334155;
const DIMMER:   u32 = 0x1E293B;
const WHITE:    u32 = 0xF8FAFC;
const AMBER:    u32 = 0xFBBF24;
const BLUE:     u32 = 0x60A5FA;
const CURSOR:   u32 = 0xF8FAFC;
const TEAL:     u32 = 0x2DD4BF;

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

// Dingir — 8-pointed star, cuneiform divine determinative
#[rustfmt::skip]
const DINGIR: [u8; 8] = [
    0x18, 0x5A, 0x3C, 0xFF, 0x3C, 0x5A, 0x18, 0x00,
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

    // Desktop gradient — three-stop: dark blue-green top → deep navy mid → dark teal bottom.
    // Gives the subtle "lit panel" depth that Ubuntu/Mint wallpapers have.
    let total_rows = tb_y.saturating_sub(TOPBAR_H);
    let mut y = TOPBAR_H;
    while y < tb_y {
        let t = (y - TOPBAR_H) as u64 * 256 / total_rows.max(1) as u64; // 0..255
        let (r, g, b) = if t < 128 {
            // top half: dark blue-green → deep navy
            let s = t;
            (0x0Au64 + s / 32, 0x16u64 + s / 20, 0x22u64 + s / 10)
        } else {
            // bottom half: deep navy → slightly teal
            let s = t - 128;
            (0x0Eu64 - s / 64, 0x1Du64 - s / 32, 0x34u64 - s / 16)
        };
        let col = ((r as u32) << 16) | ((g as u32) << 8) | b as u32;
        fb.fill_rect(0, y, w, 1, col);
        y += 1;
    }

    // Centered logo widget — 220×130px with large 2× text
    const LW: u32 = 220; const LH: u32 = 130;
    let logo_ix = w.saturating_sub(LW) / 2;
    let logo_iy = TOPBAR_H + tb_y.saturating_sub(TOPBAR_H).saturating_sub(LH + 28) / 2;
    let lx = logo_ix as i32; let ly2 = logo_iy as i32;
    // Outer glow/shadow
    fb.fill_rect_s(lx + 5, ly2 + 5, LW as i32, LH as i32, 0x030810);
    // Card background
    fb.fill_rounded_rect(lx, ly2, LW as i32, LH as i32, 8, 0x0C1E2C);
    // Green top accent strip
    fb.fill_rounded_rect(lx, ly2, LW as i32, 4, 2, 0x22C55E);
    // Border
    for off in 0..1i32 {
        fb.fill_rect_s(lx + off, ly2 + off, LW as i32 - off*2, LH as i32 - off*2, 0x1A3040);
        fb.fill_rounded_rect(lx + off + 1, ly2 + off + 1, LW as i32 - off*2 - 2, LH as i32 - off*2 - 2, 7, 0x0C1E2C);
        let _ = off;
    }
    // Dingir 3× icon (24×24) centred near top
    fb.draw_bitmap_3x(logo_ix + (LW - 24) / 2, logo_iy + 14, &DINGIR, GREEN, 0x0C1E2C);
    // "RUSTY" in 2× white (80px wide × 16px tall)
    fb.draw_str_2x(logo_ix + (LW - 5 * 16) / 2, logo_iy + 46, "RUSTY", WHITE, 0x0C1E2C);
    // "PENGUIN" in 2× green (112px wide)
    fb.draw_str_2x(logo_ix + (LW - 7 * 16) / 2, logo_iy + 68, "PENGUIN", GREEN, 0x0C1E2C);
    // Version + tagline
    fb.draw_str(logo_ix + (LW - 9 * 8) / 2, logo_iy + 96, "v1.0.0-bm", DIM, 0x0C1E2C);
    let tag = "Binary hardware.  Ternary mind.";
    let tag_y = logo_iy + LH + 14;
    if tag_y + 8 < tb_y {
        fb.draw_str(w.saturating_sub(tag.len() as u32 * 8) / 2, tag_y, tag, TEAL, 0x0B1728);
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
    fb.fill_rect(0, tb_y, w, 1, 0x1A3028);       // green-tint separator line
    fb.fill_rect(0, tb_y + 1, w, 1, 0x0D161F);   // shadow line below separator
    // Menu button — rounded pill style
    fb.fill_rounded_rect(4, tb_y as i32 + 3, 62, 22, 4, 0x152230);
    fb.fill_rect(4, tb_y + 3, 62, 1, 0x22C55E);  // green top edge
    fb.draw_bitmap_2x(8, tb_y + 6, &DINGIR, GREEN, 0x152230);
    fb.draw_str(30, tb_y + 10, "Menu", WHITE, 0x152230);
    // Separator after menu button
    fb.fill_rect(70, tb_y + 5, 1, 18, 0x1E3030);
    // "Show desktop" strip — far right of taskbar (Mint/GNOME-style)
    fb.fill_rect(w - 6, tb_y, 6, 28, 0x0E1C16);   // dark backing
    fb.fill_rect(w - 5, tb_y, 1, 28, 0x22C55E);   // green accent stripe

    // ── Topbar — gradient top→bottom ──
    for dy in 0..TOPBAR_H {
        let blend = (dy * 10 / TOPBAR_H) as u8; // 0..10
        let col = (0x06u8.saturating_add(blend) as u32) << 16
                | (0x0Cu8.saturating_add(blend) as u32) << 8
                |  0x18u8.saturating_add(blend) as u32;
        fb.fill_rect(0, dy, w, 1, col);
    }
    fb.fill_rect(0, TOPBAR_H - 1, w, 1, 0x1A2F3A);
    fb.fill_rect(0, 0, 3, TOPBAR_H, 0x22C55E); // green left accent
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
    for dy in 0..TOPBAR_H {
        let blend = (dy * 10 / TOPBAR_H) as u8;
        let col = (0x06u8.saturating_add(blend) as u32) << 16
                | (0x0Cu8.saturating_add(blend) as u32) << 8
                |  0x18u8.saturating_add(blend) as u32;
        fb.fill_rect(0, dy, fw, 1, col);
    }
    fb.fill_rect(0, TOPBAR_H - 1, fw, 1, 0x1E293B);
    fb.fill_rect(0, 0, 3, TOPBAR_H, 0x22C55E);  // green left accent
    let ty = (TOPBAR_H / 2).saturating_sub(4);

    // LEFT: brand mark
    fb.draw_bitmap_2x(6, ty.saturating_sub(4), &DINGIR, GREEN, TOPBAR);
    fb.draw_str(24, ty, "Rusty Penguin", GREEN, TOPBAR);

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
    Launcher { label: " psh ", cmd: None,               title: "psh — Terminal",   color: GREEN },
    Launcher { label: " ps  ", cmd: Some("ps\n"),       title: "ps — Processes",   color: BLUE  },
    Launcher { label: " ai  ", cmd: Some("ai 32\n"),    title: "ai — Inference",   color: AMBER },
    Launcher { label: " trit", cmd: Some("trit 42\n"),  title: "trit — Ternary",   color: 0xC084FC },
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
    DesktopIcon { label: "Term",  bitmap: &ICON_TERM, color: GREEN,    launcher_idx: 0 },
    DesktopIcon { label: "Procs", bitmap: &ICON_PROC, color: BLUE,     launcher_idx: 1 },
    DesktopIcon { label: "AI",    bitmap: &ICON_AI,   color: AMBER,    launcher_idx: 2 },
    DesktopIcon { label: "Trit",  bitmap: &ICON_TRIT, color: 0xC084FC, launcher_idx: 3 },
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

fn draw_desktop_icons(fb: &mut Framebuffer) {
    for (i, icon) in DESKTOP_ICONS.iter().enumerate() {
        let (x, y, w, h) = dicon_rect(i);
        let img_h: u32 = h - 12;
        let ix = x as i32; let iy = y as i32;
        let iw = w as i32; let ih = img_h as i32;
        // Half-brightness border color (correct bit math, no alpha tricks)
        let border_col = ((icon.color >> 16 & 0xFF) / 2) << 16
                       | ((icon.color >>  8 & 0xFF) / 2) << 8
                       |  ((icon.color      & 0xFF) / 2);
        // Drop shadow
        fb.fill_rounded_rect(ix + 2, iy + 2, iw, ih, 6, 0x040C14);
        // Colored 1px border ring, then inner fill
        fb.fill_rounded_rect(ix,     iy,     iw,     ih,     6, border_col);
        fb.fill_rounded_rect(ix + 1, iy + 1, iw - 2, ih - 2, 5, 0x0D1E2C);
        // Colored top accent strip (inside the border)
        fb.fill_rect_s(ix + 1, iy + 1, iw - 2, 4, icon.color);
        // 2x bitmap centered
        let bx = x + (w - 16) / 2;
        let by = y + (img_h - 16) / 2;
        fb.draw_bitmap_2x(bx, by, icon.bitmap, icon.color, 0x0D1E2C);
        // Label with background pill
        let lw = icon.label.len() as u32 * 8;
        let lx = if lw < w { x + (w - lw) / 2 } else { x };
        let label_bg = 0x0B1726u32;
        fb.fill_rect(lx.saturating_sub(2), y + img_h + 2, lw + 4, 11, label_bg);
        fb.draw_str(lx, y + img_h + 4, icon.label, icon.color, label_bg);
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
        let bg  = if is_minimized { 0x0D1520u32 } else { 0x152230u32 };
        let txt = if is_minimized { DIM } else { WHITE };
        let accent = if is_focused { BLUE } else { BORDER };
        // Rounded pill
        fb.fill_rounded_rect(x, y, w, h, 4, bg);
        // Top accent strip (focused = blue, unfocused = dim)
        fb.fill_rect_s(x + 4, y, w - 8, 2, accent);
        // App icon dot (small filled circle in accent color)
        fb.fill_circle(x + 6, y + h / 2, 2, accent);
        let max_chars = ((w - 18) / 8).max(0) as usize;
        let lbl = &tw.win.title[..max_chars.min(tw.win.title.len())];
        fb.draw_str((x + 14) as u32, (y + 5) as u32, lbl, txt, bg);
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
    // Drop shadow
    fb.fill_rounded_rect(x + 3, y + 3, w, h, 6, 0x030810);
    // Panel
    fb.fill_rounded_rect(x, y, w, h, 6, 0x0F1E2E);
    // Green accent header bar
    fb.fill_rounded_rect(x, y, w, 18, 4, 0x152E22);
    fb.fill_rect_s(x, y + 14, w, 4, 0x152E22); // square bottom of header
    fb.fill_rect_s(x, y, w, 2, 0x22C55E);       // top green line
    fb.draw_bitmap_2x((x + 3) as u32, (y + 2) as u32, &DINGIR, GREEN, 0x152E22);
    fb.draw_str((x + 23) as u32, (y + 5) as u32, "RUSTY PENGUIN", GREEN, 0x152E22);
    fb.fill_rect_s(x, y + 17, w, 1, 0x253545);  // separator
    for (i, l) in LAUNCHERS.iter().enumerate() {
        let iy = y + 19 + i as i32 * 20;
        let bg = if i % 2 == 0 { 0x0F1E2Eu32 } else { 0x111F30u32 };
        fb.fill_rect_s(x + 1, iy, w - 2, 20, bg);
        // Colored left accent bar per item
        fb.fill_rect_s(x + 2, iy + 3, 3, 14, l.color);
        fb.draw_str((x + 8) as u32, (iy + 6) as u32, l.label, l.color, bg);
        let desc = l.title.split('—').nth(1).unwrap_or("").trim();
        fb.draw_str((x + 52) as u32, (iy + 6) as u32, desc, DIM, bg);
    }
    // Bottom border
    fb.fill_rect_s(x + 1, y + h - 2, w - 2, 1, 0x253545);
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
    fb.fill_rounded_rect(x + 3, y + 3, w, h, 5, 0x030810); // shadow
    fb.fill_rounded_rect(x, y, w, h, 5, 0x0F1E2E);
    fb.fill_rect_s(x, y, w, 1, 0x334155);  // top border
    for (i, (label, color)) in CTX_ITEMS.iter().enumerate() {
        let iy = y + 3 + i as i32 * 20;
        let bg = if i % 2 == 0 { 0x0F1E2Eu32 } else { 0x111F30u32 };
        fb.fill_rect_s(x + 1, iy, w - 2, 20, bg);
        fb.fill_rect_s(x + 2, iy + 4, 3, 12, *color); // color accent
        fb.draw_str((x + 8) as u32, (iy + 6) as u32, label, *color, bg);
    }
    fb.fill_rect_s(x + 1, y + h - 2, w - 2, 1, 0x253545);
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
    win_dirty:   bool,
    initial_cmd: Option<&'static [u8]>,
}

fn open_term(w: i32, h: i32, n: usize, l: &Launcher) -> Option<TermWin> {
    match term::Terminal::spawn() {
        Ok(t) => {
            let off = n as i32 * 20;
            let wx = ((w - wm::WINDOW_W) / 2 + off).max(0).min(w - wm::WINDOW_W);
            let wy = ((h - wm::WINDOW_H - 28) / 2 + off).max(TOPBAR_H as i32).min(h - wm::WINDOW_H - 28);
            Some(TermWin {
                win: wm::Window::new(wx, wy, l.title),
                term: t,
                win_dirty: true,
                initial_cmd: l.cmd.map(|s| s.as_bytes()),
            })
        }
        Err(_) => None,
    }
}

// ---- Full scene recomposite ─────────────────────────────────────────────────

fn recomposite(fb: &mut Framebuffer, wins: &mut Vec<TermWin>, start_menu: bool, ctx_menu: Option<(i32,i32)>, stats: &SysStats) {
    draw_scene_static(fb);
    draw_desktop_icons(fb);
    draw_taskbar_win_btns(fb, wins);
    let up = rtc_str();
    let n = wins.len();
    for (i, tw) in wins.iter_mut().enumerate() {
        if tw.win.minimized { continue; }
        let focused = i == n - 1;
        wm::draw_window(fb, &tw.win, focused);
        let (ox, oy) = wm::content_origin(&tw.win);
        tw.term.render(fb, ox as u32, oy as u32);
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
    let mut mouse = MouseState { x: w / 2, y: h / 2, buttons: 0 };

    let mut stats = sample_stats();
    draw_scene_static(&mut fb);
    draw_desktop_icons(&mut fb);
    let up0 = rtc_str();
    draw_topbar(&mut fb, up0.as_str(), &stats, sys_ticks());
    draw_taskbar_clock(&mut fb, up0.as_str());

    let cbl = (CURSOR_BW * CURSOR_BH) as usize;
    let mut cbuf = vec![BG; cbl];
    let mut cx = mouse.x; let mut cy = mouse.y;
    save_cursor_bg(&fb, cx, cy, &mut cbuf);
    draw_cursor(&mut fb, cx, cy);

    let mut prev_btn: u8 = 0;
    let mut last_topbar_tick: u64 = 0;
    let mut wins: Vec<TermWin> = Vec::new();
    let mut scene_dirty = false;
    let mut start_menu_open = false;
    let mut ctx_menu: Option<(i32, i32)> = None;

    // Auto-open one terminal on boot so the desktop is immediately interactive
    if let Some(tw) = open_term(w, h, 0, &LAUNCHERS[0]) {
        wins.push(tw);
        scene_dirty = true;
    }

    loop {
        sys_yield();

        // Input — drain all events; ESC sequences need all bytes in order
        let keys = input::poll(&mut mouse, w, h);
        let (nx, ny, btn) = (mouse.x, mouse.y, mouse.buttons);

        // Keyboard → global shortcuts first, then focused terminal
        for &k in keys.iter() {
            if k == 0x14 { // Ctrl+T — new terminal
                if let Some(tw) = open_term(w, h, wins.len(), &LAUNCHERS[0]) {
                    wins.push(tw);
                    scene_dirty = true;
                }
            } else if k == 0x17 { // Ctrl+W — close focused terminal
                if !wins.is_empty() {
                    restore_cursor_bg(&mut fb, cx, cy, &cbuf);
                    wins.pop();
                    recomposite(&mut fb, &mut wins, false, ctx_menu, &stats);
                    scene_dirty = false;
                    save_cursor_bg(&fb, cx, cy, &mut cbuf);
                    draw_cursor(&mut fb, cx, cy);
                }
            } else if let Some(tw) = wins.last_mut() {
                tw.term.send_key(k);
                tw.term.dirty = true;
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
        if wins.iter().any(|tw| tw.term.wants_close) {
            restore_cursor_bg(&mut fb, cx, cy, &cbuf);
            wins.retain(|tw| !tw.term.wants_close);
            recomposite(&mut fb, &mut wins, false, ctx_menu, &stats);
            scene_dirty = false;
            save_cursor_bg(&fb, cx, cy, &mut cbuf);
            draw_cursor(&mut fb, cx, cy);
        }

        // Rendering
        let cursor_moved = nx != cx || ny != cy;
        let any_chrome   = scene_dirty || wins.iter().any(|tw| tw.win_dirty);
        let any_content  = wins.iter().any(|tw| tw.term.dirty && !tw.win.minimized);

        if any_chrome || any_content || cursor_moved {
            restore_cursor_bg(&mut fb, cx, cy, &cbuf);

            if any_chrome {
                recomposite(&mut fb, &mut wins, start_menu_open, ctx_menu, &stats);
                scene_dirty = false;
            } else if any_content {
                let n = wins.len();
                for (i, tw) in wins.iter_mut().enumerate() {
                    if !tw.term.dirty || tw.win.minimized { continue; }
                    let (ox, oy) = wm::content_origin(&tw.win);
                    tw.term.render(&mut fb, ox as u32, oy as u32);
                    tw.term.dirty = false;
                    let _ = i; let _ = n;
                }
                if start_menu_open { draw_start_menu(&mut fb); }
                if let Some((cmx, cmy)) = ctx_menu { draw_ctx_menu(&mut fb, cmx, cmy); }
            }

            if cursor_moved { cx = nx; cy = ny; }
            save_cursor_bg(&fb, cx, cy, &mut cbuf);
            draw_cursor(&mut fb, cx, cy);
        }

        // Top bar + taskbar clock: update every ~2s (200 real kernel ticks @ 100Hz)
        let now_ticks = sys_ticks();
        if now_ticks.wrapping_sub(last_topbar_tick) >= 200 {
            last_topbar_tick = now_ticks;
            stats = sample_stats();
            let up = rtc_str();
            restore_cursor_bg(&mut fb, cx, cy, &cbuf);
            draw_topbar(&mut fb, up.as_str(), &stats, now_ticks);
            draw_taskbar_clock(&mut fb, up.as_str());
            save_cursor_bg(&fb, cx, cy, &mut cbuf);
            draw_cursor(&mut fb, cx, cy);
        }

        // Click handling
        let left_down  = (btn & 0x01) != 0;
        let left_edge  = left_down && (prev_btn & 0x01) == 0;
        let right_down = (btn & 0x02) != 0;
        let right_edge = right_down && (prev_btn & 0x02) == 0;

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
            // Context menu takes priority: dismiss on any left click
            if let Some((cmx, cmy)) = ctx_menu.take() {
                if let Some(item) = ctx_menu_item_hit(cx, cy, cmx, cmy, fb.width, fb.height) {
                    match item {
                        0 => { // New Terminal
                            if let Some(tw) = open_term(w, h, wins.len(), &LAUNCHERS[0]) {
                                wins.push(tw);
                            }
                        }
                        1 => { wins.clear(); } // Close All Windows
                        _ => {}                // Refresh Desktop — scene_dirty handles it
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
                restore_cursor_bg(&mut fb, cx, cy, &cbuf);
                draw_start_menu(&mut fb);
                save_cursor_bg(&fb, cx, cy, &mut cbuf);
                draw_cursor(&mut fb, cx, cy);
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
                        restore_cursor_bg(&mut fb, cx, cy, &cbuf);
                        wins.remove(last);
                        recomposite(&mut fb, &mut wins, false, ctx_menu, &stats);
                        scene_dirty = false;
                        save_cursor_bg(&fb, cx, cy, &mut cbuf);
                        draw_cursor(&mut fb, cx, cy);
                    } else if wm::min_btn_hit(&tw.win, cx, cy) {
                        tw.win.minimized = true;
                        scene_dirty = true;
                    } else if wm::max_btn_hit(&tw.win, cx, cy) {
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
                        let li = DESKTOP_ICONS[di].launcher_idx;
                        if let Some(tw) = open_term(w, h, wins.len(), &LAUNCHERS[li]) {
                            wins.push(tw);
                            scene_dirty = true;
                        }
                    }
                }
            }
        }

        // Drag / resize
        if left_down {
            if let Some(tw) = wins.last_mut() {
                if tw.win.dragging {
                    let nx2 = (cx - tw.win.drag_ox).max(0).min(w - tw.win.w);
                    let ny2 = (cy - tw.win.drag_oy).max(TOPBAR_H as i32).min(h - tw.win.h - 28);
                    if nx2 != tw.win.x || ny2 != tw.win.y {
                        tw.win.x = nx2; tw.win.y = ny2;
                        scene_dirty = true;
                    }
                } else if tw.win.resizing {
                    let nw = (tw.win.resize_ow + cx - tw.win.resize_mx).max(wm::WIN_MIN_W);
                    let nh = (tw.win.resize_oh + cy - tw.win.resize_my).max(wm::WIN_MIN_H);
                    // Also clamp so window doesn't exceed screen bounds
                    let nw = nw.min(w - tw.win.x);
                    let nh = nh.min(h - tw.win.y - 28);
                    if nw != tw.win.w || nh != tw.win.h {
                        tw.win.w = nw; tw.win.h = nh;
                        scene_dirty = true;
                    }
                }
            }
        } else {
            for tw in wins.iter_mut() {
                tw.win.dragging = false;
                tw.win.resizing = false;
            }
        }

        prev_btn = btn;
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    sys_serial_debug(b'!'); // '!' = panic in serial log
    loop {}
}
