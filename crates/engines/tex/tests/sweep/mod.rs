//! The edit-locality sweep: a one-word edit in every paragraph unit of
//! every text a build read, checked against the node model, without
//! knowing which construct read the text.
//!
//! The texts: the document, every document file the build read (any
//! form of `\input`/`\include`/`\@input`: a name the build asked for
//! that is a file of the document's directory), and every file the job
//! writes and reads back (`Build::read_back_texts`: `.aux`, `.toc`, an
//! index's `.ind`, ...). Their units are the hint pass's slices
//! (`phitex_tex::paragraph_units`). In each sampled unit (up to
//! `PHITEX_SWEEP_N` per text, default 20; the first, middle and last
//! always) one word of 4+ letters gets one letter changed:
//!
//! - in the document or a document file: a word of the unit at brace
//!   depth 0, edited there (`Build::edit`, `Build::edit_file`);
//! - in a read-back text: a word of the unit that occurs once in it and
//!   once in the document's texts, edited where the document has it
//!   (the read-back changes in that unit through the build).
//!
//! Per edit (then undone, so each starts from the cold text):
//! - the node-model bounds of `common::edit_cost_failures` (exec runs
//!   at most 2: the edited unit and one neighbour; folds, finishes,
//!   resumes and places by the pages touched; one end node; no other
//!   operation);
//! - granularity: the edited offset lies in a node of its own text
//!   (`Build::node_spans`), and that node spans at most 2 units; an
//!   `exec` run again because a whole file's value changed (a field
//!   `file:`/`wfile:`) is a coarse read;
//! - every 5th edit: the PDF equals a cold build of the edited text.
//!
//! The builds run with `fixpoint` (the read-backs in memory, as a
//! watch session builds).
//!
//! Every violation is collected; the test fails with the table (text,
//! unit, excerpt, runs by op, violation), each line naming the text,
//! the unit and the op past its bound.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use phitex_tex::{Build, Options};

use crate::common;

/// Where a text comes from, and so how an edit reaches it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Main,
    File,
    ReadBack,
}

struct Text {
    name: String,
    kind: Kind,
    text: String,
    units: Vec<(usize, usize)>,
}

/// One edit's record.
struct Row {
    text: String,
    unit: usize,
    excerpt: String,
    runs: String,
    pages: String,
    violations: Vec<String>,
}

fn opts(name: &str, dir: &Path, epoch: i64) -> Options {
    Options {
        prelude_bytes: std::fs::read(super::pdf_cache()).ok(),
        time_limit: Some(std::time::Duration::from_secs(
            std::env::var("PHITEX_TEST_LIMIT")
                .ok()
                .and_then(|x| x.parse().ok())
                .unwrap_or(120),
        )),
        fixpoint: true,
        epoch: Some(epoch),
        prelude_jobname: "pdflatex".to_owned(),
        ..common::options(&format!("./{name}.tex"), name, "", dir)
    }
}

fn cold(name: &str, dir: &Path, epoch: i64, src: &str) -> Build {
    let cache = super::pdf_cache();
    let cached = cache.exists();
    let b = common::build(opts(name, dir, epoch), Some(&super::prelude(true)), src);
    if !cached {
        let _ = std::fs::create_dir_all(cache.parent().expect("dir"));
        if let Some(bytes) = b.export_prelude() {
            let _ = std::fs::write(&cache, bytes);
        }
    }
    b
}

fn is_letter(c: u8) -> bool {
    c.is_ascii_alphabetic()
}

/// The words of 4+ letters in `t[from..to]` at brace depth 0 (from
/// `from`), outside comments, not a control word: `(start, end)`.
fn words(t: &str, from: usize, to: usize) -> Vec<(usize, usize)> {
    let b = t.as_bytes();
    let mut out = Vec::new();
    let mut depth: i64 = 0;
    let mut k = from;
    while k < to {
        let c = b[k];
        match c {
            b'\\' => {
                k += 1;
                if k < to && is_letter(b[k]) {
                    while k < to && is_letter(b[k]) {
                        k += 1;
                    }
                } else {
                    k += 1;
                }
            }
            b'%' => {
                while k < to && b[k] != b'\n' {
                    k += 1;
                }
            }
            b'{' => {
                depth += 1;
                k += 1;
            }
            b'}' => {
                depth -= 1;
                k += 1;
            }
            c if is_letter(c) => {
                let s = k;
                while k < to && is_letter(b[k]) {
                    k += 1;
                }
                if depth == 0 && k - s >= 4 && (k >= b.len() || b[k] < 0x80) {
                    out.push((s, k));
                }
            }
            _ => k += 1,
        }
    }
    out
}

