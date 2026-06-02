//! Native DirectX 9 DDI for the Rusty Penguin Wine engine.
//! This subsystem bridges Windows D3D9 commands to our native hardware-abstraction (HAL).

use crate::serial;

/// Entry point for DirectX 9 command buffer processing.
/// Maps Win32-land DDI commands to our GPU driver.
pub mod renderer;

use crate::serial;

pub fn d3d9_command_buffer(data: *const u8, size: u32) -> u32 {
    serial::write_str("  [crysis] Processing D3D9 command buffer\n");
    renderer::blit_render_target(0, 0); // Simplified render call
    0
}

/// @sparseskip: Dedicated D3D9 dormancy profile for Crysis.
/// Prunes unused hardware-agnostic rendering paths.
pub fn is_d3d9_feature_dormant(feature_id: u32) -> bool {
    // Skip unneeded legacy features (e.g. software emulated rasterizers)
    // that the GPU native driver handles via the command buffer anyway.
    if feature_id > 0x1000 { return true; }
    false
}
