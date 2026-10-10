//! SyncTeX through phitex, with TeX Live's `synctex` (its parser) as the
//! oracle: on phitex-flow's ten LaTeX documents (`pdflatex` and `latex`,
//! PDF and DVI) and five plain documents (`pdftex -ini -etex`), each built
//! with `-synctex=1` by phitex and by TeX Live's own program, both files
//! are asked the same questions:
//!
//! - forward (`synctex view -i LINE:0:NAME.tex`): for every line pdfTeX's
//!   file answers, phitex's first answer is on a page pdfTeX gives and
//!   within one line box of one of its boxes;
//! - inverse (`synctex edit -o PAGE:X:Y`): at points along every line box
//!   of pdfTeX's file, phitex's file names the same input and a line
//!   within one of pdfTeX's — or pdfTeX's names its line box's own line
//!   (pdfTeX's first word of a line takes the line of the paragraph's end:
//!   its characters have no place of their own), counted apart;
//! - positions: every `x` record of pdfTeX's (where a run of characters
//!   ends) is within 1 sp of one of phitex's on the same page.
//!
//! The tallies are printed. Inverse-search points whose pdfTeX line is
//! inside an alignment (`\halign`/`\valign`: a tabular, array, eqnarray,
//! matrix or cases cell) are counted apart: phitex-flow's alignment cells
//! carry no origins yet (CORE.md, "Source places and item provenance"),
//! so their characters have the place of the node that ended the
//! alignment. Their number may not grow ([`ALIGNMENT_POINTS`]); the bounds
//! apply to the rest. Skipped where `synctex` is not on the path.
//! Then `phitex watch` (headless) on the 48-page plain document: a word
//! edit's SyncTeX file is the cold build's of the edited text, made
//! walking the remade pages only, within the word budget.

#![allow(
    clippy::doc_markdown,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_possible_wrap,
    clippy::many_single_char_names,
    clippy::absurd_extreme_comparisons,
    reason = "a test's arithmetic on positions in points and scaled points; ALIGNMENT_POINTS goes to 0"
)]

use std::io::{BufRead as _, BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::Duration;

static ONE: Mutex<()> = Mutex::new(());

fn one() -> std::sync::MutexGuard<'static, ()> {
    ONE.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn flow() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../phitex-flow")
}

fn cache() -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("phitex-cache")
}

fn scratch(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("synctex").join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("a scratch directory");
    d
}

/// The binary under the program name `name` (a link to it).
fn as_program(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("bin-synctex");
    let _ = std::fs::create_dir_all(&dir);
    let p = dir.join(name);
    let _ = std::fs::remove_file(&p);
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_phitex"), &p).expect("a link");
    p
}

/// `program` in `dir`, stopped after ten minutes.
fn command(program: &Path, dir: &Path) -> Command {
    let mut c = Command::new("timeout");
    c.arg("600").arg(program);
    c.current_dir(dir)
        .env("PHITEX_CACHE_DIR", cache())
        .env("SOURCE_DATE_EPOCH", "0")
        .env("FORCE_SOURCE_DATE", "1")
        .env_remove("PHITEX_COMPAT");
    c
}

/// Whether `program` is there (it says something when run with `arg`).
fn have(program: &str, arg: &str) -> bool {
    Command::new(program).arg(arg).output().is_ok_and(|o| !o.stdout.is_empty() || !o.stderr.is_empty())
}

fn copy_doc(name: &str, dir: &Path) {
    let pre = format!("{name}-");
    for e in std::fs::read_dir(flow().join("tests/latex/docs")).expect("the documents").flatten() {
        let f = e.file_name().to_string_lossy().into_owned();
        if f == format!("{name}.tex") || f.starts_with(&pre) {
            std::fs::copy(e.path(), dir.join(&f)).expect("a copy");
        }
    }
}

