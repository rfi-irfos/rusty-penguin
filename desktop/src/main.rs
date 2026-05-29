mod fb;
mod font;
mod input;
mod keyboard;
mod term;
mod wm;

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use fb::Framebuffer;
use input::MouseState;

fn slog(msg: &str) {
    use std::io::Write;
    if let Ok(mut s) = std::fs::OpenOptions::new().write(true).open("/dev/ttyS0") {
        let _ = writeln!(s, "[desktop] {}", msg);
    }
}

fn unbind_fbcon() {
    for v in &["vtcon0", "vtcon1"] {
        std::fs::write(format!("/sys/class/vtconsole/{}/bind", v), "0\n").ok();
    }
}

// ---- Palette — warm-stone-green (v2 design) ----
const BG:           u32 = 0x1B211E;
const GREEN:        u32 = 0x6FE18B;
const DIM:          u32 = 0xA8B0A6;
const WHITE:        u32 = 0xECEDE5;
const AMBER:        u32 = 0xF5C451;
const BLUE:         u32 = 0x8CC6E5;
const ACCENT_CREAM: u32 = 0xECDAA7;
const TRIT_NEG:     u32 = 0xEF7575;
const TRIT_ZERO:    u32 = 0x909A92;
const TRIT_POS:     u32 = 0x6FE18B;
const PANEL_SOLID:  u32 = 0x2A332F;
const PANEL_EDGE:   u32 = 0x3C4641;
const CURSOR_CLR:   u32 = 0xF5F5F7;

// Bottom panel geometry
const PANEL_MARGIN: i32 = 14;
const PANEL_BOTTOM: i32 = 12;
const PANEL_H:      i32 = 54;
const PANEL_R:      i32 = 14;
const MENU_BTN_W:   i32 = 72;
const FAV_TILE:     i32 = 40;
const FAV_GAP:      i32 = 8;

fn panel_top(h: i32) -> i32 { h - PANEL_BOTTOM - PANEL_H }
fn fav_x(i: usize) -> i32 { PANEL_MARGIN + 8 + MENU_BTN_W + 16 + i as i32 * (FAV_TILE + FAV_GAP) }

const CURSOR_W: u32 = 12;
const CURSOR_H: u32 = 20;

// Dingir 𒀭 — 8-pointed star, cuneiform divine determinative
#[rustfmt::skip]
const DINGIR: [u8; 8] = [
    0x18, // ...##...  top arm
    0x5A, // .#.##.#.  diagonal junction
    0x3C, // ..####..  inner ring
    0xFF, // ########  horizontal
    0x3C, // ..####..
    0x5A, // .#.##.#.
    0x18, // ...##...  bottom arm
    0x00,
];

#[rustfmt::skip]
const CURSOR_SHAPE: [[bool; 12]; 20] = [
    [true,  false, false, false, false, false, false, false, false, false, false, false],
    [true,  true,  false, false, false, false, false, false, false, false, false, false],
    [true,  true,  true,  false, false, false, false, false, false, false, false, false],
    [true,  true,  true,  true,  false, false, false, false, false, false, false, false],
    [true,  true,  true,  true,  true,  false, false, false, false, false, false, false],
    [true,  true,  true,  true,  true,  true,  false, false, false, false, false, false],
    [true,  true,  true,  true,  true,  true,  true,  false, false, false, false, false],
    [true,  true,  true,  true,  true,  true,  true,  true,  false, false, false, false],
    [true,  true,  true,  true,  true,  true,  true,  true,  true,  false, false, false],
    [true,  true,  true,  true,  true,  true,  true,  true,  true,  true,  false, false],
    [true,  true,  true,  true,  true,  true,  false, false, false, false, false, false],
    [true,  true,  true,  false, true,  true,  false, false, false, false, false, false],
    [true,  true,  false, false, true,  false, false, false, false, false, false, false],
    [true,  false, false, false, false, false, false, false, false, false, false, false],
    [true,  false, false, false, false, false, false, false, false, false, false, false],
    [true,  false, false, false, false, false, false, false, false, false, false, false],
    [true,  false, false, false, false, false, false, false, false, false, false, false],
    [true,  false, false, false, false, false, false, false, false, false, false, false],
    [false, false, false, false, false, false, false, false, false, false, false, false],
    [false, false, false, false, false, false, false, false, false, false, false, false],
];

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

