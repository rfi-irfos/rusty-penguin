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
use alloc::string::String;
use alloc::format;

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

const CURSOR_W: u32 = 10;
const CURSOR_H: u32 = 16;

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
    for row in 0..CURSOR_H as i32 {
        for col in 0..CURSOR_W as i32 {
            let px = x + col; let py = y + row;
            let idx = (row * CURSOR_W as i32 + col) as usize;
            buf[idx] = if px >= 0 && py >= 0 && (px as u32) < fb.width && (py as u32) < fb.height {
                fb.get_pixel(px as u32, py as u32)
            } else { BG };
        }
    }
}

fn restore_cursor_bg(fb: &mut Framebuffer, x: i32, y: i32, buf: &[u32]) {
    for row in 0..CURSOR_H as i32 {
        for col in 0..CURSOR_W as i32 {
            let px = x + col; let py = y + row;
            if px >= 0 && py >= 0 && (px as u32) < fb.width && (py as u32) < fb.height {
                fb.set_pixel(px as u32, py as u32, buf[(row * CURSOR_W as i32 + col) as usize]);
            }
        }
    }
}

fn cursor_mask(col: i32, row: i32) -> bool {
    if row < 0 || col < 0 { return false; }
    // Arrow head: right triangle expanding 1px/row (0..=6 → 1..7px wide)
    // Arrow shaft: 2px wide continuing down from left edge
    if row <= 6 { col <= row }
    else        { col <= 1 }
}

