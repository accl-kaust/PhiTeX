//! epstopdf.pl's rewrite of an EPS before gs reads it (TeX Live's
//! epstopdf 2.36, without `--filter`, `--hires`, `--exact`): the DOS EPS
//! binary header and PJL dropped, junk before `%!` dropped, the first
//! `%%BoundingBox:` of the header replaced by a page of the box's size
//! and a translation, `grestore` at the end.

use alloc::string::String;
use alloc::vec::Vec;

/// The PostScript section of a DOS EPS binary file (TN 5002), else the
/// file.
pub fn dos_eps_section(d: &[u8]) -> Vec<u8> {
    if d.len() > 30 && d.starts_with(&[0xC5, 0xD0, 0xD3, 0xC6]) {
        let le = |i: usize| u32::from_le_bytes([d[i], d[i + 1], d[i + 2], d[i + 3]]) as usize;
        let (off, len) = (le(4), le(8));
        let s = off.min(d.len());
        let e = off.saturating_add(len).min(d.len());
        return d[s..e].to_vec();
    }
    d.to_vec()
}

/// The lines of `d`, each with its end (`$/`).
fn lines(d: &[u8], eol: u8) -> Vec<&[u8]> {
    let mut v = Vec::new();
    let mut s = 0;
    for (i, &c) in d.iter().enumerate() {
        if c == eol {
            v.push(&d[s..=i]);
            s = i + 1;
        }
    }
    if s < d.len() {
        v.push(&d[s..]);
    }
    v
}

/// Perl's numeric value of a string (the longest decimal prefix).
fn perl_value(s: &[u8]) -> f64 {
    let mut i = 0;
    let n = s.len();
    while i < n && matches!(s[i], b' ' | b'\t' | b'\n' | b'\r' | b'\x0c' | b'\x0b') {
        i += 1;
    }
    let start = i;
    if i < n && (s[i] == b'+' || s[i] == b'-') {
        i += 1;
    }
    let mut digits = 0;
    while i < n && s[i].is_ascii_digit() {
        i += 1;
        digits += 1;
    }
    if i < n && s[i] == b'.' {
        i += 1;
        while i < n && s[i].is_ascii_digit() {
            i += 1;
            digits += 1;
        }
    }
    if digits == 0 {
        return 0.0;
    }
    let mut end = i;
    if i < n && (s[i] == b'e' || s[i] == b'E') {
        let mut j = i + 1;
        if j < n && (s[j] == b'+' || s[j] == b'-') {
            j += 1;
        }
        let k = j;
        while j < n && s[j].is_ascii_digit() {
            j += 1;
        }
        if j > k {
            end = j;
        }
    }
    let t = core::str::from_utf8(&s[start..end]).unwrap_or("0");
    t.parse::<f64>().unwrap_or(0.0)
}

/// Perl's string of a number (`%.15g`; integers as integers).
pub fn perl_num(v: f64) -> String {
    if v == 0.0 {
        return String::from("0");
    }
    crate::num::g_prec(v, 15)
}

/// Whether `s` (a whole line) is `%%BoundingBox:` with four values, as
/// epstopdf's `^%%BoundingBox:\s*(V+)\s+(V+)\s+(V+)\s+(V+)`, V in
/// `[0-9eE.-]`.
fn bbox_values(s: &[u8]) -> Option<[f64; 4]> {
    let r = s.strip_prefix(b"%%BoundingBox:")?;
    let ws = |c: u8| matches!(c, b' ' | b'\t' | b'\n' | b'\r' | b'\x0c' | b'\x0b');
    let val = |c: u8| c.is_ascii_digit() || matches!(c, b'e' | b'E' | b'.' | b'-');
    let mut i = 0;
    let mut out = [0.0; 4];
    for (k, o) in out.iter_mut().enumerate() {
        let w0 = i;
        while i < r.len() && ws(r[i]) {
            i += 1;
        }
        if k > 0 && i == w0 {
            return None;
        }
        let v0 = i;
        while i < r.len() && val(r[i]) {
            i += 1;
        }
        if i == v0 {
            return None;
        }
        *o = perl_value(&r[v0..i]);
    }
    Some(out)
}