fn draw_cursor_fn(fb: &mut Framebuffer, x: i32, y: i32) {
    for row in 0..CURSOR_H as i32 {
        for col in 0..CURSOR_W as i32 {
            if CURSOR_SHAPE[row as usize][col as usize] {
                let px = x + col; let py = y + row;
                if px >= 0 && py >= 0 && (px as u32) < fb.width && (py as u32) < fb.height {
                    fb.set_pixel(px as u32, py as u32, CURSOR_CLR);
                }
            }
        }
    }
}

fn draw_cursor(fb: &mut Framebuffer, x: i32, y: i32) {
    draw_cursor_fn(fb, x, y);
}

// ---- Dock icon definitions ----
// label, terminal command (None = browser launch), title, accent color
struct Launcher { label: &'static str, cmd: Option<&'static str>, title: &'static str, color: u32 }
const LAUNCHERS: &[Launcher] = &[
    Launcher { label: "Term",    cmd: Some(""),          title: "Terminal",        color: 0x6FE18B },
    Launcher { label: "Files",   cmd: Some("ls -la\n"),  title: "Files",           color: 0x8CC6E5 },
    Launcher { label: "Edit",    cmd: Some("nano\n"),    title: "Text Editor",     color: 0xF5C451 },
    Launcher { label: "Proc",    cmd: Some("ps aux\n"),  title: "Processes",       color: 0x909A92 },
    Launcher { label: "AI",      cmd: Some("ai 32\n"),   title: "TIS Runtime",     color: 0xECDAA7 },
    Launcher { label: "Web",     cmd: None,              title: "Browser",         color: 0x60C0FF },
];
const N_ICONS: usize = 6;

// ---- Static desktop — warm-stone gradient + hero card + bottom dock ----
fn draw_scene_static(fb: &mut Framebuffer) {
    let w = fb.width as i32; let h = fb.height as i32;
    let ptop = panel_top(h);

    // Wallpaper gradient #252D29 → #1B211E + soft green glow at top
    for y in 0..h {
        let t = y as u64 * 256 / h as u64;
        let mut r = 0x25u64.saturating_sub(0x0Au64 * t / 255);
        let mut g = 0x2Du64.saturating_sub(0x0Cu64 * t / 255);
        let b = 0x29u64.saturating_sub(0x0Bu64 * t / 255);
        if t < 140 { let glow = (140 - t) * 10 / 140; g += glow; r += glow / 3; }
        fb.fill_rect_s(0, y, w, 1, ((r as u32) << 16) | ((g as u32) << 8) | b as u32);
    }

    // Hero card (frosted glass)
    let lw = 260i32; let lh = 152i32;
    let lx = (w - lw) / 2;
    let ly = (ptop - lh) / 2;
    fb.fill_rounded_rect(lx - 1, ly + 7, lw + 2, lh, 17, 0x070A08);
    fb.fill_rounded_rect(lx,     ly + 3, lw,     lh, 15, 0x0C110E);
    fb.fill_rounded_rect_glass(lx, ly, lw, lh, 14, 0x222B27, 210);
    fb.fill_rect_s(lx + 14, ly + 1, lw - 28, 1, 0x4C564F);
    fb.draw_star8(lx + lw / 2, ly + 28, 14, ACCENT_CREAM);
    fb.draw_str_2x((lx as u32) + (lw as u32 - 5 * 16) / 2, (ly + 50) as u32, "RUSTY", WHITE, 0x222B27);
    fb.draw_str_2x((lx as u32) + (lw as u32 - 7 * 16) / 2, (ly + 72) as u32, "PENGUIN", GREEN, 0x222B27);
    fb.draw_str((lx as u32) + (lw as u32 - 10 * 8) / 2, (ly + 102) as u32, "OS v1.0.0", DIM, 0x222B27);
    let tag = "Bare-metal Rust · Ternary mind · Linux kernel";
    let tag_y = ly + lh + 14;
    if tag_y + 8 < ptop {
        let tx = (w - tag.len() as i32 * 8) / 2;
        fb.draw_str(tx.max(0) as u32, tag_y as u32, tag, DIM, BG);
    }

    // Bottom dock — frosted glass panel
    let px = PANEL_MARGIN; let pw = w - 2 * PANEL_MARGIN;
    fb.fill_rounded_rect(px - 1, ptop + 3, pw + 2, PANEL_H, PANEL_R + 2, 0x0C110E);
    fb.fill_rounded_rect_glass(px, ptop, pw, PANEL_H, PANEL_R, PANEL_SOLID, 210);
    fb.fill_rect_s(px + PANEL_R, ptop + 1, pw - 2 * PANEL_R, 1, PANEL_EDGE);

    // Menu button (dingir + "Menu")
    let mbx = PANEL_MARGIN + 8; let mby = ptop + 7;
    fb.fill_rounded_rect(mbx, mby, MENU_BTN_W, 40, 10, 0x323C37);
    fb.fill_rect_s(mbx, mby, MENU_BTN_W, 2, GREEN);
    fb.draw_star8(mbx + 15, mby + 20, 9, GREEN);
    fb.draw_str((mbx + 30) as u32, (mby + 16) as u32, "Menu", WHITE, 0x323C37);
    // separator
    fb.fill_rect_s(mbx + MENU_BTN_W + 7, ptop + 12, 1, PANEL_H - 24, PANEL_EDGE);

    // Dock icons
    for i in 0..N_ICONS {
        let ix = fav_x(i); let iy = ptop + 7;
        let l = &LAUNCHERS[i];
        fb.fill_rounded_rect(ix, iy, FAV_TILE, FAV_TILE, 8, 0x2E3A34);
        fb.fill_rect_s(ix, iy, FAV_TILE, 2, l.color);
        let lx2 = ix as u32 + (FAV_TILE as u32 - l.label.len() as u32 * 8) / 2;
        fb.draw_str(lx2, (iy + 16) as u32, l.label, l.color, 0x2E3A34);
    }
}

// ---- Taskbar: task buttons inside the dock tasks area ----
fn tasks_start_x() -> i32 {
    fav_x(N_ICONS) + FAV_GAP
}

fn tbwin_rect(fh: i32, slot: usize) -> (i32, i32, i32, i32) {
    (tasks_start_x() + slot as i32 * 116, panel_top(fh) + 7, 108, 40)
}

fn draw_taskbar_win_btns(fb: &mut Framebuffer, term_wins: &[TermWin]) {
    let fh = fb.height as i32;
    let ptop = panel_top(fh);
    let pr = PANEL_MARGIN + fb.width as i32 - 2 * PANEL_MARGIN;
    let n = term_wins.len();
    for (slot, tw) in term_wins.iter().enumerate() {
        let (x, y, w, h) = tbwin_rect(fh, slot);
        if x + w >= pr - 120 { break; }
        let is_focused = slot == n - 1;
        let is_min = tw.win.minimized;
        let bg = if is_focused { 0x3A4A3Eu32 } else { 0x2A352Fu32 };
        let col = if is_min { DIM } else if is_focused { GREEN } else { WHITE };
        fb.fill_rounded_rect_glass(x, y, w, h, 6, bg, 200);
        if is_focused { fb.fill_rect_s(x + 4, y + h - 3, w - 8, 2, GREEN); }
        let dot = if is_min { "  " } else { "• " };
        let lbl: String = dot.chars().chain(tw.win.title.chars()).take(((w - 8) / 8) as usize).collect();
        fb.draw_str((x + 4) as u32, (y + (h - 8) / 2) as u32, &lbl, col, bg);
    }
}

fn tbwin_hit(fw: u32, fh: u32, wins: &[TermWin], mx: i32, my: i32) -> Option<usize> {
    for (slot, _) in wins.iter().enumerate() {
        let (x, y, w, h) = tbwin_rect(fh as i32, slot);
        if mx >= x && mx < x + w && my >= y && my < y + h { return Some(slot); }
    }
    None
}

// Dock icon hit test
fn dock_icon_hit(h: i32, mx: i32, my: i32) -> Option<usize> {
    let ptop = panel_top(h);
    for i in 0..N_ICONS {
        let ix = fav_x(i); let iy = ptop + 7;
        if mx >= ix && mx < ix + FAV_TILE && my >= iy && my < iy + FAV_TILE {
            return Some(i);
        }
    }
    None
}

fn menu_btn_hit(h: i32, mx: i32, my: i32) -> bool {
    let ptop = panel_top(h);
    let mbx = PANEL_MARGIN + 8; let mby = ptop + 7;
    mx >= mbx && mx < mbx + MENU_BTN_W && my >= mby && my < mby + 40
}

// Launch a browser.
// On installed systems (firefox/chromium in PATH): spawn and return immediately.
// On live ISO with initrd-web.img (X11 available): start an X11 session with
// the browser. This BLOCKS until the user closes the browser, then the RP
// desktop redraws. The initrd-web.img bundles Xorg + Firefox + Chrome so
// /usr/bin/firefox and /usr/bin/startx are available in the live environment.
fn launch_browser() -> bool {
    // Fast path: browser already in PATH (installed system)
    for browser in &["firefox", "chromium-browser", "chromium", "google-chrome"] {
        if std::process::Command::new(browser).spawn().is_ok() { return false; }
    }
    // X11 session path (live ISO with initrd-web.img): blocking — desktop
    // resumes automatically when the user closes the browser.
    let browsers = &[
        "/usr/bin/firefox",
        "/usr/bin/chromium-browser",
        "/usr/bin/chromium",
    ];
    for xb in browsers {
        if std::path::Path::new(xb).exists() {
            // startx <browser> — Xorg starts, browser runs, X exits when browser closes
            let _ = std::process::Command::new("startx").arg(xb).status();
            return true;  // true = full-screen session ended, redraw needed
        }
    }
    // Last resort: terminal with install instructions
    let _ = std::process::Command::new("/bin/psh")
        .arg("-c")
        .arg("echo 'No browser found. Run: apt install firefox'; exec /bin/psh")
        .spawn();
    false
}


fn read_rtc() -> String {
    if let Ok(s) = std::fs::read_to_string("/proc/driver/rtc") {
        for l in s.lines() { if l.starts_with("rtc_time") {
            if let Some(v) = l.splitn(2, ':').nth(1) { return v.trim().to_string(); }
        }}
    }
    "--:--:--".to_string()
}

// ---- System stats (read from /proc) ----
struct SysStats { cpu_pct: u8, mem_pct: u8, swap_pct: u8, net_rx_kb: u32, net_tx_kb: u32 }
struct CpuSample { idle: u64, total: u64 }
struct NetSample { rx: u64, tx: u64 }

fn read_cpu_sample() -> CpuSample {
    if let Ok(s) = std::fs::read_to_string("/proc/stat") {
        if let Some(l) = s.lines().next() {
            let nums: Vec<u64> = l.split_whitespace().skip(1)
                .filter_map(|x| x.parse().ok()).collect();
            if nums.len() >= 4 {
                let idle  = nums[3] + nums.get(4).copied().unwrap_or(0);
                let total = nums.iter().copied().sum();
                return CpuSample { idle, total };
            }
        }
    }
    CpuSample { idle: 0, total: 1 }
}

fn read_net_sample() -> NetSample {
    let mut rx = 0u64; let mut tx = 0u64;
    if let Ok(s) = std::fs::read_to_string("/proc/net/dev") {
        for l in s.lines().skip(2) {
            let parts: Vec<&str> = l.split_whitespace().collect();
            if parts.len() >= 10 {
                let iface = parts[0].trim_end_matches(':');
                if iface == "lo" { continue; }
                rx += parts[1].parse::<u64>().unwrap_or(0);
                tx += parts[9].parse::<u64>().unwrap_or(0);
            }
        }
    }
    NetSample { rx, tx }
}

fn read_mem_stats() -> (u8, u8) {
    let mut total = 1u64; let mut avail = 0u64;
    let mut swap_total = 0u64; let mut swap_free = 0u64;
    if let Ok(s) = std::fs::read_to_string("/proc/meminfo") {
        for l in s.lines() {
            let mut it = l.split_whitespace();
            match it.next() {
                Some("MemTotal:")     => { total      = it.next().and_then(|x| x.parse().ok()).unwrap_or(1); }
                Some("MemAvailable:") => { avail      = it.next().and_then(|x| x.parse().ok()).unwrap_or(0); }
                Some("SwapTotal:")    => { swap_total = it.next().and_then(|x| x.parse().ok()).unwrap_or(0); }
                Some("SwapFree:")     => { swap_free  = it.next().and_then(|x| x.parse().ok()).unwrap_or(0); }
                _ => {}
            }
        }
    }
    let mem_pct  = (100u64.saturating_sub(avail * 100 / total.max(1))) as u8;
    let swap_pct = if swap_total > 0 { ((swap_total - swap_free) * 100 / swap_total) as u8 } else { 0 };
    (mem_pct.min(100), swap_pct.min(100))
}

struct StatSampler {
    cpu_prev:  CpuSample,
    net_prev:  NetSample,
    stats:     SysStats,
}

impl StatSampler {
    fn new() -> Self {
        StatSampler {
            cpu_prev: read_cpu_sample(),
            net_prev: read_net_sample(),
            stats: SysStats { cpu_pct: 0, mem_pct: 0, swap_pct: 0, net_rx_kb: 0, net_tx_kb: 0 },
        }
    }

