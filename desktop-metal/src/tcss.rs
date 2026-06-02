// Ternary CSS engine — brick 2: tokenizer + parser + cascade.
//
#![allow(dead_code)] // public API + self_test wired in but not yet called from main
//
// A from-scratch, no_std CSS engine that goes beyond css.rs (which styles
// native panels). This brick parses a real CSS subset with *selectors* and
// resolves the cascade for an HTML-ish element descriptor + inline style into
// a `ComputedStyle`. It is the foundation for rendering CSS-styled DOM content
// (the native browser track), not just hardcoded desktop chrome.
//
// Supported:
//   selectors: tag (`p`,`h1`), class (`.x`), id (`#y`), universal (`*`)
//   declarations: color, font-size, font-weight, text-align, display
//   value forms: #rgb / #rrggbb, ~16 named colors, font-size keywords + px
//                buckets, font-weight:bold, text-align:left|center|right,
//                display:none
//   comments: /* ... */
//
// Ternary angle (honest, see SPEC_TRIT below):
//   - `text_align` is genuinely tri-valued: -1 left / 0 center / +1 right,
//     carried in one balanced-ternary i8.
//   - specificity comparison is a 3-way result {Less,Equal,Greater}; the
//     comparator `spec_cmp_trit` returns it as ONE balanced trit {-1,0,+1}.
//     A measured comparison against the naive binary two-comparison approach
//     lives in `self_test` / `bench_spec_cmp` — see report at end of file.

use alloc::string::String;
use alloc::vec::Vec;

// Font-size tiers map onto the AA font coverage sets (font_aa.rs):
//   AA_T (tiny,  ascent 13) · AA_S (small, ascent 15) · AA_L (large, ascent 28)
// We carry the *tier* as the resolved font_size byte so the renderer can pick
// the matching glyph table without re-deriving a pixel size.
pub const AA_T: u8 = 0; // tiny  (~12px and below)
pub const AA_S: u8 = 1; // small (~13..20px) — body default
pub const AA_L: u8 = 2; // large (~21px and up) — headings

// ── Tokenizer ──────────────────────────────────────────────────────────────

#[derive(Clone, PartialEq, Debug)]
pub enum Tok {
    Ident(String), // selector chunk or property/value word (kept raw incl. .#*)
    LBrace,
    RBrace,
    Colon,
    Semi,
}

/// Tokenize a CSS string. Strips `/* ... */` comments. Selector text and
/// declaration text are emitted as `Ident` runs split on structural chars.
pub fn tokenize(src: &str) -> Vec<Tok> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    let mut start = 0usize; // start of the current ident run
    let mut have = false;   // is there a pending ident run?

    fn flush(out: &mut Vec<Tok>, src: &str, start: usize, end: usize, have: &mut bool) {
        if *have {
            let s = src[start..end].trim();
            if !s.is_empty() {
                out.push(Tok::Ident(String::from(s)));
            }
            *have = false;
        }
    }

    while i < b.len() {
        // comment?
        if b[i] == b'/' && i + 1 < b.len() && b[i + 1] == b'*' {
            flush(&mut out, src, start, i, &mut have);
            i += 2;
            while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                i += 1;
            }
            i += 2; // skip "*/" (or run off the end harmlessly)
            continue;
        }
        match b[i] {
            b'{' => { flush(&mut out, src, start, i, &mut have); out.push(Tok::LBrace); }
            b'}' => { flush(&mut out, src, start, i, &mut have); out.push(Tok::RBrace); }
            b':' => { flush(&mut out, src, start, i, &mut have); out.push(Tok::Colon); }
            b';' => { flush(&mut out, src, start, i, &mut have); out.push(Tok::Semi); }
            _ => {
                if !have { start = i; have = true; }
            }
        }
        i += 1;
    }
    flush(&mut out, src, start, b.len(), &mut have);
    out
}

// ── Selectors ────────────────────────────────────────────────────────────────

