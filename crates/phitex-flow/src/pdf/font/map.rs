//! pdfTeX's font map (mapfile.c), copied: which Type 1 file, encoding
//! and PostScript name each TFM font gets in the PDF. A map file is a
//! value (its bytes); an entry is a pure function of the file and the
//! TFM name (the first valid line naming it: TeX Live's map files are
//! read with duplicates ignored).

use std::collections::HashMap;
use std::sync::Arc;

/// mapfile.c's `updatemode`: what a map item does with an entry that
/// already exists.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mode {
    /// `+`: keep the old entry (`FM_DUPIGNORE`).
    DupIgnore,
    /// `=`: replace it (`FM_REPLACE`).
    Replace,
    /// `-`: delete it (`FM_DELETE`).
    Delete,
}

/// ptexmac.h's `F_*` bits of an entry's `type`.
pub const F_INCLUDED: u8 = 0x01;
pub const F_SUBSETTED: u8 = 0x02;
pub const F_STDT1FONT: u8 = 0x04;
pub const F_TYPE1: u8 = 0x10;
pub const F_TRUETYPE: u8 = 0x20;
pub const F_OTF: u8 = 0x40;
pub const F_PK: u8 = 0x80;

/// mapfile.c's `fm_entry`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct MapEntry {
    pub tfm_name: Vec<u8>,
    pub ps_name: Option<Vec<u8>>,
    /// The font descriptor's `/Flags`; -1 if the line gives none.
    pub fd_flags: i32,
    pub ff_name: Option<Vec<u8>>,
    pub encname: Option<Vec<u8>>,
    pub kind: u8,
    /// `SlantFont` and `ExtendFont`, times 1000.
    pub slant: i32,
    pub extend: i32,
    pub pid: i32,
    pub eid: i32,
}

impl MapEntry {
    fn new() -> Self {
        Self {
            tfm_name: Vec::new(),
            ps_name: None,
            fd_flags: -1,
            ff_name: None,
            encname: None,
            kind: 0,
            slant: 0,
            extend: 0,
            pid: -1,
            eid: -1,
        }
    }

    pub fn is(&self, bit: u8) -> bool {
        self.kind & bit != 0
    }

    pub fn is_t1fontfile(&self) -> bool {
        self.ff_name.is_some() && self.is(F_TYPE1)
    }

    /// The `ps_tree`'s key: the PostScript name, slant and extend.
    pub fn ps_key(&self) -> Option<(Vec<u8>, i32, i32)> {
        Some((self.ps_name.clone()?, self.slant, self.extend))
    }

    fn is_reencoded(&self) -> bool {
        self.encname.is_some()
    }
}

/// ptexmac.h's `is_cfg_comment`.
fn is_cfg_comment(c: u8) -> bool {
    matches!(c, 10 | b'*' | b'#' | b';' | b'%')
}

/// The lines of a map file as `fm_scan_line` reads them
/// (`append_char_to_buf`): tabs become spaces, a CR ends a line as an LF
/// does, and runs of spaces (and leading ones) collapse.
pub fn map_file_lines(data: &[u8]) -> Vec<Vec<u8>> {
    let mut lines = Vec::new();
    for_map_file_lines(data, |l| lines.push(l.to_vec()));
    lines
}

/// [`map_file_lines`], each line lent to `f` in turn (one buffer).
pub fn for_map_file_lines(data: &[u8], mut f: impl FnMut(&[u8])) {
    let mut line = Vec::new();
    // `while (!fm_eof())`: a line is read while the end has not been
    // hit; the read that hits it ends a last (possibly empty) line
    let mut rest = data;
    loop {
        let end = rest.iter().position(|&c| c == 10 || c == 13);
        let chunk = &rest[..end.unwrap_or(rest.len())];
        line.clear();
        for &c in chunk {
            let c = if c == 9 { b' ' } else { c };
            if c != b' ' || line.last().is_some_and(|&l| l != b' ') {
                line.push(c);
            }
        }
        f(&line);
        match end {
            Some(e) => rest = &rest[e + 1..],
            None => break,
        }
    }
}

/// `read_field`: bytes up to a blank, `<`, `"` or the end, then one
/// blank skipped.
fn read_field<'a>(r: &mut &'a [u8]) -> &'a [u8] {
    let n = r
        .iter()
        .position(|&c| c == b' ' || c == b'<' || c == b'"')
        .unwrap_or(r.len());
    let (f, rest) = r.split_at(n);
    *r = rest;
    skip(r, b' ');
    f
}