    fn sample(&mut self) {
        let cpu_cur = read_cpu_sample();
        let d_idle  = cpu_cur.idle.saturating_sub(self.cpu_prev.idle);
        let d_total = cpu_cur.total.saturating_sub(self.cpu_prev.total).max(1);
        let cpu_pct = (100u64.saturating_sub(d_idle * 100 / d_total)) as u8;
        self.cpu_prev = cpu_cur;

        let net_cur  = read_net_sample();
        let d_rx     = net_cur.rx.saturating_sub(self.net_prev.rx);
        let d_tx     = net_cur.tx.saturating_sub(self.net_prev.tx);
        let rx_kb    = (d_rx / 1024) as u32;
        let tx_kb    = (d_tx / 1024) as u32;
        self.net_prev = net_cur;

        let (mem_pct, swap_pct) = read_mem_stats();
        self.stats = SysStats { cpu_pct: cpu_pct.min(100), mem_pct, swap_pct, net_rx_kb: rx_kb, net_tx_kb: tx_kb };
    }
}

// draw_topbar now draws the bottom-panel TRAY (right side) — clock + MEM + ternary bus.
fn draw_topbar(fb: &mut Framebuffer, time: &str, s: &SysStats) {
    let w = fb.width as i32; let h = fb.height as i32;
    let ptop = panel_top(h);
    let pr = PANEL_MARGIN + w - 2 * PANEL_MARGIN; // panel right edge
    let ty = ptop + 7;
    let tray_w = 360;
    let trx = (pr - tray_w - 6).max(PANEL_MARGIN + 8);
    // Clear tray region
    fb.fill_rounded_rect_glass(trx, ty, pr - trx - 6, 40, 6, PANEL_SOLID, 230);

    // Ternary bus — 5 cycling cells
    let phase = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) / 1;
    let mut cx = trx + 6;
    let cyy = ptop + (PANEL_H - 12) / 2;
    for i in 0..5i64 {
        let col = match (phase as i64 + i).rem_euclid(3) { 0 => TRIT_NEG, 1 => TRIT_ZERO, _ => TRIT_POS };
        fb.fill_rounded_rect(cx, cyy, 11, 12, 3, col);
        cx += 15;
    }

