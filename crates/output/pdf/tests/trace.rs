//! The PDF writer against pdfTeX's output for the files of
//! `oracle/fixtures/pdf`.
//!
//! Each fixture `<name>.pdf` is `pdftex -ini <name>.tex` (pdfTeX
//! 1.40.29) with `SOURCE_DATE_EPOCH=1 FORCE_SOURCE_DATE=1`. The boxes
//! pdfTeX shipped are read from `tests/fixtures/trace/<name>.log`, the
//! same run with `\tracingoutput=1` and unlimited `\showboxdepth` and
//! `\showboxbreadth` (the log's box displays; glue settings, which the
//! display rounds, packed again); forms' boxes from `\showbox`. The
//! calls the TeX side makes when it reads the commands are written out
//! for each fixture, in the order pdfTeX makes them; whatsits come from
//! the displays, with the object numbers the display leaves out. The
//! fonts, map, encodings and images are TeX Live's (`kpsewhich`); the
//! whole file must come out the same.

#![allow(
    clippy::unreadable_literal,
    clippy::too_many_lines,
    clippy::missing_panics_doc,
    clippy::many_single_char_names
)]

use std::collections::{BTreeMap, HashMap, VecDeque};

use phitex_pdf::map::{MapEntry, parse_map, parse_map_ps};
use phitex_pdf::{
    Action, ColorStackCmd, DestKind, FontSource, Ident, ImageSpec, LiteralMode, ObjBody, PageBox,
    PdfBytes, PdfParams, PdfWriter, VirtualFont, Whatsit,
};
use phitex_tfm::ScaledFont;
use phitex_typeset::arith::{Scaled, UNITY};
use phitex_typeset::fonts::{CharInfo, FontId, Fonts, Left, LigKern};
use phitex_typeset::glue::{Glue, GlueSet, GlueSign, Order};
use phitex_typeset::node::{
    BoxKind, BoxNode, Disc, GlueKind, GlueNode, KernKind, Ligature, MathKind, Node, RUNNING, Rule,
    WhatsitSize,
};
use phitex_typeset::pack::{PackParams, Spec, hpack, vpackage};

const PT: Scaled = 65536;

// ---- files ----

fn kpse(name: &str) -> Option<Vec<u8>> {
    std::fs::read(kpse_path(name)?).ok()
}

fn kpse_path(name: &str) -> Option<String> {
    thread_local! {
        static CACHE: std::cell::RefCell<HashMap<String, Option<String>>> =
            std::cell::RefCell::new(HashMap::new());
    }
    CACHE.with(|c| {
        c.borrow_mut()
            .entry(name.to_owned())
            .or_insert_with(|| {
                let out = std::process::Command::new("kpsewhich")
                    .arg(name)
                    .output()
                    .ok()?;
                let s = String::from_utf8(out.stdout).ok()?;
                let s = s.trim();
                (!s.is_empty()).then(|| s.to_owned())
            })
            .clone()
    })
}