/// The whole-word occurrences of `w` in `t` that are not a control word.
fn occurrences(t: &str, w: &str) -> Vec<usize> {
    let b = t.as_bytes();
    t.match_indices(w)
        .map(|(k, _)| k)
        .filter(|&k| {
            let e = k + w.len();
            (k == 0 || (!is_letter(b[k - 1]) && b[k - 1] != b'\\'))
                && (e >= b.len() || !is_letter(b[e]))
        })
        .collect()
}

/// The word with its last letter changed (`e`, or `a` for an `e`).
fn changed(w: &str) -> String {
    let (head, last) = w.split_at(w.len() - 1);
    let l = last.as_bytes()[0];
    let n = if l.eq_ignore_ascii_case(&b'e') { b'a' } else { b'e' };
    let n = if l.is_ascii_uppercase() {
        n.to_ascii_uppercase()
    } else {
        n
    };
    format!("{head}{}", char::from(n))
}

/// The units to edit: up to `n`, the first, middle and last always, the
/// rest evenly spaced.
fn sample(len: usize, n: usize) -> Vec<usize> {
    if len <= n {
        return (0..len).collect();
    }
    let mut v: Vec<usize> = (0..n).map(|k| k * (len - 1) / (n - 1).max(1)).collect();
    v.push(0);
    v.push(len / 2);
    v.push(len - 1);
    v.sort_unstable();
    v.dedup();
    while v.len() > n.max(3) {
        // (drop the second of the closest pair that is not a must)
        let k = (1..v.len() - 1)
            .filter(|&k| v[k] != len / 2)
            .min_by_key(|&k| v[k + 1] - v[k])
            .expect("a droppable unit");
        v.remove(k);
    }
    v
}

fn latin1(s: &str) -> Vec<u8> {
    s.chars().map(|c| u8::try_from(c).unwrap_or(b'?')).collect()
}

/// The texts of a cold build: the document, its files, the read-backs.
fn texts(b: &Build, name: &str, dir: &Path, src: &str) -> Vec<Text> {
    let mut v = vec![Text {
        name: format!("{name}.tex"),
        kind: Kind::Main,
        text: src.to_owned(),
        units: phitex_tex::paragraph_units(src),
    }];
    let back = b.read_back_texts();
    let mut seen = vec![format!("{name}.tex")];
    for (f, found) in b.reads() {
        let lower = f.to_ascii_lowercase();
        if !found
            || seen.contains(&f)
            || back.iter().any(|(n, _)| *n == f)
            || !lower.ends_with(".tex")
            || !dir.join(&f).is_file()
        {
            continue;
        }
        seen.push(f.clone());
        let t = common::read(&dir.join(&f));
        v.push(Text {
            name: f,
            kind: Kind::File,
            units: phitex_tex::paragraph_units(&t),
            text: t,
        });
    }
    // (a file the job writes is a read-back when a run read it: an
    // `.idx` only makeindex reads is not one)
    let read: Vec<String> = b
        .files_read()
        .into_iter()
        .map(|x| x.0)
        .chain(b.reads().into_iter().map(|x| x.0))
        .map(|n| n.trim_start_matches("./").to_owned())
        .collect();
    for (f, t) in back {
        if seen.contains(&f) {
            continue;
        }
        if !read.contains(&f) {
            eprintln!("sweep {name}: {f} written, not read back");
            continue;
        }
        seen.push(f.clone());
        v.push(Text {
            name: f,
            kind: Kind::ReadBack,
            units: phitex_tex::paragraph_units(&t),
            text: t,
        });
    }
    v
}

/// An edit: the text it changes (an index into the texts), the offset
/// and the word there; and where it shows (the text and offset checked
/// for granularity).
struct Edit {
    at_text: usize,
    at: usize,
    word: String,
    seen_text: usize,
    seen_at: usize,
}

/// The editable region of a text (the document's body only).
fn region(t: &Text) -> (usize, usize) {
    if t.kind == Kind::Main {
        let a = t.text.find("\\begin{document}").map_or(0, |a| a + 16);
        let e = t.text.find("\\end{document}").unwrap_or(t.text.len());
        (a, e)
    } else {
        (0, t.text.len())
    }
}

