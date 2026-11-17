//! What `phitex session png` and `phitex session where` find and draw
//! (`session_cmd.rs`): the part of a page a label's float (or any
//! element a label is in) or a source line was typeset to.
//!
//! - **The source.** A label's place is its `\label{…}` in the files the
//!   build read; what it labels is the innermost environment around it
//!   (a `figure`, a `subfigure`, a `table`, an `equation`, …; none but
//!   the document: the paragraph it ends, a heading's). A line is its
//!   bytes.
//! - **The page.** The engine says which pages it placed the source's
//!   lines on; the element's words ([`tokens`]) are found among a page's
//!   glyphs in their order and close together ([`find`]), as the
//!   viewer's forward search finds a line's words: a float's text far
//!   from where its source is (a `figure*` on the next page) is found as
//!   well as any. The engine's glyph origins (docs/note-origins.md) say
//!   which glyphs around them are another element's typed text
//!   ([`trusted`]).
//! - **The part.** The page's drawings ([`phitex_render::Mark`]), in the
//!   order the page draws them, hold the rest: a float is one box, drawn
//!   whole, so what is drawn between its first glyph and its last is its
//!   own, and the pictures and rules drawn just before or after its text
//!   (an image above its caption, a table's last rule) are taken where
//!   the source says the element has such things there and the
//!   neighbour drawn beside it does not ([`region`]).

use phitex_render::{Mark, MarkKind};

/// A page's glyph origin: the file (an index into the answer's files),
/// the source bytes, the glyph's origin (points from the page's top left).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Code {
    pub file: u32,
    pub start: u32,
    pub end: u32,
    /// A macro's glyph (its range the call's, as the matcher found it).
    pub synth: bool,
    pub x: f64,
    pub y: f64,
}

/// Which of a page's `codes` (in the page's order) the source places
/// for certain: typed text, each code one of at least three in a row
/// matched to bytes of one file one after another (at most a few bytes
/// apart: a space, `~`, ` & `), the row's bytes whole words (`whole`:
/// whether bytes `start..end` of a file are, no letter or digit just
/// before or after them). The origins match the page's text to the
/// sources' (docs/note-origins.md §2.2): a macro's text (a caption's
/// "Figure 3:", a counter) is given the bytes of letters alike wherever
/// the alignment found them, scattered, or a part of a word ("igure" of
/// a "figure" in the text).
#[must_use]
pub fn trusted(codes: &[Code], whole: &dyn Fn(u32, usize, usize) -> bool) -> Vec<bool> {
    let ok = |c: &Code| !c.synth && c.end > c.start && c.file != u32::MAX;
    let next = |a: &Code, b: &Code| {
        ok(a) && ok(b) && a.file == b.file && b.start >= a.end && b.start - a.end <= 8
    };
    let mut out = vec![false; codes.len()];
    let mut i = 0;
    while i < codes.len() {
        let mut j = i;
        while j + 1 < codes.len() && next(&codes[j], &codes[j + 1]) {
            j += 1;
        }
        if j + 1 - i >= 3
            && whole(
                codes[i].file,
                codes[i].start as usize,
                codes[j].end as usize,
            )
        {
            out[i..=j].fill(true);
        }
        i = j + 1;
    }
    out
}

/// Whether bytes `a..b` of `t` are whole words: no letter or digit just
/// before or after them.
#[must_use]
pub fn whole_words(t: &[u8], a: usize, b: usize) -> bool {
    let word = |i: usize| t.get(i).is_some_and(u8::is_ascii_alphanumeric);
    b <= t.len() && (a == 0 || !word(a - 1)) && !word(b)
}

/// Whether `origin`, a file as the glyph origins name it, is `file` (a
/// name from the project's root): with or without `./` and `.tex`, or a
/// rooted path ([`rooted`]) whose last components are it. Either may use
/// `/` or `\\` between its components (a Windows host's names).
#[must_use]
pub fn same_file(origin: &str, file: &str) -> bool {
    let (a, b) = (slashed(origin), slashed(file));
    // (the names without `./` and `.tex`, either side)
    let bare = |s: &str| -> String {
        let s = s.trim_start_matches("./");
        s.strip_suffix(".tex").unwrap_or(s).to_owned()
    };
    let (a, b) = (bare(&a), bare(&b));
    a == b || (rooted(&a) && a.ends_with(&format!("/{b}")))
}

/// `p` with every `\\` a `/`: a name compared whatever its host's separator.
fn slashed(p: &str) -> String {
    p.replace('\\', "/")
}

/// Whether `p` names a file from a root, not from the project's: `/` or
/// `\\` first (a POSIX path, a Windows UNC or rooted one), or a drive
/// letter (`C:`), on every host alike ([`std::path::Path::is_absolute`] says false
/// for `/home/...` on Windows and for `C:\\...` elsewhere).
#[must_use]
pub fn rooted(p: &str) -> bool {
    let b = p.as_bytes();
    matches!(b.first(), Some(b'/' | b'\\'))
        || (b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':')
}

/// `t` with its comments blanked (`%` to the line's end, unless escaped):
/// the same offsets.
#[must_use]
pub fn uncommented(t: &[u8]) -> Vec<u8> {
    let mut out = t.to_vec();
    let mut i = 0;
    while i < t.len() {
        match t[i] {
            b'\\' => i += 2,
            b'%' => {
                while i < t.len() && t[i] != b'\n' {
                    out[i] = b' ';
                    i += 1;
                }
            }
            _ => i += 1,
        }
    }
    out
}

/// The group `{…}` at `i` (spaces skipped first): its contents' range and
/// the byte after it.
fn group(t: &[u8], mut i: usize, open: u8, close: u8) -> Option<(usize, usize, usize)> {
    while i < t.len() && t[i].is_ascii_whitespace() {
        i += 1;
    }
    if t.get(i) != Some(&open) {
        return None;
    }
    let mut depth = 0usize;
    let mut j = i;
    while j < t.len() {
        match t[j] {
            b'\\' => j += 1,
            c if c == open => depth += 1,
            c if c == close => {
                depth -= 1;
                if depth == 0 {
                    return Some((i + 1, j, j + 1));
                }
            }
            _ => {}
        }
        j += 1;
    }
    None
}

