// Bare-metal framebuffer via sys_fb_query (nr=6).
// Kernel fills a 24-byte struct: [u64 base][u32 width][u32 height][u32 pitch][u32 bpp]
// Framebuffer is identity-mapped so the returned pointer is usable directly.

extern crate alloc;

use crate::font::FONT;

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
}

unsafe impl Send for Framebuffer {}

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
        })
    }

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
        let y0 = y0 as usize;
        let y1 = (y1 as usize).min(self.height as usize);
        if y1 <= y0 { return; }
        let off = y0 * stride;
        let len = (y1 - y0) * stride;
        unsafe {
            core::ptr::copy_nonoverlapping(self.back.as_ptr().add(off), self.real.add(off), len);
        }
    }

    /// Copy the backbuffer to the real framebuffer in one block. Call this
    /// exactly once per frame after all draw ops complete; intermediate
    /// paint states never become visible.
    pub fn present(&mut self) {
        let n = self.back.len();
        unsafe {
            core::ptr::copy_nonoverlapping(self.back.as_ptr(), self.real, n);
        }
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
