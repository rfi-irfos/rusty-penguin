//! Universal Driver Interface (UDI)
//! A hardware-agnostic bus that maps any driver type (WDM, Linux, Native)
//! to the Rusty Penguin Ternary HAL.

#[derive(Clone, Copy)]
pub enum DriverType {
    Native,
    WDM,
    LinuxShim,
}

pub trait UniversalDriver {
    fn init(&self) -> bool;
    fn handle_interrupt(&self, irq: u8);
    fn control(&self, code: u32, input: &[u8], output: &mut [u8]) -> u32;
}

pub struct UdiBus {
    pub drivers: [Option<&'static dyn UniversalDriver>; 32],
}

static mut BUS: UdiBus = UdiBus { drivers: [None; 32] };

pub fn register_driver(driver: &'static dyn UniversalDriver) {
    unsafe {
        for slot in BUS.drivers.iter_mut() {
            if slot.is_none() {
                *slot = Some(driver);
                return;
            }
        }
    }
}
