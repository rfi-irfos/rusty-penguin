use alloc::string::{String, ToString};
use crate::fb::Framebuffer;
use crate::term;

pub const TITLEBAR_H: i32 = 34;
pub const WINDOW_W:   i32 = term::TERM_PIX_W as i32 + 2;
pub const WINDOW_H:   i32 = term::TERM_PIX_H as i32 + 2 + TITLEBAR_H;

// Window styling — "Rusty Penguin v2" warm-stone-green palette, adopted from
// Simeon's HTML design mockup (rusty-penguin-os.html): warm dark stone (not cool
// graphite), spring-green accent, gold/cream highlights, ternary neg/zero/pos.
const SHADOW:      u32 = 0x080B09;  // Deep shadow (warm black)
const BORDER_DIM:  u32 = 0x2A332F;  // Inactive window border (warm hairline)
const BORDER_ACT:  u32 = 0x5A6A5E;  // Active window border (warm light edge)
const TITLE_DIM:   u32 = 0x252E2A;  // Inactive titlebar (warm stone)
const TITLE_ACT:   u32 = 0x323C37;  // Active titlebar (panel-soft)
const TITLE_LINE:  u32 = 0x3C4641;  // Separator hairline
const CONTENT_BG:  u32 = 0x222B27;  // Content background (warm stone glass-over-wall)
const TXT_DIM:     u32 = 0xA8B0A6;  // Secondary label (warm grey-green)
const TXT_ACT:     u32 = 0xECEDE5;  // Primary text (warm off-white)
const BTN_CLOSE:   u32 = 0xEF7575;  // Red (--neg)
const BTN_MIN:     u32 = 0xF5C451;  // Amber
const BTN_MAX:     u32 = 0x6FE18B;  // Spring green (--pos)
// Shared accent + ternary triad (mockup tokens) for the rest of the desktop.
pub const ACCENT_GREEN: u32 = 0x6FE18B;  // --green / --pos
pub const ACCENT_CREAM: u32 = 0xECDAA7;  // --cream (dingir gold)
pub const TRIT_NEG:     u32 = 0xEF7575;  // --neg
pub const TRIT_ZERO:    u32 = 0x909A92;  // --zero
pub const TRIT_POS:     u32 = 0x6FE18B;  // --pos

// Traffic-light buttons on the RIGHT side of the titlebar.
const BTN_R:      i32 = 6;   // radius in pixels
const BTN_GAP:    i32 = 17;  // center-to-center spacing
const BTN_MARGIN: i32 = 16;  // distance from window right edge to close button center

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