fn plan(ts: &[Text], k: usize, (us, ue): (usize, usize)) -> Option<Edit> {
    let t = &ts[k];
    if t.kind != Kind::ReadBack {
        let (a, e) = region(t);
        let (s, e) = (us.max(a), ue.min(e));
        if s >= e {
            return None;
        }
        let ws = words(&t.text, s, e);
        let &(ws0, we0) = ws.get(ws.len() / 2)?;
        return Some(Edit {
            at_text: k,
            at: ws0,
            word: t.text[ws0..we0].to_owned(),
            seen_text: k,
            seen_at: ws0,
        });
    }
    // a word of the read-back unit, once in it and once in the
    // document's texts (at any depth: `\index{word}`)
    let b = t.text.as_bytes();
    let mut c = us;
    while c < ue {
        if !is_letter(b[c]) {
            c += 1;
            continue;
        }
        let s = c;
        while c < ue && is_letter(b[c]) {
            c += 1;
        }
        let w = &t.text[s..c];
        if w.len() < 4 || (s > 0 && b[s - 1] == b'\\') || occurrences(&t.text, w).len() != 1 {
            continue;
        }
        let mut found = Vec::new();
        for (j, d) in ts.iter().enumerate() {
            if d.kind == Kind::ReadBack {
                continue;
            }
            let (a, e) = region(d);
            for o in occurrences(&d.text, w) {
                if a <= o && o < e {
                    found.push((j, o));
                }
            }
        }
        if let [(j, o)] = found[..] {
            return Some(Edit {
                at_text: j,
                at: o,
                word: w.to_owned(),
                seen_text: k,
                seen_at: s,
            });
        }
    }
    None
}