    // MEM %
    let mem_col = if s.mem_pct > 80 { TRIT_NEG } else if s.mem_pct > 60 { AMBER } else { GREEN };
    let mem_s = format!("MEM {}%", s.mem_pct);
    let cpu_s = format!("CPU {}%", s.cpu_pct);
    let clk_x = (pr - 10 - time.len() as i32 * 8).max(trx + 120);
    let lbl_x = (clk_x - (mem_s.len() as i32 + cpu_s.len() as i32 + 2) * 8 - 8).max(trx + 90);
    let row_y = (ty + 14) as u32;
    fb.draw_str(lbl_x as u32, row_y, &cpu_s, DIM, PANEL_SOLID);
    fb.draw_str((lbl_x + cpu_s.len() as i32 * 8 + 8) as u32, row_y, &mem_s, mem_col, PANEL_SOLID);
    fb.draw_str(clk_x as u32, row_y, time, WHITE, PANEL_SOLID);
}

// ---- Start menu (Aero glass style) ----
fn start_menu_bounds(fh: i32) -> (i32, i32, i32, i32) {
    let mh = 10 + LAUNCHERS.len() as i32 * 28 + 8;
    let mw = 200i32;
    let ptop = panel_top(fh);
    (PANEL_MARGIN, ptop - mh - 6, mw, mh)
}