/// A gzip file's members, decoded one after the other.
fn gunzip(b: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut p = 0;
    while p + 10 < b.len() {
        assert_eq!(&b[p..p + 3], &[0x1f, 0x8b, 8], "a gzip member at {p}");
        let used = phitex_codecs::inflate::inflate_raw_len(&b[p + 10..], &mut out).expect("a deflate stream");
        p += 10 + used + 8;
    }
    out
}

/// A record of a SyncTeX file: its page, kind, tag, line, point and
/// dimensions.
#[derive(Clone, Copy, Debug)]
struct Rec {
    page: usize,
    kind: u8,
    line: i64,
    h: i64,
    v: i64,
    w: i64,
    ht: i64,
    dp: i64,
}

/// A SyncTeX file's records and offsets.
fn records(text: &str) -> (Vec<Rec>, i64, i64) {
    let mut out = Vec::new();
    let (mut page, mut xo, mut yo, mut last_v) = (0, 0, 0, 0);
    let num = |s: &str| s.trim().parse::<i64>().unwrap_or(0);
    for l in text.lines() {
        if let Some(v) = l.strip_prefix("X Offset:") {
            xo = num(v);
        } else if let Some(v) = l.strip_prefix("Y Offset:") {
            yo = num(v);
        } else if let Some(k) = l.strip_prefix('{') {
            page = usize::try_from(num(k)).unwrap_or(0);
        } else if let Some(&kind) = l.as_bytes().first()
            && b"([vhxkg$r".contains(&kind)
            && page > 0
        {
            let mut parts = l[1..].split(':');
            let (Some(tl), Some(hv)) = (parts.next(), parts.next()) else { continue };
            let (Some((_, line)), Some((h, v))) = (tl.split_once(','), hv.split_once(',')) else { continue };
            let v = if v == "=" { last_v } else { num(v) };
            last_v = v;
            let d: Vec<i64> = parts.next().map(|d| d.split(',').map(num).collect()).unwrap_or_default();
            out.push(Rec {
                page,
                kind,
                line: num(line),
                h: num(h),
                v,
                w: d.first().copied().unwrap_or(0),
                ht: d.get(1).copied().unwrap_or(0),
                dp: d.get(2).copied().unwrap_or(0),
            });
        }
    }
    (out, xo, yo)
}

/// `synctex view`'s answers: page, box bottom `v`, height `H` (big points).
fn view(dir: &Path, out: &str, input: &str, line: usize) -> Vec<(usize, f64, f64)> {
    let o = Command::new("synctex")
        .args(["view", "-i", &format!("{line}:0:{input}"), "-o", out])
        .current_dir(dir)
        .output()
        .expect("synctex runs");
    let text = String::from_utf8_lossy(&o.stdout);
    let mut v = Vec::new();
    let (mut page, mut vv) = (0usize, 0f64);
    for l in text.lines() {
        if let Some(p) = l.strip_prefix("Page:") {
            page = p.trim().parse().unwrap_or(0);
        } else if let Some(x) = l.strip_prefix("v:") {
            vv = x.trim().parse().unwrap_or(0.0);
        } else if let Some(x) = l.strip_prefix("H:") {
            v.push((page, vv, x.trim().parse().unwrap_or(0.0)));
        }
    }
    v
}

/// `synctex edit`'s first answer: the input's base name and the line.
fn edit(dir: &Path, out: &str, page: usize, x: f64, y: f64) -> Option<(String, i64)> {
    let o = Command::new("synctex")
        .args(["edit", "-o", &format!("{page}:{x:.3}:{y:.3}:{out}")])
        .current_dir(dir)
        .output()
        .expect("synctex runs");
    let text = String::from_utf8_lossy(&o.stdout);
    let input = text.lines().find_map(|l| l.strip_prefix("Input:"))?;
    let line = text.lines().find_map(|l| l.strip_prefix("Line:"))?.trim().parse().ok()?;
    Some((input.rsplit('/').next().unwrap_or(input).to_owned(), line))
}