// Bottom-right corner for resize drag. 20×20 hit zone so it's actually
// grabbable with a mouse — the visible grip is still small (drawn by
// draw_resize_grip) but the click area extends inward.
pub fn resize_corner_hit(win: &Window, mx: i32, my: i32) -> bool {
    !win.minimized && !win.maximized
        && mx >= win.x + win.w - 20 && mx < win.x + win.w
        && my >= win.y + win.h - 20 && my < win.y + win.h
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

    // ── Aero-style atmospheric shadow — large, soft, diffused, suspended look.
    // Outer bloom (wide, near-transparent): window appears to float above the desk.
    // Inner layers tighten into the classic drop shadow.
    fb.fill_rounded_rect(x + 12, y + 16, w - 4, h, 14, 0x07090A); // atmosphere bloom
    fb.fill_rounded_rect(x +  8, y + 10, w - 2, h, 12, 0x060808); // mid bloom
    fb.fill_rounded_rect(x +  4, y +  6, w,     h, 10, 0x0A0F0D); // near shadow
    fb.fill_rounded_rect(x +  2, y +  3, w,     h,  9, SHADOW);   // crisp drop

    // ── Outer border — focused gets a warm cream-tinted top edge (Aero "light catch").
    let border = if focused { BORDER_ACT } else { BORDER_DIM };
    fb.fill_rounded_rect(x, y, w, h, 8, border);
    // Top-edge light catch: a 1-px highlight simulating overhead illumination.
    // Focused = cream-warm, background = barely visible.
    let top_light = if focused { 0x7A8A7E } else { 0x3A4540 };
    fb.fill_rect_s(x + 8, y, w - 16, 1, top_light);

    // ── Titlebar — frosted glass with a top-to-bottom gradient so it reads as a
    // lit physical surface, not a flat fill. Top rows are brightened, bottom
    // rows darkened, blended over the wallpaper showing through.
    let glass_alpha = if focused { 232u32 } else { 188u32 };
    let tb_col      = if focused { TITLE_ACT } else { TITLE_DIM };
    fb.fill_rounded_rect_glass(x + 1, y + 1, w - 2, TITLEBAR_H, 7, tb_col, glass_alpha);
    // gradient overlay: lighten the upper third, darken the lower third
    let lighten = |c: u32, d: u32| {
        (((c >> 16 & 0xFF).saturating_add(d).min(0xFF)) << 16)
      | (((c >>  8 & 0xFF).saturating_add(d).min(0xFF)) << 8)
      |  ((c       & 0xFF).saturating_add(d).min(0xFF))
    };
    for row in 0..(TITLEBAR_H / 2) {
        let d = (10 - row).max(0) as u32 * 2; // fade out over ~10px
        if d > 0 { fb.fill_rect_s(x + 2, y + 1 + row, w - 4, 1, lighten(tb_col, d)); }
    }

    // Aero "inner glow" — a faint warm-green luminance line just inside the top.
    if focused {
        fb.fill_rect_s(x + 8, y + 1, w - 16, 1, 0x4A5D50);
    }

    // Bottom edge of titlebar (hairline separator)
    fb.fill_rect((x + 1) as u32, (y + 1 + TITLEBAR_H) as u32, (w - 2) as u32, 1, TITLE_LINE);

    // ── Content area — frosted glass for focused, solid for background (depth).
    let cy2 = y + 2 + TITLEBAR_H;
    let ch  = h - 3 - TITLEBAR_H;
    if ch > 0 {
        if focused {
            fb.fill_rounded_rect_glass(x + 1, cy2, w - 2, ch, 7, CONTENT_BG, 220);
        } else {
            fb.fill_rounded_rect(x + 1, cy2, w - 2, ch, 7, CONTENT_BG);
        }
    }

    // ── App mark (dingir) on the left + left-aligned title, like the mockup's
    // window bar (icon + title). The mark is the green brand star; the title
    // sits just to its right and vertically centered in the taller bar.
    let mark_cx = x + 16;
    let mark_cy = y + 1 + TITLEBAR_H / 2;
    let mark_col = if focused { ACCENT_GREEN } else { TRIT_ZERO };
    fb.draw_star8(mark_cx, mark_cy, 7, mark_col);

    let title_x = x + 30;
    let right_reserved = BTN_MARGIN + BTN_GAP * 2 + BTN_R + 8;
    let avail_w = (x + w - right_reserved - title_x).max(0);
    let max_chars = (avail_w / 8).max(0) as usize;
    let n_bytes = max_chars.min(win.title.len());
    let title = &win.title[..n_bytes];
    let txt_col = if focused { TXT_ACT } else { TXT_DIM };
    fb.draw_aa(title_x, y + 9, title, txt_col, crate::fb::AA_S);

    // ── Traffic-light buttons — dim when unfocused (Aero z-hierarchy convention).
    let bcy = btn_cy(win);
    if focused {
        draw_btn(fb, close_cx(win), bcy, BTN_CLOSE);
        draw_btn(fb, min_cx(win),   bcy, BTN_MIN);
        draw_btn(fb, max_cx(win),   bcy, BTN_MAX);
    } else {
        draw_btn(fb, close_cx(win), bcy, 0x4A2020);
        draw_btn(fb, min_cx(win),   bcy, 0x4A3808);
        draw_btn(fb, max_cx(win),   bcy, 0x0F3A18);
    }
}

/// Called AFTER the terminal renders so the grip is drawn on top of any content.
pub fn draw_resize_grip(fb: &mut Framebuffer, win: &Window, focused: bool) {
    if win.minimized || win.maximized || win.w < 20 || win.h < 20 { return; }
    let col = if focused { 0x6B7280 } else { 0x2C2C38 };
    // Diagonal striped grip in the bottom-right corner. Six 3-pixel ticks
    // along the anti-diagonal makes the affordance visible without
    // overpowering the window content.
    let bx = win.x + win.w - 2;
    let by = win.y + win.h - 2;
    for i in 0..6i32 {
        let off = i * 3;
        if (bx - off) <= win.x + 1 || (by - off) <= win.y + TITLEBAR_H + 1 { break; }
        // Each tick is a 3-pixel anti-diagonal segment.
        for k in 0..3i32 {
            let px = bx - off + k;
            let py = by - off - k;
            if px >= 0 && py >= 0 && px < win.x + win.w && py < win.y + win.h {
                fb.set_pixel(px as u32, py as u32, col);
            }
        }
    }
}
