// PCI configuration space access via I/O ports 0xCF8 (addr) / 0xCFC (data).
// Supports 32-bit config reads; no ECAM/MMIO config needed for x86 PIIX/ICH.

const PCI_ADDR: u16 = 0xCF8;
const PCI_DATA: u16 = 0xCFC;

fn cfg_addr(bus: u8, dev: u8, func: u8, off: u8) -> u32 {
    0x8000_0000
        | ((bus  as u32) << 16)
        | ((dev  as u32) << 11)
        | ((func as u32) <<  8)
        | ((off  as u32) & 0xFC)
}

pub unsafe fn read32(bus: u8, dev: u8, func: u8, off: u8) -> u32 {
    crate::port::outl(PCI_ADDR, cfg_addr(bus, dev, func, off));
    crate::port::inl(PCI_DATA)
}

pub unsafe fn write32(bus: u8, dev: u8, func: u8, off: u8, val: u32) {
    crate::port::outl(PCI_ADDR, cfg_addr(bus, dev, func, off));
    crate::port::outl(PCI_DATA, val);
}

pub fn vendor_device(bus: u8, dev: u8, func: u8) -> (u16, u16) {
    let v = unsafe { read32(bus, dev, func, 0) };
    if v == 0xFFFF_FFFF { return (0xFFFF, 0xFFFF); }
    (v as u16, (v >> 16) as u16)
}

/// Read a 32-bit BAR value.  Returns the full value (address + flags).
pub fn bar(bus: u8, dev: u8, func: u8, n: u8) -> u32 {
    unsafe { read32(bus, dev, func, 0x10 + n * 4) }
}

/// Enable Bus Master + Memory Space in command register (offset 0x04).
pub fn enable_bus_master(bus: u8, dev: u8, func: u8) {
    let cmd = unsafe { read32(bus, dev, func, 0x04) };
    unsafe { write32(bus, dev, func, 0x04, cmd | 0x06) }; // Mem + BusMaster
}

/// Read class/subclass/prog-if from config offset 0x08.
pub fn class(bus: u8, dev: u8, func: u8) -> (u8, u8, u8) {
    let v = unsafe { read32(bus, dev, func, 0x08) };
    ((v >> 24) as u8, (v >> 16) as u8, (v >> 8) as u8)
}

/// Scan for the first device with matching (class, subclass). Returns
/// (bus, dev, func, prog_if).
pub fn find_class(class_code: u8, subclass: u8) -> Option<(u8, u8, u8, u8)> {
    for bus in 0u8..=3 {
        for dev in 0u8..32 {
            for func in 0u8..8 {
                let (vendor, _) = vendor_device(bus, dev, func);
                if vendor == 0xFFFF { if func == 0 { break; } continue; }
                let (c, s, p) = class(bus, dev, func);
                if c == class_code && s == subclass { return Some((bus, dev, func, p)); }
                if func == 0 {
                    let hdr = unsafe { read32(bus, dev, 0, 0x0C) };
                    if (hdr >> 23) & 1 == 0 { break; }
                }
            }
        }
    }
    None
}

/// Scan buses 0..4 for a device matching (vendor, device_id).
/// Returns (bus, dev, func) if found.
pub fn find(vendor: u16, device_id: u16) -> Option<(u8, u8, u8)> {
    for bus in 0u8..=3 {
        for dev in 0u8..32 {
            for func in 0u8..8 {
                let (v, d) = vendor_device(bus, dev, func);
                if v != 0xFFFF {
                    crate::drivers::udi::probe_device(v, d);
                }
                if v == vendor && d == device_id { return Some((bus, dev, func)); }
                if func == 0 {
                    // Check multi-function bit (header type bit 7)
                    let hdr = unsafe { read32(bus, dev, 0, 0x0C) };
                    if (hdr >> 23) & 1 == 0 { break; }  // single-function
                }
            }
        }
    }
    None
}
