//! The incremental fuzz gate (AGENTS_BRIEF §5 "Edit", §6; `tests/fuzz.rs`,
//! `phitex-flow --fuzz`): from a seed, random edits applied in sequence to
//! a document, each edit on the text the last one left. After each edit
//! the build that saw every edit (`Build::update`, at a random schedule,
//! lean or not) must give what a cold build of the same text gives: the
//! DVI bytes, the log and the written files. Where a seed's builds keep
//! their nodes (not lean), the nodes run again are also bounded: at most
//! the readers of the names whose values changed and the nodes of the
//! pieces the edit made (over-invalidation is a failure too).
//!
//! A failure is written as a reproduction (the text before the edit, the
//! edit, the schedule) that `phitex-flow --fuzz-replay FILE` runs again.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crate::build::{Build, Options, Order, Output, Reads};
use crate::print::Limits;
use crate::tfm::Font;

// ------------------------------------------------------------ randomness

/// splitmix64: a seed's whole sequence of choices.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed ^ 0x5DEE_CE66_D1CE_4E5B)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A number below `n` (0 if `n` is 0).
    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.next_u64() % n as u64) as usize }
    }

    pub fn pick<T: Copy>(&mut self, v: &[T]) -> Option<T> {
        if v.is_empty() { None } else { Some(v[self.below(v.len())]) }
    }
}

// ------------------------------------------------------------ schedules

/// How a build is run: the threads, the reading policy, the scheduler's
/// order, whether settled pieces drop their nodes.
#[derive(Clone, Copy, Debug)]
pub struct Schedule {
    pub threads: usize,
    pub reads: Reads,
    pub order: Order,
    pub lean: bool,
}

impl Schedule {
    /// A schedule with one of `threads`, the rest at random; lean as
    /// `lean` says (`None`: at random).
    pub fn random(rng: &mut Rng, threads: &[usize], lean: Option<bool>) -> Schedule {
        let reads = if rng.below(2) == 0 { Reads::Suspend } else { Reads::Speculate };
        let order = if rng.below(2) == 0 { Order::Priority } else { Order::Random(rng.next_u64() % 1000) };
        Schedule {
            threads: rng.pick(threads).unwrap_or(1),
            reads,
            order,
            lean: lean.unwrap_or_else(|| rng.below(2) == 0),
        }
    }

    pub fn options(self, etex: bool, dump: bool) -> Options {
        Options {
            threads: self.threads,
            reads: self.reads,
            order: self.order,
            dump,
            etex,
            ..Options::default()
        }
    }

    fn text(self) -> String {
        format!(
            "threads={} reads={:?} order={:?} lean={}",
            self.threads, self.reads, self.order, self.lean
        )
    }

    fn parse(s: &str) -> Option<Schedule> {
        let mut sch = Schedule {
            threads: 1,
            reads: Reads::Suspend,
            order: Order::Priority,
            lean: true,
        };
        for kv in s.split_whitespace() {
            let (k, v) = kv.split_once('=')?;
            match k {
                "threads" => sch.threads = v.parse().ok()?,
                "reads" => {
                    sch.reads = match v {
                        "Suspend" => Reads::Suspend,
                        "Speculate" => Reads::Speculate,
                        _ => return None,
                    }
                }
                "order" => {
                    sch.order = match v {
                        "Priority" => Order::Priority,
                        _ => Order::Random(v.strip_prefix("Random(")?.strip_suffix(')')?.parse().ok()?),
                    }
                }
                "lean" => sch.lean = v.parse().ok()?,
                _ => return None,
            }
        }
        Some(sch)
    }
}

// ------------------------------------------------------------ documents

/// What a document is built with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    /// plain.tex, cmr10 and the fonts TeX Live has.
    Plain,
    /// TRIP's two jobs as a session (`graph::jobs`), the edits in pass
    /// 2's text.
    Trip,
    /// A LaTeX document: a session of two jobs, `latex.ini` (the format,
    /// `latex.ltx`, as the prelude until the saved format comes) and the
    /// document (`&latex NAME`), in extended mode, the edits in the
    /// document's text.
    Latex(Arc<LatexSetup>),
}

impl Kind {
    /// Whether its builds run in e-TeX's extended mode.
    pub fn etex(&self) -> bool {
        matches!(self, Kind::Latex(_))
    }
}

/// Where a LaTeX document's files are, as `tests/latex_docs.rs` sets them:
/// the directories `latex -recorder` read them from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LatexSetup {
    pub jobname: String,
    pub input_dirs: Vec<PathBuf>,
    pub font_dirs: Vec<PathBuf>,
}

/// A document of the corpus.
#[derive(Clone, Debug)]
pub struct Doc {
    pub name: String,
    pub kind: Kind,
    pub text: String,
    /// Where the body begins: edits go after it (never into plain.tex).
    pub body: usize,
    /// Files `\input` finds in memory (`Build::set_file`).
    pub files: Vec<(String, String)>,
    pub font: Arc<Font>,
}

/// The second file an edit may `\input`, and edit.
pub const SUB: &str = "fzsub.tex";

/// What every plain document begins its body with: a macro its body may
/// use and redefine, a conditional it may test and toggle.
const SCAFFOLD: &str = "\\def\\fz{lazy}\\newif\\iffz \\fztrue\n";

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn latin1(b: &[u8]) -> String {
    b.iter().map(|&c| char::from(c)).collect()
}

/// plain.tex (`tests/dvi/plaintext.tex` before its body, with
/// `hyphen.tex`), as `tests/plain.rs` makes it.
pub fn plain() -> Option<String> {
    let r = root();
    let s = std::fs::read_to_string(r.join("tests/dvi/plaintext.tex")).ok()?;
    let at = s.find("\\output={}\n")?;
    let h = std::fs::read_to_string(r.join("tests/hyph/hyphen.tex")).ok()?;
    Some(s[..at].replace("% (\\input hyphen: no hyphenation patterns on the graph yet)\n", &h))
}

fn cmr10() -> Option<Arc<Font>> {
    let b = std::fs::read(root().join("../partex-core/testdata/cmr10.tfm")).ok()?;
    Font::parse("cmr10", &b).ok().map(Arc::new)
}

/// The bodies of `tests/plain/` (each ends with `\bye`).
pub const BODIES: [&str; 5] = ["story", "pages", "display", "table", "hyphens"];

fn plain_doc(name: &str, body: &str, font: &Arc<Font>, prelude: &str) -> Doc {
    let text = format!("{prelude}{SCAFFOLD}{body}");
    Doc {
        name: name.to_string(),
        kind: Kind::Plain,
        body: prelude.len(),
        text,
        files: vec![(SUB.to_string(), "Sub file text one.\n\nSub file text two, with \\fz.\n".to_string())],
        font: font.clone(),
    }
}

/// A large document (big2-like): every body of `tests/plain/` `copies`
/// times, the copies told apart, one `\bye` at the end.
pub fn big_body(copies: usize) -> Option<String> {
    let mut s = String::new();
    for k in 0..copies {
        for b in BODIES {
            let t = std::fs::read_to_string(root().join(format!("tests/plain/{b}.tex"))).ok()?;
            let t = t.trim_end().strip_suffix("\\bye").unwrap_or(&t).to_string();
            let _ = write!(s, "\\message{{[part {k} {b}]}}\n{t}\n\n");
        }
    }
    s.push_str("\\bye\n");
    Some(s)
}

/// The corpus: the plain documents, a large one, and TRIP's pass 2.
pub fn corpus() -> Vec<Doc> {
    let mut out = Vec::new();
    if let (Some(prelude), Some(font)) = (plain(), cmr10()) {
        for b in BODIES {
            if let Ok(t) = std::fs::read_to_string(root().join(format!("tests/plain/{b}.tex"))) {
                out.push(plain_doc(b, &t, &font, &prelude));
            }
        }
        if let Some(big) = big_body(4) {
            out.push(plain_doc("big", &big, &font, &prelude));
        }
    }
    if let Some(d) = trip_doc(None) {
        out.push(d);
    }
    out
}

