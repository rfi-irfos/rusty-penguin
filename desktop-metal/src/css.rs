// Ternary CSS engine — brick 1.
//
// A pure-Rust, no_std styling engine: it parses a small CSS subset into a
// `Style`, and paints Apple-like styled panels (soft multi-layer shadow,
// rounded corners, hairline highlight + border) to the framebuffer. The goal
// is to move the desktop's look from hardcoded fill_rects to declarative,
// restyleable components — the foundation for an Apple-OS-grade UI and, long
// term, for rendering CSS-styled content.
//
// Ternary: every component carries a `state` Trit:
//   +1 Active   — accent ring / brighter
//    0 Normal   — as styled
//   -1 Disabled — dimmed
//
// Supported declarations (selectors come in a later brick):
//   background:#RRGGBB;  color:#RRGGBB;  border:#RRGGBB;  accent:#RRGGBB;
//   border-width:N;  radius:N;  pad-x:N;  pad-y:N;  shadow:0|1;

use crate::fb::Framebuffer;
use crate::trit::Trit;
use alloc::string::String;
use alloc::vec::Vec;

#[derive(Clone, Copy)]
pub struct Style {
    pub bg: u32,
    pub fg: u32,
    pub border: u32,
    pub border_w: u32,
    pub accent: u32,
    pub radius: u32,
    pub pad_x: u32,
    pub pad_y: u32,
    pub shadow: bool,
}

impl Style {
    /// Sensible Apple-like defaults: dark card, hairline border, soft radius.
    pub const fn new() -> Self {
        Style {
            bg: 0x1C1C1E,      // graphite (close to macOS dark surface)
            fg: 0xF5F5F7,      // near-white label
            border: 0x3A3A3C,  // hairline separator
            border_w: 1,
            accent: 0x0A84FF,  // system blue
            radius: 14,
            pad_x: 16,
            pad_y: 12,
            shadow: true,
        }
    }
}

fn hex(s: &str) -> Option<u32> {
    let s = s.trim().trim_start_matches('#');
    if s.len() != 6 { return None; }
    u32::from_str_radix(s, 16).ok()
}

fn num(s: &str) -> Option<u32> {
    s.trim().trim_end_matches("px").trim().parse::<u32>().ok()
}

/// Parse a CSS-subset declaration block (`prop: value; …`) onto a base Style.
pub fn parse_onto(mut base: Style, css: &str) -> Style {
    for decl in css.split(';') {
        let mut it = decl.splitn(2, ':');
        let key = match it.next() { Some(k) => k.trim(), None => continue };
        let val = match it.next() { Some(v) => v.trim(), None => continue };
        if key.is_empty() || val.is_empty() { continue; }
        match key {
            "background" | "background-color" => if let Some(c) = hex(val) { base.bg = c; },
            "color"                           => if let Some(c) = hex(val) { base.fg = c; },
            "border" | "border-color"         => if let Some(c) = hex(val) { base.border = c; },
            "accent" | "accent-color"         => if let Some(c) = hex(val) { base.accent = c; },
            "border-width"                    => if let Some(n) = num(val) { base.border_w = n; },
            "radius" | "border-radius"        => if let Some(n) = num(val) { base.radius = n; },
            "pad-x" | "padding-x"             => if let Some(n) = num(val) { base.pad_x = n; },
            "pad-y" | "padding-y"             => if let Some(n) = num(val) { base.pad_y = n; },
            "shadow" | "box-shadow"           => base.shadow = val != "0" && val != "none",
            _ => {}
        }
    }
    base
}

/// Parse a declaration block onto the default style.
pub fn parse(css: &str) -> Style { parse_onto(Style::new(), css) }

fn dim(c: u32, shift: u32) -> u32 { (c >> shift) & (0xFFFFFF >> shift) }

/// Paint an Apple-like styled panel. Returns the inner content rect
/// (x, y, w, h) after padding, so callers can place content inside.
pub fn paint_panel(fb: &mut Framebuffer, x: i32, y: i32, w: i32, h: i32,
                   style: &Style, state: Trit) -> (i32, i32, i32, i32) {
    let r = style.radius as i32;
    let (bg, border, fg_ring) = match state {
        Trit::Pos  => (style.bg, style.accent, true),         // active: accent ring
        Trit::Zero => (style.bg, style.border, false),        // normal
        Trit::Neg  => (dim(style.bg, 1), dim(style.border, 1), false), // disabled: dimmed
    };

    // Soft multi-layer drop shadow (macOS-style): a few translucent-ish darker
    // rounded rects offset down/right with decreasing spread.
    if style.shadow {
        fb.fill_rounded_rect(x - 1, y + 6, w + 2, h, r + 2, 0x05050A);
        fb.fill_rounded_rect(x,     y + 3, w,     h, r + 1, 0x0A0A12);
    }

    // Border first, then inset background → a crisp hairline edge.
    let bw = style.border_w.max(1) as i32;
    fb.fill_rounded_rect(x, y, w, h, r, border);
    fb.fill_rounded_rect(x + bw, y + bw, w - 2 * bw, h - 2 * bw, (r - bw).max(0), bg);

    // Subtle top highlight (the macOS "light from above" sheen).
    fb.fill_rect_s(x + r, y + bw, w - 2 * r, 1, 0x2C2C2E);

    // Accent ring when active.
    if fg_ring {
        fb.fill_rounded_rect(x, y, w, h, r, style.accent);
        fb.fill_rounded_rect(x + 2, y + 2, w - 4, h - 4, (r - 2).max(0), bg);
        fb.fill_rect_s(x + r, y + 2, w - 2 * r, 1, 0x2C2C2E);
    }

    (x + style.pad_x as i32, y + style.pad_y as i32,
     w - 2 * style.pad_x as i32, h - 2 * style.pad_y as i32)
}

/// Convenience: a styled panel from a CSS string, returning its inner rect.
pub fn panel_css(fb: &mut Framebuffer, x: i32, y: i32, w: i32, h: i32,
                 css: &str, state: Trit) -> (i32, i32, i32, i32) {
    let style = parse(css);
    paint_panel(fb, x, y, w, h, &style, state)
}

/// A parsed stylesheet: selector → Style rules. Supports the CSS subset
/// `.selector { decl; decl; } .other { … }`. Selectors are simple names
/// (class-like); cascading/specificity comes in a later brick.
pub struct StyleSheet {
    rules: Vec<(String, Style)>,
}

impl StyleSheet {
    /// Parse `.name { props } .name2 { props }` into selector→Style rules.
    pub fn parse(css: &str) -> Self {
        let mut rules = Vec::new();
        let bytes = css;
        let mut rest = bytes;
        while let Some(open) = rest.find('{') {
            let selector = rest[..open].trim().trim_start_matches('.').trim();
            let after = &rest[open + 1..];
            let close = match after.find('}') { Some(c) => c, None => break };
            let block = &after[..close];
            if !selector.is_empty() {
                rules.push((String::from(selector), parse(block)));
            }
            rest = &after[close + 1..];
        }
        StyleSheet { rules }
    }

    /// Look up a selector's Style (name with or without leading `.`),
    /// falling back to the engine default if absent.
    pub fn get(&self, selector: &str) -> Style {
        let name = selector.trim_start_matches('.');
        for (sel, style) in &self.rules {
            if sel == name { return *style; }
        }
        Style::new()
    }
}