#[derive(Clone, PartialEq, Debug)]
pub enum SimpleSel {
    Universal,
    Tag(String),
    Class(String),
    Id(String),
}

#[derive(Clone, Debug)]
pub struct Selector {
    pub part: SimpleSel,
}

impl Selector {
    /// Specificity as (id_count, class_count, tag_count).
    pub fn specificity(&self) -> (u32, u32, u32) {
        match self.part {
            SimpleSel::Id(_) => (1, 0, 0),
            SimpleSel::Class(_) => (0, 1, 0),
            SimpleSel::Tag(_) => (0, 0, 1),
            SimpleSel::Universal => (0, 0, 0),
        }
    }

    /// Does this selector match the element descriptor?
    pub fn matches(&self, el: &Element) -> bool {
        match &self.part {
            SimpleSel::Universal => true,
            SimpleSel::Tag(t) => el.tag == *t,
            SimpleSel::Id(i) => el.id.as_deref() == Some(i.as_str()),
            SimpleSel::Class(c) => el.classes.iter().any(|x| x == c),
        }
    }
}

/// Parse a single simple selector token like `p`, `.box`, `#main`, `*`.
fn parse_selector(s: &str) -> Option<Selector> {
    let s = s.trim();
    if s.is_empty() { return None; }
    let part = if s == "*" {
        SimpleSel::Universal
    } else if let Some(rest) = s.strip_prefix('.') {
        if rest.is_empty() { return None; }
        SimpleSel::Class(String::from(rest))
    } else if let Some(rest) = s.strip_prefix('#') {
        if rest.is_empty() { return None; }
        SimpleSel::Id(String::from(rest))
    } else {
        SimpleSel::Tag(String::from(s))
    };
    Some(Selector { part })
}

// ── Rules / Stylesheet ───────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct Rule {
    pub selectors: Vec<Selector>,
    pub decls: Vec<(String, String)>,
}

pub struct Stylesheet {
    pub rules: Vec<Rule>,
}

impl Stylesheet {
    /// Parse a full stylesheet from source.
    pub fn parse(src: &str) -> Self {
        let toks = tokenize(src);
        let mut rules = Vec::new();
        let mut i = 0usize;

        while i < toks.len() {
            // Collect selector idents up to the next LBrace.
            let mut sel_idents: Vec<String> = Vec::new();
            while i < toks.len() {
                match &toks[i] {
                    Tok::Ident(s) => { sel_idents.push(s.clone()); i += 1; }
                    Tok::LBrace => break,
                    // stray separators between selectors — skip
                    _ => { i += 1; }
                }
            }
            if i >= toks.len() { break; }       // no block — done
            i += 1;                              // consume LBrace

            // Each ident may itself be a comma/space-joined list (e.g. "h1, h2").
            let mut selectors = Vec::new();
            for raw in &sel_idents {
                for piece in raw.split([',', ' ', '\t', '\n']) {
                    if let Some(sel) = parse_selector(piece) {
                        selectors.push(sel);
                    }
                }
            }

            // Parse declarations until RBrace.
            let mut decls: Vec<(String, String)> = Vec::new();
            let mut prop: Option<String> = None;
            while i < toks.len() {
                match &toks[i] {
                    Tok::RBrace => { i += 1; break; }
                    Tok::Ident(s) => {
                        if prop.is_none() { prop = Some(s.clone()); }
                        else {
                            // value side (could span if not for colon split)
                            if let Some(p) = prop.take() {
                                decls.push((p.to_ascii_lowercase(), s.trim().into()));
                            }
                        }
                        i += 1;
                    }
                    Tok::Colon => { i += 1; } // separator between prop and value
                    Tok::Semi => { prop = None; i += 1; }
                    Tok::LBrace => { i += 1; } // malformed; skip
                }
            }

            if !selectors.is_empty() {
                rules.push(Rule { selectors, decls });
            }
        }

        Stylesheet { rules }
    }

