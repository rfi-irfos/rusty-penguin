//! Minimal TLS 1.3 client (RFC 8446) for HTTPS, built on the from-scratch
//! crypto in crypto.rs and the TcpConn in net.rs. One cipher suite only:
//! TLS_CHACHA20_POLY1305_SHA256 with X25519 key exchange.
//!
//! HONEST SECURITY CAVEAT: this client does NOT validate the server
//! certificate (no CA trust store, no wall clock for expiry checks). It
//! authenticates that the peer completed the key exchange (server Finished is
//! verified), giving confidentiality + integrity against a passive attacker,
//! but NOT protection against an active man-in-the-middle. It is honest hobby
//! TLS — enough to fetch real https:// pages, not enough to bank with. The
//! randomness source is RDTSC-seeded, also not cryptographic-grade.

extern crate alloc;
use alloc::vec::Vec;
use alloc::vec;
use crate::crypto::*;
use crate::net::TcpConn;

const CT_CHANGE_CIPHER_SPEC: u8 = 20;
const CT_ALERT: u8 = 21;
const CT_HANDSHAKE: u8 = 22;
const CT_APPLICATION_DATA: u8 = 23;

const HS_CLIENT_HELLO: u8 = 1;
const HS_SERVER_HELLO: u8 = 2;
const HS_NEW_SESSION_TICKET: u8 = 4;
const HS_ENCRYPTED_EXTENSIONS: u8 = 8;
const HS_CERTIFICATE: u8 = 11;
const HS_CERTIFICATE_VERIFY: u8 = 15;
const HS_FINISHED: u8 = 20;

// ── RNG (RDTSC-seeded SHA-256 stream; not cryptographic) ─────────────────────

fn rand_bytes(out: &mut [u8]) {
    let mut counter: u64 = 0;
    let mut off = 0;
    while off < out.len() {
        let tsc = unsafe { core::arch::x86_64::_rdtsc() };
        let mut seed = [0u8; 24];
        seed[..8].copy_from_slice(&tsc.to_le_bytes());
        seed[8..16].copy_from_slice(&counter.to_le_bytes());
        seed[16..24].copy_from_slice(&unsafe { core::arch::x86_64::_rdtsc() }.to_le_bytes());
        let h = sha256(&seed);
        let take = (out.len() - off).min(32);
        out[off..off+take].copy_from_slice(&h[..take]);
        off += take;
        counter = counter.wrapping_add(1);
    }
}

// ── Buffered TCP stream + TLS record framing ─────────────────────────────────

struct Stream {
    conn: TcpConn,
    buf: Vec<u8>,
    pos: usize,
}

impl Stream {
    fn new(conn: TcpConn) -> Self { Stream { conn, buf: Vec::new(), pos: 0 } }

    /// Fill `out` completely from the TCP stream. False on close/timeout.
    fn read_exact(&mut self, out: &mut [u8]) -> bool {
        let mut got = 0;
        while got < out.len() {
            if self.pos < self.buf.len() {
                let avail = self.buf.len() - self.pos;
                let take = avail.min(out.len() - got);
                out[got..got+take].copy_from_slice(&self.buf[self.pos..self.pos+take]);
                self.pos += take; got += take;
            } else {
                self.buf.clear(); self.pos = 0;
                let mut tmp = [0u8; 1600];
                match self.conn.recv(&mut tmp) {
                    Some(0) | None => return false,
                    Some(k) => self.buf.extend_from_slice(&tmp[..k]),
                }
            }
        }
        true
    }

    /// Read one TLS record. Returns (outer_type, header[5], payload).
    fn read_record(&mut self) -> Option<(u8, [u8; 5], Vec<u8>)> {
        let mut hdr = [0u8; 5];
        if !self.read_exact(&mut hdr) { return None; }
        let len = u16::from_be_bytes([hdr[3], hdr[4]]) as usize;
        if len == 0 || len > 18432 { return None; }
        let mut body = vec![0u8; len];
        if !self.read_exact(&mut body) { return None; }
        Some((hdr[0], hdr, body))
    }

    fn send_raw(&mut self, data: &[u8]) -> bool { self.conn.send(data) }
}

