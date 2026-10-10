//! The incremental fuzz gate (`phitex_flow::fuzz`): random edits in
//! sequence on the corpus (the plain documents, a large one, TRIP's pass
//! 2), each build that saw the edits equal to a cold build of the same text
//! (DVI, log, written files) at a random schedule, lean or not; and, where
//! the builds keep their nodes, no more nodes run than the readers of what
//! changed and the edited pieces hold. A failure's reproduction goes to
//! `$CARGO_TARGET_TMPDIR/fuzz/` (`phitex-flow --fuzz-replay FILE`).
//!
//! `PHITEX_FUZZ_SEEDS` (default 24) seeds of `PHITEX_FUZZ_EDITS` (6) edits
//! here; `phitex-flow --fuzz SEED COUNT SEEDS` for thousands.
//!
//! And TRIP edited: TeX (`tex`, the system's) is the oracle of a sample of
//! edited `trip.tex`s, `DVItype`'s listings compared.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use phitex_flow::fuzz::{self, Config, Rng};
use phitex_flow::print::Limits;
use phitex_flow::session::Session;
use phitex_flow::{Options, Order, Reads};

/// Documents whose every update may differ from a cold build whatever the
/// edit (a no-op edit fails too), each an open finding in CORE.md ("Fuzz
/// findings"): such a seed is reported, not counted, while it stays open.
const NOOP_OPEN: [(&str, &str); 1] = [("trip", "F1: a second update of TRIP differs from a cold build, edit or not")];

/// Whether a no-op edit fails too, at 1 thread in priority order (where
/// F1 always shows; the 8-thread schedule of the failure may hide it).
fn noop_fails(r: &fuzz::Repro) -> bool {
    let mut n = r.noop();
    for s in [&mut n.before, &mut n.schedule] {
        s.threads = 1;
        s.reads = phitex_flow::Reads::Suspend;
        s.order = phitex_flow::Order::Priority;
    }
    fuzz::replay(&n).is_some()
}

fn env_num(name: &str, default: u64) -> u64 {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

fn out_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("fuzz")
}

#[test]
fn edits_equal_cold_builds() {
    let corpus = Arc::new(fuzz::corpus());
    assert!(corpus.len() >= 6, "the corpus: {} documents", corpus.len());
    let seeds = env_num("PHITEX_FUZZ_SEEDS", 24);
    let edits = usize::try_from(env_num("PHITEX_FUZZ_EDITS", 6)).unwrap_or(6);
    let cfg = Config {
        threads: vec![1, 8],
        over: true,
        strict: false,
        timeout: Duration::from_mins(15),
        doc: None,
    };
    let mut bad = Vec::new();
    for seed in 0..seeds {
        let Some(r) = fuzz::run_seed_timed(&corpus, seed, edits, &cfg) else { continue };
        let p = fuzz::save(&out_dir(), &r);
        let line = format!("seed {seed} ({}, edit {} {}): {:?} -> {}", r.doc, r.step, r.edit.what, r.fail, p.display());
        let open = NOOP_OPEN.iter().find(|(d, _)| *d == r.doc);
        match open {
            Some((_, why)) if !matches!(r.fail, fuzz::Fail::Over { .. }) && noop_fails(&r) => {
                eprintln!("open: {line} [{why}]");
            }
            _ => bad.push(line),
        }
    }
    assert!(bad.is_empty(), "fuzz failures:\n{}", bad.join("\n"));
}

/// The open findings' reproductions (`tests/fuzz/open/`, CORE.md "Fuzz
/// findings") fail still: one that passes is fixed, and goes to
/// `tests/fuzz/fixed/`, whose reproductions must pass.
#[test]
fn open_findings_fail_fixed_ones_pass() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fuzz");
    let read = |sub: &str| -> Vec<(PathBuf, fuzz::Repro)> {
        let Ok(rd) = std::fs::read_dir(dir.join(sub)) else { return Vec::new() };
        let mut v: Vec<_> = rd
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "repro"))
            .map(|p| {
                let r = fuzz::Repro::parse(&std::fs::read_to_string(&p).unwrap()).unwrap_or_else(|| panic!("{}", p.display()));
                (p, r)
            })
            .collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    };
    let open = read("open");
    assert!(!open.is_empty() || dir.join("fixed").exists(), "no reproductions");
    for (p, r) in open {
        let got = fuzz::replay(&r);
        eprintln!("{}: {got:?}", p.display());
        assert!(got.is_some(), "{} passes now: fixed, move it to tests/fuzz/fixed/ (and close it in CORE.md)", p.display());
    }
    for (p, r) in read("fixed") {
        assert_eq!(fuzz::replay(&r).map(|f| format!("{f:?}")), None, "{} fails again", p.display());
    }
}