    /// Cascade: resolve the computed style for `el` + inline `style=""`.
    pub fn resolve(&self, el: &Element, inline: &str) -> ComputedStyle {
        self.resolve_with_base(el, inline, ComputedStyle::default())
    }

    /// Like `resolve`, but starts the cascade from a caller-supplied `base`
    /// instead of the engine default — so any property the page's CSS does NOT
    /// set keeps the caller's value. This is what lets PinguBrowser pass its
    /// reader defaults and only have the page override what it explicitly styles.
    pub fn resolve_with_base(&self, el: &Element, inline: &str, base: ComputedStyle) -> ComputedStyle {
        // Gather every (specificity, source_order, &decls) that matches.
        // We apply in cascade order: lower specificity first, ties by source
        // order, then inline last (always wins).
        let mut matched: Vec<(Spec, usize, &Vec<(String, String)>)> = Vec::new();
        for (order, rule) in self.rules.iter().enumerate() {
            // A rule applies if ANY of its selectors matches; use the highest
            // specificity among the matching selectors.
            let mut best: Option<Spec> = None;
            for sel in &rule.selectors {
                if sel.matches(el) {
                    let s = Spec::from_tuple(sel.specificity());
                    best = Some(match best {
                        Some(b) if spec_cmp_trit(s, b) == 1 => s,
                        Some(b) => b,
                        None => s,
                    });
                }
            }
            if let Some(s) = best {
                matched.push((s, order, &rule.decls));
            }
        }

        // Sort ascending by (specificity, source order) using the trit
        // comparator. Insertion sort keeps it tiny + stable for no_std.
        let n = matched.len();
        for a in 1..n {
            let mut j = a;
            while j > 0 {
                let (sa, oa, _) = matched[j];
                let (sb, ob, _) = matched[j - 1];
                let c = spec_cmp_trit(sb, sa); // is prev > cur ?
                let prev_greater = c == 1 || (c == 0 && ob > oa);
                if prev_greater { matched.swap(j, j - 1); j -= 1; } else { break; }
            }
        }

        let mut cs = base;
        for (_, _, decls) in &matched {
            for (k, v) in decls.iter() {
                apply_decl(&mut cs, k, v);
            }
        }
        // Inline style wins unconditionally.
        for (k, v) in parse_decls(inline) {
            apply_decl(&mut cs, &k, &v);
        }
        cs
    }
}

/// Parse a flat `prop: val; prop: val` declaration string (inline style).
pub fn parse_decls(s: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for chunk in s.split(';') {
        let mut it = chunk.splitn(2, ':');
        let k = match it.next() { Some(k) => k.trim(), None => continue };
        let v = match it.next() { Some(v) => v.trim(), None => continue };
        if k.is_empty() || v.is_empty() { continue; }
        out.push((k.to_ascii_lowercase(), String::from(v)));
    }
    out
}

// ── Element descriptor ───────────────────────────────────────────────────────

#[derive(Clone)]
pub struct Element {
    pub tag: String,
    pub id: Option<String>,
    pub classes: Vec<String>,
}

impl Element {
    pub fn new(tag: &str) -> Self {
        Element { tag: String::from(tag), id: None, classes: Vec::new() }
    }
    pub fn id(mut self, id: &str) -> Self { self.id = Some(String::from(id)); self }
    pub fn class(mut self, c: &str) -> Self { self.classes.push(String::from(c)); self }
}

// ── Computed style ───────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct ComputedStyle {
    pub color: u32,           // 0xRRGGBB
    pub font_size: u8,        // AA_T / AA_S / AA_L tier
    pub font_weight_bold: bool,
    pub text_align: i8,       // balanced ternary: -1 left, 0 center, +1 right
    pub display_none: bool,
}

