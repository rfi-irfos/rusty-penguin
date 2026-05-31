// Bare-metal linear framebuffer driver.
// Physical address comes from the Multiboot2 framebuffer tag (type 8).
// The VRAM is mapped into the HIGHER HALF (PHYSMAP_BASE + phys) via
// vmm::map_mmio_high, so the framebuffer is reached through the kernel's own
// mapping rather than the low identity map (PML4[0]) — see docs/VMM_HIGHER_HALF.md.

use crate::font::FONT;

static mut FB_BASE:   *mut u8 = core::ptr::null_mut();
static mut FB_WIDTH:  u32     = 0;
static mut FB_HEIGHT: u32     = 0;
static mut FB_PITCH:  u32     = 0;
static mut FB_BPP:    u8      = 0;
static mut FB_LIVE:   bool    = false;

pub fn init(phys: u64, w: u32, h: u32, pitch: u32, bpp: u8) {
    let size = pitch as u64 * h as u64;
    let virt = unsafe { crate::vmm::map_mmio_high(phys, size) };
    unsafe {
        FB_BASE   = virt as *mut u8;
        FB_WIDTH  = w;
        FB_HEIGHT = h;
        FB_PITCH  = pitch;
        FB_BPP    = bpp;
        FB_LIVE   = true;
    }
}

pub fn is_live() -> bool  { unsafe { FB_LIVE } }
pub fn width()   -> u32   { unsafe { FB_WIDTH } }
pub fn height()  -> u32   { unsafe { FB_HEIGHT } }
pub fn pitch()   -> u32   { unsafe { FB_PITCH } }
pub fn base()    -> *mut u8 { unsafe { FB_BASE } }
pub fn bpp()     -> u8    { unsafe { FB_BPP } }

#[inline(always)]
pub fn read_pixel(x: u32, y: u32) -> u32 {
    unsafe {
        if x >= FB_WIDTH || y >= FB_HEIGHT { return 0; }
        let off = (y * FB_PITCH + x * (FB_BPP as u32 / 8)) as usize;
        let ptr = FB_BASE.add(off);
        match FB_BPP {
            32 => ptr.cast::<u32>().read_volatile() & 0x00FF_FFFF,
            24 => {
                let b = ptr.read_volatile() as u32;
                let g = ptr.add(1).read_volatile() as u32;
                let r = ptr.add(2).read_volatile() as u32;
                (r << 16) | (g << 8) | b
            }
            _ => 0,
        }
    }
}

#[inline(always)]
pub fn pixel(x: u32, y: u32, rgb: u32) {
    unsafe {
        if x >= FB_WIDTH || y >= FB_HEIGHT { return; }
        let off = (y * FB_PITCH + x * (FB_BPP as u32 / 8)) as usize;
        let ptr = FB_BASE.add(off);
        match FB_BPP {
            32 => ptr.cast::<u32>().write_volatile(rgb),
            24 => {
                ptr.write_volatile((rgb & 0xFF) as u8);
                ptr.add(1).write_volatile(((rgb >> 8) & 0xFF) as u8);
                ptr.add(2).write_volatile(((rgb >> 16) & 0xFF) as u8);
            }
            _ => {}
        }
    }
}

pub fn fill(x0: u32, y0: u32, w: u32, h: u32, rgb: u32) {
    let x1 = (x0 + w).min(width());
    let y1 = (y0 + h).min(height());
    for y in y0..y1 {
        for x in x0..x1 {
            pixel(x, y, rgb);
        }
    }
}

pub fn blit_char(x0: u32, y0: u32, ch: u8, fg: u32, bg: u32) {
    if ch < 0x20 || ch > 0x7E { return; }
    let glyph = &FONT[(ch - 0x20) as usize];
    for row in 0..8u32 {
        let bits = glyph[row as usize];
        for col in 0..8u32 {
            let lit = bits & (0x80 >> col) != 0;
            pixel(x0 + col, y0 + row, if lit { fg } else { bg });
        }
    }
}

pub fn draw_str(x0: u32, y0: u32, s: &[u8], fg: u32, bg: u32) {
    for (i, &ch) in s.iter().enumerate() {
        blit_char(x0 + i as u32 * 8, y0, ch, fg, bg);
    }
}
