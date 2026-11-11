//! The viewer's `cites` op (citation cards): the keys of the citation
//! command a glyph was typeset from ([`cite_at`]), each key's label as
//! the build's `.aux` gives it ([`bibcites`]), and where its entry is
//! in the bibliography's `.bbl` ([`bibitem_at`]); the session puts them
//! together (`Session::cites_at`).

use std::collections::HashMap;

/// The keys of the citation command whose text holds byte `at` of `text`
/// (the command's name, its options, or its keys): any command whose
/// name has `cite` in it and takes keys (`\cite`, `\citep`, `\citet`,
/// `\autocite`, `\parencite`, `\textcite`, `\Cite`…; `\cites` and the
/// other multicites, every key group), not `\nocite`. None: `at` is in no
/// such command.
#[must_use]
pub fn cite_at(text: &[u8], at: usize) -> Option<Vec<String>> {
    // (from the nearest backslash before `at` back, a command at a time:
    // the first that is a citation spanning `at` answers)
    let from = at.saturating_sub(600);
    let mut k = at.min(text.len().saturating_sub(1));
    loop {
        if text.get(k) == Some(&b'\\')
            && let Some((end, keys)) = command(text, k)
            && at < end
        {
            return Some(keys);
        }
        if k <= from {
            return None;
        }
        k -= 1;
    }
}

/// The citation command at `text[k]` (a backslash): its end (past its
/// last key group) and its keys; none: not one.
fn command(text: &[u8], k: usize) -> Option<(usize, Vec<String>)> {
    let mut i = k + 1;
    while text.get(i).is_some_and(u8::is_ascii_alphabetic) {
        i += 1;
    }
    let name = std::str::from_utf8(&text[k + 1..i])
        .ok()?
        .to_ascii_lowercase();
    if !name.contains("cite") || name == "nocite" {
        return None;
    }
    let multi = name.ends_with("cites");
    let skip = |i: &mut usize| {
        while text.get(*i).is_some_and(u8::is_ascii_whitespace) {
            *i += 1;
        }
    };
    if text.get(i) == Some(&b'*') {
        i += 1;
    }
    let mut keys = Vec::new();
    loop {
        skip(&mut i);
        match text.get(i) {
            // (options and pre/post notes, `(...)` biblatex's multicite's)
            Some(&o @ (b'[' | b'(')) => {
                let close = if o == b'[' { b']' } else { b')' };
                i = group(text, i, o, close)?;
            }
            Some(b'{') => {
                let end = group(text, i, b'{', b'}')?;
                let body = std::str::from_utf8(&text[i + 1..end - 1]).ok()?;
                keys.extend(
                    body.split(',')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned),
                );
                i = end;
                if !multi {
                    break;
                }
            }
            _ => break,
        }
    }
    (!keys.is_empty()).then_some((i, keys))
}

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

/// Where `key`'s entry begins in a `.bbl`: the byte of its `\bibitem`
/// (`\bibitem{KEY}`, `\bibitem[LABEL]{KEY}`) or biblatex's
/// `\entry{KEY}`; none: not there.
#[must_use]
pub fn bibitem_at(bbl: &[u8], key: &str) -> Option<usize> {
    let text = std::str::from_utf8(bbl).ok()?;
    let mut at = 0;
    while let Some(k) = text[at..].find(['\\']) {
        let s = at + k;
        at = s + 1;
        let rest = &text[s..];
        let Some(after) = rest
            .strip_prefix("\\bibitem")
            .or_else(|| rest.strip_prefix("\\entry"))
        else {
            continue;
        };
        let mut i = s + (rest.len() - after.len());
        let b = text.as_bytes();
        while b.get(i).is_some_and(u8::is_ascii_whitespace) {
            i += 1;
        }
        if b.get(i) == Some(&b'[') {
            let Some(e) = group(b, i, b'[', b']') else {
                continue;
            };
            i = e;
        }
        if b.get(i) != Some(&b'{') {
            continue;
        }
        let Some(e) = group(b, i, b'{', b'}') else {
            continue;
        };
        if text[i + 1..e - 1].trim() == key {
            return Some(s);
        }
    }
    None
}

/// Where the `.bbl` entry beginning at `start` ends: the next entry's
/// `\bibitem` or `\entry`, else the file's end.
#[must_use]
pub fn entry_end(bbl: &[u8], start: usize) -> usize {
    let from = start.saturating_add(1).min(bbl.len());
    bbl[from..]
        .windows(6)
        .position(|w| w == b"\\bibit" || w == b"\\entry")
        .map_or(bbl.len(), |k| from + k)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cite_around_a_byte() {
        let t = br"As shown \citep[p.~3]{knuth81, lamport94} and \cite{a}, \nocite{z}.";
        let at = |s: &str| std::str::from_utf8(t).unwrap().find(s).unwrap();
        assert_eq!(
            cite_at(t, at("citep")),
            Some(vec!["knuth81".into(), "lamport94".into()])
        );
        assert_eq!(
            cite_at(t, at("lamport")),
            Some(vec!["knuth81".into(), "lamport94".into()])
        );
        assert_eq!(cite_at(t, at("{a}")), Some(vec!["a".into()]));
        assert_eq!(cite_at(t, at("As")), None);
        assert_eq!(cite_at(t, at("and")), None);
        assert_eq!(cite_at(t, at("{z}")), None);
        let m = br"\textcites[see][]{a,b}[][4]{c}";
        assert_eq!(
            cite_at(m, 3),
            Some(vec!["a".into(), "b".into(), "c".into()])
        );
    }

    #[test]
    fn labels_from_the_aux() {
        let aux = "\\relax\n\\bibcite{knuth81}{1}\n\\bibcite{kp}{{2}{1981}{{Knuth and Plass}}{{Knuth, D. and Plass, M.}}}\n\\bibcite{x}{{3}{}{{Anon}}{{}}}\n";
        let l = bibcites(aux);
        assert_eq!(l["knuth81"], "1");
        assert_eq!(l["kp"], "Knuth and Plass (1981)");
        assert_eq!(l["x"], "Anon");
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
            for at in 0..=t.len() + 2 {
                let _ = cite_at(t, at);
            }
            let text = String::from_utf8_lossy(t);
            let _ = bibcites(&text);
            let _ = bibitem_at(t, "k");
            let _ = bibitem_at(t, "");
            for at in [0, 1, t.len(), t.len() + 5] {
                let _ = entry_end(t, at);
            }
        }
    }

    #[test]
    fn entries_in_the_bbl() {
        let bbl =
            b"\\begin{thebibliography}{9}\n\\bibitem[Knuth(1981)]{kp} Knuth.\n\\bibitem{a} A.\n";
        assert_eq!(bibitem_at(bbl, "a"), Some(60));
        assert_eq!(bibitem_at(bbl, "kp"), Some(27));
        assert_eq!(bibitem_at(bbl, "none"), None);
        assert_eq!(entry_end(bbl, 27), 60);
        assert_eq!(entry_end(bbl, 60), bbl.len());
        assert_eq!(entry_end(bbl, 10_000), bbl.len());
    }
}
