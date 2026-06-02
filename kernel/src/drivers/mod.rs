//! Universal Driver Framework root.

pub mod udi;
pub mod thermal;
pub mod nvme;
pub mod soc;
pub mod hid;
pub mod audio {
    pub mod mixer;
    pub mod alsa;
}

pub fn init() {
    crate::e1000::udi_init();
    crate::virtio_gpu::udi_init();
    crate::drivers::thermal::init();
    crate::drivers::nvme::udi_init();
    crate::drivers::soc::snapdragon::udi_init();
    crate::drivers::hid::udi_init();
    crate::drivers::audio::mixer::udi_init();
    crate::drivers::audio::alsa::udi_init();
    crate::serial::write_str("  [udi] Driver bus initialized.\n");
}