fn oracle(name: &str) -> Vec<u8> {
    std::fs::read(format!(
        "{}/../../../oracle/fixtures/pdf/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

fn trace(name: &str) -> String {
    let b = std::fs::read(format!(
        "{}/tests/fixtures/trace/{name}.log",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    String::from_utf8_lossy(&b).into_owned()
}

// ---- fonts ----

struct Fnt {
    name: String,
    scaled: ScaledFont,
    vf: Option<VirtualFont>,
}

/// The job's fonts, TeX's internal numbers: font `k` is `fonts[k-1]`.
struct Src {
    fonts: Vec<Fnt>,
    map: BTreeMap<Vec<u8>, MapEntry>,
    ps_map: BTreeMap<(Vec<u8>, i32, i32), MapEntry>,
    space: Option<FontId>,
}

/// `store_scaled_f` (pdfTeX), for VF local font sizes.
fn store_scaled_f(sq: i32, z: Scaled) -> Scaled {
    let mut z = z;
    let mut alpha = 16;
    while z >= 0o40000000 {
        z /= 2;
        alpha += alpha;
    }
    let beta = 256 / alpha;
    let alpha = alpha * z;
    let u = if sq >= 0 {
        sq.cast_unsigned()
    } else {
        (sq + 0x4000_0000 + 0x4000_0000).cast_unsigned()
    };
    let [a, b, c, d] = u.to_be_bytes();
    let a = if sq >= 0 { a } else { a.wrapping_add(128) };
    let (b, c, d) = (i32::from(b), i32::from(c), i32::from(d));
    let sw = (((((d * z) / 0o400) + (c * z)) / 0o400) + (b * z)) / beta;
    if a == 0 { sw } else { sw - alpha }
}

impl Src {
    fn new() -> Src {
        Src {
            fonts: Vec::new(),
            map: parse_map(&kpse("pdftex.map").unwrap()),
            ps_map: parse_map_ps(&kpse("pdftex.map").unwrap()),
            space: None,
        }
    }

    /// `\font\x=name [at size]` (`new_font`): the font, loaded if no
    /// font of that name and size is.
    fn font(&mut self, name: &str, at: Option<Scaled>) -> FontId {
        let tfm = phitex_tfm::parse(&kpse(&format!("{name}.tfm")).unwrap()).unwrap();
        let size = at.unwrap_or(tfm.design_size);
        if let Some(i) = self
            .fonts
            .iter()
            .position(|f| f.name == name && f.scaled.size == size)
        {
            return FontId::try_from(i + 1).unwrap();
        }
        let scaled = tfm.at(size).unwrap();
        self.fonts.push(Fnt {
            name: name.to_owned(),
            scaled,
            vf: None,
        });
        FontId::try_from(self.fonts.len()).unwrap()
    }

    fn get(&self, f: FontId) -> &Fnt {
        &self.fonts[f as usize - 1]
    }

    /// `do_vf`: font `f`'s virtual font read, its local fonts loaded
    /// (`vf_def_font`: `tfm_lookup` or `read_font_info`).
    fn load_vf(&mut self, f: FontId) {
        let name = self.get(f).name.clone();
        let Some(bytes) = kpse(&format!("{name}.vf")) else {
            return;
        };
        let vf = phitex_tfm::parse_vf(&bytes).unwrap();
        let size = self.get(f).scaled.size;
        let mut fonts = Vec::new();
        for lf in &vf.fonts {
            let fs = store_scaled_f(lf.scaled, size);
            let n = String::from_utf8(lf.name.clone()).unwrap();
            fonts.push((lf.number, self.font(&n, Some(fs))));
        }
        let packets = vf
            .chars
            .iter()
            .map(|c| (u8::try_from(c.code).unwrap(), c.dvi.to_vec()))
            .collect();
        self.fonts[f as usize - 1].vf = Some(VirtualFont { fonts, packets });
    }

    /// `pdf_read_dummy_font`'s font, loaded now.
    fn load_space(&mut self) {
        let f = self.font("pdftexspace", None);
        self.space = Some(f);
    }
}

impl Fonts for Src {
    fn char_info(&self, font: FontId, ch: u16) -> Option<CharInfo> {
        let d = self.get(font).scaled.char(u8::try_from(ch).ok()?)?;
        Some(CharInfo {
            width: d.width,
            height: d.height,
            depth: d.depth,
            italic: d.italic,
            ..CharInfo::default()
        })
    }
    fn lig_kern(&self, _: FontId, _: Left, _: u16) -> Option<LigKern> {
        None
    }
    fn has_left_boundary(&self, _: FontId) -> bool {
        false
    }
    fn bchar(&self, _: FontId) -> Option<u16> {
        None
    }
    fn param(&self, font: FontId, n: u16) -> Scaled {
        self.get(font).scaled.param(usize::from(n))
    }
    fn param_count(&self, font: FontId) -> u16 {
        u16::try_from(self.get(font).scaled.params.len()).unwrap()
    }
}

impl FontSource for Src {
    fn metrics(&self, f: FontId) -> &ScaledFont {
        &self.get(f).scaled
    }
    fn name(&self, f: FontId) -> &[u8] {
        self.get(f).name.as_bytes()
    }
    fn map_entry(&self, f: FontId) -> Option<MapEntry> {
        self.map.get(self.name(f)).cloned()
    }
    fn font_file(&self, f: FontId) -> Option<Vec<u8>> {
        let m = self.map_entry(f)?;
        kpse(std::str::from_utf8(m.ff_name.as_deref()?).ok()?)
    }
    fn encoding_file(&self, f: FontId) -> Option<Vec<u8>> {
        let m = self.map_entry(f)?;
        kpse(std::str::from_utf8(m.encname.as_deref()?).ok()?)
    }
    fn file(&self, name: &[u8]) -> Option<Vec<u8>> {
        kpse(std::str::from_utf8(name).ok()?)
    }
    fn virtual_font(&self, f: FontId) -> Option<VirtualFont> {
        self.get(f).vf.clone()
    }
    fn space_font(&self) -> Option<FontId> {
        self.space
    }
    fn map_by_ps_name(&self, ps: &[u8], slant: i32, extend: i32) -> Option<MapEntry> {
        self.ps_map.get(&(ps.to_vec(), slant, extend)).cloned()
    }
}

// ---- box displays ----

/// `print_scaled` read back (§102, §452).
fn dimen(s: &str) -> Scaled {
    let (neg, s) = s.strip_prefix('-').map_or((false, s), |r| (true, r));
    let (i, f) = s.split_once('.').unwrap_or((s, ""));
    let digits: Vec<u8> = f.bytes().map(|b| b - b'0').take(17).collect();
    let v = i.parse::<Scaled>().unwrap() * UNITY + phitex_typeset::arith::round_decimals(&digits);
    if neg { -v } else { v }
}

fn rule_dimen(s: &str) -> Scaled {
    if s == "*" { RUNNING } else { dimen(s) }
}

/// `(h+d)xw` at the start of `s`: (w, h, d) and the rest.
fn rule_spec(s: &str) -> ([Scaled; 3], &str) {
    let s = s.strip_prefix('(').unwrap();
    let (hd, rest) = s.split_once(")x").unwrap();
    let (h, d) = hd.split_once('+').unwrap();
    let end = rest.find([' ', '{']).unwrap_or(rest.len());
    (
        [rule_dimen(&rest[..end]), rule_dimen(h), rule_dimen(d)],
        &rest[end..],
    )
}

fn glue_text(s: &str) -> Glue {
    let mut g = Glue::ZERO;
    let mut it = s.split(' ');
    g.width = dimen(it.next().unwrap());
    let amount = |x: &str| -> (Scaled, Order) {
        for (suf, o) in [
            ("filll", Order::Filll),
            ("fill", Order::Fill),
            ("fil", Order::Fil),
        ] {
            if let Some(v) = x.strip_suffix(suf) {
                return (dimen(v), o);
            }
        }
        (dimen(x), Order::Normal)
    };
    while let Some(k) = it.next() {
        let (v, o) = amount(it.next().unwrap());
        if k == "plus" {
            (g.stretch, g.stretch_order) = (v, o);
        } else {
            (g.shrink, g.shrink_order) = (v, o);
        }
    }
    g
}

fn chars(s: &str) -> Vec<u16> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'^' && b.get(i + 1) == Some(&b'^') {
            let c = b[i + 2];
            if c.is_ascii_hexdigit()
                && c.is_ascii_lowercase() | c.is_ascii_digit()
                && b.get(i + 3)
                    .is_some_and(|d| d.is_ascii_hexdigit() && !d.is_ascii_uppercase())
            {
                let h = u16::from_str_radix(std::str::from_utf8(&b[i + 2..i + 4]).unwrap(), 16);
                out.push(h.unwrap());
                i += 4;
                continue;
            }
            let k = u16::from(c);
            out.push(if k >= 64 { k - 64 } else { k + 64 });
            i += 3;
        } else {
            out.push(u16::from(b[i]));
            i += 1;
        }
    }
    out
}

const SKIP_PARAMS: [&str; 18] = [
    "lineskip",
    "baselineskip",
    "parskip",
    "abovedisplayskip",
    "belowdisplayskip",
    "abovedisplayshortskip",
    "belowdisplayshortskip",
    "leftskip",
    "rightskip",
    "topskip",
    "splittopskip",
    "tabskip",
    "spaceskip",
    "xspaceskip",
    "parfillskip",
    "thinmuskip",
    "medmuskip",
    "thickmuskip",
];

/// A box display read: the nodes, and each whatsit's display (the
/// node's handle is its index).
struct Reader<'a> {
    fonts: &'a HashMap<String, FontId>,
    /// The handle of the next whatsit (handles count across boxes).
    next: u32,
    whatsits: Vec<(u32, String)>,
}

impl Reader<'_> {
    fn font(&self, body: &str) -> Option<(FontId, String)> {
        let rest = body.strip_prefix('\\')?;
        let (name, rest) = rest.split_once(' ')?;
        self.fonts.get(name).map(|&f| (f, rest.to_owned()))
    }

    /// The lines at `prefix` from `i` on (and their sub-lists).
    fn list(&mut self, lines: &[String], prefix: &str) -> Vec<Node> {
        let mut out: Vec<Node> = Vec::new();
        let mut i = 0;
        let mut replace: Option<(usize, usize)> = None;
        let inner = format!("{prefix}.");
        let bar = format!("{prefix}|");
        while i < lines.len() {
            let l = &lines[i];
            assert!(l.starts_with(prefix), "{l}");
            let body = &l[prefix.len()..];
            i += 1;
            let start = i;
            while i < lines.len() && (lines[i].starts_with(&inner) || lines[i].starts_with(&bar)) {
                i += 1;
            }
            let sub = &lines[start..i];
            let node = if let Some((font, rest)) = self.font(body) {
                if let Some((c, lig)) = rest.split_once(" (ligature ") {
                    let lig = lig.strip_suffix(')').unwrap();
                    let left_hit = lig.starts_with('|');
                    let lig = lig.strip_prefix('|').unwrap_or(lig);
                    let right_hit = lig.ends_with('|') && !lig.is_empty();
                    let lig = lig.strip_suffix('|').unwrap_or(lig);
                    Node::Lig(Ligature {
                        font,
                        ch: chars(c)[0],
                        original: chars(lig),
                        left_hit,
                        right_hit,
                    })
                } else {
                    Node::Char {
                        font,
                        ch: chars(&rest)[0],
                    }
                }
            } else if body.starts_with("\\hbox(") || body.starts_with("\\vbox(") {
                let list = self.list(sub, &inner);
                Node::Box(box_header(body, list, self.fonts_ref()))
            } else if let Some(rest) = body.strip_prefix("\\rule") {
                let ([w, h, d], _) = rule_spec(rest);
                Node::Rule(Rule {
                    width: w,
                    height: h,
                    depth: d,
                })
            } else if body.starts_with("\\mathon") || body.starts_with("\\mathoff") {
                let kind = if body.starts_with("\\mathon") {
                    MathKind::Before
                } else {
                    MathKind::After
                };
                let w = body
                    .split_once(", surrounded ")
                    .map_or(0, |(_, w)| dimen(w));
                Node::Math(kind, w)
            } else if let Some(rest) = body
                .strip_prefix("\\leaders ")
                .map(|r| (GlueKind::ALeaders, r))
                .or_else(|| {
                    body.strip_prefix("\\cleaders ")
                        .map(|r| (GlueKind::CLeaders, r))
                })
                .or_else(|| {
                    body.strip_prefix("\\xleaders ")
                        .map(|r| (GlueKind::XLeaders, r))
                })
            {
                let leader = self.list(sub, &inner).pop().map(Box::new);
                Node::Glue(GlueNode {
                    spec: glue_text(rest.1),
                    kind: rest.0,
                    leader,
                })
            } else if let Some(rest) = body.strip_prefix("\\glue(\\") {
                let (name, g) = rest.split_once(") ").unwrap();
                let code = SKIP_PARAMS.iter().position(|x| *x == name).unwrap();
                Node::param_glue(u8::try_from(code).unwrap(), glue_text(g))
            } else if let Some(rest) = body.strip_prefix("\\glue ") {
                Node::glue(glue_text(rest))
            } else if let Some(rest) = body.strip_prefix("\\kern ") {
                if let Some(r) = rest.strip_suffix(" (for accent)") {
                    Node::Kern(dimen(r), KernKind::AccKern)
                } else {
                    Node::Kern(dimen(rest), KernKind::Explicit)
                }
            } else if let Some(rest) = body.strip_prefix("\\kern") {
                Node::Kern(dimen(rest), KernKind::Normal)
            } else if let Some(rest) = body.strip_prefix("\\penalty ") {
                Node::Penalty(rest.parse().unwrap())
            } else if body.starts_with("\\mark") {
                Node::Mark(0)
            } else if body.starts_with("\\insert") || body.starts_with("\\vadjust") {
                continue;
            } else if let Some(rest) = body.strip_prefix("\\discretionary") {
                let pre_lines: Vec<String> = sub
                    .iter()
                    .filter(|x| x.starts_with(&inner))
                    .cloned()
                    .collect();
                let post_lines: Vec<String> = sub
                    .iter()
                    .filter(|x| x.starts_with(&bar))
                    .cloned()
                    .collect();
                let pre = self.list(&pre_lines, &inner);
                let post = self.list(&post_lines, &bar);
                if let Some(n) = rest.strip_prefix(" replacing ") {
                    replace = Some((out.len(), n.parse().unwrap()));
                }
                out.push(Node::Disc(Disc {
                    pre,
                    post,
                    replace: Vec::new(),
                }));
                continue;
            } else if body.starts_with('\\') {
                // a whatsit; a form or image carries its size
                self.whatsits.push((self.next, body.to_owned()));
                self.next += 1;
                if let Some(r) = body
                    .strip_prefix("\\pdfrefxform")
                    .or_else(|| body.strip_prefix("\\pdfrefximage"))
                {
                    let ([width, height, depth], _) = rule_spec(r);
                    Node::Whatsit(
                        self.next - 1,
                        Some(WhatsitSize {
                            width,
                            height,
                            depth,
                        }),
                    )
                } else {
                    Node::Whatsit(self.next - 1, None)
                }
            } else {
                panic!("box display: {l}");
            };
            if let Some((d, n)) = replace
                && let Node::Disc(disc) = &mut out[d]
            {
                disc.replace.push(node);
                if disc.replace.len() == n {
                    replace = None;
                }
                continue;
            }
            out.push(node);
        }
        out
    }

    fn fonts_ref(&self) -> &HashMap<String, FontId> {
        self.fonts
    }
}

/// `\hbox(h+d)xw, glue set ..., shifted s`: the box, its glue set as
/// packing the list again sets it.
fn box_header(body: &str, list: Vec<Node>, _: &HashMap<String, FontId>) -> BoxNode {
    let kind = if body.starts_with("\\hbox") {
        BoxKind::H
    } else {
        BoxKind::V
    };
    let (dims, rest) = body[6..].split_once(")x").unwrap();
    let (h, d) = dims.split_once('+').unwrap();
    let mut parts = rest.split(", ");
    let w = dimen(parts.next().unwrap());
    let mut b = BoxNode {
        kind,
        width: w,
        height: dimen(h),
        depth: dimen(d),
        list,
        ..BoxNode::default()
    };
    for p in parts {
        if let Some(g) = p.strip_prefix("glue set ") {
            // recomputed below; keep the display to check
            b.glue = GlueSet {
                ratio: f64::NAN,
                sign: if g.starts_with("- ") {
                    GlueSign::Shrinking
                } else {
                    GlueSign::Stretching
                },
                order: Order::Normal,
            };
        } else if let Some(s) = p.strip_prefix("shifted ") {
            b.shift = dimen(s);
        }
    }
    b
}

/// The glue settings, packed again (`hpack` or `vpackage` to the box's
/// size, as TeX set them).
fn repack(b: &mut BoxNode, fonts: &dyn Fonts) {
    for n in &mut b.list {
        match n {
            Node::Box(x) => repack(x, fonts),
            Node::Glue(GlueNode {
                leader: Some(l), ..
            }) => {
                if let Node::Box(x) = &mut **l {
                    repack(x, fonts);
                }
            }
            Node::Disc(d) => {
                for x in d.pre.iter_mut().chain(&mut d.post).chain(&mut d.replace) {
                    if let Node::Box(x) = x {
                        repack(x, fonts);
                    }
                }
            }
            _ => {}
        }
    }
    if !b.glue.ratio.is_nan() {
        return;
    }
    let params = PackParams {
        overfull_rule: 0,
        ..PackParams::default()
    };
    let packed = match b.kind {
        BoxKind::H => {
            hpack(
                b.list.clone(),
                Spec::Exactly(b.width),
                fonts,
                &params,
                false,
            )
            .node
        }
        BoxKind::V => vpackage(b.list.clone(), Spec::Exactly(b.height), b.depth, &params).node,
    };
    b.glue = packed.glue;
}

/// The boxes shipped (`Completed box being shipped out`) and the boxes
/// shown (`> \box`n`=`), in the log's order: each with its whatsits.
struct Shown {
    /// Shipped, not shown.
    shipped: bool,
    /// A page (`[n]`), not a form.
    page: bool,
    /// The display's lines.
    lines: Vec<String>,
    /// The box, once read.
    boxed: Option<BoxNode>,
}

/// The log's lines, those TeX broke at `max_print_line` joined.
fn log_lines(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut join = false;
    for l in text.lines() {
        // character 0 is `\newlinechar` in INITEX: `\f ` and an empty line
        if l.is_empty()
            && let Some(p) = out.last_mut()
            && p.starts_with('.')
            && p.ends_with(' ')
            && p.trim_start_matches('.').starts_with('\\')
            && !p.trim_start_matches('.').trim_end().contains(' ')
        {
            p.push_str("^^@");
            join = false;
            continue;
        }
        if join {
            out.last_mut().unwrap().push_str(l);
        } else {
            out.push(l.to_owned());
        }
        join = l.len() == 79;
    }
    out
}

fn read_log(text: &str) -> Vec<Shown> {
    let lines = log_lines(text);
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let shipped = lines[i].starts_with("Completed box being shipped out");
        let page = shipped && lines[i].contains('[');
        let shown = lines[i].starts_with("> \\box");
        if !(shipped || shown) {
            i += 1;
            continue;
        }
        let mut first = if shipped {
            i += 1;
            lines[i].clone()
        } else {
            lines[i].split_once('=').unwrap().1.to_owned()
        };
        if first == "void" {
            i += 1;
            continue;
        }
        first.insert(0, '#');
        let mut block = vec![first];
        i += 1;
        while i < lines.len() && lines[i].starts_with('.') {
            block.push(format!("#{}", lines[i]));
            i += 1;
        }
        out.push(Shown {
            shipped,
            page,
            lines: block,
            boxed: None,
        });
    }
    out
}