#[derive(Default, Debug)]
struct Tally {
    fwd: usize,
    fwd_ok: usize,
    /// Lines all of whose boxes in phitex's file are near pdfTeX's.
    fwd_all: usize,
    fwd_bad: Vec<String>,
    inv: usize,
    inv_same: usize,
    inv_one: usize,
    inv_box: usize,
    /// Points whose pdfTeX line is inside an alignment (counted apart).
    inv_align: usize,
    inv_bad: Vec<String>,
    x: usize,
    x_ok: usize,
}

impl Tally {
    fn add(&mut self, o: &Tally) {
        self.fwd += o.fwd;
        self.fwd_ok += o.fwd_ok;
        self.fwd_all += o.fwd_all;
        self.inv += o.inv;
        self.inv_same += o.inv_same;
        self.inv_one += o.inv_one;
        self.inv_box += o.inv_box;
        self.inv_align += o.inv_align;
        self.x += o.x;
        self.x_ok += o.x_ok;
        self.fwd_bad.extend(o.fwd_bad.iter().take(3).cloned());
        self.inv_bad.extend(o.inv_bad.iter().take(3).cloned());
    }

    fn line(&self, name: &str) -> String {
        format!(
            "{name:<22} forward {}/{} within a box ({} with every box so); inverse {} points: {} same line, {} within one, {} pdfTeX's line-box line, {} other ({} in alignments apart); x records {}/{} within 1 sp",
            self.fwd_ok,
            self.fwd,
            self.fwd_all,
            self.inv,
            self.inv_same,
            self.inv_one,
            self.inv_box,
            self.inv - self.inv_same - self.inv_one - self.inv_box,
            self.inv_align,
            self.x_ok,
            self.x
        )
    }
}

/// Document `name` (`NAME.tex` in `ours` and `theirs`, built there), the
/// output `ext`, forward search over lines `lines`: the comparison.
fn compare(name: &str, ext: &str, ours: &Path, theirs: &Path, lines: std::ops::RangeInclusive<usize>) -> Tally {
    let out = format!("{name}.{ext}");
    let input = format!("{name}.tex");
    let mut t = Tally::default();
    let read = |d: &Path| String::from_utf8_lossy(&gunzip(&std::fs::read(d.join(format!("{name}.synctex.gz"))).unwrap_or_default())).into_owned();
    let (mine, theirs_text) = (read(ours), read(theirs));
    assert!(!mine.is_empty(), "{name}.{ext}: phitex wrote no SyncTeX file in {}", ours.display());
    assert!(!theirs_text.is_empty(), "{name}.{ext}: TeX Live wrote no SyncTeX file in {}", theirs.display());
    // (forward)
    for l in lines {
        let want = view(theirs, &out, &input, l);
        if want.is_empty() {
            continue;
        }
        t.fwd += 1;
        let got = view(ours, &out, &input, l);
        // (synctex's answers come in no particular order: one of phitex's
        // boxes is one of pdfTeX's, or the line box next to it)
        let near = |g: &(usize, f64, f64), w: &(usize, f64, f64)| w.0 == g.0 && (w.1 - g.1).abs() <= 1.5 * w.2.max(g.2).max(10.0);
        let ok = got.iter().any(|g| want.iter().any(|w| near(g, w)));
        if got.iter().all(|g| want.iter().any(|w| near(g, w))) && !got.is_empty() {
            t.fwd_all += 1;
        }
        if ok {
            t.fwd_ok += 1;
        } else {
            t.fwd_bad.push(format!("{name}.{ext} line {l}: pdfTeX {want:?}, phitex {got:?}"));
        }
    }
    // (inverse: points along pdfTeX's line boxes, at most about 150)
    let aligned = alignment_lines(&std::fs::read_to_string(theirs.join(&input)).unwrap_or_default());
    let (recs, xo, yo) = records(&theirs_text);
    let boxes: Vec<&Rec> = recs.iter().filter(|r| r.kind == b'(' && r.w > 0 && r.ht + r.dp > 0).collect();
    let step = (boxes.len() * 3).div_ceil(150).max(1);
    let bp = |sp: i64, off: i64| (sp + off) as f64 / 65_781.76;
    let mut k = 0;
    for b in &boxes {
        for f in [0.15, 0.5, 0.85] {
            k += 1;
            if k % step != 0 {
                continue;
            }
            let x = bp(b.h + (b.w as f64 * f) as i64, xo);
            let y = bp(b.v - b.ht / 2, yo);
            let Some(want) = edit(theirs, &out, b.page, x, y) else { continue };
            if want.0 == input && usize::try_from(want.1).is_ok_and(|l| aligned.contains(&l)) {
                t.inv_align += 1;
                continue;
            }
            t.inv += 1;
            let got = edit(ours, &out, b.page, x, y);
            match &got {
                Some(g) if g.0 == want.0 && g.1 == want.1 => t.inv_same += 1,
                Some(g) if g.0 == want.0 && (g.1 - want.1).abs() <= 1 => t.inv_one += 1,
                Some(g) if g.0 == want.0 && want.1 == b.line => t.inv_box += 1,
                _ => t.inv_bad.push(format!("{name}.{ext} page {} at ({x:.1}, {y:.1}): pdfTeX {want:?}, phitex {got:?}", b.page)),
            }
        }
    }
    // (positions: pdfTeX's ends of runs of characters among phitex's records)
    let (mine_recs, _, _) = records(&mine);
    let mut at: std::collections::HashSet<(usize, i64, i64)> = std::collections::HashSet::new();
    for r in mine_recs.iter().filter(|r| r.kind == b'x') {
        for dh in -1..=1 {
            for dv in -1..=1 {
                at.insert((r.page, r.h + dh, r.v + dv));
            }
        }
    }
    for r in recs.iter().filter(|r| r.kind == b'x') {
        t.x += 1;
        if at.contains(&(r.page, r.h, r.v)) {
            t.x_ok += 1;
        }
    }
    t
}

