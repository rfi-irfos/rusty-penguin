// Rusty Penguin network stack — multi-NIC aware.
// Supports RTL8139 (QEMU, some older hardware) and Intel e1000/e1000e
// (modern laptops, QEMU -device e1000). Both have the same .mac/.send/.poll_rx
// API; ActiveNic wraps whichever initialised first.

use crate::rtl8139;
use crate::e1000;
use crate::r8169;

// ── NIC abstraction ───────────────────────────────────────────────────────────
static mut ACTIVE_NIC: ActiveNicKind = ActiveNicKind::None;

enum ActiveNicKind { None, Rtl8139, E1000, R8169 }

fn active_mac() -> Option<[u8; 6]> {
    unsafe { match ACTIVE_NIC {
        ActiveNicKind::Rtl8139 => rtl8139::nic().map(|n| n.mac),
        ActiveNicKind::E1000   => e1000::nic().map(|n| n.mac),
        ActiveNicKind::R8169   => r8169::nic().map(|n| n.mac),
        ActiveNicKind::None    => None,
    }}
}

fn nic_send(frame: &[u8]) {
    unsafe { match ACTIVE_NIC {
        ActiveNicKind::Rtl8139 => { if let Some(n) = rtl8139::nic() { n.send(frame); } }
        ActiveNicKind::E1000   => { if let Some(n) = e1000::nic()   { n.send(frame); } }
        ActiveNicKind::R8169   => { if let Some(n) = r8169::nic()   { n.send(frame); } }
        ActiveNicKind::None    => {}
    }}
}

fn nic_poll_rx(buf: &mut [u8]) -> Option<usize> {
    unsafe { match ACTIVE_NIC {
        ActiveNicKind::Rtl8139 => rtl8139::nic()?.poll_rx(buf),
        ActiveNicKind::E1000   => e1000::nic()?.poll_rx(buf),
        ActiveNicKind::R8169   => r8169::nic()?.poll_rx(buf),
        ActiveNicKind::None    => None,
    }}
}

// Proxy NIC object with the same API used everywhere in this file.
struct Nic { mac: [u8; 6] }
impl Nic {
    fn send(&self, frame: &[u8]) { nic_send(frame); }
    fn poll_rx(&self, buf: &mut [u8]) -> Option<usize> { nic_poll_rx(buf) }
}
fn nic() -> Option<Nic> { active_mac().map(|m| Nic { mac: m }) }

// QEMU user-mode networking (SLIRP) defaults: guest 10.0.2.15, gateway 10.0.2.2.
const OUR_IP: [u8; 4] = [10, 0, 2, 15];
const GW_IP:  [u8; 4] = [10, 0, 2, 2];
const BCAST:  [u8; 6] = [0xFF; 6];
const ETHERTYPE_ARP: u16 = 0x0806;
const ETHERTYPE_IP:  u16 = 0x0800;
const PROTO_ICMP:    u8  = 1;
const PROTO_UDP:     u8  = 17;
const PROTO_TCP:     u8  = 6;

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

fn log_dec(mut n: u32) {
    let mut buf = [0u8; 10];
    let mut i = 10;
    if n == 0 { crate::serial::write_str("0"); return; }
    while n > 0 && i > 0 { i -= 1; buf[i] = b'0' + (n % 10) as u8; n /= 10; }
    crate::serial::write_str(core::str::from_utf8(&buf[i..]).unwrap_or("?"));
}

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

