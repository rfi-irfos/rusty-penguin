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
            bg: 0x222B27,      // warm stone surface (mockup --panel-solid-ish)
            fg: 0xECEDE5,      // warm off-white label (--txt)
            border: 0x3C4641,  // warm hairline separator
            border_w: 1,
            accent: 0x6FE18B,  // spring green (--green / --pos)
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

    // Soft multi-layer drop shadow: darker rounded rects offset down with
    // decreasing spread (warm-black, matching the mockup's layered shadow).
    if style.shadow {
        fb.fill_rounded_rect(x - 1, y + 7, w + 2, h, r + 3, 0x070A08);
        fb.fill_rounded_rect(x,     y + 3, w,     h, r + 1, 0x0C110E);
    }

    // Frosted-glass body: alpha-blend the panel color over the wallpaper behind
    // it (the mockup's translucent `--panel` look). The wallpaper's green glow
    // tints through subtly. Sheen line at top = "light from above".
    let glass = 205; // ~0.80 opacity
    if fg_ring {
        // Focused: crisp accent ring, frosted interior.
        fb.fill_rounded_rect(x, y, w, h, r, style.accent);
        fb.fill_rounded_rect_glass(x + 2, y + 2, w - 4, h - 4, (r - 2).max(0), bg, glass);
        fb.fill_rect_s(x + r, y + 3, w - 2 * r, 1, 0x4C564F);
    } else {
        // Normal/dimmed: frosted glass straight over the wallpaper + faint edge.
        fb.fill_rounded_rect_glass(x, y, w, h, r, bg, glass);
        fb.fill_rect_s(x + r, y + 1, w - 2 * r, 1, 0x4C564F);     // top sheen
        let _ = border;                                            // hairline implied by frost edge
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

// ── Ternary Node — component-level invalidation ───────────────────────────
// Each UI component is a Node.  The `dirty` Trit controls the render path:
//   +1 (Pos)  — needs repaint (initial or state-changed)
//    0 (Zero) — clean; paint() is a no-op (the sparse skip)
//   -1 (Neg)  — gone/hidden; paint() is a no-op
//
// This is the CSS-layer embodiment of the ternary sparse-rendering thesis:
// only non-zero nodes perform any work in the layout/paint path.
pub struct Node {
    pub x: i32, pub y: i32, pub w: i32, pub h: i32,
    pub style:  Style,
    pub visual: Trit,  // appearance: +1 active / 0 normal / -1 disabled
    pub dirty:  Trit,  // invalidation: +1 repaint / 0 clean / -1 gone
}

impl Node {
    pub const fn new(x: i32, y: i32, w: i32, h: i32, style: Style) -> Self {
        Node { x, y, w, h, style, visual: Trit::Zero, dirty: Trit::Pos }
    }

    /// Paint this node if dirty (+1), then mark it clean.
    /// Returns true when a paint actually occurred (caller can union damage rect).
    pub fn paint(&mut self, fb: &mut Framebuffer) -> bool {
        match self.dirty {
            Trit::Pos => {
                paint_panel(fb, self.x, self.y, self.w, self.h, &self.style, self.visual);
                self.dirty = Trit::Zero;
                true
            }
            Trit::Zero | Trit::Neg => false,
        }
    }

    /// Force a repaint next frame (idempotent; does not resurrect a Neg node).
    pub fn mark_dirty(&mut self) {
        if self.dirty != Trit::Neg { self.dirty = Trit::Pos; }
    }

    /// Change visual state; auto-dirty if changed.
    pub fn set_visual(&mut self, v: Trit) {
        if self.visual != v { self.visual = v; self.mark_dirty(); }
    }

    /// Move to a new position; auto-dirty if changed.
    pub fn move_to(&mut self, x: i32, y: i32) {
        if self.x != x || self.y != y { self.x = x; self.y = y; self.mark_dirty(); }
    }

    /// Resize; auto-dirty if changed.
    pub fn resize(&mut self, w: i32, h: i32) {
        if self.w != w || self.h != h { self.w = w; self.h = h; self.mark_dirty(); }
    }

    /// Remove from the scene (paint is a no-op until mark_dirty re-activates).
    pub fn hide(&mut self) { self.dirty = Trit::Neg; }
}

// ── FlexRow — one-axis layout primitive ───────────────────────────────────
// A minimal flex layout container: positions N items of fixed width in a
// horizontal strip, optionally center-aligned within a parent rect.
// This replaces hardcoded arithmetic like `MARGIN + i * (TILE + GAP)` with
// a declarative spec — the same logic, but owned by the CSS engine and
// reusable across the dock, start menu, and any future panels.
pub struct FlexRow {
    pub x: i32, pub y: i32,  // top-left of the container
    pub w: i32, pub h: i32,  // container size
    pub gap: i32,             // gap between items
}

impl FlexRow {
    pub const fn new(x: i32, y: i32, w: i32, h: i32, gap: i32) -> Self {
        FlexRow { x, y, w, h, gap }
    }

    /// X position of item `i` in a row of `n` items each `item_w` wide.
    /// Items are left-packed from `self.x`; call `center_x` variant to center.
    pub fn item_x(&self, i: usize, item_w: i32) -> i32 {
        self.x + i as i32 * (item_w + self.gap)
    }

    /// X position of item `i`, with the whole row centered inside self.w.
    pub fn item_x_centered(&self, n: usize, i: usize, item_w: i32) -> i32 {
        if n == 0 { return self.x; }
        let total = n as i32 * item_w + (n as i32 - 1).max(0) * self.gap;
        let start = self.x + (self.w - total).max(0) / 2;
        start + i as i32 * (item_w + self.gap)
    }

    /// Y position to vertically center an item of `item_h` inside self.h.
    pub fn item_y_centered(&self, item_h: i32) -> i32 {
        self.y + (self.h - item_h).max(0) / 2
    }

    /// Returns the (x, y) pair for item `i` of size (item_w, item_h),
    /// centered in both axes within this container.
    pub fn item_rect_centered(&self, n: usize, i: usize, item_w: i32, item_h: i32) -> (i32, i32) {
        (self.item_x_centered(n, i, item_w), self.item_y_centered(item_h))
    }
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