fn draw_start_menu(fb: &mut Framebuffer) {
    let (x, y, w, h) = start_menu_bounds(fb.height as i32);
    fb.fill_rounded_rect(x + 2, y + 4, w, h, 10, 0x0A0E0C);
    fb.fill_rounded_rect_glass(x, y, w, h, 10, 0x1E2620, 230);
    fb.fill_rect_s(x + 10, y, w - 20, 2, GREEN);
    fb.draw_star8(x + 14, y + 20, 8, ACCENT_CREAM);
    fb.draw_str((x + 28) as u32, (y + 14) as u32, "RUSTY PENGUIN", GREEN, 0x1E2620);
    fb.fill_rect_s(x + 8, y + 28, w - 16, 1, PANEL_EDGE);
    for (i, l) in LAUNCHERS.iter().enumerate() {
        let iy = y + 34 + i as i32 * 28;
        fb.fill_rect_s(x + 4, iy, w - 8, 26, 0x1E2620);
        fb.fill_rect_s(x + 6, iy + 9, 3, 8, l.color);
        fb.draw_str((x + 14) as u32, (iy + 9) as u32, l.title, l.color, 0x1E2620);
    }
}

fn start_menu_hit(fh: i32, mx: i32, my: i32) -> Option<usize> {
    let (x, y, w, _) = start_menu_bounds(fh);
    if mx < x || mx >= x + w { return None; }
    for i in 0..LAUNCHERS.len() {
        let iy = y + 34 + i as i32 * 28;
        if my >= iy && my < iy + 28 { return Some(i); }
    }
    None
}