/// ptexmac.h's `skip`: one `c`, if there.
fn skip(r: &mut &[u8], c: u8) {
    if r.first() == Some(&c) {
        *r = &r[1..];
    }
}

/// `sscanf("%f %n")`: a float, then any white space; the value and the
/// bytes taken.
fn scan_float(s: &[u8]) -> Option<(f32, usize)> {
    let mut i = s.iter().take_while(|c| c.is_ascii_whitespace()).count();
    let start = i;
    if matches!(s.get(i), Some(b'+' | b'-')) {
        i += 1;
    }
    let int = s[i..].iter().take_while(|c| c.is_ascii_digit()).count();
    i += int;
    let mut frac = 0;
    if s.get(i) == Some(&b'.') {
        frac = s[i + 1..].iter().take_while(|c| c.is_ascii_digit()).count();
        i += 1 + frac;
    }
    if int + frac == 0 {
        return None;
    }
    let mut end = i;
    if matches!(s.get(i), Some(b'e' | b'E')) {
        let mut j = i + 1;
        if matches!(s.get(j), Some(b'+' | b'-')) {
            j += 1;
        }
        let digits = s[j..].iter().take_while(|c| c.is_ascii_digit()).count();
        if digits > 0 {
            end = j + digits;
        }
    }
    let text = std::str::from_utf8(&s[start..end]).ok()?;
    let v: f32 = text.trim_end_matches('.').parse().ok().or_else(|| {
        // "5." parses in C; Rust wants a digit after the point
        text.parse().ok()
    })?;
    let blanks = s[end..]
        .iter()
        .take_while(|c| c.is_ascii_whitespace())
        .count();
    Some((v, end + blanks))
}

/// `sscanf(r, "PidEid=%i, %i %n")`.
fn scan_pid_eid(r: &[u8]) -> Option<(i32, i32, usize)> {
    let s = r.strip_prefix(b"PidEid=")?;
    let (a, n) = strtol_c(s)?;
    let mut i = 7 + n;
    if r.get(i) != Some(&b',') {
        return None;
    }
    i += 1;
    i += r[i..]
        .iter()
        .take_while(|c| c.is_ascii_whitespace())
        .count();
    let (b, n) = strtol_c(&r[i..])?;
    i += n;
    i += r[i..]
        .iter()
        .take_while(|c| c.is_ascii_whitespace())
        .count();
    #[expect(clippy::cast_possible_truncation, reason = "C's int conversion")]
    Some((a as i32, b as i32, i))
}

/// mapfile.c's `check_std_t1font`: one of the 14 standard fonts.
fn is_std_t1font(s: &[u8]) -> bool {
    [
        "Courier",
        "Courier-Bold",
        "Courier-Oblique",
        "Courier-BoldOblique",
        "Helvetica",
        "Helvetica-Bold",
        "Helvetica-Oblique",
        "Helvetica-BoldOblique",
        "Symbol",
        "Times-Roman",
        "Times-Bold",
        "Times-Italic",
        "Times-BoldItalic",
        "ZapfDingbats",
    ]
    .iter()
    .any(|n| n.as_bytes() == s)
}

fn ends_with_ci(s: &[u8], suffix: &[u8]) -> bool {
    s.len() >= suffix.len() && s[s.len() - suffix.len()..].eq_ignore_ascii_case(suffix)
}

/// C's `%g` of `v` (six significant digits).
pub fn percent_g(v: f64) -> Vec<u8> {
    if v == 0.0 {
        return b"0".to_vec();
    }
    let e = format!("{v:.5e}");
    let (mantissa, exp) = e.split_once('e').unwrap_or((&e, "0"));
    let x: i32 = exp.parse().unwrap_or(0);
    let strip = |s: &str| -> String {
        if s.contains('.') {
            s.trim_end_matches('0').trim_end_matches('.').into()
        } else {
            s.into()
        }
    };
    if (-4..6).contains(&x) {
        let decimals = usize::try_from(5 - x).unwrap_or(0);
        strip(&format!("{v:.decimals$}")).into_bytes()
    } else {
        let sign = if x < 0 { '-' } else { '+' };
        format!("{}e{sign}{:02}", strip(mantissa), x.abs()).into_bytes()
    }
}

/// `(integer)(d > 0 ? d + 0.5 : d - 0.5)` of `d * 1000`, with `d` a float.
#[expect(clippy::cast_possible_truncation, reason = "C's conversion")]
fn thousandths(d: f32) -> i32 {
    let d = f64::from((f64::from(d) * 1000.0) as f32);
    (if d > 0.0 { d + 0.5 } else { d - 0.5 }) as i32
}