/// The command at `i` (a backslash): its name and the byte after it.
fn command(t: &[u8], i: usize) -> (&[u8], usize) {
    let mut j = i + 1;
    while j < t.len() && t[j].is_ascii_alphabetic() {
        j += 1;
    }
    if j == i + 1 && j < t.len() {
        // (a control symbol: `\\`, `\%`)
        j += 1;
    }
    let mut e = j;
    if t.get(e) == Some(&b'*') {
        e += 1;
    }
    (&t[i + 1..j], e)
}

/// An environment of a source: its name, from its `\begin` to the end of
/// its `\end{…}`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Env {
    pub name: String,
    pub start: usize,
    pub end: usize,
}

/// The environments of `t` (uncommented), each `\begin` with its `\end`.
#[must_use]
pub fn envs(t: &[u8]) -> Vec<Env> {
    let mut out = Vec::new();
    let mut open: Vec<(String, usize)> = Vec::new();
    let mut i = 0;
    while i < t.len() {
        if t[i] != b'\\' {
            i += 1;
            continue;
        }
        let (name, after) = command(t, i);
        let begin = name == b"begin";
        if (begin || name == b"end")
            && let Some((a, b, next)) = group(t, after, b'{', b'}')
        {
            let env = String::from_utf8_lossy(&t[a..b]).trim().to_owned();
            if begin {
                open.push((env, i));
            } else if let Some(k) = open.iter().rposition(|(n, _)| *n == env) {
                let (name, start) = open.swap_remove(k);
                open.truncate(k);
                out.push(Env {
                    name,
                    start,
                    end: next,
                });
            }
            i = next;
            continue;
        }
        i = after.max(i + 1);
    }
    out
}

/// Where `\label{label}` is in `t` (uncommented): its start and end.
#[must_use]
pub fn label_place(t: &[u8], label: &str) -> Option<(usize, usize)> {
    let mut i = 0;
    while i < t.len() {
        if t[i] == b'\\' {
            let (name, mut after) = command(t, i);
            if name == b"label" {
                // (cleveref's `\label[type]{…}`)
                if let Some((_, _, n)) = group(t, after, b'[', b']') {
                    after = n;
                }
                if let Some((a, b, n)) = group(t, after, b'{', b'}')
                    && t[a..b].trim_ascii() == label.as_bytes()
                {
                    return Some((i, n));
                }
            }
            i = after.max(i + 1);
        } else {
            i += 1;
        }
    }
    None
}

/// What a label at `at..end` of `t` (uncommented) labels: the innermost
/// environment around it but the document, its name; else the paragraph
/// it ends (a heading's label: the heading), from the blank line before
/// it (or the document's start) to the label's end, with no name.
#[must_use]
pub fn labelled(t: &[u8], at: usize, end: usize) -> (usize, usize, Option<String>) {
    let all = envs(t);
    if let Some(e) = all
        .iter()
        .filter(|e| e.start <= at && end <= e.end && e.name != "document")
        .min_by_key(|e| e.end - e.start)
    {
        return (e.start, e.end, Some(e.name.clone()));
    }
    let floor = all
        .iter()
        .filter(|e| e.name == "document" && e.start <= at)
        .map(|e| {
            // (after `\begin{document}`)
            let (_, after) = command(t, e.start);
            group(t, after, b'{', b'}').map_or(e.start, |g| g.2)
        })
        .max()
        .unwrap_or(0);
    // (a blank line: a newline, then only spaces up to the next)
    let mut start = floor;
    let mut i = at;
    while i > floor {
        i -= 1;
        if t[i] == b'\n' {
            let mut j = i;
            while j > floor && (t[j - 1] == b' ' || t[j - 1] == b'\t') {
                j -= 1;
            }
            if j > floor && t[j - 1] == b'\n' {
                start = i + 1;
                break;
            }
        }
    }
    // (the environments that end before the label within it: a figure
    // above a heading is not the heading's)
    if let Some(e) = all
        .iter()
        .filter(|e| e.end <= at && e.end > start && e.start >= floor)
        .map(|e| e.end)
        .max()
    {
        start = e;
    }
    while start < at && t[start].is_ascii_whitespace() {
        start += 1;
    }
    (start, end, None)
}

/// The files `\input`, `\include` or `\subfile` reads in `t[a..b]`
/// (uncommented), as written.
#[must_use]
pub fn inputs(t: &[u8], a: usize, b: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = a;
    while i < b {
        if t[i] == b'\\' {
            let (name, after) = command(t, i);
            if matches!(name, b"input" | b"include" | b"subfile" | b"subfileinclude")
                && let Some((x, y, n)) = group(t, after, b'{', b'}')
            {
                out.push(String::from_utf8_lossy(&t[x..y]).trim().to_owned());
                i = n;
                continue;
            }
            i = after.max(i + 1);
        } else {
            i += 1;
        }
    }
    out
}

/// Commands that draw nothing of their own (their text, if any, is
/// glyphs, which the origins place): spacing, alignment, fonts, labels,
/// references, the sectioning commands and captions.
const NO_INK: &[&str] = &[
    "label",
    "centering",
    "raggedright",
    "raggedleft",
    "vspace",
    "hspace",
    "vskip",
    "hskip",
    "vfill",
    "hfill",
    "hfil",
    "vfil",
    "par",
    "medskip",
    "smallskip",
    "bigskip",
    "noindent",
    "indent",
    "small",
    "footnotesize",
    "scriptsize",
    "tiny",
    "normalsize",
    "large",
    "Large",
    "LARGE",
    "huge",
    "Huge",
    "captionsetup",
    "phantomsection",
    "relax",
    "newline",
    "linebreak",
    "nolinebreak",
    "pagebreak",
    "nopagebreak",
    "quad",
    "qquad",
    "protect",
    "part",
    "chapter",
    "section",
    "subsection",
    "subsubsection",
    "paragraph",
    "subparagraph",
    "caption",
    "subcaption",
    "subcaptionbox",
    "captionof",
    "textbf",
    "textit",
    "emph",
    "textrm",
    "textsf",
    "texttt",
    "textsc",
    "bfseries",
    "itshape",
    "rmfamily",
    "sffamily",
    "ttfamily",
    "scshape",
    "upshape",
    "mdseries",
    "normalfont",
    "ref",
    "eqref",
    "cref",
    "Cref",
    "autoref",
    "pageref",
    "nameref",
    "cite",
    "citep",
    "citet",
    "footnote",
    "item",
    "nonumber",
    "notag",
    "tag",
    "strut",
    "linewidth",
    "textwidth",
    "columnwidth",
    "hsize",
    "textheight",
    "baselineskip",
    "paperwidth",
    "\\",
    "addlinespace",
    "arraystretch",
    "renewcommand",
    "setlength",
    "selectfont",
    "fontsize",
    "ignorespaces",
    "unskip",
    "allowbreak",
    "displaystyle",
    "textstyle",
    "left",
    "right",
    "big",
    "Big",
    "bigg",
    "Bigg",
    "mathrm",
    "mathbf",
    "mathit",
    "mathcal",
    "text",
    "frac",
    "sum",
    "int",
    "prod",
    "sqrt",
    "begin",
    "end",
];

