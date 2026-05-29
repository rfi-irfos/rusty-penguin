// Minimal networking: Ethernet framing + ARP, enough to prove a full TX→RX
// round-trip on the RTL8139. We send an ARP "who-has <gateway>" and poll for
// the reply, then log the gateway's MAC. This is brick 1 of the net stack;
// IPv4/ICMP/UDP/DHCP/TCP ride on top later.

use crate::rtl8139;

// QEMU user-mode networking (SLIRP) defaults: guest 10.0.2.15, gateway 10.0.2.2.
const OUR_IP: [u8; 4] = [10, 0, 2, 15];
const GW_IP:  [u8; 4] = [10, 0, 2, 2];
const BCAST:  [u8; 6] = [0xFF; 6];
const ETHERTYPE_ARP: u16 = 0x0806;

fn nib(n: u8) -> u8 { if n < 10 { b'0' + n } else { b'a' + (n - 10) } }

fn log_mac(prefix: &str, mac: &[u8; 6]) {
    crate::serial::write_str(prefix);
    let mut buf = [0u8; 17]; // aa:bb:cc:dd:ee:ff
    let mut p = 0;
    for (i, &b) in mac.iter().enumerate() {
        buf[p] = nib(b >> 4); buf[p + 1] = nib(b & 0xF); p += 2;
        if i < 5 { buf[p] = b':'; p += 1; }
    }
    crate::serial::write_str(core::str::from_utf8(&buf[..p]).unwrap_or("?"));
    crate::serial::write_str("\n");
}

/// Build an ARP request (who-has `target_ip`) into `frame`; returns its length.
fn build_arp_request(frame: &mut [u8; 42], src_mac: &[u8; 6], target_ip: &[u8; 4]) {
    // Ethernet header
    frame[0..6].copy_from_slice(&BCAST);
    frame[6..12].copy_from_slice(src_mac);
    frame[12] = (ETHERTYPE_ARP >> 8) as u8;
    frame[13] = (ETHERTYPE_ARP & 0xFF) as u8;
    // ARP payload
    let a = &mut frame[14..42];
    a[0] = 0x00; a[1] = 0x01;           // htype = Ethernet
    a[2] = 0x08; a[3] = 0x00;           // ptype = IPv4
    a[4] = 6; a[5] = 4;                 // hlen, plen
    a[6] = 0x00; a[7] = 0x01;           // op = request
    a[8..14].copy_from_slice(src_mac);  // sender MAC
    a[14..18].copy_from_slice(&OUR_IP); // sender IP
    // a[18..24] target MAC = zeros
    a[24..28].copy_from_slice(target_ip);
}

/// ARP-probe the gateway to prove TX+RX. Returns the gateway MAC if a reply
/// arrives within the poll budget.
pub fn arp_probe() -> Option<[u8; 6]> {
    let nic = rtl8139::nic()?;
    log_mac("  [net] our MAC ", &nic.mac);

    let mut frame = [0u8; 42];
    build_arp_request(&mut frame, &nic.mac, &GW_IP);
    nic.send(&frame);
    crate::serial::write_str("  [net] ARP who-has 10.0.2.2 sent\n");

    let mut rx = [0u8; 1536];
    for _ in 0..2_000_000u32 {
        if let Some(len) = nic.poll_rx(&mut rx) {
            if len >= 42
                && rx[12] == 0x08 && rx[13] == 0x06          // ethertype ARP
                && rx[20] == 0x00 && rx[21] == 0x02           // op = reply
                && rx[28..32] == GW_IP                        // sender IP = gateway
            {
                let mut mac = [0u8; 6];
                mac.copy_from_slice(&rx[22..28]);             // ARP sender MAC
                log_mac("  [net] gateway MAC ", &mac);
                return Some(mac);
            }
        }
    }
    crate::serial::write_str("  [net] no ARP reply (timeout)\n");
    None
}

/// Bring up the NIC and run the ARP round-trip. Returns a ternary result:
/// Pos = full TX+RX proven, Zero = NIC up but no reply, Neg = no NIC.
pub fn init() -> ternary_core::Trit {
    use ternary_core::Trit;
    if !rtl8139::init() {
        return Trit::Neg;
    }
    match arp_probe() {
        Some(_) => Trit::Pos,
        None    => Trit::Zero,
    }
}
