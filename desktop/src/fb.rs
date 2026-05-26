// Framebuffer access via /dev/fb0
// Uses FBIOGET_VSCREENINFO / FBIOGET_FSCREENINFO ioctls (more reliable than sysfs).
use std::fs;
use std::os::unix::io::AsRawFd;

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

// SAFETY: single-threaded draw loop; mmap lifetime tied to _file.
unsafe impl Send for Framebuffer {}

// FBIOGET_VSCREENINFO = 0x4600  (struct fb_var_screeninfo, 160 bytes)
// FBIOGET_FSCREENINFO = 0x4602  (struct fb_fix_screeninfo, 80 bytes on x86_64)
// We read only the fields we care about from raw byte buffers to avoid struct-size
// encoding issues with the nix ioctl macro.
const FBIOGET_VSCREENINFO: u64 = 0x4600;
const FBIOGET_FSCREENINFO: u64 = 0x4602;

unsafe fn read_fb_ioctl(fd: i32) -> Option<(u32, u32, u32, u32)> {
    let mut vbuf = [0u8; 160]; // fb_var_screeninfo
    let mut fbuf = [0u8; 80];  // fb_fix_screeninfo

    if libc::ioctl(fd, FBIOGET_VSCREENINFO, vbuf.as_mut_ptr()) < 0 {
        return None;
    }
    if libc::ioctl(fd, FBIOGET_FSCREENINFO, fbuf.as_mut_ptr()) < 0 {
        return None;
    }

    // fb_var_screeninfo layout:
    //   u32 xres @ 0, u32 yres @ 4, ..., u32 bits_per_pixel @ 24
    let xres = u32::from_ne_bytes(vbuf[0..4].try_into().ok()?);
    let yres = u32::from_ne_bytes(vbuf[4..8].try_into().ok()?);
    let bpp  = u32::from_ne_bytes(vbuf[24..28].try_into().ok()?);

    // fb_fix_screeninfo layout (x86_64):
    //   char id[16], u64 smem_start, u32 smem_len, u32 type, u32 type_aux,
    //   u32 visual, u16 xpan, u16 ypan, u16 ywrap, u16 _pad,
    //   u32 line_length @ offset 48
    let line_length = u32::from_ne_bytes(fbuf[48..52].try_into().ok()?);
    let stride = if line_length > 0 { line_length } else { xres * (bpp / 8) };

    Some((xres, yres, bpp, stride))
}

fn read_fb_sysfs() -> Option<(u32, u32, u32)> {
    let vsize = fs::read_to_string("/sys/class/graphics/fb0/virtual_size").ok()?;
    let vsize = vsize.trim();
    let mut parts = vsize.split(',');
    let w: u32 = parts.next()?.parse().ok()?;
    let h: u32 = parts.next()?.parse().ok()?;
    let bpp_s = fs::read_to_string("/sys/class/graphics/fb0/bits_per_pixel").ok()?;
    let bpp: u32 = bpp_s.trim().parse().ok()?;
    Some((w, h, bpp))
}

impl Framebuffer {
    pub fn open() -> Result<Self, String> {
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/fb0")
            .map_err(|e| format!("open /dev/fb0: {}", e))?;

        let fd = file.as_raw_fd();

        let (width, height, bpp, stride) = unsafe { read_fb_ioctl(fd) }
            .or_else(|| {
                read_fb_sysfs().map(|(w, h, b)| (w, h, b, w * (b / 8)))
            })
            .ok_or_else(|| "cannot read framebuffer info (ioctl and sysfs both failed)".to_string())?;

        if width == 0 || height == 0 || bpp == 0 {
            return Err(format!("bad fb dimensions: {}x{}x{}", width, height, bpp));
        }

        eprintln!("[desktop] fb: {}x{}x{} stride={}", width, height, bpp, stride);

        let len = (stride * height) as usize;

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

        Ok(Framebuffer { width, height, bpp, stride, data, len, _file: file })
    }

    #[inline]
    pub fn get_pixel(&self, x: u32, y: u32) -> u32 {
        if x >= self.width || y >= self.height { return 0; }
        let offset = (y * self.stride + x * (self.bpp / 8)) as usize;
        unsafe {
            match self.bpp {
                32 | 24 => {
                    let b = *self.data.add(offset)     as u32;
                    let g = *self.data.add(offset + 1) as u32;
                    let r = *self.data.add(offset + 2) as u32;
                    (r << 16) | (g << 8) | b
                }
                _ => 0,
            }
        }
    }

    #[inline]
    pub fn set_pixel(&mut self, x: u32, y: u32, color: u32) {
        if x >= self.width || y >= self.height { return; }
        let bpp = self.bpp;
        let offset = (y * self.stride + x * (bpp / 8)) as usize;
        let r = ((color >> 16) & 0xFF) as u8;
        let g = ((color >> 8)  & 0xFF) as u8;
        let b = (color         & 0xFF) as u8;
        unsafe {
            match bpp {
                32 => {
                    *self.data.add(offset)     = b;
                    *self.data.add(offset + 1) = g;
                    *self.data.add(offset + 2) = r;
                    *self.data.add(offset + 3) = 0xFF;
                }
                24 => {
                    *self.data.add(offset)     = b;
                    *self.data.add(offset + 1) = g;
                    *self.data.add(offset + 2) = r;
                }
                16 => {
                    let px: u16 = (((r as u16) & 0xF8) << 8)
                        | (((g as u16) & 0xFC) << 3)
                        | (((b as u16) & 0xF8) >> 3);
                    *self.data.add(offset)     = (px & 0xFF) as u8;
                    *self.data.add(offset + 1) = (px >> 8) as u8;
                }
                _ => {}
            }
        }
    }

    // Fast fill: builds one pixel row then memcpy-s it across all rows.
    pub fn fill_rect(&mut self, x: u32, y: u32, w: u32, h: u32, color: u32) {
        if w == 0 || h == 0 { return; }
        let bpp = self.bpp / 8;
        let r = ((color >> 16) & 0xFF) as u8;
        let g = ((color >> 8)  & 0xFF) as u8;
        let b = (color         & 0xFF) as u8;

        // Build a single pixel pattern
        let pixel: [u8; 4] = match self.bpp {
            32 => [b, g, r, 0xFF],
            24 => [b, g, r, 0],
            16 => {
                let px: u16 = (((r as u16) & 0xF8) << 8)
                    | (((g as u16) & 0xFC) << 3)
                    | (((b as u16) & 0xF8) >> 3);
                [(px & 0xFF) as u8, (px >> 8) as u8, 0, 0]
            }
            _ => return,
        };

        let bpp = bpp as usize;
        let clamp_w = w.min(self.width.saturating_sub(x)) as usize;
        let clamp_h = h.min(self.height.saturating_sub(y));

        // Build one row in a Vec, then copy it to each fb row
        let row_bytes = clamp_w * bpp;
        let mut row_buf: Vec<u8> = Vec::with_capacity(row_bytes);
        for _ in 0..clamp_w {
            row_buf.extend_from_slice(&pixel[..bpp]);
        }

        for row in 0..clamp_h {
            let fb_off = ((y + row) * self.stride + x * (self.bpp / 8)) as usize;
            unsafe {
                std::ptr::copy_nonoverlapping(row_buf.as_ptr(), self.data.add(fb_off), row_bytes);
            }
        }
    }

    // Draw an 8×8 bitmap at 2× scale (16×16 output).
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

    #[inline]
    pub fn flush(&self) {}
}

impl Drop for Framebuffer {
    fn drop(&mut self) {
        unsafe { let _ = nix::sys::mman::munmap(self.data as *mut _, self.len); }
    }
}