fn trip_dir() -> PathBuf {
    root().join("../../bench/trip")
}

/// TRIP's text as pass 2 reads it (`pass2`: edited), the session's text.
pub fn trip_doc(pass2: Option<&str>) -> Option<Doc> {
    let d = trip_dir();
    let tex = latin1(&std::fs::read(d.join("trip.tex")).ok()?);
    let font = Arc::new(Font::parse("trip", &std::fs::read(d.join("trip.tfm")).ok()?).ok()?);
    let p2 = pass2.unwrap_or(&tex);
    let text = crate::graph::jobs(&[&tex, p2]);
    // (edits from pass 2's line 100 on: pass 1 has stopped long before)
    let job2 = text.rfind(crate::graph::JOB).map_or(0, |i| i + crate::graph::JOB.len_utf8());
    let line100 = p2.match_indices('\n').nth(99).map_or(0, |(i, _)| i + 1);
    Some(Doc {
        name: "trip".to_string(),
        kind: Kind::Trip,
        body: job2 + line100,
        text,
        files: Vec::new(),
        font,
    })
}

/// A build as `doc` is built (its font, its files; TRIP's session).
pub fn builder(doc: &Doc, files: &[(String, String)]) -> Build {
    let mut b = Build::new(doc.font.clone());
    if let Kind::Latex(s) = &doc.kind {
        b.set_font_dirs(s.font_dirs.clone());
        b.set_input_dirs(s.input_dirs.clone());
        b.set_jobname(&s.jobname);
        b.set_terminal(&["latex.ini", &format!("&latex {}", s.jobname)]);
        b.set_limits(Limits::TEX_LIVE);
    }
    if doc.kind == Kind::Trip {
        let d = trip_dir();
        b.set_font_dirs(vec![d.clone()]);
        b.set_jobname("trip");
        let mut lines = Vec::new();
        for f in ["trip1.in", "trip2.in"] {
            if let Ok(t) = std::fs::read_to_string(d.join(f)) {
                lines.extend(t.lines().map(str::to_string));
            }
        }
        let l: Vec<&str> = lines.iter().map(String::as_str).collect();
        b.set_terminal(&l);
        b.set_interaction(3);
        b.set_limits(Limits::TRIP);
    }
    for (n, t) in files {
        b.set_file(n, t);
    }
    b
}

// ------------------------------------------------------------ edits

/// An edit: `del` bytes at byte `at` of the main text (or of `file`)
/// replaced by `ins`; `what` names the kind.
#[derive(Clone, Debug)]
pub struct Edit {
    pub file: Option<String>,
    pub at: usize,
    pub del: usize,
    pub ins: String,
    pub what: String,
}

impl Edit {
    pub fn apply(&self, text: &mut String, files: &mut [(String, String)]) {
        let t = match &self.file {
            None => text,
            Some(f) => match files.iter_mut().find(|(n, _)| n == f) {
                Some((_, t)) => t,
                None => return,
            },
        };
        let end = (self.at + self.del).min(t.len());
        if t.is_char_boundary(self.at) && t.is_char_boundary(end) {
            t.replace_range(self.at..end, &self.ins);
        }
    }
}

const WORDS: [&str; 8] = ["fox", "quick", "graph", "edit", "zebra", "jumps", "over", "lazy"];

/// The facts about a text an edit is chosen from (positions are byte
/// offsets, all on character boundaries, all at or after `from`).
struct Scan {
    /// Letters not in a control sequence's name, nor in a `\def`'s first
    /// line (its parameter text and the start of its body).
    letters: Vec<usize>,
    /// Spaces in a line of text, at brace depth 0.
    spaces: Vec<usize>,
    /// Line starts at brace depth 0 whose line has text.
    lines: Vec<usize>,
    /// Empty lines (their `\n`), at brace depth 0.
    blanks: Vec<usize>,
    /// Paragraphs (between empty lines at brace depth 0): start, end.
    paras: Vec<(usize, usize)>,
    /// Letters inside an `\halign{…}` or a `$$…$$`.
    inner: Vec<usize>,
}

fn scan(t: &str, from: usize) -> Scan {
    let b = t.as_bytes();
    let mut s = Scan {
        letters: Vec::new(),
        spaces: Vec::new(),
        lines: Vec::new(),
        blanks: Vec::new(),
        paras: Vec::new(),
        inner: Vec::new(),
    };
    let mut depth = 0i32;
    let mut i = from;
    let mut in_cs = false;
    let mut line_start = true;
    let mut def_line = false;
    let mut para_start = from;
    // (inside an `\halign{`: its brace depth; inside `$$`)
    let mut align: Option<i32> = None;
    let mut display = false;
    while i < b.len() {
        let c = b[i];
        if !t.is_char_boundary(i) {
            i += 1;
            continue;
        }
        if line_start {
            def_line = t[i..].starts_with("\\def") || t[i..].starts_with("\\gdef") || t[i..].starts_with("\\edef");
            if c == b'\n' && depth == 0 {
                s.blanks.push(i);
                if para_start < i {
                    s.paras.push((para_start, i));
                }
                para_start = i + 1;
            } else if depth == 0 && c != b'\n' && c != b'%' {
                s.lines.push(i);
            }
        }
        line_start = c == b'\n';
        match c {
            b'\\' => {
                if t[i..].starts_with("\\halign{") {
                    align = Some(depth + 1);
                }
                // (a one-character name: skip it)
                if i + 1 < b.len() && !b[i + 1].is_ascii_alphabetic() {
                    i += 2;
                    in_cs = false;
                    continue;
                }
                in_cs = true;
            }
            b'{' => depth += 1,
            b'}' => {
                if align == Some(depth) {
                    align = None;
                }
                depth -= 1;
            }
            b'$' if b.get(i + 1) == Some(&b'$') => {
                display = !display;
                i += 2;
                in_cs = false;
                continue;
            }
            b'%' => {
                // (a comment: to the line's end)
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
                in_cs = false;
                continue;
            }
            b' ' if depth == 0 && !in_cs && !def_line => s.spaces.push(i),
            c if c.is_ascii_alphabetic() => {
                if !in_cs && !def_line {
                    if align.is_some() || display {
                        s.inner.push(i);
                    } else {
                        s.letters.push(i);
                    }
                }
                i += 1;
                continue;
            }
            _ => {}
        }
        if c != b'\\' {
            in_cs = false;
        }
        i += 1;
    }
    if para_start < b.len() {
        s.paras.push((para_start, b.len()));
    }
    s
}

fn letter(rng: &mut Rng) -> char {
    char::from(b'a' + rng.below(26) as u8)
}

