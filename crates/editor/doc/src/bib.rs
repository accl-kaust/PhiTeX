//! The keys of a BibTeX database: enough to tell whether a citation's key
//! is in it (BibTeX's own reading of entries, without their fields).

/// A database's keys, with their lines.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Bib {
    pub keys: Vec<(String, u32)>,
}

impl Bib {
    #[must_use]
    pub fn scan(src: &str) -> Bib {
        Bib {
            keys: entries(src)
                .into_iter()
                .map(|(k, l)| (k.to_owned(), l))
                .collect(),
        }
    }
}

/// The keys of a database's text.
pub(crate) fn keys_in(src: &str) -> Vec<&str> {
    entries(src).into_iter().map(|(k, _)| k).collect()
}

/// The entries' keys, with their lines: `@type{key,` or `@type(key,`;
/// `@comment`, `@string` and `@preamble` have none. An entry is read past
/// with its braces balanced, so an `@` inside a field starts nothing.
fn entries(src: &str) -> Vec<(&str, u32)> {
    let bytes = src.as_bytes();
    let mut keys: Vec<(usize, usize)> = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] != b'@' {
            at += 1;
            continue;
        }
        let t0 = at + 1;
        let mut j = t0;
        while j < bytes.len()
            && (bytes[j].is_ascii_alphanumeric() || matches!(bytes[j], b'_' | b'-'))
        {
            j += 1;
        }
        let ty = src[t0..j].to_ascii_lowercase();
        while j < bytes.len() && bytes[j].is_ascii_whitespace() {
            j += 1;
        }
        let Some(&open) = bytes.get(j) else { break };
        if open != b'{' && open != b'(' {
            at = j.max(at + 1);
            continue;
        }
        let close = if open == b'{' { b'}' } else { b')' };
        let body = j + 1;
        if !matches!(ty.as_str(), "comment" | "string" | "preamble") {
            let mut k = body;
            while k < bytes.len() && bytes[k].is_ascii_whitespace() {
                k += 1;
            }
            let k0 = k;
            while k < bytes.len()
                && !matches!(bytes[k], b',' | b'}' | b')')
                && !bytes[k].is_ascii_whitespace()
            {
                k += 1;
            }
            if k > k0 {
                keys.push((k0, k));
            }
        }
        // (past the entry)
        let mut depth = 0usize;
        let mut k = body;
        while k < bytes.len() {
            match bytes[k] {
                b'{' => depth += 1,
                b'}' if depth > 0 => depth -= 1,
                c if c == close && depth == 0 => break,
                _ => {}
            }
            k += 1;
        }
        at = k + 1;
    }
    // (the lines, in one pass)
    let mut out = Vec::with_capacity(keys.len());
    let mut line = 1u32;
    let mut at = 0;
    for (k0, k) in keys {
        line += crate::text::newlines(&src[at..k0]);
        at = k0;
        out.push((&src[k0..k], line));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys() {
        let src = "@string{x = {y}}\n@Book{knuth,\n  title = {T@X {a}},\n}\n\n% @misc{not}\n@article( a:b ,\n note = \"x\")\n@comment{c, d}\n";
        let b = Bib::scan(src);
        assert_eq!(
            b.keys,
            [
                ("knuth".to_owned(), 2),
                ("not".to_owned(), 6),
                ("a:b".to_owned(), 7)
            ]
        );
    }
}