/// Runs `program` (phitex as it, or TeX Live's) on `file` in `dir` with
/// `-synctex=1`.
fn run(program: &Path, dir: &Path, pre: &[&str], file: &str) {
    let mut args: Vec<&str> = pre.to_vec();
    args.extend(["-synctex=1", "-interaction=batchmode", file]);
    let o = command(program, dir).args(&args).output().expect("it runs");
    if !o.status.success() {
        eprintln!("{} {args:?} in {}: status {:?}", program.display(), dir.display(), o.status.code());
    }
}

/// The inverse-search points inside alignments (all fifteen documents,
/// both outputs): may not grow; 0 once alignment cells carry origins.
const ALIGNMENT_POINTS: usize = 728;

/// The lines (from 1) of `text` inside an alignment: a LaTeX environment
/// set by `\halign` (`tabular`, `array`, `eqnarray`, amsmath's matrices,
/// `cases`, `align`, `gather`…), or the braced argument of plain's
/// `\halign`, `\valign`, `\matrix`, `\pmatrix`, `\cases`, `\eqalign`…, or
/// a `\+` line.
fn alignment_lines(text: &str) -> std::collections::HashSet<usize> {
    const ENVS: [&str; 18] = [
        "tabular", "tabular*", "array", "eqnarray", "eqnarray*", "align", "align*", "alignat", "gather", "gather*",
        "matrix", "pmatrix", "bmatrix", "vmatrix", "Vmatrix", "cases", "aligned", "split",
    ];
    const MACROS: [&str; 10] =
        ["\\halign", "\\valign", "\\matrix", "\\pmatrix", "\\bordermatrix", "\\cases", "\\eqalignno", "\\leqalignno", "\\eqalign", "\\displaylines"];
    let mut out = std::collections::HashSet::new();
    let lines: Vec<&str> = text.lines().collect();
    let mut open: Vec<&str> = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        let code = l.split('%').next().unwrap_or("");
        let starts_inside = !open.is_empty();
        for e in ENVS {
            if code.contains(&format!("\\begin{{{e}}}")) {
                open.push(e);
            }
        }
        if !open.is_empty() || starts_inside || code.trim_start().starts_with("\\+") {
            out.insert(i + 1);
        }
        open.retain(|e| !code.contains(&format!("\\end{{{e}}}")));
    }
    // (plain's: from the macro to its argument's closing brace)
    let flat: Vec<(usize, char)> =
        lines.iter().enumerate().flat_map(|(i, l)| l.split('%').next().unwrap_or("").chars().chain(['\n']).map(move |c| (i + 1, c))).collect();
    let chars: String = flat.iter().map(|x| x.1).collect();
    for m in MACROS {
        let mut from = 0;
        while let Some(at) = chars[from..].find(m).map(|x| x + from) {
            from = at + m.len();
            // (a whole name: `\matrix` not `\matrixx`)
            if chars[from..].chars().next().is_some_and(char::is_alphabetic) {
                continue;
            }
            let Some(b) = chars[from..].find('{').map(|x| x + from) else { break };
            let mut depth = 0i32;
            let start = chars[..at].chars().count();
            for (k, c) in chars[b..].char_indices() {
                match c {
                    '{' => depth += 1,
                    '}' => depth -= 1,
                    _ => {}
                }
                if depth == 0 {
                    let end = chars[..b + k].chars().count();
                    for x in &flat[start..=end.min(flat.len() - 1)] {
                        out.insert(x.0);
                    }
                    break;
                }
            }
        }
    }
    out
}