/// epstopdf.pl's output for EPS `d`.
pub fn epstopdf_feed(d: &[u8]) -> Vec<u8> {
    let mut d = dos_eps_section(d);
    // (PJL: UEL then @PJL lines)
    let uel = b"\x1b%-12345X";
    if d.starts_with(uel) {
        let mut i = uel.len();
        let mut any = false;
        while d[i..].starts_with(b"@PJL") {
            match d[i..].iter().position(|&c| c == b'\n') {
                Some(p) => {
                    i += p + 1;
                    any = true;
                }
                None => break,
            }
        }
        if any {
            d.drain(..i);
        }
    } else {
        // (binary junk before the first %! of its line, within the
        // first 2048 bytes)
        let head = &d[..d.len().min(2048)];
        if let Some(p) = head.windows(2).position(|w| w == b"%!") {
            let ls = head[..p]
                .iter()
                .rposition(|&c| c == b'\n')
                .map_or(0, |q| q + 1);
            d.drain(ls..p);
        }
    }
    // ($/: CR when the first CR is not followed by LF and comes before
    // any LF)
    let head = &d[..d.len().min(2048)];
    let cr = head.iter().position(|&c| c == b'\r');
    let lf = head.iter().position(|&c| c == b'\n');
    let eol = match (cr, lf) {
        (Some(c), None) if c > 0 => b'\r',
        (Some(c), Some(l)) if c > 0 && l > c + 1 => b'\r',
        _ => b'\n',
    };
    let ls = lines(&d, eol);
    let mut out: Vec<u8> = Vec::new();
    let mut it = 0;
    // (the first line, junk before its last %! dropped)
    let mut header = false;
    if let Some(first) = ls.first() {
        let mut f: &[u8] = first;
        if let Some(p) = f.windows(2).rposition(|w| w == b"%!") {
            // ('.' does not match a newline: only within the line)
            if !f[..p].contains(&b'\n') {
                f = &f[p..];
            }
        }
        header = f.starts_with(b"%");
        out.extend_from_slice(f);
        it = 1;
    }
    let mut corrected = false;
    let correct = |out: &mut Vec<u8>, b: [f64; 4]| {
        let (w, h) = (b[2] - b[0], b[3] - b[1]);
        let (xo, yo) = (-b[0], -b[1]);
        let e = eol as char;
        let s = alloc::format!(
            "%%BoundingBox: 0 0 {w} {h}{e}<< /PageSize [{w} {h}] >> setpagedevice{e}gsave {xo} {yo} translate{e}",
            w = perl_num(w),
            h = perl_num(h),
            xo = perl_num(xo),
            yo = perl_num(yo),
        );
        out.extend_from_slice(s.as_bytes());
    };
    if header {
        while it < ls.len() {
            let l = ls[it];
            it += 1;
            let nonblank = l
                .iter()
                .any(|&c| !matches!(c, b' ' | b'\t' | b'\n' | b'\r' | b'\x0c' | b'\x0b'));
            if !nonblank {
                continue;
            }
            if !l.starts_with(b"%") || l.starts_with(b"%%EndComments") {
                out.extend_from_slice(l);
                break;
            }
            if let Some(b) = bbox_values(l) {
                correct(&mut out, b);
                corrected = true;
                break;
            }
            let atend = l.strip_prefix(b"%%BoundingBox:").is_some_and(|r| {
                let r = r
                    .iter()
                    .position(|&c| !matches!(c, b' ' | b'\t' | b'\n' | b'\r' | b'\x0c' | b'\x0b'))
                    .map_or(&r[r.len()..], |p| &r[p..]);
                r.starts_with(b"(atend)")
            });
            if atend {
                if let Some(b) = ls[it..].iter().find_map(|l| bbox_values(l)) {
                    correct(&mut out, b);
                    corrected = true;
                }
                break;
            }
            out.extend_from_slice(l);
        }
    }
    for l in &ls[it.min(ls.len())..] {
        out.extend_from_slice(l);
    }
    if corrected {
        out.push(eol);
        out.extend_from_slice(b"grestore");
        out.push(eol);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn revtex_fig() {
        let eps = b"%!PS-Adobe-3.0\n%%BoundingBox: 0 0 100 100\n0 0 moveto\n%%EOF\n";
        let f = epstopdf_feed(eps);
        assert_eq!(
            f,
            b"%!PS-Adobe-3.0\n%%BoundingBox: 0 0 100 100\n<< /PageSize [100 100] >> setpagedevice\ngsave 0 0 translate\n0 0 moveto\n%%EOF\n\ngrestore\n".to_vec()
        );
    }

    #[test]
    fn perl_numbers() {
        assert_eq!(perl_num(perl_value(b"10.5") - perl_value(b"0.25")), "10.25");
        assert_eq!(perl_num(-perl_value(b"-3")), "3");
        assert_eq!(perl_num(-perl_value(b"0")), "0");
        assert_eq!(perl_num(perl_value(b"1e2")), "100");
    }
}
