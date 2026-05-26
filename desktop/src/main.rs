// Rusty Penguin Desktop — framebuffer GUI with PTY terminal windows

mod fb;
mod font;
mod input;
mod keyboard;
mod term;
mod wm;

use std::os::unix::process::CommandExt;
use std::process::Command;
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
    for vtcon in &["vtcon0", "vtcon1"] {
        let path = format!("/sys/class/vtconsole/{}/bind", vtcon);
        std::fs::write(&path, "0\n").ok();
    }
}

// ---- Color palette ----
const BG: u32      = 0x0F172A;
const TASKBAR: u32 = 0x1E293B;
const BORDER: u32  = 0x334155;
const GREEN: u32   = 0x4ADE80;
const DIM: u32     = 0x475569;
const WHITE: u32   = 0xF8FAFC;
const AMBER: u32   = 0xFBBF24;
const CURSOR: u32  = 0xF8FAFC;

const CURSOR_W: u32 = 12;
const CURSOR_H: u32 = 20;

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
            let idx = (row * CURSOR_W as i32 + col) as usize;
            let px = x + col;
            let py = y + row;
            buf[idx] = if px >= 0 && py >= 0 && (px as u32) < fb.width && (py as u32) < fb.height {
                fb.get_pixel(px as u32, py as u32)
            } else { BG };
        }
    }
}

fn restore_cursor_bg(fb: &mut Framebuffer, x: i32, y: i32, buf: &[u32]) {
    for row in 0..CURSOR_H as i32 {
        for col in 0..CURSOR_W as i32 {
            let px = x + col;
            let py = y + row;
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
                let px = x + col;
                let py = y + row;
                if px >= 0 && py >= 0 && (px as u32) < fb.width && (py as u32) < fb.height {
                    fb.set_pixel(px as u32, py as u32, CURSOR);
                }
            }
        }
    }
}

fn draw_desktop_bg(fb: &mut Framebuffer) {
    let w = fb.width;
    let h = fb.height;
    fb.fill_rect(0, 0, w, h, BG);

    // Taskbar
    let tb_y = h - 28;
    fb.fill_rect(0, tb_y, w, 28, TASKBAR);
    fb.fill_rect(0, tb_y, w, 1, BORDER);
    fb.draw_str(12, tb_y + 10, "RUSTY PENGUIN", GREEN, TASKBAR);

    // Tux ASCII art
    let art = [
        "   .--.   ",
        "  |o_o |  ",
        "  |:_/ |  ",
        " //   \\ \\ ",
        "(|     | )",
        " \\'\\_ _/'\\",
        " \\___)=(_/",
    ];
    let art_w = 10u32 * 8;
    let art_h = 7u32 * 8;
    let art_x = (w.saturating_sub(art_w)) / 2;
    let art_y = (h.saturating_sub(art_h + 80)) / 2;
    for (i, line) in art.iter().enumerate() {
        fb.draw_str(art_x, art_y + i as u32 * 8, line, AMBER, BG);
    }

    let tag = "Binary hardware. Ternary mind.";
    let tag_w = tag.len() as u32 * 8;
    let tag_x = (w.saturating_sub(tag_w)) / 2;
    fb.draw_str(tag_x, art_y + art_h + 8, tag, DIM, BG);
}

fn draw_psh_button(fb: &mut Framebuffer, bx: u32, by: u32) {
    fb.fill_rect(bx, by, 80, 24, GREEN);
    fb.fill_rect(bx + 1, by + 1, 78, 22, TASKBAR);
    fb.draw_str(bx + 12, by + 8, "[ psh ]", GREEN, TASKBAR);
}

