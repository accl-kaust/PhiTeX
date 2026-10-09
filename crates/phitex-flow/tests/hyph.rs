//! The oracle for hyphenation: `Patterns` and `Exceptions` built from TeX
//! Live's `hyphen.tex` (a copy, unchanged, in `tests/hyph/`), and the
//! words of `tests/hyph/words.txt` hyphenated by `hyph::hyphenate`, must
//! break where `tex -ini` breaks them: TeX's `\showhyphens` (plain.tex's,
//! with `\tenrm` = cmr10) shows each word in an Underfull box's line,
//! `[] \tenrm hy-phen-ation`. TeX's lines are recorded in
//! `tests/hyph/words.log`; where a `tex` binary is found they are checked
//! against a fresh run; `PHITEX_FLOW_BLESS=1` records them anew.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use phitex_flow::hyph::{self, Block, Exceptions, Patterns, Tok};

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/hyph")
}

/// `IniTeX`'s `\lccode`s (§232): a letter's is its lowercase, others 0.
fn lc(c: char) -> char {
    if c.is_ascii_alphabetic() { c.to_ascii_lowercase() } else { '\0' }
}

/// A line of `words.txt`: a word, or a setting.
enum Line {
    Word(String),
    Set(String, i32),
    Hyphenation(String),
}

fn lines() -> Vec<Line> {
    let text = std::fs::read_to_string(dir().join("words.txt")).unwrap();
    let mut out = Vec::new();
    for l in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('%')) {
        if let Some(arg) = l.strip_prefix("\\hyphenation{") {
            out.push(Line::Hyphenation(arg.trim_end_matches('}').to_string()));
        } else if let Some(s) = l.strip_prefix('\\') {
            let (name, v) = s.split_once('=').unwrap();
            out.push(Line::Set(name.to_string(), v.parse().unwrap()));
        } else {
            out.push(Line::Word(l.to_string()));
        }
    }
    out
}

/// The document TeX runs: plain.tex's `\showhyphens` for every word.
fn document(lines: &[Line]) -> String {
    let mut s = String::from(
        "\\catcode`\\{=1 \\catcode`\\}=2 \\catcode`\\#=6\n\
         \\defaulthyphenchar=`\\-\n\
         \\input hyphen\n\
         \\font\\tenrm=cmr10\n\
         \\def\\showhyphens#1{\\setbox0\\vbox{\\parfillskip0pt \\hsize16383.99999pt \\tenrm\n\
         \x20 \\pretolerance-1 \\tolerance-1 \\hbadness0 \\showboxdepth0 \\ #1}}\n",
    );
    for l in lines {
        match l {
            Line::Word(w) => writeln!(s, "\\showhyphens{{{w}}}").unwrap(),
            Line::Set(n, v) => writeln!(s, "\\{n}={v}").unwrap(),
            Line::Hyphenation(h) => writeln!(s, "\\hyphenation{{{h}}}").unwrap(),
        }
    }
    s.push_str("\\end\n");
    s
}

