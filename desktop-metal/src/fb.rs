// Bare-metal framebuffer via sys_fb_query (nr=6).
// Kernel fills a 24-byte struct: [u64 base][u32 width][u32 height][u32 pitch][u32 bpp]
// Framebuffer is identity-mapped so the returned pointer is usable directly.

use crate::font::FONT;

pub struct Framebuffer {
    pub data:   *mut u8,
    pub width:  u32,
    pub height: u32,
    pub bpp:    u32,
    pub stride: u32,
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
        Ok(Framebuffer { data: base as *mut u8, width, height, bpp, stride: pitch })
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
