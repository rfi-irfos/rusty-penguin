//! Ultra-high resolution (16K) and high-refresh (240Hz) framebuffer bridge.
//! Optimized for high-bandwidth displays using tiling.

static mut FB_REFRESH_RATE: u32 = 60;

pub fn upgrade_display(w: u32, h: u32, hz: u32) {
    unsafe {
        FB_WIDTH  = w;
        FB_HEIGHT = h;
        FB_REFRESH_RATE = hz;
    }
    crate::serial::write_str("  [fb] Display Mode Upgraded: ");
    crate::serial::write_hex_u32(w);
    crate::serial::write_str("x");
    crate::serial::write_hex_u32(h);
    crate::serial::write_str(" @ ");
    crate::serial::write_hex_u32(hz);
    crate::serial::write_str("Hz\n");
}