/// TeX's `\showhyphens` lines (after `[] \tenrm `), if there is a `tex`.
fn tex(lines: &[Line]) -> Option<Vec<String>> {
    let tmp = std::env::temp_dir().join(format!("phitex-flow-hyph-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).ok()?;
    std::fs::copy(dir().join("hyphen.tex"), tmp.join("hyphen.tex")).ok()?;
    std::fs::write(tmp.join("words.tex"), document(lines)).ok()?;
    let st = Command::new("tex").args(["-ini", "-interaction=batchmode", "words.tex"]).current_dir(&tmp).output().ok();
    let log = std::fs::read_to_string(tmp.join("words.log")).ok();
    let _ = std::fs::remove_dir_all(&tmp);
    let (st, log) = (st?, log?);
    assert!(st.status.success(), "tex failed:\n{log}");
    assert!(!log.contains("\n! "), "tex reported an error:\n{log}");
    Some(log.lines().filter_map(|l| l.strip_prefix("[] \\tenrm ")).map(str::to_string).collect())
}

/// A word with a `-` after each of its break positions.
fn shown(word: &str, at: &[usize]) -> String {
    let mut s = String::new();
    for (i, c) in word.chars().enumerate() {
        if i > 0 && at.contains(&i) {
            s.push('-');
        }
        s.push(c);
    }
    s
}

#[test]
fn hyphen_tex_as_tex_hyphenates() {
    let blocks = hyph::read_blocks(&std::fs::read_to_string(dir().join("hyphen.tex")).unwrap());
    assert_eq!(blocks.iter().map(|b| b.0).collect::<Vec<_>>(), [Block::Patterns, Block::Hyphenation]);
    let (patterns, e1) = Patterns::new().add(&blocks[0].1, lc);
    let (mut exceptions, e2) = Exceptions::new().add(&blocks[1].1, lc);
    assert!(e1.is_empty() && e2.is_empty(), "{e1:?} {e2:?}");
    assert_eq!((patterns.len(), exceptions.len()), (4447, 14));

    let lines = lines();
    let t = tex(&lines);
    let path = dir().join("words.log");
    if std::env::var_os("PHITEX_FLOW_BLESS").is_some() {
        let t = t.as_ref().expect("PHITEX_FLOW_BLESS needs a tex binary");
        std::fs::write(&path, t.join("\n") + "\n").unwrap();
    }
    let want: Vec<String> = std::fs::read_to_string(&path).unwrap().lines().map(str::to_string).collect();
    if let Some(t) = &t {
        assert_eq!(t, &want, "TeX's output is not the recorded one");
    }

    let (mut lhmin, mut rhmin, mut uchyph) = (0, 0, 0);
    let mut got = Vec::new();
    for l in &lines {
        match l {
            Line::Set(n, v) => match n.as_str() {
                "lefthyphenmin" => lhmin = *v,
                "righthyphenmin" => rhmin = *v,
                "uchyph" => uchyph = *v,
                _ => panic!("unknown setting {n}"),
            },
            Line::Hyphenation(h) => {
                let toks: Vec<Tok> =
                    h.chars().map(|c| if c.is_whitespace() { Tok::Space } else { Tok::Char(c) }).collect();
                let (e, errors) = exceptions.add(&toks, lc);
                assert_eq!(errors, []);
                exceptions = e;
            }
            Line::Word(w) => {
                let word: Vec<char> = w.chars().collect();
                let at = if hyph::uc_hyph_allows(word[0], lc, uchyph) {
                    hyph::hyphenate(&word, lc, lhmin, rhmin, &patterns, &exceptions)
                } else {
                    Vec::new()
                };
                got.push(shown(w, &at));
            }
        }
    }
    assert_eq!(got.len(), want.len());
    let bad: Vec<String> =
        got.iter().zip(&want).filter(|(g, w)| g != w).map(|(g, w)| format!("graph {g}  TeX {w}")).collect();
    assert!(bad.is_empty(), "{} of {} words differ:\n{}", bad.len(), want.len(), bad.join("\n"));
}

/// `\patterns` and `\hyphenation` as TeX reads them: digits, edges,
/// nonletters, duplicates; a later exception replacing an earlier one.
#[test]
fn reading_patterns_and_exceptions() {
    let toks = |s: &str| -> Vec<Tok> {
        s.chars().map(|c| if c == ' ' { Tok::Space } else { Tok::Char(c) }).collect::<Vec<_>>()
    };
    let (p, e) = Patterns::new().add(&toks("1ba .ab4 a1b2c3. 2b1a"), lc);
    assert_eq!(e, [hyph::HyphError::DuplicatePattern]);
    assert_eq!(p.get(&['b', 'a']), Some(&[2, 1, 0][..]));
    assert_eq!(p.get(&['\0', 'a', 'b']), Some(&[0, 0, 0, 4][..]));
    // the edge's digit is dropped (§965)
    assert_eq!(p.get(&['a', 'b', 'c', '\0']), Some(&[0, 1, 2, 3, 0][..]));
    let (_, e) = Patterns::new().add(&toks("a!b"), lc);
    assert_eq!(e, [hyph::HyphError::Nonletter]);

    let (x, _) = Exceptions::new().add(&toks("ta-ble a b-c"), lc);
    assert_eq!(x.get(&['t', 'a', 'b', 'l', 'e']), Some(&[2][..]));
    assert_eq!(x.len(), 2);
    let (x, _) = x.add(&toks("tab-le"), lc);
    assert_eq!(x.get(&['t', 'a', 'b', 'l', 'e']), Some(&[3][..]));
    assert_eq!(hyph::hyphenate(&['t', 'a', 'b', 'l', 'e'], lc, 1, 1, &Patterns::new(), &x), [3]);
}