/// Environments that draw rules or pictures of their own.
const INK_ENVS: &[&str] = &[
    "tikzpicture",
    "picture",
    "pgfpicture",
    "axis",
    "semilogxaxis",
    "semilogyaxis",
    "loglogaxis",
    "polaraxis",
    "tikzcd",
    "circuitikz",
    "forest",
    "pspicture",
    "overpic",
];

/// The arguments a command of [`NO_INK`] takes that are no text: skipped.
fn skip_args(name: &[u8]) -> usize {
    match name {
        b"label" | b"vspace" | b"hspace" | b"captionsetup" | b"ref" | b"eqref" | b"cref"
        | b"Cref" | b"autoref" | b"pageref" | b"nameref" | b"vskip" | b"hskip" | b"end" => 1,
        b"setlength" | b"renewcommand" | b"fontsize" => 2,
        _ => 0,
    }
}

/// Whether source `t` (uncommented) may draw something other than
/// glyphs: a command not of [`NO_INK`] (`\includegraphics`, a table's
/// rules, a user's macro), an environment of [`INK_ENVS`], a table's `|`.
#[must_use]
pub fn has_ink(t: &[u8]) -> bool {
    let mut i = 0;
    while i < t.len() {
        match t[i] {
            b'\\' => {
                let (name, mut after) = command(t, i);
                if name == b"begin" {
                    if let Some((a, b, n)) = group(t, after, b'{', b'}') {
                        let env = String::from_utf8_lossy(&t[a..b]);
                        if INK_ENVS.contains(&env.trim()) {
                            return true;
                        }
                        after = n;
                        // (its options: a float's placement)
                        if let Some((_, _, n)) = group(t, after, b'[', b']') {
                            after = n;
                        }
                    }
                } else if NO_INK.contains(&String::from_utf8_lossy(name).as_ref()) {
                    for _ in 0..skip_args(name) {
                        if let Some((_, _, n)) = group(t, after, b'[', b']') {
                            after = n;
                        }
                        match group(t, after, b'{', b'}') {
                            Some((_, _, n)) => after = n,
                            None => break,
                        }
                    }
                } else {
                    return true;
                }
                i = after.max(i + 1);
            }
            b'|' => return true,
            _ => i += 1,
        }
    }
    false
}

/// A word of an element's source, as a page would show it: its text
/// (letters and digits, folded) and its bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    pub text: String,
    pub start: usize,
    pub end: usize,
}

/// Commands whose arguments the page does not show as they are written:
/// skipped with them (how many required ones; their optional ones too).
fn hidden_args(name: &[u8]) -> usize {
    match name {
        b"label" | b"ref" | b"eqref" | b"cref" | b"Cref" | b"autoref" | b"pageref" | b"nameref"
        | b"cite" | b"citep" | b"citet" | b"citealp" | b"parencite" | b"textcite" | b"autocite"
        | b"includegraphics" | b"input" | b"include" | b"subfile" | b"usepackage" | b"vspace"
        | b"hspace" | b"captionsetup" | b"color" | b"pagecolor" | b"end" | b"bibliography"
        | b"bibliographystyle" | b"hyperref" | b"href" | b"url" | b"includepdf" | b"subref"
        | b"label*" | b"vskip" | b"hskip" | b"cmidrule" | b"addlinespace" => 1,
        b"setlength" | b"addtolength" | b"textcolor" | b"colorbox" | b"rule" | b"resizebox"
        | b"setcounter" | b"addtocounter" => 2,
        b"fcolorbox" | b"renewcommand" | b"newcommand" | b"multicolumn" => 3,
        _ => 0,
    }
}

/// The arguments after `\begin{ENV}` that set it up (a table's columns,
/// a box's width), shown by no glyph.
fn env_args(env: &str) -> usize {
    match env.trim_end_matches('*') {
        "tabular" | "array" | "subfigure" | "subtable" | "minipage" | "multicols" | "longtable" => {
            1
        }
        "tabularx" | "tabulary" | "wrapfigure" | "wraptable" => 2,
        _ => 0,
    }
}

/// A character as words compare: lower case, a ligature its letters.
fn fold_into(c: char, out: &mut String) {
    match c {
        '\u{FB00}' => out.push_str("ff"),
        '\u{FB01}' => out.push_str("fi"),
        '\u{FB02}' => out.push_str("fl"),
        '\u{FB03}' => out.push_str("ffi"),
        '\u{FB04}' => out.push_str("ffl"),
        c => out.extend(c.to_lowercase()),
    }
}