/// One random edit of the document as it is now (`None`: the kind drawn
/// had nothing to edit).
fn edit_once(rng: &mut Rng, doc: &Doc, text: &str, files: &[(String, String)]) -> Option<Edit> {
    if matches!(doc.kind, Kind::Latex(_)) {
        return edit_latex(rng, doc, text);
    }
    let s = scan(text, doc.body);
    let main = |at: usize, del: usize, ins: String, what: &str| Edit {
        file: None,
        at,
        del,
        ins,
        what: what.to_string(),
    };
    let trip = doc.kind == Kind::Trip;
    let kinds: &[&str] = if trip {
        &["char-insert", "char-delete", "char-replace", "word", "blank-insert", "blank-delete", "split", "swap"]
    } else {
        &[
            "char-insert",
            "char-delete",
            "char-replace",
            "word",
            "blank-insert",
            "blank-delete",
            "split",
            "swap",
            "param",
            "use-macro",
            "redefine",
            "use-if",
            "toggle",
            "group",
            "input-add",
            "input-remove",
            "sub-edit",
            "footnote",
            "mark",
            "inner",
        ]
    };
    let kind = rng.pick(kinds)?;
    match kind {
        "char-insert" => {
            let at = rng.pick(&s.letters)?;
            Some(main(at, 0, letter(rng).to_string(), kind))
        }
        "char-delete" => {
            let at = rng.pick(&s.letters)?;
            Some(main(at, 1, String::new(), kind))
        }
        "char-replace" => {
            let at = rng.pick(&s.letters)?;
            Some(main(at, 1, letter(rng).to_string(), kind))
        }
        "word" => {
            let at = rng.pick(&s.spaces)?;
            let w = rng.pick(&WORDS)?;
            match rng.below(3) {
                0 => Some(main(at, 0, format!(" {w}"), "word-insert")),
                1 => {
                    // (the word after the space, to the next space or line end)
                    let rest = &text[at + 1..];
                    let n = rest.find([' ', '\n']).unwrap_or(rest.len());
                    let word = &rest[..n];
                    (n > 0 && word.bytes().all(|c| c.is_ascii_alphabetic()))
                        .then(|| main(at, n + 1, String::new(), "word-delete"))
                }
                _ => {
                    let rest = &text[at + 1..];
                    let n = rest.find([' ', '\n']).unwrap_or(rest.len());
                    let word = &rest[..n];
                    (n > 0 && word.bytes().all(|c| c.is_ascii_alphabetic()))
                        .then(|| main(at + 1, n, w.to_string(), "word-replace"))
                }
            }
        }
        "blank-insert" => {
            let at = rng.pick(&s.lines)?;
            Some(main(at, 0, "\n".into(), kind))
        }
        "blank-delete" => {
            let at = rng.pick(&s.blanks)?;
            Some(main(at, 1, String::new(), kind))
        }
        "split" => {
            let at = rng.pick(&s.spaces)?;
            Some(main(at, 1, "\n\n".into(), kind))
        }
        "swap" => {
            if s.paras.len() < 2 {
                return None;
            }
            let k = rng.below(s.paras.len() - 1);
            let (a0, a1) = s.paras[k];
            let (b0, b1) = s.paras[k + 1];
            let a = &text[a0..a1];
            let b = &text[b0..b1];
            // (never the paragraph holding `\bye`: the document's end stays)
            if a.contains("\\bye") || b.contains("\\bye") || a.contains("\\end") || b.contains("\\end") {
                return None;
            }
            Some(main(a0, b1 - a0, format!("{b}{}{a}", &text[a1..b0]), kind))
        }
        "param" => {
            let at = rng.pick(&s.lines)?;
            let p = match rng.below(4) {
                0 => format!("\\hsize={}pt", 200 + rng.below(200)),
                1 => format!("\\parindent={}pt", rng.below(30)),
                2 => format!("\\baselineskip={}pt", 10 + rng.below(8)),
                _ => format!("\\tolerance={}", 100 + rng.below(9900)),
            };
            Some(main(at, 0, format!("{p}\n"), kind))
        }
        "use-macro" => {
            let at = rng.pick(&s.spaces)?;
            Some(main(at, 0, " \\fz\\ ".into(), kind))
        }
        "redefine" => {
            let at = text.find("\\def\\fz{")? + "\\def\\fz{".len();
            let n = text[at..].find('}')?;
            let w = rng.pick(&WORDS)?;
            Some(main(at, n, w.to_string(), kind))
        }
        "use-if" => {
            let at = rng.pick(&s.spaces)?;
            Some(main(at, 0, " \\iffz yes\\else no\\fi\\ ".into(), kind))
        }
        "toggle" => {
            if rng.below(2) == 0 {
                // (the switch)
                if let Some(at) = text.find("\\fztrue") {
                    return Some(main(at, 7, "\\fzfalse".into(), kind));
                }
                let at = text.find("\\fzfalse")?;
                Some(main(at, 8, "\\fztrue".into(), kind))
            } else {
                // (an arm's text)
                let at = text.find("\\else no")? + "\\else ".len();
                Some(main(at, 2, rng.pick(&WORDS)?.to_string(), "toggle-arm"))
            }
        }
        "group" => {
            let a = rng.pick(&s.spaces)?;
            let line_end = text[a..].find('\n').map_or(text.len(), |n| a + n);
            let later: Vec<usize> = s.spaces.iter().copied().filter(|&x| x > a && x < line_end).collect();
            let b = rng.pick(&later)?;
            Some(main(a + 1, b - a - 1, format!("{{{}}}", &text[a + 1..b]), kind))
        }
        "input-add" => {
            let at = rng.pick(&s.lines)?;
            Some(main(at, 0, "\\input fzsub\n".into(), kind))
        }
        "input-remove" => {
            let at = text.find("\\input fzsub\n")?;
            Some(main(at, "\\input fzsub\n".len(), String::new(), kind))
        }
        "sub-edit" => {
            let (name, sub) = files.iter().find(|(n, _)| n == SUB)?;
            let sc = scan(sub, 0);
            let at = rng.pick(&sc.letters)?;
            let (del, ins) = match rng.below(3) {
                0 => (0, letter(rng).to_string()),
                1 => (1, String::new()),
                _ => (1, letter(rng).to_string()),
            };
            Some(Edit {
                file: Some(name.clone()),
                at,
                del,
                ins,
                what: kind.to_string(),
            })
        }
        "footnote" => {
            let at = rng.pick(&s.spaces)?;
            Some(main(at, 0, format!("\\footnote*{{A note on {}.}} ", rng.pick(&WORDS)?), kind))
        }
        "mark" => {
            let at = rng.pick(&s.spaces)?;
            Some(main(at, 0, format!(" \\mark{{{}}}", rng.pick(&WORDS)?), kind))
        }
        "inner" => {
            let at = rng.pick(&s.inner)?;
            Some(main(at, 1, letter(rng).to_string(), kind))
        }
        _ => None,
    }
}

/// A random edit (a few kinds tried until one has something to edit).
pub fn random_edit(rng: &mut Rng, doc: &Doc, text: &str, files: &[(String, String)]) -> Option<Edit> {
    (0..32).find_map(|_| edit_once(rng, doc, text, files))
}

// ------------------------------------------------------------ checks

/// What a failure is.
#[derive(Clone, Debug)]
pub enum Fail {
    /// The edit's output differs from the cold build's (which parts).
    Mismatch(String),
    /// More nodes ran than the readers of what changed and the edit's
    /// pieces hold.
    Over { ran: u64, allowed: u64, detail: String },
    /// A build panicked.
    Panic(String),
    /// The seed did not finish in time.
    Timeout,
}

impl Fail {
    pub fn is_over(&self) -> bool {
        matches!(self, Fail::Over { .. })
    }

    /// What [`Fail::text`] wrote (an over-invalidation's numbers kept, its
    /// detail as written).
    fn parse(v: &str) -> Fail {
        let num = |k: &str| v.split_whitespace().find_map(|w| w.strip_prefix(k)?.parse().ok()).unwrap_or(0);
        if let Some(w) = v.strip_prefix("mismatch ") {
            Fail::Mismatch(w.to_string())
        } else if v.starts_with("over ") {
            let detail = v.splitn(4, ' ').nth(3).unwrap_or("").to_string();
            Fail::Over { ran: num("ran="), allowed: num("allowed="), detail }
        } else if let Some(m) = v.strip_prefix("panic ") {
            Fail::Panic(m.to_string())
        } else if v == "timeout" {
            Fail::Timeout
        } else {
            Fail::Mismatch(v.to_string())
        }
    }

    fn text(&self) -> String {
        match self {
            Fail::Mismatch(w) => format!("mismatch {w}"),
            Fail::Over { ran, allowed, detail } => format!("over ran={ran} allowed={allowed} {detail}"),
            Fail::Panic(m) => format!("panic {}", m.replace('\n', " ")),
            Fail::Timeout => "timeout".into(),
        }
    }
}

