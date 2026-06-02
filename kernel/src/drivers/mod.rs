//! Universal Driver Framework root.

pub mod udi;
pub mod thermal;

pub fn init() {
    crate::e1000::udi_init();
    crate::virtio_gpu::udi_init();
    crate::drivers::thermal::init();
    crate::serial::write_str("  [udi] Driver bus initialized.\n");
}