/// The words source `t[a..b]` (uncommented) shows: its letters and
/// digits in runs, the commands and the arguments no glyph shows (a
/// label's, a picture's file, a table's columns) left out.
#[must_use]
pub fn tokens(t: &[u8], a: usize, b: usize) -> Vec<Token> {
    let b = b.min(t.len());
    let mut out: Vec<Token> = Vec::new();
    let mut cur: Option<Token> = None;
    let end = |cur: &mut Option<Token>, out: &mut Vec<Token>| {
        if let Some(tk) = cur.take() {
            out.push(tk);
        }
    };
    let mut i = a;
    while i < b {
        let c = t[i];
        if c == b'\\' {
            end(&mut cur, &mut out);
            let (name, mut after) = command(t, i);
            let mut skip = hidden_args(name);
            if name == b"begin"
                && let Some((x, y, n)) = group(t, after, b'{', b'}')
            {
                let env = String::from_utf8_lossy(&t[x..y]).trim().to_owned();
                after = n;
                if let Some((_, _, n)) = group(t, after, b'[', b']') {
                    after = n;
                }
                skip = env_args(&env);
            }
            for _ in 0..skip {
                while let Some((_, _, n)) = group(t, after, b'[', b']') {
                    after = n;
                }
                match group(t, after, b'{', b'}') {
                    Some((_, _, n)) => after = n,
                    None => break,
                }
            }
            i = after.max(i + 1).min(b.max(i + 1));
            continue;
        }
        // (a character of UTF-8: whole)
        let len = match c {
            0xF0..=0xF7 => 4,
            0xE0..=0xEF => 3,
            0xC0..=0xDF => 2,
            _ => 1,
        };
        let ch = std::str::from_utf8(&t[i..(i + len).min(b)])
            .ok()
            .and_then(|s| s.chars().next())
            .unwrap_or('\u{FFFD}');
        if ch.is_alphanumeric() {
            let tk = cur.get_or_insert_with(|| Token {
                text: String::new(),
                start: i,
                end: i,
            });
            fold_into(ch, &mut tk.text);
            tk.end = i + len;
        } else {
            end(&mut cur, &mut out);
        }
        i += len;
    }
    end(&mut cur, &mut out);
    out
}

/// A word of a page: its text (folded) and its glyphs (indices of the
/// page's marks).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Word {
    pub text: String,
    pub marks: Vec<usize>,
}

/// The words of a page's glyphs (`marks`, in the page's order): runs of
/// letters and digits, a word ending where the baseline moves, the
/// next glyph steps back or leaves a word space.
#[must_use]
pub fn page_words(marks: &[Mark]) -> Vec<Word> {
    let mut out: Vec<Word> = Vec::new();
    let mut cur: Option<Word> = None;
    let mut prev: Option<(f64, f64, f64, f64)> = None;
    for (i, m) in marks.iter().enumerate() {
        let MarkKind::Glyph { x, y, size, text } = &m.kind else {
            continue;
        };
        let size = size.max(1.0);
        if let Some((px, py, pr, ps)) = prev {
            let s = ps.max(size);
            if (y - py).abs() > 0.3 * s || *x < px - 0.1 * s || *x > pr + 0.18 * s {
                out.extend(cur.take());
            }
        }
        prev = Some((*x, *y, m.rect[2].max(*x), size));
        for ch in text.chars() {
            if ch.is_alphanumeric() {
                let w = cur.get_or_insert_with(|| Word {
                    text: String::new(),
                    marks: Vec::new(),
                });
                fold_into(ch, &mut w.text);
                if w.marks.last() != Some(&i) {
                    w.marks.push(i);
                }
            } else {
                out.extend(cur.take());
            }
        }
    }
    out.extend(cur);
    out
}

/// Where an element's words show on a page: the page's words matched to
/// them (word, token), the score of the match.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    pub pairs: Vec<(usize, usize)>,
    pub score: f64,
}

/// The places on a page that show most of the element's words `tokens`
/// in their order and close together: a local alignment of the page's
/// words `words` with them (Smith and Waterman's: a word matched scores
/// by its length, a word skipped on either side costs one), every
/// alignment of the best score. None: no word of it on the page.
#[must_use]
pub fn find(tokens: &[Token], words: &[Word]) -> Vec<Found> {
    // (an element of thousands of words: its first ones are enough)
    let tokens = &tokens[..tokens.len().min(1500)];
    let (n, m) = (tokens.len(), words.len());
    if n == 0 || m == 0 {
        return Vec::new();
    }
    let gain = |t: &Token| {
        1.0 + 0.5 * f64::from(u32::try_from(t.text.chars().count().min(8)).unwrap_or(8))
    };
    let w = m + 1;
    let mut h = vec![0f32; (n + 1) * w];
    let mut best = 0f32;
    for i in 1..=n {
        for j in 1..=m {
            let diag = if tokens[i - 1].text == words[j - 1].text {
                #[allow(clippy::cast_possible_truncation, reason = "a small score")]
                let g = gain(&tokens[i - 1]) as f32;
                h[(i - 1) * w + j - 1] + g
            } else {
                h[(i - 1) * w + j - 1] - 1.0
            };
            let v = diag
                .max(h[(i - 1) * w + j] - 1.0)
                .max(h[i * w + j - 1] - 1.0)
                .max(0.0);
            h[i * w + j] = v;
            best = best.max(v);
        }
    }
    if best <= 0.0 {
        return Vec::new();
    }
    let mut out: Vec<Found> = Vec::new();
    let mut ends: Vec<usize> = Vec::new();
    for i in 1..=n {
        for j in 1..=m {
            if (h[i * w + j] - best).abs() < 1e-3 && !ends.contains(&j) {
                ends.push(j);
                // (back to where the score began)
                let (mut a, mut b) = (i, j);
                let mut pairs = Vec::new();
                while a > 0 && b > 0 && h[a * w + b] > 0.0 {
                    let v = h[a * w + b];
                    let same = tokens[a - 1].text == words[b - 1].text;
                    #[allow(clippy::cast_possible_truncation, reason = "a small score")]
                    let g = gain(&tokens[a - 1]) as f32;
                    if same && (v - (h[(a - 1) * w + b - 1] + g)).abs() < 1e-3 {
                        pairs.push((b - 1, a - 1));
                        a -= 1;
                        b -= 1;
                    } else if (v - (h[(a - 1) * w + b] - 1.0)).abs() < 1e-3 {
                        a -= 1;
                    } else if (v - (h[a * w + b - 1] - 1.0)).abs() < 1e-3 {
                        b -= 1;
                    } else {
                        a -= 1;
                        b -= 1;
                    }
                }
                pairs.reverse();
                out.push(Found {
                    pairs,
                    score: f64::from(best),
                });
            }
        }
    }
    out
}

/// What a glyph of the page is to [`region`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    /// The element's.
    Mine,
    /// Another element's of the document for certain (typed text of a
    /// project file, [`trusted`]): its code.
    Sure(usize),
    /// A macro's (a running head, a caption's number), a picture's, or
    /// text whose source is not known for certain.
    Neutral,
}

/// What the source says of the drawings beside the glyphs [`region`]
/// found: whether the element has drawings before its first glyph
/// (`head`) or after its last, and whether the element a neighbour glyph
/// is of has them after it (`tail`) or before it.
pub trait Claims {
    /// The element: whether its source before the text found (`head`) or
    /// after it may draw.
    fn ours(&self, head: bool) -> bool;
    /// The element of code `code`, a neighbour: whether its source after
    /// it (`tail`) or before it may draw.
    fn theirs(&self, code: usize, tail: bool) -> bool;
}

