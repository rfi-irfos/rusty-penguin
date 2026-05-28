use alloc::string::{String, ToString};
use crate::fb::Framebuffer;
use crate::term;

pub const TITLEBAR_H: i32 = 22;
pub const WINDOW_W:   i32 = term::TERM_PIX_W as i32 + 2;
pub const WINDOW_H:   i32 = term::TERM_PIX_H as i32 + 2 + TITLEBAR_H;

// Window styling — matches Ubuntu Yaru theme
const SHADOW:      u32 = 0x05080F;  // Deep shadow
const BORDER_DIM:  u32 = 0x3C3C48;  // Inactive window border
const BORDER_ACT:  u32 = 0x4A9EFF;  // Active window border (blue)
const TITLE_DIM:   u32 = 0x1F1F2B;  // Inactive titlebar
const TITLE_ACT:   u32 = 0x24242F;  // Active titlebar (slightly lighter)
const TITLE_LINE:  u32 = 0x3C3C48;  // Separator line
const CONTENT_BG:  u32 = 0x1A1A24;  // Content background (matches main BG)
const TXT_DIM:     u32 = 0x6B7280;  // Inactive text
const TXT_ACT:     u32 = 0xF5F5F7;  // Active text (warm white)
const BTN_CLOSE:   u32 = 0xFF6B6B;  // Red close button
const BTN_MIN:     u32 = 0xFFD43B;  // Yellow minimize button
const BTN_MAX:     u32 = 0x51CF66;  // Green maximize button

// Traffic-light buttons on the RIGHT side of the titlebar.
const BTN_R:      i32 = 5;   // radius in pixels
const BTN_GAP:    i32 = 14;  // center-to-center spacing
const BTN_MARGIN: i32 = 12;  // distance from window right edge to close button center

// Minimum window size — can't resize below the default terminal dimensions.
pub const WIN_MIN_W: i32 = WINDOW_W;
pub const WIN_MIN_H: i32 = WINDOW_H;

pub struct Window {
    pub x: i32, pub y: i32,
    pub w: i32, pub h: i32,
    pub title: String,
    pub dragging: bool,
    pub drag_ox: i32, pub drag_oy: i32,
    pub resizing: bool,
    pub resize_mx: i32, pub resize_my: i32,  // mouse pos when resize started
    pub resize_ow: i32, pub resize_oh: i32,  // window size when resize started
    pub minimized: bool,
    pub maximized: bool,
    pub restore_x: i32, pub restore_y: i32,
    pub restore_w: i32, pub restore_h: i32,
}

impl Window {
    pub fn new(x: i32, y: i32, title: &str) -> Self {
        Window {
            x, y, w: WINDOW_W, h: WINDOW_H,
            title: title.to_string(),
            dragging: false, drag_ox: 0, drag_oy: 0,
            resizing: false, resize_mx: 0, resize_my: 0,
            resize_ow: WINDOW_W, resize_oh: WINDOW_H,
            minimized: false, maximized: false,
            restore_x: x, restore_y: y,
            restore_w: WINDOW_W, restore_h: WINDOW_H,
        }
    }

    pub fn toggle_maximize(&mut self, sw: i32, sh: i32, topbar_h: i32) {
        if self.maximized {
            self.x = self.restore_x; self.y = self.restore_y;
            self.w = self.restore_w; self.h = self.restore_h;
            self.maximized = false;
        } else {
            self.restore_x = self.x; self.restore_y = self.y;
            self.restore_w = self.w; self.restore_h = self.h;
            self.x = 0; self.y = topbar_h;
            self.w = sw; self.h = sh - 28 - topbar_h;
            self.maximized = true;
        }
    }
}

fn btn_cy(win: &Window) -> i32   { win.y + TITLEBAR_H / 2 }
fn close_cx(win: &Window) -> i32 { win.x + win.w - BTN_MARGIN }
fn min_cx  (win: &Window) -> i32 { win.x + win.w - BTN_MARGIN - BTN_GAP }
fn max_cx  (win: &Window) -> i32 { win.x + win.w - BTN_MARGIN - BTN_GAP * 2 }

fn hit_btn(mx: i32, my: i32, cx: i32, cy: i32) -> bool {
    let dx = mx - cx; let dy = my - cy;
    dx * dx + dy * dy <= (BTN_R + 1) * (BTN_R + 1)
}