// ---- whatsits ----

/// `{...}` at the start of `s`: the text and the rest.
fn braced(s: &str) -> (Vec<u8>, &str) {
    let s = s.strip_prefix('{').unwrap_or_else(|| panic!("braces: {s}"));
    let mut depth = 0;
    for (i, c) in s.char_indices() {
        match c {
            '{' => depth += 1,
            '}' if depth == 0 => {
                let text = &s[..i];
                assert!(!text.contains("\\ETC."), "truncated: {text}");
                return (text.as_bytes().to_vec(), &s[i + 1..]);
            }
            '}' => depth -= 1,
            _ => {}
        }
    }
    panic!("unbalanced: {s}")
}

fn ident(s: &str) -> (Ident, &str) {
    if let Some(r) = s.strip_prefix("name") {
        let (t, r) = braced(r);
        (Ident::Name(t), r)
    } else {
        let r = s.strip_prefix("num").unwrap();
        let end = r
            .find(|c: char| !c.is_ascii_digit() && c != '-')
            .unwrap_or(r.len());
        (Ident::Num(r[..end].parse().unwrap()), &r[end..])
    }
}

/// Object numbers the displays leave out, in the order the whatsits
/// are met; and the actions' details (`newwindow`).
#[derive(Default)]
struct Nums {
    annots: VecDeque<usize>,
    links: VecDeque<usize>,
    objs: VecDeque<usize>,
    xforms: VecDeque<usize>,
    ximages: VecDeque<usize>,
    new_window: VecDeque<u8>,
}

fn action(s: &str, nums: &mut Nums) -> Action {
    let s = s.trim_start();
    if let Some(r) = s.strip_prefix("user") {
        return Action::User(braced(r).0);
    }
    let (file, s) = match s.strip_prefix("file") {
        Some(r) => {
            let (f, r) = braced(r);
            (Some(f), r.trim_start())
        }
        None => (None, s),
    };
    let new_window = if file.is_some() {
        nums.new_window.pop_front().unwrap_or(0)
    } else {
        0
    };
    if let Some(r) = s.strip_prefix("goto ") {
        let (id, _) = ident(r);
        Action::Goto {
            file,
            id,
            new_window,
            struct_id: None,
        }
    } else if let Some(r) = s.strip_prefix("page") {
        let end = r.find('{').unwrap();
        let page = r[..end].parse().unwrap();
        let (spec, _) = braced(&r[end..]);
        Action::Page {
            file,
            page,
            spec,
            new_window,
            struct_id: None,
        }
    } else if let Some(r) = s.strip_prefix("thread ") {
        let (id, _) = ident(r);
        Action::Thread { file, id }
    } else {
        panic!("action: {s}")
    }
}

fn dest_kind(s: &str) -> DestKind {
    let s = s.trim_start();
    if let Some(r) = s.strip_prefix("xyz") {
        return DestKind::Xyz(r.strip_prefix(" zoom").map(|z| z.parse().unwrap()));
    }
    if let Some(r) = s.strip_prefix("fitr") {
        let ([w, h, d], _) = rule_spec(r);
        return DestKind::FitR(w, h, d);
    }
    match s {
        "fit" => DestKind::Fit,
        "fith" => DestKind::FitH,
        "fitv" => DestKind::FitV,
        "fitb" => DestKind::FitB,
        "fitbh" => DestKind::FitBH,
        "fitbv" => DestKind::FitBV,
        _ => panic!("dest: {s}"),
    }
}

/// A whatsit from its display.
fn whatsit(d: &str, nums: &mut Nums) -> Option<Whatsit> {
    let w = if let Some(r) = d.strip_prefix("\\pdfliteral") {
        let (mode, r) = if let Some(r) = r.strip_prefix(" page") {
            (LiteralMode::DirectPage, r)
        } else if let Some(r) = r.strip_prefix(" direct") {
            (LiteralMode::DirectAlways, r)
        } else {
            (LiteralMode::SetOrigin, r)
        };
        Whatsit::Literal(mode, braced(r).0)
    } else if let Some(r) = d.strip_prefix("\\special") {
        Whatsit::Special(braced(r).0)
    } else if let Some(r) = d.strip_prefix("\\pdfcolorstack ") {
        let (n, r) = r.split_once(' ').unwrap();
        let n = n.parse().unwrap();
        let cmd = if let Some(r) = r.strip_prefix("set ") {
            ColorStackCmd::Set(braced(r).0)
        } else if let Some(r) = r.strip_prefix("push ") {
            ColorStackCmd::Push(braced(r).0)
        } else if r == "pop" {
            ColorStackCmd::Pop
        } else {
            ColorStackCmd::Current
        };
        Whatsit::ColorStack(n, cmd)
    } else if let Some(r) = d.strip_prefix("\\pdfsetmatrix") {
        Whatsit::SetMatrix(braced(r).0)
    } else if d == "\\pdfsave" {
        Whatsit::Save
    } else if d == "\\pdfrestore" {
        Whatsit::Restore
    } else if d.starts_with("\\pdfrefobj") {
        Whatsit::RefObj(nums.objs.pop_front().unwrap())
    } else if d.starts_with("\\pdfrefxform") {
        Whatsit::RefXForm(nums.xforms.pop_front().unwrap())
    } else if d.starts_with("\\pdfrefximage") {
        Whatsit::RefXImage(nums.ximages.pop_front().unwrap())
    } else if let Some(r) = d.strip_prefix("\\pdfannot") {
        let ([width, height, depth], r) = rule_spec(r);
        Whatsit::Annot {
            objnum: nums.annots.pop_front().unwrap(),
            width,
            height,
            depth,
            data: braced(r).0,
        }
    } else if let Some(r) = d.strip_prefix("\\pdfstartlink") {
        let ([width, height, depth], r) = rule_spec(r);
        let r = r.trim_start();
        let (attr, r) = match r.strip_prefix("attr") {
            Some(r) => {
                let (a, r) = braced(r);
                (Some(a), r.trim_start())
            }
            None => (None, r),
        };
        let r = r.strip_prefix("action").unwrap();
        Whatsit::StartLink {
            objnum: nums.links.pop_front().unwrap(),
            width,
            height,
            depth,
            attr,
            action: action(r, nums),
        }
    } else if d == "\\pdfendlink" {
        Whatsit::EndLink
    } else if let Some(r) = d.strip_prefix("\\pdfdest ") {
        let (struct_num, r) = match r.strip_prefix("struct") {
            Some(r) => {
                let end = r.find(' ').unwrap();
                (Some(r[..end].parse().unwrap()), &r[end + 1..])
            }
            None => (None, r),
        };
        let (id, r) = ident(r);
        Whatsit::Dest {
            id,
            struct_num,
            kind: dest_kind(r),
        }
    } else if let Some((start, r)) = d
        .strip_prefix("\\pdfthread")
        .map(|r| (false, r))
        .or_else(|| d.strip_prefix("\\pdfstartthread").map(|r| (true, r)))
    {
        let ([width, height, depth], r) = rule_spec(r);
        let r = r.trim_start();
        let (attr, r) = match r.strip_prefix("attr") {
            Some(r) => {
                let (a, r) = braced(r);
                (Some(a), r.trim_start())
            }
            None => (None, r),
        };
        let (id, _) = ident(r.trim_start());
        Whatsit::Thread {
            start,
            id,
            width,
            height,
            depth,
            attr,
        }
    } else if d == "\\pdfendthread" {
        Whatsit::EndThread
    } else if d == "\\pdfsavepos" {
        Whatsit::SavePos
    } else if d == "\\pdfinterwordspaceon" {
        Whatsit::InterwordSpaceOn
    } else if d == "\\pdfinterwordspaceoff" {
        Whatsit::InterwordSpaceOff
    } else if d == "\\pdffakespace" {
        Whatsit::FakeSpace
    } else if d == "\\pdfrunninglinkoff" {
        Whatsit::RunningLinkOff
    } else if d == "\\pdfrunninglinkon" {
        Whatsit::RunningLinkOn
    } else if d.starts_with("\\write")
        || d.starts_with("\\openout")
        || d.starts_with("\\closeout")
        || d.starts_with("\\setlanguage")
    {
        return None;
    } else {
        panic!("whatsit: {d}")
    };
    Some(w)
}

