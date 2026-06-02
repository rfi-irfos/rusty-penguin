// Bare-metal framebuffer via sys_fb_query (nr=6).
// Kernel fills a 24-byte struct: [u64 base][u32 width][u32 height][u32 pitch][u32 bpp]
// Framebuffer is identity-mapped so the returned pointer is usable directly.

extern crate alloc;

use crate::font::FONT;

// AA font size selectors (see draw_aa).
pub const AA_T: u8 = 0;  // tiny — captions, descriptions, section labels, tray
pub const AA_S: u8 = 1;  // body — names, titles, menu items
pub const AA_L: u8 = 2;  // display — hero title, page H1

pub struct Framebuffer {
    pub data:   *mut u8,           // points at the backbuffer when double-buffered
    pub width:  u32,
    pub height: u32,
    pub bpp:    u32,
    pub stride: u32,
    // Double-buffer support. While `back` is Some, all draw ops write to the
    // backbuffer; present() copies it to `real` in one block. This prevents
    // the user from seeing intermediate paint states (e.g. the BLUE border
    // fill flashing through before content fills in).
    real: *mut u8,
    back: alloc::boxed::Box<[u8]>,
    // Cached static background (desktop gradient + logo + icon dock). Recomposite
    // blits this instead of recomputing the gradient row-by-row every frame —
    // the expensive part of dragging a window at 1080p. Invalidated when the
    // static scene changes (icon hover).
    bg_cache: alloc::boxed::Box<[u8]>,
    bg_cached: bool,
    // Software brightness (0..=100). When < 100, present() maps every byte on the
    // way to the real framebuffer through `bright_lut` (a precomputed v*b/100
    // table), dimming the whole display. This is a real, usable brightness control
    // on ANY panel — including those with no ACPI/hardware backlight (and QEMU).
    // brightness == 100 fast-paths to a plain block copy (zero overhead).
    brightness: u8,
    bright_lut: [u8; 256],
}

unsafe impl Send for Framebuffer {}

/// sys_gpu_flush(y, h) (nr=34): present rows [y, y+h) of the GPU backing. The
/// kernel no-ops this unless the desktop is routed through the virtio-gpu, so
/// it is safe (and cheap) to call on every present regardless of display path.
fn gpu_flush(y: u32, h: u32) {
    unsafe {
        core::arch::asm!(
            "syscall",
            in("rax") 34u64,
            in("rdi") y as u64,
            in("rsi") h as u64,
            out("rcx") _,
            out("r11") _,
            options(nostack),
        );
    }
}

fn sys_fb_query_raw() -> (u64, u32, u32, u32, u32) {
    let mut buf = [0u8; 24];
    let _ret: u64;
    unsafe {
        core::arch::asm!(
            "syscall",
            inout("rax") 6u64 => _ret,
            in("rdi") buf.as_mut_ptr(),
            in("rsi") 0u64,
            out("rcx") _,
            out("r11") _,
            options(nostack),
        );
    }
    let base   = u64::from_le_bytes(buf[0..8].try_into().unwrap_or([0u8;8]));
    let width  = u32::from_le_bytes(buf[8..12].try_into().unwrap_or([0u8;4]));
    let height = u32::from_le_bytes(buf[12..16].try_into().unwrap_or([0u8;4]));
    let pitch  = u32::from_le_bytes(buf[16..20].try_into().unwrap_or([0u8;4]));
    let bpp    = u32::from_le_bytes(buf[20..24].try_into().unwrap_or([0u8;4]));
    (base, width, height, pitch, bpp)
}

