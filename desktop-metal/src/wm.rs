use alloc::string::{String, ToString};
use crate::fb::Framebuffer;
use crate::term;

pub const TITLEBAR_H: i32 = 34;
pub const WINDOW_W:   i32 = term::TERM_PIX_W as i32 + 2;
pub const WINDOW_H:   i32 = term::TERM_PIX_H as i32 + 2 + TITLEBAR_H;

// Window styling — darker graphite, brushed-metal panels, and restrained teal /
// amber accents so the desktop reads more like a premium automotive UI.
const SHADOW:      u32 = 0x050708;  // Deep shadow
const BORDER_DIM:  u32 = 0x242A2D;  // Inactive window border
const BORDER_ACT:  u32 = 0x5C6B6B;  // Active window border
const TITLE_DIM:   u32 = 0x1F252A;  // Inactive titlebar
const TITLE_ACT:   u32 = 0x2B333A;  // Active titlebar
const TITLE_LINE:  u32 = 0x3A444A;  // Separator hairline
const CONTENT_BG:  u32 = 0x171C20;  // Content background
const TXT_DIM:     u32 = 0xA4ADB2;  // Secondary label
const TXT_ACT:     u32 = 0xF0F3F5;  // Primary text
const BTN_CLOSE:   u32 = 0xE66D73;  // Red (--neg)
const BTN_MIN:     u32 = 0xD8B05F;  // Amber
const BTN_MAX:     u32 = 0x63C7AD;  // Teal (--pos)
// Shared accent + ternary triad (mockup tokens) for the rest of the desktop.
pub const ACCENT_GREEN: u32 = 0x63C7AD;  // --green / --pos
pub const ACCENT_CREAM: u32 = 0xE8D39B;  // --cream / warm gold
pub const TRIT_NEG:     u32 = 0xE66D73;  // --neg
pub const TRIT_ZERO:    u32 = 0x909AA0;  // --zero
pub const TRIT_POS:     u32 = 0x63C7AD;  // --pos

// Ubuntu/Mint-style flat rectangular window buttons on the RIGHT side.
// Standard theme. Toggle via Settings → Window buttons → Classic (macOS-style).
const BTN_W:      i32 = 24;  // button width
const BTN_H:      i32 = 20;  // button height
const BTN_GAP:    i32 = 4;   // gap between buttons
const BTN_MARGIN: i32 = 8;   // gap from window right edge to close button right edge

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

// Left edge of each button (close is rightmost).
fn close_bx(win: &Window) -> i32 { win.x + win.w - BTN_MARGIN - BTN_W }
fn max_bx  (win: &Window) -> i32 { close_bx(win) - BTN_GAP - BTN_W }
fn min_bx  (win: &Window) -> i32 { max_bx(win)   - BTN_GAP - BTN_W }
fn btn_by  (win: &Window) -> i32 { win.y + (TITLEBAR_H - BTN_H) / 2 }

fn hit_btn_rect(mx: i32, my: i32, bx: i32, by: i32) -> bool {
    mx >= bx && mx < bx + BTN_W && my >= by && my < by + BTN_H
}

pub fn close_btn_hit(win: &Window, mx: i32, my: i32) -> bool { hit_btn_rect(mx, my, close_bx(win), btn_by(win)) }
pub fn min_btn_hit  (win: &Window, mx: i32, my: i32) -> bool { hit_btn_rect(mx, my, min_bx(win),   btn_by(win)) }
pub fn max_btn_hit  (win: &Window, mx: i32, my: i32) -> bool { hit_btn_rect(mx, my, max_bx(win),   btn_by(win)) }

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

// Ubuntu/Mint-style flat window button.
// kind: 0=close (×), 1=min (−), 2=max (□)
fn draw_btn_ubuntu(fb: &mut Framebuffer, bx: i32, by: i32, kind: u8, focused: bool) {
    // Background: close gets a subtle red tint; min/max stay neutral and metallic.
    let bg = match (kind, focused) {
        (0, true)  => 0x4A2828,  // close focused: dark crimson
        (0, false) => 0x2A1A1A,  // close unfocused: near-invisible dark red
        (_, true)  => 0x343D43,  // min/max focused: dark metal
        _          => 0x262C31,  // min/max unfocused: very dim
    };
    fb.fill_rounded_rect(bx, by, BTN_W, BTN_H, 4, bg);
    // 1-px inner top-edge sheen for depth.
    let sheen = if focused { 0x66737A } else { 0x323840 };
    fb.fill_rect_s(bx + 4, by + 1, BTN_W - 8, 1, sheen);

    let icon_col = if focused { 0xE9EEF0 } else { 0x3D454B };
    let cx = bx + BTN_W / 2;
    let cy = by + BTN_H / 2;
    match kind {
        0 => {
            // × close icon — 5-pixel diagonal cross
            let sp = |fb: &mut Framebuffer, x: i32, y: i32, c: u32| {
                if x >= 0 && y >= 0 { fb.set_pixel(x as u32, y as u32, c); }
            };
            for d in -3i32..=3 {
                sp(fb, cx + d, cy + d, icon_col);
                sp(fb, cx + d, cy - d, icon_col);
                sp(fb, cx + d + 1, cy + d, icon_col);
                sp(fb, cx + d + 1, cy - d, icon_col);
            }
        }
        1 => {
            // − minimize icon — thin horizontal bar
            fb.fill_rect_s(cx - 4, cy,     9, 1, icon_col);
            fb.fill_rect_s(cx - 4, cy + 1, 9, 1, icon_col);
        }
        _ => {
            // □ maximize icon — thin square outline
            fb.fill_rect_s(cx - 4, cy - 4, 9, 1, icon_col);  // top
            fb.fill_rect_s(cx - 4, cy + 4, 9, 1, icon_col);  // bottom
            fb.fill_rect_s(cx - 4, cy - 4, 1, 9, icon_col);  // left
            fb.fill_rect_s(cx + 4, cy - 4, 1, 9, icon_col);  // right
            // double-line top for the title-bar convention
            fb.fill_rect_s(cx - 4, cy - 3, 9, 1, icon_col);
        }
    }
}

