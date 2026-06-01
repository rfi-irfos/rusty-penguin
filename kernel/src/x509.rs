//! Minimal X.509 / certificate-chain validation — the trust half of TLS.
//!
//! The handshake in `tls.rs` already does ECDHE + AES-GCM, but without this it
//! accepts *any* certificate, so a MITM with a self-signed cert sails right
//! through. This module closes that hole: parse the DER chain the server sends,
//! verify each signature (RSA-PKCS#1 v1.5 via `bignum`, or ECDSA-P256 via
//! `p256`), walk leaf -> intermediate(s) -> a root in our embedded trust store,
//! and check expiry (against the CMOS clock) and the hostname (SAN dNSName).
//!
//! Scope, honestly: this is signature + chain + validity + hostname. It does
//! NOT yet check basicConstraints CA flags, keyUsage, name constraints, or
//! revocation (CRL/OCSP). Those are real and follow; this is the load-bearing
//! 80% that turns "trusts anything" into "trusts a known root". Self-tested at
//! boot against an embedded chain (and a tampered copy that must be rejected).

extern crate alloc;
use alloc::vec::Vec;

// ---------------------------------------------------------------------------
// DER / ASN.1 — just enough TLV walking for certificates.
// ---------------------------------------------------------------------------

/// One parsed TLV: tag byte, the *content* bytes, and the total encoded length
/// (header + content) so a caller can step to the next sibling.
struct Tlv<'a> {
    tag: u8,
    content: &'a [u8],
    total: usize,
}

/// Read a single DER TLV at the front of `buf`. Returns None on truncation or a
/// length we refuse to parse (multi-byte > 3 length octets — no real cert needs it).
fn tlv(buf: &[u8]) -> Option<Tlv<'_>> {
    if buf.len() < 2 {
        return None;
    }
    let tag = buf[0];
    let b1 = buf[1] as usize;
    let (len, hdr) = if b1 < 0x80 {
        (b1, 2)
    } else {
        let n = b1 & 0x7f;
        if n == 0 || n > 3 || buf.len() < 2 + n {
            return None;
        }
        let mut l = 0usize;
        for i in 0..n {
            l = (l << 8) | buf[2 + i] as usize;
        }
        (l, 2 + n)
    };
    if buf.len() < hdr + len {
        return None;
    }
    Some(Tlv {
        tag,
        content: &buf[hdr..hdr + len],
        total: hdr + len,
    })
}

/// Iterate the TLVs that make up the content of a SEQUENCE/SET.
struct Seq<'a> {
    rest: &'a [u8],
}
impl<'a> Iterator for Seq<'a> {
    type Item = Tlv<'a>;
    fn next(&mut self) -> Option<Tlv<'a>> {
        let t = tlv(self.rest)?;
        self.rest = &self.rest[t.total..];
        Some(t)
    }
}
fn seq(content: &[u8]) -> Seq<'_> {
    Seq { rest: content }
}

const TAG_SEQ: u8 = 0x30;
const TAG_BITSTRING: u8 = 0x03;
const TAG_UTCTIME: u8 = 0x17;
const TAG_GENTIME: u8 = 0x18;
const TAG_CTX0: u8 = 0xa0; // [0] EXPLICIT (version, extensions)
const TAG_CTX3: u8 = 0xa3; // [3] EXPLICIT extensions in TBSCertificate

// OIDs (the body bytes after the 06 LL tag/length).
const OID_RSA_ENC: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01];
const OID_RSA_SHA256: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0b];
const OID_EC_PUBKEY: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01];
const OID_ECDSA_SHA256: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x02];
const OID_SAN: &[u8] = &[0x55, 0x1d, 0x11]; // subjectAltName 2.5.29.17

/// SHA-256 DigestInfo prefix for RSA-PKCS#1 v1.5: SEQ{ SEQ{ oid sha256, NULL }, OCTET-STRING(32) }.
const SHA256_DIGEST_PREFIX: &[u8] = &[
    0x30, 0x31, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01, 0x05,
    0x00, 0x04, 0x20,
];