impl ComputedStyle {
    pub fn default() -> Self {
        ComputedStyle {
            color: 0xECEDE5,       // warm off-white (matches css.rs --txt)
            font_size: AA_S,       // body default
            font_weight_bold: false,
            text_align: -1,        // CSS default is left
            display_none: false,
        }
    }
}

// ── Value parsing ────────────────────────────────────────────────────────────

/// Parse a hex color: `#rgb` or `#rrggbb` → 0xRRGGBB.
pub fn parse_hex(s: &str) -> Option<u32> {
    let s = s.trim();
    let s = s.strip_prefix('#')?;
    match s.len() {
        3 => {
            let mut v = 0u32;
            for ch in s.bytes() {
                let d = hex_digit(ch)?;
                // expand each nibble: r -> rr
                v = (v << 8) | ((d as u32) << 4 | d as u32);
            }
            Some(v)
        }
        6 => u32::from_str_radix(s, 16).ok(),
        _ => None,
    }
}

fn hex_digit(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// The ~16 basic CSS named colors → 0xRRGGBB.
pub fn named_color(name: &str) -> Option<u32> {
    Some(match name.trim() {
        "black" => 0x000000,
        "silver" => 0xC0C0C0,
        "gray" | "grey" => 0x808080,
        "white" => 0xFFFFFF,
        "maroon" => 0x800000,
        "red" => 0xFF0000,
        "purple" => 0x800080,
        "fuchsia" | "magenta" => 0xFF00FF,
        "green" => 0x008000,
        "lime" => 0x00FF00,
        "olive" => 0x808000,
        "yellow" => 0xFFFF00,
        "navy" => 0x000080,
        "blue" => 0x0000FF,
        "teal" => 0x008080,
        "aqua" | "cyan" => 0x00FFFF,
        "orange" => 0xFFA500,
        _ => return None,
    })
}

/// Resolve any supported color value form.
pub fn parse_color(v: &str) -> Option<u32> {
    let v = v.trim();
    if v.starts_with('#') { parse_hex(v) } else { named_color(&v.to_ascii_lowercase()) }
}

/// Map a font-size value to an AA tier.
/// Keywords (small/medium/large/x-large/h-style) + px buckets.
pub fn parse_font_size(v: &str) -> Option<u8> {
    let v = v.trim().to_ascii_lowercase();
    match v.as_str() {
        "xx-small" | "x-small" | "small" => return Some(AA_T),
        "medium" => return Some(AA_S),
        "large" | "x-large" | "xx-large" | "larger" => return Some(AA_L),
        "smaller" => return Some(AA_T),
        _ => {}
    }
    // px bucket: <=12 -> T, 13..=20 -> S, >=21 -> L
    let num = v.trim_end_matches("px").trim_end_matches("pt").trim();
    if let Ok(px) = num.parse::<u32>() {
        return Some(if px <= 12 { AA_T } else if px <= 20 { AA_S } else { AA_L });
    }
    None
}

/// Apply a single declaration onto a ComputedStyle (mutating).
pub fn apply_decl(cs: &mut ComputedStyle, key: &str, val: &str) {
    match key {
        "color" => { if let Some(c) = parse_color(val) { cs.color = c; } }
        "font-size" => { if let Some(t) = parse_font_size(val) { cs.font_size = t; } }
        "font-weight" => {
            let v = val.trim().to_ascii_lowercase();
            cs.font_weight_bold = v == "bold" || v == "bolder"
                || v.parse::<u32>().map(|n| n >= 600).unwrap_or(false);
        }
        "text-align" => {
            match val.trim().to_ascii_lowercase().as_str() {
                "left" | "start" => cs.text_align = -1,
                "center" => cs.text_align = 0,
                "right" | "end" => cs.text_align = 1,
                _ => {}
            }
        }
        "display" => {
            cs.display_none = val.trim().eq_ignore_ascii_case("none");
        }
        _ => {}
    }
}

// ── Specificity as one balanced trit ─────────────────────────────────────────
//
// CSS specificity is the 3-tuple (id, class, tag) compared lexicographically.
// The *result* of a comparison is genuinely ternary: Less / Equal / Greater.
// We pack the tuple into one u32 (8 bits per field — far more than any real
// selector needs) so the whole comparison is a single integer compare, and we
// return the outcome as a balanced trit in one i8: -1 / 0 / +1.

pub type Spec = u32;

pub struct SpecExt;
impl SpecExt {
    pub fn pack(id: u32, class: u32, tag: u32) -> Spec {
        (id.min(255) << 16) | (class.min(255) << 8) | tag.min(255)
    }
}

trait SpecFrom { fn from_tuple(t: (u32, u32, u32)) -> Spec; }
impl SpecFrom for Spec {
    fn from_tuple(t: (u32, u32, u32)) -> Spec { SpecExt::pack(t.0, t.1, t.2) }
}

/// Balanced-ternary comparator: returns -1 if a<b, 0 if a==b, +1 if a>b — in
/// ONE value, branchlessly. This is the trit-native form of the cascade order.
#[inline]
pub fn spec_cmp_trit(a: Spec, b: Spec) -> i8 {
    ((a > b) as i8) - ((a < b) as i8)
}

/// Naive binary baseline: the same outcome via the conventional two-comparison
/// style a binary-minded engine writes. Kept ONLY for the honest benchmark.
#[inline]
pub fn spec_cmp_binary(a: Spec, b: Spec) -> i8 {
    if a < b { -1 } else if a > b { 1 } else { 0 }
}

// ── Self-test + honest benchmark ─────────────────────────────────────────────

/// Count comparison operations a comparator strategy *executes* across a loop.
/// We can't read cycles in no_std without a syscall, so we count the actual
/// `<`/`>` comparisons performed — the thing that differs between the two
/// strategies — which is what the ternary claim is really about.
///
/// Returns (trit_ops, binary_ops, agreement_count).
pub fn bench_spec_cmp() -> (u64, u64, u64) {
    // A spread of specificity values to exercise <, ==, > paths evenly.
    let vals: [Spec; 8] = [
        SpecExt::pack(0, 0, 0),
        SpecExt::pack(0, 0, 1),
        SpecExt::pack(0, 1, 0),
        SpecExt::pack(0, 1, 2),
        SpecExt::pack(1, 0, 0),
        SpecExt::pack(1, 2, 0),
        SpecExt::pack(1, 0, 1),
        SpecExt::pack(2, 0, 0),
    ];

    let mut trit_ops = 0u64;
    let mut bin_ops = 0u64;
    let mut agree = 0u64;

    for &a in vals.iter() {
        for &b in vals.iter() {
            // Trit form ALWAYS performs exactly two comparisons: (a>b),(a<b).
            let t = spec_cmp_trit(a, b);
            trit_ops += 2;

            // Binary form performs 1 comparison when a<b is true, else 2.
            // (`if a<b ... else if a>b ...`)
            let bn = if a < b {
                bin_ops += 1;
                -1
            } else if a > b {
                bin_ops += 2;
                1
            } else {
                bin_ops += 2;
                0
            };

            if t == bn { agree += 1; }
        }
    }
    (trit_ops, bin_ops, agree)
}

// ── @sparseskip: cascade dormancy gate ───────────────────────────────────────
// A rule is DORMANT for an element when none of its selectors match — its whole
// declaration block is the `0` state and is physically skipped. This mirrors the
// kernel's @sparseskip (skip Zero-weight work). The HONEST question the bench
// answers: is this a *ternary* advantage, or just standard selector culling that
// a binary engine does identically?
#[inline]
pub fn is_rule_dormant(rule: &Rule, el: &Element) -> bool {
    !rule.selectors.iter().any(|s| s.matches(el))
}

/// Measure cascade work over a representative sheet × page. Returns
/// (applied_decls, dormant_decls_skipped, match_checks). `applied` are the
/// declaration-applies actually performed; `dormant` are the ones skipped
/// because the rule didn't match; `match_checks` is the selector-match work paid
/// either way (and which a binary engine pays too).
pub fn bench_sparseskip() -> (u64, u64, u64) {
    let sheet = Stylesheet::parse(
        "* { color:#222; } body { color:#111; } p { color:#333; font-size:16px; } \
         h1 { color:#1a4a80; font-size:24px; } h2 { color:#b4502a; } a { color:#1a5fbe; } \
         .warn { color:red; text-align:right; } .muted { color:#888; } .big { font-size:24px; } \
         #title { color:#000; font-weight:bold; } #nav { display:none; } li { color:#333; } \
         .card { color:#222; } .card h2 { color:#444; } strong { font-weight:bold; } code { color:#a33; }",
    );
    // A small but realistic page: 9 elements with varied tags/classes/ids.
    let els = [
        Element::new("p"),
        Element::new("h1").id("title"),
        Element::new("p").class("warn"),
        Element::new("a").class("muted"),
        Element::new("h2").class("card"),
        Element::new("li"),
        Element::new("div").id("nav"),
        Element::new("strong"),
        Element::new("span").class("big"),
    ];
    let mut applied = 0u64;
    let mut dormant = 0u64;
    let mut checks = 0u64;
    for el in els.iter() {
        for rule in &sheet.rules {
            checks += rule.selectors.len() as u64; // paid by binary engines too
            let decls = rule.decls.len() as u64;
            if is_rule_dormant(rule, el) { dormant += decls; } else { applied += decls; }
        }
    }
    (applied, dormant, checks)
}

/// In-code verification. Returns true iff every assert holds.
pub fn self_test() -> bool {
    // 1. Value parsers.
    if parse_hex("#fff") != Some(0xFFFFFF) { return false; }
    if parse_hex("#abc") != Some(0xAABBCC) { return false; }
    if parse_hex("#102030") != Some(0x102030) { return false; }
    if parse_hex("#zz") != None { return false; }
    if named_color("red") != Some(0xFF0000) { return false; }
    if named_color("teal") != Some(0x008080) { return false; }
    if parse_color("blue") != Some(0x0000FF) { return false; }
    if parse_font_size("12px") != Some(AA_T) { return false; }
    if parse_font_size("16px") != Some(AA_S) { return false; }
    if parse_font_size("24px") != Some(AA_L) { return false; }
    if parse_font_size("large") != Some(AA_L) { return false; }

    // 2. Tokenizer: comments stripped, structure emitted.
    let toks = tokenize("/* c */ p { color: red; }");
    if toks.first() != Some(&Tok::Ident(String::from("p"))) { return false; }

    // 3. Parse a small stylesheet with mixed selectors.
    let css = "
        * { color: #111111; }
        p { color: green; font-size: 16px; }
        .warn { color: orange; text-align: right; }
        #title { color: #ff0000; font-size: 28px; font-weight: bold; }
        .hidden { display: none; }
    ";
    let sheet = Stylesheet::parse(css);
    if sheet.rules.len() != 5 { return false; }

    // Specificity sanity.
    let id_sel = parse_selector("#title").unwrap();
    let cl_sel = parse_selector(".warn").unwrap();
    let tg_sel = parse_selector("p").unwrap();
    if id_sel.specificity() != (1, 0, 0) { return false; }
    if cl_sel.specificity() != (0, 1, 0) { return false; }
    if tg_sel.specificity() != (0, 0, 1) { return false; }

    // 4. Cascade resolution.

    // A plain <p>: tag rule beats universal rule (higher specificity) → green,
    // 16px (AA_S), left-aligned default, not bold.
    let p = Element::new("p");
    let cs = sheet.resolve(&p, "");
    if cs.color != 0x008000 { return false; }
    if cs.font_size != AA_S { return false; }
    if cs.text_align != -1 { return false; }
    if cs.font_weight_bold { return false; }
    if cs.display_none { return false; }

    // <p class="warn">: .warn (class, spec 0,1,0) beats p (tag) for color →
    // orange; text-align right (+1); font-size still inherited from p rule? No
    // — cascade only applies matched rules; .warn doesn't set font-size, p does
    // and p still matches → AA_S.
    let pw = Element::new("p").class("warn");
    let cs = sheet.resolve(&pw, "");
    if cs.color != 0xFFA500 { return false; }   // orange wins over green
    if cs.text_align != 1 { return false; }       // right
    if cs.font_size != AA_S { return false; }     // from p rule

    // <p id="title" class="warn">: #title (id, 1,0,0) beats .warn (0,1,0) and p
    // → red, 28px (AA_L), bold. text-align comes only from .warn → right.
    let pt = Element::new("p").id("title").class("warn");
    let cs = sheet.resolve(&pt, "");
    if cs.color != 0xFF0000 { return false; }
    if cs.font_size != AA_L { return false; }
    if !cs.font_weight_bold { return false; }
    if cs.text_align != 1 { return false; }

    // Inline style wins over everything.
    let cs = sheet.resolve(&pt, "color: #00ff00; text-align: center;");
    if cs.color != 0x00FF00 { return false; }
    if cs.text_align != 0 { return false; } // center

    // display:none via class.
    let h = Element::new("div").class("hidden");
    let cs = sheet.resolve(&h, "");
    if !cs.display_none { return false; }

    // 5. Specificity trit comparator: correctness vs the binary baseline.
    let a = SpecExt::pack(1, 0, 0);
    let b = SpecExt::pack(0, 9, 9);
    if spec_cmp_trit(a, b) != 1 { return false; }            // id beats many classes
    if spec_cmp_trit(b, a) != -1 { return false; }
    if spec_cmp_trit(a, a) != 0 { return false; }
    let (t_ops, b_ops, agree) = bench_spec_cmp();
    if agree != 64 { return false; }   // 8x8: both strategies must always agree
    // Honest expectation: the binary baseline executes FEWER comparisons,
    // because it short-circuits on the a<b case. Assert that the measurement
    // reflects reality rather than a fabricated ternary win.
    if !(b_ops <= t_ops) { return false; }
    let _ = (t_ops, b_ops);

    true
}

// ── HONEST TERNARY FINDING ───────────────────────────────────────────────────
//
// Claim under test: "a balanced-trit specificity comparator beats the naive
// binary two-comparison approach."
//
// Measurement (bench_spec_cmp, 8x8 = 64 comparisons over a spread of specs):
//   * Correctness: 64/64 agreement — the trit comparator is exactly correct.
//   * Comparison operations executed:
//       - trit form  `((a>b) - (a<b))`  ALWAYS does 2 comparisons → 128 total.
//       - binary form `if a<b {..} else if a>b {..}` does 1 comparison on the
//         a<b branch and 2 otherwise → for this balanced spread it does FEWER
//         (it short-circuits whenever a<b).
//
// VERDICT — NO SPEED WIN. The branchless trit form does NOT execute fewer
// comparisons; the binary form can short-circuit and is <= the trit form on
// op-count. The trit comparator's real, honest advantages are architectural,
// not throughput:
//   1. It returns the full 3-way ordering in ONE i8 value (-1/0/+1), so the
//      cascade sort and the "is prev greater" test read it directly without a
//      second comparison or an enum match — fewer *call sites* re-deriving the
//      relation, even though each call does the same/more work internally.
//   2. It is branchless (no misprediction), which a pure op-count cannot
//      capture; that *could* help on a real pipeline, but we did not measure
//      cycles (no rdtsc in this no_std target), so we DO NOT claim it.
//
// So: ternary `text_align` (-1/0/+1) is a genuine, natural tri-state fit; the
// trit specificity comparator is semantically clean and correct but shows NO
// measured operation-count speedup over binary. Reported honestly.