/// A fixture's run: its fonts by identifier, its boxes.
struct Job {
    name: &'static str,
    src: Src,
    ids: HashMap<String, FontId>,
    w: PdfWriter,
    shown: Vec<Shown>,
    /// The next shown box.
    next: usize,
    nums: Nums,
    /// The whatsits made, by handle.
    ws: HashMap<u32, Option<Whatsit>>,
    /// The boxes taken.
    taken: Vec<usize>,
    /// The next whatsit handle.
    handles: u32,
    /// The fonts `do_vf` saw.
    vf_done: std::collections::BTreeSet<FontId>,
    /// The bytes flushed, a chunk a page.
    bytes: PdfBytes,
    /// The pages shipped, with the parameters at each (for the relaxed
    /// mode's check).
    pages: Vec<(BoxNode, PdfParams)>,
}

/// pdfTeX's banner on the machine that made the fixtures.
const BANNER: &str = "This is pdfTeX, Version 3.141592653-2.6-1.40.29 (TeX Live 2026/Arch Linux) kpathsea version 6.4.2";

impl Job {
    fn new(name: &'static str, compress_level: i32) -> Job {
        let p = PdfParams {
            compress_level,
            // the fixtures were made with SOURCE_DATE_EPOCH=0
            date: phitex_pdf::pdf_date(0),
            banner: BANNER.to_owned(),
            output_file_name: format!("{name}.pdf"),
            ..PdfParams::default()
        };
        Job {
            name,
            src: Src::new(),
            ids: HashMap::new(),
            w: PdfWriter::new(p),
            shown: Vec::new(),
            next: 0,
            nums: Nums::default(),
            ws: HashMap::new(),
            taken: Vec::new(),
            handles: 0,
            vf_done: std::collections::BTreeSet::new(),
            bytes: PdfBytes::new(),
            pages: Vec::new(),
        }
    }

    /// `\font\ident=name [at size]`.
    fn font(&mut self, ident: &str, name: &str, at: Option<Scaled>) -> FontId {
        let f = self.src.font(name, at);
        self.ids.insert(ident.to_owned(), f);
        f
    }

    /// Reads the log once the fonts are known.
    fn read(&mut self) {
        let text = trace(self.name);
        self.shown = read_log(&text);
    }

    /// Box `k` of the log, its whatsits made.
    fn take_at(&mut self, k: usize) -> BoxNode {
        let mut r = Reader {
            fonts: &self.ids,
            next: self.handles,
            whatsits: Vec::new(),
        };
        let mut list = r.list(&self.shown[k].lines, "#");
        self.handles = r.next;
        let Some(Node::Box(mut b)) = list.pop() else {
            panic!("no box")
        };
        repack(&mut b, &self.src);
        for (h, d) in &r.whatsits {
            self.ws.insert(*h, whatsit(d, &mut self.nums));
        }
        self.taken.push(k);
        self.shown[k].boxed = Some(b.clone());
        b
    }

    /// `do_vf` for the fonts of `b` not yet output, in the order they
    /// are output.
    fn prepare_vf(&mut self, b: &BoxNode) {
        fn walk(list: &[Node], out: &mut Vec<FontId>) {
            for n in list {
                match n {
                    Node::Char { font, .. } | Node::Lig(Ligature { font, .. }) => out.push(*font),
                    Node::Box(b) => walk(&b.list, out),
                    Node::Glue(GlueNode {
                        leader: Some(l), ..
                    }) => walk(std::slice::from_ref(&**l), out),
                    Node::Disc(d) => walk(&d.replace, out),
                    _ => {}
                }
            }
        }
        let mut fonts = Vec::new();
        walk(std::slice::from_ref(&Node::Box(b.clone())), &mut fonts);
        for f in fonts {
            self.do_vf(f);
        }
    }

    fn do_vf(&mut self, f: FontId) {
        if !self.vf_done.insert(f) {
            return;
        }
        self.src.load_vf(f);
        let locals: Vec<FontId> = self
            .src
            .get(f)
            .vf
            .as_ref()
            .map(|v| v.fonts.iter().map(|x| x.1).collect())
            .unwrap_or_default();
        for k in locals {
            self.do_vf(k);
        }
    }

    /// Ships the next page of the log.
    fn ship(&mut self) {
        while !self.shown[self.next].page {
            self.next += 1;
        }
        self.next += 1;
        let b = self.take_at(self.next - 1);
        self.prepare_vf(&b);
        for i in 0..self.shown.len() {
            if !self.shown[i].page
                && let Some(f) = self.shown[i].boxed.clone()
            {
                self.prepare_vf(&f);
            }
        }
        self.pages.push((b.clone(), self.w.params_mut().clone()));
        let ws = &self.ws;
        self.w
            .ship_out_with(&b, &self.src, &mut |h, _| ws[&h].clone())
            .unwrap();
        self.w.flush(&mut self.bytes);
    }

    /// Ships the remaining pages.
    fn ship_all(&mut self) {
        while self.shown[self.next..].iter().any(|s| s.page) {
            self.ship();
        }
    }

    /// `\pdfxform` of the `k`th form the log shows shipped, with
    /// `attr`/`resources`.
    fn xform(&mut self, k: usize, attr: Option<&str>, resources: Option<&str>) -> usize {
        let i = (0..self.shown.len())
            .filter(|&i| self.shown[i].shipped && !self.shown[i].page)
            .nth(k)
            .unwrap();
        let b = self.take_at(i);
        self.w.xform(
            attr.map(|s| s.as_bytes().to_vec()),
            resources.map(|s| s.as_bytes().to_vec()),
            b,
        )
    }

    /// `\immediate\pdfxform`: the form written now.
    fn write_xform(&mut self, n: usize) {
        let ws = &self.ws;
        self.w
            .write_xform(n, &self.src, &mut |h, _| ws[&h].clone())
            .unwrap();
    }

    /// The file.
    fn output(self) -> Vec<u8> {
        phitex_pdf::finish(self.w, &self.bytes).unwrap()
    }

    fn finish(self) {
        let name = self.name;
        let got = self.output();
        let want = oracle(&format!("{name}.pdf"));
        compare(name, &got, &want);
    }
}

/// The first difference, with the objects it is in.
fn compare(name: &str, got: &[u8], want: &[u8]) {
    if got == want {
        return;
    }
    let at = got
        .iter()
        .zip(want)
        .position(|(a, b)| a != b)
        .unwrap_or(got.len().min(want.len()));
    let show = |b: &[u8]| {
        String::from_utf8_lossy(&b[at.saturating_sub(400).min(b.len())..(at + 300).min(b.len())])
            .into_owned()
    };
    panic!(
        "{name}.pdf: first difference at byte {at} (got {} bytes, want {})\n--- got:\n{}\n--- want:\n{}",
        got.len(),
        want.len(),
        show(got),
        show(want)
    );
}

// ---- the fixtures ----

/// `\font\tenrm=cmr10` and text only.
fn text_job(name: &'static str, fonts: &[(&str, &str, Option<Scaled>)], level: i32) {
    let mut j = Job::new(name, level);
    for &(i, n, at) in fonts {
        j.font(i, n, at);
    }
    j.read();
    j.ship_all();
    j.finish();
}

#[test]
fn hello() {
    text_job("hello", &[("tenrm", "cmr10", None)], 0);
}

#[test]
fn compressed() {
    text_job("compressed", &[("tenrm", "cmr10", None)], 9);
}

#[test]
fn mixed1() {
    text_job(
        "mixed1",
        &[("mi", "cmmi10", None), ("big", "cmr10", Some(12 * PT))],
        0,
    );
}

#[test]
fn mixed2() {
    text_job(
        "mixed2",
        &[
            ("tenrm", "cmr10", None),
            ("big", "cmr10", Some(12 * PT)),
            ("mi", "cmmi10", None),
        ],
        0,
    );
}

#[test]
fn guess() {
    // \pdfdecimaldigits=2 before the second page
    let mut j = Job::new("guess", 0);
    j.font("tenrm", "cmr10", None);
    j.font("big", "cmr10", Some(12 * PT));
    j.font("bf", "cmbx10", None);
    j.read();
    j.ship();
    j.w.params_mut().decimal_digits = 2;
    j.ship_all();
    j.finish();
}

