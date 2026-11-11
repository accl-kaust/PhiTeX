//! The viewer's `cites` op (citation cards): each key's label and number
//! as the build's `.aux` gives them ([`bibcites`], [`bibnumbers`]). The
//! keys and their entries come from the PDF, hyperref's citation links
//! (`crate::view::Viewer::cite_links`), exact; none are guessed from the
//! glyphs' origins (their words matched to the source, a citation's to the
//! bibliography's as often as to its call). The engine's own record of
//! which command typeset a glyph is the next step (1.0.1).

use std::collections::HashMap;

/// The end (past its closing `close`) of the group opened at `text[i]`
/// (`open`), nested groups counted; none: not closed within 2000 bytes.
fn group(text: &[u8], i: usize, open: u8, close: u8) -> Option<usize> {
    let mut depth = 0usize;
    for (j, &c) in text.iter().enumerate().skip(i).take(2000) {
        if c == open {
            depth += 1;
        } else if c == close {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                return Some(j + 1);
            }
        }
    }
    None
}

/// Each key's label from an `.aux`'s `\bibcite{KEY}{LABEL}`: the label as
/// printed. natbib's `{{N}{YEAR}{{SHORT}}{{LONG}}}` as `SHORT (YEAR)`
/// (`SHORT YEAR` where there is no year); a plain one as it is (`1`).
#[must_use]
pub fn bibcites(aux: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let b = aux.as_bytes();
    let mut at = 0;
    while let Some(k) = aux[at..].find("\\bibcite{") {
        let s = at + k + "\\bibcite".len();
        let Some(e1) = group(b, s, b'{', b'}') else {
            break;
        };
        let key = aux[s + 1..e1 - 1].to_owned();
        let mut i = e1;
        while b.get(i).is_some_and(u8::is_ascii_whitespace) {
            i += 1;
        }
        let Some(e2) = (b.get(i) == Some(&b'{'))
            .then(|| group(b, i, b'{', b'}'))
            .flatten()
        else {
            at = e1;
            continue;
        };
        out.insert(key, label(&aux[i + 1..e2 - 1]));
        at = e2;
    }
    out
}

/// Each key's number from an `.aux`'s `\bibcite`: natbib's first group
/// (`{{N}{YEAR}...}`), or a plain label that is a number; what a numbered
/// style prints (`[1]`) where [`bibcites`]' label is natbib's author-year.
#[must_use]
pub fn bibnumbers(aux: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let b = aux.as_bytes();
    let mut at = 0;
    while let Some(k) = aux[at..].find("\\bibcite{") {
        let s = at + k + "\\bibcite".len();
        let Some(e1) = group(b, s, b'{', b'}') else {
            break;
        };
        let key = aux[s + 1..e1 - 1].to_owned();
        let mut i = e1;
        while b.get(i).is_some_and(u8::is_ascii_whitespace) {
            i += 1;
        }
        let Some(e2) = (b.get(i) == Some(&b'{'))
            .then(|| group(b, i, b'{', b'}'))
            .flatten()
        else {
            at = e1;
            continue;
        };
        let v = aux[i + 1..e2 - 1].trim();
        let n = match v.strip_prefix('{') {
            Some(r) => r.split('}').next().unwrap_or("").trim(),
            None => v,
        };
        if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) {
            out.insert(key, n.to_owned());
        }
        at = e2;
    }
    out
}

/// A `\bibcite` label's text as printed (its groups taken apart).
fn label(v: &str) -> String {
    let b = v.as_bytes();
    if !v.starts_with('{') {
        return v.trim().to_owned();
    }
    // (natbib's groups: number, year, short authors, long authors)
    let mut groups = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'{' {
            let Some(e) = group(b, i, b'{', b'}') else {
                break;
            };
            groups.push(
                v[i + 1..e - 1]
                    .trim_matches(|c| c == '{' || c == '}')
                    .trim(),
            );
            i = e;
        } else {
            i += 1;
        }
    }
    match groups.as_slice() {
        [n] => (*n).to_owned(),
        [n, year, short, ..] if !short.is_empty() => {
            if year.is_empty() {
                (*short).to_owned()
            } else {
                let _ = n;
                format!("{short} ({year})")
            }
        }
        [n, ..] => (*n).to_owned(),
        [] => v.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_from_the_aux() {
        let aux = "\\relax\n\\bibcite{knuth81}{1}\n\\bibcite{kp}{{2}{1981}{{Knuth and Plass}}{{Knuth, D. and Plass, M.}}}\n\\bibcite{x}{{3}{}{{Anon}}{{}}}\n";
        let l = bibcites(aux);
        assert_eq!(l["knuth81"], "1");
        assert_eq!(l["kp"], "Knuth and Plass (1981)");
        assert_eq!(l["x"], "Anon");
        let n = bibnumbers(aux);
        assert_eq!(n["knuth81"], "1");
        assert_eq!(n["kp"], "2");
        assert_eq!(n["x"], "3");
    }

    /// No input breaks them: random bytes (invalid UTF-8 too), every cut
    /// of real citations, unbalanced and deep braces, at every byte. The
    /// browser's core aborts at a panic, so a citation half typed must
    /// give no cards, never an error.
    #[test]
    fn nothing_breaks_them() {
        let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let real = "x \\citep[p.~3][see]{a, b} \\cites{c}[4]{d} \\cite*{ \\bibcite{k}{{1}{2}{{A}}{{}}} \\bibitem[L]{k} \\entry{e}{book}{}";
        let mut inputs: Vec<Vec<u8>> = (0..=real.len())
            .map(|n| real.as_bytes()[..n].to_vec())
            .collect();
        for _ in 0..400 {
            let n = usize::try_from(next() % 120).unwrap();
            let alphabet = b"\\{}[]()*, citeboxbibitementry\xff\xc3";
            inputs.push(
                (0..n)
                    .map(|_| alphabet[usize::try_from(next()).unwrap() % alphabet.len()])
                    .collect(),
            );
        }
        inputs.push(vec![b'{'; 5000]);
        inputs.push([b"\\cite".as_slice(), &[b'{'; 3000]].concat());
        inputs.push(Vec::new());
        for t in &inputs {
            let text = String::from_utf8_lossy(t);
            let _ = bibcites(&text);
            let _ = bibnumbers(&text);
        }
    }
}