/// mapfile.c's `fm_scan_line` up to (not including) registering: the
/// entry of a map line, if it is one and valid, and the warnings.
pub fn scan_line(line: &[u8], warn: &mut Vec<Vec<u8>>) -> Option<MapEntry> {
    let mut r = line;
    if r.first().is_none_or(|&c| is_cfg_comment(c)) {
        return None;
    }
    let mut fm = MapEntry::new();
    let mut u = 0u8;
    let mut v = 0u8;
    'fields: {
        fm.tfm_name = read_field(&mut r).to_vec();
        if r.is_empty() {
            break 'fields;
        }
        if !r.first().is_some_and(u8::is_ascii_digit) {
            let f = read_field(&mut r);
            if !f.is_empty() {
                fm.ps_name = Some(f.to_vec());
            }
            if r.is_empty() {
                break 'fields;
            }
        }
        if r.first().is_some_and(u8::is_ascii_digit) {
            let n = r.iter().take_while(|c| c.is_ascii_digit()).count();
            if matches!(r.get(n), None | Some(b' ' | b'"' | b'<')) {
                // `atoi`
                let digits = &r[..n];
                let value = digits.iter().fold(0i32, |a, &c| {
                    a.wrapping_mul(10).wrapping_add(i32::from(c - b'0'))
                });
                fm.fd_flags = value;
                r = &r[n..];
            }
        }
        loop {
            skip(&mut r, b' ');
            match r.first() {
                None => break 'fields,
                Some(b'"') => {
                    r = &r[1..];
                    u = 0;
                    v = 0;
                    loop {
                        skip(&mut r, b' ');
                        if let Some((d, j)) = scan_float(r) {
                            let mut s = j;
                            if matches!(r[s - 1], b'E' | b'e') {
                                s -= 1;
                            }
                            let rest = &r[s..];
                            if rest.starts_with(b"SlantFont") {
                                fm.slant = thousandths(d);
                                r = &rest[9..];
                            } else if rest.starts_with(b"ExtendFont") {
                                fm.extend = thousandths(d);
                                if fm.extend == 1000 {
                                    fm.extend = 0;
                                }
                                r = &rest[10..];
                            } else {
                                let n = rest
                                    .iter()
                                    .position(|&c| c == b' ' || c == b'"')
                                    .unwrap_or(rest.len());
                                warn.push(
                                    [
                                        b"invalid entry for `".as_slice(),
                                        &fm.tfm_name,
                                        b"': unknown name `",
                                        &rest[..n],
                                        b"' ignored",
                                    ]
                                    .concat(),
                                );
                                r = &rest[n..];
                            }
                        } else {
                            let n = r
                                .iter()
                                .position(|&c| c == b' ' || c == b'"')
                                .unwrap_or(r.len());
                            r = &r[n..];
                        }
                        if r.first() != Some(&b' ') {
                            break;
                        }
                    }
                    if r.first() == Some(&b'"') {
                        r = &r[1..];
                    } else {
                        warn.push(
                            [
                                b"invalid entry for `".as_slice(),
                                &fm.tfm_name,
                                b"': closing quote missing",
                            ]
                            .concat(),
                        );
                        return None;
                    }
                }
                Some(&c) => {
                    if c == b'P'
                        && let Some((a, b, n)) = scan_pid_eid(r)
                    {
                        fm.pid = a;
                        fm.eid = b;
                        r = &r[n..];
                        continue;
                    }
                    let (mut a, mut b) = (0u8, 0u8);
                    if c == b'<' {
                        a = b'<';
                        r = &r[1..];
                        if let Some(&d @ (b'<' | b'[')) = r.first() {
                            b = d;
                            r = &r[1..];
                        }
                    }
                    let buf = read_field(&mut r);
                    if buf.len() > 4 && ends_with_ci(buf, b".enc") {
                        fm.encname = Some(buf.to_vec());
                        u = 0;
                        v = 0;
                    } else if !buf.is_empty() {
                        if a == b'<' || u == b'<' {
                            fm.kind |= F_INCLUDED;
                            if (a == b'<' && b == 0) || (a == 0 && v == 0) {
                                fm.kind |= F_SUBSETTED;
                            }
                        }
                        fm.ff_name = Some(buf.to_vec());
                        if r.is_empty() {
                            break 'fields;
                        }
                        u = 0;
                        v = 0;
                    } else {
                        u = a;
                        v = b;
                    }
                }
            }
        }
    }
    // done:
    if fm.ps_name.as_deref().is_some_and(is_std_t1font) {
        fm.kind |= F_STDT1FONT;
    }
    match &fm.ff_name {
        Some(f) if f.len() > 3 => {
            if ends_with_ci(f, b".ttf") || ends_with_ci(f, b".ttc") {
                fm.kind |= F_TRUETYPE;
            } else if ends_with_ci(f, b".otf") {
                fm.kind |= F_OTF;
            } else {
                fm.kind |= F_TYPE1;
            }
        }
        _ if fm.ps_name.is_none() => fm.kind |= F_PK,
        _ => fm.kind |= F_TYPE1,
    }
    if check_entry(&mut fm, warn) != 0 {
        return None;
    }
    Some(fm)
}

