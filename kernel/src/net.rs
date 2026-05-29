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
const ETHERTYPE_IP:  u16 = 0x0800;
const PROTO_ICMP:    u8  = 1;
const PROTO_UDP:     u8  = 17;

// Internet (RFC 1071) ones-complement checksum over a big-endian byte buffer.
fn inet_checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    let mut i = 0;
    while i + 1 < data.len() {
        sum += ((data[i] as u32) << 8) | data[i + 1] as u32;
        i += 2;
    }
    if i < data.len() { sum += (data[i] as u32) << 8; }
    while sum >> 16 != 0 { sum = (sum & 0xFFFF) + (sum >> 16); }
    !(sum as u16)
}

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

fn log_ip(prefix: &str, ip: &[u8]) {
    crate::serial::write_str(prefix);
    let mut buf = [0u8; 15];
    let mut p = 0;
    for (i, &b) in ip.iter().take(4).enumerate() {
        if b >= 100 { buf[p] = b'0' + b / 100; p += 1; }
        if b >= 10  { buf[p] = b'0' + (b / 10) % 10; p += 1; }
        buf[p] = b'0' + b % 10; p += 1;
        if i < 3 { buf[p] = b'.'; p += 1; }
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

/// Send an ICMP echo request to the gateway and poll for the echo reply.
/// Proves the IPv4 + ICMP layer end to end. `gw_mac` comes from ARP.
fn icmp_ping(gw_mac: &[u8; 6]) -> bool {
    let nic = match rtl8139::nic() { Some(n) => n, None => return false };
    const PAYLOAD: usize = 32;
    const ICMP_LEN: usize = 8 + PAYLOAD;
    const TOTAL: usize = 14 + 20 + ICMP_LEN;
    let mut f = [0u8; TOTAL];

    // Ethernet
    f[0..6].copy_from_slice(gw_mac);
    f[6..12].copy_from_slice(&nic.mac);
    f[12] = (ETHERTYPE_IP >> 8) as u8; f[13] = (ETHERTYPE_IP & 0xFF) as u8;

    // IPv4 header
    f[14] = 0x45;                                   // v4, IHL=5
    let total_len = (20 + ICMP_LEN) as u16;
    f[16] = (total_len >> 8) as u8; f[17] = total_len as u8;
    f[20] = 0x40;                                   // flags: Don't Fragment
    f[22] = 64;                                     // TTL
    f[23] = PROTO_ICMP;
    f[26..30].copy_from_slice(&OUR_IP);
    f[30..34].copy_from_slice(&GW_IP);
    let ipck = inet_checksum(&f[14..34]);
    f[24] = (ipck >> 8) as u8; f[25] = ipck as u8;

    // ICMP echo request
    let id: u16 = 0x1234; let seq: u16 = 1;
    f[34] = 8;                                       // type = echo request
    f[38] = (id >> 8) as u8; f[39] = id as u8;
    f[40] = (seq >> 8) as u8; f[41] = seq as u8;
    for i in 0..PAYLOAD { f[42 + i] = b'a' + (i as u8 % 26); }
    let icck = inet_checksum(&f[34..34 + ICMP_LEN]);
    f[36] = (icck >> 8) as u8; f[37] = icck as u8;

    nic.send(&f);
    crate::serial::write_str("  [net] ICMP echo -> 10.0.2.2 sent\n");

    let mut rx = [0u8; 1536];
    for _ in 0..2_000_000u32 {
        if let Some(len) = nic.poll_rx(&mut rx) {
            if len >= 42
                && rx[12] == 0x08 && rx[13] == 0x00      // IPv4
                && rx[23] == PROTO_ICMP                   // ICMP
                && rx[34] == 0                            // echo reply
                && rx[26..30] == GW_IP                    // from the gateway
                && rx[38] == (id >> 8) as u8 && rx[39] == id as u8
            {
                crate::serial::write_str("  [net] ICMP echo reply from 10.0.2.2 (ping OK)\n");
                return true;
            }
        }
    }
    crate::serial::write_str("  [net] no ICMP reply (timeout)\n");
    false
}

// ── DHCP ────────────────────────────────────────────────────────────────────
// DISCOVER → OFFER → REQUEST → ACK over UDP 68→67. SLIRP runs an internal DHCP
// server, so this works without any real network. UDP checksum 0 (allowed v4).
const DHCP_XID: [u8; 4] = [0x39, 0x03, 0xF3, 0x26];

/// Build a full Ethernet+IP+UDP+DHCP frame into `f`; returns its length.
/// `msg_type` 1=DISCOVER, 3=REQUEST. `req_ip`/`server_id` add options 50/54.
fn build_dhcp(f: &mut [u8], mac: &[u8; 6], msg_type: u8,
              req_ip: Option<[u8; 4]>, server_id: Option<[u8; 4]>) -> usize {
    for b in f.iter_mut() { *b = 0; }
    // Ethernet: broadcast
    f[0..6].copy_from_slice(&BCAST);
    f[6..12].copy_from_slice(mac);
    f[12] = 0x08; f[13] = 0x00;

    // DHCP payload starts at 42 (14 eth + 20 ip + 8 udp)
    let d = 42;
    f[d] = 1; f[d + 1] = 1; f[d + 2] = 6;          // op=BOOTREQUEST, htype, hlen
    f[d + 4..d + 8].copy_from_slice(&DHCP_XID);     // xid
    f[d + 10] = 0x80;                               // flags: broadcast
    f[d + 28..d + 34].copy_from_slice(mac);         // chaddr
    f[d + 236] = 0x63; f[d + 237] = 0x82; f[d + 238] = 0x53; f[d + 239] = 0x63; // magic
    let mut o = d + 240;
    f[o] = 53; f[o + 1] = 1; f[o + 2] = msg_type; o += 3;        // DHCP msg type
    if let Some(ip) = req_ip { f[o] = 50; f[o + 1] = 4; f[o + 2..o + 6].copy_from_slice(&ip); o += 6; }
    if let Some(s) = server_id { f[o] = 54; f[o + 1] = 4; f[o + 2..o + 6].copy_from_slice(&s); o += 6; }
    f[o] = 55; f[o + 1] = 4; f[o + 2] = 1; f[o + 3] = 3; f[o + 4] = 6; f[o + 5] = 15; o += 6; // param req
    f[o] = 255; o += 1;                                          // end
    let dhcp_len = o - d;

    // UDP (src 68 → dst 67), checksum 0
    let u = 34;
    f[u] = 0; f[u + 1] = 68; f[u + 2] = 0; f[u + 3] = 67;
    let udp_len = (8 + dhcp_len) as u16;
    f[u + 4] = (udp_len >> 8) as u8; f[u + 5] = udp_len as u8;

    // IPv4 (0.0.0.0 → 255.255.255.255, proto UDP)
    f[14] = 0x45;
    let total_len = (20 + 8 + dhcp_len) as u16;
    f[16] = (total_len >> 8) as u8; f[17] = total_len as u8;
    f[22] = 64; f[23] = PROTO_UDP;
    f[30] = 255; f[31] = 255; f[32] = 255; f[33] = 255;          // dst broadcast
    let ipck = inet_checksum(&f[14..34]);
    f[24] = (ipck >> 8) as u8; f[25] = ipck as u8;

    14 + total_len as usize
}

/// Walk DHCP options for a given code; returns the value slice.
fn dhcp_opt(opts: &[u8], code: u8) -> Option<&[u8]> {
    let mut i = 0;
    while i < opts.len() {
        match opts[i] {
            255 => break,        // end
            0   => { i += 1; }   // pad
            c   => {
                if i + 1 >= opts.len() { break; }
                let len = opts[i + 1] as usize;
                if i + 2 + len > opts.len() { break; }
                if c == code { return Some(&opts[i + 2..i + 2 + len]); }
                i += 2 + len;
            }
        }
    }
    None
}

/// Parse a received DHCP reply of `want_type`; returns (yiaddr, server_id).
fn parse_dhcp(rx: &[u8], len: usize, want_type: u8) -> Option<([u8; 4], [u8; 4])> {
    if len < 282 { return None; }
    if !(rx[12] == 0x08 && rx[13] == 0x00 && rx[23] == PROTO_UDP) { return None; }
    if !(rx[34] == 0 && rx[35] == 67 && rx[36] == 0 && rx[37] == 68) { return None; } // 67→68
    let d = 42;
    if rx[d] != 2 { return None; }                                   // BOOTREPLY
    if rx[d + 4..d + 8] != DHCP_XID { return None; }                 // our xid
    if !(rx[d + 236] == 0x63 && rx[d + 237] == 0x82) { return None; } // magic cookie
    let opts = &rx[d + 240..len.min(rx.len())];
    let mt = dhcp_opt(opts, 53)?;
    if mt.is_empty() || mt[0] != want_type { return None; }
    let mut yi = [0u8; 4]; yi.copy_from_slice(&rx[d + 16..d + 20]);
    let mut sid = [0u8; 4];
    if let Some(s) = dhcp_opt(opts, 54) { if s.len() == 4 { sid.copy_from_slice(s); } }
    Some((yi, sid))
}

/// Run a DHCP handshake; on success logs and returns the leased IP.
fn dhcp() -> Option<[u8; 4]> {
    let nic = rtl8139::nic()?;
    let mac = nic.mac;
    let mut f = [0u8; 400];
    let mut rx = [0u8; 1536];

    // DISCOVER → OFFER
    let n = build_dhcp(&mut f, &mac, 1, None, None);
    nic.send(&f[..n]);
    crate::serial::write_str("  [net] DHCP DISCOVER sent\n");
    let mut offer = None;
    for _ in 0..3_000_000u32 {
        if let Some(l) = nic.poll_rx(&mut rx) {
            if let Some(r) = parse_dhcp(&rx, l, 2) { offer = Some(r); break; }
        }
    }
    let (yiaddr, sid) = match offer { Some(r) => r, None => {
        crate::serial::write_str("  [net] no DHCP OFFER (timeout)\n"); return None; } };
    log_ip("  [net] DHCP OFFER ", &yiaddr);

    // REQUEST → ACK
    let n = build_dhcp(&mut f, &mac, 3, Some(yiaddr), Some(sid));
    nic.send(&f[..n]);
    crate::serial::write_str("  [net] DHCP REQUEST sent\n");
    for _ in 0..3_000_000u32 {
        if let Some(l) = nic.poll_rx(&mut rx) {
            if let Some((ip, _)) = parse_dhcp(&rx, l, 5) { // ACK
                log_ip("  [net] DHCP ACK — lease ", &ip);
                return Some(ip);
            }
        }
    }
    crate::serial::write_str("  [net] no DHCP ACK (timeout)\n");
    None
}

/// Bring up the NIC, ARP the gateway, then ping it. Ternary result:
/// Pos = ping round-trip OK, Zero = NIC up but ARP/ping failed, Neg = no NIC.
pub fn init() -> ternary_core::Trit {
    use ternary_core::Trit;
    if !rtl8139::init() {
        return Trit::Neg;
    }
    let gw = match arp_probe() {
        Some(m) => m,
        None    => return Trit::Zero,
    };
    let _ = icmp_ping(&gw);   // ping (result logged)
    match dhcp() {
        Some(_) => Trit::Pos, // full DHCP lease — the strongest proof
        None    => Trit::Zero,
    }
}
