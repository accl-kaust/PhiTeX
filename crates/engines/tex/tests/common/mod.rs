//! What the gates share: the oracle's fixtures, the file finder (the
//! fixture's directory, then kpathsea for fonts), comparisons.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use phitex_tex::{Build, Finder, Options};

pub fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../oracle/fixtures")
}

pub fn read(p: &Path) -> String {
    let b = std::fs::read(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    b.iter().map(|&c| char::from(c)).collect()
}

fn kpse() -> &'static Mutex<Option<phitex_kpse::Kpse>> {
    static K: OnceLock<Mutex<Option<phitex_kpse::Kpse>>> = OnceLock::new();
    K.get_or_init(|| {
        let path = std::env::var("PATH").unwrap_or_default();
        let dir = std::env::split_paths(&path).find(|d| d.join("tex").is_file());
        Mutex::new(dir.map(|d| phitex_kpse::Kpse::new(&d, "tex", "tex")))
    })
}

/// Files from `dir` (printed as `./name`), fonts through kpathsea.
pub fn finder(dir: &Path) -> Finder {
    let dir = dir.to_path_buf();
    Arc::new(move |name: &str| {
        let p = dir.join(name);
        if p.is_file() {
            return Some((format!("./{name}"), std::fs::read(&p).ok()?));
        }
        let ext = Path::new(name)
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        let fmt = match ext.as_str() {
            "tfm" => phitex_kpse::Format::Tfm,
            "map" => phitex_kpse::Format::FontMap,
            "pfb" | "pfa" => phitex_kpse::Format::Type1,
            "enc" => phitex_kpse::Format::Enc,
            "ttf" | "ttc" => phitex_kpse::Format::TrueType,
            "vf" => phitex_kpse::Format::Vf,
            _ => phitex_kpse::Format::Tex,
        };
        let mut k = kpse().lock().ok()?;
        let path = k.as_mut()?.find_file(name.as_bytes(), fmt, false)?;
        let path = String::from_utf8(path).ok()?;
        Some((path.clone(), std::fs::read(&path).ok()?))
    })
}

/// A tool's support files (`phitex_tex::ToolSearch`): from `dir`, else
/// through kpathsea in the search's format.
pub fn tool_finder(dir: &Path) -> phitex_tex::ToolFinder {
    let dir = dir.to_path_buf();
    Arc::new(move |search: phitex_tex::ToolSearch, name: &str| {
        let (fmt, ext) = match search {
            phitex_tex::ToolSearch::BibStyle => (phitex_kpse::Format::Bst, "bst"),
            phitex_tex::ToolSearch::BibData => (phitex_kpse::Format::Bib, "bib"),
            phitex_tex::ToolSearch::IndexStyle => (phitex_kpse::Format::Ist, "ist"),
        };
        for n in [name.to_owned(), format!("{name}.{ext}")] {
            let p = dir.join(&n);
            if p.is_file() {
                return Some((format!("./{n}"), std::fs::read(&p).ok()?));
            }
        }
        let mut k = kpse().lock().ok()?;
        let path = k.as_mut()?.find_file(name.as_bytes(), fmt, false)?;
        drop(k);
        let path = String::from_utf8(path).ok()?;
        Some((path.clone(), std::fs::read(&path).ok()?))
    })
}

pub fn options(name: &str, jobname: &str, comment: &str, dir: &Path) -> Options {
    Options {
        name: name.to_owned(),
        jobname: jobname.to_owned(),
        comment: comment.to_owned(),
        finder: finder(dir),
        ..Options::default()
    }
}

pub fn build(opts: Options, prelude: Option<&str>, doc: &str) -> Build {
    Build::with(opts, prelude, doc)
}