// ---------------------------------------------------------------------------
// Parsed certificate.
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum KeyKind {
    Rsa,
    EcP256,
    Unknown,
}

/// A certificate, with slices borrowed from the original DER buffer.
struct Cert<'a> {
    tbs: &'a [u8],         // raw TBSCertificate bytes (what the signature covers)
    sig_alg: &'a [u8],     // signatureAlgorithm OID body
    sig: &'a [u8],         // signature BIT STRING content (sans the unused-bits byte)
    issuer: &'a [u8],      // raw issuer Name (the SEQUENCE content)
    subject: &'a [u8],     // raw subject Name (the SEQUENCE content)
    not_before: &'a [u8],  // validity start (raw time bytes)
    not_after: &'a [u8],   // validity end
    key_kind: KeyKind,
    // RSA SPKI:
    rsa_n: &'a [u8],
    rsa_e: &'a [u8],
    // EC SPKI:
    ec_point: &'a [u8],    // uncompressed point 0x04 || X(32) || Y(32)
    san: Option<&'a [u8]>, // raw SubjectAltName extension value (the GeneralNames SEQUENCE)
}

fn parse_cert(der: &[u8]) -> Option<Cert<'_>> {
    // Certificate ::= SEQ { tbsCertificate, signatureAlgorithm, signatureValue }
    let cert = tlv(der)?;
    if cert.tag != TAG_SEQ {
        return None;
    }
    let mut it = seq(cert.content);
    let tbs_tlv = it.next()?;
    if tbs_tlv.tag != TAG_SEQ {
        return None;
    }
    // The signature covers the *encoded* TBS (tag+len+content), so re-slice it
    // out of the parent content by its total length.
    let tbs = &cert.content[..tbs_tlv.total];

    let sigalg_tlv = it.next()?; // SEQUENCE { OID, params }
    let sig_alg = seq(sigalg_tlv.content).next()?.content; // first element = OID body

    let sig_bs = it.next()?; // BIT STRING
    if sig_bs.tag != TAG_BITSTRING || sig_bs.content.is_empty() {
        return None;
    }
    let sig = &sig_bs.content[1..]; // drop the "unused bits" count byte

    // --- walk TBSCertificate ---
    let mut t = seq(tbs_tlv.content);
    let mut first = t.next()?;
    // Optional [0] EXPLICIT version.
    if first.tag == TAG_CTX0 {
        first = t.next()?; // serialNumber
    }
    let _serial = first;
    let _sig_alg_inner = t.next()?; // signature (repeat of alg)
    let issuer_tlv = t.next()?; // issuer Name (SEQUENCE)
    let validity = t.next()?; // SEQUENCE { notBefore, notAfter }
    let subject_tlv = t.next()?; // subject Name (SEQUENCE)
    let spki = t.next()?; // SubjectPublicKeyInfo (SEQUENCE)

    let mut v = seq(validity.content);
    let not_before = v.next()?.content;
    let not_after = v.next()?.content;

    // --- SubjectPublicKeyInfo: { AlgorithmIdentifier, BIT STRING key } ---
    let mut s = seq(spki.content);
    let alg = s.next()?; // SEQUENCE { OID, params }
    let key_bs = s.next()?; // BIT STRING
    if key_bs.tag != TAG_BITSTRING || key_bs.content.is_empty() {
        return None;
    }
    let key_bits = &key_bs.content[1..]; // drop unused-bits byte

    let mut alg_it = seq(alg.content);
    let alg_oid = alg_it.next()?.content;

    let mut key_kind = KeyKind::Unknown;
    let mut rsa_n: &[u8] = &[];
    let mut rsa_e: &[u8] = &[];
    let mut ec_point: &[u8] = &[];

    if alg_oid == OID_RSA_ENC {
        key_kind = KeyKind::Rsa;
        // RSAPublicKey ::= SEQ { modulus INTEGER, publicExponent INTEGER }
        let rsa = tlv(key_bits)?;
        let mut r = seq(rsa.content);
        rsa_n = strip_int(r.next()?.content);
        rsa_e = strip_int(r.next()?.content);
    } else if alg_oid == OID_EC_PUBKEY {
        key_kind = KeyKind::EcP256;
        ec_point = key_bits; // 0x04 || X || Y
    }

    // --- extensions: TBS tail [3] EXPLICIT SEQUENCE OF Extension ---
    let mut san: Option<&[u8]> = None;
    for ext_outer in t {
        if ext_outer.tag == TAG_CTX3 {
            if let Some(ext_seq) = tlv(ext_outer.content) {
                for ext in seq(ext_seq.content) {
                    // Extension ::= SEQ { extnID OID, critical? BOOL, extnValue OCTET STRING }
                    let mut e = seq(ext.content);
                    let oid = match e.next() {
                        Some(x) => x,
                        None => continue,
                    };
                    if oid.content != OID_SAN {
                        continue;
                    }
                    // skip optional critical BOOLEAN, take the OCTET STRING
                    let mut val = e.next();
                    while let Some(ref x) = val {
                        if x.tag == 0x04 {
                            break;
                        }
                        val = e.next();
                    }
                    if let Some(octet) = val {
                        // extnValue wraps a DER GeneralNames SEQUENCE.
                        if let Some(names) = tlv(octet.content) {
                            san = Some(names.content);
                        }
                    }
                }
            }
        }
    }

    Some(Cert {
        tbs,
        sig_alg,
        sig,
        issuer: issuer_tlv.content,
        subject: subject_tlv.content,
        not_before,
        not_after,
        key_kind,
        rsa_n,
        rsa_e,
        ec_point,
        san,
    })
}