const LATEX: [&str; 10] = ["sections", "math", "tables", "figures", "titlepage", "packages", "ams", "gfx", "layout", "rules"];

/// phitex's SyncTeX files against TeX Live's (the module's comparison).
#[test]
fn synctex_against_pdftex() {
    if !have("synctex", "help") || !have("pdftex", "--version") {
        eprintln!("synctex_against_pdftex: no `synctex` or `pdftex` here: not checked");
        return;
    }
    let _one = one();
    let mut all = Tally::default();
    let mut lines = Vec::new();
    for (program, ext) in [("pdflatex", "pdf"), ("latex", "dvi")] {
        for name in LATEX {
            let ours = scratch(&format!("{program}-{name}"));
            let theirs = scratch(&format!("{program}-{name}-texlive"));
            copy_doc(name, &ours);
            copy_doc(name, &theirs);
            let file = format!("{name}.tex");
            run(&as_program(program), &ours, &[], &file);
            run(Path::new(program), &theirs, &[], &file);
            let n = std::fs::read_to_string(ours.join(&file)).expect("the document").lines().count();
            let t = compare(name, ext, &ours, &theirs, 1..=n);
            lines.push(t.line(&format!("{program} {name}")));
            all.add(&t);
        }
    }
    let plain = phitex_flow::fuzz::plain().expect("plain.tex of phitex-flow's tests");
    for body in ["story", "pages", "display", "table", "hyphens"] {
        let b = std::fs::read_to_string(flow().join(format!("tests/plain/{body}.tex"))).expect("a body");
        let head = "\\pdfoutput=1\n".to_string() + &plain;
        let text = head.clone() + &b;
        let ours = scratch(&format!("pdftex-{body}"));
        let theirs = scratch(&format!("pdftex-{body}-texlive"));
        let file = format!("{body}.tex");
        std::fs::write(ours.join(&file), &text).expect("the document");
        std::fs::write(theirs.join(&file), &text).expect("the document");
        run(&as_program("pdftex"), &ours, &["-ini", "-etex"], &file);
        run(Path::new("pdftex"), &theirs, &["-ini", "-etex"], &file);
        let first = head.lines().count() + 1;
        let t = compare(body, "pdf", &ours, &theirs, first..=first + b.lines().count());
        lines.push(t.line(&format!("pdftex {body}")));
        all.add(&t);
    }
    lines.push(all.line("all"));
    eprintln!("SyncTeX, phitex against TeX Live (synctex's answers):\n{}", lines.join("\n"));
    for b in all.fwd_bad.iter().take(30).chain(all.inv_bad.iter().take(40)) {
        eprintln!("  {b}");
    }
    let pct = |a: usize, b: usize| if b == 0 { 100.0 } else { 100.0 * a as f64 / b as f64 };
    assert!(all.fwd > 0 && all.inv > 0 && all.x > 0, "nothing compared: {all:?}");
    let fwd = pct(all.fwd_ok, all.fwd);
    let inv = pct(all.inv_same + all.inv_one + all.inv_box, all.inv);
    let x = pct(all.x_ok, all.x);
    eprintln!("forward {fwd:.1}%, inverse {inv:.1}% (same or within one line, or pdfTeX's line-box line), positions {x:.1}%");
    eprintln!("{} alignment points skipped: flow's alignment cells have no origins", all.inv_align);
    assert!(
        all.inv_align <= ALIGNMENT_POINTS,
        "{} inverse-search points inside alignments, more than the {ALIGNMENT_POINTS} known",
        all.inv_align
    );
    assert!(fwd >= 95.0, "forward search: {fwd:.1}% within a box of pdfTeX's");
    assert!(inv >= 95.0, "inverse search: {inv:.1}% within a line of pdfTeX's (alignments apart)");
    assert!(x >= 99.0, "positions: {x:.1}% of pdfTeX's x records within 1 sp of phitex's");
}

