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

// ---- Palette ----
const BG:      u32 = 0x0B1220;
const TOPBAR:  u32 = 0x080F1C;
const TASKBAR: u32 = 0x111827;
const TOPBAR_H: u32 = 16;
const BORDER:  u32 = 0x1E293B;
const GREEN:   u32 = 0x4ADE80;
const DIM:     u32 = 0x334155;
const DIMMER:  u32 = 0x1E293B;
const WHITE:   u32 = 0xF8FAFC;
const AMBER:   u32 = 0xFBBF24;
const BLUE:    u32 = 0x60A5FA;
const CURSOR:  u32 = 0xF8FAFC;

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

fn draw_cursor(fb: &mut Framebuffer, x: i32, y: i32) {
    for row in 0..CURSOR_H as i32 {
        for col in 0..CURSOR_W as i32 {
            if CURSOR_SHAPE[row as usize][col as usize] {
                let px = x + col; let py = y + row;
                if px >= 0 && py >= 0 && (px as u32) < fb.width && (py as u32) < fb.height {
                    fb.set_pixel(px as u32, py as u32, CURSOR);
                }
            }
        }
    }
}

// ---- Static desktop (bg + taskbar) — only called on scene dirty ----
fn draw_scene_static(fb: &mut Framebuffer) {
    let w = fb.width; let h = fb.height;
    let tb_y = h - 28;
    fb.fill_rect(0, 0, w, h, BG);
    // Subtle grid (below topbar, above taskbar)
    let mut gx = 0u32; while gx < w { fb.fill_rect(gx, TOPBAR_H, 1, tb_y.saturating_sub(TOPBAR_H), 0x0D1628); gx += 40; }
    let mut gy = TOPBAR_H; while gy < tb_y { fb.fill_rect(0, gy, w, 1, 0x0D1628); gy += 40; }
    // Tux art (centered between topbar and taskbar)
    let art = [
        "   .--.   ",
        "  |o_o |  ",
        "  |:_/ |  ",
        " //   \\ \\ ",
        "(|     | )",
        " \\'\\_._/'\\",
        " \\___)=(_/",
    ];
    let art_w = 10u32 * 8; let art_h = 7u32 * 8;
    let art_x = w.saturating_sub(art_w) / 2;
    let canvas_h = tb_y.saturating_sub(TOPBAR_H);
    let art_y = TOPBAR_H + canvas_h.saturating_sub(art_h + 80) / 2;
    for (i, line) in art.iter().enumerate() {
        fb.draw_str(art_x, art_y + i as u32 * 8, line, AMBER, BG);
    }
    let tag = "Binary hardware. Ternary mind.";
    fb.draw_str(w.saturating_sub(tag.len() as u32 * 8) / 2, art_y + art_h + 8, tag, DIM, BG);
    // Bottom taskbar
    fb.fill_rect(0, tb_y, w, 28, TASKBAR);
    fb.fill_rect(0, tb_y, w, 1, BORDER);
    fb.draw_bitmap_2x(4, tb_y + 6, &DINGIR, GREEN, TASKBAR);
    fb.draw_str(28, tb_y + 10, "RUSTY PENGUIN", GREEN, TASKBAR);
    // Top bar placeholder (will be filled by draw_topbar)
    fb.fill_rect(0, 0, w, TOPBAR_H, TOPBAR);
    fb.fill_rect(0, TOPBAR_H - 1, w, 1, 0x1E293B);
}