// Horizontal clip from each side for the inner fill (inner_r=7) at the top corner zone.
// Index = row offset dy from y+1. Valid for dy in 0..7; beyond that, no clip.
const TOP_INNER_CLIP: [i32; 7] = [7, 4, 3, 2, 1, 1, 1];

pub fn draw_window(fb: &mut Framebuffer, win: &Window, focused: bool) {
    if win.minimized || win.w <= 0 || win.h <= 0 { return; }

    let x = win.x; let y = win.y; let w = win.w; let h = win.h;

    // ── Atmospheric shadow — large, soft, diffused, suspended look.
    // Outer bloom (wide, near-transparent): window appears to float above the desk.
    // Inner layers tighten into the classic drop shadow.
    fb.fill_rounded_rect(x + 12, y + 16, w - 4, h, 14, 0x07090A); // atmosphere bloom
    fb.fill_rounded_rect(x +  8, y + 10, w - 2, h, 12, 0x060808); // mid bloom
    fb.fill_rounded_rect(x +  4, y +  6, w,     h, 10, 0x0A0F0D); // near shadow
    fb.fill_rounded_rect(x +  2, y +  3, w,     h,  9, SHADOW);   // crisp drop

    // ── Outer border — focused gets a cool light catch along the top edge.
    let border = if focused { BORDER_ACT } else { BORDER_DIM };
    fb.fill_rounded_rect(x, y, w, h, 8, border);
    // Top-edge light catch: a 1-px highlight simulating overhead illumination.
    // Focused = cream-warm, background = barely visible.
    let top_light = if focused { 0x748287 } else { 0x3A4548 };
    fb.fill_rect_s(x + 8, y, w - 16, 1, top_light);

    // ── Titlebar — frosted glass with a top-to-bottom gradient so it reads as a
    // lit physical surface, not a flat fill.
    let glass_alpha = if focused { 232u32 } else { 188u32 };
    let tb_col      = if focused { TITLE_ACT } else { TITLE_DIM };
    fb.fill_rounded_rect_glass(x + 1, y + 1, w - 2, TITLEBAR_H, 7, tb_col, glass_alpha);
    // Gradient overlay: lighten the upper third, darken the lower third.
    let lighten = |c: u32, d: u32| {
        (((c >> 16 & 0xFF).saturating_add(d).min(0xFF)) << 16)
      | (((c >>  8 & 0xFF).saturating_add(d).min(0xFF)) << 8)
      |  ((c       & 0xFF).saturating_add(d).min(0xFF))
    };
    for row in 0..(TITLEBAR_H / 2) {
        let d = (10 - row).max(0) as u32 * 2; // fade out over ~10px
        if d > 0 { fb.fill_rect_s(x + 2, y + 1 + row, w - 4, 1, lighten(tb_col, d)); }
    }

    // Inner glow — a faint cool luminance line just inside the top.
    if focused {
        fb.fill_rect_s(x + 8, y + 1, w - 16, 1, 0x4D5B61);
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
    let right_reserved = BTN_MARGIN + (BTN_W + BTN_GAP) * 3 + 4;
    let avail_w = (x + w - right_reserved - title_x).max(0);
    let max_chars = (avail_w / 8).max(0) as usize;
    let n_bytes = max_chars.min(win.title.len());
    let title = &win.title[..n_bytes];
    let txt_col = if focused { TXT_ACT } else { TXT_DIM };
    fb.draw_aa(title_x, y + 9, title, txt_col, crate::fb::AA_S);

    // ── Ubuntu/Mint-style flat window buttons — right-aligned: min · max · close.
    let by2 = btn_by(win);
    draw_btn_ubuntu(fb, min_bx(win),   by2, 1, focused);
    draw_btn_ubuntu(fb, max_bx(win),   by2, 2, focused);
    draw_btn_ubuntu(fb, close_bx(win), by2, 0, focused);
}

/// Called AFTER the terminal renders so the grip is drawn on top of any content.
pub fn draw_resize_grip(fb: &mut Framebuffer, win: &Window, focused: bool) {
    if win.minimized || win.maximized || win.w < 20 || win.h < 20 { return; }
    let col = if focused { 0x7A848B } else { 0x313840 };
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