fn draw_taskbar_clock(fb: &mut Framebuffer, time_str: &str) {
    let h = fb.height;
    let w = fb.width;
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

fn exec_psh() -> ! {
    let candidates = ["/bin/psh", "/usr/local/bin/psh"];
    for psh in &candidates {
        if std::path::Path::new(psh).exists() {
            let _ = Command::new(psh).exec();
        }
    }
    eprintln!("[desktop] psh not found");
    loop { std::thread::sleep(Duration::from_secs(60)); }
}

// ---- Terminal window state ----
struct TermWin {
    win: wm::Window,
    term: term::Terminal,
    win_dirty: bool,  // window chrome needs full redraw
}

fn main() {
    slog("=== desktop starting ===");

    let mut fb = match Framebuffer::open() {
        Ok(f) => f,
        Err(e) => {
            slog(&format!("framebuffer unavailable: {}", e));
            eprintln!("[desktop] framebuffer unavailable ({}), exec psh", e);
            exec_psh();
        }
    };

    slog(&format!("framebuffer: {}x{}x{}", fb.width, fb.height, fb.bpp));
    unbind_fbcon();

    let width  = fb.width  as i32;
    let height = fb.height as i32;

    // Shared mouse state
    let mouse_state = Arc::new(Mutex::new(MouseState {
        x: width / 2,
        y: height / 2,
        buttons: 0,
    }));

    // Mouse reader thread
    {
        let ms = Arc::clone(&mouse_state);
        thread::spawn(move || input::mouse_thread(ms, width, height));
    }

    // Keyboard channel: raw stdin bytes → main loop → PTY master
    let (kb_tx, kb_rx) = std::sync::mpsc::channel::<Vec<u8>>();
    thread::spawn(move || keyboard::keyboard_thread(kb_tx));

    // Initial desktop draw
    draw_desktop_bg(&mut fb);
    let btn_x = (fb.width / 2 - 40) as i32;
    let btn_y = (fb.height / 2 + 60) as i32;
    draw_psh_button(&mut fb, btn_x as u32, btn_y as u32);

    // Cursor setup
    let cursor_buf_len = (CURSOR_W * CURSOR_H) as usize;
    let mut cursor_bg = vec![BG; cursor_buf_len];
    let mut cur_x;
    let mut cur_y;
    {
        let s = mouse_state.lock().unwrap();
        cur_x = s.x;
        cur_y = s.y;
    }
    save_cursor_bg(&fb, cur_x, cur_y, &mut cursor_bg);
    draw_cursor(&mut fb, cur_x, cur_y);

    let mut prev_buttons: u8 = 0;
    let mut tick: u64 = 0;
    let mut term_win: Option<TermWin> = None;

    loop {
        thread::sleep(Duration::from_millis(16));

        let (new_x, new_y, buttons) = {
            let s = mouse_state.lock().unwrap();
            (s.x, s.y, s.buttons)
        };

        // --- Forward keyboard input to active terminal ---
        while let Ok(data) = kb_rx.try_recv() {
            if let Some(ref tw) = term_win {
                tw.term.write_input(&data);
            }
        }

        // --- Poll PTY for new output ---
        if let Some(ref mut tw) = term_win {
            if tw.term.poll() {
                tw.term.dirty = true;
            }
            // Detect child exit (EIO on read already stops poll; check status)
            if matches!(tw.term.child.try_wait(), Ok(Some(_))) {
                // psh exited — write a notice to the terminal grid
                let msg = b"[process exited]";
                for (i, &b) in msg.iter().enumerate() {
                    let idx = (tw.term.cur_row * term::COLS).min(term::COLS * term::ROWS - 1) + i;
                    if idx < term::COLS * term::ROWS {
                        tw.term.cells[idx] = term::Cell { ch: b, fg: 0xFBBF24, bg: 0x0F172A };
                    }
                }
                tw.term.dirty = true;
            }
        }

        // --- Determine what needs redrawing ---
        let cursor_moved = new_x != cur_x || new_y != cur_y;
        let term_content_dirty = term_win.as_ref().map(|tw| tw.term.dirty).unwrap_or(false);
        let win_chrome_dirty   = term_win.as_ref().map(|tw| tw.win_dirty).unwrap_or(false);
        let needs_redraw = cursor_moved || term_content_dirty || win_chrome_dirty;

        if needs_redraw {
            restore_cursor_bg(&mut fb, cur_x, cur_y, &cursor_bg);

            if win_chrome_dirty {
                if let Some(ref mut tw) = term_win {
                    wm::draw_window(&mut fb, &tw.win);
                    let (ox, oy) = wm::content_origin(&tw.win);
                    tw.term.render(&mut fb, ox as u32, oy as u32);
                    tw.term.dirty = false;
                    tw.win_dirty = false;
                }
            } else if term_content_dirty {
                if let Some(ref mut tw) = term_win {
                    let (ox, oy) = wm::content_origin(&tw.win);
                    tw.term.render(&mut fb, ox as u32, oy as u32);
                    tw.term.dirty = false;
                }
            }

            if cursor_moved {
                cur_x = new_x;
                cur_y = new_y;
            }
            save_cursor_bg(&fb, cur_x, cur_y, &mut cursor_bg);
            draw_cursor(&mut fb, cur_x, cur_y);
        }

        // --- Clock (~once per second) ---
        if tick % 60 == 0 {
            restore_cursor_bg(&mut fb, cur_x, cur_y, &cursor_bg);
            draw_taskbar_clock(&mut fb, &read_rtc_time());
            save_cursor_bg(&fb, cur_x, cur_y, &mut cursor_bg);
            draw_cursor(&mut fb, cur_x, cur_y);
        }
        tick = tick.wrapping_add(1);

        // --- Mouse click (rising edge) ---
        let left_now = (buttons & 0x01) != 0;
        let left_was = (prev_buttons & 0x01) != 0;

        if left_now && !left_was {
            // Check close / drag-start on existing window
            let mut close_window = false;
            let mut start_drag   = false;
            if let Some(ref tw) = term_win {
                if wm::close_btn_hit(&tw.win, cur_x, cur_y) {
                    close_window = true;
                } else if wm::titlebar_hit(&tw.win, cur_x, cur_y) {
                    start_drag = true;
                }
                // min/max: stubs — don't crash
            }

            if close_window {
                restore_cursor_bg(&mut fb, cur_x, cur_y, &cursor_bg);
                drop(term_win.take()); // kills child, closes PTY fd
                draw_desktop_bg(&mut fb);
                draw_psh_button(&mut fb, btn_x as u32, btn_y as u32);
                save_cursor_bg(&fb, cur_x, cur_y, &mut cursor_bg);
                draw_cursor(&mut fb, cur_x, cur_y);
            } else if start_drag {
                if let Some(ref mut tw) = term_win {
                    tw.win.dragging = true;
                    tw.win.drag_ox = cur_x - tw.win.x;
                    tw.win.drag_oy = cur_y - tw.win.y;
                }
            } else if term_win.is_none() {
                // PSH button hit → open terminal window
                if cur_x >= btn_x && cur_x < btn_x + 80
                    && cur_y >= btn_y && cur_y < btn_y + 24
                {
                    match term::Terminal::spawn() {
                        Ok(t) => {
                            let wx = ((width  - wm::WINDOW_W) / 2).max(0);
                            let wy = ((height - wm::WINDOW_H - 28) / 2).max(0);
                            let win = wm::Window {
                                x: wx, y: wy,
                                w: wm::WINDOW_W,
                                h: wm::WINDOW_H,
                                title: "psh — Penguin Shell".to_string(),
                                dragging: false,
                                drag_ox: 0,
                                drag_oy: 0,
                            };
                            slog(&format!("terminal window opened at {}x{}", wx, wy));
                            term_win = Some(TermWin { win, term: t, win_dirty: true });
                        }
                        Err(e) => {
                            slog(&format!("spawn failed: {}", e));
                        }
                    }
                }
            }
        }

        // --- Window drag (button held) ---
        if left_now {
            let mut moved = false;
            let mut nx = 0i32;
            let mut ny = 0i32;
            let mut old_x = 0i32;
            let mut old_y = 0i32;
            let mut old_w = 0i32;
            let mut old_h = 0i32;
            if let Some(ref tw) = term_win {
                if tw.win.dragging {
                    let cx2 = cur_x - tw.win.drag_ox;
                    let cy2 = cur_y - tw.win.drag_oy;
                    let clamped_x = cx2.max(0).min(width  - tw.win.w);
                    let clamped_y = cy2.max(0).min(height - tw.win.h - 28);
                    if clamped_x != tw.win.x || clamped_y != tw.win.y {
                        moved = true;
                        old_x = tw.win.x; old_y = tw.win.y;
                        old_w = tw.win.w; old_h = tw.win.h;
                        nx = clamped_x;   ny = clamped_y;
                    }
                }
            }
            if moved {
                if let Some(ref mut tw) = term_win {
                    restore_cursor_bg(&mut fb, cur_x, cur_y, &cursor_bg);
                    // Clear old window rect by redrawing desktop there
                    fb.fill_rect(old_x as u32, old_y as u32, old_w as u32, old_h as u32, BG);
                    // Repaint desktop art that may have been under the old position
                    draw_desktop_bg(&mut fb);
                    draw_psh_button(&mut fb, btn_x as u32, btn_y as u32);
                    tw.win.x = nx; tw.win.y = ny;
                    wm::draw_window(&mut fb, &tw.win);
                    let (ox, oy) = wm::content_origin(&tw.win);
                    tw.term.render(&mut fb, ox as u32, oy as u32);
                    tw.term.dirty = false;
                    save_cursor_bg(&fb, cur_x, cur_y, &mut cursor_bg);
                    draw_cursor(&mut fb, cur_x, cur_y);
                }
            }
        } else {
            // Mouse button released — end drag
            if let Some(ref mut tw) = term_win {
                tw.win.dragging = false;
            }
        }

        prev_buttons = buttons;
    }
}