// ---- Launcher buttons ----
struct Launcher { label: &'static str, cmd: Option<&'static str>, title: &'static str, color: u32 }
const LAUNCHERS: &[Launcher] = &[
    Launcher { label: " psh ", cmd: None,              title: "psh — Terminal",    color: GREEN },
    Launcher { label: " ps  ", cmd: Some("ps\n"),      title: "ps — Processes",    color: BLUE  },
    Launcher { label: " ai  ", cmd: Some("ai 32\n"),   title: "ai — Inference",    color: AMBER },
    Launcher { label: " trit", cmd: Some("trit 42\n"), title: "trit — Ternary",    color: 0xC084FC },
];

fn launcher_rects(fw: u32, fh: u32) -> [(u32, u32, u32, u32); 4] {
    let bw: u32 = 52; let bh: u32 = 20; let gap: u32 = 8;
    let total = 4 * bw + 3 * gap;
    let sx = fw.saturating_sub(total) / 2;
    let y  = fh - 28 - bh - 10;
    [
        (sx,               y, bw, bh),
        (sx + bw + gap,    y, bw, bh),
        (sx + 2*(bw+gap),  y, bw, bh),
        (sx + 3*(bw+gap),  y, bw, bh),
    ]
}

fn draw_launchers(fb: &mut Framebuffer) {
    let rects = launcher_rects(fb.width, fb.height);
    for (l, (x, y, w, h)) in LAUNCHERS.iter().zip(rects.iter()) {
        fb.fill_rect(*x, *y, *w, *h, DIMMER);
        fb.fill_rect(*x, *y, *w, 1,  l.color);
        fb.fill_rect(*x, *y, 1,  *h, l.color);
        fb.fill_rect(*x + *w - 1, *y, 1, *h, l.color);
        fb.fill_rect(*x, *y + *h - 1, *w, 1, l.color);
        fb.draw_str(*x + 2, *y + 6, l.label, l.color, DIMMER);
    }
}

fn launcher_hit(fw: u32, fh: u32, mx: i32, my: i32) -> Option<usize> {
    for (i, (x, y, w, h)) in launcher_rects(fw, fh).iter().enumerate() {
        if mx >= *x as i32 && mx < (*x + *w) as i32 && my >= *y as i32 && my < (*y + *h) as i32 {
            return Some(i);
        }
    }
    None
}

// ---- Taskbar minimized restore buttons ----
fn tbmin_rect(fw: u32, fh: u32, slot: usize) -> (i32, i32, i32, i32) {
    (160 + slot as i32 * 100, (fh - 22) as i32, 92, 18)
}

fn draw_taskbar_min_btns(fb: &mut Framebuffer, term_wins: &[TermWin]) {
    let fw = fb.width; let fh = fb.height;
    let mut slot = 0;
    for tw in term_wins { if !tw.win.minimized { continue; }
        let (x, y, w, h) = tbmin_rect(fw, fh, slot);
        if x + w >= fw as i32 { break; }
        fb.fill_rect(x as u32, y as u32, w as u32, h as u32, 0x1E293B);
        fb.fill_rect(x as u32, y as u32, w as u32, 1, BORDER);
        let lbl: String = tw.win.title.chars().take(((w - 4) / 8) as usize).collect();
        fb.draw_str((x + 2) as u32, (y + 5) as u32, &lbl, WHITE, 0x1E293B);
        slot += 1;
    }
}

fn tbmin_hit(fw: u32, fh: u32, wins: &[TermWin], mx: i32, my: i32) -> Option<usize> {
    let mut slot = 0;
    for (i, tw) in wins.iter().enumerate() {
        if !tw.win.minimized { continue; }
        let (x, y, w, h) = tbmin_rect(fw, fh, slot);
        if mx >= x && mx < x + w && my >= y && my < y + h { return Some(i); }
        slot += 1;
    }
    None
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

fn draw_topbar(fb: &mut Framebuffer, time: &str, s: &SysStats) {
    let fw = fb.width;
    fb.fill_rect(0, 0, fw, TOPBAR_H, TOPBAR);
    fb.fill_rect(0, TOPBAR_H - 1, fw, 1, 0x1E293B);

    // Left: time
    fb.draw_str(8, 4, time, WHITE, TOPBAR);

    // Right: stats — build a compact string right-aligned
    // Format: CPU 23%  MEM 74%  SWAP 5%  v0.1kB ^0.1kB
    let rx_str = if s.net_rx_kb > 999 { format!("{}M", s.net_rx_kb/1024) }
                 else                 { format!("{}K", s.net_rx_kb) };
    let tx_str = if s.net_tx_kb > 999 { format!("{}M", s.net_tx_kb/1024) }
                 else                 { format!("{}K", s.net_tx_kb) };

    // Draw each stat from right to left
    let mut rx = fw as i32 - 8;

    // net tx (upload arrow up)
    let up = format!("^{}", tx_str);
    rx -= up.len() as i32 * 8;
    fb.draw_str(rx as u32, 4, &up, 0x34D399, TOPBAR);
    rx -= 8;

    // net rx (download arrow down)
    let dn = format!("v{}", rx_str);
    rx -= dn.len() as i32 * 8;
    fb.draw_str(rx as u32, 4, &dn, 0x60A5FA, TOPBAR);
    rx -= 16;

    if s.swap_pct > 0 {
        let sw = format!("SW{}%", s.swap_pct);
        rx -= sw.len() as i32 * 8;
        fb.draw_str(rx as u32, 4, &sw, 0xA78BFA, TOPBAR);
        rx -= 16;
    }

    let mem = format!("M{}%", s.mem_pct);
    rx -= mem.len() as i32 * 8;
    fb.draw_str(rx as u32, 4, &mem, 0x4ADE80, TOPBAR);
    rx -= 16;

    let cpu = format!("C{}%", s.cpu_pct);
    rx -= cpu.len() as i32 * 8;
    fb.draw_str(rx as u32, 4, &cpu, 0xFBBF24, TOPBAR);
}

// ---- Start menu ----
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

fn start_menu_bounds_hit(fh: u32, mx: i32, my: i32) -> bool {
    let (x, y, w, h) = start_menu_bounds(fh);
    mx >= x && mx < x + w && my >= y && my < y + h
}

// ---- Full scene recomposite ----
fn recomposite(fb: &mut Framebuffer, wins: &mut Vec<TermWin>, start_menu: bool, stats: &SysStats) {
    draw_scene_static(fb);
    draw_launchers(fb);
    draw_taskbar_min_btns(fb, wins);
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
    // Topbar always on top
    draw_topbar(fb, &read_rtc(), stats);
}

// ---- Window opening ----
struct TermWin { win: wm::Window, term: term::Terminal, win_dirty: bool, initial_cmd: Option<Vec<u8>> }

fn open_term(w: i32, h: i32, n: usize, l: &Launcher) -> Option<TermWin> {
    match term::Terminal::spawn() {
        Ok(t) => {
            let off = n as i32 * 20;
            let wx = ((w - wm::WINDOW_W) / 2 + off).max(0).min(w - wm::WINDOW_W);
            let wy = ((h - wm::WINDOW_H - 28) / 2 + off).max(0).min(h - wm::WINDOW_H - 28);
            slog(&format!("terminal '{}' opened at {}x{}", l.title, wx, wy));
            Some(TermWin {
                win: wm::Window::new(wx, wy, l.title),
                term: t, win_dirty: true,
                initial_cmd: l.cmd.map(|s| s.as_bytes().to_vec()),
            })
        }
        Err(e) => { slog(&format!("spawn failed: {}", e)); None }
    }
}

fn exec_psh() -> ! {
    use std::os::unix::process::CommandExt;
    let _ = std::process::Command::new("/bin/psh").exec();
    let _ = std::process::Command::new("/usr/local/bin/psh").exec();
    loop { thread::sleep(Duration::from_secs(60)); }
}

fn main() {
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
    draw_launchers(&mut fb);
    let mut sampler = StatSampler::new();
    sampler.sample();
    draw_topbar(&mut fb, &read_rtc(), &sampler.stats);

    let cbl = (CURSOR_W * CURSOR_H) as usize;
    let mut cbuf = vec![BG; cbl];
    let mut cx: i32; let mut cy: i32;
    { let s = mouse.lock().unwrap(); cx = s.x; cy = s.y; }
    save_cursor_bg(&fb, cx, cy, &mut cbuf);
    draw_cursor(&mut fb, cx, cy);

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
        }

        // Top bar: clock + live stats, update every ~2s
        if tick % 120 == 0 {
            sampler.sample();
            restore_cursor_bg(&mut fb, cx, cy, &cbuf);
            draw_topbar(&mut fb, &read_rtc(), &sampler.stats);
            save_cursor_bg(&fb, cx, cy, &mut cbuf);
            draw_cursor(&mut fb, cx, cy);
        }
        tick = tick.wrapping_add(1);

        // ---- Click handling ----
        let left_down = (btn & 0x01) != 0;
        let left_edge = left_down && (prev_btn & 0x01) == 0;

        if left_edge {
            // Start menu intercepts all clicks
            if start_menu_open {
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
                // Find topmost window under click
                let hit = wins.iter().enumerate().rev()
                    .find(|(_, tw)| wm::window_hit(&tw.win, cx, cy))
                    .map(|(i, _)| i);

                if let Some(hi) = hit {
                    // Bring to front
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
                } else {
                    // Taskbar minimized button?
                    if let Some(mi) = tbmin_hit(fb.width, fb.height, &wins, cx, cy) {
                        wins[mi].win.minimized = false;
                        let tw = wins.remove(mi); wins.push(tw);
                        scene_dirty = true;
                    } else if let Some(li) = launcher_hit(fb.width, fb.height, cx, cy) {
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
                    let ny2 = (cy - tw.win.drag_oy).max(0).min(h - tw.win.h - 28);
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