/// The first difference, for the report.
pub fn first_difference(got: &[u8], want: &[u8]) -> Option<String> {
    let n = got.iter().zip(want).take_while(|(a, b)| a == b).count();
    if n == got.len() && n == want.len() {
        return None;
    }
    let show = |b: &[u8]| -> String {
        b[n.saturating_sub(40).min(b.len())..(n + 40).min(b.len())]
            .iter()
            .map(|&c| {
                if (32..127).contains(&c) || c == b'\n' {
                    char::from(c)
                } else {
                    '?'
                }
            })
            .collect()
    };
    Some(format!(
        "byte {n} (got {} bytes, want {}):\n  got:  {:?}\n  want: {:?}",
        got.len(),
        want.len(),
        show(got),
        show(want)
    ))
}

/// The DVI preamble's comment of an oracle file.
pub fn dvi_comment(dvi: &[u8]) -> String {
    let k = usize::from(dvi.get(14).copied().unwrap_or(0));
    dvi.get(15..15 + k)
        .map(|b| b.iter().map(|&c| char::from(c)).collect())
        .unwrap_or_default()
}

/// plain.tex as the oracle's plain fixtures load it.
pub fn plain() -> String {
    let s = read(&fixtures().join("dvi/plaintext.tex"));
    let at = s.find("\\output={}\n").expect("plaintext.tex's body");
    let h = read(&fixtures().join("hyph/hyphen.tex"));
    s[..at].replace(
        "% (\\input hyphen: no hyphenation patterns on the graph yet)\n",
        &h,
    )
}

/// The log as the `.log` fixtures keep it: from the first file opened to
/// the last `)`; their capture leaves out the banner before and §642's
/// line after ("Output written on ..." or "No pages of output."), which
/// `ini.rs` checks against whole oracle logs.
pub fn log_excerpt(log: &str) -> String {
    let t = log.trim_end_matches('\n');
    match t.rsplit_once('\n') {
        Some((head, last))
            if last.starts_with("Output written on ") || last == "No pages of output." =>
        {
            head.to_owned()
        }
        _ => t.to_owned(),
    }
}

/// The pages whose content an edit changed: the indices of the page
/// hashes before and after that differ, and of the pages one side has
/// and the other not.
#[must_use]
pub fn pages_touched(before: &[u64], after: &[u64]) -> Vec<usize> {
    (0..before.len().max(after.len()))
        .filter(|&k| before.get(k) != after.get(k))
        .collect()
}

/// The runs of `op` in `b`'s last build or edit.
#[must_use]
pub fn runs_of(b: &Build, op: &str) -> u64 {
    b.stats
        .runs_by_op
        .iter()
        .filter(|x| x.0 == op)
        .map(|x| x.1)
        .sum()
}

/// What an edit may run in the node model, by operation (runs, not
/// nodes): the touched pieces' `exec` (and one more); the `scan` step
/// of every piece on a touched page (one per piece and one per page
/// after its finisher: the edited piece's and the ones after it on its
/// page; steps per page = the cold build's scan runs / its pages,
/// rounded up);
/// one `finish` and one `resume` per touched page; a `place` for every
/// page from the first touched one to the last (in exact mode place
/// replays the PDF state in order); the link and the end node (`dvi`) once each; nothing
/// of any other operation; and at most 1.1 runs a node.
/// `cold_folds` and `before` are the cold build's scan runs and page
/// hashes.
#[must_use]
pub fn edit_cost_failures(b: &Build, touched: u64, before: &[u64], cold_folds: u64) -> Vec<String> {
    let after = b.pages();
    let pages = pages_touched(before, &after);
    let n = pages.len() as u64;
    let per_page = cold_folds.div_ceil(before.len().max(1) as u64);
    let to_end = pages
        .first()
        .map_or(0, |&k| (after.len().max(before.len()) - k) as u64);
    let bounds = [
        ("exec", touched + 1),
        ("scan", per_page * n),
        ("finish", n),
        ("resume", n),
        ("place", to_end),
        ("link", 1),
        ("dvi", 1),
    ];
    let mut f = Vec::new();
    for &(op, bound) in &bounds {
        let r = runs_of(b, op);
        if r > bound {
            f.push(format!("{op}: {r} runs, bound {bound}"));
        }
    }
    for &(op, r) in &b.stats.runs_by_op {
        if r > 0 && !bounds.iter().any(|x| x.0 == op) {
            f.push(format!("{op}: {r} runs, bound 0"));
        }
    }
    #[allow(clippy::cast_precision_loss)]
    let per_node = b.stats.runs as f64 / b.stats.nodes_run.max(1) as f64;
    if per_node > 1.1 {
        f.push(format!("runs a node {per_node:.2}"));
    }
    f
}

