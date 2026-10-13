//! The correctness ladder (`oracle/fixtures/ladder/<nn>-<name>/`):
//! documents of rising complexity, each built by `phitex build` (the
//! CLI's own fixpoint) at 1 and at 8 threads, and its two edits
//! (`edit-word/`, `edit-para/`) built as edits in one `phitex watch`
//! session and as fresh cold builds. Every PDF is compared byte for byte
//! with the oracle's (`<variant>/oracle/`, from `scripts/bless-ladder.sh`:
//! the installed pdflatex, bibtex, makeindex, biber), and the files the job
//! reads back (`.aux`, `.toc`, `.bbl`, `.ind`, ...) too; the watch's PDF
//! with the cold build's. One row per rung on standard error (`ROW ...`).
//! Run once with `PHITEX_DOC_SCOPE` unset and once with `=1`.

use std::io::{BufRead as _, BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn ladder() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../oracle/fixtures/ladder")
}

fn cache() -> PathBuf {
    std::env::var_os("PHITEX_CACHE_DIR").map_or_else(
        || PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("phitex-cache"),
        PathBuf::from,
    )
}

fn scratch(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("ladder")
        .join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("a scratch directory");
    d
}

fn phitex(dir: &Path) -> Command {
    let mut c = Command::new("timeout");
    c.arg("900").arg(env!("CARGO_BIN_EXE_phitex"));
    c.current_dir(dir)
        .env("PHITEX_CACHE_DIR", cache())
        .env("SOURCE_DATE_EPOCH", "0")
        .env("FORCE_SOURCE_DATE", "1")
        .env_remove("PHITEX_COMPAT");
    c
}

/// The rung's directory, its job name.
fn rung_dir(nn: &str) -> (PathBuf, String) {
    for e in std::fs::read_dir(ladder()).expect("the ladder").flatten() {
        let f = e.file_name().to_string_lossy().into_owned();
        if let Some(name) = f.strip_prefix(&format!("{nn}-")) {
            return (e.path(), name.to_string());
        }
    }
    panic!("no rung {nn}");
}

/// A fresh directory with the rung's assets and `text` as NAME.tex.
fn setup(rung: &Path, name: &str, tag: &str, text: &str) -> PathBuf {
    let dir = scratch(tag);
    if let Some(u) = upstream_doc(rung) {
        copy_tree(&u, &dir);
    }
    for e in std::fs::read_dir(rung).expect("the rung").flatten() {
        let p = e.path();
        if p.is_file() && p.extension().is_none_or(|x| x != "tex") && e.file_name() != "upstream" {
            std::fs::copy(&p, dir.join(e.file_name())).expect("an asset");
        }
    }
    std::fs::write(dir.join(format!("{name}.tex")), text).expect("the text");
    dir
}

fn text(rung: &Path, variant: &str, name: &str) -> String {
    if variant == "."
        && let Some(u) = upstream_doc(rung)
    {
        return std::fs::read_to_string(u.join(format!("{name}.tex"))).expect("a text");
    }
    std::fs::read_to_string(rung.join(variant).join(format!("{name}.tex"))).expect("a text")
}

/// The first differing byte and the PDF object around it.
fn diff(a: &[u8], b: &[u8]) -> Option<String> {
    if a == b {
        return None;
    }
    let at = a
        .iter()
        .zip(b)
        .position(|(x, y)| x != y)
        .unwrap_or(a.len().min(b.len()));
    let head = String::from_utf8_lossy(&b[..at.min(b.len())]);
    let obj = head.rmatch_indices(" 0 obj").next().map_or_else(
        || "header".to_string(),
        |(i, _)| {
            let s = &head[..i];
            let n = s
                .rsplit(|c: char| !c.is_ascii_digit())
                .next()
                .unwrap_or("?");
            format!("obj {n}")
        },
    );
    Some(format!(
        "byte {at} in {obj} (lengths {} vs oracle {})",
        a.len(),
        b.len()
    ))
}

/// Compares `dir`'s outputs with the oracle `odir`: the PDF and each file
/// read back the oracle has. The differences, empty if identical.
fn compare(dir: &Path, odir: &Path, name: &str) -> Vec<String> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(odir).expect("the oracle").flatten() {
        let f = e.file_name().to_string_lossy().into_owned();
        if f == "stats" {
            continue;
        }
        if f == "sha256" {
            let want = std::fs::read_to_string(e.path()).unwrap_or_default();
            let got = sha256(&dir.join(format!("{name}.pdf")));
            if got != want.trim() {
                out.push(format!("{name}.pdf: sha256 {got} is not the oracle's"));
            }
            continue;
        }
        let want = std::fs::read(e.path()).expect("an oracle file");
        let got = std::fs::read(dir.join(&f)).unwrap_or_default();
        if let Some(d) = diff(&got, &want) {
            out.push(format!("{f}: {d}"));
        }
    }
    let _ = name;
    out
}