/// The LaTeX documents xelatex makes as phitex does (gfx's `dvips` driver
/// apart), and fontspec's of the XeTeX documents.
const XELATEX: [&str; 10] = ["sections", "math", "tables", "figures", "titlepage", "packages", "ams", "layout", "rules", "fontspec"];

/// phitex's SyncTeX files against xelatex's, as against pdfTeX's above
/// (job 8065: forward 98.5%, inverse 98.6%, positions 100%). Both files
/// stay in the scratch directories.
#[test]
fn synctex_against_xetex() {
    if !have("synctex", "help") || !have("xelatex", "--version") {
        eprintln!("synctex_against_xetex: no `synctex` or `xelatex` here: not checked");
        return;
    }
    let _one = one();
    let mut all = Tally::default();
    let mut lines = Vec::new();
    for name in XELATEX {
        let ours = scratch(&format!("xelatex-{name}"));
        let theirs = scratch(&format!("xelatex-{name}-texlive"));
        for d in [&ours, &theirs] {
            if name == "fontspec" {
                std::fs::copy(flow().join("tests/xetex/docs/fontspec.tex"), d.join("fontspec.tex")).expect("a copy");
            } else {
                copy_doc(name, d);
            }
        }
        let file = format!("{name}.tex");
        run(&as_program("xelatex"), &ours, &[], &file);
        run(Path::new("xelatex"), &theirs, &[], &file);
        let n = std::fs::read_to_string(ours.join(&file)).expect("the document").lines().count();
        let t = compare(name, "pdf", &ours, &theirs, 1..=n);
        lines.push(t.line(&format!("xelatex {name}")));
        all.add(&t);
    }
    lines.push(all.line("all"));
    eprintln!("SyncTeX, phitex against xelatex (synctex's answers):\n{}", lines.join("\n"));
    for b in all.fwd_bad.iter().take(30).chain(all.inv_bad.iter().take(40)) {
        eprintln!("  {b}");
    }
    let pct = |a: usize, b: usize| if b == 0 { 100.0 } else { 100.0 * a as f64 / b as f64 };
    assert!(all.fwd > 0 && all.inv > 0, "nothing compared: {all:?}");
    let fwd = pct(all.fwd_ok, all.fwd);
    let inv = pct(all.inv_same + all.inv_one + all.inv_box, all.inv);
    let x = pct(all.x_ok, all.x);
    eprintln!("forward {fwd:.1}%, inverse {inv:.1}%, positions {x:.1}%");
    assert!(fwd >= 95.0, "forward search: {fwd:.1}% within a box of xelatex's");
    assert!(inv >= 95.0, "inverse search: {inv:.1}% within a line of xelatex's (alignments apart)");
    assert!(x >= 99.0, "positions: {x:.1}% of xelatex's x records within 1 sp of phitex's");
}