/// Asserts [`edit_cost_failures`] on an edit, printing the runs by
/// operation and the touched pages either way.
pub fn assert_edit_cost(name: &str, b: &Build, touched: u64, before: &[u64], cold_folds: u64) {
    eprintln!(
        "{name}: edit runs by op {:?}; {touched} touched pieces, pages touched {:?} of {}, cold folds {cold_folds}",
        b.stats.runs_by_op,
        pages_touched(before, &b.pages()),
        b.pages().len()
    );
    let f = edit_cost_failures(b, touched, before, cold_folds);
    assert!(
        f.is_empty(),
        "{name}: edit ran beyond the node model: {}",
        f.join("; ")
    );
}

/// One document's chain census, cold and after a one-word edit: the
/// gate the engine's parallelism and speculation rest on.
pub struct Census {
    pub name: String,
    pub cold_cp: usize,
    pub cold_rounds: u64,
    pub cold_runs_per_node: f64,
    pub step_links: usize,
    pub undeclared: u64,
    pub violations: u64,
    pub edit_cp: usize,
    pub edit_rounds: u64,
    pub edit_runs_per_node: f64,
    pub edit_nodes: u64,
    pub edit_page_steps: u64,
    pub causes: String,
}

#[allow(clippy::cast_precision_loss)]
fn per_node(b: &Build) -> f64 {
    b.stats.runs as f64 / (b.stats.nodes_run.max(1) as f64)
}

fn causes(b: &Build) -> String {
    let mut by = std::collections::BTreeMap::<String, usize>::new();
    for (_, kind, field) in b.critical_chain() {
        *by.entry(format!("{kind} {field}")).or_default() += 1;
    }
    format!("{by:?}")
}

impl Census {
    /// Takes the cold census of `b`, edits `new` into it, and takes the
    /// edit's.
    pub fn take(name: &str, b: &mut Build, new: &str) -> Census {
        let cold_cp = b.stats.critical_path.unwrap_or(usize::MAX);
        let cold_rounds = b.stats.loop_rounds;
        let cold_runs_per_node = per_node(b);
        let step_links = b.page_step_links().len();
        let cold_causes = causes(b);
        let (u0, v0) = (b.stats.undeclared_definitions, b.stats.regime_violations);
        let p0 = b.stats.page_steps;
        let mut notes: Vec<String> = b
            .diagnostics()
            .iter()
            .filter(|d| d.message.contains("undeclared"))
            .take(6)
            .map(|d| d.message.chars().take(300).collect())
            .collect();
        b.edit(new);
        notes.extend(
            b.diagnostics()
                .iter()
                .filter(|d| d.message.contains("undeclared"))
                .take(3)
                .map(|d| format!("edit: {}", d.message.chars().take(300).collect::<String>())),
        );
        let s = &b.stats;
        Census {
            name: name.to_owned(),
            cold_cp,
            cold_rounds,
            cold_runs_per_node,
            step_links: step_links + b.page_step_links().len(),
            undeclared: u0.max(s.undeclared_definitions),
            violations: v0.max(s.regime_violations),
            edit_cp: s.critical_path.unwrap_or(usize::MAX),
            edit_rounds: s.loop_rounds,
            edit_runs_per_node: per_node(b),
            edit_nodes: s.nodes_run,
            edit_page_steps: if s.page_steps >= p0 {
                s.page_steps - p0
            } else {
                s.page_steps
            },
            causes: format!("cold {cold_causes}; edit {}; {notes:#?}", causes(b)),
        }
    }