#[test]
fn rules() {
    let mut j = Job::new("rules", 0);
    j.font("tenrm", "cmr10", None);
    j.font("big", "cmr10", Some(12 * PT));
    j.font("bf", "cmbx10", None);
    j.font("mi", "cmmi10", None);
    j.w.params_mut().h_offset = -20 * PT;
    j.w.params_mut().v_offset = 5 * PT;
    j.read();
    j.ship_all();
    j.finish();
}

#[test]
fn berasizes() {
    text_job(
        "berasizes",
        &[
            ("a", "fvmr8t", Some(9 * PT)),
            ("b", "fvmr8t", Some(11 * PT)),
            ("c", "fvsr8t", Some(8 * PT)),
            ("d", "fvsr8t", None),
        ],
        0,
    );
}

#[test]
fn random() {
    text_job("random", &[("r", "cmr10", None)], 0);
}

#[test]
fn objstm() {
    let mut j = Job::new("objstm", 9);
    j.font("tenrm", "cmr10", None);
    j.w.params_mut().minor_version = 5;
    j.w.params_mut().obj_compress_level = 2;
    j.read();
    j.ship_all();
    j.finish();
}

#[test]
fn mag() {
    // \mag=1500 \pdfdecimaldigits=2 \pdfpageattr{/Rotate 90}
    // \pdfpagesattr{/Rotate 0}; 100truept = 66.66666pt
    let mut j = Job::new("mag", 9);
    j.font("tenrm", "cmr10", None);
    let p = j.w.params_mut();
    p.mag = 1500;
    p.decimal_digits = 2;
    p.page_attr = Some(b"/Rotate 90".to_vec());
    p.pages_attr = Some(b"/Rotate 0".to_vec());
    j.read();
    j.ship_all();
    j.finish();
}

#[test]
fn literal() {
    text_job("literal", &[("r", "cmr10", None)], 0);
}

#[test]
fn colorstack() {
    let mut j = Job::new("colorstack", 0);
    j.font("r", "cmr10", None);
    assert_eq!(j.w.colorstack_init(true, true, b"0 g 0 G").unwrap(), 1);
    assert_eq!(j.w.colorstack_init(false, true, b"1 0 0 rg").unwrap(), 2);
    assert_eq!(j.w.colorstack_init(false, false, b"0 0 1 rg").unwrap(), 3);
    j.read();
    j.ship_all();
    j.finish();
}

#[test]
fn tounicode0() {
    let mut j = Job::new("tounicode0", 0);
    j.w.params_mut().gen_tounicode = 1;
    j.w.glyph_to_unicode(b"a", b"zz");
    j.font("r", "cmr10", None);
    j.read();
    j.ship_all();
    j.finish();
}

#[test]
fn tounicode() {
    let mut j = Job::new("tounicode", 0);
    j.w.params_mut().gen_tounicode = 1;
    for (g, u) in [
        ("A", "0041"),
        ("B", "0042"),
        ("C", "0043"),
        ("D", "0044"),
        ("E", "0045"),
        ("F", "00FF"),
        ("G", "0100"),
        ("ff", "0066 0066"),
        ("tfm:cmr10/C", "1D400"),
        ("fi", "110000"),
        ("a", "zz"),
        ("b", "0062"),
        ("c", " 0063 "),
        ("fl", "0066"),
        ("fl", "FB02"),
    ] {
        j.w.glyph_to_unicode(g.as_bytes(), u.as_bytes());
    }
    j.font("t", "ecrm1000", None);
    j.font("r", "cmr10", None);
    j.read();
    j.ship_all();
    j.finish();
}

#[test]
fn trailerid() {
    let mut j = Job::new("trailerid", 0);
    j.font("r", "cmr10", None);
    let p = j.w.params_mut();
    p.obj_compress_level = 0;
    p.info_omit_date = 1;
    p.suppress_ptex_info = -1;
    j.w.trailer_id(b"abc");
    j.w.catalog(
        b"/PageMode /UseOutlines",
        Some(&Action::Page {
            file: None,
            page: 1,
            spec: b"/Fit".to_vec(),
            new_window: 0,
            struct_id: None,
        }),
    )
    .unwrap();
    j.read();
    j.ship_all();
    j.finish();
}

#[test]
fn pageref() {
    let mut j = Job::new("pageref", 0);
    j.font("r", "cmr10", None);
    j.read();
    j.w.page_ref(2);
    j.ship();
    j.w.page_ref(1);
    j.w.page_ref(3);
    j.ship();
    j.finish();
}

#[test]
fn interword() {
    let mut j = Job::new("interword", 0);
    j.font("r", "cmr10", None);
    j.font("t", "ptmr8r", None);
    j.read();
    j.src.load_space();
    j.ship_all();
    j.finish();
}

#[test]
fn vf() {
    let mut j = Job::new("vf", 0);
    j.font("r", "cmr10", None);
    j.font("t", "ptmr7t", None);
    j.font("h", "phvr7t", Some(12 * PT));
    j.read();
    j.ship();
    j.font("tb", "ptmb7t", None);
    j.font("r", "cmr10", Some(12 * PT));
    let x = j.xform(0, None, None);
    j.nums.xforms.push_back(x);
    j.ship();
    j.finish();
}

#[test]
fn bera() {
    let mut j = Job::new("bera", 0);
    j.font("r", "fver8t", None);
    j.font("c", "fver8c", None);
    j.font("m", "fvmr8t", Some(9 * PT));
    j.font("b", "fveb8t", None);
    j.font("s", "fvsr8t", None);
    j.font("o", "fvero8t", None);
    j.read();
    let x = j.xform(0, None, None);
    j.nums.xforms.push_back(x);
    j.ship();
    j.finish();
}

#[test]
fn xform() {
    let mut j = Job::new("xform", 0);
    j.font("r", "cmr10", None);
    j.read();
    let x = j.xform(1, Some("/Group 1"), Some("/ProcSet [/PDF]"));
    let y = j.xform(0, None, None);
    j.write_xform(y);
    j.nums.xforms.push_back(x);
    let z = j.xform(2, None, None);
    j.nums.xforms.extend([x, x, y, z]);
    j.ship();
    j.finish();
}

#[test]
fn obj() {
    let mut j = Job::new("obj", 0);
    j.font("r", "cmr10", None);
    j.w.params_mut().compress_level = 0;
    let t = |s: &str| ObjBody::Text(s.as_bytes().to_vec());
    let file = oracle("obj.tex");
    let (a, _) = j.w.obj(None, false, None, t("<< /A 1 >>"));
    let (b, _) = j.w.obj(None, false, None, t("[1 2 3]"));
    j.w.write_obj(b).unwrap();
    let c = j.w.obj_reserve();
    j.w.obj(
        Some(i64::try_from(c).unwrap()),
        false,
        None,
        t("(reserved)"),
    );
    let (d, _) = j.w.obj(None, true, Some(b"/X 1".to_vec()), t("q Q"));
    let (k, _) = j.w.obj(None, true, None, t("BT ET"));
    j.w.write_obj(k).unwrap();
    let (k, _) = j.w.obj(None, false, None, ObjBody::File(file.clone()));
    j.w.write_obj(k).unwrap();
    let (e, _) = j.w.obj(None, true, None, ObjBody::File(file));
    j.read();
    j.nums.objs.extend([a, d, e, c]);
    assert!(j.w.obj(Some(999), false, None, t("bad")).1);
    j.w.catalog(b"/PageMode /UseNone", None).unwrap();
    j.w.info(b"/Title (Objects) /Author (Q)");
    j.w.names(b"/EmbeddedFiles << /Names [] >>");
    j.w.trailer(b"/Extra 1");
    let p = j.w.params_mut();
    p.page_attr = Some(b"/Rotate 0".to_vec());
    p.pages_attr = Some(b"/CropBox [0 0 200 300]".to_vec());
    p.page_resources = Some(b"/Pattern << >>".to_vec());
    j.ship();
    j.finish();
}

#[test]
fn rerun() {
    let mut j = Job::new("rerun", 0);
    j.font("r", "cmr10", None);
    j.w.params_mut().compress_level = 0;
    let t = |s: &str| ObjBody::Text(s.as_bytes().to_vec());
    j.w.obj(None, false, None, t("1"));
    j.w.obj(None, false, None, t("2"));
    let (n, _) = j.w.obj(None, false, None, t("3"));
    assert!(n > 2);
    let (k, _) = j.w.obj(None, false, None, t("4"));
    j.w.write_obj(k).unwrap();
    let (x, _) = j.w.obj(None, false, None, t(&format!("({k})")));
    j.read();
    j.nums.objs.push_back(x);
    j.ship_all();
    j.finish();
}

fn name(s: &str) -> Ident {
    Ident::Name(s.as_bytes().to_vec())
}

fn goto(id: Ident) -> Action {
    Action::Goto {
        file: None,
        id,
        new_window: 0,
        struct_id: None,
    }
}

#[test]
fn outline() {
    let mut j = Job::new("outline", 0);
    j.font("r", "cmr10", None);
    j.w.params_mut().compress_level = 0;
    j.w.outline(None, &goto(name("a")), 2, b"First").unwrap();
    j.w.outline(None, &goto(name("b")), 0, b"Sub one").unwrap();
    j.w.outline(
        Some(b"/C [1 0 0]".to_vec()),
        &goto(name("c")),
        -1,
        b"Sub two",
    )
    .unwrap();
    j.w.outline(None, &goto(Ident::Num(3)), 0, b"Deep").unwrap();
    j.w.outline(
        None,
        &Action::User(b"<< /S /URI /URI (http://y) >>".to_vec()),
        0,
        b"Second",
    )
    .unwrap();
    j.w.outline(
        None,
        &Action::Page {
            file: None,
            page: 1,
            spec: b"/XYZ 0 0 null".to_vec(),
            new_window: 0,
            struct_id: None,
        },
        0,
        b"Third (with) parens",
    )
    .unwrap();
    j.read();
    j.ship_all();
    j.finish();
}