/// A failure's reproduction: the document and its files before the edit,
/// the edit, the schedule of the build that saw the edit (and of the one
/// before it).
#[derive(Clone, Debug)]
pub struct Repro {
    pub doc: String,
    pub seed: u64,
    pub step: usize,
    pub before: Schedule,
    pub schedule: Schedule,
    pub fail: Fail,
    pub text: String,
    pub files: Vec<(String, String)>,
    pub edit: Edit,
    /// The run it came from (`Config::threads`, `Config::doc`): replayed with
    /// its history, the seed is run again to its step.
    pub threads: Vec<usize>,
    pub only: Option<String>,
}

const SEP: &str = "\n\u{1}";

impl Repro {
    /// The same, its edit none: whether the update fails whatever the edit.
    #[must_use]
    pub fn noop(&self) -> Repro {
        let mut r = self.clone();
        r.edit = Edit { file: None, at: 0, del: 0, ins: String::new(), what: "none".into() };
        r
    }

    pub fn write(&self) -> String {
        let mut s = String::from("phitex-flow fuzz repro\n");
        let _ = writeln!(s, "doc {}", self.doc);
        let _ = writeln!(s, "seed {} step {}", self.seed, self.step);
        let _ = writeln!(s, "before {}", self.before.text());
        let _ = writeln!(s, "schedule {}", self.schedule.text());
        let _ = writeln!(s, "fail {}", self.fail.text());
        let threads: Vec<String> = self.threads.iter().map(ToString::to_string).collect();
        let _ = writeln!(s, "history threads={} doc={}", threads.join(","), self.only.as_deref().unwrap_or("-"));
        let _ = write!(
            s,
            "edit {} {} {} {}",
            self.edit.file.as_deref().unwrap_or("-"),
            self.edit.at,
            self.edit.del,
            self.edit.what
        );
        let _ = write!(s, "{SEP}ins\n{}", self.edit.ins);
        let _ = write!(s, "{SEP}text\n{}", self.text);
        for (n, t) in &self.files {
            let _ = write!(s, "{SEP}file {n}\n{t}");
        }
        s
    }

    pub fn parse(s: &str) -> Option<Repro> {
        let mut parts = s.split(SEP);
        let head = parts.next()?;
        let mut doc = None;
        let (mut seed, mut step) = (0, 0);
        let (mut before, mut schedule) = (None, None);
        let mut fail = Fail::Mismatch(String::new());
        let mut edit = None;
        let (mut threads, mut only) = (vec![1, 8], None);
        for l in head.lines() {
            let (k, v) = l.split_once(' ').unwrap_or((l, ""));
            match k {
                "doc" => doc = Some(v.to_string()),
                "seed" => {
                    let mut w = v.split_whitespace();
                    seed = w.next()?.parse().ok()?;
                    step = w.nth(1)?.parse().ok()?;
                }
                "before" => before = Schedule::parse(v),
                "schedule" => schedule = Schedule::parse(v),
                "fail" => fail = Fail::parse(v),
                "history" => {
                    for w in v.split_whitespace() {
                        if let Some(t) = w.strip_prefix("threads=") {
                            threads = t.split(',').filter_map(|x| x.parse().ok()).collect();
                        } else if let Some(d) = w.strip_prefix("doc=") {
                            only = (d != "-").then(|| d.to_string());
                        }
                    }
                }
                "edit" => {
                    let mut w = v.splitn(4, ' ');
                    let f = w.next()?;
                    edit = Some(Edit {
                        file: (f != "-").then(|| f.to_string()),
                        at: w.next()?.parse().ok()?,
                        del: w.next()?.parse().ok()?,
                        ins: String::new(),
                        what: w.next().unwrap_or("").to_string(),
                    });
                }
                _ => {}
            }
        }
        let mut edit = edit?;
        let mut text = String::new();
        let mut files = Vec::new();
        for p in parts {
            let (name, body) = p.split_once('\n').unwrap_or((p, ""));
            if name == "ins" {
                edit.ins = body.to_string();
            } else if name == "text" {
                text = body.to_string();
            } else if let Some(f) = name.strip_prefix("file ") {
                files.push((f.to_string(), body.to_string()));
            }
        }
        Some(Repro {
            doc: doc?,
            seed,
            step,
            before: before?,
            schedule: schedule?,
            fail,
            text,
            files,
            edit,
            threads,
            only,
        })
    }
}

/// How a run of seeds is made.
#[derive(Clone, Debug)]
pub struct Config {
    /// The thread counts schedules draw from.
    pub threads: Vec<usize>,
    /// Check over-invalidation (seeds whose builds keep their nodes).
    pub over: bool,
    /// ... against the bound of a graph of nodes (`over_invalidation`).
    pub strict: bool,
    /// A seed's time.
    pub timeout: Duration,
    /// Only this document (by name).
    pub doc: Option<String>,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            threads: vec![1, 8],
            over: true,
            strict: false,
            timeout: Duration::from_secs(600),
            doc: None,
        }
    }
}

/// Where an edit's output differs from a cold build's.
fn differs(a: &Output, b: &Output) -> Option<String> {
    let mut w = Vec::new();
    if a.dvi != b.dvi {
        w.push("dvi");
    }
    if a.log != b.log {
        w.push("log");
    }
    if a.writes != b.writes {
        w.push("writes");
    }
    (!w.is_empty()).then(|| w.join(","))
}

/// The cold build of `text` the edit's output must equal.
pub fn cold(doc: &Doc, text: &str, files: &[(String, String)]) -> Output {
    let opts = Options {
        // (a LaTeX document's at 8: its format's prelude is long)
        threads: if doc.kind.etex() { 8 } else { 1 },
        reads: Reads::Suspend,
        order: Order::Priority,
        dump: false,
        etex: doc.kind.etex(),
        ..Options::default()
    };
    builder(doc, files).update(text, opts)
}

/// A piece as a dump's header and node lines show it.
#[derive(Default)]
struct PieceFacts {
    /// Its id, the same in every build while the piece is.
    id: u64,
    /// Its first characters (`shown40`).
    text: String,
    nodes: u64,
    /// Its nodes run in this build.
    run: u64,
    /// Entered at the start (not in a predecessor's state).
    start: bool,
    /// Run in this build.
    ran: bool,
    /// Each node's reads from before the piece: (name, value).
    reads: Vec<Vec<(String, String)>>,
}

/// What a dump says (`Options::dump`): each piece's header facts and its
/// nodes' reads, and the names a piece run defines with values shown only
/// in part.
#[derive(Default)]
struct Facts {
    pieces: Vec<PieceFacts>,
    /// The names shown only in part (`opaque`) some node of a piece run
    /// in this build defines.
    defined_in: BTreeSet<String>,
}

/// Whether the dump shows a value only in part (a token list cut at 24,
/// a box by its dimensions, a stream by its position…): two such values
/// shown the same may differ.
fn opaque(v: &str) -> bool {
    v.ends_with("...")
        || v.ends_with("...}")
        || v.contains("box(")
        || v.ends_with(" bytes")
        || v.starts_with("stream at ")
        || v.starts_with("parshape ")
        || v.starts_with("penalties ")
        || v.starts_with("split marks ")
        || v.ends_with(" patterns")
        || v.ends_with(" exceptions")
}

/// The number before `what` in `s` (`"318 nodes"`).
fn count_before(s: &str, what: &str) -> Option<u64> {
    let i = s.find(what)?;
    s[..i].rsplit(|c: char| !c.is_ascii_digit()).next()?.parse().ok()
}