/// A reproduction written and read back is the same edit.
#[test]
fn repro_round_trips() {
    let corpus = fuzz::corpus();
    let doc = corpus.iter().find(|d| d.name == "story").expect("story");
    let mut rng = Rng::new(7);
    let edit = fuzz::random_edit(&mut rng, doc, &doc.text, &doc.files).expect("an edit");
    let s = fuzz::Schedule::random(&mut rng, &[1, 8], None);
    let r = fuzz::Repro {
        doc: doc.name.clone(),
        seed: 7,
        step: 1,
        before: s,
        schedule: s,
        fail: fuzz::Fail::Mismatch("dvi".into()),
        text: doc.text.clone(),
        files: doc.files.clone(),
        edit,
        threads: vec![1, 8],
        only: None,
    };
    let back = fuzz::Repro::parse(&r.write()).expect("parses");
    assert_eq!(back.text, r.text);
    assert_eq!(back.files, r.files);
    assert_eq!((back.edit.at, back.edit.del, &back.edit.ins), (r.edit.at, r.edit.del, &r.edit.ins));
    assert_eq!(fuzz::replay(&back).map(|f| format!("{f:?}")), None, "the edit passes");
}

// ------------------------------------------------------------ TRIP edited

fn trip_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../bench/trip")
}

/// `DVItype`'s listing (TRIP's options) from the first page, as `bench/trip.sh` compares.
fn listing(dir: &Path, dvi: &str) -> Option<String> {
    let o = Command::new("dvitype")
        .args(["-output-level=2", "-dpi=72.27", "-page-start=*.*.*.*.*.*.*.*.*.*", dvi])
        .env("TFMFONTS", ".")
        .current_dir(dir)
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&o.stdout).into_owned();
    let from = s.find("beginning of page")?;
    Some(s[from..].to_string())
}

/// TeX's DVI of TRIP's two passes (`orig` the first's `trip.tex`, `tex` the second's), in `dir`.
fn tex_trip(dir: &Path, orig: &str, tex: &str) -> Option<String> {
    std::fs::create_dir_all(dir).ok()?;
    let d = trip_dir();
    for f in ["trip.tfm", "trip1.in", "trip2.in"] {
        std::fs::copy(d.join(f), dir.join(f)).ok()?;
    }
    for (args, input, text) in [(vec!["-ini"], "trip1.in", orig), (vec![], "trip2.in", tex)] {
        std::fs::write(dir.join("trip.tex"), text.chars().map(|c| u8::try_from(c).unwrap_or(b'?')).collect::<Vec<u8>>()).ok()?;
        let stdin = std::fs::File::open(dir.join(input)).ok()?;
        Command::new("timeout")
            .arg("120")
            .arg("tex")
            .args(&args)
            .arg("-interaction=batchmode")
            .env("TFMFONTS", ".")
            .env("TEXFORMATS", ".")
            .env("TEXINPUTS", ".")
            .current_dir(dir)
            .stdin(stdin)
            .output()
            .ok()?;
    }
    listing(dir, "trip.dvi")
}

/// Ours, the same edited text as TRIP's session.
fn flow_trip(dir: &Path, orig: &str, pass2: &str) -> Option<String> {
    let d = trip_dir();
    let tfm = std::fs::read(d.join("trip.tfm")).ok()?;
    let font = Arc::new(phitex_flow::Font::parse("trip", &tfm).ok()?);
    let mut terminal = Vec::new();
    for f in ["trip1.in", "trip2.in"] {
        terminal.extend(std::fs::read_to_string(d.join(f)).ok()?.lines().map(str::to_string));
    }
    let s = Session {
        jobs: vec![orig.to_string(), pass2.to_string()],
        jobname: "trip".into(),
        terminal,
        interaction: 3,
        limits: Limits::TRIP,
        font_dirs: vec![d.clone()],
        input_dirs: Vec::new(),
        font,
    };
    let o = s.run(Options {
        threads: 1,
        reads: Reads::Suspend,
        order: Order::Priority,
        dump: false,
        etex: false,
        ..Options::default()
    });
    std::fs::create_dir_all(dir).ok()?;
    std::fs::copy(d.join("trip.tfm"), dir.join("trip.tfm")).ok()?;
    std::fs::write(dir.join("flow.dvi"), &o.dvi).ok()?;
    listing(dir, "flow.dvi")
}