#[test]
fn thread() {
    let mut j = Job::new("thread", 0);
    j.font("r", "cmr10", None);
    j.w.params_mut().compress_level = 0;
    j.read();
    j.ship();
    let l = j.w.new_objnum();
    j.nums.links.push_back(l);
    j.ship();
    j.finish();
}

#[test]
fn annot() {
    let mut j = Job::new("annot", 0);
    j.font("r", "cmr10", None);
    j.w.params_mut().compress_level = 0;
    let a = j.w.new_objnum();
    let ra = j.w.new_objnum();
    j.nums.annots.extend([a, ra]);
    // the links in reading order: 2 + U, N, P, M, R, T + nested 2 + across 1
    for _ in 0..10 {
        let l = j.w.new_objnum();
        j.nums.links.push_back(l);
    }
    j.nums.new_window.push_back(1);
    j.read();
    j.ship_all();
    j.finish();
}

/// `\\pdfximage` of `file`: an oracle fixture's file by its name, any
/// other by its `kpsewhich` path.
fn image(file: &str, res: i32, f: impl FnOnce(&mut ImageSpec)) -> ImageSpec {
    let local = format!(
        "{}/../../../oracle/fixtures/pdf/{file}",
        env!("CARGO_MANIFEST_DIR")
    );
    let (name, data) = if let Ok(d) = std::fs::read(&local) {
        (format!("./{file}"), d)
    } else {
        let p = kpse_path(file).unwrap();
        let d = std::fs::read(&p).unwrap();
        (p, d)
    };
    let mut s = ImageSpec {
        width: RUNNING,
        height: RUNNING,
        depth: RUNNING,
        attr: None,
        page: 1,
        named: None,
        colorspace: 0,
        page_box: PageBox::Crop,
        name: name.into_bytes(),
        data: data.into(),
        image_resolution: res,
        inclusion_errorlevel: 0,
    };
    f(&mut s);
    s
}

#[test]
fn savepos() {
    let mut j = Job::new("savepos", 0);
    j.font("r", "cmr10", None);
    j.w.params_mut().compress_level = 0;
    j.read();
    j.ship_all();
    j.finish();
}

#[test]
fn misc() {
    let mut j = Job::new("misc", 0);
    let r = j.font("r", "cmr10", None);
    j.w.params_mut().compress_level = 0;
    let b = j.font("b", "cmbx10", None);
    j.w.font_attr(b, b"/Extra (bold)", &j.src);
    j.w.include_chars(b, b"xyz", &j.src).unwrap();
    j.w.font_name(r, &j.src).unwrap();
    j.w.font_objnum(r, &j.src).unwrap();
    j.w.font_name(b, &j.src).unwrap();
    j.w.font_objnum(b, &j.src).unwrap();
    j.w.map_line(b"+cmbx10 CMBX10 <cmbx10.pfb");
    j.read();
    j.ship_all();
    j.finish();
}

#[test]
fn jpgres() {
    let mut j = Job::new("jpgres", 0);
    j.font("r", "cmr10", None);
    j.w.params_mut().compress_level = 0;
    j.read();
    for (f, res) in [
        ("jpg-jfif0.jpg", 0),
        ("jpg-jfif0.jpg", 96),
        ("jpg-jfifcm.jpg", 0),
        ("jpg-exif.jpg", 0),
        ("jpg-exifcm.jpg", 0),
        ("jpg-exifcm.jpg", 72),
    ] {
        let n = j.w.ximage(image(f, res, |_| {})).unwrap();
        j.nums.ximages.push_back(n);
    }
    j.ship_all();
    j.finish();
}

#[test]
fn ximage() {
    let mut j = Job::new("ximage", 0);
    j.font("r", "cmr10", None);
    j.w.params_mut().compress_level = 0;
    j.read();
    let p = j.w.ximage(image("example-image.png", 0, |_| {})).unwrap();
    let jp =
        j.w.ximage(image("example-image.jpg", 0, |s| s.width = 50 * PT))
            .unwrap();
    let f =
        j.w.ximage(image("example-image.pdf", 0, |s| {
            s.height = 40 * PT;
            s.depth = 5 * PT;
        }))
        .unwrap();
    let q =
        j.w.ximage(image("example-image-a.png", 0, |s| {
            s.width = 30 * PT;
            s.height = 20 * PT;
            s.attr = Some(b"/Interpolate true".to_vec());
        }))
        .unwrap();
    j.nums.ximages.extend([p, jp, f, q, jp, f]);
    j.ship_all();
    j.finish();
}

#[test]
fn ximage2() {
    let mut j = Job::new("ximage2", 0);
    j.font("r", "cmr10", None);
    let p = j.w.params_mut();
    p.compress_level = 0;
    p.inclusion_copy_fonts = 1;
    j.read();
    let a =
        j.w.ximage(image("example-image-a.pdf", 300, |_| {}))
            .unwrap();
    let b =
        j.w.ximage(image("example-image-a.jpg", 300, |_| {}))
            .unwrap();
    let c =
        j.w.ximage(image("example-image-b.pdf", 300, |s| {
            s.page_box = PageBox::Media;
        }))
        .unwrap();
    j.nums.ximages.extend([a, b, c]);
    j.ship_all();
    j.finish();
}

#[test]
fn readback() {
    let mut j = Job::new("readback", 0);
    j.font("r", "cmr10", None);
    j.w.params_mut().compress_level = 0;
    j.read();
    j.w.obj(None, false, None, ObjBody::Text(b"1".to_vec()));
    let r = j.ids["r"];
    let params = PackParams {
        overfull_rule: 0,
        ..PackParams::default()
    };
    let b = hpack(
        vec![Node::Char {
            font: r,
            ch: u16::from(b'x'),
        }],
        Spec::Additional(0),
        &j.src,
        &params,
        false,
    )
    .node;
    j.w.xform(None, None, b);
    j.w.ximage(image("example-image.png", 0, |_| {})).unwrap();
    let a = j.w.new_objnum();
    j.nums.annots.push_back(a);
    let l = j.w.new_objnum();
    j.nums.links.push_back(l);
    assert!(
        j.w.obj(Some(9999), false, None, ObjBody::Text(b"x".to_vec()))
            .1
    );
    j.ship_all();
    j.finish();
}

// ---- goldens made for this crate (see the README) ----

/// The PNG fixtures, each `width 20pt`.
fn png_job(name: &'static str, hicolor: i32) {
    let mut j = Job::new(name, 0);
    j.font("r", "cmr10", None);
    j.w.params_mut().image_hicolor = hicolor;
    j.read();
    for f in [
        "png-gray.png",
        "png-gray16.png",
        "png-rgb16.png",
        "png-pal.png",
        "png-paltrns.png",
        "png-rgba.png",
        "png-graya.png",
        "png-gama.png",
    ] {
        let n = j.w.ximage(image(f, 0, |s| s.width = 20 * PT)).unwrap();
        j.nums.ximages.push_back(n);
    }
    j.ship_all();
    j.finish();
}

#[test]
fn png() {
    png_job("png", 1);
}

#[test]
fn png8() {
    png_job("png8", 0);
}

#[test]
fn jpeg() {
    let mut j = Job::new("jpeg", 0);
    j.font("r", "cmr10", None);
    j.w.params_mut().compress_level = 0;
    j.read();
    let a = j.w.ximage(image("jpg-jfif0.jpg", 0, |_| {})).unwrap();
    let b =
        j.w.ximage(image("example-image.jpg", 0, |s| {
            s.width = 40 * PT;
            s.attr = Some(b"/Interpolate true".to_vec());
        }))
        .unwrap();
    let c =
        j.w.ximage(image("jpg-exif.jpg", 0, |s| {
            s.height = 30 * PT;
            s.depth = 3 * PT;
        }))
        .unwrap();
    j.nums.ximages.extend([a, b, c, b, a]);
    j.ship_all();
    j.finish();
}

#[test]
fn ownpdf() {
    let mut j = Job::new("ownpdf", 0);
    j.font("r", "cmr10", None);
    let p = j.w.params_mut();
    p.compress_level = 0;
    p.inclusion_copy_fonts = 1;
    j.read();
    let a = j.w.ximage(image("hello.pdf", 0, |_| {})).unwrap();
    let b =
        j.w.ximage(image("objstm.pdf", 0, |s| {
            s.width = 100 * PT;
            s.page = 2;
        }))
        .unwrap();
    let c =
        j.w.ximage(image("colorstack.pdf", 0, |s| s.page_box = PageBox::Trim))
            .unwrap();
    j.nums.ximages.extend([a, b, c, a]);
    j.ship_all();
    j.finish();
}

#[test]
fn ownpdf0() {
    let mut j = Job::new("ownpdf0", 0);
    j.font("r", "cmr10", None);
    let p = j.w.params_mut();
    p.compress_level = 0;
    p.inclusion_copy_fonts = 0;
    j.read();
    let a = j.w.ximage(image("hello.pdf", 0, |_| {})).unwrap();
    let b =
        j.w.ximage(image("objstm.pdf", 0, |s| {
            s.width = 100 * PT;
            s.page = 2;
        }))
        .unwrap();
    let c =
        j.w.ximage(image("colorstack.pdf", 0, |s| s.page_box = PageBox::Trim))
            .unwrap();
    j.nums.ximages.extend([a, b, c, a]);
    j.ship_all();
    j.finish();
}