// ── AEAD record helpers ──────────────────────────────────────────────────────

fn make_nonce(iv: &[u8; 12], seq: u64) -> [u8; 12] {
    let mut n = *iv;
    let s = seq.to_be_bytes();
    for i in 0..8 { n[4+i] ^= s[i]; }
    n
}

/// Encrypt `inner` (= content || content_type) into a full TLS ciphertext record.
fn seal_record(key: &[u8; 32], iv: &[u8; 12], seq: u64, content: &[u8], inner_type: u8) -> Vec<u8> {
    let mut plain = Vec::with_capacity(content.len() + 1);
    plain.extend_from_slice(content);
    plain.push(inner_type);
    let total = plain.len() + 16; // + tag
    let mut rec = Vec::with_capacity(5 + total);
    rec.push(CT_APPLICATION_DATA);
    rec.push(0x03); rec.push(0x03);
    rec.push((total >> 8) as u8); rec.push(total as u8);
    let aad = [rec[0], rec[1], rec[2], rec[3], rec[4]];
    let nonce = make_nonce(iv, seq);
    let tag = aead_seal(key, &nonce, &aad, &mut plain);
    rec.extend_from_slice(&plain);
    rec.extend_from_slice(&tag);
    rec
}

/// Decrypt a TLS ciphertext record body. Returns (inner_content_type, plaintext).
fn open_record(key: &[u8; 32], iv: &[u8; 12], seq: u64, hdr: &[u8; 5], body: &[u8]) -> Option<(u8, Vec<u8>)> {
    if body.len() < 16 { return None; }
    let ct_len = body.len() - 16;
    let mut buf = body[..ct_len].to_vec();
    let mut tag = [0u8; 16];
    tag.copy_from_slice(&body[ct_len..]);
    let nonce = make_nonce(iv, seq);
    if !aead_open(key, &nonce, hdr, &mut buf, &tag) { return None; }
    // Strip trailing zero padding; the last non-zero byte is the content type.
    let mut end = buf.len();
    while end > 0 && buf[end-1] == 0 { end -= 1; }
    if end == 0 { return None; }
    let inner_type = buf[end-1];
    buf.truncate(end - 1);
    Some((inner_type, buf))
}

// ── Key schedule helpers ─────────────────────────────────────────────────────

struct Keys { key: [u8; 32], iv: [u8; 12] }

fn traffic_keys(secret: &[u8; 32]) -> Keys {
    let mut key = [0u8; 32];
    let mut iv = [0u8; 12];
    hkdf_expand_label(secret, "key", &[], &mut key);
    hkdf_expand_label(secret, "iv", &[], &mut iv);
    Keys { key, iv }
}

fn empty_hash() -> [u8; 32] { sha256(&[]) }

// ── ClientHello builder ──────────────────────────────────────────────────────

fn push_u16_len<F: FnOnce(&mut Vec<u8>)>(v: &mut Vec<u8>, f: F) {
    let at = v.len();
    v.push(0); v.push(0);
    f(v);
    let len = v.len() - at - 2;
    v[at] = (len >> 8) as u8; v[at+1] = len as u8;
}

