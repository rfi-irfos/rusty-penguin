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
const TASKBAR: u32 = 0x111827;
const BORDER:  u32 = 0x1E293B;
const GREEN:   u32 = 0x4ADE80;
const DIM:     u32 = 0x334155;
const DIMMER:  u32 = 0x1E293B;
const WHITE:   u32 = 0xF8FAFC;
const AMBER:   u32 = 0xFBBF24;
const BLUE:    u32 = 0x60A5FA;
const CURSOR:  u32 = 0xF8FAFC;
const TASKBTN_BG:  u32 = 0x1E293B;
const TASKBTN_ACT: u32 = 0x334155;

const CURSOR_W: u32 = 12;
const CURSOR_H: u32 = 20;

// Dingir (𒀭) — 8-pointed star
#[rustfmt::skip]
const DINGIR: [u8; 8] = [
    0x18, 0x5A, 0x3C, 0xFF, 0x3C, 0x5A, 0x18, 0x00,
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

// ---- Desktop background ----
fn draw_desktop_bg(fb: &mut Framebuffer) {
    let w = fb.width; let h = fb.height;
    fb.fill_rect(0, 0, w, h, BG);
    // Subtle grid
    let mut gx = 0u32;
    while gx < w { fb.fill_rect(gx, 0, 1, h.saturating_sub(28), 0x0D1628); gx += 40; }
    let mut gy = 0u32;
    while gy < h.saturating_sub(28) { fb.fill_rect(0, gy, w, 1, 0x0D1628); gy += 40; }

    // Tux art — centered
    let art = [
        "   .--.   ",
        "  |o_o |  ",
        "  |:_/ |  ",
        " //   \\ \\ ",
        "(|     | )",
        " \\'\\_._/'\\",
        " \\___)=(_/",
    ];
    let art_w = 10u32 * 8;
    let art_h = 7u32 * 8;
    let art_x = w.saturating_sub(art_w) / 2;
    let art_y = (h.saturating_sub(28 + art_h + 72)) / 2;
    for (i, line) in art.iter().enumerate() {
        fb.draw_str(art_x, art_y + i as u32 * 8, line, AMBER, BG);
    }
    let tag = "Binary hardware. Ternary mind.";
    let tag_w = tag.len() as u32 * 8;
    fb.draw_str(w.saturating_sub(tag_w) / 2, art_y + art_h + 8, tag, DIM, BG);

    // Taskbar
    let tb_y = h - 28;
    fb.fill_rect(0, tb_y, w, 28, TASKBAR);
    fb.fill_rect(0, tb_y, w, 1, BORDER);
    fb.draw_bitmap_2x(4, tb_y + 6, &DINGIR, GREEN, TASKBAR);
    fb.draw_str(28, tb_y + 10, "RUSTY PENGUIN", GREEN, TASKBAR);
}

// ---- Launcher buttons ----
struct Launcher {
    label: &'static str,
    cmd:   Option<&'static str>, // psh command pre-seeded on open
    title: &'static str,
    color: u32,
}

const LAUNCHERS: &[Launcher] = &[
    Launcher { label: " psh ",  cmd: None,              title: "psh — Penguin Shell",   color: GREEN },
    Launcher { label: " ps  ",  cmd: Some("ps\n"),      title: "ps — Processes",        color: BLUE  },
    Launcher { label: " ai  ",  cmd: Some("ai 32\n"),   title: "ai — Ternary Inference", color: AMBER },
    Launcher { label: " trit",  cmd: Some("trit 42\n"), title: "trit — Ternary Calc",   color: 0xC084FC },
];

fn launcher_rects(fb_w: u32, fb_h: u32) -> Vec<(u32, u32, u32, u32)> {
    let btn_w: u32 = 52;
    let btn_h: u32 = 20;
    let gap:   u32 = 8;
    let n = LAUNCHERS.len() as u32;
    let total_w = n * btn_w + (n - 1) * gap;
    let start_x = fb_w.saturating_sub(total_w) / 2;
    let y = fb_h - 28 - btn_h - 12;
    (0..n).map(|i| (start_x + i * (btn_w + gap), y, btn_w, btn_h)).collect()
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

fn launcher_hit(fb_w: u32, fb_h: u32, mx: i32, my: i32) -> Option<usize> {
    let rects = launcher_rects(fb_w, fb_h);
    for (i, (x, y, w, h)) in rects.iter().enumerate() {
        if mx >= *x as i32 && mx < (*x + *w) as i32
            && my >= *y as i32 && my < (*y + *h) as i32
        {
            return Some(i);
        }
    }
    None
}

// ---- Taskbar minimized buttons ----
fn taskbar_min_btn_rect(fb_w: u32, fb_h: u32, idx: usize) -> (i32, i32, i32, i32) {
    let x = 160 + idx as i32 * 96;
    let y = (fb_h - 22) as i32;
    (x, y, 88, 18)
}

fn draw_taskbar_min_buttons(fb: &mut Framebuffer, term_wins: &[TermWin]) {
    let w = fb.width; let h = fb.height;
    let mut slot = 0;
    for tw in term_wins.iter() {
        if !tw.win.minimized { continue; }
        let (x, y, bw, bh) = taskbar_min_btn_rect(w, h, slot);
        if x + bw >= w as i32 { break; }
        fb.fill_rect(x as u32, y as u32, bw as u32, bh as u32, TASKBTN_ACT);
        fb.fill_rect(x as u32, y as u32, bw as u32, 1, BORDER);
        let max_chars = ((bw - 4) / 8) as usize;
        let label: String = tw.win.title.chars().take(max_chars).collect();
        fb.draw_str((x + 2) as u32, (y + 5) as u32, &label, WHITE, TASKBTN_ACT);
        slot += 1;
    }
}

fn taskbar_min_btn_hit(fb_w: u32, fb_h: u32, term_wins: &[TermWin], mx: i32, my: i32) -> Option<usize> {
    let mut slot = 0;
    for (i, tw) in term_wins.iter().enumerate() {
        if !tw.win.minimized { continue; }
        let (x, y, bw, bh) = taskbar_min_btn_rect(fb_w, fb_h, slot);
        if mx >= x && mx < x + bw && my >= y && my < y + bh {
            return Some(i);
        }
        slot += 1;
    }
    None
}

fn draw_taskbar_clock(fb: &mut Framebuffer, time_str: &str) {
    let h = fb.height; let w = fb.width;
    let tb_y = h - 28;
    let text_w = time_str.len() as u32 * 8;
    let tx = w.saturating_sub(text_w + 12);
    fb.fill_rect(tx.saturating_sub(4), tb_y + 1, text_w + 8, 26, TASKBAR);
    fb.draw_str(tx, tb_y + 10, time_str, WHITE, TASKBAR);
}

fn read_rtc_time() -> String {
    if let Ok(rtc) = std::fs::read_to_string("/proc/driver/rtc") {
        for line in rtc.lines() {
            if line.starts_with("rtc_time") {
                if let Some(v) = line.splitn(2, ':').nth(1) {
                    return v.trim().to_string();
                }
            }
        }
    }
    "--:--:--".to_string()
}

// ---- Full scene recomposite ----
fn recomposite(fb: &mut Framebuffer, term_wins: &mut Vec<TermWin>) {
    draw_desktop_bg(fb);
    draw_launchers(fb);
    draw_taskbar_min_buttons(fb, term_wins);
    let n = term_wins.len();
    for (i, tw) in term_wins.iter_mut().enumerate() {
        if tw.win.minimized { continue; }
        wm::draw_window(fb, &tw.win, i == n - 1);
        let (ox, oy) = wm::content_origin(&tw.win);
        tw.term.render(fb, ox as u32, oy as u32);
        tw.term.dirty = false;
        tw.win_dirty = false;
    }
}

// ---- Terminal window ----
struct TermWin {
    win:         wm::Window,
    term:        term::Terminal,
    win_dirty:   bool,
    initial_cmd: Option<Vec<u8>>,
}

fn open_term(width: i32, height: i32, idx: usize, l: &Launcher) -> Option<TermWin> {
    match term::Terminal::spawn() {
        Ok(t) => {
            let cascade = idx as i32 * 20;
            let wx = ((width  - wm::WINDOW_W) / 2 + cascade).max(0).min(width  - wm::WINDOW_W);
            let wy = ((height - wm::WINDOW_H - 28) / 2 + cascade).max(0).min(height - wm::WINDOW_H - 28);
            slog(&format!("terminal '{}' opened at {}x{}", l.title, wx, wy));
            Some(TermWin {
                win: wm::Window::new(wx, wy, l.title),
                term: t,
                win_dirty: true,
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
        Ok(f) => f,
        Err(e) => {
            slog(&format!("framebuffer unavailable: {}", e));
            exec_psh();
        }
    };

    slog(&format!("framebuffer: {}x{}x{}", fb.width, fb.height, fb.bpp));
    unbind_fbcon();

    let width  = fb.width  as i32;
    let height = fb.height as i32;

    let mouse_state = Arc::new(Mutex::new(MouseState { x: width / 2, y: height / 2, buttons: 0 }));
    { let ms = Arc::clone(&mouse_state); thread::spawn(move || input::mouse_thread(ms, width, height)); }

    let (kb_tx, kb_rx) = std::sync::mpsc::channel::<Vec<u8>>();
    thread::spawn(move || keyboard::keyboard_thread(kb_tx));

    // Initial draw
    draw_desktop_bg(&mut fb);
    draw_launchers(&mut fb);

    let cursor_buf_len = (CURSOR_W * CURSOR_H) as usize;
    let mut cursor_bg  = vec![BG; cursor_buf_len];
    let mut cur_x; let mut cur_y;
    { let s = mouse_state.lock().unwrap(); cur_x = s.x; cur_y = s.y; }
    save_cursor_bg(&fb, cur_x, cur_y, &mut cursor_bg);
    draw_cursor(&mut fb, cur_x, cur_y);

    let mut prev_buttons: u8 = 0;
    let mut tick: u64 = 0;
    let mut term_wins: Vec<TermWin> = Vec::new();

    loop {
        thread::sleep(Duration::from_millis(16));

        let (new_x, new_y, buttons) = {
            let s = mouse_state.lock().unwrap(); (s.x, s.y, s.buttons)
        };

        // Keyboard → focused (topmost) window
        while let Ok(data) = kb_rx.try_recv() {
            if let Some(tw) = term_wins.last() {
                tw.term.write_input(&data);
            }
        }

        // PTY poll + initial command injection
        for tw in term_wins.iter_mut() {
            if let Some(cmd) = tw.initial_cmd.take() {
                tw.term.write_input(&cmd);
            }
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

        let cursor_moved    = new_x != cur_x || new_y != cur_y;
        let any_dirty       = term_wins.iter().any(|tw| tw.term.dirty || tw.win_dirty);
        let needs_redraw    = cursor_moved || any_dirty;

        if needs_redraw {
            restore_cursor_bg(&mut fb, cur_x, cur_y, &cursor_bg);
            if any_dirty {
                recomposite(&mut fb, &mut term_wins);
            }
            if cursor_moved { cur_x = new_x; cur_y = new_y; }
            save_cursor_bg(&fb, cur_x, cur_y, &mut cursor_bg);
            draw_cursor(&mut fb, cur_x, cur_y);
        }

        // Clock (~1/s)
        if tick % 60 == 0 {
            restore_cursor_bg(&mut fb, cur_x, cur_y, &cursor_bg);
            draw_taskbar_clock(&mut fb, &read_rtc_time());
            save_cursor_bg(&fb, cur_x, cur_y, &mut cursor_bg);
            draw_cursor(&mut fb, cur_x, cur_y);
        }
        tick = tick.wrapping_add(1);

        // ---- Click handling ----
        let left_now = (buttons & 0x01) != 0;
        let left_was = (prev_buttons & 0x01) != 0;

        if left_now && !left_was {
            // Find topmost window under click (reverse order = topmost first)
            let hit_idx = term_wins.iter().enumerate().rev()
                .find(|(_, tw)| wm::window_hit(&tw.win, cur_x, cur_y))
                .map(|(i, _)| i);

            if let Some(hi) = hit_idx {
                // Bring to front if not already
                if hi != term_wins.len() - 1 {
                    let tw = term_wins.remove(hi);
                    term_wins.push(tw);
                    let last = term_wins.len() - 1;
                    term_wins[last].win_dirty = true;
                }
                let last = term_wins.len() - 1;
                let tw = &mut term_wins[last];

                if wm::close_btn_hit(&tw.win, cur_x, cur_y) {
                    restore_cursor_bg(&mut fb, cur_x, cur_y, &cursor_bg);
                    term_wins.remove(last);
                    recomposite(&mut fb, &mut term_wins);
                    save_cursor_bg(&fb, cur_x, cur_y, &mut cursor_bg);
                    draw_cursor(&mut fb, cur_x, cur_y);
                } else if wm::min_btn_hit(&tw.win, cur_x, cur_y) {
                    tw.win.minimized = true;
                    tw.win_dirty = true;
                } else if wm::max_btn_hit(&tw.win, cur_x, cur_y) {
                    tw.win.toggle_maximize(width, height);
                    tw.win_dirty = true;
                } else if wm::titlebar_hit(&tw.win, cur_x, cur_y) {
                    tw.win.dragging = true;
                    tw.win.drag_ox  = cur_x - tw.win.x;
                    tw.win.drag_oy  = cur_y - tw.win.y;
                }
            } else {
                // No window hit — check taskbar minimized buttons
                let min_hit = taskbar_min_btn_hit(fb.width, fb.height, &term_wins, cur_x, cur_y);
                if let Some(mi) = min_hit {
                    term_wins[mi].win.minimized = false;
                    term_wins[mi].win_dirty = true;
                    // Bring restored window to front
                    let tw = term_wins.remove(mi);
                    term_wins.push(tw);
                } else if let Some(li) = launcher_hit(fb.width, fb.height, cur_x, cur_y) {
                    // Open a new terminal window
                    if let Some(tw) = open_term(width, height, term_wins.len(), &LAUNCHERS[li]) {
                        term_wins.push(tw);
                    }
                }
            }
        }

        // ---- Drag ----
        if left_now {
            if let Some(tw) = term_wins.last_mut() {
                if tw.win.dragging {
                    let nx = (cur_x - tw.win.drag_ox).max(0).min(width  - tw.win.w);
                    let ny = (cur_y - tw.win.drag_oy).max(0).min(height - tw.win.h - 28);
                    if nx != tw.win.x || ny != tw.win.y {
                        tw.win.x = nx; tw.win.y = ny;
                        tw.win_dirty = true;
                    }
                }
            }
        } else {
            for tw in term_wins.iter_mut() { tw.win.dragging = false; }
        }

        prev_buttons = buttons;
    }
}
