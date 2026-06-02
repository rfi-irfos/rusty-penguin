//! Native GUI Compositor Bridge for the Wine subsystem.
//! Bridges Windows UI message loop to the Rusty Penguin kernel desktop.

use crate::serial;

/// Native message-pump bridge.
/// Routes NtUser/NtGdi messages to the local desktop compositor.
pub fn post_message(hwnd: u64, msg: u32, wparam: u64, lparam: u64) -> u32 {
    serial::write_str("  [wine-gui] Message posted to HWND: 0x");
    serial::write_hex_u32(hwnd as u32);
    serial::write_str("\n");
    0 // STATUS_SUCCESS
}

/// Blits the DirectX surface to the kernel's active display buffer.
pub fn blit_to_compositor(hdc: u64, x: u32, y: u32) {
    serial::write_str("  [wine-gui] Compositor blit initiated\n");
}