pub fn close_btn_hit(win: &Window, mx: i32, my: i32) -> bool { hit_btn(mx, my, close_cx(win), btn_cy(win)) }
pub fn min_btn_hit  (win: &Window, mx: i32, my: i32) -> bool { hit_btn(mx, my, min_cx(win),   btn_cy(win)) }
pub fn max_btn_hit  (win: &Window, mx: i32, my: i32) -> bool { hit_btn(mx, my, max_cx(win),   btn_cy(win)) }

pub fn titlebar_hit(win: &Window, mx: i32, my: i32) -> bool {
    mx >= win.x && mx < win.x + win.w
        && my >= win.y && my < win.y + TITLEBAR_H
        && !close_btn_hit(win, mx, my)
        && !min_btn_hit(win, mx, my)
        && !max_btn_hit(win, mx, my)
}

pub fn window_hit(win: &Window, mx: i32, my: i32) -> bool {
    !win.minimized
        && mx >= win.x && mx < win.x + win.w
        && my >= win.y && my < win.y + win.h
}

// Bottom-right 12×12 corner for resize drag.
pub fn resize_corner_hit(win: &Window, mx: i32, my: i32) -> bool {
    !win.minimized && !win.maximized
        && mx >= win.x + win.w - 12 && mx < win.x + win.w
        && my >= win.y + win.h - 12 && my < win.y + win.h
}

pub fn content_origin(win: &Window) -> (i32, i32) {
    (win.x + 1, win.y + 1 + TITLEBAR_H)
}

fn darken(c: u32) -> u32 {
    ((((c >> 16) & 0xFF) * 2 / 3) << 16)
  | ((((c >>  8) & 0xFF) * 2 / 3) << 8)
  |   (((c       & 0xFF) * 2 / 3))
}

fn draw_btn(fb: &mut Framebuffer, cx: i32, cy: i32, color: u32) {
    if cx < BTN_R + 2 || cy < BTN_R + 2 { return; }
    fb.fill_circle(cx, cy, BTN_R + 1, darken(color));
    fb.fill_circle(cx, cy, BTN_R,     color);
    // Soft highlight dot — top-left quadrant for 3D sphere look
    let hi = (((color >> 16 & 0xFF).saturating_add(0x50).min(0xFF)) << 16)
           | (((color >>  8 & 0xFF).saturating_add(0x50).min(0xFF)) << 8)
           |   (color       & 0xFF).saturating_add(0x50).min(0xFF);
    if cx >= 3 && cy >= 3 {
        fb.set_pixel((cx as u32) - 2, (cy as u32) - 2, hi);
        fb.set_pixel((cx as u32) - 1, (cy as u32) - 2, hi);
        fb.set_pixel((cx as u32) - 2, (cy as u32) - 1, hi);
    }
}

// Horizontal clip from each side for the inner fill (inner_r=7) at the top corner zone.
// Index = row offset dy from y+1. Valid for dy in 0..7; beyond that, no clip.
const TOP_INNER_CLIP: [i32; 7] = [7, 4, 3, 2, 1, 1, 1];