/// Strip a leading 0x00 sign-padding byte from a DER positive INTEGER.
fn strip_int(b: &[u8]) -> &[u8] {
    if b.len() > 1 && b[0] == 0x00 {
        &b[1..]
    } else {
        b
    }
}

// ---------------------------------------------------------------------------
// Signature verification.
// ---------------------------------------------------------------------------

/// Verify `cert`'s signature using `issuer`'s public key. Both signature schemes
/// hash the TBS with SHA-256 (we only support the -SHA256 alg OIDs).
fn verify_sig(cert: &Cert, issuer: &Cert) -> bool {
    let hash = crate::crypto::sha256(cert.tbs);
    if cert.sig_alg == OID_RSA_SHA256 {
        if issuer.key_kind != KeyKind::Rsa {
            return false;
        }
        rsa_pkcs1_sha256_verify(issuer.rsa_n, issuer.rsa_e, cert.sig, &hash)
    } else if cert.sig_alg == OID_ECDSA_SHA256 {
        if issuer.key_kind != KeyKind::EcP256 || issuer.ec_point.len() < 65 || issuer.ec_point[0] != 0x04 {
            return false;
        }
        // ECDSA-Sig-Value ::= SEQUENCE { r INTEGER, s INTEGER }
        let sv = match tlv(cert.sig) {
            Some(x) => x,
            None => return false,
        };
        let mut p = seq(sv.content);
        let r = match p.next() {
            Some(x) => x.content,
            None => return false,
        };
        let s = match p.next() {
            Some(x) => x.content,
            None => return false,
        };
        let x = &issuer.ec_point[1..33];
        let y = &issuer.ec_point[33..65];
        crate::p256::verify(x, y, &hash, r, s)
    } else {
        false
    }
}

