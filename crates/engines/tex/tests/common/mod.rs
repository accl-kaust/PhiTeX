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
        b.edit(new);
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
            causes: format!("cold {cold_causes}; edit {}", causes(b)),
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
            f.push(format!("{n}: {} loop rounds on a word edit", self.edit_rounds));
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
    let mut t = String::from(
        "doc | cp cold | cp edit | step links | runs/node cold | runs/node edit | rounds cold | rounds edit | undeclared | violations | edit nodes | edit page steps\n",
    );
    for r in rows {
        t.push_str(&format!(
            "{} | {} | {} | {} | {:.2} | {:.2} | {} | {} | {} | {} | {} | {}\n",
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
        ));
    }
    for r in rows {
        t.push_str(&format!("{}: {}\n", r.name, r.causes));
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
