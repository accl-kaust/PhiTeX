//! The e-TeX test e-TRIP (`bench/etrip/etripman.tex`, part 2) on the
//! graph, as a gate.
//!
//! Pass 1 (e-IniTeX, the terminal typing `etrip2.in`: `*etrip`, extended
//! mode) and pass 2 (the format pass 1 dumped, typing `etrip3.in`: `&etrip
//! etrip`) are one session (`phitex_flow::session`), in e-TeX's extended
//! mode, compiled with e-TRIP's sizes (`texmf.cnf`: TRIP's,
//! `print::Limits::TRIP`).
//!
//! THE GATE is the document: `DVItype`'s listing of our `etrip.dvi` must be
//! `bench/etrip/etrip.typ` but for its header (TeX Live's
//! `etexdir/etrip/etrip.diffs`). The logs (`etripin.log`, `etrip.log`,
//! `etrip.fot`) and `etrip.out` are diagnostics only: they are written to
//! `$CARGO_TARGET_TMPDIR/etrip/` with the listing, for navigation.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use phitex_flow::print::Limits;
use phitex_flow::session::{Session, SessionOut};
use phitex_flow::{Font, Options, Order, Reads};

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../bench/etrip")
}

fn session() -> Session {
    let d = dir();
    let tex = std::fs::read(d.join("etrip.tex")).expect("bench/etrip/etrip.tex");
    let tex: String = tex.iter().map(|&b| char::from(b)).collect();
    let tfm = std::fs::read(d.join("etrip.tfm")).expect("bench/etrip/etrip.tfm");
    let font = Arc::new(Font::parse("etrip", &tfm).expect("etrip.tfm parses"));
    let mut terminal = Vec::new();
    for f in ["etrip2.in", "etrip3.in"] {
        let t = std::fs::read_to_string(d.join(f)).expect("bench/etrip/etrip?.in");
        terminal.extend(t.lines().map(str::to_string));
    }
    Session {
        jobs: vec![tex.clone(), tex],
        jobname: "etrip".into(),
        terminal,
        interaction: 3,
        limits: Limits::TRIP,
        font_dirs: vec![d.clone()],
        input_dirs: Vec::new(),
        font,
    }
}

/// The five schedules of `tests/dvi.rs`, in e-TeX's extended mode, with
/// web2c's main memory.
fn runs() -> [Options; 5] {
    let o = |threads, reads, order| Options { threads, reads, order, dump: false, etex: true, ..Options::default() };
    [
        o(1, Reads::Suspend, Order::Priority),
        o(8, Reads::Suspend, Order::Priority),
        o(8, Reads::Speculate, Order::Priority),
        o(8, Reads::Suspend, Order::Random(1)),
        o(8, Reads::Speculate, Order::Random(7)),
    ]
}

/// `DVItype`'s listing of a DVI file (e-TRIP's options), if `dvitype` runs.
fn dvitype(dvi: &[u8], tmp: &Path) -> Option<String> {
    std::fs::create_dir_all(tmp).ok()?;
    std::fs::write(tmp.join("etrip.dvi"), dvi).ok()?;
    std::fs::copy(dir().join("etrip.tfm"), tmp.join("etrip.tfm")).ok()?;
    let o = Command::new("dvitype")
        .args(["-output-level=2", "-dpi=72.27", "-page-start=*.*.*.*.*.*.*.*.*.*", "etrip.dvi"])
        .env("TFMFONTS", ".")
        .current_dir(tmp)
        .output()
        .ok()?;
    Some(String::from_utf8_lossy(&o.stdout).into_owned())
}

/// A listing with what may differ masked (TeX Live's `etrip.diffs`: the
/// version line, the file's dated comment).
fn masked(typ: &str) -> Vec<String> {
    typ.lines()
        .enumerate()
        .map(|(i, l)| {
            if i == 0 && l.starts_with("This is DVItype") {
                return "This is DVItype".to_string();
            }
            if l.starts_with("' TeX output ") {
                return "' TeX output YYYY.MM.DD:hhmm'".to_string();
            }
            l.to_string()
        })
        .collect()
}

/// Where the logs and the listing go, for navigation.
fn out_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("etrip")
}

fn save(o: &SessionOut, typ: Option<&str>) {
    let d = out_dir();
    let _ = std::fs::create_dir_all(&d);
    let names = ["etripin.log", "etrip.log"];
    for (k, l) in o.logs.iter().enumerate() {
        if let Some(n) = names.get(k) {
            let _ = std::fs::write(d.join(n), &l.log);
        }
    }
    if let Some(l) = o.logs.get(1) {
        let _ = std::fs::write(d.join("etrip.fot"), &l.term);
    }
    let _ = std::fs::write(d.join("etrip.dvi"), &o.dvi);
    if let Some(t) = typ {
        let _ = std::fs::write(d.join("etrip.typ"), t);
    }
    for (n, t) in &o.writes {
        let _ = std::fs::write(d.join(n), t);
    }
}

#[test]
fn etrip() {
    let s = session();
    let want = std::fs::read_to_string(dir().join("etrip.typ")).expect("bench/etrip/etrip.typ");
    let want = masked(&want);
    let runs = runs();
    let first = s.run(runs[0]);
    let tmp = out_dir().join("dvitype");
    let typ = dvitype(&first.dvi, &tmp);
    save(&first, typ.as_deref());
    for &o in &runs[1..] {
        let other = s.run(o);
        assert!(other.dvi == first.dvi, "e-TRIP ({o:?}): the DVI file differs from one thread's");
        // (the log is not compared: the DVI is the bar)
        assert_eq!(other.writes, first.writes, "e-TRIP ({o:?}): the written files differ from one thread's");
    }
    let Some(typ) = typ else {
        panic!("dvitype did not run (TeX Live's dvitype is the gate's oracle)");
    };
    let got = masked(&typ);
    let at = got.iter().zip(&want).position(|(a, b)| a != b);
    let Some(i) = at.or_else(|| (got.len() != want.len()).then(|| got.len().min(want.len()))) else {
        eprintln!("e-TRIP: etrip.typ is Knuth's ({} lines)", want.len());
        return;
    };
    panic!(
        "e-TRIP: etrip.typ differs from line {} of {} (ours: {:?}, Knuth's: {:?}); \
         {} pages of DVI, {} bytes; outputs in {}",
        i + 1,
        want.len(),
        got.get(i),
        want.get(i),
        typ.matches("beginning of page").count(),
        first.dvi.len(),
        out_dir().display()
    );
}
