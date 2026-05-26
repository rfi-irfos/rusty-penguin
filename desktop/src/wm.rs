// Minimal window manager
use crate::fb::Framebuffer;
use crate::term;

pub const TITLEBAR_H: i32 = 18;

// Terminal window dimensions: 1px border on each side + title bar
pub const WINDOW_W: i32 = term::TERM_PIX_W as i32 + 2;
pub const WINDOW_H: i32 = term::TERM_PIX_H as i32 + 2 + TITLEBAR_H;

const BORDER_COLOR:   u32 = 0x334155;
const TITLEBAR_COLOR: u32 = 0x1E293B;
const CONTENT_COLOR:  u32 = 0x0F172A;
const TITLE_FG:       u32 = 0xF8FAFC;
const BTN_CLOSE:      u32 = 0xEF4444;
const BTN_MIN:        u32 = 0xFBBF24;
const BTN_MAX:        u32 = 0x4ADE80;

pub struct Window {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub title: String,
    pub dragging: bool,
    pub drag_ox: i32,
    pub drag_oy: i32,
}

// Button bounding boxes: (x, y, w, h) relative to framebuffer
fn close_rect(win: &Window) -> (i32, i32, i32, i32) {
    (win.x + win.w - 14, win.y + 4, 10, 10)
}
fn min_rect(win: &Window) -> (i32, i32, i32, i32) {
    (win.x + win.w - 26, win.y + 4, 10, 10)
}
fn max_rect(win: &Window) -> (i32, i32, i32, i32) {
    (win.x + win.w - 38, win.y + 4, 10, 10)
}

fn in_rect(mx: i32, my: i32, x: i32, y: i32, w: i32, h: i32) -> bool {
    mx >= x && mx < x + w && my >= y && my < y + h
}

pub fn close_btn_hit(win: &Window, mx: i32, my: i32) -> bool {
    let (x, y, w, h) = close_rect(win);
    in_rect(mx, my, x, y, w, h)
}
pub fn min_btn_hit(win: &Window, mx: i32, my: i32) -> bool {
    let (x, y, w, h) = min_rect(win);
    in_rect(mx, my, x, y, w, h)
}
pub fn max_btn_hit(win: &Window, mx: i32, my: i32) -> bool {
    let (x, y, w, h) = max_rect(win);
    in_rect(mx, my, x, y, w, h)
}

pub fn titlebar_hit(win: &Window, mx: i32, my: i32) -> bool {
    mx >= win.x + 1 && mx < win.x + win.w - 1
        && my >= win.y + 1 && my < win.y + 1 + TITLEBAR_H
        && !close_btn_hit(win, mx, my)
        && !min_btn_hit(win, mx, my)
        && !max_btn_hit(win, mx, my)
}

// Top-left pixel of the terminal content area inside the window
pub fn content_origin(win: &Window) -> (i32, i32) {
    (win.x + 1, win.y + 1 + TITLEBAR_H)
}

fn draw_btn(fb: &mut Framebuffer, x: i32, y: i32, color: u32, glyph: char) {
    if x < 0 || y < 0 { return; }
    fb.fill_rect(x as u32, y as u32, 10, 10, color);
    fb.draw_char((x + 1) as u32, (y + 1) as u32, glyph, 0x1E293B, color);
}

pub fn draw_window(fb: &mut Framebuffer, win: &Window) {
    if win.w <= 0 || win.h <= 0 { return; }

    // Outer border
    fb.fill_rect(win.x as u32, win.y as u32, win.w as u32, win.h as u32, BORDER_COLOR);

    // Title bar
    if win.x >= 0 && win.y >= 0 {
        fb.fill_rect(
            (win.x + 1) as u32, (win.y + 1) as u32,
            (win.w - 2).max(0) as u32, TITLEBAR_H as u32,
            TITLEBAR_COLOR,
        );

        // Title text — leave 44px for the three buttons on the right
        let max_chars = ((win.w - 50) / 8).max(0) as usize;
        let title: String = win.title.chars().take(max_chars).collect();
        fb.draw_str(
            (win.x + 4) as u32, (win.y + 5) as u32,
            &title, TITLE_FG, TITLEBAR_COLOR,
        );

        // Buttons (close, min, max left-to-right from right edge)
        let (cx, cy, _, _) = close_rect(win);
        draw_btn(fb, cx, cy, BTN_CLOSE, 'x');

        let (mx, my, _, _) = min_rect(win);
        draw_btn(fb, mx, my, BTN_MIN, '-');

        let (ax, ay, _, _) = max_rect(win);
        draw_btn(fb, ax, ay, BTN_MAX, '+');
    }

    // Content area — cleared to terminal background
    let cy = win.y + 1 + TITLEBAR_H;
    let ch = win.h - 2 - TITLEBAR_H;
    if ch > 0 && cy >= 0 {
        fb.fill_rect(
            (win.x + 1) as u32, cy as u32,
            (win.w - 2).max(0) as u32, ch as u32,
            CONTENT_COLOR,
        );
    }
}