fn stat(odir: &Path, key: &str) -> String {
    std::fs::read_to_string(odir.join("stats"))
        .unwrap_or_default()
        .lines()
        .find_map(|l| l.strip_prefix(&format!("{key} ")).map(str::to_string))
        .unwrap_or_default()
}

struct Cold {
    wall: f64,
    ok: bool,
    pages: String,
    nodes: String,
    errors: String,
    diffs: Vec<String>,
    phases: String,
}

/// The `PHITEX_TEX_TIME` phase lines of a build's standard error, each
/// cut short.
fn phases(err: &str) -> String {
    err.lines()
        .filter(|l| {
            [
                "time:",
                "format deserialise",
                "prelude",
                "push_text",
                "round:",
                "pdf finish",
                "edit apply",
            ]
            .iter()
            .any(|p| l.starts_with(p))
        })
        .map(|l| l.split(", ").take(4).collect::<Vec<_>>().join(", "))
        .collect::<Vec<_>>()
        .join(" ; ")
}

/// A big upstream document (rung 13, the TikZ manual): its files copied
/// beside the job, its oracle a hash (`oracle/sha256`; the PDF itself is
/// too big to commit).
fn upstream_doc(rung: &Path) -> Option<PathBuf> {
    let f = std::fs::read_to_string(rung.join("upstream")).ok()?;
    Some(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(f.trim()),
    )
}

fn copy_tree(from: &Path, to: &Path) {
    for e in std::fs::read_dir(from).expect("a directory").flatten() {
        let t = to.join(e.file_name());
        if e.path().is_dir() {
            let _ = std::fs::create_dir_all(&t);
            copy_tree(&e.path(), &t);
        } else {
            std::fs::copy(e.path(), &t).expect("a copy");
        }
    }
}

fn sha256(path: &Path) -> String {
    let out = Command::new("sha256sum")
        .arg(path)
        .output()
        .expect("sha256sum");
    String::from_utf8_lossy(&out.stdout)
        .split(' ')
        .next()
        .unwrap_or("")
        .to_string()
}

fn cold(rung: &Path, name: &str, variant: &str, threads: usize, tag: &str) -> (Cold, PathBuf) {
    let dir = setup(rung, name, tag, &text(rung, variant, name));
    let t = Instant::now();
    let mut c = phitex(&dir);
    if threads == 1 {
        c.env("PHITEX_TEX_TIME", "1");
    }
    let out = c
        .args([
            "build",
            "--json",
            "-j",
            &threads.to_string(),
            &format!("{name}.tex"),
        ])
        .output()
        .expect("phitex build runs");
    let wall = t.elapsed().as_secs_f64();
    let so = String::from_utf8_lossy(&out.stdout);
    let ev = so
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .rfind(|v| v["event"] == "build")
        .unwrap_or_default();
    let mut diffs = compare(&dir, &rung.join(variant).join("oracle"), name);
    if !out.status.success() {
        diffs.push(format!(
            "exit {:?}: {}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
                .lines()
                .take(5)
                .collect::<Vec<_>>()
                .join(" | ")
        ));
    }
    (
        Cold {
            wall,
            ok: diffs.is_empty(),
            pages: ev["pages"].to_string(),
            nodes: ev["nodes_run"].to_string(),
            errors: ev["errors"].to_string(),
            diffs,
            phases: if threads == 1 {
                phases(&String::from_utf8_lossy(&out.stderr))
            } else {
                String::new()
            },
        },
        dir,
    )
}

fn until(lines: &mut impl Iterator<Item = String>, build: u64) -> serde_json::Value {
    for l in lines.by_ref() {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&l)
            && (v["event"] == "build" || v["event"] == "failed")
            && v["build"] == build
        {
            return v;
        }
    }
    serde_json::Value::Null
}