fn runs_line(b: &Build) -> String {
    let mut r: Vec<_> = b.stats.runs_by_op.iter().filter(|x| x.1 > 0).collect();
    r.sort();
    r.iter()
        .map(|(o, n)| format!("{o} {n}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Applies `new` (the text `k`'s new content) to the build.
fn apply(b: &mut Build, t: &Text, new: &str) {
    match t.kind {
        Kind::Main => b.edit(new),
        _ => b.edit_file(&t.name, Some(&latin1(new))),
    }
}

/// The granularity of the node the edit shows in.
fn granularity(b: &Build, t: &Text, content: &str, at: usize) -> Vec<String> {
    let same = |x: &Arc<str>| x.trim_end_matches('\n') == content.trim_end_matches('\n');
    let spans: Vec<(usize, usize)> = b
        .node_spans()
        .into_iter()
        .filter(|(_, x, _, _)| match (t.kind, x) {
            (Kind::Main, None) => true,
            (Kind::Main, Some(_)) | (_, None) => false,
            (_, Some(x)) => same(x),
        })
        .map(|(_, _, s, e)| (s, e))
        .collect();
    let mut f = Vec::new();
    match spans.iter().find(|&&(s, e)| s <= at && at < e) {
        None => f.push(format!(
            "exec: no node of its own in {} (read inside another node's run)",
            t.name
        )),
        Some(&(s, e)) => {
            let k = t.units.iter().filter(|&&(a, z)| a < e && s < z).count();
            if k > 2 {
                f.push(format!("exec: one node over {k} units of {}", t.name));
            }
        }
    }
    let mut coarse: Vec<String> = b
        .stats
        .reruns
        .iter()
        .filter(|(_, op, why)| *op == "exec" && why.contains("file:"))
        .map(|(_, _, why)| why.clone())
        .collect();
    coarse.sort();
    coarse.dedup();
    for c in coarse {
        f.push(format!("exec: ran again on a whole file's value ({c})"));
    }
    f
}

/// The sweep of one document in `src_dir` (its files copied to a
/// directory of its own): the rows, and the cold build's check.
#[allow(clippy::too_many_lines)]
fn sweep(name: &str, src_dir: &Path, files: &[&str], n: usize, epoch: i64, oracle: Option<&Path>) -> (Vec<Row>, String) {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("sweep-{name}"));
    let _ = std::fs::create_dir_all(&dir);
    for f in files {
        std::fs::copy(src_dir.join(f), dir.join(f)).expect("copy");
    }
    let src = common::read(&dir.join(format!("{name}.tex")));
    let t0 = std::time::Instant::now();
    let mut b = cold(name, &dir, epoch, &src);
    let mut head = format!(
        "sweep {name}: cold {:?}, {} pages, {} nodes run",
        t0.elapsed(),
        b.pages().len(),
        b.stats.nodes_run
    );
    if let Some(o) = oracle {
        let want = std::fs::read(o).expect("the oracle");
        if let Some(d) = common::first_difference(&b.output().0, &want) {
            let _ = write!(head, "; COLD differs from the oracle: {d}");
        } else {
            head.push_str("; cold equals the oracle");
        }
    }
    let cold_folds = common::runs_of(&b, "fold");
    let ts = texts(&b, name, &dir, &src);
    for t in &ts {
        let _ = write!(head, "\n  text {} ({:?}): {} units", t.name, t.kind, t.units.len());
    }
    eprintln!("{head}");
    let mut rows = Vec::new();
    let mut edits = 0usize;
    for (k, t) in ts.iter().enumerate() {
        // (the units an edit can reach)
        let plans: Vec<(usize, Edit)> = t
            .units
            .iter()
            .enumerate()
            .filter_map(|(u, &r)| plan(&ts, k, r).map(|e| (u, e)))
            .collect();
        if plans.is_empty() {
            continue;
        }
        let picks = sample(plans.len(), n);
        for p in picks {
            let (u, e) = &plans[p];
            let at = &ts[e.at_text];
            let w2 = changed(&e.word);
            let new = format!("{}{w2}{}", &at.text[..e.at], &at.text[e.at + e.word.len()..]);
            let before = b.pages();
            let back_before = b.read_back_texts();
            let t1 = std::time::Instant::now();
            apply(&mut b, at, &new);
            let wall = t1.elapsed();
            let mut v = common::edit_cost_failures(&b, 1, &before, cold_folds);
            for x in &mut v {
                if !x.contains(':') {
                    *x = format!("runs/node: {x}");
                }
            }
            // an edit that changes a file the job writes and reads back
            // may run the read-back publisher (`files`) once; else none
            let back_changed: Vec<String> = {
                let after = b.read_back_texts();
                let mut c: Vec<String> = after
                    .iter()
                    .filter(|x| !back_before.contains(x))
                    .map(|x| x.0.clone())
                    .chain(
                        back_before
                            .iter()
                            .filter(|x| !after.iter().any(|y| y.0 == x.0))
                            .map(|x| x.0.clone()),
                    )
                    .collect();
                c.sort();
                c.dedup();
                c
            };
            if !back_changed.is_empty() {
                v.retain(|x| !x.starts_with("files:"));
                let f = common::runs_of(&b, "files");
                if f > 1 {
                    v.push(format!(
                        "files: {f} runs, bound 1 (read-back changed: {})",
                        back_changed.join(", ")
                    ));
                }
            }
            // a second round of a fixpoint build: name the loop-back
            // fields whose new values made it
            for x in &mut v {
                if x.starts_with("dvi:") {
                    let mut loops: Vec<&str> = b
                        .stats
                        .reruns
                        .iter()
                        .filter_map(|(_, _, why)| why.strip_prefix("Loop "))
                        .collect();
                    loops.sort_unstable();
                    loops.dedup();
                    let shown: Vec<&str> = loops.iter().take(6).copied().collect();
                    let _ = write!(
                        x,
                        " (second round by loop-back {:?}{}; read-back changed: {:?})",
                        shown,
                        if loops.len() > 6 { format!(" +{} more", loops.len() - 6) } else { String::new() },
                        back_changed
                    );
                }
            }
            let content = if e.seen_text == e.at_text { new.as_str() } else { ts[e.seen_text].text.as_str() };
            v.extend(granularity(&b, &ts[e.seen_text], content, e.seen_at));
            edits += 1;
            if edits % 5 == 0 {
                let c = if at.kind == Kind::Main {
                    cold(name, &dir, epoch, &new)
                } else {
                    std::fs::write(dir.join(&at.name), latin1(&new)).expect("write");
                    let c = cold(name, &dir, epoch, &src);
                    std::fs::write(dir.join(&at.name), latin1(&at.text)).expect("write");
                    c
                };
                if let Some(d) = common::first_difference(&b.output().0, &c.output().0) {
                    v.push(format!("pdf: edit differs from a cold build: {d}"));
                }
            }
            rows.push(Row {
                text: t.name.clone(),
                unit: *u,
                excerpt: format!(
                    "{}{} <{}>",
                    if e.seen_text == e.at_text { String::new() } else { format!("[via {}] ", at.name) },
                    {
                        let (us, ue) = t.units[*u];
                        t.text[us..ue].trim_start().chars().take(20).collect::<String>().replace('\n', " ")
                    },
                    e.word
                ),
                runs: runs_line(&b),
                pages: format!(
                    "{:?}/{} {wall:.1?}",
                    common::pages_touched(&before, &b.pages()),
                    b.pages().len()
                ),
                violations: v,
            });
            // undone: the next edit starts from the cold text
            apply(&mut b, at, &at.text);
        }
    }
    (rows, head)
}

fn table(name: &str, head: &str, rows: &[Row]) -> String {
    let mut s = format!("{head}\n");
    let _ = writeln!(s, "{:<18} {:>4}  {:<44} {:<46} {:<16} violation", "text", "unit", "excerpt", "runs by op", "pages/wall");
    for r in rows {
        let _ = writeln!(
            s,
            "{:<18} {:>4}  {:<44} {:<46} {:<16} {}",
            r.text,
            r.unit,
            r.excerpt.chars().take(44).collect::<String>(),
            r.runs,
            r.pages,
            if r.violations.is_empty() { "-".to_owned() } else { r.violations.join("; ") }
        );
    }
    let mut dist = std::collections::BTreeMap::<u64, usize>::new();
    for r in rows {
        let e = r
            .runs
            .split(", ")
            .find_map(|x| x.strip_prefix("exec ").and_then(|n| n.parse().ok()))
            .unwrap_or(0);
        *dist.entry(e).or_default() += 1;
    }
    let bad = rows.iter().filter(|r| !r.violations.is_empty()).count();
    let _ = writeln!(s, "{name}: {} edits, {bad} with violations; exec runs per edit (runs: edits) {dist:?}", rows.len());
    // per text: edits, edits with violations, exec runs, the most
    // frequent kinds of violation (numbers masked)
    let mut names: Vec<&str> = rows.iter().map(|r| r.text.as_str()).collect();
    names.dedup();
    for t in names {
        let rs: Vec<&Row> = rows.iter().filter(|r| r.text == t).collect();
        let bad = rs.iter().filter(|r| !r.violations.is_empty()).count();
        let mut dist = std::collections::BTreeMap::<u64, usize>::new();
        let mut kinds = std::collections::BTreeMap::<String, usize>::new();
        for r in &rs {
            let e = r
                .runs
                .split(", ")
                .find_map(|x| x.strip_prefix("exec ").and_then(|n| n.parse().ok()))
                .unwrap_or(0);
            *dist.entry(e).or_default() += 1;
            for v in &r.violations {
                let k: String = v.split(" (").next().unwrap_or(v).chars().map(|c| if c.is_ascii_digit() { '#' } else { c }).collect();
                let mut k = k.replace("##", "#");
                while k.contains("##") {
                    k = k.replace("##", "#");
                }
                *kinds.entry(k).or_default() += 1;
            }
        }
        let mut kinds: Vec<(String, usize)> = kinds.into_iter().collect();
        kinds.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        kinds.truncate(5);
        let _ = writeln!(s, "  per text {t}: {} edits, {bad} with violations, exec runs {dist:?}, top violations {kinds:?}", rs.len());
    }
    s
}

fn check(name: &str, src_dir: &Path, files: &[&str], n: usize, epoch: i64, oracle: Option<&Path>) {
    let t0 = std::time::Instant::now();
    let (rows, head) = sweep(name, src_dir, files, n, epoch, oracle);
    let t = table(name, &head, &rows);
    eprintln!("{t}sweep {name}: {:?}", t0.elapsed());
    let red: Vec<String> = rows
        .iter()
        .flat_map(|r| {
            r.violations
                .iter()
                .map(move |v| format!("{name}: {} unit {} ({}): {v}", r.text, r.unit, r.excerpt))
        })
        .collect();
    assert!(red.is_empty(), "edit-locality sweep {name}: {} violations\n{}", red.len(), red.join("\n"));
}

fn per_text(default: usize) -> usize {
    std::env::var("PHITEX_SWEEP_N")
        .ok()
        .and_then(|x| x.parse().ok())
        .unwrap_or(default)
}

fn pdfdocs() -> PathBuf {
    common::fixtures().join("latex/pdfdocs")
}

/// The main file, a chapter read by `\input{chap1}`, one by
/// `\include{chap2}` (its `.aux` read back by `\@input`).
#[test]
fn sweep_input_include() {
    check(
        "inputs",
        &pdfdocs(),
        &["inputs.tex", "chap1.tex", "chap2.tex"],
        per_text(20),
        0,
        Some(&common::fixtures().join("latex/pdf/inputs.pdf")),
    );
}

/// An article with thirty `\index` entries and `\printindex`
/// (`imakeidx`): the `.ind` read back is a text like any other.
#[test]
fn sweep_index() {
    check(
        "index30",
        &pdfdocs(),
        &["index30.tex"],
        per_text(20),
        0,
        Some(&common::fixtures().join("latex/pdf/index30.pdf")),
    );
}

/// amsldoc (`bench/amsldoc`), 5 units per text: slow.
#[test]
#[ignore = "slow: amsldoc, a cold build per 5 edits"]
fn sweep_amsldoc() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../bench/amsldoc");
    check("amsldoc", &dir, &["amsldoc.tex"], per_text(5), 1_700_000_000, None);
}
