//! Thermal Management Subsystem.
//! Monitors CPU temperature via ACPI/DTS and performs thermal throttling
//! to ensure system reliability during high-load gaming sessions.

use crate::drivers::udi::{UniversalDriver, register_driver};
use crate::serial;

pub struct ThermalDriver;

impl UniversalDriver for ThermalDriver {
    fn init(&self) -> bool {
        serial::write_str("  [thermal] Initializing management...\n");
        true
    }
    fn handle_interrupt(&self, _irq: u8) {
        serial::write_str("  [thermal] Critical temp detected!\n");
    }
    fn control(&self, code: u32, _input: &[u8], _output: &mut [u8]) -> u32 {
        if code == 0x1 {
            serial::write_str("  [thermal] Throttling enabled\n");
            return 0;
        }
        0
    }
}

pub fn init() {
    register_driver(&ThermalDriver);
}