fn build_client_hello(host: &str, client_random: &[u8; 32], session_id: &[u8; 32], pubkey: &[u8; 32]) -> Vec<u8> {
    // body of the handshake message (after the 4-byte handshake header)
    let mut b: Vec<u8> = Vec::new();
    b.push(0x03); b.push(0x03);                 // legacy_version TLS 1.2
    b.extend_from_slice(client_random);          // random[32]
    b.push(32); b.extend_from_slice(session_id); // legacy_session_id
    // cipher_suites: TLS_CHACHA20_POLY1305_SHA256
    b.push(0x00); b.push(0x02); b.push(0x13); b.push(0x03);
    // legacy_compression_methods: null
    b.push(0x01); b.push(0x00);
    // extensions
    push_u16_len(&mut b, |e| {
        // server_name (0)
        e.push(0x00); e.push(0x00);
        push_u16_len(e, |x| {
            push_u16_len(x, |list| {
                list.push(0x00); // host_name
                push_u16_len(list, |h| h.extend_from_slice(host.as_bytes()));
            });
        });
        // supported_groups (10): x25519
        e.push(0x00); e.push(0x0a);
        push_u16_len(e, |x| { push_u16_len(x, |g| { g.push(0x00); g.push(0x1d); }); });
        // signature_algorithms (13)
        e.push(0x00); e.push(0x0d);
        push_u16_len(e, |x| {
            push_u16_len(x, |s| {
                for sa in [0x0403u16, 0x0804, 0x0401, 0x0805, 0x0806, 0x0503, 0x0603, 0x0201] {
                    s.push((sa >> 8) as u8); s.push(sa as u8);
                }
            });
        });
        // supported_versions (43): TLS 1.3
        e.push(0x00); e.push(0x2b);
        push_u16_len(e, |x| { x.push(0x02); x.push(0x03); x.push(0x04); });
        // psk_key_exchange_modes (45): psk_dhe_ke (some servers want it present)
        e.push(0x00); e.push(0x2d);
        push_u16_len(e, |x| { x.push(0x01); x.push(0x01); });
        // key_share (51): x25519
        e.push(0x00); e.push(0x33);
        push_u16_len(e, |x| {
            push_u16_len(x, |ks| {
                ks.push(0x00); ks.push(0x1d);
                push_u16_len(ks, |k| k.extend_from_slice(pubkey));
            });
        });
    });
    // wrap in handshake header
    let mut msg = Vec::with_capacity(b.len() + 4);
    msg.push(HS_CLIENT_HELLO);
    msg.push((b.len() >> 16) as u8); msg.push((b.len() >> 8) as u8); msg.push(b.len() as u8);
    msg.extend_from_slice(&b);
    msg
}

/// Extract the server's X25519 public key from a ServerHello handshake body.
fn parse_server_hello(msg: &[u8]) -> Option<[u8; 32]> {
    // msg starts at handshake header [type=2, len(3)]
    if msg.len() < 4 || msg[0] != HS_SERVER_HELLO { return None; }
    let body = &msg[4..];
    let mut i = 0;
    i += 2;                       // legacy_version
    i += 32;                      // random
    if i >= body.len() { return None; }
    let sid_len = body[i] as usize; i += 1 + sid_len;   // legacy_session_id_echo
    i += 2;                       // cipher_suite
    i += 1;                       // legacy_compression_method
    if i + 2 > body.len() { return None; }
    let ext_total = u16::from_be_bytes([body[i], body[i+1]]) as usize; i += 2;
    let ext_end = (i + ext_total).min(body.len());
    while i + 4 <= ext_end {
        let etype = u16::from_be_bytes([body[i], body[i+1]]);
        let elen = u16::from_be_bytes([body[i+2], body[i+3]]) as usize;
        i += 4;
        if i + elen > ext_end { break; }
        if etype == 0x0033 {      // key_share
            // server share: group(2) + len(2) + key
            if elen >= 4 {
                let klen = u16::from_be_bytes([body[i+2], body[i+3]]) as usize;
                if klen == 32 && i + 4 + 32 <= body.len() {
                    let mut k = [0u8; 32];
                    k.copy_from_slice(&body[i+4..i+4+32]);
                    return Some(k);
                }
            }
        }
        i += elen;
    }
    None
}

// ── The handshake ────────────────────────────────────────────────────────────