fn draw_cursor(fb: &mut Framebuffer, x: i32, y: i32) {
    let outline = 0x000000u32; // black — visible against any background
    let put = |fb: &mut Framebuffer, px: i32, py: i32, c: u32| {
        if px >= 0 && py >= 0 && (px as u32) < fb.width && (py as u32) < fb.height {
            fb.set_pixel(px as u32, py as u32, c);
        }
    };

    // Outline — clamped to [0, CURSOR_W) × [0, CURSOR_H) so all pixels stay
    // inside the save/restore bounding box. Pixels at -1 offsets would never
    // be restored and leave permanent trails.
    for row in 0..CURSOR_H as i32 {
        for col in 0..CURSOR_W as i32 {
            if !cursor_mask(col, row)
                && (cursor_mask(col - 1, row)
                    || cursor_mask(col + 1, row)
                    || cursor_mask(col, row - 1)
                    || cursor_mask(col, row + 1))
            {
                put(fb, x + col, y + row, outline);
            }
        }
    }

    for row in 0..CURSOR_H as i32 {
        for col in 0..CURSOR_W as i32 {
            if cursor_mask(col, row) {
                put(fb, x + col, y + row, CURSOR);
            }
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

const MONTH_NAMES: [&str; 13] = ["",  "Jan", "Feb", "Mar", "Apr", "May", "Jun",
                                  "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const DAY_FULL:    [&str; 8]  = ["",  "Sunday", "Monday", "Tuesday", "Wednesday",
                                  "Thursday", "Friday", "Saturday"];

fn rtc_str() -> String {
    let rtc = sys_rtc();
    let wday  = (rtc        & 0xFF) as u8;
    let sec   = ((rtc >>  8) & 0xFF) as u8;
    let min   = ((rtc >> 16) & 0xFF) as u8;
    let hour  = ((rtc >> 24) & 0xFF) as u8;
    let mday  = ((rtc >> 32) & 0xFF) as u8;
    let month = ((rtc >> 40) & 0xFF) as u8;
    // Sanity check — if RTC returned garbage, fall back to uptime
    if month == 0 || month > 12 || mday == 0 || mday > 31 || hour > 23 || min > 59 || sec > 59 {
        let ticks = sys_ticks();
        let secs = ticks / 100;
        let h = secs / 3600; let m = (secs % 3600) / 60; let s = secs % 60;
        return format!("Up {:02}:{:02}:{:02}", h, m, s);
    }
    let day_s = if (wday as usize) < DAY_FULL.len()   { DAY_FULL[wday as usize]   } else { "???" };
    let mon_s = if (month as usize) < MONTH_NAMES.len() { MONTH_NAMES[month as usize] } else { "???" };
    format!("{} {} {:02}  {:02}:{:02}:{:02}", day_s, mon_s, mday, hour, min, sec)
}

fn uptime_str() -> String {
    let ticks = sys_ticks();
    let secs = ticks / 100;
    let h = secs / 3600; let m = (secs % 3600) / 60; let s = secs % 60;
    format!("{:02}:{:02}:{:02}", h, m, s)
}

// ---- Scene drawing ──────────────────────────────────────────────────────────

fn draw_scene_static(fb: &mut Framebuffer) {
    let w = fb.width; let h = fb.height;
    let tb_y = h - 28;

    // Base fill — single dark tone, then subtle gradient bands
    fb.fill_rect(0, 0, w, h, 0x08131D);
    let total_rows = tb_y.saturating_sub(TOPBAR_H);
    let mut y = TOPBAR_H;
    while y < tb_y {
        let frac = (y - TOPBAR_H) as u64 * 8 / total_rows.max(1) as u64; // 0..8
        let r = (0x0Au64 + frac / 2) as u8;
        let g = (0x18u64 + frac / 3) as u8;
        let b = (0x22u64 + frac / 2) as u8;
        let col = ((r as u32) << 16) | ((g as u32) << 8) | b as u32;
        fb.fill_rect(0, y, w, 1, col);
        y += 1;
    }

    // Subtle dot-grid every 32px — much softer than full grid lines
    let mut gy = TOPBAR_H + 32;
    while gy < tb_y {
        let mut gx = 32u32;
        while gx < w {
            fb.fill_rect(gx, gy, 1, 1, 0x132030);
            gx += 32;
        }
        gy += 32;
    }

    // Centered logo widget — 160×80px, prominent
    const LW: u32 = 160; const LH: u32 = 80;
    let logo_x = w.saturating_sub(LW) / 2;
    let logo_y = TOPBAR_H + tb_y.saturating_sub(TOPBAR_H).saturating_sub(LH + 24) / 2;
    // Outer shadow
    fb.fill_rect(logo_x + 4, logo_y + 4, LW, LH, 0x040A10);
    // Background
    fb.fill_rect(logo_x, logo_y, LW, LH, 0x0C1E2A);
    // Green top accent bar
    fb.fill_rect(logo_x, logo_y, LW, 3, 0x22C55E);
    // Subtle inner gradient fill rows
    let mut ly = logo_y + 3;
    while ly < logo_y + LH - 1 {
        let bright = if ly < logo_y + LH / 2 { 0x0D2030u32 } else { 0x0B1C2Au32 };
        fb.fill_rect(logo_x + 1, ly, LW - 2, 1, bright);
        ly += 1;
    }
    // Side borders
    fb.fill_rect(logo_x, logo_y + 3, 1, LH - 4, 0x1A3040);
    fb.fill_rect(logo_x + LW - 1, logo_y + 3, 1, LH - 4, 0x1A3040);
    fb.fill_rect(logo_x, logo_y + LH - 1, LW, 1, 0x1A3040);
    // Dingir icon 2x — centred
    fb.draw_bitmap_2x(logo_x + (LW - 16) / 2, logo_y + 10, &DINGIR, GREEN, 0x0D2030);
    // Name text
    fb.draw_str(logo_x + (LW - 5 * 8) / 2, logo_y + 38, "RUSTY", WHITE, 0x0D2030);
    fb.draw_str(logo_x + (LW - 7 * 8) / 2, logo_y + 50, "PENGUIN", GREEN, 0x0D2030);
    // Version
    fb.draw_str(logo_x + (LW - 9 * 8) / 2, logo_y + 64, "v1.0.0-bm", DIM, 0x0B1C2A);

    // Tagline below logo
    let tag = "Binary hardware.  Ternary mind.";
    let tag_y = logo_y + LH + 12;
    if tag_y < tb_y.saturating_sub(8) {
        fb.draw_str(w.saturating_sub(tag.len() as u32 * 8) / 2, tag_y, tag, 0x2DD4BF, 0x08131D);
    }

    // ── Taskbar ──
    fb.fill_rect(0, tb_y, w, 28, TASKBAR);
    // Top separator with green tint
    fb.fill_rect(0, tb_y, w, 1, 0x1A3028);
    // Menu button
    fb.draw_bitmap_2x(6, tb_y + 6, &DINGIR, GREEN, TASKBAR);
    fb.draw_str(28, tb_y + 10, "Menu", WHITE, TASKBAR);
    // Vertical separator after menu
    fb.fill_rect(70, tb_y + 4, 1, 20, 0x1E3030);

    // ── Topbar ──
    fb.fill_rect(0, 0, w, TOPBAR_H, TOPBAR);
    // Bottom edge of topbar
    fb.fill_rect(0, TOPBAR_H - 1, w, 1, 0x1A2F3A);
    // Small decorative left accent on topbar
    fb.fill_rect(0, 0, 3, TOPBAR_H, 0x22C55E);
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
    fb.fill_rect(0, 0, fw, TOPBAR_H, TOPBAR);
    fb.fill_rect(0, TOPBAR_H - 1, fw, 1, 0x1E293B);
    fb.fill_rect(0, 0, 3, TOPBAR_H, 0x22C55E);  // green left accent
    let ty = (TOPBAR_H / 2).saturating_sub(4);

    // LEFT: brand mark
    fb.draw_bitmap_2x(6, ty.saturating_sub(4), &DINGIR, GREEN, TOPBAR);
    fb.draw_str(24, ty, "Rusty Penguin", GREEN, TOPBAR);

    // CENTER: uptime clock
    let cx = (fw - time.len() as u32 * 8) / 2;
    fb.draw_str(cx, ty, time, WHITE, TOPBAR);

    // RIGHT: trit indicator + labeled memory
    let mem_str = format!("MEM: {}/{}M", s.used_mib, s.total_mib);
    let ind = trit_indicator(ticks);
    let ind_str = core::str::from_utf8(&ind).unwrap_or("T[+--+]");
    let mut rx = fw as i32 - 8;
    rx -= mem_str.len() as i32 * 8;
    if rx > 120 { fb.draw_str(rx as u32, ty, &mem_str, GREEN, TOPBAR); }
    rx -= 16;
    rx -= ind_str.len() as i32 * 8;
    if rx > 120 { fb.draw_str(rx as u32, ty, ind_str, AMBER, TOPBAR); }
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
        let img_h: u32 = h - 12;  // image area (48px for DICON_H=60: 60-12=48)
        // Icon image background box
        fb.fill_rect(x, y, w, img_h, 0x0D1E2C);
        fb.fill_rect(x, y,         w, 1, icon.color);
        fb.fill_rect(x, y,         1, img_h, icon.color);
        fb.fill_rect(x + w - 1, y, 1, img_h, icon.color);
        fb.fill_rect(x, y + img_h - 1, w, 1, icon.color);
        // 2x bitmap centered in image area (16px rendered at 2x scale)
        let bx = x + (w - 16) / 2;
        let by = y + (img_h - 16) / 2;
        fb.draw_bitmap_2x(bx, by, icon.bitmap, icon.color, 0x0D1E2C);
        // Label centered below image
        let lw = icon.label.len() as u32 * 8;
        let lx = if lw < w { x + (w - lw) / 2 } else { x };
        fb.draw_str(lx, y + img_h + 4, icon.label, icon.color, BG);
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
        let bg  = if is_minimized { 0x111827u32 } else { 0x1E293Bu32 };
        let txt = if is_minimized { DIM } else { WHITE };
        let top = if is_focused   { BLUE } else { BORDER };
        fb.fill_rect(x as u32, y as u32, w as u32, h as u32, bg);
        fb.fill_rect(x as u32, y as u32, w as u32, 1, top);
        let lbl: String = tw.win.title.chars().take(((w - 4) / 8) as usize).collect();
        fb.draw_str((x + 2) as u32, (y + 5) as u32, &lbl, txt, bg);
    }
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

fn start_menu_bounds(fh: u32) -> (i32, i32, i32, i32) {
    let h = 14 + LAUNCHERS.len() as i32 * 20 + 4;
    let w = 160i32;
    (2, fh as i32 - 28 - h, w, h)
}

fn draw_start_menu(fb: &mut Framebuffer) {
    let (x, y, w, h) = start_menu_bounds(fb.height);
    fb.fill_rect(x as u32, y as u32, w as u32, h as u32, 0x1A2535);
    fb.fill_rect(x as u32, y as u32, w as u32, 1, 0x475569);
    fb.fill_rect(x as u32, y as u32, 1, h as u32, 0x475569);
    fb.fill_rect((x + w - 1) as u32, y as u32, 1, h as u32, 0x475569);
    fb.draw_bitmap_2x((x + 2) as u32, (y + 2) as u32, &DINGIR, GREEN, 0x1A2535);
    fb.draw_str((x + 22) as u32, (y + 4) as u32, "RUSTY PENGUIN", GREEN, 0x1A2535);
    fb.fill_rect(x as u32, (y + 14) as u32, w as u32, 1, 0x334155);
    for (i, l) in LAUNCHERS.iter().enumerate() {
        let iy = y + 16 + i as i32 * 20;
        fb.fill_rect(x as u32, iy as u32, w as u32, 20, 0x1A2535);
        fb.draw_str((x + 6) as u32, (iy + 6) as u32, l.label, l.color, 0x1A2535);
        let desc = l.title.split('—').nth(1).unwrap_or("").trim();
        fb.draw_str((x + 48) as u32, (iy + 6) as u32, desc, 0x64748B, 0x1A2535);
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
    fb.fill_rect(x as u32, y as u32, w as u32, h as u32, 0x1A2535);
    fb.fill_rect(x as u32, y as u32, w as u32, 1, 0x475569);
    fb.fill_rect(x as u32, (y + h - 1) as u32, w as u32, 1, 0x475569);
    fb.fill_rect(x as u32, y as u32, 1, h as u32, 0x475569);
    fb.fill_rect((x + w - 1) as u32, y as u32, 1, h as u32, 0x475569);
    for (i, (label, color)) in CTX_ITEMS.iter().enumerate() {
        let iy = y + 2 + i as i32 * 20;
        fb.draw_str((x + 8) as u32, (iy + 6) as u32, label, *color, 0x1A2535);
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
        wm::draw_window(fb, &tw.win, i == n - 1);
        let (ox, oy) = wm::content_origin(&tw.win);
        tw.term.render(fb, ox as u32, oy as u32);
        tw.term.dirty = false;
        tw.win_dirty  = false;
    }
    if start_menu { draw_start_menu(fb); }
    if let Some((cmx, cmy)) = ctx_menu { draw_ctx_menu(fb, cmx, cmy); }
    draw_topbar(fb, &up, stats, sys_ticks());
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
    draw_topbar(&mut fb, &up0, &stats, sys_ticks());

    let cbl = (CURSOR_W * CURSOR_H) as usize;
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
            draw_topbar(&mut fb, &up, &stats, now_ticks);
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
                    } else if wm::titlebar_hit(&tw.win, cx, cy) {
                        tw.win.dragging = true;
                        tw.win.drag_ox  = cx - tw.win.x;
                        tw.win.drag_oy  = cy - tw.win.y;
                    }
                } else {
                    if let Some(mi) = tbwin_hit(fb.width, fb.height, &wins, cx, cy) {
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

        // Drag
        if left_down {
            if let Some(tw) = wins.last_mut() {
                if tw.win.dragging {
                    let nx2 = (cx - tw.win.drag_ox).max(0).min(w - tw.win.w);
                    let ny2 = (cy - tw.win.drag_oy).max(TOPBAR_H as i32).min(h - tw.win.h - 28);
                    if nx2 != tw.win.x || ny2 != tw.win.y {
                        tw.win.x = nx2; tw.win.y = ny2;
                        scene_dirty = true;
                    }
                }
            }
        } else {
            for tw in wins.iter_mut() { tw.win.dragging = false; }
        }

        prev_btn = btn;
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    sys_serial_debug(b'!'); // '!' = panic in serial log
    loop {}
}