fn facts(dump: &str) -> Facts {
    let mut f = Facts::default();
    for l in dump.lines() {
        if let Some(rest) = l.strip_prefix("piece ") {
            let head = rest.split_once("]: ").map_or(rest, |(h, _)| h);
            f.pieces.push(PieceFacts {
                id: head
                    .split_once("(id ")
                    .and_then(|(_, r)| r.split_once(')'))
                    .and_then(|(n, _)| n.parse().ok())
                    .unwrap_or(u64::MAX),
                text: rest.find("; \"").map_or("", |i| &rest[i + 2..]).to_string(),
                nodes: count_before(head, " nodes").unwrap_or(0),
                run: count_before(head, " run").unwrap_or(0),
                start: rest.contains("entered Start"),
                ran: head.contains(") [ran;"),
                reads: Vec::new(),
            });
        } else if l.starts_with("  n") {
            let ran = f.pieces.last().is_some_and(|p| p.ran);
            let mut reads = Vec::new();
            for seg in l.split(" | ").skip(1) {
                if let Some(kv) = seg.strip_prefix("reads ") {
                    let Some((k, v)) = kv.split_once(" = ") else { continue };
                    // (from a node of the piece: inside it, not a read from before)
                    let v = match v.rsplit_once(" from ") {
                        Some((_, src)) if src.starts_with('n') => continue,
                        Some((x, _)) => x,
                        None => v.strip_suffix(" initial").unwrap_or(v),
                    };
                    reads.push((k.to_string(), v.to_string()));
                } else if let Some(kv) = seg.strip_prefix("defines ") {
                    let kv = kv.strip_prefix("global ").unwrap_or(kv);
                    if let Some((k, v)) = kv.split_once(" = ")
                        && ran
                        && opaque(v)
                    {
                        f.defined_in.insert(k.to_string());
                    }
                }
            }
            if let Some(p) = f.pieces.last_mut() {
                p.reads.push(reads);
            }
        }
    }
    f
}

/// A piece's first characters as the dump shows them.
fn shown40(p: &[char]) -> String {
    format!("{:?}", p.iter().take(40).collect::<String>())
}

/// Whether a build after an edit ran more than it had to, from the graphs
/// before and after (`dump_before`, `dump_after`). A piece whose text the
/// edit changed (the main text's paragraphs, `text_before` to
/// `text_after`; an `\input` file's, `in_file`), new, or entered in a
/// predecessor's state may run all its nodes. Any other piece reads from
/// before it the same names as it did: the names it reads with another
/// value now (or with a value shown only in part that a piece run
/// defines) are its changed names, and it may run none of its nodes
/// before the first reading one (a piece runs in order: from there on, all
/// of it may run). `strict`: the bound the design asks of a graph of
/// nodes, only the readers of changed names outside the edited pieces (a
/// dump does not mark the nodes run: checked on the sum).
/// `None` if within it, else the bound and what it is made of.
fn over_invalidation(
    text_before: &str,
    text_after: &str,
    in_file: bool,
    dump_before: &str,
    dump_after: &str,
    strict: bool,
) -> Option<(u64, String)> {
    let (a, b) = (facts(dump_before), facts(dump_after));
    let old_pieces: BTreeMap<u64, &PieceFacts> = a.pieces.iter().map(|p| (p.id, p)).collect();
    // (the main text's paragraphs the edit made: not among the old ones)
    let mut old: BTreeMap<Arc<[char]>, usize> = BTreeMap::new();
    for p in crate::graph::cut(text_before) {
        *old.entry(p).or_default() += 1;
    }
    let new_cut = crate::graph::cut(text_after);
    let debug = std::env::var_os("PHITEX_FUZZ_DEBUG").is_some();
    if debug {
        eprintln!("{} paragraphs before, {} after", old.values().sum::<usize>(), new_cut.len());
    }
    let mut edited = Vec::with_capacity(new_cut.len());
    for p in &new_cut {
        edited.push(match old.get_mut(p) {
            Some(c) if *c > 0 => {
                *c -= 1;
                false
            }
            _ => true,
        });
    }
    // (the dump's pieces in order, each the main text's next paragraph,
    // or a file's after the piece reading it)
    let (mut ran, mut allowed, mut readers, mut free) = (0, 0, 0, 0);
    let mut changed_all: BTreeSet<String> = BTreeSet::new();
    let mut worst: Option<(u64, u64, &str)> = None;
    let mut j = 0;
    for p in &b.pieces {
        // (a kept piece's header counts the nodes it ran when it was built)
        let run = if p.ran { p.run } else { 0 };
        ran += run;
        let main = j < new_cut.len() && p.text == shown40(&new_cut[j]);
        let span = usize::from(main);
        let is_edited = (main && edited[j..(j + span).min(edited.len())].iter().any(|e| *e)) || (!main && in_file);
        j += span;
        let old_p = old_pieces.get(&p.id).filter(|o| o.text == p.text);
        if debug {
            eprintln!("piece id {} at paragraph {}: main {main} span {span} edited {is_edited} old {} ran {run} of {}", p.id, j - span, old_p.is_some(), p.nodes);
        }
        let may = match old_p {
            Some(o) if !is_edited && p.start => {
                // (its changed names: read with values it did not read before)
                let before: BTreeSet<(&str, &str)> =
                    o.reads.iter().flatten().map(|(k, v)| (k.as_str(), v.as_str())).collect();
                let changed = |(k, v): &(String, String)| {
                    !before.contains(&(k.as_str(), v.as_str()))
                        || (opaque(v) && b.defined_in.contains(k))
                        || (in_file && k.starts_with("file "))
                };
                let reading: Vec<bool> = p.reads.iter().map(|rs| rs.iter().any(changed)).collect();
                for (k, v) in p.reads.iter().flatten() {
                    if changed(&(k.clone(), v.clone())) {
                        changed_all.insert(k.clone());
                    }
                }
                readers += reading.iter().filter(|r| **r).count() as u64;
                if strict {
                    0
                } else {
                    reading.iter().position(|r| *r).map_or(0, |i| p.nodes.saturating_sub(i as u64))
                }
            }
            _ => {
                free += p.nodes;
                p.nodes
            }
        };
        allowed += may;
        if !strict && run > may && worst.is_none_or(|w| run - may > w.0 - w.1) {
            worst = Some((run, may, p.text.as_str()));
        }
    }
    if strict {
        allowed += readers;
    }
    (ran > allowed || worst.is_some()).then(|| {
        let names: Vec<&str> = changed_all.iter().take(12).map(String::as_str).collect();
        let at = worst.map_or(String::new(), |(r, m, t)| format!(" worst piece {t} ran {r} of {m} allowed;"));
        (
            allowed,
            format!(
                "{}readers={readers} edited-new-or-edge-nodes={free};{at} changed={}",
                if strict { "strict " } else { "" },
                names.join(",")
            ),
        )
    })
}

fn panic_text(e: &(dyn std::any::Any + Send)) -> String {
    e.downcast_ref::<String>()
        .cloned()
        .or_else(|| e.downcast_ref::<&str>().map(|s| (*s).to_string()))
        .unwrap_or_else(|| "panic".into())
}

/// A seed: `count` edits on document `doc`, each checked. The first
/// failure ends the seed (later edits would build on it).
pub fn run_seed(corpus: &[Doc], seed: u64, count: usize, cfg: &Config) -> Option<Repro> {
    run_seed_at(corpus, seed, count, cfg, None)
}