/// Fetch `https://host{path}` and write the raw HTTP response (headers+body,
/// truncated to `out`) into `out`. Returns bytes written, or None on failure.
pub fn https_get(host: &str, path: &str, out: &mut [u8]) -> Option<usize> {
    let ip = crate::net::resolve_host(host)?;
    let conn = TcpConn::connect(ip, 443)?;
    crate::serial::write_str("  [tls] TCP 443 connected, starting handshake\n");
    let mut s = Stream::new(conn);

    // 1. Keypair + ClientHello
    let mut priv_key = [0u8; 32];
    let mut client_random = [0u8; 32];
    let mut session_id = [0u8; 32];
    rand_bytes(&mut priv_key);
    rand_bytes(&mut client_random);
    rand_bytes(&mut session_id);
    let mut pubkey = [0u8; 32];
    x25519_base(&mut pubkey, &priv_key);

    let ch = build_client_hello(host, &client_random, &session_id, &pubkey);
    let mut transcript = Sha256::new();
    transcript.update(&ch);
    // CH record (plaintext handshake)
    let mut ch_rec = Vec::with_capacity(ch.len() + 5);
    ch_rec.push(CT_HANDSHAKE); ch_rec.push(0x03); ch_rec.push(0x01);
    ch_rec.push((ch.len() >> 8) as u8); ch_rec.push(ch.len() as u8);
    ch_rec.extend_from_slice(&ch);
    if !s.send_raw(&ch_rec) { return None; }

    // 2. ServerHello (plaintext handshake record)
    let server_pub;
    loop {
        let (typ, _hdr, body) = s.read_record()?;
        if typ == CT_CHANGE_CIPHER_SPEC { continue; }
        if typ == CT_ALERT { crate::serial::write_str("  [tls] alert during ServerHello\n"); return None; }
        if typ == CT_HANDSHAKE {
            transcript.update(&body);
            match parse_server_hello(&body) {
                Some(k) => { server_pub = k; break; }
                None => { crate::serial::write_str("  [tls] bad ServerHello (HRR/unsupported group?)\n"); return None; }
            }
        } else { return None; }
    }
    crate::serial::write_str("  [tls] ServerHello OK (x25519, ChaCha20-Poly1305)\n");

    // 3. Key schedule — handshake secrets
    let th_chsh = transcript.clone().finalize();
    let mut shared = [0u8; 32];
    x25519(&mut shared, &priv_key, &server_pub);

    let zero = [0u8; 32];
    let early_secret = hkdf_extract(&zero, &zero);
    let derived = derive_secret(&early_secret, "derived", &empty_hash());
    let handshake_secret = hkdf_extract(&derived, &shared);
    let c_hs_secret = derive_secret(&handshake_secret, "c hs traffic", &th_chsh);
    let s_hs_secret = derive_secret(&handshake_secret, "s hs traffic", &th_chsh);
    let c_hs = traffic_keys(&c_hs_secret);
    let s_hs = traffic_keys(&s_hs_secret);

    // Send ChangeCipherSpec for middlebox compatibility (not in transcript).
    let _ = s.send_raw(&[CT_CHANGE_CIPHER_SPEC, 0x03, 0x03, 0x00, 0x01, 0x01]);

    // 4. Read encrypted server flight: EE, Certificate, CertVerify, Finished.
    let mut srv_seq: u64 = 0;
    let mut hs_acc: Vec<u8> = Vec::new();
    let mut th_before_sfin = [0u8; 32];
    let mut got_finished = false;
    let mut guard = 0;
    while !got_finished {
        guard += 1; if guard > 64 { return None; }
        let (typ, hdr, body) = s.read_record()?;
        if typ == CT_CHANGE_CIPHER_SPEC { continue; }
        if typ == CT_ALERT { crate::serial::write_str("  [tls] alert in server flight\n"); return None; }
        if typ != CT_APPLICATION_DATA { return None; }
        let (inner_type, plain) = open_record(&s_hs.key, &s_hs.iv, srv_seq, &hdr, &body)?;
        srv_seq += 1;
        if inner_type == CT_ALERT { crate::serial::write_str("  [tls] encrypted alert\n"); return None; }
        if inner_type != CT_HANDSHAKE { continue; }
        hs_acc.extend_from_slice(&plain);
        // Parse out complete handshake messages.
        loop {
            if hs_acc.len() < 4 { break; }
            let mlen = ((hs_acc[1] as usize) << 16) | ((hs_acc[2] as usize) << 8) | hs_acc[3] as usize;
            if hs_acc.len() < 4 + mlen { break; }
            let mtype = hs_acc[0];
            let msg: Vec<u8> = hs_acc[..4+mlen].to_vec();
            // remove consumed bytes
            hs_acc.drain(0..4+mlen);
            if mtype == HS_FINISHED {
                // verify server Finished BEFORE folding it into the transcript
                th_before_sfin = transcript.clone().finalize();
                let mut fin_key = [0u8; 32];
                hkdf_expand_label(&s_hs_secret, "finished", &[], &mut fin_key);
                let expect = hmac_sha256(&fin_key, &th_before_sfin);
                if msg.len() < 4 + 32 || &expect[..] != &msg[4..4+32] {
                    crate::serial::write_str("  [tls] server Finished MAC mismatch\n");
                    return None;
                }
                transcript.update(&msg);
                got_finished = true;
                break;
            } else {
                // EE / Certificate / CertVerify — folded in, not validated.
                let _ = mtype;
                transcript.update(&msg);
            }
        }
    }
    crate::serial::write_str("  [tls] server Finished verified; handshake complete\n");

    // 5. transcript through server Finished → app secrets + client Finished
    let th_sfin = transcript.clone().finalize();
    let derived2 = derive_secret(&handshake_secret, "derived", &empty_hash());
    let master_secret = hkdf_extract(&derived2, &zero);
    let c_ap_secret = derive_secret(&master_secret, "c ap traffic", &th_sfin);
    let s_ap_secret = derive_secret(&master_secret, "s ap traffic", &th_sfin);
    let c_ap = traffic_keys(&c_ap_secret);
    let s_ap = traffic_keys(&s_ap_secret);

    // client Finished
    let mut c_fin_key = [0u8; 32];
    hkdf_expand_label(&c_hs_secret, "finished", &[], &mut c_fin_key);
    let verify = hmac_sha256(&c_fin_key, &th_sfin);
    let mut fin_msg = Vec::with_capacity(4 + 32);
    fin_msg.push(HS_FINISHED); fin_msg.push(0); fin_msg.push(0); fin_msg.push(32);
    fin_msg.extend_from_slice(&verify);
    let fin_rec = seal_record(&c_hs.key, &c_hs.iv, 0, &fin_msg, CT_HANDSHAKE);
    if !s.send_raw(&fin_rec) { return None; }

    // 6. Send the HTTP request under the client application key.
    let mut req: Vec<u8> = Vec::new();
    let safe_path = if path.is_empty() { "/" } else { path };
    req.extend_from_slice(b"GET ");
    req.extend_from_slice(safe_path.as_bytes());
    req.extend_from_slice(b" HTTP/1.1\r\nHost: ");
    req.extend_from_slice(host.as_bytes());
    req.extend_from_slice(b"\r\nUser-Agent: RustyPenguin/2.1\r\nAccept: */*\r\nConnection: close\r\n\r\n");
    let req_rec = seal_record(&c_ap.key, &c_ap.iv, 0, &req, CT_APPLICATION_DATA);
    if !s.send_raw(&req_rec) { return None; }
    crate::serial::write_str("  [tls] HTTPS request sent; reading response\n");

    // 7. Read application data response (skip post-handshake tickets).
    let mut rcv_seq: u64 = 0;
    let mut written = 0usize;
    let mut guard2 = 0;
    loop {
        guard2 += 1; if guard2 > 4096 { break; }
        let rec = match s.read_record() { Some(r) => r, None => break };
        let (typ, hdr, body) = rec;
        if typ == CT_CHANGE_CIPHER_SPEC { continue; }
        if typ != CT_APPLICATION_DATA { break; }
        let (inner_type, plain) = match open_record(&s_ap.key, &s_ap.iv, rcv_seq, &hdr, &body) {
            Some(p) => p, None => { crate::serial::write_str("  [tls] decrypt failed in app data\n"); break; }
        };
        rcv_seq += 1;
        match inner_type {
            CT_APPLICATION_DATA => {
                let take = plain.len().min(out.len() - written);
                out[written..written+take].copy_from_slice(&plain[..take]);
                written += take;
                if written >= out.len() { break; }
            }
            CT_HANDSHAKE => { /* NewSessionTicket etc — ignore */ }
            CT_ALERT => break, // close_notify or error
            _ => {}
        }
    }
    s.conn.close();
    if written > 0 {
        crate::serial::write_str("  [tls] response ");
        crate::serial::write_str("received\n");
        Some(written)
    } else {
        None
    }
}