/// mapfile.c's `check_fm_entry` (with warnings): zero if the entry is
/// fine.
fn check_entry(fm: &mut MapEntry, warn: &mut Vec<Vec<u8>>) -> i32 {
    let mut a = 0;
    let t = fm.tfm_name.clone();
    let invalid = |w: &mut Vec<Vec<u8>>, m: &[u8]| {
        w.push([b"invalid entry for `".as_slice(), &t, b"': ", m].concat());
    };
    if fm.ff_name.is_some() && !fm.is(F_INCLUDED) {
        warn.push(
            [
                b"ambiguous entry for `".as_slice(),
                &t,
                b"': font file present but not included, will be treated as font file not present",
            ]
            .concat(),
        );
        fm.ff_name = None;
    }
    if t.is_empty() {
        warn.push(b"invalid map entry: tfm missing".to_vec());
        a += 1;
    }
    if fm.is(F_TRUETYPE) && fm.is_reencoded() && !fm.is(F_SUBSETTED) {
        invalid(warn, b"only subsetted TrueType fonts can be reencoded");
        a += 2;
    }
    if (fm.slant != 0 || fm.extend != 0)
        && (t.is_empty() || !(fm.is_t1fontfile() && fm.is(F_INCLUDED)))
    {
        invalid(
            warn,
            b"SlantFont/ExtendFont can be used only with embedded Type1 fonts",
        );
        a += 4;
    }
    if fm.slant.unsigned_abs() > 1000 {
        let v = percent_g(f64::from(fm.slant) / 1000.0);
        invalid(
            warn,
            &[b"SlantFont value too big: ".as_slice(), &v].concat(),
        );
        a += 8;
    }
    if fm.extend.unsigned_abs() > 2000 {
        let v = percent_g(f64::from(fm.extend) / 1000.0);
        invalid(
            warn,
            &[b"ExtendFont value too big: ".as_slice(), &v].concat(),
        );
        a += 16;
    }
    if fm.pid != -1 && !(fm.is(F_TRUETYPE) && fm.is(F_SUBSETTED) && !fm.is_reencoded()) {
        invalid(
            warn,
            b"PidEid can be used only with subsetted non-reencoded TrueType fonts",
        );
        a += 32;
    }
    if fm.is(F_PK) {
        if let Some(f) = &fm.ff_name {
            let m = [
                b"FontFile cannot be specified for bitmap PK font: ".as_slice(),
                f,
            ]
            .concat();
            invalid(warn, &m);
            a += 64;
        }
        if let Some(p) = &fm.ps_name {
            let m = [
                b"PsName cannot be specified for bitmap PK font: ".as_slice(),
                p,
            ]
            .concat();
            invalid(warn, &m);
            a += 128;
        }
    }
    a
}

/// A `\pdfmapfile` or `\pdfmapline` argument split as `process_map_item`
/// does: the mode, whether it flushes the default map file, and the item
/// (empty if there is none).
pub fn map_item(s: &[u8], file: bool) -> (Mode, bool, &[u8]) {
    let mut s = s;
    skip(&mut s, b' ');
    let (mode, flush) = match s.first() {
        Some(b'+') => (Mode::DupIgnore, false),
        Some(b'=') => (Mode::Replace, false),
        Some(b'-') => (Mode::Delete, false),
        _ => (Mode::DupIgnore, true),
    };
    if !flush {
        s = &s[1..];
    }
    skip(&mut s, b' ');
    if file {
        let n = s.iter().position(|&c| c == b' ').unwrap_or(s.len());
        s = &s[..n];
    }
    // a C string ends at a null byte
    let n = s.iter().position(|&c| c == 0).unwrap_or(s.len());
    (mode, flush, &s[..n])
}