/// RSA-PKCS#1 v1.5 verify: compute m = sig^e mod n, then check the EMSA-PKCS1
/// padding `00 01 FF..FF 00 || DigestInfo(SHA-256) || hash`.
fn rsa_pkcs1_sha256_verify(n: &[u8], e: &[u8], sig: &[u8], hash: &[u8; 32]) -> bool {
    let n = strip_int(n);
    let k = n.len();
    if sig.len() > k || k < SHA256_DIGEST_PREFIX.len() + 32 + 11 {
        return false;
    }
    let m = crate::bignum::mod_exp(sig, e, n);
    // mod_exp returns limb-padded big-endian; left-pad/trim to k bytes.
    let em = left_pad(&m, k);
    if em.len() != k {
        return false;
    }
    // 00 01 FF.. FF 00 T
    if em[0] != 0x00 || em[1] != 0x01 {
        return false;
    }
    let mut i = 2;
    while i < k && em[i] == 0xff {
        i += 1;
    }
    // need at least 8 padding 0xFF and a 0x00 separator
    if i < 10 || i >= k || em[i] != 0x00 {
        return false;
    }
    i += 1;
    let t = &em[i..];
    if t.len() != SHA256_DIGEST_PREFIX.len() + 32 {
        return false;
    }
    &t[..SHA256_DIGEST_PREFIX.len()] == SHA256_DIGEST_PREFIX
        && &t[SHA256_DIGEST_PREFIX.len()..] == hash
}

/// Left-pad big-endian bytes with leading zeros (or trim leading zeros) to `k`.
fn left_pad(b: &[u8], k: usize) -> Vec<u8> {
    // strip leading zeros first
    let mut s = b;
    while s.len() > k && s[0] == 0 {
        s = &s[1..];
    }
    if s.len() > k {
        return Vec::new(); // value too big for k bytes — invalid
    }
    let mut out = Vec::with_capacity(k);
    for _ in 0..(k - s.len()) {
        out.push(0u8);
    }
    out.extend_from_slice(s);
    out
}

// ---------------------------------------------------------------------------
// Validity (CMOS clock) and hostname (SAN).
// ---------------------------------------------------------------------------

#[inline]
fn cmos_rd(reg: u8) -> u8 {
    unsafe {
        crate::port::outb(0x70, reg);
        crate::port::inb(0x71)
    }
}
fn bcd(v: u8) -> u32 {
    ((v >> 4) * 10 + (v & 0x0f)) as u32
}