/// A gap between drawings past which a float's drawings do not reach
/// (points): wider than a caption's skip (10 pt) and the floats' (12 pt),
/// narrower than the header's (25 pt).
const LOOSE: f64 = 24.0;

#[derive(Clone, Debug)]
enum Kind {
    /// Glyphs of the element.
    Mine,
    /// Another's glyph: its code where it is another element's for
    /// certain ([`Class::Sure`]).
    Other(Option<usize>),
    /// A path, an image, a form or a clipped picture.
    Ink,
}

#[derive(Clone, Debug)]
struct Unit {
    rect: [f64; 4],
    kind: Kind,
}

fn union(a: [f64; 4], b: [f64; 4]) -> [f64; 4] {
    [
        a[0].min(b[0]),
        a[1].min(b[1]),
        a[2].max(b[2]),
        a[3].max(b[3]),
    ]
}

/// The distance between two boxes: across or down, the larger.
fn gap(a: [f64; 4], b: [f64; 4]) -> f64 {
    let dy = (b[1] - a[3]).max(a[1] - b[3]).max(0.0);
    let dx = (b[0] - a[2]).max(a[0] - b[2]).max(0.0);
    dx.max(dy)
}

/// A glyph mark's code (an index of `codes`): the one at its origin (to
/// a quarter point).
pub fn codes_at(codes: &[Code]) -> impl Fn(f64, f64) -> Option<usize> + '_ {
    let mut at: std::collections::HashMap<(i64, i64), Vec<usize>> =
        std::collections::HashMap::new();
    #[allow(clippy::cast_possible_truncation, reason = "a page's points")]
    let key = |x: f64, y: f64| ((x * 4.0).round() as i64, (y * 4.0).round() as i64);
    for (i, c) in codes.iter().enumerate() {
        at.entry(key(c.x, c.y)).or_default().push(i);
    }
    move |x, y| {
        let (kx, ky) = key(x, y);
        for dx in [0, -1, 1] {
            for dy in [0, -1, 1] {
                if let Some(c) = at.get(&(kx + dx, ky + dy)).and_then(|v| v.first()) {
                    return Some(*c);
                }
            }
        }
        None
    }
}

/// The drawings `marks` of a page `w` by `h` points as units: each glyph
/// as `class` says, each path and image, and each clip or group the page
/// opens (a form, a clipped picture) whole, the element's where a glyph
/// of it is. A fill of most of the page (its background) is none.
fn units(marks: &[Mark], class: &[Class], (w, h): (f64, f64)) -> Vec<Unit> {
    let mut out: Vec<Unit> = Vec::new();
    let mut nest_of: Vec<u32> = Vec::new();
    for (i, m) in marks.iter().enumerate() {
        let kind = match m.kind {
            MarkKind::Glyph { .. } => match class.get(i) {
                Some(Class::Mine) => Kind::Mine,
                Some(Class::Sure(c)) => Kind::Other(Some(*c)),
                _ => Kind::Other(None),
            },
            MarkKind::Path | MarkKind::Image => {
                let r = m.rect;
                if (r[2] - r[0]) * (r[3] - r[1]) > 0.8 * w * h {
                    continue;
                }
                Kind::Ink
            }
        };
        if m.nest != 0
            && nest_of.last() == Some(&m.nest)
            && let Some(u) = out.last_mut()
        {
            u.rect = union(u.rect, m.rect);
            if matches!(kind, Kind::Mine) {
                u.kind = Kind::Mine;
            }
            continue;
        }
        // (a clip's or group's glyphs not the element's: its picture's)
        let kind = match kind {
            Kind::Other(_) if m.nest != 0 => Kind::Ink,
            k => k,
        };
        out.push(Unit { rect: m.rect, kind });
        nest_of.push(m.nest);
    }
    out
}

/// The units of `seg` (the drawings beside the element's glyphs, from
/// the nearest out, up to a neighbour's glyph `stop`) that are the
/// element's, taken into `reg`: none where its source has nothing to
/// draw on that side (`ours`); those within [`LOOSE`] of it where the
/// neighbour's has nothing (`theirs`); where both have, those before
/// the widest gap of the row from `reg` through them to `stop`.
fn take(seg: &[&Unit], ours: bool, theirs: bool, stop: Option<[f64; 4]>, reg: &mut [f64; 4]) {
    if !ours || seg.is_empty() {
        return;
    }
    if !theirs || stop.is_none() {
        for u in seg {
            if gap(*reg, u.rect) >= LOOSE {
                break;
            }
            *reg = union(*reg, u.rect);
        }
        return;
    }
    let mut chain: Vec<[f64; 4]> = vec![*reg];
    chain.extend(seg.iter().map(|u| u.rect));
    chain.extend(stop);
    let (mut best, mut widest) = (0, f64::NEG_INFINITY);
    for k in 0..chain.len() - 1 {
        let g = gap(chain[k], chain[k + 1]);
        if g > widest {
            (best, widest) = (k, g);
        }
    }
    for u in &seg[..best] {
        *reg = union(*reg, u.rect);
    }
}

/// Whether two boxes are on one line: their heights overlap by half
/// the smaller's.
fn one_line(a: [f64; 4], b: [f64; 4]) -> bool {
    let overlap = a[3].min(b[3]) - a[1].max(b[1]);
    overlap > 0.5 * (a[3] - a[1]).min(b[3] - b[1])
}