/// [`run_seed`], each step's reproduction (as a timeout) sent to `pending`
/// before its update: a seed that does not end says where it hung.
fn run_seed_at(
    corpus: &[Doc],
    seed: u64,
    count: usize,
    cfg: &Config,
    pending: Option<&std::sync::mpsc::Sender<Repro>>,
) -> Option<Repro> {
    let docs: Vec<&Doc> = corpus.iter().filter(|d| cfg.doc.as_ref().is_none_or(|n| *n == d.name)).collect();
    let doc = (*docs.get((seed % docs.len().max(1) as u64) as usize)?).clone();
    let mut rng = Rng::new(seed);
    // (seeds keeping their nodes check over-invalidation; the others are
    // lean, or lean at random)
    let lean = match seed % 3 {
        0 => None,
        1 => Some(true),
        _ => Some(false),
    };
    let check_over = cfg.over && lean == Some(false);
    let mut text = doc.text.clone();
    let mut files = doc.files.clone();
    let etex = doc.kind.etex();
    // (a LaTeX build's graph is latex.ltx's, millions of nodes: no dumps,
    // no over-invalidation check, until the saved format)
    let check_over = check_over && (!etex || std::env::var_os("PHITEX_FUZZ_LATEX_OVER").is_some());
    let mut sched = Schedule::random(&mut rng, &cfg.threads, lean);
    let mut b = builder(&doc, &files);
    b.set_lean(sched.lean);
    let mut prev = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        b.update(&text, sched.options(etex, check_over))
    })) {
        Ok(o) => o,
        Err(e) => {
            return Some(Repro {
                doc: doc.name.clone(),
                seed,
                step: 0,
                before: sched,
                schedule: sched,
                fail: Fail::Panic(panic_text(&*e)),
                text: text.clone(),
                files,
                edit: Edit { file: None, at: 0, del: 0, ins: String::new(), what: "none".into() },
                threads: cfg.threads.clone(),
                only: cfg.doc.clone(),
            });
        }
    };
    for step in 1..=count {
        let Some(edit) = random_edit(&mut rng, &doc, &text, &files) else { continue };
        let before = (text.clone(), files.clone());
        edit.apply(&mut text, &mut files);
        for (n, t) in &files {
            b.set_file(n, t);
        }
        let last = sched;
        sched = Schedule::random(&mut rng, &cfg.threads, lean);
        b.set_lean(sched.lean);
        let repro = |fail: Fail| Repro {
            doc: doc.name.clone(),
            seed,
            step,
            before: last,
            schedule: sched,
            fail,
            text: before.0.clone(),
            files: before.1.clone(),
            edit: edit.clone(),
            threads: cfg.threads.clone(),
            only: cfg.doc.clone(),
        };
        if let Some(p) = pending {
            let _ = p.send(repro(Fail::Timeout));
        }
        let got = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let inc = b.update(&text, sched.options(etex, check_over));
            let c = cold(&doc, &text, &files);
            (inc, c)
        }));
        let (inc, c) = match got {
            Ok(x) => x,
            Err(e) => return Some(repro(Fail::Panic(panic_text(&*e)))),
        };
        if let Some(w) = differs(&inc, &c) {
            if let Some(d) = std::env::var_os("PHITEX_FUZZ_DUMP") {
                write_outputs(&PathBuf::from(d), &prev, &inc, &c);
            }
            return Some(repro(Fail::Mismatch(w)));
        }
        if check_over
            && let Some((allowed, detail)) =
                over_invalidation(&before.0, &text, edit.file.is_some(), &prev.dump, &inc.dump, cfg.strict)
        {
            return Some(repro(Fail::Over {
                ran: inc.stats.commands_run,
                allowed,
                detail,
            }));
        }
        prev = inc;
    }
    None
}

/// [`run_seed`] in a thread of its own, given `cfg.timeout`: a seed still
/// running then is a failure (its thread is left behind).
pub fn run_seed_timed(corpus: &Arc<Vec<Doc>>, seed: u64, count: usize, cfg: &Config) -> Option<Repro> {
    let (tx, rx) = std::sync::mpsc::channel();
    let (c, g) = (corpus.clone(), cfg.clone());
    let (at, pending) = std::sync::mpsc::channel();
    let _ = std::thread::Builder::new()
        .name(format!("fuzz-{seed}"))
        .stack_size(64 << 20)
        .spawn(move || {
            let _ = tx.send(run_seed_at(&c, seed, count, &g, Some(&at)));
        });
    if let Ok(r) = rx.recv_timeout(cfg.timeout) {
        return r;
    }
    // (the step it hung at, if it got past its first build)
    if let Some(r) = pending.try_iter().last() {
        return Some(r);
    }
    let doc = corpus.get((seed % corpus.len().max(1) as u64) as usize).map_or_else(String::new, |d| d.name.clone());
    let s = Schedule { threads: 1, reads: Reads::Suspend, order: Order::Priority, lean: true };
    Some(Repro {
        doc,
        seed,
        step: 0,
        before: s,
        schedule: s,
        fail: Fail::Timeout,
        text: String::new(),
        files: Vec::new(),
        edit: Edit { file: None, at: 0, del: 0, ins: String::new(), what: "none".into() },
        threads: cfg.threads.clone(),
        only: cfg.doc.clone(),
    })
}

/// Writes `r` into `dir`; its path.
pub fn save(dir: &Path, r: &Repro) -> PathBuf {
    let _ = std::fs::create_dir_all(dir);
    let p = dir.join(format!("fuzz-{}-{}-{}.repro", r.doc, r.seed, r.step));
    let _ = std::fs::write(&p, r.write());
    p
}

/// Runs a reproduction again: the build of the text before the edit at
/// its schedule, the edit at the failing schedule, against the cold
/// build. The failure, if it still fails.
pub fn replay(r: &Repro) -> Option<Fail> {
    replay_outputs(r)?.0
}

/// [`replay`], with the edit's and the cold build's outputs.
fn replay_outputs(r: &Repro) -> Option<(Option<Fail>, Output, Output)> {
    let doc = find_doc(&r.doc)?;
    let mut text = r.text.clone();
    let mut files = r.files.clone();
    let mut b = builder(&doc, &files);
    b.set_lean(r.before.lean);
    let over = !r.before.lean && !r.schedule.lean;
    let etex = doc.kind.etex();
    let over = over && (!etex || std::env::var_os("PHITEX_FUZZ_LATEX_OVER").is_some());
    let prev = b.update(&text, r.before.options(etex, over));
    r.edit.apply(&mut text, &mut files);
    for (n, t) in &files {
        b.set_file(n, t);
    }
    b.set_lean(r.schedule.lean);
    let inc = b.update(&text, r.schedule.options(etex, over));
    let c = cold(&doc, &text, &files);
    // (the two graphs, for looking at: `PHITEX_FUZZ_DUMP=DIR`)
    if let Some(d) = std::env::var_os("PHITEX_FUZZ_DUMP") {
        write_outputs(&PathBuf::from(d), &prev, &inc, &c);
    }
    if let Some(w) = differs(&inc, &c) {
        return Some((Some(Fail::Mismatch(w)), inc, c));
    }
    if over
        && let Some((allowed, detail)) = over_invalidation(
            &r.text,
            &text,
            r.edit.file.is_some(),
            &prev.dump,
            &inc.dump,
            std::env::var_os("PHITEX_FUZZ_STRICT").is_some(),
        )
    {
        let f = Fail::Over {
            ran: inc.stats.commands_run,
            allowed,
            detail,
        };
        return Some((Some(f), inc, c));
    }
    Some((None, inc, c))
}

/// The first log entry two outputs differ at (the cold build's `b`), a
/// signature: its first 48 characters, control characters as spaces and
/// digits as `#`.
fn first_log_diff(a: &Output, b: &Output) -> String {
    let i = a.log.iter().zip(&b.log).position(|(x, y)| x != y).unwrap_or(a.log.len().min(b.log.len()));
    let e = b.log.get(i).or_else(|| a.log.get(i)).cloned().unwrap_or_default();
    let e: String = e
        .chars()
        .map(|c| if c.is_ascii_digit() { '#' } else if c < ' ' || ('\u{E000}'..='\u{F8FF}').contains(&c) { ' ' } else { c })
        .collect();
    let e = e.split_whitespace().collect::<Vec<_>>().join(" ");
    let short: String = e.chars().take(48).collect();
    if a.dvi == b.dvi { format!("log: {short}") } else { format!("dvi: {short}") }
}