/// Pack the RTC into a comparable YYYYMMDDhhmmss integer. Reads twice to avoid a
/// mid-update tear; assumes BCD (QEMU default) and 21st century.
fn now_packed() -> u64 {
    loop {
        // wait out an in-progress update
        while cmos_rd(0x0a) & 0x80 != 0 {}
        let s = cmos_rd(0x00);
        let mi = cmos_rd(0x02);
        let h = cmos_rd(0x04);
        let d = cmos_rd(0x07);
        let mo = cmos_rd(0x08);
        let y = cmos_rd(0x09);
        // re-read to confirm stable
        if cmos_rd(0x0a) & 0x80 != 0 {
            continue;
        }
        let s2 = cmos_rd(0x00);
        if s2 != s {
            continue;
        }
        let year = 2000 + bcd(y);
        return pack(year, bcd(mo), bcd(d), bcd(h), bcd(mi), bcd(s));
    }
}
fn pack(y: u32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> u64 {
    (y as u64) * 10_000_000_000
        + (mo as u64) * 100_000_000
        + (d as u64) * 1_000_000
        + (h as u64) * 10_000
        + (mi as u64) * 100
        + (s as u64)
}

/// Parse a DER UTCTime ("YYMMDDhhmmssZ") or GeneralizedTime ("YYYYMMDDhhmmssZ")
/// into the same packed integer. `tag` distinguishes the two centuries rule.
fn parse_time(tag: u8, b: &[u8]) -> Option<u64> {
    fn d2(b: &[u8], i: usize) -> Option<u32> {
        if i + 1 >= b.len() || !b[i].is_ascii_digit() || !b[i + 1].is_ascii_digit() {
            return None;
        }
        Some(((b[i] - b'0') * 10 + (b[i + 1] - b'0')) as u32)
    }
    let (year, off) = if tag == TAG_GENTIME {
        let hi = d2(b, 0)?;
        let lo = d2(b, 2)?;
        (hi * 100 + lo, 4)
    } else {
        // UTCTime: 2-digit year, <50 => 20xx, >=50 => 19xx
        let yy = d2(b, 0)?;
        (if yy < 50 { 2000 + yy } else { 1900 + yy }, 2)
    };
    let mo = d2(b, off)?;
    let d = d2(b, off + 2)?;
    let h = d2(b, off + 4)?;
    let mi = d2(b, off + 6)?;
    let s = d2(b, off + 8)?;
    Some(pack(year, mo, d, h, mi, s))
}

/// Is `cert` within its validity window right now?
fn check_validity(cert: &Cert, now: u64) -> bool {
    let nb = match parse_time(time_tag(cert.not_before, cert.tbs), cert.not_before) {
        Some(x) => x,
        None => return false,
    };
    let na = match parse_time(time_tag(cert.not_after, cert.tbs), cert.not_after) {
        Some(x) => x,
        None => return false,
    };
    now >= nb && now <= na
}
/// We sliced time *content* out, losing the tag; recover it by length heuristic
/// (UTCTime is 13 bytes incl. 'Z', GeneralizedTime is 15).
fn time_tag(content: &[u8], _tbs: &[u8]) -> u8 {
    if content.len() >= 14 {
        TAG_GENTIME
    } else {
        TAG_UTCTIME
    }
}

/// Match `host` against a SAN dNSName list (supports a single leading `*.` wildcard).
fn hostname_ok(cert: &Cert, host: &[u8]) -> bool {
    let names = match cert.san {
        Some(n) => n,
        None => return false,
    };
    for gn in seq(names) {
        // dNSName is context tag [2] primitive == 0x82
        if gn.tag == 0x82 && dns_match(gn.content, host) {
            return true;
        }
    }
    false
}
fn dns_match(pat: &[u8], host: &[u8]) -> bool {
    if pat.starts_with(b"*.") {
        // wildcard matches exactly one leading label
        let suffix = &pat[1..]; // ".example.com"
        if let Some(dot) = host.iter().position(|&c| c == b'.') {
            return eq_ic(&host[dot..], suffix);
        }
        return false;
    }
    eq_ic(pat, host)
}
fn eq_ic(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter()
        .zip(b.iter())
        .all(|(x, y)| x.to_ascii_lowercase() == y.to_ascii_lowercase())
}

// ---------------------------------------------------------------------------
// Trust store + chain validation.
// ---------------------------------------------------------------------------

/// Embedded trust anchors (DER): real public CA roots plus our self-test root.
/// Real roots let the browser validate live sites; the self-test root proves the
/// machinery. The validation logic is anchor-agnostic — append roots here freely.
fn trust_anchors() -> &'static [&'static [u8]] {
    static ANCHORS: &[&[u8]] = &[
        &crate::ca_roots::GTS_ROOT_R1,
        &crate::ca_roots::ISRG_ROOT_X1,
        &crate::test_certs::TEST_ROOT_DER,
    ];
    ANCHORS
}

/// Do two certs carry the same identity *and* public key? Used to recognise a
/// presented cert as one of our embedded anchors (we trust our embedded copy's
/// key, never the bytes the server happened to send).
fn same_anchor(a: &Cert, b: &Cert) -> bool {
    if a.subject != b.subject || a.key_kind != b.key_kind {
        return false;
    }
    match a.key_kind {
        KeyKind::Rsa => a.rsa_n == b.rsa_n && a.rsa_e == b.rsa_e,
        KeyKind::EcP256 => a.ec_point == b.ec_point,
        KeyKind::Unknown => false,
    }
}

/// Result of validating a presented chain.
#[derive(PartialEq, Clone, Copy, Debug)]
pub enum Verdict {
    Ok,
    Expired,
    BadHostname,
    BadSignature,
    Untrusted, // chain doesn't reach a known root
    Malformed,
}