/// The part of a page the element is drawn on (points from the page's
/// top left: left, top, right, bottom), from the page's drawings
/// `marks` in order (each glyph's [`Class`] in `class`) on a page `w`
/// by `h`: the run of its glyphs (the longest, until another element's
/// glyph) and what is drawn among them, with the glyphs on its first
/// and last line beside it that are no other element's (a caption's
/// "Figure 3:", an equation's number); with `extend`, the drawings
/// beside it that are the element's ([`take`], `claims`). `None`: no
/// glyph of it on the page.
#[must_use]
pub fn region(
    marks: &[Mark],
    class: &[Class],
    size: (f64, f64),
    extend: Option<&dyn Claims>,
) -> Option<[f64; 4]> {
    let us = units(marks, class, size);
    let mut runs: Vec<(usize, usize, usize)> = Vec::new();
    let mut cur: Option<(usize, usize, usize)> = None;
    for (i, u) in us.iter().enumerate() {
        match &u.kind {
            Kind::Mine => {
                let r = cur.get_or_insert((i, i, 0));
                r.1 = i;
                r.2 += 1;
            }
            Kind::Other(Some(_)) => runs.extend(cur.take()),
            _ => {}
        }
    }
    runs.extend(cur);
    let &(mut a, mut b, _) = runs.iter().rev().max_by_key(|r| r.2)?;
    // (the glyphs on its first and last line that are no other element's)
    let glyph = |u: &Unit| matches!(u.kind, Kind::Other(None)) && u.rect[3] > u.rect[1];
    while a > 0 && glyph(&us[a - 1]) && one_line(us[a - 1].rect, us[a].rect) {
        a -= 1;
    }
    while b + 1 < us.len() && glyph(&us[b + 1]) && one_line(us[b + 1].rect, us[b].rect) {
        b += 1;
    }
    let mut reg = us[a..=b].iter().map(|u| u.rect).reduce(union)?;
    let Some(claims) = extend else {
        return Some(reg);
    };
    // (the neighbour's box, and the code its claims are read at: its
    // own, or, a macro's glyph, that of the element's text on its line
    // nearest, the way away from the run)
    let neighbour = |i: Option<usize>, back: bool| -> (Option<[f64; 4]>, Option<usize>) {
        let Some(i) = i else {
            return (None, None);
        };
        let rect = us[i].rect;
        let mut k = i;
        loop {
            match &us[k].kind {
                Kind::Other(Some(c)) => return (Some(rect), Some(*c)),
                Kind::Other(None) if one_line(us[k].rect, rect) => {}
                _ => return (Some(rect), None),
            }
            match (back, k) {
                (true, 0) => return (Some(rect), None),
                (true, _) => k -= 1,
                (false, _) if k + 1 >= us.len() => return (Some(rect), None),
                (false, _) => k += 1,
            }
        }
    };
    // (before: from the run's first back to a glyph)
    let mut i = a;
    let mut before: Vec<&Unit> = Vec::new();
    while i > 0 && matches!(us[i - 1].kind, Kind::Ink) {
        i -= 1;
        before.push(&us[i]);
    }
    let (stop, theirs) = neighbour(i.checked_sub(1), true);
    let theirs = theirs.is_some_and(|c| claims.theirs(c, true));
    take(&before, claims.ours(true), theirs, stop, &mut reg);
    // (after)
    let mut j = b + 1;
    let mut after: Vec<&Unit> = Vec::new();
    while j < us.len() && matches!(us[j].kind, Kind::Ink) {
        after.push(&us[j]);
        j += 1;
    }
    let (stop, theirs) = neighbour((j < us.len()).then_some(j), false);
    let theirs = theirs.is_some_and(|c| claims.theirs(c, false));
    take(&after, claims.ours(false), theirs, stop, &mut reg);
    Some(reg)
}

/// `rect` grown by `pad` points each way, kept on the page `w` by `h`.
#[must_use]
pub fn padded(rect: [f64; 4], pad: f64, (w, h): (f64, f64)) -> [f64; 4] {
    [
        (rect[0] - pad).max(0.0),
        (rect[1] - pad).max(0.0),
        (rect[2] + pad).min(w),
        (rect[3] + pad).min(h),
    ]
}

/// `w` by `h` pixels, RGBA, as a PNG file (8-bit RGB: the alpha dropped,
/// the drawing is on white).
#[must_use]
pub fn png(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
    let row = w as usize * 4;
    let mut raw = Vec::with_capacity((w as usize * 3 + 1) * h as usize);
    for y in 0..h as usize {
        raw.push(0);
        for px in rgba[y * row..(y + 1) * row].as_chunks::<4>().0 {
            raw.extend_from_slice(&px[..3]);
        }
    }
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let chunk = |out: &mut Vec<u8>, kind: &[u8], data: &[u8]| {
        out.extend_from_slice(&u32::try_from(data.len()).unwrap_or(u32::MAX).to_be_bytes());
        let mut c = kind.to_vec();
        c.extend_from_slice(data);
        out.extend_from_slice(&c);
        out.extend_from_slice(&phitex_codecs::zlib::crc32(&c).to_be_bytes());
    };
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    // (8 bits, RGB, deflate, no filter method, no interlace)
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(
        &mut out,
        b"IDAT",
        &phitex_codecs::deflate::compress(&raw, 6),
    );
    chunk(&mut out, b"IEND", &[]);
    out
}