impl Framebuffer {
    pub fn open() -> Result<Self, &'static str> {
        let (base, width, height, pitch, bpp) = sys_fb_query_raw();
        if base == 0 || width == 0 || height == 0 {
            return Err("framebuffer not available");
        }
        // Allocate a backbuffer the same size as the real framebuffer (pitch
        // is bytes-per-row, so pitch * height covers it exactly). All draw
        // ops will target this; present() flushes to the real FB.
        // Box<[u8]> doesn't reallocate, so the data pointer stays valid for
        // the lifetime of the Framebuffer.
        let size = (pitch * height) as usize;
        let mut back: alloc::boxed::Box<[u8]> = alloc::vec![0u8; size].into_boxed_slice();
        let data_ptr = back.as_mut_ptr();
        let bg_cache: alloc::boxed::Box<[u8]> = alloc::vec![0u8; size].into_boxed_slice();
        Ok(Framebuffer {
            data:   data_ptr,
            width, height, bpp, stride: pitch,
            real:   base as *mut u8,
            back,
            bg_cache,
            bg_cached: false,
            brightness: 100,
            bright_lut: {
                let mut l = [0u8; 256];
                let mut i = 0;
                while i < 256 { l[i] = i as u8; i += 1; }
                l
            },
        })
    }

    /// Set software brightness 0..=100 (clamped to a usable floor so the screen
    /// never goes fully black and strands the user). Recomputes the dimming LUT.
    pub fn set_brightness(&mut self, pct: u8) {
        let b = pct.clamp(15, 100);
        self.brightness = b;
        let bb = b as u32;
        for i in 0..256u32 {
            self.bright_lut[i as usize] = ((i * bb) / 100) as u8;
        }
    }

    pub fn brightness(&self) -> u8 { self.brightness }

    /// Save the current backbuffer as the static-background cache.
    pub fn snapshot_bg(&mut self) {
        self.bg_cache.copy_from_slice(&self.back);
        self.bg_cached = true;
    }

    /// Restore the cached static background into the backbuffer (a fast RAM copy
    /// in place of recomputing the gradient + redrawing the icon dock).
    pub fn restore_bg(&mut self) {
        self.back.copy_from_slice(&self.bg_cache);
    }

    pub fn bg_cached(&self) -> bool { self.bg_cached }
    pub fn invalidate_bg(&mut self) { self.bg_cached = false; }

    /// Erase a rectangle back to the cached static background (dirty-rect
    /// compositing primitive: cheaply clear where a window used to be).
    #[allow(dead_code)]
    pub fn restore_bg_rect(&mut self, x: u32, y: u32, w: u32, h: u32) {
        let stride = self.stride as usize;
        let bpp = (self.bpp / 8) as usize;
        let x0 = (x as usize) * bpp;
        let row_len = ((w as usize) * bpp).min(stride.saturating_sub(x0));
        let y1 = (y + h).min(self.height);
        for row in y..y1 {
            let off = row as usize * stride + x0;
            self.back[off..off + row_len].copy_from_slice(&self.bg_cache[off..off + row_len]);
        }
    }

    /// Present only rows [y0, y1) to the real framebuffer (dirty-rect present:
    /// avoid the full-screen MMIO copy when only a band changed).
    #[allow(dead_code)]
    pub fn present_rows(&mut self, y0: u32, y1: u32) {
        let stride = self.stride as usize;
        let y0u = y0 as usize;
        let y1u = (y1 as usize).min(self.height as usize);
        if y1u <= y0u { return; }
        let off = y0u * stride;
        let len = (y1u - y0u) * stride;
        unsafe {
            if self.brightness >= 100 {
                core::ptr::copy_nonoverlapping(self.back.as_ptr().add(off), self.real.add(off), len);
            } else {
                let src = self.back.as_ptr().add(off);
                let dst = self.real.add(off);
                for i in 0..len { *dst.add(i) = self.bright_lut[*src.add(i) as usize]; }
            }
        }
        // GPU path: when `real` is the virtio-gpu backing (RAM), the copy above
        // is RAM→RAM; this DMA-scans the band out. No-op under the VBE fb.
        gpu_flush(y0, (y1u as u32).saturating_sub(y0));
    }

    /// Copy the backbuffer to the real framebuffer in one block. Call this
    /// exactly once per frame after all draw ops complete; intermediate
    /// paint states never become visible.
    pub fn present(&mut self) {
        let n = self.back.len();
        unsafe {
            if self.brightness >= 100 {
                core::ptr::copy_nonoverlapping(self.back.as_ptr(), self.real, n);
            } else {
                let src = self.back.as_ptr();
                let dst = self.real;
                for i in 0..n { *dst.add(i) = self.bright_lut[*src.add(i) as usize]; }
            }
        }
        gpu_flush(0, self.height);
    }

    #[inline]
    pub fn get_pixel(&self, x: u32, y: u32) -> u32 {
        if x >= self.width || y >= self.height { return 0; }
        let off = (y * self.stride + x * (self.bpp / 8)) as usize;
        unsafe {
            let b = *self.data.add(off)     as u32;
            let g = *self.data.add(off + 1) as u32;
            let r = *self.data.add(off + 2) as u32;
            (r << 16) | (g << 8) | b
        }
    }

    #[inline]
    pub fn set_pixel(&mut self, x: u32, y: u32, color: u32) {
        if x >= self.width || y >= self.height { return; }
        let bpp = self.bpp;
        let off = (y * self.stride + x * (bpp / 8)) as usize;
        let r = ((color >> 16) & 0xFF) as u8;
        let g = ((color >> 8)  & 0xFF) as u8;
        let b = (color         & 0xFF) as u8;
        unsafe {
            match bpp {
                32 => {
                    *self.data.add(off)     = b;
                    *self.data.add(off + 1) = g;
                    *self.data.add(off + 2) = r;
                    *self.data.add(off + 3) = 0xFF;
                }
                24 => {
                    *self.data.add(off)     = b;
                    *self.data.add(off + 1) = g;
                    *self.data.add(off + 2) = r;
                }
                _ => {}
            }
        }
    }

    pub fn fill_rect(&mut self, x: u32, y: u32, w: u32, h: u32, color: u32) {
        let x1 = (x + w).min(self.width);
        let y1 = (y + h).min(self.height);
        let bpp = self.bpp / 8;
        let r = ((color >> 16) & 0xFF) as u8;
        let g = ((color >> 8)  & 0xFF) as u8;
        let b = (color         & 0xFF) as u8;
        let pixel: [u8; 4] = match self.bpp {
            32 => [b, g, r, 0xFF],
            24 => [b, g, r, 0],
            _ => return,
        };
        for py in y..y1 {
            for px in x..x1 {
                let off = (py * self.stride + px * bpp) as usize;
                unsafe {
                    for (i, &byte) in pixel[..bpp as usize].iter().enumerate() {
                        *self.data.add(off + i) = byte;
                    }
                }
            }
        }
    }

    pub fn draw_bitmap_2x(&mut self, x: u32, y: u32, bitmap: &[u8; 8], fg: u32, bg: u32) {
        for row in 0..8u32 {
            let byte = bitmap[row as usize];
            for col in 0..8u32 {
                let color = if (byte >> (7 - col)) & 1 != 0 { fg } else { bg };
                self.set_pixel(x + col * 2,     y + row * 2,     color);
                self.set_pixel(x + col * 2 + 1, y + row * 2,     color);
                self.set_pixel(x + col * 2,     y + row * 2 + 1, color);
                self.set_pixel(x + col * 2 + 1, y + row * 2 + 1, color);
            }
        }
    }

    pub fn draw_char(&mut self, x: u32, y: u32, ch: char, fg: u32, bg: u32) {
        let idx = (ch as u32).wrapping_sub(0x20);
        if idx >= 95 { return; }
        let bitmap = &FONT[idx as usize];
        for row in 0..8u32 {
            let byte = bitmap[row as usize];
            for col in 0..8u32 {
                let bit = (byte >> (7 - col)) & 1;
                self.set_pixel(x + col, y + row, if bit != 0 { fg } else { bg });
            }
        }
    }

    pub fn draw_str(&mut self, x: u32, y: u32, s: &str, fg: u32, bg: u32) {
        for (i, ch) in s.chars().enumerate() {
            self.draw_char(x + (i as u32) * 8, y, ch, fg, bg);
        }
    }

    // ── Transparent-background text ───────────────────────────────────────────
    // Only the lit pixels are drawn; the gaps keep whatever is already on screen
    // (the wallpaper gradient/glows). This is what lets the hero text and tray
    // labels float on the wallpaper instead of sitting in a solid box — the
    // single biggest "this is a real desktop, not a debug console" tell.
    pub fn draw_char_t(&mut self, x: u32, y: u32, ch: char, fg: u32) {
        let idx = (ch as u32).wrapping_sub(0x20);
        if idx >= 95 { return; }
        let bitmap = &FONT[idx as usize];
        for row in 0..8u32 {
            let byte = bitmap[row as usize];
            for col in 0..8u32 {
                if (byte >> (7 - col)) & 1 != 0 { self.set_pixel(x + col, y + row, fg); }
            }
        }
    }

    pub fn draw_str_t(&mut self, x: u32, y: u32, s: &str, fg: u32) {
        for (i, ch) in s.chars().enumerate() {
            self.draw_char_t(x + (i as u32) * 8, y, ch, fg);
        }
    }

    // Scaled transparent glyph: each font pixel becomes a scale×scale block,
    // gaps left transparent. scale=2 → 16px, scale=3 → 24px tall.
    pub fn draw_str_scaled_t(&mut self, x: u32, y: u32, s: &str, fg: u32, scale: u32) {
        let sc = scale.max(1);
        for (i, ch) in s.chars().enumerate() {
            let idx = (ch as u32).wrapping_sub(0x20);
            if idx >= 95 { continue; }
            let bitmap = &FONT[idx as usize];
            let gx = x + i as u32 * 8 * sc;
            for row in 0..8u32 {
                let byte = bitmap[row as usize];
                for col in 0..8u32 {
                    if (byte >> (7 - col)) & 1 != 0 {
                        for dr in 0..sc { for dc in 0..sc {
                            self.set_pixel(gx + col * sc + dc, y + row * sc + dr, fg);
                        }}
                    }
                }
            }
        }
    }

    // ── Anti-aliased proportional text ─────────────────────────────────────────
    // Smooth Ubuntu-Sans glyphs (grayscale coverage atlas in font_aa.rs),
    // alpha-blended over whatever is on screen. This is what lifts the desktop
    // out of the "Minecraft" 8x8-bitmap look toward the smooth mockup type.
    // `big` selects the display size (hero/headings) vs the body size.
    // Coordinates: (x, top) is the top-left of the text box; we add the ascent
    // internally to land on the baseline.
    // Font size selector: AA_T (tiny/secondary), AA_S (body), AA_L (display).
    pub fn aa_w(s: &str, sz: u8) -> i32 {
        let glyphs: &[crate::font_aa::Glyph] = match sz {
            crate::fb::AA_L => &crate::font_aa::GLYPHS_L,
            crate::fb::AA_T => &crate::font_aa::GLYPHS_T,
            _ => &crate::font_aa::GLYPHS_S,
        };
        let mut w = 0i32;
        for ch in s.chars() {
            let c = ch as u32;
            if (0x20..=0x7E).contains(&c) { w += glyphs[(c - 0x20) as usize].adv as i32; }
        }
        w
    }

    pub fn aa_line(sz: u8) -> i32 {
        match sz { crate::fb::AA_L => crate::font_aa::LINE_L, crate::fb::AA_T => crate::font_aa::LINE_T, _ => crate::font_aa::LINE_S }
    }

    pub fn draw_aa(&mut self, x: i32, top: i32, s: &str, fg: u32, sz: u8) -> i32 {
        let (glyphs, cov, ascent): (&[crate::font_aa::Glyph], &[u8], i32) = match sz {
            crate::fb::AA_L => (&crate::font_aa::GLYPHS_L, &crate::font_aa::COV_L, crate::font_aa::ASCENT_L),
            crate::fb::AA_T => (&crate::font_aa::GLYPHS_T, &crate::font_aa::COV_T, crate::font_aa::ASCENT_T),
            _ => (&crate::font_aa::GLYPHS_S, &crate::font_aa::COV_S, crate::font_aa::ASCENT_S),
        };
        let baseline = top + ascent;
        let fr = (fg >> 16) & 0xFF; let fgc = (fg >> 8) & 0xFF; let fb_ = fg & 0xFF;
        let mut pen = x;
        for ch in s.chars() {
            let c = ch as u32;
            if !(0x20..=0x7E).contains(&c) { continue; }
            let g = &glyphs[(c - 0x20) as usize];
            let gx = pen + g.bx as i32;
            let gy = baseline - g.top as i32;
            let gw = g.w as i32; let gh = g.h as i32; let off = g.off as usize;
            for row in 0..gh {
                let py = gy + row;
                if py < 0 || py as u32 >= self.height { continue; }
                for col in 0..gw {
                    let a = cov[off + (row * gw + col) as usize] as u32;
                    if a == 0 { continue; }
                    let px = gx + col;
                    if px < 0 || px as u32 >= self.width { continue; }
                    let d = self.get_pixel(px as u32, py as u32);
                    let ia = 255 - a;
                    let or = (fr * a + ((d >> 16) & 0xFF) * ia) / 255;
                    let og = (fgc * a + ((d >> 8) & 0xFF) * ia) / 255;
                    let ob = (fb_ * a + (d & 0xFF) * ia) / 255;
                    self.set_pixel(px as u32, py as u32, (or << 16) | (og << 8) | ob);
                }
            }
            pen += g.adv as i32;
        }
        pen - x
    }

    // Centered AA text within [x, x+w).
    pub fn draw_aa_centered(&mut self, x: i32, w: i32, top: i32, s: &str, fg: u32, sz: u8) {
        let tw = Self::aa_w(s, sz);
        self.draw_aa(x + (w - tw).max(0) / 2, top, s, fg, sz);
    }

    // ── HD icon blit ────────────────────────────────────────────────────────────
    // Alpha-blend an anti-aliased coverage icon (icons.rs) tinted with `color`
    // over the framebuffer — the mockup's crisp line-art look, no SVG runtime.
    pub fn draw_icon(&mut self, x: i32, y: i32, id: usize, color: u32) {
        if id >= crate::icons::ICON_OFF.len() { return; }
        let off = crate::icons::ICON_OFF[id] as usize;
        let px = crate::icons::ICON_PX as i32;
        let cr = (color >> 16) & 0xFF; let cg = (color >> 8) & 0xFF; let cb = color & 0xFF;
        for row in 0..px {
            let py = y + row;
            if py < 0 || py as u32 >= self.height { continue; }
            for col in 0..px {
                let a = crate::icons::ICON_COV[off + (row * px + col) as usize] as u32;
                if a == 0 { continue; }
                let pxn = x + col;
                if pxn < 0 || pxn as u32 >= self.width { continue; }
                let d = self.get_pixel(pxn as u32, py as u32);
                let ia = 255 - a;
                let or = (cr * a + ((d >> 16) & 0xFF) * ia) / 255;
                let og = (cg * a + ((d >> 8) & 0xFF) * ia) / 255;
                let ob = (cb * a + (d & 0xFF) * ia) / 255;
                self.set_pixel(pxn as u32, py as u32, (or << 16) | (og << 8) | ob);
            }
        }
    }

    // ── Soft radial glow ──────────────────────────────────────────────────────
    // Additive-feel light pool: blends `color` toward the wallpaper with a
    // quadratic falloff from the center. `max_alpha` (0..=255) is the opacity at
    // the very center. Drawn once into the cached background, so cost is fine.
    pub fn glow(&mut self, cx: i32, cy: i32, r: i32, color: u32, max_alpha: u32) {
        if r <= 0 { return; }
        let r2 = (r * r) as i64;
        let sr = (color >> 16) & 0xFF; let sg = (color >> 8) & 0xFF; let sb = color & 0xFF;
        let xa = (cx - r).max(0); let ya = (cy - r).max(0);
        let xb = (cx + r).min(self.width as i32); let yb = (cy + r).min(self.height as i32);
        for py in ya..yb {
            for px in xa..xb {
                let dx = (px - cx) as i64; let dy = (py - cy) as i64;
                let d2 = dx * dx + dy * dy;
                if d2 >= r2 { continue; }
                // quadratic falloff: full at center, 0 at the rim
                let f = ((r2 - d2) * (r2 - d2)) / (r2 * r2 / 256); // 0..256
                let a = (max_alpha as i64 * f / 256).min(255) as u32;
                if a == 0 { continue; }
                let d = self.get_pixel(px as u32, py as u32);
                let dr = (d >> 16) & 0xFF; let dg = (d >> 8) & 0xFF; let db = d & 0xFF;
                let or = (sr * a + dr * (255 - a)) / 255;
                let og = (sg * a + dg * (255 - a)) / 255;
                let ob = (sb * a + db * (255 - a)) / 255;
                self.set_pixel(px as u32, py as u32, (or << 16) | (og << 8) | ob);
            }
        }
    }

    // Signed-coordinate fill_rect — clips to framebuffer bounds.
    pub fn fill_rect_s(&mut self, x: i32, y: i32, w: i32, h: i32, color: u32) {
        if w <= 0 || h <= 0 { return; }
        let x0 = x.max(0) as u32;
        let y0 = y.max(0) as u32;
        let x1 = (x + w).min(self.width as i32).max(0) as u32;
        let y1 = (y + h).min(self.height as i32).max(0) as u32;
        if x1 > x0 && y1 > y0 { self.fill_rect(x0, y0, x1 - x0, y1 - y0, color); }
    }

    // Rounded rectangle — pixel-exact, no antialiasing.
    pub fn fill_rounded_rect(&mut self, x: i32, y: i32, w: i32, h: i32, r: i32, color: u32) {
        if w <= 0 || h <= 0 { return; }
        let r = r.min(w / 2).min(h / 2).max(0);
        if r == 0 { self.fill_rect_s(x, y, w, h, color); return; }
        let r2 = r * r;
        let cxl = x + r; let cxr = x + w - 1 - r;
        let cyt = y + r; let cyb = y + h - 1 - r;
        let xa = x.max(0); let ya = y.max(0);
        let xb = (x + w).min(self.width as i32);
        let yb = (y + h).min(self.height as i32);
        for py in ya..yb {
            for px in xa..xb {
                if (px < cxl && py < cyt && { let dx=px-cxl; let dy=py-cyt; dx*dx+dy*dy>r2 })
                || (px > cxr && py < cyt && { let dx=px-cxr; let dy=py-cyt; dx*dx+dy*dy>r2 })
                || (px < cxl && py > cyb && { let dx=px-cxl; let dy=py-cyb; dx*dx+dy*dy>r2 })
                || (px > cxr && py > cyb && { let dx=px-cxr; let dy=py-cyb; dx*dx+dy*dy>r2 })
                { continue; }
                self.set_pixel(px as u32, py as u32, color);
            }
        }
    }

    /// Frosted-glass rounded fill: alpha-blend `color` over the pixels already
    /// there (which carry the wallpaper), giving the mockup's translucent panel
    /// look without a full backdrop blur. `alpha` is 0..=255 (opacity of color).
    /// This is the sparse-rendering thesis applied to chrome: we read what's
    /// dormant behind the panel and only tint it, rather than re-deriving it.
    pub fn fill_rounded_rect_glass(&mut self, x: i32, y: i32, w: i32, h: i32, r: i32, color: u32, alpha: u32) {
        if w <= 0 || h <= 0 { return; }
        let r = r.min(w / 2).min(h / 2).max(0);
        let r2 = r * r;
        let cxl = x + r; let cxr = x + w - 1 - r;
        let cyt = y + r; let cyb = y + h - 1 - r;
        let xa = x.max(0); let ya = y.max(0);
        let xb = (x + w).min(self.width as i32);
        let yb = (y + h).min(self.height as i32);
        let a = alpha.min(255); let ia = 255 - a;
        let sr = (color >> 16) & 0xFF; let sg = (color >> 8) & 0xFF; let sb = color & 0xFF;
        for py in ya..yb {
            for px in xa..xb {
                if r > 0 && (
                   (px < cxl && py < cyt && { let dx=px-cxl; let dy=py-cyt; dx*dx+dy*dy>r2 })
                || (px > cxr && py < cyt && { let dx=px-cxr; let dy=py-cyt; dx*dx+dy*dy>r2 })
                || (px < cxl && py > cyb && { let dx=px-cxl; let dy=py-cyb; dx*dx+dy*dy>r2 })
                || (px > cxr && py > cyb && { let dx=px-cxr; let dy=py-cyb; dx*dx+dy*dy>r2 }))
                { continue; }
                let d = self.get_pixel(px as u32, py as u32);
                let dr = (d >> 16) & 0xFF; let dg = (d >> 8) & 0xFF; let db = d & 0xFF;
                let or = (sr * a + dr * ia) / 255;
                let og = (sg * a + dg * ia) / 255;
                let ob = (sb * a + db * ia) / 255;
                self.set_pixel(px as u32, py as u32, (or << 16) | (og << 8) | ob);
            }
        }
    }

    /// Draw the DINGIR 𒀭 8-point star (Simeon's brand mark) at radius `r`,
    /// scanline-filled from the mockup's 16-point geometry (8 outer @ r, 8 inner
    /// @ ~0.27r, alternating). Crisp at any size — replaces the bitmap glyph.
    pub fn draw_star8(&mut self, cx: i32, cy: i32, r: i32, color: u32) {
        const N: usize = 16;
        // unit points ×1000, math orientation (y up): outer(k·45°)/inner(k·45°+22.5°)
        const UX: [i32; N] = [1000,249,707,103,0,-103,-707,-249,-1000,-249,-707,-103,0,103,707,249];
        const UY: [i32; N] = [0,103,707,249,1000,249,707,103,0,-103,-707,-249,-1000,-249,-707,-103];
        let mut vx = [0i32; N]; let mut vy = [0i32; N];
        for i in 0..N { vx[i] = cx + r * UX[i] / 1000; vy[i] = cy - r * UY[i] / 1000; }
        let mut py = cy - r;
        while py <= cy + r {
            let mut xs = [0i32; N]; let mut nx = 0usize;
            let mut j = N - 1;
            for i in 0..N {
                let (yi, yj) = (vy[i], vy[j]);
                if (yi <= py && yj > py) || (yj <= py && yi > py) {
                    let x = vx[i] + (py - yi) * (vx[j] - vx[i]) / (yj - yi);
                    if nx < N { xs[nx] = x; nx += 1; }
                }
                j = i;
            }
            // sort intersections
            for a in 0..nx { for b in (a + 1)..nx { if xs[b] < xs[a] { xs.swap(a, b); } } }
            let mut k = 0;
            while k + 1 < nx {
                let mut x = xs[k];
                while x <= xs[k + 1] {
                    if x >= 0 && py >= 0 { self.set_pixel(x as u32, py as u32, color); }
                    x += 1;
                }
                k += 2;
            }
            py += 1;
        }
    }

    // Draw a single character at 2× scale (16×16 px per glyph).
    pub fn draw_char_2x(&mut self, x: u32, y: u32, ch: char, fg: u32, bg: u32) {
        let idx = (ch as u32).wrapping_sub(0x20);
        if idx >= 95 { return; }
        let bitmap = &FONT[idx as usize];
        for row in 0..8u32 {
            let byte = bitmap[row as usize];
            for col in 0..8u32 {
                let c = if (byte >> (7 - col)) & 1 != 0 { fg } else { bg };
                self.set_pixel(x + col*2,   y + row*2,   c);
                self.set_pixel(x + col*2+1, y + row*2,   c);
                self.set_pixel(x + col*2,   y + row*2+1, c);
                self.set_pixel(x + col*2+1, y + row*2+1, c);
            }
        }
    }

    // Draw a string at 2× scale (each glyph is 16 px wide).
    pub fn draw_str_2x(&mut self, x: u32, y: u32, s: &str, fg: u32, bg: u32) {
        for (i, ch) in s.chars().enumerate() {
            self.draw_char_2x(x + i as u32 * 16, y, ch, fg, bg);
        }
    }

    // Draw an 8×8 bitmap at 3× scale (produces a 24×24 px image).
    pub fn draw_bitmap_3x(&mut self, x: u32, y: u32, bitmap: &[u8; 8], fg: u32, bg: u32) {
        for row in 0..8u32 {
            let byte = bitmap[row as usize];
            for col in 0..8u32 {
                let c = if (byte >> (7 - col)) & 1 != 0 { fg } else { bg };
                for dr in 0..3u32 { for dc in 0..3u32 {
                    self.set_pixel(x + col*3 + dc, y + row*3 + dr, c);
                }}
            }
        }
    }

    pub fn fill_circle(&mut self, cx: i32, cy: i32, r: i32, color: u32) {
        let r2 = r * r;
        for dy in -r..=r {
            for dx in -r..=r {
                if dx * dx + dy * dy <= r2 {
                    let px = cx + dx;
                    let py = cy + dy;
                    if px >= 0 && py >= 0 && (px as u32) < self.width && (py as u32) < self.height {
                        self.set_pixel(px as u32, py as u32, color);
                    }
                }
            }
        }
    }

    pub fn flush(&self) {}
}