/// Validate a leaf-first DER chain for `hostname`. `chain[0]` is the server
/// (leaf) cert; the rest are intermediates in order. The root is matched from
/// our embedded store (and need not be sent by the server).
pub fn validate_chain(chain: &[&[u8]], hostname: &[u8]) -> Verdict {
    if chain.is_empty() {
        return Verdict::Malformed;
    }
    let now = now_packed();

    // Parse the presented chain.
    let mut certs: Vec<Cert> = Vec::with_capacity(chain.len());
    for d in chain {
        match parse_cert(d) {
            Some(c) => certs.push(c),
            None => return Verdict::Malformed,
        }
    }

    // Leaf hostname + every cert's validity window.
    if !hostname_ok(&certs[0], hostname) {
        return Verdict::BadHostname;
    }
    for c in &certs {
        if !check_validity(c, now) {
            return Verdict::Expired;
        }
    }

    // Parse trust anchors once.
    let anchors: Vec<Cert> = trust_anchors()
        .iter()
        .filter_map(|d| parse_cert(d))
        .collect();

    // Walk leaf -> up. Two ways the chain can reach trust:
    //   (a) a presented cert IS one of our anchors (same subject + key) — e.g.
    //       Google sends GTS Root R1 in-band. We trust our embedded copy's key,
    //       so once a presented cert matches an anchor we stop: everything below
    //       it has been signature-checked against the next cert up.
    //   (b) the topmost presented cert is signed by an anchor that wasn't sent
    //       (issuer == anchor.subject, and the anchor's key verifies it).
    // Either way, every link below the trust point must verify.
    for i in 0..certs.len() {
        // (a) reached an embedded anchor in-band?
        if anchors.iter().any(|a| same_anchor(a, &certs[i])) {
            return Verdict::Ok;
        }
        if i + 1 < certs.len() {
            // intermediate link: signed by the next presented cert
            if certs[i].issuer != certs[i + 1].subject {
                return Verdict::Untrusted;
            }
            if !verify_sig(&certs[i], &certs[i + 1]) {
                return Verdict::BadSignature;
            }
        } else {
            // (b) top of presented chain: must be signed by an out-of-band anchor
            match anchors.iter().find(|a| a.subject == certs[i].issuer) {
                Some(a) => {
                    if !verify_sig(&certs[i], a) {
                        return Verdict::BadSignature;
                    }
                    return Verdict::Ok;
                }
                None => return Verdict::Untrusted,
            }
        }
    }

    Verdict::Untrusted
}

// ---------------------------------------------------------------------------
// Boot self-test.
// ---------------------------------------------------------------------------

/// Deterministic boot check: validate the embedded leaf->int->root chain (must
/// pass), then a one-byte-tampered leaf (must be rejected with BadSignature).
/// Logs to serial; returns true only if both expectations hold. Hostname and
/// validity are real checks too, so we pass the embedded SAN and rely on the
/// CMOS clock being inside the test certs' 10-year window.
pub fn selftest() -> bool {
    let leaf: &[u8] = &crate::test_certs::TEST_LEAF_DER;
    let int: &[u8] = &crate::test_certs::TEST_INT_DER;
    let chain: [&[u8]; 2] = [leaf, int];
    let host = b"test.rustypenguin";

    let good = validate_chain(&chain, host);

    // Tamper: flip a byte deep inside the leaf's TBS (well past the header) so
    // the signature no longer matches.
    let mut bad_leaf: Vec<u8> = leaf.to_vec();
    let idx = bad_leaf.len() / 2;
    bad_leaf[idx] ^= 0x01;
    let bad_chain: [&[u8]; 2] = [&bad_leaf, int];
    let bad = validate_chain(&bad_chain, host);

    let ok = good == Verdict::Ok && bad != Verdict::Ok;
    if ok {
        crate::serial::write_str("[x509] chain self-test OK (valid chain trusted, tampered rejected)\n");
    } else {
        crate::serial::write_str("[x509] chain self-test FAILED\n");
        match good {
            Verdict::Ok => crate::serial::write_str("  good=Ok\n"),
            Verdict::Expired => crate::serial::write_str("  good=Expired (check RTC year)\n"),
            Verdict::BadHostname => crate::serial::write_str("  good=BadHostname\n"),
            Verdict::BadSignature => crate::serial::write_str("  good=BadSignature\n"),
            Verdict::Untrusted => crate::serial::write_str("  good=Untrusted\n"),
            Verdict::Malformed => crate::serial::write_str("  good=Malformed\n"),
        }
    }
    ok
}