#[test]
fn linkbreak() {
    let mut j = Job::new("linkbreak", 0);
    j.font("r", "cmr10", None);
    j.w.params_mut().compress_level = 0;
    j.read();
    let l = j.w.new_objnum();
    j.nums.links.push_back(l);
    j.ship();
    let l = j.w.new_objnum();
    j.nums.links.push_back(l);
    j.ship_all();
    j.finish();
}

#[test]
fn outlinetree() {
    let mut j = Job::new("outlinetree", 0);
    j.font("r", "cmr10", None);
    j.w.params_mut().compress_level = 0;
    let n = |k| goto(Ident::Num(k));
    j.w.outline(None, &n(1), 3, b"One").unwrap();
    j.w.outline(None, &n(2), -2, b"One.A").unwrap();
    j.w.outline(None, &n(3), 0, b"One.A.i").unwrap();
    j.w.outline(None, &n(4), 0, b"One.A.ii").unwrap();
    j.w.outline(None, &n(5), 0, b"One.B").unwrap();
    j.w.outline(None, &goto(name("x")), 1, b"Two").unwrap();
    let page = Action::Page {
        file: None,
        page: 2,
        spec: b"/Fit".to_vec(),
        new_window: 0,
        struct_id: None,
    };
    j.w.outline(None, &page, 1, b"Two.A").unwrap();
    j.w.outline(None, &n(1), 0, b"Two.A.i").unwrap();
    j.w.outline(Some(b"/F 2".to_vec()), &goto(name("x")), 0, b"Three")
        .unwrap();
    j.read();
    j.ship_all();
    j.finish();
}

#[test]
fn csform() {
    let mut j = Job::new("csform", 0);
    j.font("r", "cmr10", None);
    j.w.params_mut().compress_level = 0;
    assert_eq!(j.w.colorstack_init(true, true, b"0 g 0 G").unwrap(), 1);
    j.read();
    let f = j.xform(0, None, None);
    let g = j.xform(1, None, None);
    j.nums.xforms.extend([f, g, g, f]);
    j.ship_all();
    j.finish();
}

#[test]
fn objstm2() {
    let mut j = Job::new("objstm2", 9);
    j.font("r", "cmr10", None);
    let p = j.w.params_mut();
    p.minor_version = 5;
    p.obj_compress_level = 2;
    let t = |s: &str| ObjBody::Text(s.as_bytes().to_vec());
    let (a, _) = j.w.obj(None, false, None, t("<< /A 1 >>"));
    let (b, _) = j.w.obj(None, false, None, t("[1 2]"));
    j.w.write_obj(b).unwrap();
    let (d, _) = j.w.obj(None, true, None, t("q Q"));
    j.w.outline(None, &goto(name("a")), 0, b"First").unwrap();
    j.w.outline(None, &goto(name("b")), 0, b"Second").unwrap();
    j.read();
    let f = j.xform(0, None, None);
    j.w.info(b"/Title (Streams)");
    j.w.catalog(b"/PageMode /UseOutlines", None).unwrap();
    j.nums.objs.extend([a, d]);
    j.nums.xforms.push_back(f);
    let n = j.w.new_objnum();
    j.nums.annots.push_back(n);
    j.ship_all();
    j.finish();
}

#[test]
fn tounicode2() {
    let mut j = Job::new("tounicode2", 0);
    j.w.params_mut().gen_tounicode = 1;
    j.w.params_mut().compress_level = 0;
    j.font("r", "cmr10", None);
    j.font("i", "cmmi10", None);
    j.font("s", "cmsy10", None);
    j.font("t", "cmtt10", None);
    j.w.glyph_to_unicode(b"ff", b"0066 0066");
    j.w.glyph_to_unicode(b"dotlessi", b"0131");
    j.read();
    j.ship_all();
    j.finish();
}

// ---- the state as a value ----

/// Runs `setup`, ships `k` pages, snapshots the state and its bytes,
/// and ships the rest; then runs the same job again to page `k` and
/// continues from the snapshot instead. Both files are the oracle's.
fn split_run(name: &'static str, level: i32, setup: &dyn Fn(&mut Job), k: usize) {
    let mut a = Job::new(name, level);
    setup(&mut a);
    for _ in 0..k {
        a.ship();
    }
    let state = a.w.clone();
    let bytes = a.bytes.clone();
    assert_eq!(bytes.chunks().len(), k);
    a.ship_all();
    let straight = a.output();
    let mut b = Job::new(name, level);
    setup(&mut b);
    for _ in 0..k {
        b.ship();
    }
    b.w = state;
    b.bytes = bytes;
    b.ship_all();
    let resumed = b.output();
    assert!(straight == resumed, "{name}: resumed at page {k} differs");
    compare(name, &resumed, &oracle(&format!("{name}.pdf")));
}

#[test]
fn snapshot_objstm() {
    // objects left in an object stream across the snapshot
    let setup = |j: &mut Job| {
        j.font("tenrm", "cmr10", None);
        j.w.params_mut().minor_version = 5;
        j.w.params_mut().obj_compress_level = 2;
        j.read();
    };
    for k in [1, 7, 30] {
        split_run("objstm", 9, &setup, k);
    }
}

#[test]
fn snapshot_annot() {
    let setup = |j: &mut Job| {
        j.font("r", "cmr10", None);
        j.w.params_mut().compress_level = 0;
        let a = j.w.new_objnum();
        let ra = j.w.new_objnum();
        j.nums.annots.extend([a, ra]);
        for _ in 0..10 {
            let l = j.w.new_objnum();
            j.nums.links.push_back(l);
        }
        j.nums.new_window.push_back(1);
        j.read();
    };
    split_run("annot", 0, &setup, 1);
}

#[test]
fn snapshot_ownpdf() {
    // images and included documents shared by the clone
    let setup = |j: &mut Job| {
        j.font("r", "cmr10", None);
        let p = j.w.params_mut();
        p.compress_level = 0;
        p.inclusion_copy_fonts = 1;
        j.read();
        let a = j.w.ximage(image("hello.pdf", 0, |_| {})).unwrap();
        let b =
            j.w.ximage(image("objstm.pdf", 0, |s| {
                s.width = 100 * PT;
                s.page = 2;
            }))
            .unwrap();
        let c =
            j.w.ximage(image("colorstack.pdf", 0, |s| s.page_box = PageBox::Trim))
                .unwrap();
        j.nums.ximages.extend([a, b, c, a]);
    };
    split_run("ownpdf", 0, &setup, 1);
}

#[test]
fn ttf() {
    let mut j = Job::new("ttf", 0);
    j.font("r", "cmr10", None);
    j.w.map_line(
        b"=Arvo-Italic-tlf-ot1 Arvo-Italic \" AutoEnc_zmrji7vzn237ysql425ntisraa ReEncodeFont \" <[a_zmrji7.enc <<Arvo-Italic.ttf",
    );
    j.font("a", "Arvo-tlf-ot1", None);
    j.font("i", "Arvo-Italic-tlf-ot1", None);
    j.font("b", "Arvo-Bold-tlf-ot1", Some(12 * PT));
    j.read();
    j.ship_all();
    j.finish();
}

// ---- object sets of the LaTeX oracles ----

/// An object with the objects it refers to, followed (fonts' objects
/// form no cycles): streams decoded, without `/Length`, `/Filter` and
/// `/DecodeParms`, numbers as the reader read them.
fn canon(doc: &phitex_codecs::pdfread::Doc, o: &phitex_codecs::pdfread::Obj, out: &mut Vec<u8>) {
    use phitex_codecs::pdfread::Obj;
    match o {
        Obj::Ref(_) => canon(doc, &doc.follow(o), out),
        Obj::Array(a) => {
            out.push(b'[');
            for x in a {
                canon(doc, x, out);
                out.push(b' ');
            }
            out.push(b']');
        }
        Obj::Dict(d) => {
            out.extend_from_slice(b"<<");
            for (k, v) in &d.0 {
                out.push(b'/');
                out.extend_from_slice(k);
                out.push(b' ');
                canon(doc, v, out);
                out.push(b' ');
            }
            out.extend_from_slice(b">>");
        }
        Obj::Stream(s) => {
            out.extend_from_slice(b"<<");
            for (k, v) in &s.dict.0 {
                if matches!(&k[..], b"Length" | b"Filter" | b"DecodeParms") {
                    continue;
                }
                out.push(b'/');
                out.extend_from_slice(k);
                out.push(b' ');
                canon(doc, v, out);
                out.push(b' ');
            }
            out.extend_from_slice(b">>stream ");
            out.extend_from_slice(&doc.decode(s));
        }
        other => out.extend_from_slice(format!("{other:?}").as_bytes()),
    }
}

/// [`canon`] of a font dictionary without its `/Encoding`: an encoding
/// object is shared by the fonts of an encoding file, its differences
/// the codes all of them use (`fe_entry`'s `tx_tree`).
fn canon_font(doc: &phitex_codecs::pdfread::Doc, r: phitex_codecs::pdfread::Ref) -> Vec<u8> {
    use phitex_codecs::pdfread::Obj;
    let mut out = Vec::new();
    if let Obj::Dict(mut d) = doc.fetch(r) {
        d.0.retain(|(k, _)| &k[..] != b"Encoding");
        canon(doc, &Obj::Dict(d), &mut out);
    }
    out
}