/// ARP-resolve any on-link IP. Returns the host's MAC if a reply arrives within
/// the poll budget. Used for the gateway and the DNS server.
pub fn arp_resolve(target_ip: &[u8; 4]) -> Option<[u8; 6]> {
    let nic = nic()?;

    let mut frame = [0u8; 42];
    build_arp_request(&mut frame, &nic.mac, target_ip);
    nic.send(&frame);
    crate::serial::write_str("  [net] ARP who-has ");
    let mut ipbuf = [0u8; 15]; // dotted-quad for the log line
    let mut p = 0;
    for (i, &b) in target_ip.iter().enumerate() {
        if b >= 100 { ipbuf[p] = b'0' + b / 100; p += 1; }
        if b >= 10  { ipbuf[p] = b'0' + (b / 10) % 10; p += 1; }
        ipbuf[p] = b'0' + b % 10; p += 1;
        if i < 3 { ipbuf[p] = b'.'; p += 1; }
    }
    crate::serial::write_str(core::str::from_utf8(&ipbuf[..p]).unwrap_or("?"));
    crate::serial::write_str(" sent\n");


    let mut rx = [0u8; 1536];
    for _ in 0..10_000_000u32 {
        if let Some(len) = nic.poll_rx(&mut rx) {
            if len >= 42
                && rx[12] == 0x08 && rx[13] == 0x06          // ethertype ARP
                && rx[20] == 0x00 && rx[21] == 0x02           // op = reply
                && rx[28..32] == *target_ip                   // sender IP = our target
            {
                let mut mac = [0u8; 6];
                mac.copy_from_slice(&rx[22..28]);             // ARP sender MAC
                log_mac("  [net] resolved MAC ", &mac);
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
    let nic = match nic() { Some(n) => n, None => return false };
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
    for _ in 0..10_000_000u32 {
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
    let nic = nic()?;
    let mac = nic.mac;
    let mut f = [0u8; 400];
    let mut rx = [0u8; 1536];

    // DISCOVER → OFFER
    let n = build_dhcp(&mut f, &mac, 1, None, None);
    nic.send(&f[..n]);
    crate::serial::write_str("  [net] DHCP DISCOVER sent\n");
    let mut offer = None;
    for _ in 0..10_000_000u32 {
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
    for _ in 0..10_000_000u32 {
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

// ── TCP + HTTP ────────────────────────────────────────────────────────────────
// A minimal one-shot TCP client: 3-way handshake, send an HTTP GET, ACK the
// response stream, log the status line, then FIN. No retransmit/reordering —
// fine over lossless in-order SLIRP. TCP checksum covers the IPv4 pseudo-header.
const TCP_FIN: u8 = 0x01;
const TCP_SYN: u8 = 0x02;
const TCP_RST: u8 = 0x04;
const TCP_PSH: u8 = 0x08;
const TCP_ACK: u8 = 0x10;
const OUR_PORT: u16 = 49152;

fn tcp_checksum(src: &[u8; 4], dst: &[u8; 4], seg: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    sum += ((src[0] as u32) << 8) | src[1] as u32;
    sum += ((src[2] as u32) << 8) | src[3] as u32;
    sum += ((dst[0] as u32) << 8) | dst[1] as u32;
    sum += ((dst[2] as u32) << 8) | dst[3] as u32;
    sum += PROTO_TCP as u32;
    sum += seg.len() as u32;
    let mut i = 0;
    while i + 1 < seg.len() { sum += ((seg[i] as u32) << 8) | seg[i + 1] as u32; i += 2; }
    if i < seg.len() { sum += (seg[i] as u32) << 8; }
    while sum >> 16 != 0 { sum = (sum & 0xFFFF) + (sum >> 16); }
    !(sum as u16)
}

/// Build an Ethernet+IPv4+TCP frame into `f`; returns its length.
fn build_tcp(f: &mut [u8], dst_mac: &[u8; 6], src_mac: &[u8; 6],
             src_ip: &[u8; 4], dst_ip: &[u8; 4], dport: u16,
             seq: u32, ack: u32, flags: u8, payload: &[u8]) -> usize {
    for b in f.iter_mut() { *b = 0; }
    f[0..6].copy_from_slice(dst_mac);
    f[6..12].copy_from_slice(src_mac);
    f[12] = 0x08; f[13] = 0x00;

    let t = 34; // TCP header offset
    f[t]     = (OUR_PORT >> 8) as u8; f[t + 1] = OUR_PORT as u8;
    f[t + 2] = (dport >> 8) as u8;    f[t + 3] = dport as u8;
    f[t + 4..t + 8].copy_from_slice(&seq.to_be_bytes());
    f[t + 8..t + 12].copy_from_slice(&ack.to_be_bytes());
    f[t + 12] = 5 << 4;               // data offset = 5 words (20 bytes)
    f[t + 13] = flags;
    f[t + 14] = 0x20; f[t + 15] = 0x00; // window 8192
    let plen = payload.len();
    f[t + 20..t + 20 + plen].copy_from_slice(payload);
    let tcp_len = 20 + plen;
    let ck = tcp_checksum(src_ip, dst_ip, &f[t..t + tcp_len]);
    f[t + 16] = (ck >> 8) as u8; f[t + 17] = ck as u8;

    // IPv4
    f[14] = 0x45;
    let total = (20 + tcp_len) as u16;
    f[16] = (total >> 8) as u8; f[17] = total as u8;
    f[20] = 0x40; f[22] = 64; f[23] = PROTO_TCP;
    f[26..30].copy_from_slice(src_ip);
    f[30..34].copy_from_slice(dst_ip);
    let ipck = inet_checksum(&f[14..34]);
    f[24] = (ipck >> 8) as u8; f[25] = ipck as u8;
    14 + total as usize
}

// Is `rx[..len]` a TCP segment addressed to our client port? If so return
// (their_seq, their_ack, flags, payload_offset, payload_len).
fn parse_tcp(rx: &[u8], len: usize) -> Option<(u32, u32, u8, usize, usize)> {
    if len < 54 { return None; }
    if !(rx[12] == 0x08 && rx[13] == 0x00 && rx[23] == PROTO_TCP) { return None; }
    if !(rx[36] == (OUR_PORT >> 8) as u8 && rx[37] == OUR_PORT as u8) { return None; }
    let seq = u32::from_be_bytes([rx[38], rx[39], rx[40], rx[41]]);
    let ackn = u32::from_be_bytes([rx[42], rx[43], rx[44], rx[45]]);
    let flags = rx[47];
    let data_off = ((rx[46] >> 4) as usize) * 4;
    let poff = 34 + data_off;
    let ip_total = u16::from_be_bytes([rx[16], rx[17]]) as usize;
    let seg_end = (14 + ip_total).min(len);
    let plen = seg_end.saturating_sub(poff);
    Some((seq, ackn, flags, poff, plen))
}

/// Fetch `GET /` from dst_ip:dport over TCP, sending `host` in the Host header.
/// Captures the response (headers+body, truncated to `out`) and returns the
/// number of bytes captured, or `None` on failure. `next_mac` is the on-link
/// next hop — the SLIRP gateway for any remote (off-link) destination.
fn tcp_http_get(next_mac: &[u8; 6], dst_ip: [u8; 4], dport: u16, host: &str,
                path: &str, out: &mut [u8]) -> Option<usize> {
    let gw_mac = next_mac;
    let nic = match nic() { Some(n) => n, None => return None };
    let mac = nic.mac;
    let mut f = [0u8; 256];
    let mut rx = [0u8; 1536];
    let isn: u32 = 0x0001_2000;
    let mut written = 0usize; // bytes copied into `out`

    // SYN
    let n = build_tcp(&mut f, gw_mac, &mac, &OUR_IP, &dst_ip, dport, isn, 0, TCP_SYN, &[]);
    nic.send(&f[..n]);
    crate::serial::write_str("  [net] TCP SYN -> ");
    crate::serial::write_str(host);
    crate::serial::write_str("\n");

    // SYN-ACK
    let mut their_seq = 0u32;
    let mut established = false;
    for _ in 0..10_000_000u32 {
        if let Some(l) = nic.poll_rx(&mut rx) {
            if let Some((seq, ackn, flags, _, _)) = parse_tcp(&rx, l) {
                if flags & TCP_RST != 0 {
                    crate::serial::write_str("  [net] TCP connection refused (RST)\n");
                    return None;
                }
                if flags & TCP_SYN != 0 && flags & TCP_ACK != 0 && ackn == isn.wrapping_add(1) {
                    their_seq = seq;
                    established = true;
                    break;
                }
            }
        }
    }
    if !established { crate::serial::write_str("  [net] no SYN-ACK (timeout)\n"); return None; }

    let mut rcv_nxt = their_seq.wrapping_add(1);
    let snd = isn.wrapping_add(1);
    // ACK the handshake, then PSH the HTTP request.
    let n = build_tcp(&mut f, gw_mac, &mac, &OUR_IP, &dst_ip, dport, snd, rcv_nxt, TCP_ACK, &[]);
    nic.send(&f[..n]);
    // Build "GET <path> HTTP/1.0\r\nHost: <host>\r\nConnection: close\r\n\r\n".
    let mut reqbuf = [0u8; 384];
    let mut rl = 0;
    let safe_path = if path.is_empty() { "/" } else { path };
    for part in [b"GET ".as_slice(), safe_path.as_bytes(),
                 b" HTTP/1.0\r\nHost: ".as_slice(), host.as_bytes(),
                 b"\r\nUser-Agent: RustyPenguin/2.1\r\nConnection: close\r\n\r\n".as_slice()] {
        let take = part.len().min(reqbuf.len() - rl);
        reqbuf[rl..rl + take].copy_from_slice(&part[..take]);
        rl += take;
    }
    let req = &reqbuf[..rl];
    let n = build_tcp(&mut f, gw_mac, &mac, &OUR_IP, &dst_ip, dport, snd, rcv_nxt, TCP_PSH | TCP_ACK, req);
    nic.send(&f[..n]);
    crate::serial::write_str("  [net] HTTP GET sent\n");

    // Receive the response stream; ACK in-order data, log the status line.
    let mut total = 0usize;
    let mut logged = false;
    let snd_after = snd.wrapping_add(req.len() as u32);
    for _ in 0..15_000_000u32 {
        if let Some(l) = nic.poll_rx(&mut rx) {
            if let Some((seq, _ackn, flags, poff, plen)) = parse_tcp(&rx, l) {
                if plen > 0 && seq == rcv_nxt {
                    if !logged {
                        // log the HTTP status line (up to CRLF)
                        let mut end = poff;
                        while end < poff + plen && end + 1 < rx.len()
                            && !(rx[end] == b'\r' && rx[end + 1] == b'\n') { end += 1; }
                        crate::serial::write_str("  [net] <- ");
                        crate::serial::write_str(core::str::from_utf8(&rx[poff..end]).unwrap_or("?"));
                        crate::serial::write_str("\n");
                        logged = true;
                    }
                    total += plen;
                    // Capture the response into the caller's buffer (truncate).
                    if written < out.len() {
                        let take = plen.min(out.len() - written);
                        out[written..written + take].copy_from_slice(&rx[poff..poff + take]);
                        written += take;
                    }
                    rcv_nxt = rcv_nxt.wrapping_add(plen as u32);
                    let n = build_tcp(&mut f, gw_mac, &mac, &OUR_IP, &dst_ip, dport, snd_after, rcv_nxt, TCP_ACK, &[]);
                    nic.send(&f[..n]);
                }
                if flags & TCP_FIN != 0 {
                    rcv_nxt = rcv_nxt.wrapping_add(1);
                    // ACK the FIN, then close our side.
                    let n = build_tcp(&mut f, gw_mac, &mac, &OUR_IP, &dst_ip, dport, snd_after, rcv_nxt, TCP_ACK, &[]);
                    nic.send(&f[..n]);
                    let n = build_tcp(&mut f, gw_mac, &mac, &OUR_IP, &dst_ip, dport, snd_after, rcv_nxt, TCP_FIN | TCP_ACK, &[]);
                    nic.send(&f[..n]);
                    break;
                }
            }
        }
    }
    if total > 0 {
        crate::serial::write_str("  [net] HTTP body ");
        log_dec(total as u32);
        crate::serial::write_str(" bytes received (TCP fetch OK)\n");
        Some(written)
    } else {
        crate::serial::write_str("  [net] TCP: no data received\n");
        None
    }
}

// ── Stateful TCP connection (for the TLS client) ────────────────────────────
// The one-shot tcp_http_get above can't drive a multi-flight protocol. TcpConn
// holds the sequence/ack state so tls.rs can send and receive records across
// several round trips. SLIRP is lossless + in-order, so reassembly stays simple.

const TCP_MSS: usize = 1460;

pub struct TcpConn {
    gw_mac: [u8; 6],
    our_mac: [u8; 6],
    dst_ip: [u8; 4],
    dport: u16,
    snd: u32,        // our next sequence number
    rcv: u32,        // next sequence we expect from the peer
    pending_fin: bool,
    pub closed: bool,
}

impl TcpConn {
    /// Open a connection to dst_ip:dport via the cached gateway. Performs the
    /// SYN / SYN-ACK / ACK handshake. Returns None on timeout or refusal.
    pub fn connect(dst_ip: [u8; 4], dport: u16) -> Option<Self> {
        let gw = unsafe { if !NET_UP { return None; } GW_MAC? };
        let nic = nic()?;
        let our_mac = nic.mac;
        let mut f = [0u8; 1600];
        let mut rx = [0u8; 1600];
        let isn: u32 = 0x00C0_FFEE;
        let n = build_tcp(&mut f, &gw, &our_mac, &OUR_IP, &dst_ip, dport, isn, 0, TCP_SYN, &[]);
        nic.send(&f[..n]);
        let mut their_seq = 0u32;
        let mut ok = false;
        for _ in 0..15_000_000u32 {
            if let Some(l) = nic.poll_rx(&mut rx) {
                if let Some((seq, ackn, flags, _, _)) = parse_tcp(&rx, l) {
                    if flags & TCP_RST != 0 { return None; }
                    if flags & TCP_SYN != 0 && flags & TCP_ACK != 0 && ackn == isn.wrapping_add(1) {
                        their_seq = seq; ok = true; break;
                    }
                }
            }
        }
        if !ok { return None; }
        let rcv = their_seq.wrapping_add(1);
        let snd = isn.wrapping_add(1);
        let n = build_tcp(&mut f, &gw, &our_mac, &OUR_IP, &dst_ip, dport, snd, rcv, TCP_ACK, &[]);
        nic.send(&f[..n]);
        Some(TcpConn { gw_mac: gw, our_mac, dst_ip, dport, snd, rcv, pending_fin: false, closed: false })
    }

    /// Send `data`, segmenting at the MSS. Returns false if the NIC is gone.
    pub fn send(&mut self, data: &[u8]) -> bool {
        let nic = match nic() { Some(n) => n, None => return false };
        let mut f = [0u8; 1600];
        let mut off = 0;
        while off < data.len() {
            let take = (data.len() - off).min(TCP_MSS);
            let n = build_tcp(&mut f, &self.gw_mac, &self.our_mac, &OUR_IP, &self.dst_ip,
                              self.dport, self.snd, self.rcv, TCP_PSH | TCP_ACK, &data[off..off+take]);
            nic.send(&f[..n]);
            self.snd = self.snd.wrapping_add(take as u32);
            off += take;
        }
        true
    }

    /// Receive in-order bytes into `buf`. Returns Some(n>0) on data, Some(0) on
    /// peer FIN (connection closing), or None on timeout. ACKs in-order data.
    pub fn recv(&mut self, buf: &mut [u8]) -> Option<usize> {
        if self.pending_fin { self.pending_fin = false; self.closed = true; return Some(0); }
        let nic = nic()?;
        let mut f = [0u8; 1600];
        let mut rx = [0u8; 1600];
        for _ in 0..20_000_000u32 {
            if let Some(l) = nic.poll_rx(&mut rx) {
                if let Some((seq, _ackn, flags, poff, plen)) = parse_tcp(&rx, l) {
                    if flags & TCP_RST != 0 { self.closed = true; return Some(0); }
                    if plen > 0 && seq == self.rcv {
                        let take = plen.min(buf.len());
                        buf[..take].copy_from_slice(&rx[poff..poff+take]);
                        self.rcv = self.rcv.wrapping_add(plen as u32);
                        // ACK the data (and note a piggybacked FIN for next call).
                        if flags & TCP_FIN != 0 { self.rcv = self.rcv.wrapping_add(1); self.pending_fin = true; }
                        let n = build_tcp(&mut f, &self.gw_mac, &self.our_mac, &OUR_IP, &self.dst_ip,
                                          self.dport, self.snd, self.rcv, TCP_ACK, &[]);
                        nic.send(&f[..n]);
                        return Some(take);
                    }
                    if flags & TCP_FIN != 0 && seq == self.rcv {
                        self.rcv = self.rcv.wrapping_add(1);
                        let n = build_tcp(&mut f, &self.gw_mac, &self.our_mac, &OUR_IP, &self.dst_ip,
                                          self.dport, self.snd, self.rcv, TCP_ACK, &[]);
                        nic.send(&f[..n]);
                        self.closed = true;
                        return Some(0);
                    }
                }
            }
        }
        None
    }

    /// Close our side (FIN/ACK). Best-effort; does not wait for the peer's FIN.
    pub fn close(&mut self) {
        if self.closed { return; }
        if let Some(nic) = nic() {
            let mut f = [0u8; 1600];
            let n = build_tcp(&mut f, &self.gw_mac, &self.our_mac, &OUR_IP, &self.dst_ip,
                              self.dport, self.snd, self.rcv, TCP_FIN | TCP_ACK, &[]);
            nic.send(&f[..n]);
            self.snd = self.snd.wrapping_add(1);
        }
        self.closed = true;
    }
}

/// Resolve a hostname to IPv4 using the cached DNS server MAC. Public so the
/// TLS path can resolve https hosts the same way http_get does.
pub fn resolve_host(host: &str) -> Option<[u8; 4]> {
    let dns = unsafe { if !NET_UP { return None; } DNS_MAC? };
    dns_resolve(&dns, host)
}

// ── UDP transport + DNS ─────────────────────────────────────────────────────
// Brick 5: resolve a hostname to an IPv4 address over UDP/53. SLIRP runs a DNS
// proxy at 10.0.2.3 that forwards to the host's resolver, so a real-domain
// lookup works whenever the host has outbound DNS. UDP checksum 0 (allowed v4).
const DNS_IP:       [u8; 4] = [10, 0, 2, 3];
const DNS_PORT:     u16     = 53;
const OUR_DNS_PORT: u16     = 49153;

/// Build an Ethernet+IPv4+UDP frame carrying `payload`; returns its length.
fn build_udp(f: &mut [u8], dst_mac: &[u8; 6], src_mac: &[u8; 6],
             src_ip: &[u8; 4], dst_ip: &[u8; 4], sport: u16, dport: u16,
             payload: &[u8]) -> usize {
    for b in f.iter_mut() { *b = 0; }
    f[0..6].copy_from_slice(dst_mac);
    f[6..12].copy_from_slice(src_mac);
    f[12] = 0x08; f[13] = 0x00;

    let u = 34; // UDP header offset
    f[u]     = (sport >> 8) as u8; f[u + 1] = sport as u8;
    f[u + 2] = (dport >> 8) as u8; f[u + 3] = dport as u8;
    let udp_len = (8 + payload.len()) as u16;
    f[u + 4] = (udp_len >> 8) as u8; f[u + 5] = udp_len as u8;
    f[u + 8..u + 8 + payload.len()].copy_from_slice(payload);
    // checksum (f[u+6..u+8]) left 0 — optional for IPv4 UDP

    // IPv4
    f[14] = 0x45;
    let total = (20 + 8 + payload.len()) as u16;
    f[16] = (total >> 8) as u8; f[17] = total as u8;
    f[20] = 0x40; f[22] = 64; f[23] = PROTO_UDP;
    f[26..30].copy_from_slice(src_ip);
    f[30..34].copy_from_slice(dst_ip);
    let ipck = inet_checksum(&f[14..34]);
    f[24] = (ipck >> 8) as u8; f[25] = ipck as u8;
    14 + total as usize
}

/// If `rx[..len]` is a UDP datagram matching the (src,dst) ports, return the
/// payload window (offset, length).
fn parse_udp_payload(rx: &[u8], len: usize, want_sport: u16, want_dport: u16)
    -> Option<(usize, usize)> {
    if len < 42 { return None; }
    if !(rx[12] == 0x08 && rx[13] == 0x00 && rx[23] == PROTO_UDP) { return None; }
    let u = 34;
    let sport = u16::from_be_bytes([rx[u], rx[u + 1]]);
    let dport = u16::from_be_bytes([rx[u + 2], rx[u + 3]]);
    if sport != want_sport || dport != want_dport { return None; }
    let udp_len = u16::from_be_bytes([rx[u + 4], rx[u + 5]]) as usize;
    let poff = u + 8;
    let plen = udp_len.saturating_sub(8).min(len.saturating_sub(poff));
    Some((poff, plen))
}

/// Encode a DNS query for an A record of `domain` into `buf`; returns its length.
fn build_dns_query(buf: &mut [u8], domain: &str, txid: u16) -> usize {
    buf[0] = (txid >> 8) as u8; buf[1] = txid as u8;
    buf[2] = 0x01; buf[3] = 0x00;        // flags: RD (recursion desired)
    buf[4] = 0; buf[5] = 1;              // QDCOUNT = 1
    buf[6] = 0; buf[7] = 0;              // ANCOUNT
    buf[8] = 0; buf[9] = 0;              // NSCOUNT
    buf[10] = 0; buf[11] = 0;            // ARCOUNT
    let mut p = 12;
    for label in domain.split('.') {
        let l = label.as_bytes();
        if l.is_empty() || l.len() > 63 || p + 1 + l.len() >= buf.len() { break; }
        buf[p] = l.len() as u8; p += 1;
        buf[p..p + l.len()].copy_from_slice(l); p += l.len();
    }
    buf[p] = 0; p += 1;                  // root label
    buf[p] = 0; buf[p + 1] = 1; p += 2;  // QTYPE = A
    buf[p] = 0; buf[p + 1] = 1; p += 2;  // QCLASS = IN
    p
}

/// Advance past a DNS name starting at `p` (handles a trailing compression
/// pointer, which terminates the name). Returns the offset just after it.
fn dns_skip_name(msg: &[u8], mut p: usize) -> Option<usize> {
    loop {
        if p >= msg.len() { return None; }
        let len = msg[p];
        if len == 0 { return Some(p + 1); }
        if len & 0xC0 == 0xC0 { return Some(p + 2); } // pointer ends the name
        p += 1 + len as usize;
    }
}

/// Parse the first A record out of a DNS response with our `txid`.
fn parse_dns_a(msg: &[u8], txid: u16) -> Option<[u8; 4]> {
    if msg.len() < 12 { return None; }
    if msg[0] != (txid >> 8) as u8 || msg[1] != txid as u8 { return None; }
    if msg[2] & 0x80 == 0 { return None; }                 // QR = response
    let qd = u16::from_be_bytes([msg[4], msg[5]]);
    let an = u16::from_be_bytes([msg[6], msg[7]]);
    if an == 0 { return None; }
    let mut p = 12;
    for _ in 0..qd {                                       // skip questions
        p = dns_skip_name(msg, p)?;
        p += 4;                                            // QTYPE + QCLASS
    }
    for _ in 0..an {                                       // walk answers
        p = dns_skip_name(msg, p)?;
        if p + 10 > msg.len() { return None; }
        let rtype = u16::from_be_bytes([msg[p], msg[p + 1]]);
        let rdlen = u16::from_be_bytes([msg[p + 8], msg[p + 9]]) as usize;
        p += 10;
        if rtype == 1 && rdlen == 4 && p + 4 <= msg.len() {
            return Some([msg[p], msg[p + 1], msg[p + 2], msg[p + 3]]);
        }
        p += rdlen;                                        // skip non-A (e.g. CNAME)
    }
    None
}

/// Resolve `domain` to an IPv4 address via the DNS server. `dns_mac` is the
/// on-link MAC of the DNS host (from ARP).
fn dns_resolve(dns_mac: &[u8; 6], domain: &str) -> Option<[u8; 4]> {
    let nic = nic()?;
    let mac = nic.mac;
    let txid: u16 = 0x1A2B;
    let mut query = [0u8; 256];
    let qlen = build_dns_query(&mut query, domain, txid);

    let mut f = [0u8; 512];
    let n = build_udp(&mut f, dns_mac, &mac, &OUR_IP, &DNS_IP,
                      OUR_DNS_PORT, DNS_PORT, &query[..qlen]);
    nic.send(&f[..n]);
    crate::serial::write_str("  [net] DNS query A? ");
    crate::serial::write_str(domain);
    crate::serial::write_str(" -> 10.0.2.3\n");

    let mut rx = [0u8; 1536];
    for _ in 0..10_000_000u32 {
        if let Some(l) = nic.poll_rx(&mut rx) {
            if let Some((poff, plen)) = parse_udp_payload(&rx, l, DNS_PORT, OUR_DNS_PORT) {
                if let Some(ip) = parse_dns_a(&rx[poff..poff + plen], txid) {
                    crate::serial::write_str("  [net] DNS ");
                    crate::serial::write_str(domain);
                    log_ip(" -> ", &ip);
                    return Some(ip);
                }
            }
        }
    }
    crate::serial::write_str("  [net] no DNS reply (timeout)\n");
    None
}

// On-link next-hop MACs cached at boot so userspace fetches (http_get) can
// reuse them without re-ARPing every call. virt==phys identity map, single core,
// no preemption during these short routines → plain statics are fine here.
static mut GW_MAC:  Option<[u8; 6]> = None;
static mut DNS_MAC: Option<[u8; 6]> = None;
static mut NET_UP:  bool            = false;

/// Public userspace-facing fetch: resolve `host` and `GET /` it over HTTP/1.0,
/// writing the raw response (headers+body, truncated) into `out`. Returns the
/// number of bytes written, or `None` if the network is down or the fetch fails.
/// Backs the `sys_http_get` syscall (brick 6 — the OS lets programs hit the web).
/// Trust state of the most recent fetch, for the browser's security indicator:
/// +1 = HTTPS with a certificate chain we validated to a trusted root, 0 = plain
/// HTTP (no transport security), -1 = no fetch yet or the last one failed (which,
/// for https, includes a rejected/untrusted certificate). A ternary Trit.
static mut LAST_FETCH_TRUST: i8 = -1;
/// Read the last fetch's trust state (see `LAST_FETCH_TRUST`).
pub fn last_fetch_trust() -> i8 { unsafe { LAST_FETCH_TRUST } }
fn set_trust(t: i8) { unsafe { LAST_FETCH_TRUST = t; } }

pub fn http_get(target: &str, out: &mut [u8]) -> Option<usize> {
    set_trust(-1); // pending; promoted on success below
    let (gw, dns) = unsafe {
        if !NET_UP { return None; }
        (GW_MAC?, DNS_MAC?)
    };
    // Parse an optional scheme. https:// goes straight to the TLS client.
    let (is_https, rest) = if starts_with_ci(target.as_bytes(), b"https://") {
        (true, &target[8..])
    } else if starts_with_ci(target.as_bytes(), b"http://") {
        (false, &target[7..])
    } else {
        (false, target)
    };
    // Split host / path on the first '/'.
    let (in_host, in_path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    if is_https {
        // https_get only returns Some when the cert chain validated to a trusted
        // root — so bytes back here means a genuinely verified secure channel.
        let r = crate::tls::https_get(in_host, in_path, out);
        set_trust(if r.is_some() { 1 } else { -1 });
        return r;
    }
    // Current target host + path; follow up to MAX_REDIR plain-HTTP 3xx hops.
    let mut hostbuf = [0u8; 128];
    let mut pathbuf = [0u8; 256];
    let hl = in_host.len().min(hostbuf.len());
    hostbuf[..hl].copy_from_slice(&in_host.as_bytes()[..hl]);
    let mut host_len = hl;
    let pl0 = in_path.len().min(pathbuf.len());
    pathbuf[..pl0].copy_from_slice(&in_path.as_bytes()[..pl0]);
    let mut path_len = pl0.max(1);
    if pl0 == 0 { pathbuf[0] = b'/'; }

    const MAX_REDIR: usize = 5;
    for hop in 0..=MAX_REDIR {
        let cur_host = core::str::from_utf8(&hostbuf[..host_len]).ok()?;
        let cur_path = core::str::from_utf8(&pathbuf[..path_len]).unwrap_or("/");
        let ip = dns_resolve(&dns, cur_host)?;
        let n = tcp_http_get(&gw, ip, 80, cur_host, cur_path, out)?;
        let resp = &out[..n];
        match http_status(resp) {
            Some(code) if (301..=308).contains(&code) && code != 304 && hop < MAX_REDIR => {
                // Follow the Location header. On plain http:// we re-fetch; on
                // https:// we cannot (no TLS in the HTTP path) — write an honest
                // notice into `out` and stop following.
                let mut nh = [0u8; 128];
                let mut np = [0u8; 256];
                match parse_location(resp, &mut nh, &mut np) {
                    Some((true /* https */, nhl, npl)) => {
                        // Redirect upgraded us to TLS — hand off to the https client.
                        let nh_host = if nhl == 0 { cur_host } else {
                            core::str::from_utf8(&nh[..nhl]).unwrap_or(cur_host)
                        };
                        let npl = npl.max(1);
                        let nh_path = core::str::from_utf8(&np[..npl]).unwrap_or("/");
                        crate::serial::write_str("  [net] redirect -> https, switching to TLS\n");
                        let r = crate::tls::https_get(nh_host, nh_path, out);
                        set_trust(if r.is_some() { 1 } else { -1 });
                        return r;
                    }
                    Some((false, nhl, npl)) => {
                        hostbuf[..nhl].copy_from_slice(&nh[..nhl]);
                        host_len = nhl;
                        let npl = npl.max(1);
                        pathbuf[..npl].copy_from_slice(&np[..npl]);
                        path_len = npl;
                        crate::serial::write_str("  [net] redirect -> ");
                        crate::serial::write_str(core::str::from_utf8(&hostbuf[..host_len]).unwrap_or("?"));
                        crate::serial::write_str("\n");
                        continue;
                    }
                    None => { set_trust(0); return Some(n); } // no Location → show what we got
                }
            }
            _ => { set_trust(0); return Some(n); } // plain HTTP delivered a page
        }
    }
    None
}

/// Parse the numeric HTTP status code from a response's status line
/// ("HTTP/1.1 301 ..." -> 301). Returns None if it doesn't look like HTTP.
fn http_status(resp: &[u8]) -> Option<u16> {
    if !resp.starts_with(b"HTTP/") { return None; }
    // status line: HTTP/x.y SP code SP reason
    let sp = resp.iter().position(|&b| b == b' ')?;
    let rest = &resp[sp + 1..];
    let mut code = 0u16;
    let mut any = false;
    for &b in rest {
        if b.is_ascii_digit() { code = code.wrapping_mul(10) + (b - b'0') as u16; any = true; }
        else { break; }
    }
    if any { Some(code) } else { None }
}

/// Extract the `Location:` header target into (host, path) buffers.
/// Returns (is_https, host_len, path_len). Handles absolute URLs
/// (`http://host/path`) and bare paths (`/path`, relative to current host=unchanged).
fn parse_location(resp: &[u8], host_out: &mut [u8], path_out: &mut [u8]) -> Option<(bool, usize, usize)> {
    // Find "\nlocation:" case-insensitively.
    let loc = find_header(resp, b"location:")?;
    // loc points just past the colon; skip spaces.
    let mut i = 0;
    while i < loc.len() && (loc[i] == b' ' || loc[i] == b'\t') { i += 1; }
    let val = &loc[i..];
    // value ends at CR or LF
    let end = val.iter().position(|&b| b == b'\r' || b == b'\n').unwrap_or(val.len());
    let val = &val[..end];
    if val.is_empty() { return None; }

    let (is_https, after_scheme) = if starts_with_ci(val, b"https://") {
        (true, &val[8..])
    } else if starts_with_ci(val, b"http://") {
        (false, &val[7..])
    } else {
        // Relative path: keep host (signal by host_len 0), copy path.
        let pl = val.len().min(path_out.len());
        path_out[..pl].copy_from_slice(&val[..pl]);
        return Some((false, 0, pl));
    };
    // host runs until '/', '?', or end; path is the remainder (default "/").
    let hsep = after_scheme.iter().position(|&b| b == b'/' || b == b'?').unwrap_or(after_scheme.len());
    let host = &after_scheme[..hsep];
    let path = &after_scheme[hsep..];
    let hl = host.len().min(host_out.len());
    host_out[..hl].copy_from_slice(&host[..hl]);
    if path.is_empty() {
        path_out[0] = b'/';
        Some((is_https, hl, 1))
    } else {
        let pl = path.len().min(path_out.len());
        path_out[..pl].copy_from_slice(&path[..pl]);
        Some((is_https, hl, pl))
    }
}

/// Case-insensitive search for a header line; returns the slice just after the
/// header name + ':' (the value, still including leading spaces and CRLF).
fn find_header<'a>(resp: &'a [u8], name_colon: &[u8]) -> Option<&'a [u8]> {
    let mut i = 0;
    while i + name_colon.len() <= resp.len() {
        // header starts at line beginning (after a \n) or at buffer start
        let at_line_start = i == 0 || resp[i - 1] == b'\n';
        if at_line_start && starts_with_ci(&resp[i..], name_colon) {
            return Some(&resp[i + name_colon.len()..]);
        }
        i += 1;
    }
    None
}

fn starts_with_ci(hay: &[u8], needle: &[u8]) -> bool {
    if hay.len() < needle.len() { return false; }
    for (a, b) in hay.iter().zip(needle.iter()) {
        if a.to_ascii_lowercase() != b.to_ascii_lowercase() { return false; }
    }
    true
}


/// Bring up the NIC, ARP the gateway, then ping it. Ternary result:
/// Pos = ping round-trip OK, Zero = NIC up but ARP/ping failed, Neg = no NIC.
pub fn init() -> ternary_core::Trit {
    use ternary_core::Trit;
    // Probe NICs: RTL8139 (QEMU), then Intel e1000/e1000e, then Realtek r8169.
    // The first found wins; only one NIC is ever active.
    if rtl8139::init() {
        unsafe { ACTIVE_NIC = ActiveNicKind::Rtl8139; }
        crate::serial::write_str("  [net] NIC = RTL8139\n");
    } else if e1000::init() {
        unsafe { ACTIVE_NIC = ActiveNicKind::E1000; }
        crate::serial::write_str("  [net] NIC = Intel e1000\n");
    } else if r8169::init() {
        unsafe { ACTIVE_NIC = ActiveNicKind::R8169; }
        crate::serial::write_str("  [net] NIC = Realtek r8169\n");
    } else {
        return Trit::Neg;
    }
    if let Some(m) = nic().map(|n| n.mac) { log_mac("  [net] our MAC ", &m); }
    let gw = match arp_resolve(&GW_IP) {
        Some(m) => m,
        None    => return Trit::Zero,
    };
    unsafe { GW_MAC = Some(gw); NET_UP = true; }
    let _ = icmp_ping(&gw);   // ping (result logged)
    let lease = dhcp();
    // DNS (brick 5): resolve a real hostname over UDP/53. The DNS host (10.0.2.3)
    // is on-link, so ARP it directly. Real-domain resolves whenever the host has
    // outbound DNS; logged either way and never stalls boot.
    if let Some(dns_mac) = arp_resolve(&DNS_IP) {
        unsafe { DNS_MAC = Some(dns_mac); }
        // Brick 6 proof: exercise the public http_get() wrapper end to end — the
        // exact path the sys_http_get syscall (and the browser) use.
        let mut scratch = [0u8; 1024];
        if let Some(n) = http_get("example.com", &mut scratch) {
            crate::serial::write_str("  [net] http_get(\"example.com\") -> ");
            log_dec(n as u32);
            crate::serial::write_str(" bytes (userspace fetch path OK)\n");
        }
        // Brick 7 proof: real TLS 1.3 handshake + HTTPS GET against the live web,
        // now with full certificate-chain validation. www.google.com chains
        // leaf -> WR2 -> GTS Root R1 (all sha256WithRSA), and GTS Root R1 is in
        // our embedded trust store — so a clean fetch proves the whole trust path
        // (DER parse, RSA-PKCS#1 verify, chain walk, SAN + expiry). A MITM cert
        // would make https_get return None here instead of bytes.
        let mut tlsbuf = [0u8; 2048];
        if let Some(n) = crate::tls::https_get("www.google.com", "/", &mut tlsbuf) {
            crate::serial::write_str("  [net] https_get(\"www.google.com\") -> ");
            log_dec(n as u32);
            crate::serial::write_str(" bytes decrypted (TLS 1.3 + verified cert chain)\n");
            // Log the HTTP status line from inside the encrypted channel.
            let mut end = 0;
            while end + 1 < n && !(tlsbuf[end] == b'\r' && tlsbuf[end+1] == b'\n') { end += 1; }
            crate::serial::write_str("  [net] <- ");
            crate::serial::write_str(core::str::from_utf8(&tlsbuf[..end]).unwrap_or("?"));
            crate::serial::write_str("\n");
        } else {
            crate::serial::write_str("  [net] https_get(\"www.google.com\") failed (cert rejected or no route)\n");
        }
    }
    // Local loopback proof: TCP + HTTP from the host via SLIRP (10.0.2.2:8088).
    // Logged; fails fast with RST if no server is listening, so it never stalls.
    let mut scratch = [0u8; 256];
    let _ = tcp_http_get(&gw, GW_IP, 8088, "10.0.2.2:8088", "/", &mut scratch);
    match lease {
        Some(_) => Trit::Pos, // full DHCP lease — the strongest proof
        None    => Trit::Zero,
    }
}