/// A failure's class, to tell findings apart over many seeds: the open
/// findings it matches (CORE.md "Fuzz findings") by how it fails, else a
/// signature (the document, the edit's kind, where the log first differs).
/// - a no-op edit fails too, and passes after a first build in priority
///   order: F6 (the order of the first build);
/// - a no-op edit fails too: F1 on TRIP, else "noop" (F4's kind);
/// - the last edit alone passes, with the seed's history it fails: a
///   "history" failure (F2 is one, its log only);
/// - otherwise "edit" (marked F2? when the log alone differs, at an
///   error's entry).
pub fn classify(r: &Repro) -> String {
    match &r.fail {
        Fail::Timeout => return format!("timeout {}", r.doc),
        Fail::Panic(p) => return format!("panic {} | {}", r.doc, p.lines().next().unwrap_or("").chars().take(60).collect::<String>()),
        Fail::Over { .. } | Fail::Mismatch(_) => {}
    }
    // (an over-invalidation as the last edit alone shows it)
    if let Some((Some(Fail::Over { .. }), ..)) = replay_outputs(r) {
        return format!("over {} {}", r.doc, r.edit.what);
    }
    // (the no-op at 1 thread in priority order, deterministic; else at the
    // failure's schedules)
    let mut quiet = r.noop();
    for s in [&mut quiet.before, &mut quiet.schedule] {
        s.threads = 1;
        s.reads = Reads::Suspend;
        s.order = Order::Priority;
    }
    let noop = if replay(&quiet).is_some() { quiet } else { r.noop() };
    if let Some((Some(_), inc, c)) = replay_outputs(&noop) {
        let mut p = noop.clone();
        p.before = Schedule { threads: 1, reads: Reads::Suspend, order: Order::Priority, lean: r.before.lean };
        if replay(&p).is_none() {
            return format!("F6 order {}", r.doc);
        }
        if r.doc == "trip" {
            return "F1 trip".to_string();
        }
        return format!("noop {} | {}", r.doc, first_log_diff(&inc, &c));
    }
    match replay_outputs(r) {
        Some((Some(_), inc, c)) => {
            let d = first_log_diff(&inc, &c);
            // (an error's entry, the log only: F2's shape)
            let f2 = if d.starts_with("log: ! ") { "F2? " } else { "" };
            format!("{f2}edit {} {} | {d}", r.doc, r.edit.what)
        }
        _ => match replay_history(r) {
            Some(h) => {
                let f2 = matches!(&h.fail, Fail::Mismatch(w) if w == "log");
                format!("{}history {} {} | {}", if f2 { "F2? " } else { "" }, r.doc, r.edit.what, h.fail.text())
            }
            None => format!("passes {}", r.doc),
        },
    }
}

/// A reproduction with its history: its seed run again, the same edits at
/// the same schedules, to its step (a failure that needs what the build
/// went through before the last edit; the schedules of several threads
/// may still order the work otherwise). What fails now, if anything.
pub fn replay_history(r: &Repro) -> Option<Repro> {
    let cfg = Config {
        threads: r.threads.clone(),
        over: true,
        strict: std::env::var_os("PHITEX_FUZZ_STRICT").is_some(),
        timeout: Duration::from_secs(600),
        doc: r.only.clone(),
    };
    let corpus = Arc::new(corpus());
    run_seed_timed(&corpus, r.seed, r.step, &cfg)
}

/// The graphs before and after an edit, and the edit's and the cold
/// build's logs and DVI files, into `d` (`PHITEX_FUZZ_DUMP=DIR`).
fn write_outputs(d: &Path, prev: &Output, inc: &Output, c: &Output) {
    let _ = std::fs::create_dir_all(d);
    let _ = std::fs::write(d.join("before.dump"), &prev.dump);
    let _ = std::fs::write(d.join("after.dump"), &inc.dump);
    let _ = std::fs::write(d.join("edit.log"), inc.log.join("\n"));
    let _ = std::fs::write(d.join("cold.log"), c.log.join("\n"));
    let _ = std::fs::write(d.join("edit.dvi"), &inc.dvi);
    let _ = std::fs::write(d.join("cold.dvi"), &c.dvi);
}

/// An order as the CLI writes it: `priority` or `random:N`.
pub fn parse_order(s: &str) -> Option<Order> {
    if s == "priority" {
        return Some(Order::Priority);
    }
    s.strip_prefix("random:")?.parse().ok().map(Order::Random)
}

/// Whether two cold builds of `text`, as document `doc` is built, at 1
/// worker with waiting reads in orders `a` and `b`, differ (DVI, log,
/// writes); and where first (`differs`' names). `dump`: both graphs.
pub fn orders_differ(doc: &Doc, text: &str, a: Order, b: Order, dump: bool) -> (Option<String>, Output, Output) {
    let o = |order| Options { threads: 1, reads: Reads::Suspend, order, dump, etex: false, ..Options::default() };
    let x = builder(doc, &doc.files).update(text, o(a));
    let y = builder(doc, &doc.files).update(text, o(b));
    (differs(&x, &y), x, y)
}

/// A plain document of the corpus's kind with `body` after plain.tex (and
/// the scaffold), named `name`.
pub fn plain_body_doc(name: &str, body: &str) -> Option<Doc> {
    Some(plain_doc(name, body, &cmr10()?, &plain()?))
}

// ------------------------------------------------------------ LaTeX

fn latex_dir() -> PathBuf {
    root().join("tests/latex")
}

/// A LaTeX document of the corpus (`tests/fuzz/latex/NAME.tex`, or B's
/// `tests/latex/docs/NAME.tex`), named `latex-NAME`. The directories are
/// those `latex` read B's documents' files from (their `.dirs`, and
/// `tests/latex/dirs.txt` for the format's); `None` without `latex.ini`.
pub fn latex_doc(name: &str) -> Option<Doc> {
    let tex = [root().join(format!("tests/fuzz/latex/{name}.tex")), latex_dir().join(format!("docs/{name}.tex"))]
        .into_iter()
        .find(|p| p.exists())?;
    let mut ini = None;
    let (mut inputs, mut fonts) = (vec![latex_dir()], Vec::new());
    let add = |v: &mut Vec<PathBuf>, d: PathBuf| {
        if !v.contains(&d) {
            v.push(d);
        }
    };
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(latex_dir().join("docs")).ok()?.filter_map(|e| e.ok().map(|e| e.path())).collect();
    dirs.sort();
    for p in dirs.iter().filter(|p| p.extension().is_some_and(|x| x == "dirs")) {
        for l in std::fs::read_to_string(p).ok()?.lines() {
            match l.split_once(' ') {
                Some(("ini", p)) => ini = ini.or_else(|| Some(PathBuf::from(p))),
                Some(("input", d)) => add(&mut inputs, PathBuf::from(d)),
                Some(("font", d)) => add(&mut fonts, PathBuf::from(d)),
                _ => {}
            }
        }
    }
    for d in std::fs::read_to_string(latex_dir().join("dirs.txt")).ok()?.lines() {
        add(&mut inputs, PathBuf::from(d));
    }
    for d in crate::tfm::default_dirs() {
        add(&mut fonts, d);
    }
    let ini = latin1(&std::fs::read(ini?).ok()?);
    let doc = latin1(&std::fs::read(&tex).ok()?);
    let text = crate::graph::jobs(&[&ini, &doc]);
    let body = text.rfind(crate::graph::JOB).map_or(0, |i| i + crate::graph::JOB.len_utf8());
    Some(Doc {
        name: format!("latex-{name}"),
        kind: Kind::Latex(Arc::new(LatexSetup { jobname: name.to_string(), input_dirs: inputs, font_dirs: fonts })),
        text,
        body,
        files: Vec::new(),
        font: cmr10()?,
    })
}

/// The LaTeX documents: the small one the harness is made on, and B's.
pub fn latex_corpus() -> Vec<Doc> {
    ["tiny", "sections", "math", "tables", "figures", "titlepage", "packages"].iter().filter_map(|n| latex_doc(n)).collect()
}

/// A document of either corpus by name.
pub fn find_doc(name: &str) -> Option<Doc> {
    if let Some(n) = name.strip_prefix("latex-") {
        return latex_doc(n);
    }
    corpus().into_iter().find(|d| d.name == name)
}

/// The text from `at` to the end of its line (the `\n` included).
fn line_at(text: &str, at: usize) -> (usize, usize) {
    let s = text[..at].rfind('\n').map_or(0, |i| i + 1);
    let e = text[at..].find('\n').map_or(text.len(), |i| at + i + 1);
    (s, e)
}

