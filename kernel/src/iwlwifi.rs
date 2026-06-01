//! Intel iwlwifi WiFi driver — the hardest single brick in the OS, started honestly.
//!
//! Full native WiFi is a multi-chip, firmware-driven beast: load a per-chip
//! firmware image, bring up the host command ring, run the 802.11 station state
//! machine (scan/auth/assoc), then a WPA2/WPA3 supplicant (EAPOL 4-way, CCMP).
//! And crucially: **QEMU does not emulate iwlwifi**, so none of the device
//! bring-up can be verified here — only on a real Intel-WiFi laptop. So we build
//! it brick by brick, leading with the parts that ARE verifiable off-hardware.
//!
//! Brick 1 (this file): recognise an Intel WiFi card on the PCI bus, map it to
//! its chip and the firmware family it needs, and parse a real firmware image
//! (`iwlwifi_fw`). The parser is host-tested against a genuine Intel `.ucode`.
//! Brick 2+ (later, needs hardware): MMIO/NIC prph access, firmware DMA upload +
//! the ALIVE handshake, RX/TX rings, then 802.11 + WPA.

#![allow(dead_code)]

use crate::iwlwifi_fw;

const INTEL_VENDOR: u16 = 0x8086;
const CLASS_NETWORK: u8 = 0x02;
const SUBCLASS_OTHER: u8 = 0x80; // wireless controllers report network/other

/// A detected Intel WiFi card.
pub struct Card {
    pub bus: u8,
    pub dev: u8,
    pub func: u8,
    pub device_id: u16,
    pub mmio: u64,         // BAR0 physical base (the device's register window)
    pub chip: &'static str,
    pub fw_family: &'static str, // firmware basename, sans "-<api>.ucode"
}

/// Map an Intel WiFi PCI device id to (chip name, firmware family basename).
/// A representative slice of the iwlwifi id table — the common laptop parts from
/// the 7260 generation through Wi-Fi 6E/7. Unknown Intel wireless ids still get
/// recognised as a card (just with a generic firmware family).
fn lookup(device_id: u16) -> (&'static str, &'static str) {
    match device_id {
        0x08b1 | 0x08b2 => ("Wireless 7260", "iwlwifi-7260-17"),
        0x095a | 0x095b => ("Wireless 7265", "iwlwifi-7265D-29"),
        0x3165 | 0x3166 => ("Wireless 3165", "iwlwifi-7265D-29"),
        0x24f3 | 0x24f4 => ("Wireless 8260", "iwlwifi-8000C-36"),
        0x24fd          => ("Wireless 8265", "iwlwifi-8265-36"),
        0x2526          => ("Wireless 9260", "iwlwifi-9260-th-b0-jf-b0-46"),
        0x30dc | 0x31dc => ("Wireless 9560", "iwlwifi-9000-pu-b0-jf-b0-46"),
        0x9df0 | 0xa370 => ("Wireless 9560", "iwlwifi-9000-pu-b0-jf-b0-46"),
        0x2723          => ("Wi-Fi 6 AX200", "iwlwifi-cc-a0-77"),
        0x02f0 | 0x06f0 | 0x34f0 | 0x3df0 | 0x43f0 | 0xa0f0 =>
            ("Wi-Fi 6 AX201", "iwlwifi-QuZ-a0-hr-b0-77"),
        0x271b | 0x271c => ("Wi-Fi 6 AX2xx", "iwlwifi-cc-a0-77"),
        0x2725          => ("Wi-Fi 6E AX210", "iwlwifi-ty-a0-gf-a0-77"),
        0x51f0 | 0x51f1 | 0x54f0 => ("Wi-Fi 6E AX211", "iwlwifi-so-a0-gf-a0-77"),
        0x7af0 | 0x7e40 => ("Wi-Fi 6E AX211", "iwlwifi-so-a0-gf-a0-77"),
        _ => ("Intel wireless", "iwlwifi"),
    }
}

/// Scan PCI for an Intel WiFi card. Returns None when there's none (the normal
/// case under QEMU, which has no iwlwifi device).
pub fn detect() -> Option<Card> {
    let (bus, dev, func, _pif) = crate::pci::find_class(CLASS_NETWORK, SUBCLASS_OTHER)?;
    let (vendor, device_id) = crate::pci::vendor_device(bus, dev, func);
    if vendor != INTEL_VENDOR {
        return None;
    }
    // BAR0 is a 64-bit memory BAR on these parts; mask the low type bits.
    let bar0 = crate::pci::bar(bus, dev, func, 0) as u64;
    let mmio = bar0 & !0xf;
    let (chip, fw_family) = lookup(device_id);
    Some(Card { bus, dev, func, device_id, mmio, chip, fw_family })
}

/// Boot-time probe. Logs what we found; under QEMU this is just an honest
/// "no card". Returns true if an Intel WiFi card is present.
pub fn init() -> bool {
    match detect() {
        Some(c) => {
            crate::serial::write_str("  [iwlwifi] found Intel ");
            crate::serial::write_str(c.chip);
            crate::serial::write_str(" (8086:");
            log_hex16(c.device_id);
            crate::serial::write_str(") MMIO ");
            crate::serial::write_hex_u64(c.mmio);
            crate::serial::write_str("\n  [iwlwifi] firmware family ");
            crate::serial::write_str(c.fw_family);
            crate::serial::write_str(".ucode — parser ready; DMA upload is brick 2 (needs HW)\n");
            crate::pci::enable_bus_master(c.bus, c.dev, c.func);
            true
        }
        None => {
            crate::serial::write_str("  [iwlwifi] no Intel WiFi card (expected under QEMU — no iwlwifi emulation)\n");
            false
        }
    }
}

/// Parse a firmware image already loaded into memory and log its inventory.
/// Called by brick 2 once we can pull the `.ucode` off disk; exposed now so the
/// parser is wired into the driver, not just the host test.
pub fn inspect_firmware(blob: &[u8]) -> bool {
    match iwlwifi_fw::parse(blob) {
        Some(fw) if fw.magic_ok => {
            crate::serial::write_str("  [iwlwifi] firmware OK: ");
            log_dec(fw.tlv_count as u64);
            crate::serial::write_str(" TLVs, ");
            log_dec(fw.sec_count as u64);
            crate::serial::write_str(" sections, api v");
            log_dec(fw.api_version() as u64);
            crate::serial::write_str("\n");
            true
        }
        _ => {
            crate::serial::write_str("  [iwlwifi] firmware image rejected (bad magic)\n");
            false
        }
    }
}

fn log_hex16(v: u16) {
    let d = |n: u8| if n < 10 { b'0' + n } else { b'a' + n - 10 };
    for i in (0..4).rev() {
        crate::serial::write_byte(d(((v >> (i * 4)) & 0xf) as u8));
    }
}

fn log_dec(mut v: u64) {
    if v == 0 {
        crate::serial::write_byte(b'0');
        return;
    }
    let mut buf = [0u8; 20];
    let mut i = buf.len();
    while v > 0 {
        i -= 1;
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
    }
    for &b in &buf[i..] {
        crate::serial::write_byte(b);
    }
}