pub fn draw_window(fb: &mut Framebuffer, win: &Window, focused: bool) {
    if win.minimized || win.w <= 0 || win.h <= 0 { return; }

    let x = win.x; let y = win.y; let w = win.w; let h = win.h;

    // Soft shadow — multi-layer for modern depth (Ubuntu-style)
    fb.fill_rounded_rect(x + 6, y + 6, w, h, 8, 0x000000);  // Deep shadow far
    fb.fill_rounded_rect(x + 4, y + 4, w, h, 8, 0x0A0A14);  // Medium shadow
    fb.fill_rounded_rect(x + 2, y + 2, w, h, 8, SHADOW);    // Soft shadow near

    // Outer border (1px) — rounded corners, color signals focus.
    let border = if focused { BORDER_ACT } else { BORDER_DIM };
    fb.fill_rounded_rect(x, y, w, h, 8, border);

    // Titlebar — vertical gradient, rows clipped to respect the rounded top corners.
    // Inner radius = 7 (= outer 8 − 1px border). TOP_INNER_CLIP gives the extra
    // horizontal inset needed for the first 7 rows inside the border.
    let tb_col = if focused { TITLE_ACT } else { TITLE_DIM };
    let tr = (tb_col >> 16 & 0xFF) as u8;
    let tg = (tb_col >>  8 & 0xFF) as u8;
    let tb = (tb_col       & 0xFF) as u8;
    for dy in 0..TITLEBAR_H as u32 {
        let hi = (0x14u32 * (TITLEBAR_H as u32 - 1 - dy) / (TITLEBAR_H as u32 - 1)) as u8;
        let row_col = (tr.saturating_add(hi) as u32) << 16
                    | (tg.saturating_add(hi) as u32) << 8
                    |  tb.saturating_add(hi) as u32;
        let clip = if (dy as usize) < TOP_INNER_CLIP.len() { TOP_INNER_CLIP[dy as usize] } else { 0 };
        let rx = x + 1 + clip;
        let rw = (w - 2 - clip * 2).max(0);
        if rw > 0 { fb.fill_rect_s(rx, y + 1 + dy as i32, rw, 1, row_col); }
    }
    // Bottom edge of titlebar
    fb.fill_rect((x + 1) as u32, (y + 1 + TITLEBAR_H) as u32, (w - 2) as u32, 1, TITLE_LINE);

    // Content area — rounded bottom corners to match the window border.
    let cy2 = y + 2 + TITLEBAR_H;
    let ch  = h - 3 - TITLEBAR_H;
    if ch > 0 {
        fb.fill_rounded_rect(x + 1, cy2, w - 2, ch, 7, CONTENT_BG);
    }

    // Title text
    let right_reserved = BTN_MARGIN + BTN_GAP * 2 + BTN_R + 6;
    let left_reserved  = 6;
    let avail_w = (w - right_reserved - left_reserved).max(0);
    let max_chars = (avail_w / 8).max(0) as usize;
    let n_bytes = max_chars.min(win.title.len());
    let title = &win.title[..n_bytes];
    let title_px_w = title.len() as i32 * 8;
    let title_x = x + left_reserved + (avail_w - title_px_w).max(0) / 2;
    let txt_col = if focused { TXT_ACT } else { TXT_DIM };
    let txt_dy  = (TITLEBAR_H - 8) / 2;
    let txt_hi  = (0x14u32 * (TITLEBAR_H as u32 - 1 - txt_dy as u32) / (TITLEBAR_H as u32 - 1)) as u8;
    let txt_bg  = (tr.saturating_add(txt_hi) as u32) << 16
               | (tg.saturating_add(txt_hi) as u32) << 8
               |  tb.saturating_add(txt_hi) as u32;
    fb.draw_str(title_x as u32, (y + 1 + txt_dy) as u32, title, txt_col, txt_bg);

    // Traffic-light buttons — dim when unfocused (macOS convention).
    let bcy = btn_cy(win);
    if focused {
        draw_btn(fb, close_cx(win), bcy, BTN_CLOSE);
        draw_btn(fb, min_cx(win),   bcy, BTN_MIN);
        draw_btn(fb, max_cx(win),   bcy, BTN_MAX);
    } else {
        draw_btn(fb, close_cx(win), bcy, 0x5C2020);
        draw_btn(fb, min_cx(win),   bcy, 0x5C4008);
        draw_btn(fb, max_cx(win),   bcy, 0x134D20);
    }
}

/// Called AFTER the terminal renders so the grip is drawn on top of any content.
pub fn draw_resize_grip(fb: &mut Framebuffer, win: &Window, focused: bool) {
    if win.minimized || win.maximized || win.w < 20 || win.h < 20 { return; }
    let col = if focused { 0x475569 } else { 0x1E293B };
    // Three 2-pixel dots along the bottom-right diagonal, inside the content area
    let bx = win.x + win.w - 3;
    let by = win.y + win.h - 3;
    for i in 0..3i32 {
        let px = (bx - i * 4) as u32;
        let py = (by - i * 4) as u32;
        if (bx - i * 4) > win.x + 1 && (by - i * 4) > win.y + TITLEBAR_H + 1 {
            fb.set_pixel(px, py, col);
            fb.set_pixel(px.wrapping_sub(1), py, col);
            fb.set_pixel(px, py.wrapping_sub(1), col);
        }
    }
}