/// `phitex sync --pdf` and `--tex` answer as `synctex edit` and `synctex
/// view` do on the same file.
#[test]
fn phitex_sync_answers_as_synctex() {
    if !have("synctex", "help") {
        eprintln!("phitex_sync_answers_as_synctex: no `synctex` here: not checked");
        return;
    }
    let _one = one();
    let dir = scratch("sync-cli");
    copy_doc("sections", &dir);
    run(&as_program("pdflatex"), &dir, &[], "sections.tex");
    assert!(dir.join("sections.synctex.gz").is_file(), "no SyncTeX file in {}", dir.display());
    let text = String::from_utf8_lossy(&gunzip(&std::fs::read(dir.join("sections.synctex.gz")).expect("the file"))).into_owned();
    let (recs, xo, yo) = records(&text);
    let ours = |args: &[&str]| {
        let o = command(Path::new(env!("CARGO_BIN_EXE_phitex")), &dir).arg("sync").args(args).output().expect("phitex sync runs");
        String::from_utf8_lossy(&o.stdout).into_owned()
    };
    let (mut same, mut n) = (0, 0);
    for b in recs.iter().filter(|r| r.kind == b'(' && r.w > 0).take(40) {
        let x = (b.h + b.w / 3 + xo) as f64 / 65_781.76;
        let y = (b.v - b.ht / 2 + yo) as f64 / 65_781.76;
        let want = edit(&dir, "sections.pdf", b.page, x, y).map(|w| w.1);
        let got = ours(&["--pdf", &b.page.to_string(), &format!("{x:.3}"), &format!("{y:.3}"), "sections.tex"]);
        let got: Option<i64> = got.lines().find_map(|l| l.strip_prefix("Line:")).and_then(|l| l.trim().parse().ok());
        n += 1;
        if want.is_some() && got == want {
            same += 1;
        }
    }
    let (mut fsame, mut fn_) = (0, 0);
    for l in 4..=27 {
        let want = view(&dir, "sections.pdf", "sections.tex", l);
        let got = ours(&["--tex", "sections.tex", &l.to_string(), "sections.tex"]);
        let page: Option<usize> = got.lines().find_map(|x| x.strip_prefix("Page:")).and_then(|p| p.trim().parse().ok());
        let v: Option<f64> = got.lines().find_map(|x| x.strip_prefix("v:")).and_then(|p| p.trim().parse().ok());
        fn_ += 1;
        // (synctex's answers come in no particular order: phitex's first is one of them)
        if want.iter().any(|w| Some(w.0) == page && v.is_some_and(|v| (v - w.1).abs() < 0.01)) {
            fsame += 1;
        }
    }
    eprintln!("phitex sync: --pdf {same}/{n} the line synctex edit gives; --tex {fsame}/{fn_} synctex view's first box");
    assert!(same * 10 >= n * 9, "phitex sync --pdf: {same}/{n} as synctex edit");
    assert!(fsame * 10 >= fn_ * 9, "phitex sync --tex: {fsame}/{fn_} as synctex view");
}

/// Reads `child`'s lines until one starts with `prefix`.
fn until(lines: &mut impl Iterator<Item = String>, prefix: &str, seen: &mut Vec<String>) -> String {
    for l in lines.by_ref() {
        seen.push(l.clone());
        if l.starts_with(prefix) {
            return l;
        }
    }
    panic!("the watch ended before `{prefix}`:\n{}", seen.join("\n"));
}

/// The number after `key` in `line`.
fn after(line: &str, key: &str) -> f64 {
    let at = line.find(key).unwrap_or_else(|| panic!("no `{key}` in {line}")) + key.len();
    line[at..].split([' ', ';', ')']).next().and_then(|n| n.parse().ok()).unwrap_or_else(|| panic!("a number after `{key}` in {line}"))
}

