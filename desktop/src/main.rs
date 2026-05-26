// Rusty Penguin Desktop — minimal framebuffer GUI
// Falls back to exec psh if /dev/fb0 is unavailable.

mod fb;
mod font;
mod input;
mod wm;

use std::os::unix::io::AsRawFd;
use std::os::unix::process::CommandExt;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use fb::Framebuffer;
use input::MouseState;

// Stop the kernel fbcon driver from writing text over our pixels.
// Must be called once we own the framebuffer.
fn set_vt_graphics() {
    const KDSETMODE: u64 = 0x4B3A;
    const KD_GRAPHICS: u64 = 0x01;
    for path in &["/dev/tty0", "/dev/tty1", "/dev/tty"] {
        if let Ok(f) = std::fs::OpenOptions::new().read(true).write(true).open(path) {
            unsafe { libc::ioctl(f.as_raw_fd(), KDSETMODE, KD_GRAPHICS); }
            return;
        }
    }
}

// Restore text mode before handing off to psh.
fn set_vt_text() {
    const KDSETMODE: u64 = 0x4B3A;
    const KD_TEXT: u64 = 0x00;
    for path in &["/dev/tty0", "/dev/tty1", "/dev/tty"] {
        if let Ok(f) = std::fs::OpenOptions::new().read(true).write(true).open(path) {
            unsafe { libc::ioctl(f.as_raw_fd(), KDSETMODE, KD_TEXT); }
            return;
        }
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

// ---- Cursor shape: 12 wide x 20 tall, arrow pointing upper-left ----
// Row index = y, bit index = x (left to right)
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

fn draw_cursor(fb: &mut Framebuffer, x: i32, y: i32) {
    for row in 0..CURSOR_H as i32 {
        for col in 0..CURSOR_W as i32 {
            if CURSOR_SHAPE[row as usize][col as usize] {
                let px = x + col;
                let py = y + row;
                if px >= 0 && py >= 0 {
                    fb.set_pixel(px as u32, py as u32, CURSOR);
                }
            }
        }
    }
}

fn erase_cursor(fb: &mut Framebuffer, x: i32, y: i32, height: u32) {
    // Erase by filling cursor bounding box with BG.
    // Rows in taskbar area get TASKBAR color instead.
    let taskbar_y = height as i32 - 28;
    for row in 0..CURSOR_H as i32 {
        for col in 0..CURSOR_W as i32 {
            let px = x + col;
            let py = y + row;
            if px >= 0 && py >= 0 && (px as u32) < fb.width && (py as u32) < fb.height {
                let color = if py >= taskbar_y { TASKBAR } else { BG };
                fb.set_pixel(px as u32, py as u32, color);
            }
        }
    }
}

fn draw_initial_desktop(fb: &mut Framebuffer) {
    let w = fb.width;
    let h = fb.height;

    // Background
    fb.fill_rect(0, 0, w, h, BG);

    // Taskbar (bottom 28px)
    let tb_y = h - 28;
    fb.fill_rect(0, tb_y, w, 28, TASKBAR);

    // Taskbar border line
    fb.fill_rect(0, tb_y, w, 1, BORDER);

    // "RUSTY PENGUIN" label in taskbar
    fb.draw_str(12, tb_y + 10, "RUSTY PENGUIN", GREEN, TASKBAR);

    // Penguin ASCII art — centered
    let art = [
        "  .---.  ",
        " ( o o ) ",
        "  ( u )  ",
        " /     \\ ",
        "|       |",
        " \\_____/ ",
        "   | |   ",
    ];
    let art_w = 9u32 * 8;
    let art_h = 7u32 * 8;
    let art_x = (w.saturating_sub(art_w)) / 2;
    let art_y = (h.saturating_sub(art_h + 80)) / 2;
    for (i, line) in art.iter().enumerate() {
        fb.draw_str(art_x, art_y + (i as u32) * 8, line, AMBER, BG);
    }

    // Tagline below art
    let tag = "Binary hardware. Ternary mind.";
    let tag_w = tag.len() as u32 * 8;
    let tag_x = (w.saturating_sub(tag_w)) / 2;
    fb.draw_str(tag_x, art_y + art_h + 8, tag, DIM, BG);

    // PSH button centered
    let btn_x = w / 2 - 40;
    let btn_y = h / 2 + 60;
    draw_psh_button(fb, btn_x, btn_y);
}

fn draw_psh_button(fb: &mut Framebuffer, bx: u32, by: u32) {
    // Border
    fb.fill_rect(bx, by, 80, 24, GREEN);
    // Inner fill
    fb.fill_rect(bx + 1, by + 1, 78, 22, TASKBAR);
    // Text "[ psh ]" — 7 chars = 56px, center in 80px => offset 12
    fb.draw_str(bx + 12, by + 8, "[ psh ]", GREEN, TASKBAR);
}

fn draw_taskbar_clock(fb: &mut Framebuffer, time_str: &str) {
    let h = fb.height;
    let w = fb.width;
    let tb_y = h - 28;
    let text_w = time_str.len() as u32 * 8;
    let tx = w.saturating_sub(text_w + 12);
    // Clear clock area
    fb.fill_rect(tx.saturating_sub(4), tb_y + 1, text_w + 8, 26, TASKBAR);
    fb.draw_str(tx, tb_y + 10, time_str, WHITE, TASKBAR);
}

fn read_rtc_time() -> String {
    // Read time from /proc/driver/rtc or fall back to a static placeholder.
    // In the initramfs, full RTC access may not be available.
    if let Ok(rtc) = std::fs::read_to_string("/proc/driver/rtc") {
        for line in rtc.lines() {
            if line.starts_with("rtc_time") {
                let parts: Vec<&str> = line.splitn(2, ':').collect();
                if parts.len() == 2 {
                    return parts[1].trim().to_string();
                }
            }
        }
    }
    // Fallback — just show dashes
    "--:--:--".to_string()
}

fn exec_psh() -> ! {
    set_vt_text();
    let candidates = ["/bin/psh", "/usr/local/bin/psh"];
    for psh in &candidates {
        if std::path::Path::new(psh).exists() {
            let _ = Command::new(psh).exec();
        }
    }
    eprintln!("[desktop] psh not found");
    loop { std::thread::sleep(Duration::from_secs(60)); }
}

fn main() {
    // Attempt to open framebuffer
    let mut fb = match Framebuffer::open() {
        Ok(f) => f,
        Err(e) => {
            eprintln!("[desktop] framebuffer unavailable ({}), exec psh", e);
            set_vt_text();
            exec_psh();
        }
    };

    // Take ownership: tell the kernel to stop printing text onto our framebuffer
    set_vt_graphics();

    let width = fb.width as i32;
    let height = fb.height as i32;

    // Shared mouse state
    let mouse_state = Arc::new(Mutex::new(MouseState {
        x: width / 2,
        y: height / 2,
        buttons: 0,
    }));

    // Spawn mouse reader thread
    {
        let ms = Arc::clone(&mouse_state);
        thread::spawn(move || {
            input::mouse_thread(ms, width, height);
        });
    }

    // Initial desktop draw
    draw_initial_desktop(&mut fb);

    // Draw initial cursor
    let mut cur_x;
    let mut cur_y;
    {
        let s = mouse_state.lock().unwrap();
        cur_x = s.x;
        cur_y = s.y;
    }
    draw_cursor(&mut fb, cur_x, cur_y);

    // PSH button rect for hit testing
    let btn_x = (fb.width / 2 - 40) as i32;
    let btn_y = (fb.height / 2 + 60) as i32;

    let mut prev_buttons: u8 = 0;
    let mut tick: u64 = 0;

    loop {
        std::thread::sleep(Duration::from_millis(16));

        let (new_x, new_y, buttons) = {
            let s = mouse_state.lock().unwrap();
            (s.x, s.y, s.buttons)
        };

        // Redraw cursor if moved
        if new_x != cur_x || new_y != cur_y {
            let fh = fb.height;
            erase_cursor(&mut fb, cur_x, cur_y, fh);
            cur_x = new_x;
            cur_y = new_y;
            draw_cursor(&mut fb, cur_x, cur_y);
        }

        // Update clock roughly once per second (~60 frames)
        if tick % 60 == 0 {
            let t = read_rtc_time();
            draw_taskbar_clock(&mut fb, &t);
            // Redraw cursor if clock area overlapped (unlikely but safe)
            draw_cursor(&mut fb, cur_x, cur_y);
        }
        tick = tick.wrapping_add(1);

        // Detect left button click (rising edge)
        let left_pressed = (buttons & 0x01) != 0;
        let left_was_pressed = (prev_buttons & 0x01) != 0;
        if left_pressed && !left_was_pressed {
            // Check psh button hit
            if cur_x >= btn_x
                && cur_x < btn_x + 80
                && cur_y >= btn_y
                && cur_y < btn_y + 24
            {
                // Flash button
                let bx = btn_x as u32;
                let by = btn_y as u32;
                fb.fill_rect(bx + 1, by + 1, 78, 22, BG);
                fb.draw_str(bx + 12, by + 8, "[ psh ]", GREEN, BG);
                std::thread::sleep(Duration::from_millis(100));
                exec_psh();
            }
        }
        prev_buttons = buttons;
    }
}
