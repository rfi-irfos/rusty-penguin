// Framebuffer access via /dev/fb0 and /sys/class/graphics/fb0/
use std::fs;

use crate::font::FONT;

pub struct Framebuffer {
    pub width: u32,
    pub height: u32,
    pub bpp: u32,
    pub stride: u32,
    pub data: *mut u8,
    pub len: usize,
    _file: fs::File,
}

// SAFETY: We control the mmap lifetime via _file and len. Single-threaded draw loop.
unsafe impl Send for Framebuffer {}

impl Framebuffer {
    pub fn open() -> Result<Self, String> {
        // Read dimensions from sysfs
        let vsize = fs::read_to_string("/sys/class/graphics/fb0/virtual_size")
            .map_err(|e| format!("read virtual_size: {}", e))?;
        let vsize = vsize.trim();
        let mut parts = vsize.split(',');
        let width: u32 = parts.next().ok_or("no width")?.parse().map_err(|e| format!("parse width: {}", e))?;
        let height: u32 = parts.next().ok_or("no height")?.parse().map_err(|e| format!("parse height: {}", e))?;

        let bpp_str = fs::read_to_string("/sys/class/graphics/fb0/bits_per_pixel")
            .map_err(|e| format!("read bits_per_pixel: {}", e))?;
        let bpp: u32 = bpp_str.trim().parse().map_err(|e| format!("parse bpp: {}", e))?;

        let bytes_per_pixel = bpp / 8;
        let stride = width * bytes_per_pixel;
        let len = (stride * height) as usize;

        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/fb0")
            .map_err(|e| format!("open /dev/fb0: {}", e))?;

        let data = unsafe {
            let ptr = nix::sys::mman::mmap(
                None,
                std::num::NonZeroUsize::new(len).ok_or("zero len")?,
                nix::sys::mman::ProtFlags::PROT_READ | nix::sys::mman::ProtFlags::PROT_WRITE,
                nix::sys::mman::MapFlags::MAP_SHARED,
                Some(&file),
                0,
            ).map_err(|e| format!("mmap: {}", e))?;
            ptr as *mut u8
        };

        Ok(Framebuffer {
            width,
            height,
            bpp,
            stride,
            data,
            len,
            _file: file,
        })
    }

    #[inline]
    pub fn set_pixel(&mut self, x: u32, y: u32, color: u32) {
        if x >= self.width || y >= self.height {
            return;
        }
        let bpp = self.bpp;
        let stride = self.stride;
        let offset = (y * stride + x * (bpp / 8)) as usize;
        let r = ((color >> 16) & 0xFF) as u8;
        let g = ((color >> 8) & 0xFF) as u8;
        let b = (color & 0xFF) as u8;
        unsafe {
            if bpp == 32 {
                *self.data.add(offset)     = b;
                *self.data.add(offset + 1) = g;
                *self.data.add(offset + 2) = r;
                *self.data.add(offset + 3) = 0xFF;
            } else if bpp == 24 {
                *self.data.add(offset)     = b;
                *self.data.add(offset + 1) = g;
                *self.data.add(offset + 2) = r;
            } else if bpp == 16 {
                // RGB565
                let pixel: u16 = (((r as u16) & 0xF8) << 8)
                    | (((g as u16) & 0xFC) << 3)
                    | (((b as u16) & 0xF8) >> 3);
                *self.data.add(offset)     = (pixel & 0xFF) as u8;
                *self.data.add(offset + 1) = (pixel >> 8) as u8;
            }
        }
    }

    pub fn fill_rect(&mut self, x: u32, y: u32, w: u32, h: u32, color: u32) {
        for row in 0..h {
            for col in 0..w {
                self.set_pixel(x + col, y + row, color);
            }
        }
    }

    pub fn draw_char(&mut self, x: u32, y: u32, ch: char, fg: u32, bg: u32) {
        let idx = (ch as u32).wrapping_sub(0x20);
        if idx >= 95 {
            return;
        }
        let bitmap = &FONT[idx as usize];
        for row in 0..8u32 {
            let byte = bitmap[row as usize];
            for col in 0..8u32 {
                let bit = (byte >> (7 - col)) & 1;
                let color = if bit != 0 { fg } else { bg };
                self.set_pixel(x + col, y + row, color);
            }
        }
    }

    pub fn draw_str(&mut self, x: u32, y: u32, s: &str, fg: u32, bg: u32) {
        for (i, ch) in s.chars().enumerate() {
            self.draw_char(x + (i as u32) * 8, y, ch, fg, bg);
        }
    }

    #[inline]
    pub fn flush(&self) {
        // mmap writes are immediately visible; no explicit flush needed
    }
}

impl Drop for Framebuffer {
    fn drop(&mut self) {
        unsafe {
            let _ = nix::sys::mman::munmap(self.data as *mut _, self.len);
        }
    }
}