fn start_menu_bounds_hit(fh: i32, mx: i32, my: i32) -> bool {
    let (x, y, w, h) = start_menu_bounds(fh);
    mx >= x && mx < x + w && my >= y && my < y + h
}

// ---- Full scene recomposite ----
fn recomposite(fb: &mut Framebuffer, wins: &mut Vec<TermWin>, start_menu: bool, stats: &SysStats) {
    draw_scene_static(fb);
    draw_taskbar_win_btns(fb, wins);
    let n = wins.len();
    for (i, tw) in wins.iter_mut().enumerate() {
        if tw.win.minimized { continue; }
        wm::draw_window(fb, &tw.win, i == n - 1);
        let (ox, oy) = wm::content_origin(&tw.win);
        tw.term.render(fb, ox as u32, oy as u32);
        tw.term.dirty = false;
        tw.win_dirty = false;
    }
    if start_menu { draw_start_menu(fb); }
    draw_topbar(fb, &read_rtc(), stats);
}

// ---- Window opening ----
struct TermWin { win: wm::Window, term: term::Terminal, win_dirty: bool, initial_cmd: Option<Vec<u8>> }

fn open_term(w: i32, h: i32, n: usize, l: &Launcher) -> Option<TermWin> {
    match term::Terminal::spawn() {
        Ok(t) => {
            let off = n as i32 * 20;
            let wx = ((w - wm::WINDOW_W) / 2 + off).max(0).min(w - wm::WINDOW_W);
            let wy = ((h - wm::WINDOW_H - 66) / 2 + off).max(4).min(h - wm::WINDOW_H - 66);
            slog(&format!("terminal '{}' opened at {}x{}", l.title, wx, wy));
            Some(TermWin {
                win: wm::Window::new(wx, wy, l.title),
                term: t, win_dirty: true,
                initial_cmd: l.cmd.map(|s| if s.is_empty() { vec![] } else { s.as_bytes().to_vec() }),
            })
        }
        Err(e) => { slog(&format!("spawn failed: {}", e)); None }
    }
}