/// C's `strtol(s, &end, 0)`: the number at the start of `s` (after white
/// space and a sign; `0x` for hexadecimal, `0` for octal), and how many
/// bytes it took; `None` if there is none.
fn strtol_c(s: &[u8]) -> Option<(i64, usize)> {
    let mut i = s.iter().take_while(|c| c.is_ascii_whitespace() || **c == 0x0b).count();
    let negative = match s.get(i) {
        Some(b'-') => {
            i += 1;
            true
        }
        Some(b'+') => {
            i += 1;
            false
        }
        _ => false,
    };
    let (radix, from) = match (s.get(i), s.get(i + 1), s.get(i + 2)) {
        (Some(b'0'), Some(b'x' | b'X'), Some(c)) if c.is_ascii_hexdigit() => (16, i + 2),
        (Some(b'0'), ..) => (8, i),
        _ => (10, i),
    };
    let digits = s[from..].iter().take_while(|c| char::from(**c).is_digit(radix)).count();
    if digits == 0 {
        return None;
    }
    let v = s[from..from + digits].iter().fold(0i64, |v, &c| {
        let d = i64::from(char::from(c).to_digit(radix).unwrap_or(0));
        v.saturating_mul(i64::from(radix)).saturating_add(d)
    });
    Some((if negative { -v } else { v }, from + digits))
}

/// A map file's entries by TFM name: the lines naming each, in order,
/// parsed when looked up (the first valid one is the entry, as reading
/// the file with duplicates ignored makes it).
pub struct Map {
    data: Arc<[u8]>,
    lines: HashMap<Vec<u8>, Vec<(usize, usize)>>,
}

impl Map {
    /// The index of map file `data` (`for_map_file_lines`' lines, by their
    /// first field).
    pub fn new(data: Arc<[u8]>) -> Map {
        let mut lines: HashMap<Vec<u8>, Vec<(usize, usize)>> = HashMap::new();
        let mut at = 0;
        let d = &data[..];
        while at <= d.len() {
            let end = d[at..].iter().position(|&c| c == 10 || c == 13).map_or(d.len(), |e| at + e);
            let line = &d[at..end];
            // (the first field after leading blanks, up to a blank, `<` or `"`)
            let s = line.iter().position(|&c| c != b' ' && c != b'\t').unwrap_or(line.len());
            let rest = &line[s..];
            if rest.first().is_some_and(|&c| !is_cfg_comment(c)) {
                let n = rest.iter().position(|&c| c == b' ' || c == b'\t' || c == b'<' || c == b'"').unwrap_or(rest.len());
                lines.entry(rest[..n].to_vec()).or_default().push((at, end));
            }
            if end == d.len() {
                break;
            }
            at = end + 1;
        }
        Map { data, lines }
    }

    /// The entry the `ps_tree` has for `key` (an included PDF's font by
    /// its PostScript name, slant and extend): the first line of the file
    /// with that name that is an included Type 1 font, if it is also the
    /// entry of its TFM name.
    pub fn lookup_ps(&self, key: &(Vec<u8>, i32, i32)) -> Option<Arc<MapEntry>> {
        let data = &self.data[..];
        let needle = &key.0[..];
        let mut at = 0;
        while !needle.is_empty()
            && let Some(i) = data[at..].windows(needle.len()).position(|w| w == needle)
        {
            let pos = at + i;
            let start = data[..pos].iter().rposition(|&c| c == 10 || c == 13).map_or(0, |p| p + 1);
            let end = data[pos..].iter().position(|&c| c == 10 || c == 13).map_or(data.len(), |p| pos + p);
            let mut entry = None;
            let mut warn = Vec::new();
            for_map_file_lines(&data[start..end], |line| {
                if entry.is_none() {
                    entry = scan_line(line, &mut warn);
                }
            });
            if let Some(fm) = entry
                && fm.ps_key().as_ref() == Some(key)
                && fm.is_t1fontfile()
                && fm.is(F_INCLUDED)
            {
                let first = if fm.tfm_name == b"<nontfm>" { Some(Arc::new(fm.clone())) } else { self.lookup(&fm.tfm_name) };
                if let Some(f) = first
                    && *f == fm
                {
                    return Some(f);
                }
            }
            at = end.max(pos + 1);
        }
        None
    }

    /// The entry for TFM font `tfm`, if the map has one.
    pub fn lookup(&self, tfm: &[u8]) -> Option<Arc<MapEntry>> {
        for &(a, b) in self.lines.get(tfm)? {
            let mut warn = Vec::new();
            let mut found = None;
            for_map_file_lines(&self.data[a..b], |l| {
                if found.is_none() {
                    found = scan_line(l, &mut warn);
                }
            });
            if let Some(fm) = found
                && fm.tfm_name == tfm
            {
                return Some(Arc::new(fm));
            }
        }
        None
    }
}