/// A sample of edited `trip.tex`s: ours as TeX's (`PHITEX_FUZZ_TRIP`
/// seeds, default 4; none if `tex` or `dvitype` is not there).
#[test]
fn edited_trip_is_tex() {
    let Some(doc) = fuzz::trip_doc(None) else { return };
    let orig = std::fs::read(trip_dir().join("trip.tex")).expect("trip.tex");
    let orig: String = orig.iter().map(|&b| char::from(b)).collect();
    let mut bad = Vec::new();
    for seed in 0..env_num("PHITEX_FUZZ_TRIP", 4) {
        let mut rng = Rng::new(1000 + seed);
        let mut text = doc.text.clone();
        let mut files = Vec::new();
        for _ in 0..3 {
            if let Some(e) = fuzz::random_edit(&mut rng, &doc, &text, &files) {
                e.apply(&mut text, &mut files);
            }
        }
        // (pass 2's text: after the last job's mark)
        let at = text.rfind(phitex_flow::graph::JOB).map_or(0, |i| i + phitex_flow::graph::JOB.len_utf8());
        let pass2 = text[at..].to_string();
        let dir = out_dir().join(format!("trip-{seed}"));
        let Some(want) = tex_trip(&dir.join("tex"), &orig, &pass2) else {
            eprintln!("edited_trip_is_tex: no tex or dvitype here");
            return;
        };
        let got = flow_trip(&dir.join("flow"), &orig, &pass2).unwrap_or_default();
        if got != want {
            let i = got.lines().zip(want.lines()).position(|(a, b)| a != b).unwrap_or(0);
            bad.push(format!(
                "seed {seed}: listing differs from line {} (ours {:?}, TeX's {:?}); texts in {}",
                i + 1,
                got.lines().nth(i),
                want.lines().nth(i),
                dir.display()
            ));
        }
    }
    assert!(bad.is_empty(), "edited TRIP differs from TeX:\n{}", bad.join("\n"));
}

// ------------------------------------------------------------ LaTeX edited

/// A DVI file with its preamble's comment (the date) masked.
fn masked(dvi: &[u8]) -> Vec<u8> {
    let mut d = dvi.to_vec();
    if d.len() > 15 {
        let k = usize::from(d[14]);
        for b in d.iter_mut().skip(15).take(k) {
            *b = b'.';
        }
    }
    d
}

/// `latex NAME` on `text` (its first run, no `.aux`): the DVI file and the
/// `.aux` written, if there is a `latex`.
fn run_latex(dir: &Path, name: &str, text: &str) -> Option<(Vec<u8>, String)> {
    std::fs::create_dir_all(dir).ok()?;
    let _ = std::fs::remove_file(dir.join(format!("{name}.aux")));
    std::fs::write(dir.join(format!("{name}.tex")), text.chars().map(|c| u8::try_from(c).unwrap_or(b'?')).collect::<Vec<u8>>()).ok()?;
    Command::new("timeout")
        .args(["120", "latex", "-interaction=batchmode", &format!("{name}.tex")])
        .env("SOURCE_DATE_EPOCH", "0")
        .env("FORCE_SOURCE_DATE", "1")
        .current_dir(dir)
        .output()
        .ok()?;
    let dvi = std::fs::read(dir.join(format!("{name}.dvi"))).ok()?;
    let aux = std::fs::read(dir.join(format!("{name}.aux"))).ok()?;
    Some((dvi, aux.iter().map(|&b| char::from(b)).collect()))
}

/// A LaTeX document edited (`tests/fuzz/latex/tiny.tex`, latex.ltx as
/// the prelude until the saved format: one schedule, a build of about a
/// minute): the edit's build equals a cold build of the edited text
/// (DVI, log, written files), and `latex` itself on its first run (the
/// DVI, the `.aux`). `PHITEX_FUZZ_LATEX` seeds of one edit (1); none
/// without TeX Live's `latex.ini`.
#[test]
fn latex_edits_equal_cold_and_latex() {
    let Some(doc) = fuzz::latex_doc("tiny") else { return };
    let o = phitex_flow::Options { threads: 8, ..phitex_flow::Options::default() };
    for seed in 0..env_num("PHITEX_FUZZ_LATEX", 1) {
        let mut rng = Rng::new(seed);
        let edit = fuzz::random_edit(&mut rng, &doc, &doc.text, &doc.files).expect("an edit");
        let mut text = doc.text.clone();
        let mut files = doc.files.clone();
        edit.apply(&mut text, &mut files);
        let mut b = fuzz::builder(&doc, &doc.files);
        b.update(&doc.text, o);
        let inc = b.update(&text, o);
        drop(b);
        let cold = fuzz::cold(&doc, &text, &files);
        let same = inc.dvi == cold.dvi && inc.log == cold.log && inc.writes == cold.writes;
        assert!(same, "seed {seed} ({}): the edit's build is not the cold build's", edit.what);
        // (and latex's: the document's own text, after its job's mark)
        let at = text.rfind(phitex_flow::graph::JOB).map_or(0, |i| i + phitex_flow::graph::JOB.len_utf8());
        let Some((dvi, aux)) = run_latex(&out_dir().join(format!("latex-{seed}")), "tiny", &text[at..]) else {
            eprintln!("latex_edits_equal_cold_and_latex: no latex here");
            return;
        };
        let ours = inc.writes.iter().find(|(n, _)| n == "tiny.aux").map(|(_, t)| t.clone()).unwrap_or_default();
        assert!(masked(&inc.dvi) == masked(&dvi), "seed {seed} ({}): the DVI is not latex's", edit.what);
        assert_eq!(ours, aux, "seed {seed} ({}): the .aux is not latex's", edit.what);
    }
}
