use alloc::string::{String, ToString};
use crate::fb::Framebuffer;
use crate::term;

pub const TITLEBAR_H: i32 = 22;
pub const WINDOW_W:   i32 = term::TERM_PIX_W as i32 + 2;
pub const WINDOW_H:   i32 = term::TERM_PIX_H as i32 + 2 + TITLEBAR_H;

const SHADOW:      u32 = 0x06101E;
const BORDER_DIM:  u32 = 0x334155;
const BORDER_ACT:  u32 = 0x60A5FA;
const TITLE_DIM:   u32 = 0x1A2535;
const TITLE_ACT:   u32 = 0x1E293B;
const TITLE_LINE:  u32 = 0x334155;
const CONTENT_BG:  u32 = 0x0F172A;
const TXT_DIM:     u32 = 0x64748B;
const TXT_ACT:     u32 = 0xE2E8F0;
const BTN_CLOSE:   u32 = 0xEF4444;
const BTN_MIN:     u32 = 0xF59E0B;
const BTN_MAX:     u32 = 0x22C55E;

// Traffic-light buttons: circles on the RIGHT side of the titlebar (Windows/Mint style).
const BTN_R:      i32 = 5;   // radius in pixels
const BTN_GAP:    i32 = 14;  // center-to-center spacing
const BTN_MARGIN: i32 = 12;  // distance from window right edge to close button center

pub struct Window {
    pub x: i32, pub y: i32,
    pub w: i32, pub h: i32,
    pub title: String,
    pub dragging: bool,
    pub drag_ox: i32, pub drag_oy: i32,
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
}

pub fn draw_window(fb: &mut Framebuffer, win: &Window, focused: bool) {
    if win.minimized || win.w <= 0 || win.h <= 0 { return; }

    let x = win.x; let y = win.y; let w = win.w; let h = win.h;

    // Drop shadow
    if x + 5 >= 0 && y + 5 >= 0 {
        fb.fill_rect((x + 4) as u32, (y + 4) as u32, w as u32, h as u32, SHADOW);
    }

    // Outer border (1px) — color signals focus
    let border = if focused { BORDER_ACT } else { BORDER_DIM };
    fb.fill_rect(x as u32, y as u32, w as u32, h as u32, border);

    // Titlebar
    let tb_col = if focused { TITLE_ACT } else { TITLE_DIM };
    fb.fill_rect((x + 1) as u32, (y + 1) as u32, (w - 2) as u32, TITLEBAR_H as u32, tb_col);
    // Subtle top-row highlight for depth
    let hi = (((tb_col >> 16 & 0xFF).saturating_add(0x10)) << 16)
           | (((tb_col >>  8 & 0xFF).saturating_add(0x10)) << 8)
           |   (tb_col       & 0xFF).saturating_add(0x10);
    fb.fill_rect((x + 1) as u32, (y + 1) as u32, (w - 2) as u32, 3, hi);
    // Bottom edge of titlebar
    fb.fill_rect((x + 1) as u32, (y + 1 + TITLEBAR_H) as u32, (w - 2) as u32, 1, TITLE_LINE);

    // Content area
    let cy2 = y + 2 + TITLEBAR_H;
    let ch  = h - 3 - TITLEBAR_H;
    if ch > 0 {
        fb.fill_rect((x + 1) as u32, cy2 as u32, (w - 2) as u32, ch as u32, CONTENT_BG);
    }

    // Title text — centered in the space left of the buttons
    let right_reserved = BTN_MARGIN + BTN_GAP * 2 + BTN_R + 6; // ~49px
    let left_reserved  = 6;
    let avail_w = (w - right_reserved - left_reserved).max(0);
    let max_chars = (avail_w / 8).max(0) as usize;
    let n_bytes = max_chars.min(win.title.len());
    let title = &win.title[..n_bytes];
    let title_px_w = title.len() as i32 * 8;
    let title_x = x + left_reserved + (avail_w - title_px_w).max(0) / 2;
    let txt_col = if focused { TXT_ACT } else { TXT_DIM };
    let txt_y   = y + 1 + (TITLEBAR_H - 8) / 2;
    fb.draw_str(title_x as u32, txt_y as u32, title, txt_col, tb_col);

    // Traffic-light buttons (left side)
    let bcy = btn_cy(win);
    draw_btn(fb, close_cx(win), bcy, BTN_CLOSE);
    draw_btn(fb, min_cx(win),   bcy, BTN_MIN);
    draw_btn(fb, max_cx(win),   bcy, BTN_MAX);
}