/// `phitex watch` (headless) on the 48-page plain document: after a word
/// edit the SyncTeX file is a cold build's of the edited text (but for
/// the directory in its input names), and making it walked only the
/// remade pages; the update and the SyncTeX file each within the word
/// budget (50 ms) in a release build.
#[test]
fn watch_synctex_word_edit() {
    let _one = one();
    let plain = phitex_flow::fuzz::plain().expect("plain.tex of phitex-flow's tests");
    let text = "\\pdfoutput=1\n".to_string() + &plain + &phitex_flow::fuzz::big_body(4).expect("the plain bodies");
    let dir = scratch("watch");
    std::fs::write(dir.join("big.tex"), &text).expect("the document");
    let mut child = command(Path::new(env!("CARGO_BIN_EXE_phitex")), &dir)
        .args(["watch", "--no-view", "--ini", "--etex", "-j", "8", "big.tex"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("phitex watch runs");
    let stdout = child.stdout.take().expect("its output");
    let mut lines = BufReader::new(stdout).lines().map_while(Result::ok);
    let mut seen = Vec::new();
    let cold = until(&mut lines, "phitex: build 1 ", &mut seen);
    eprintln!("{cold}");
    let at = text.rfind("\\message{[part 2 story]}").expect("part 2");
    let word = text[at..].find(" the ").expect("a word") + at;
    let edited = format!("{} thy {}", &text[..word], &text[word + 5..]);
    std::thread::sleep(Duration::from_millis(50));
    std::fs::write(dir.join("big.tex"), &edited).expect("the edit");
    let line = until(&mut lines, "phitex: build 2 ", &mut seen);
    eprintln!("{line}");
    let watched = std::fs::read(dir.join("big.synctex.gz")).expect("the watch's SyncTeX file");
    if let Some(mut i) = child.stdin.take() {
        let _ = i.write_all(b"q\n");
    }
    let _ = child.wait();
    let cold_dir = scratch("watch-cold");
    std::fs::write(cold_dir.join("big.tex"), &edited).expect("the edited document");
    let out = command(Path::new(env!("CARGO_BIN_EXE_phitex")), &cold_dir)
        .args(["build", "--ini", "--etex", "-j", "8", "big.tex"])
        .output()
        .expect("phitex build runs");
    assert!(out.status.success(), "the cold build failed: {}", String::from_utf8_lossy(&out.stderr));
    // (but for the directory in the input names, and so the anchors' byte counts)
    let norm = |t: &str, d: &Path| {
        t.replace(&d.display().to_string(), "DIR").lines().filter(|l| !l.starts_with('!')).collect::<Vec<_>>().join("\n")
    };
    let a = norm(&String::from_utf8_lossy(&gunzip(&watched)), &dir);
    let b = norm(
        &String::from_utf8_lossy(&gunzip(&std::fs::read(cold_dir.join("big.synctex.gz")).expect("the cold SyncTeX file"))),
        &cold_dir,
    );
    if a != b {
        let (la, lb): (Vec<&str>, Vec<&str>) = (a.lines().collect(), b.lines().collect());
        let k = la.iter().zip(&lb).position(|(x, y)| x != y).unwrap_or(la.len().min(lb.len()));
        panic!(
            "the watch's SyncTeX file after the edit is not the cold build's: line {} ({} and {} lines): {:?} / {:?}",
            k + 1,
            la.len(),
            lb.len(),
            la.get(k),
            lb.get(k)
        );
    }
    let syn = &line[line.find("; synctex ").unwrap_or_else(|| panic!("no SyncTeX time in {line}"))..];
    let (update, sync, walked, pages) = (after(&line, "update "), after(syn, "synctex "), after(syn, "ms ("), after(syn, " of "));
    eprintln!("watch: a word edit's update {update:.2} ms, its SyncTeX file {sync:.2} ms ({walked} of {pages} pages walked)");
    assert!(walked < pages / 4.0, "the edit walked {walked} of {pages} pages again");
    if !cfg!(debug_assertions) {
        assert!(update <= 50.0, "a word edit's update took {update:.2} ms, over the brief's 50 ms");
        assert!(sync <= 50.0, "a word edit's SyncTeX file took {sync:.2} ms, over the brief's 50 ms");
    }
}