/// One rung: its row on standard error; the reds, empty if green.
fn run(nn: &str) -> Vec<String> {
    let (rung, name) = rung_dir(nn);
    let scope = std::env::var("PHITEX_DOC_SCOPE").unwrap_or_default();
    let mut reds = Vec::new();
    let o = rung.join(".").join("oracle");
    let (c1, _) = cold(&rung, &name, ".", 1, &format!("{nn}-cold-1"));
    let (c8, _) = cold(&rung, &name, ".", 8, &format!("{nn}-cold-8"));
    for (t, c) in [(1, &c1), (8, &c8)] {
        for d in &c.diffs {
            reds.push(format!("{nn} cold -j{t}: {d}"));
        }
    }
    // The watch: the original, then each edit as an edit of the session.
    let wdir = setup(
        &rung,
        &name,
        &format!("{nn}-watch"),
        &text(&rung, ".", &name),
    );
    let mut child = phitex(&wdir)
        .args([
            "watch",
            "--json",
            "--no-view",
            "-j",
            "8",
            &format!("{name}.tex"),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("phitex watch runs");
    let mut lines = BufReader::new(child.stdout.take().expect("its output"))
        .lines()
        .map_while(Result::ok);
    let first = until(&mut lines, 1);
    let par = &first["parallel"];
    let busy_share = par["busy_ms"].as_f64().unwrap_or(f64::NAN)
        / (par["engine_ms"].as_f64().unwrap_or(f64::NAN)
            * par["threads"].as_f64().unwrap_or(f64::NAN));
    let mut edit_cells = Vec::new();
    let variants: &[&str] = if rung.join("edit-word").exists() {
        &["edit-word", "edit-para"]
    } else {
        &[]
    };
    for (k, &v) in variants.iter().enumerate() {
        std::thread::sleep(Duration::from_millis(1100));
        std::fs::write(wdir.join(format!("{name}.tex")), text(&rung, v, &name)).expect("the edit");
        let ev = until(&mut lines, k as u64 + 2);
        let watched = std::fs::read(wdir.join(format!("{name}.pdf"))).unwrap_or_default();
        let wd = compare(&wdir, &rung.join(v).join("oracle"), &name);
        let (ce, cdir) = cold(&rung, &name, v, 8, &format!("{nn}-{v}-cold"));
        let cpdf = std::fs::read(cdir.join(format!("{name}.pdf"))).unwrap_or_default();
        let vs_cold = diff(&watched, &cpdf);
        for d in &wd {
            reds.push(format!("{nn} {v} watch: {d}"));
        }
        for d in &ce.diffs {
            reds.push(format!("{nn} {v} cold: {d}"));
        }
        if let Some(d) = &vs_cold {
            reds.push(format!("{nn} {v} watch vs cold: {d}"));
        }
        edit_cells.push(format!(
            "{v}: watch={} cold={} same={} nodes_rerun={} ms={:.1} errors={}",
            if wd.is_empty() { "yes" } else { "no" },
            if ce.ok { "yes" } else { "no" },
            if vs_cold.is_none() { "yes" } else { "no" },
            ev["nodes_run"],
            ev["wall_ms"].as_f64().unwrap_or(f64::NAN),
            ev["errors"],
        ));
    }
    if let Some(mut i) = child.stdin.take() {
        let _ = i.write_all(b"q\n");
    }
    std::thread::sleep(Duration::from_millis(500));
    let _ = child.kill();
    let _ = child.wait();
    eprintln!(
        "ROW {nn}-{name} scope={} pages={}/{} pdflatex_s={} rounds_oracle={} cold1_s={:.2} cold8_s={:.2} nodes={} identical1={} identical8={} first_diff={} watch_first_ok={} watch_cold_ms={:.0} critical_path={} busy_share8={:.3} | {} | errors={} | phases1: {}",
        if scope.is_empty() { "off" } else { "on" },
        c8.pages,
        stat(&o, "pages"),
        stat(&o, "wall_s"),
        stat(&o, "rounds"),
        c1.wall,
        c8.wall,
        c8.nodes,
        if c1.ok { "yes" } else { "no" },
        if c8.ok { "yes" } else { "no" },
        c8.diffs
            .first()
            .or(c1.diffs.first())
            .map_or("-", String::as_str),
        first["ok"],
        first["wall_ms"].as_f64().unwrap_or(f64::NAN),
        par["critical_path"],
        busy_share,
        edit_cells.join(" | "),
        c8.errors,
        c1.phases,
    );
    for r in &reds {
        eprintln!("RED {r}");
    }
    reds
}

fn rung(nn: &str) {
    let reds = run(nn);
    assert!(
        reds.is_empty(),
        "rung {nn}: {} reds:\n{}",
        reds.len(),
        reds.join("\n")
    );
}

macro_rules! rungs {
    ($($f:ident $nn:literal),*) => {$(
        #[test]
        #[ignore = "the correctness ladder: heavy, run on accl"]
        fn $f() { rung($nn); }
    )*};
}

rungs!(r01_plain "01", r02_article "02", r03_structure "03", r04_math "04",
    r05_floats "05", r06_bibtex "06", r07_index "07", r08_hyperref "08",
    r09_babel "09", r10_tikz "10", r11_biblatex "11", r12_book "12", r13_pgfmanual "13");

/// Every rung, one after another; reds of all of them at the end.
#[test]
#[ignore = "the correctness ladder: heavy, run on accl"]
fn all_rungs() {
    let mut reds = Vec::new();
    for nn in [
        "01", "02", "03", "04", "05", "06", "07", "08", "09", "10", "11", "12",
    ] {
        reds.extend(run(nn));
    }
    assert!(reds.is_empty(), "{} reds:\n{}", reds.len(), reds.join("\n"));
}