fn exec_psh() -> ! {
    use std::os::unix::process::CommandExt;
    // RP_RECOVERY tells the shell NOT to bounce back into the desktop — without
    // it, desktop→psh→desktop loops forever when there is no /dev/fb0.
    let _ = std::process::Command::new("/bin/psh").env("RP_RECOVERY", "1").exec();
    let _ = std::process::Command::new("/usr/local/bin/psh").env("RP_RECOVERY", "1").exec();
    loop { thread::sleep(Duration::from_secs(60)); }
}

fn install_panic_hook() {
    // Default Rust panic output goes to stderr, which for an init-spawned
    // process is effectively /dev/null. Redirect panic info to /dev/ttyS0
    // so it shows up in /tmp/rusty-penguin.log alongside slog() output.
    std::panic::set_hook(Box::new(|info| {
        let loc = info.location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_else(|| "unknown".to_string());
        let msg = if let Some(s) = info.payload().downcast_ref::<&'static str>() {
            (*s).to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "(non-string payload)".to_string()
        };
        slog(&format!("PANIC @ {} :: {}", loc, msg));
    }));
}

fn main() {
    install_panic_hook();
    slog("=== desktop starting ===");
    let mut fb = match Framebuffer::open() {
        Ok(f) => f, Err(e) => { slog(&format!("fb unavailable: {}", e)); exec_psh(); }
    };
    slog(&format!("framebuffer: {}x{}x{}", fb.width, fb.height, fb.bpp));
    unbind_fbcon();

    let w = fb.width as i32; let h = fb.height as i32;

    let mouse = Arc::new(Mutex::new(MouseState { x: w/2, y: h/2, buttons: 0 }));
    { let m = Arc::clone(&mouse); thread::spawn(move || input::mouse_thread(m, w, h)); }

    let (kb_tx, kb_rx) = std::sync::mpsc::channel::<Vec<u8>>();
    thread::spawn(move || keyboard::keyboard_thread(kb_tx));

    draw_scene_static(&mut fb);
    let mut sampler = StatSampler::new();
    sampler.sample();
    draw_topbar(&mut fb, &read_rtc(), &sampler.stats);

    let cbl = (CURSOR_W * CURSOR_H) as usize;
    let mut cbuf: Vec<u32> = vec![BG; cbl];
    let mut cx: i32; let mut cy: i32;
    { let s = mouse.lock().unwrap(); cx = s.x; cy = s.y; }
    save_cursor_bg(&fb, cx, cy, &mut cbuf);
    draw_cursor(&mut fb, cx, cy);
    fb.present();  // flush initial paint to /dev/fb0

    let mut prev_btn: u8 = 0;
    let mut tick: u64 = 0;
    let mut wins: Vec<TermWin> = Vec::new();
    let mut scene_dirty = false;
    let mut start_menu_open = false;

    loop {
        thread::sleep(Duration::from_millis(16));

        let (nx, ny, btn) = { let s = mouse.lock().unwrap(); (s.x, s.y, s.buttons) };

        // Keyboard → topmost window
        while let Ok(data) = kb_rx.try_recv() {
            if let Some(tw) = wins.last() { tw.term.write_input(&data); }
        }

        // PTY poll + initial command
        for tw in wins.iter_mut() {
            if let Some(cmd) = tw.initial_cmd.take() { tw.term.write_input(&cmd); }
            if tw.term.poll() { tw.term.dirty = true; }
            if matches!(tw.term.child.try_wait(), Ok(Some(_))) {
                let msg = b"[process exited]";
                for (i, &b) in msg.iter().enumerate() {
                    let idx = tw.term.cur_row * term::COLS + i;
                    if idx < term::COLS * term::ROWS {
                        tw.term.cells[idx] = term::Cell { ch: b, fg: 0xFBBF24, bg: 0x0F172A };
                    }
                }
                tw.term.dirty = true;
            }
        }

        // ---- Rendering (anti-flicker: only update changed regions) ----
        let cursor_moved = nx != cx || ny != cy;
        let any_chrome   = scene_dirty || wins.iter().any(|tw| tw.win_dirty);
        let any_content  = wins.iter().any(|tw| tw.term.dirty && !tw.win.minimized);

        if any_chrome || any_content || cursor_moved {
            restore_cursor_bg(&mut fb, cx, cy, &cbuf);

            if any_chrome {
                // Full scene recomposite — windows moved/opened/closed/maximized
                recomposite(&mut fb, &mut wins, start_menu_open, &sampler.stats);
                scene_dirty = false;
            } else if any_content {
                // Only redraw dirty terminal content areas — no bg, no chrome
                let n = wins.len();
                for (i, tw) in wins.iter_mut().enumerate() {
                    if !tw.term.dirty || tw.win.minimized { continue; }
                    // Re-render just the terminal grid pixels
                    let (ox, oy) = wm::content_origin(&tw.win);
                    tw.term.render(&mut fb, ox as u32, oy as u32);
                    tw.term.dirty = false;
                    // Redraw cursor blink area in titlebar if focused
                    if i == n - 1 { /* cursor rendered inside term.render */ }
                }
                if start_menu_open { draw_start_menu(&mut fb); }
            }

            if cursor_moved { cx = nx; cy = ny; }
            save_cursor_bg(&fb, cx, cy, &mut cbuf);
            draw_cursor(&mut fb, cx, cy);
            fb.present();
        }

        // Top bar: clock + live stats, update every ~2s
        if tick % 120 == 0 {
            sampler.sample();
            restore_cursor_bg(&mut fb, cx, cy, &cbuf);
            draw_topbar(&mut fb, &read_rtc(), &sampler.stats);
            save_cursor_bg(&fb, cx, cy, &mut cbuf);
            draw_cursor(&mut fb, cx, cy);
            fb.present();
        }
        tick = tick.wrapping_add(1);

        // ---- Click handling ----
        let left_down = (btn & 0x01) != 0;
        let left_edge = left_down && (prev_btn & 0x01) == 0;

        if left_edge {
            if start_menu_open {
                if let Some(li) = start_menu_hit(h, cx, cy) {
                    if li < N_ICONS && LAUNCHERS[li].cmd.is_none() {
                        let _ = launch_browser();
                    } else if let Some(tw) = open_term(w, h, wins.len(), &LAUNCHERS[li.min(N_ICONS-1)]) {
                        wins.push(tw);
                    }
                }
                start_menu_open = false;
                scene_dirty = true;
            } else if menu_btn_hit(h, cx, cy) {
                start_menu_open = !start_menu_open;
                scene_dirty = true;
            } else {
                // Dock icon click
                if let Some(di) = dock_icon_hit(h, cx, cy) {
                    if LAUNCHERS[di].cmd.is_none() {
                        // Browser: blocking if X11 session starts; redraw on return.
                        let needs_redraw = launch_browser();
                        if needs_redraw { scene_dirty = true; }
                    } else if let Some(tw) = open_term(w, h, wins.len(), &LAUNCHERS[di]) {
                        wins.push(tw);
                        scene_dirty = true;
                    }
                } else {
                    // Window clicks
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
                            recomposite(&mut fb, &mut wins, false, &sampler.stats);
                            scene_dirty = false;
                            save_cursor_bg(&fb, cx, cy, &mut cbuf);
                            draw_cursor(&mut fb, cx, cy);
                        } else if wm::min_btn_hit(&tw.win, cx, cy) {
                            tw.win.minimized = true;
                            scene_dirty = true;
                        } else if wm::max_btn_hit(&tw.win, cx, cy) {
                            tw.win.toggle_maximize(w, h);
                            scene_dirty = true;
                        } else if wm::titlebar_hit(&tw.win, cx, cy) {
                            tw.win.dragging = true;
                            tw.win.drag_ox  = cx - tw.win.x;
                            tw.win.drag_oy  = cy - tw.win.y;
                        }
                    } else if let Some(mi) = tbwin_hit(fb.width, fb.height, &wins, cx, cy) {
                        wins[mi].win.minimized = false;
                        let tw = wins.remove(mi); wins.push(tw);
                        scene_dirty = true;
                    }
                }
            }
        }

        // Drag
        if left_down {
            if let Some(tw) = wins.last_mut() {
                if tw.win.dragging {
                    let nx2 = (cx - tw.win.drag_ox).max(0).min(w - tw.win.w);
                    let ny2 = (cy - tw.win.drag_oy).max(0).min(h - tw.win.h - 66);
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
