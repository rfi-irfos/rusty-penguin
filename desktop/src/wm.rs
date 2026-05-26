// Minimal window manager
use crate::fb::Framebuffer;

const TITLEBAR_H: i32 = 18;
const BORDER_COLOR: u32 = 0x334155;
const TITLEBAR_COLOR: u32 = 0x1E293B;
const CONTENT_COLOR: u32 = 0x0F172A;
const TITLE_FG: u32 = 0xF8FAFC;

pub struct Window {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub title: String,
    pub dragging: bool,
}

pub fn draw_window(fb: &mut Framebuffer, win: &Window) {
    // Outer border
    fb.fill_rect(win.x as u32, win.y as u32, win.w as u32, win.h as u32, BORDER_COLOR);

    // Titlebar
    let tb_h = TITLEBAR_H as u32;
    if win.y >= 0 && win.x >= 0 {
        fb.fill_rect(
            (win.x + 1) as u32,
            (win.y + 1) as u32,
            (win.w - 2).max(0) as u32,
            tb_h,
            TITLEBAR_COLOR,
        );
        fb.draw_str(
            (win.x + 4) as u32,
            (win.y + 5) as u32,
            &win.title,
            TITLE_FG,
            TITLEBAR_COLOR,
        );
    }

    // Content area
    let content_y = win.y + 1 + TITLEBAR_H;
    let content_h = win.h - 2 - TITLEBAR_H;
    if content_h > 0 && content_y >= 0 {
        fb.fill_rect(
            (win.x + 1) as u32,
            content_y as u32,
            (win.w - 2).max(0) as u32,
            content_h as u32,
            CONTENT_COLOR,
        );
    }
}

pub fn titlebar_hit(win: &Window, mx: i32, my: i32) -> bool {
    mx >= win.x + 1
        && mx < win.x + win.w - 1
        && my >= win.y + 1
        && my < win.y + 1 + TITLEBAR_H
}