/// One random LaTeX edit (`None`: nothing of the kind drawn to edit). The
/// document's preamble and body are after `doc.body`.
fn edit_latex(rng: &mut Rng, doc: &Doc, text: &str) -> Option<Edit> {
    let main = |at: usize, del: usize, ins: String, what: &str| Edit { file: None, at, del, ins, what: what.to_string() };
    let from = doc.body;
    let begin = from + text[from..].find("\\begin{document}")? + "\\begin{document}\n".len();
    let end = from + text[from..].find("\\end{document}")?;
    let all = |pat: &str| -> Vec<usize> { text[begin..end].match_indices(pat).map(|(i, _)| begin + i).collect() };
    // (lines of running text: starting with a letter)
    let mut text_lines = Vec::new();
    let mut at = begin;
    for l in text[begin..end].split_inclusive('\n') {
        if l.chars().next().is_some_and(char::is_alphabetic) {
            text_lines.push(at);
        }
        at += l.len();
    }
    let spaces: Vec<usize> =
        text_lines.iter().flat_map(|&l| text[l..line_at(text, l).1].match_indices(' ').map(move |(i, _)| l + i)).collect();
    // (paragraph starts after blank lines in the body)
    let blanks = all("\n\n").into_iter().map(|i| i + 2).filter(|&i| i < end).collect::<Vec<_>>();
    let kinds = [
        "word", "para", "section", "label", "ref", "emph", "unemph", "item-add", "item-remove", "equation", "cell", "float",
        "package", "class", "newcommand", "comment", "blank-in", "blank-out",
    ];
    let kind = rng.pick(&kinds)?;
    let w = rng.pick(&WORDS)?;
    match kind {
        "word" => {
            let at = rng.pick(&spaces)?;
            Some(main(at, 0, format!(" {w}"), kind))
        }
        "para" => {
            let at = rng.pick(&blanks)?;
            Some(main(at, 0, format!("An inserted paragraph about the {w} and its words.\n\n"), kind))
        }
        "section" => {
            let at = rng.pick(&all("\\section{"))? + "\\section{".len();
            let n = text[at..].find('}')?;
            Some(main(at, n, format!("The {w}"), kind))
        }
        "label" => {
            // (a \label moved to another paragraph)
            let at = rng.pick(&all("\\label{"))?;
            let n = text[at..].find('}')? + 1;
            let to = rng.pick(&blanks)?;
            let label = text[at..at + n].to_string();
            if to >= at && to <= at + n {
                return None;
            }
            // (one edit: the span from the first place to the last, rewritten)
            let (a, b) = (at.min(to), (at + n).max(to));
            let mut span = text[a..b].to_string();
            if to < at {
                span = format!("{label}{}", &span[..at - a]) + &span[at + n - a..];
            } else {
                span = format!("{}{}{label}", &span[..0], &span[n..]);
            }
            Some(main(a, b - a, span, "label-move"))
        }
        "ref" => {
            let at = rng.pick(&all("\\ref{"))? + "\\ref{".len();
            let n = text[at..].find('}')?;
            let labels: Vec<String> = all("\\label{")
                .into_iter()
                .filter_map(|l| {
                    let s = l + "\\label{".len();
                    text[s..].find('}').map(|e| text[s..s + e].to_string())
                })
                .collect();
            let t = rng.pick(&labels.iter().map(String::as_str).collect::<Vec<_>>())?.to_string();
            (text[at..at + n] != t).then(|| main(at, n, t, "ref-target"))
        }
        "emph" => {
            let at = rng.pick(&spaces)? + 1;
            let n = text[at..].find([' ', '\n'])?;
            let word = &text[at..at + n];
            if word.is_empty() || !word.chars().all(char::is_alphabetic) {
                return None;
            }
            let cmd = if rng.below(2) == 0 { "emph" } else { "textbf" };
            Some(main(at, n, format!("\\{cmd}{{{word}}}"), "emph-wrap"))
        }
        "unemph" => {
            let mut v = all("\\emph{");
            v.extend(all("\\textbf{"));
            let at = rng.pick(&v)?;
            let open = at + text[at..].find('{')? + 1;
            let n = text[open..].find('}')?;
            Some(main(at, open + n + 1 - at, text[open..open + n].to_string(), "emph-remove"))
        }
        "item-add" => {
            let at = rng.pick(&all("\\item "))?;
            Some(main(at, 0, format!("\\item An added {w}.\n"), kind))
        }
        "item-remove" => {
            let at = rng.pick(&all("\\item "))?;
            let (_, e) = line_at(text, at);
            Some(main(at, e - at, String::new(), kind))
        }
        "equation" => {
            let at = rng.pick(&all("\\end{equation}"))?;
            Some(main(at, 0, format!("+{}\n", letter(rng)), kind))
        }
        "cell" => {
            let at = rng.pick(&all("& "))? + 2;
            let n = text[at..].find([' ', '\\', '&', '\n'])?;
            Some(main(at, n, w.to_string(), kind))
        }
        "float" => {
            let mut v = all("\\begin{figure}");
            v.extend(all("\\begin{table}"));
            let a = rng.pick(&v)?;
            let env = if text[a..].starts_with("\\begin{figure}") { "\\end{figure}\n" } else { "\\end{table}\n" };
            let b = a + text[a..].find(env)? + env.len();
            let to = rng.pick(&blanks)?;
            if to >= a && to <= b {
                return None;
            }
            let float = text[a..b].to_string();
            let (s, e) = (a.min(to), b.max(to));
            let span = if to < a {
                format!("{float}{}", &text[to..a]) + &text[b..b]
            } else {
                format!("{}{float}", &text[b..to])
            };
            Some(main(s, e - s, span, "float-move"))
        }
        "package" => {
            let pre = text[from..begin].find("\\begin{document}").map(|i| from + i)?;
            let have: Vec<usize> = ["ifthen", "calc", "array", "verbatim"]
                .iter()
                .filter_map(|p| text[from..pre].find(&format!("\\usepackage{{{p}}}\n")).map(|i| from + i))
                .collect();
            if rng.below(2) == 0 && !have.is_empty() {
                let at = rng.pick(&have)?;
                let (_, e) = line_at(text, at);
                Some(main(at, e - at, String::new(), "package-remove"))
            } else {
                let p = rng.pick(&["ifthen", "calc", "array", "verbatim"])?;
                if text[from..pre].contains(&format!("\\usepackage{{{p}}}")) {
                    return None;
                }
                Some(main(pre, 0, format!("\\usepackage{{{p}}}\n"), "package-add"))
            }
        }
        "class" => {
            let at = from + text[from..].find("\\documentclass")?;
            let (_, e) = line_at(text, at);
            let line = &text[at..e];
            let opts = ["", "[11pt]", "[12pt]", "[twocolumn]"];
            let o = rng.pick(&opts)?;
            let cls = &line[line.find('{')?..];
            let new = format!("\\documentclass{o}{cls}");
            (new != line).then(|| main(at, e - at, new, "class-option"))
        }
        "newcommand" => {
            let at = from + text[from..].find("\\newcommand{")?;
            let close = at + text[at..].find("}{")? + 2;
            let n = text[close..].find('}')?;
            Some(main(close, n, w.to_string(), kind))
        }
        "comment" => {
            let at = rng.pick(&text_lines)?;
            Some(main(at, 0, format!("% a comment about the {w}\n"), kind))
        }
        "blank-in" | "blank-out" => {
            // (a line start inside an environment, or outside every one)
            let mut depth = 0i32;
            let mut inside = Vec::new();
            let mut outside = Vec::new();
            let mut at = begin;
            for l in text[begin..end].split_inclusive('\n') {
                if depth > 0 { inside.push(at) } else { outside.push(at) }
                depth += l.matches("\\begin{").count() as i32 - l.matches("\\end{").count() as i32;
                at += l.len();
            }
            let at = rng.pick(if kind == "blank-in" { &inside } else { &outside })?;
            Some(main(at, 0, "\n".into(), kind))
        }
        _ => None,
    }
}