    /// What breaks the bar: critical path at most 6, no link between
    /// two page steps, at most 1.1 runs a node, at most 2 loop rounds
    /// cold and 1 (no round after the first) on a word edit, no
    /// undeclared definition, no regime violation.
    pub fn failures(&self) -> Vec<String> {
        let mut f = Vec::new();
        let n = &self.name;
        if self.cold_cp > 6 {
            f.push(format!("{n}: cold critical path {}", self.cold_cp));
        }
        if self.edit_cp > 6 {
            f.push(format!("{n}: edit critical path {}", self.edit_cp));
        }
        if self.step_links > 0 {
            f.push(format!("{n}: {} page step links", self.step_links));
        }
        if self.cold_runs_per_node > 1.1 || self.edit_runs_per_node > 1.1 {
            f.push(format!(
                "{n}: runs a node {:.2} cold, {:.2} edit",
                self.cold_runs_per_node, self.edit_runs_per_node
            ));
        }
        if self.cold_rounds > 2 {
            f.push(format!("{n}: {} loop rounds cold", self.cold_rounds));
        }
        if self.edit_rounds > 1 {
            f.push(format!(
                "{n}: {} loop rounds on a word edit",
                self.edit_rounds
            ));
        }
        if self.undeclared > 0 {
            f.push(format!("{n}: {} undeclared definitions", self.undeclared));
        }
        if self.violations > 0 {
            f.push(format!("{n}: {} regime violations", self.violations));
        }
        f
    }
}

/// The census table, one row a document, and its causes.
pub fn census_table(rows: &[Census]) -> String {
    use std::fmt::Write as _;
    let mut t = String::from(
        "doc | cp cold | cp edit | step links | runs/node cold | runs/node edit | rounds cold | rounds edit | undeclared | violations | edit nodes | edit page steps\n",
    );
    for r in rows {
        let _ = writeln!(
            t,
            "{} | {} | {} | {} | {:.2} | {:.2} | {} | {} | {} | {} | {} | {}",
            r.name,
            r.cold_cp,
            r.edit_cp,
            r.step_links,
            r.cold_runs_per_node,
            r.edit_runs_per_node,
            r.cold_rounds,
            r.edit_rounds,
            r.undeclared,
            r.violations,
            r.edit_nodes,
            r.edit_page_steps
        );
    }
    for r in rows {
        let _ = writeln!(t, "{}: {}", r.name, r.causes);
    }
    t
}

/// Asserts the census bar on `rows`, printing the table either way.
pub fn assert_census(rows: &[Census]) {
    let t = census_table(rows);
    eprintln!("{t}");
    let f: Vec<String> = rows.iter().flat_map(Census::failures).collect();
    assert!(f.is_empty(), "census:\n{}\n{t}", f.join("\n"));
}

/// A plain document of `pars` paragraphs with headings and a footnote
/// every twentieth (`pdf`: `\pdfoutput=1`), and the same with one word
/// changed near the middle.
pub fn plain_paragraphs(pars: usize, pdf: bool) -> (String, String) {
    use std::fmt::Write as _;
    let mut src = String::new();
    if pdf {
        src.push_str("\\pdfoutput=1\n");
    }
    src.push_str("\\hsize=6in \\vsize=8.5in \\parindent=20pt\n");
    for k in 0..pars {
        if k % 10 == 0 {
            let _ = writeln!(
                src,
                "\\bigskip\\centerline{{\\bf Section {}}}\\medskip",
                k / 10 + 1
            );
        }
        for _ in 0..4 {
            let _ = write!(
                src,
                "Paragraph {k} has some words in it, enough to fill a few lines of text set \
                 with plain's macros, so that the paragraph builder has work to do and the \
                 pages fill up one after another. "
            );
        }
        if k % 20 == 19 {
            let _ = write!(
                src,
                "\\footnote{{$^{{{}}}$}}{{A note on paragraph {k}.}}",
                k / 20 + 1
            );
        }
        src.push_str(".\n\n");
    }
    src.push_str("\\bye\n");
    let mid = src.len() / 2;
    let at = mid + src[mid..].find(" words ").expect("a word") + 1;
    let new = format!("{}wurds{}", &src[..at], &src[at + 5..]);
    (src, new)
}