/// The fonts of a file's pages and of their forms, by reference.
fn pdf_fonts(doc: &phitex_codecs::pdfread::Doc) -> Vec<phitex_codecs::pdfread::Ref> {
    use phitex_codecs::pdfread::{Dict, Obj};
    fn walk(
        doc: &phitex_codecs::pdfread::Doc,
        res: &Dict,
        out: &mut Vec<phitex_codecs::pdfread::Ref>,
        seen: &mut Vec<phitex_codecs::pdfread::Ref>,
    ) {
        if let Obj::Dict(f) = doc.lookup(res, b"Font") {
            for (_, v) in &f.0 {
                if let Obj::Ref(r) = v
                    && !out.contains(r)
                {
                    out.push(*r);
                }
            }
        }
        if let Obj::Dict(x) = doc.lookup(res, b"XObject") {
            for (_, v) in &x.0 {
                if let Obj::Ref(r) = v
                    && !seen.contains(r)
                {
                    seen.push(*r);
                    // an included PDF's page: its fonts are copied or
                    // replaced (the image fixtures check those)
                    if let Obj::Stream(s) = doc.fetch(*r)
                        && s.dict.get(b"PTEX.FileName").is_none()
                        && let Obj::Dict(r2) = doc.lookup(&s.dict, b"Resources")
                    {
                        walk(doc, &r2, out, seen);
                    }
                }
            }
        }
    }
    let mut out = Vec::new();
    let mut seen = Vec::new();
    for i in 1..=doc.num_pages() {
        if let Some(p) = doc.page(i)
            && let Some(r) = &p.resources
        {
            walk(doc, r, &mut out, &mut seen);
        }
    }
    out
}

/// The codes a subset Type 1 font of pdfTeX used: its `/Encoding`'s
/// `/Differences`, else the `dup <code> /<name> put` of its program.
fn used_codes(doc: &phitex_codecs::pdfread::Doc, font: &phitex_codecs::pdfread::Dict) -> Vec<u16> {
    use phitex_codecs::pdfread::Obj;
    if let Obj::Dict(e) = doc.lookup(font, b"Encoding")
        && let Obj::Array(a) = doc.lookup(&e, b"Differences")
    {
        // the codes of the glyphs of the font's /CharSet
        let charset: Option<Vec<Vec<u8>>> = match doc.lookup(font, b"FontDescriptor") {
            Obj::Dict(desc) => match doc.lookup(&desc, b"CharSet") {
                Obj::Str(cs) => Some(
                    cs.split(|&c| c == b'/')
                        .filter(|g| !g.is_empty())
                        .map(<[u8]>::to_vec)
                        .collect(),
                ),
                _ => None,
            },
            _ => None,
        };
        let mut code = 0;
        let mut out = Vec::new();
        for x in a {
            match x {
                Obj::Int(i) => code = i,
                Obj::Name(n) => {
                    if charset.as_ref().is_none_or(|c| c.contains(&n)) {
                        out.push(u16::try_from(code).unwrap());
                    }
                    code += 1;
                }
                _ => {}
            }
        }
        return out;
    }
    let Obj::Dict(desc) = doc.lookup(font, b"FontDescriptor") else {
        return Vec::new();
    };
    let Obj::Stream(s) = doc.lookup(&desc, b"FontFile") else {
        return Vec::new();
    };
    let data = doc.decode(&s);
    let n1 = doc.lookup(&s.dict, b"Length1").as_int().unwrap_or(0);
    let text = String::from_utf8_lossy(&data[..usize::try_from(n1).unwrap().min(data.len())]);
    let mut out = Vec::new();
    for l in text.lines() {
        let w: Vec<&str> = l.split_whitespace().collect();
        if w.len() == 4 && w[0] == "dup" && w[2].starts_with('/') && w[3] == "put" {
            out.push(w[1].parse().unwrap());
        }
    }
    out
}

/// `\pdfglyphtounicode`s of `glyphtounicode.tex`.
fn glyph_to_unicode_tex() -> Vec<(Vec<u8>, Vec<u8>)> {
    let t = kpse("glyphtounicode.tex").unwrap();
    let t = String::from_utf8_lossy(&t);
    let mut out = Vec::new();
    for l in t.lines() {
        if let Some(r) = l.strip_prefix("\\pdfglyphtounicode{")
            && let Some((g, r)) = r.split_once("}{")
            && let Some((u, _)) = r.split_once('}')
        {
            out.push((g.as_bytes().to_vec(), u.as_bytes().to_vec()));
        }
    }
    out
}

/// The font dictionary phitex-pdf writes for TFM `name` at `at` with
/// `codes` used, canonical.
fn font_alone(
    name: &str,
    at: Option<Scaled>,
    codes: &[u16],
    tounicode: Option<&[(Vec<u8>, Vec<u8>)]>,
) -> Option<Vec<u8>> {
    let mut src = Src::new();
    let f = src.font(name, at);
    let mut w = PdfWriter::new(PdfParams {
        date: phitex_pdf::pdf_date(0),
        banner: BANNER.to_owned(),
        output_file_name: "x.pdf".to_owned(),
        ..PdfParams::default()
    });
    if let Some(t) = tounicode {
        w.params_mut().gen_tounicode = 1;
        for (g, u) in t {
            w.glyph_to_unicode(g, u);
        }
    }
    let list: Vec<Node> = codes.iter().map(|&ch| Node::Char { font: f, ch }).collect();
    let params = PackParams {
        overfull_rule: 0,
        ..PackParams::default()
    };
    let b = hpack(list, Spec::Additional(0), &src, &params, false).node;
    let page = vpackage(vec![Node::Box(b)], Spec::Additional(0), 0, &params).node;
    w.ship_out_with(&page, &src, &mut |_, _| None).ok()?;
    let pdf: std::sync::Arc<[u8]> = w.finish().ok()?.into();
    let doc = phitex_codecs::pdfread::Doc::open(&pdf).ok()?;
    let r = *pdf_fonts(&doc).first()?;
    Some(canon_font(&doc, r))
}

/// The sizes LaTeX loads fonts at, tried after the design size.
const LATEX_SIZES: [Scaled; 14] = [
    5 * PT,
    6 * PT,
    7 * PT,
    8 * PT,
    9 * PT,
    10 * PT,
    655_360 + 62_259, // 10.95pt
    12 * PT,
    943_718,   // 14.4pt
    1_133_773, // 17.28pt
    1_360_527, // 20.74pt
    1_630_535, // 24.88pt
    11 * PT,
    PT * 10 + PT / 2,
];

/// Each Type 1 font of the LaTeX oracles (`oracle/fixtures/latex/pdf`)
/// as phitex-pdf writes it alone, with the codes the file's subset
/// shows used: the font dictionary with all it refers to (widths,
/// encoding, descriptor, the subset font program, `/ToUnicode`) equal
/// to pdfTeX's, object numbers aside.
#[test]
#[ignore = "heavy: every font of the LaTeX oracles at every LaTeX size; run with --ignored"]
fn latex_font_objects() {
    use phitex_codecs::pdfread::{Doc, Obj};
    let dir = format!(
        "{}/../../../oracle/fixtures/latex/pdf",
        env!("CARGO_MANIFEST_DIR")
    );
    let map = parse_map(&kpse("pdftex.map").unwrap());
    let g2u = glyph_to_unicode_tex();
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .filter_map(|e| {
            e.file_name()
                .to_string_lossy()
                .strip_suffix(".pdf")
                .map(str::to_owned)
        })
        .collect();
    names.sort();
    let only = std::env::var("PHITEX_GATE").ok();
    names.retain(|n| only.as_ref().is_none_or(|o| o == n));
    let (mut ok, mut failed) = (0, Vec::new());
    for name in names {
        let data: std::sync::Arc<[u8]> = std::fs::read(format!("{dir}/{name}.pdf")).unwrap().into();
        let doc = Doc::open(&data).unwrap();
        for r in pdf_fonts(&doc) {
            let Obj::Dict(font) = doc.fetch(r) else {
                continue;
            };
            if doc.lookup(&font, b"Subtype").as_name() != Some(b"Type1") {
                continue;
            }
            let Some(base) = doc.lookup(&font, b"BaseFont").as_name().map(<[u8]>::to_vec) else {
                continue;
            };
            let Obj::Dict(desc) = doc.lookup(&font, b"FontDescriptor") else {
                continue;
            };
            if !matches!(doc.lookup(&desc, b"FontFile"), Obj::Stream(_)) {
                continue;
            }
            let ps = if base.len() > 7 && base[6] == b'+' {
                base[7..].to_vec()
            } else {
                base.clone()
            };
            let want = canon_font(&doc, r);
            let codes = used_codes(&doc, &font);
            let tu = matches!(doc.lookup(&font, b"ToUnicode"), Obj::Stream(_));
            let candidates: Vec<&MapEntry> = map
                .values()
                .filter(|m| m.ps_name.as_deref() == Some(&ps[..]) && m.is_type1())
                .collect();
            let mut found = false;
            let mut closest: Option<(usize, String)> = None;
            'c: for m in &candidates {
                let tfm = String::from_utf8_lossy(&m.tfm_name).into_owned();
                if kpse(&format!("{tfm}.tfm")).is_none() {
                    continue;
                }
                for at in std::iter::once(None).chain(LATEX_SIZES.iter().map(|&s| Some(s))) {
                    let got = font_alone(&tfm, at, &codes, tu.then_some(&g2u[..]));
                    if got.as_deref() == Some(&want[..]) {
                        found = true;
                        break 'c;
                    }
                    if let Some(g) = &got {
                        let d = g.iter().zip(&want).take_while(|(a, b)| a == b).count();
                        if closest.as_ref().is_none_or(|c| c.0 < d) {
                            let w = String::from_utf8_lossy(
                                &want[d.saturating_sub(40)..(d + 60).min(want.len())],
                            )
                            .into_owned();
                            let gg = String::from_utf8_lossy(
                                &g[d.saturating_sub(40)..(d + 60).min(g.len())],
                            )
                            .into_owned();
                            closest =
                                Some((d, format!("{tfm} {at:?} at {d}: want {w:?} got {gg:?}")));
                        }
                    }
                }
            }
            if found {
                ok += 1;
            } else {
                failed.push(format!(
                    "{name}: {} ({})",
                    String::from_utf8_lossy(&base),
                    closest.map(|c| c.1).unwrap_or_default()
                ));
            }
        }
    }
    eprintln!("{ok} fonts equal");
    assert!(failed.is_empty(), "fonts differing: {failed:#?}");
}