/// Two drawings of the same part (`w` by `h`, RGBA) side by side, the
/// one before on the left, then the one after, then the after's faded
/// with the pixels that differ in red: the image, its size, and how
/// many pixels differ.
#[must_use]
pub fn diff_image(w: u32, h: u32, before: &[u8], after: &[u8]) -> (Vec<u8>, u32, u32, usize) {
    const GUTTER: usize = 8;
    let (w, h) = (w as usize, h as usize);
    let ow = 3 * w + 2 * GUTTER;
    let mut out = vec![0u8; ow * h * 4];
    for px in out.as_chunks_mut::<4>().0 {
        px.copy_from_slice(&[160, 160, 160, 255]);
    }
    let mut changed = 0;
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) * 4;
            let (b, a) = (&before[i..i + 4], &after[i..i + 4]);
            let put = |out: &mut [u8], panel: usize, px: [u8; 4]| {
                let o = (y * ow + panel * (w + GUTTER) + x) * 4;
                out[o..o + 4].copy_from_slice(&px);
            };
            put(&mut out, 0, [b[0], b[1], b[2], 255]);
            put(&mut out, 1, [a[0], a[1], a[2], 255]);
            let differs = (0..3).any(|k| b[k].abs_diff(a[k]) > 24);
            let px = if differs {
                changed += 1;
                [220, 30, 30, 255]
            } else {
                let fade = |c: u8| u8::try_from((u16::from(c) + 3 * 255) / 4).unwrap_or(255);
                [fade(a[0]), fade(a[1]), fade(a[2]), 255]
            };
            put(&mut out, 2, px);
        }
    }
    (
        out,
        u32::try_from(ow).unwrap_or(u32::MAX),
        u32::try_from(h).unwrap_or(u32::MAX),
        changed,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = "\\documentclass{article}\n\\begin{document}\n\\section{Intro}\\label{sec:intro}\nText. % \\label{fig:gone}\n\n\\begin{figure}[t]\n  \\centering\n  \\begin{subfigure}{.45\\linewidth}\\includegraphics{a}\\caption{Left}\\label{fig:a}\\end{subfigure}\n  \\caption{Both.}\\label{fig:both}\n\\end{figure}\n\\begin{table}\\caption{T}\\label{tab:t}\\begin{tabular}{cc}a&b\\\\\\bottomrule\\end{tabular}\\end{table}\n\\end{document}\n";

    fn at(label: &str) -> (usize, usize, Option<String>) {
        let t = uncommented(DOC.as_bytes());
        let (a, b) = label_place(&t, label).expect("the label");
        labelled(&t, a, b)
    }

    #[test]
    fn a_label_labels_its_innermost_environment() {
        let (a, b, name) = at("fig:a");
        assert_eq!(name.as_deref(), Some("subfigure"));
        assert!(
            DOC[a..b].starts_with("\\begin{subfigure}") && DOC[a..b].ends_with("\\end{subfigure}")
        );
        let (a, b, name) = at("fig:both");
        assert_eq!(name.as_deref(), Some("figure"));
        assert!(
            DOC[a..b].starts_with("\\begin{figure}[t]") && DOC[a..b].ends_with("\\end{figure}")
        );
        assert_eq!(at("tab:t").2.as_deref(), Some("table"));
        // (a heading's: its paragraph, from the document's start)
        let (a, b, name) = at("sec:intro");
        assert_eq!(name, None);
        assert_eq!(&DOC[a..b], "\\section{Intro}\\label{sec:intro}");
        // (a commented label is none)
        assert_eq!(label_place(&uncommented(DOC.as_bytes()), "fig:gone"), None);
    }

    #[test]
    fn ink_in_source() {
        assert!(!has_ink(b"\\begin{figure}[t]\n  \\centering\n"));
        assert!(has_ink(
            b"\\begin{figure}[t]\\centering\\includegraphics[width=3cm]{x}"
        ));
        assert!(!has_ink(b"}\\label{fig:x}\n\\end{figure}"));
        assert!(has_ink(b"\\\\\\bottomrule\\end{tabular}\\end{table}"));
        assert!(has_ink(b"\\begin{tikzpicture}"));
        assert!(!has_ink(b"\\begin{subfigure}{.45\\linewidth}\\centering"));
        assert!(has_ink(b"\\begin{tabular}{|c|c|}"));
        assert_eq!(
            inputs(b"\\input{fig/body}\\include{x}", 0, 26),
            ["fig/body", "x"]
        );
    }

    #[test]
    fn files_named_alike() {
        assert!(same_file("./main.tex", "main.tex"));
        assert!(same_file("chap/one", "chap/one.tex"));
        assert!(same_file("/home/u/doc/chap/one.tex", "chap/one.tex"));
        assert!(!same_file("chap/one.tex", "one.tex"));
        // (Windows' forms, on every host)
        assert!(same_file("C:\\doc\\chap\\one.tex", "chap/one.tex"));
        assert!(same_file("C:/doc/chap/one.tex", "chap\\one.tex"));
        assert!(same_file("\\\\server\\share\\chap\\one", "chap/one.tex"));
        assert!(same_file(".\\chap\\one.tex", "chap/one.tex"));
        assert!(!same_file("C:\\doc\\xchap\\one.tex", "chap/one.tex"));
        assert!(!same_file("/home/u/doc/xchap/one.tex", "chap/one.tex"));
        assert!(!same_file("chap\\one.tex", "one.tex"));
    }

    #[test]
    fn rooted_names_on_every_host() {
        for p in [
            "/home/u/a.tex",
            "\\\\server\\share\\a.tex",
            "\\a.tex",
            "C:\\a.tex",
            "c:/a.tex",
        ] {
            assert!(rooted(p), "{p}");
        }
        for p in ["a.tex", "chap/a.tex", "chap\\a.tex", "./a.tex", ":a"] {
            assert!(!rooted(p), "{p}");
        }
    }

    /// Claims as fixed answers.
    struct Fixed(bool, bool, bool, bool);

    impl Claims for Fixed {
        fn ours(&self, head: bool) -> bool {
            if head { self.0 } else { self.1 }
        }
        fn theirs(&self, _: usize, tail: bool) -> bool {
            if tail { self.2 } else { self.3 }
        }
    }

    fn image(r: [f64; 4]) -> Mark {
        Mark {
            rect: r,
            kind: MarkKind::Image,
            nest: 0,
        }
    }

    fn glyph(x: f64, y: f64, c: char) -> Mark {
        Mark {
            rect: [x, y - 7.0, x + 4.5, y + 2.0],
            kind: MarkKind::Glyph {
                x,
                y,
                size: 10.0,
                text: c.to_string(),
            },
            nest: 0,
        }
    }

    /// A page being made: its drawings, in order, and what each glyph is.
    #[derive(Default)]
    struct Page {
        marks: Vec<Mark>,
        class: Vec<Class>,
    }

    impl Page {
        /// A word of three glyphs at baseline `y`, `x` from 100.
        fn word(&mut self, x: f64, y: f64, class: Class) {
            for k in 0..3u32 {
                self.marks.push(glyph(x + 5.0 * f64::from(k), y, 'a'));
                self.class.push(class);
            }
        }

        fn image(&mut self, r: [f64; 4]) {
            self.marks.push(image(r));
            self.class.push(Class::Neutral);
        }

        fn region(&self, mine: &[usize], claims: &Fixed) -> [f64; 4] {
            let mut class = self.class.clone();
            for (i, c) in class.iter_mut().enumerate() {
                if mine.contains(&(i / 3)) && matches!(self.marks[i].kind, MarkKind::Glyph { .. }) {
                    *c = Class::Mine;
                }
            }
            region(&self.marks, &class, (612.0, 792.0), Some(claims)).expect("found")
        }
    }

    /// Two figures stacked at a page's top (each an image above its
    /// caption, "Figure N:" a macro's glyphs), then the text: each one's
    /// region is its image and its whole caption, neither the other's
    /// image.
    #[test]
    fn stacked_floats_each_their_own() {
        let mut p = Page::default();
        let n = Class::Neutral;
        // (marks, three a word: 0-2 the head, 3 image, 4-6 "Figure 1:",
        // 7-9 its caption, 10 image, 11-13 "Figure 2:", 14-16 its
        // caption, 17-19 the text)
        p.word(100.0, 30.0, n);
        p.image([100.0, 60.0, 300.0, 180.0]);
        p.word(100.0, 200.0, n);
        p.word(130.0, 200.0, Class::Sure(1));
        p.image([100.0, 215.0, 300.0, 380.0]);
        p.word(100.0, 400.0, n);
        p.word(130.0, 400.0, Class::Sure(2));
        p.word(100.0, 600.0, Class::Sure(3));
        let mine = |m: &[usize]| -> Vec<usize> {
            // (word numbers to the marks' thirds, the images counted)
            m.to_vec()
        };
        // (each: drawings before its caption, none after; the neighbours
        // the other way round)
        let first = [7usize, 8, 9];
        let r1 = {
            let mut class = p.class.clone();
            for i in first {
                class[i] = Class::Mine;
            }
            region(
                &p.marks,
                &class,
                (612.0, 792.0),
                Some(&Fixed(true, false, false, true)),
            )
            .expect("the first")
        };
        assert_eq!(r1[1], 60.0, "{r1:?}");
        assert!(r1[3] < 215.0, "the second's image taken: {r1:?}");
        assert_eq!(r1[0], 100.0, "its number: {r1:?}");
        let r2 = {
            let mut class = p.class.clone();
            for i in [14usize, 15, 16] {
                class[i] = Class::Mine;
            }
            region(
                &p.marks,
                &class,
                (612.0, 792.0),
                Some(&Fixed(true, false, false, false)),
            )
            .expect("the second")
        };
        assert_eq!(r2[1], 215.0, "{r2:?}");
        assert!(r2[3] < 600.0 - 7.0, "{r2:?}");
        let _ = mine;
    }

    /// A table whose last rule is after its text, above a figure whose
    /// image is above its caption: the rule is the table's, the image the
    /// figure's (the widest gap between).
    #[test]
    fn a_rule_and_an_image_between_two_floats() {
        let mut p = Page::default();
        p.word(100.0, 100.0, Class::Sure(0));
        p.image([90.0, 104.0, 300.0, 105.0]);
        p.image([100.0, 125.0, 300.0, 280.0]);
        p.word(100.0, 300.0, Class::Sure(1));
        // (the table: marks 0-2; the figure's caption: 5-7)
        let table = p.region(&[0], &Fixed(false, true, false, true));
        assert_eq!(table[3], 105.0, "{table:?}");
        let mut class = p.class.clone();
        for c in &mut class[5..8] {
            *c = Class::Mine;
        }
        let figure = region(
            &p.marks,
            &class,
            (612.0, 792.0),
            Some(&Fixed(true, false, true, false)),
        )
        .expect("the figure");
        assert_eq!(figure[1], 125.0, "{figure:?}");
    }

    /// Codes are certain in rows of three typed ones.
    #[test]
    fn typed_text_is_certain() {
        let code = |start: u32, synth: bool| Code {
            file: 0,
            start,
            end: start + 1,
            synth,
            x: 0.0,
            y: 0.0,
        };
        let codes = [
            code(0, false),
            code(1, false),
            code(3, false),
            code(40, true),
            code(41, true),
            code(9, false),
        ];
        assert_eq!(
            trusted(&codes, &|_, _, _| true),
            [true, true, true, false, false, false]
        );
        assert_eq!(trusted(&codes, &|_, _, _| false), [false; 6]);
        assert!(whole_words(b"a figure.", 2, 8));
        assert!(!whole_words(b"a figure.", 3, 8));
    }

    /// An element's words: its text, not its commands, labels, files or
    /// columns.
    #[test]
    fn source_words() {
        let t = uncommented(
            b"\\begin{table}[t]\\caption{A \\emph{wide} fi\\'e table.}\\label{tab:x}\n\\begin{tabular}{lr}Na\\-me & 1\\\\ \\includegraphics[w=1]{pic}\\end{tabular}\\end{table}",
        );
        let words: Vec<String> = tokens(&t, 0, t.len()).into_iter().map(|t| t.text).collect();
        assert_eq!(words, ["a", "wide", "fi", "e", "table", "na", "me", "1"]);
        let tk = tokens(b"x Caption here", 0, 14);
        assert_eq!((tk[1].start, tk[1].end), (2, 9));
    }

    /// A page's words, and an element's words found where they are close
    /// together, not where its common words are strewn.
    #[test]
    fn words_found_together() {
        let mut marks = Vec::new();
        let put = |marks: &mut Vec<Mark>, words: &[&str], y: f64| {
            let mut x = 50.0;
            for w in words {
                for c in w.chars() {
                    marks.push(glyph(x, y, c));
                    x += 5.0;
                }
                x += 4.0;
            }
        };
        put(
            &mut marks,
            &["the", "figure", "is", "a", "big", "one"],
            100.0,
        );
        put(&mut marks, &["Figure", "1:", "A", "big", "figure."], 300.0);
        let words = page_words(&marks);
        let texts: Vec<&str> = words.iter().map(|w| w.text.as_str()).collect();
        assert_eq!(
            texts,
            [
                "the", "figure", "is", "a", "big", "one", "figure", "1", "a", "big", "figure"
            ]
        );
        let t = tokens(b"\\caption{A big figure.}", 0, 23);
        let found = find(&t, &words);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].pairs, [(8, 0), (9, 1), (10, 2)]);
    }

    #[test]
    fn png_is_a_png() {
        let p = png(2, 1, &[255, 0, 0, 255, 0, 0, 255, 255]);
        assert_eq!(&p[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(&p[12..16], b"IHDR");
        assert!(p.ends_with(&[0xAE, 0x42, 0x60, 0x82]), "IEND's CRC");
        let (img, w, h, changed) = diff_image(1, 1, &[0, 0, 0, 255], &[255, 255, 255, 255]);
        assert_eq!((w, h, changed), (19, 1, 1));
        assert_eq!(&img[(2 * 9) * 4..(2 * 9) * 4 + 4], &[220, 30, 30, 255]);
    }
}
