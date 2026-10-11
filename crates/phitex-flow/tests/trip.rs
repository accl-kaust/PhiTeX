//! Knuth's TRIP test (bench/trip/tripman.tex) on the graph, as a gate.
//!
//! Pass 1 (`tex -ini`, the terminal typing `trip1.in`) and pass 2 (`tex`
//! with the format pass 1 dumped, typing `trip2.in`) are one session
//! (`phitex_flow::session`): one build whose text is `trip.tex` twice,
//! pass 2's reads resolving to what pass 1's `\dump` reached. TeX is
//! compiled with TRIP's sizes (`print::Limits::TRIP`).
//!
//! THE GATE is the document: `DVItype`'s listing of our `trip.dvi`
//! (`dvitype -output-level=2 -dpi=72.27 -page-start='*.*.*.*.*.*.*.*.*.*'`)
//! must be `bench/trip/trip.typ`, but for its header (`DVItype`'s version
//! line and the DVI file's dated comment) and the glue rounding TeX Live's
//! `triptrap/trip.diffs` allows (`down4 639342208`, `y4`/`y0
//! 203921760`). The logs (`tripin.log`, `trip.log`, `trip.fot`) are
//! diagnostics only (the user's decision, 2026-10-10): they are written to
//! `$CARGO_TARGET_TMPDIR/trip/` with the listing, for navigation.
//!
//! The listing is Knuth's, and all five schedules of `tests/dvi.rs` give
//! the same bytes.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use phitex_flow::print::Limits;
use phitex_flow::session::{Session, SessionOut};
use phitex_flow::{Font, Options, Order, Reads};

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../bench/trip")
}

fn session() -> Session {
    let d = dir();
    let tex = std::fs::read(d.join("trip.tex")).expect("bench/trip/trip.tex (bench/trip.sh fetch)");
    let tex: String = tex.iter().map(|&b| char::from(b)).collect();
    let tfm = std::fs::read(d.join("trip.tfm")).expect("bench/trip/trip.tfm");
    let font = Arc::new(Font::parse("trip", &tfm).expect("trip.tfm parses"));
    let mut terminal = Vec::new();
    for f in ["trip1.in", "trip2.in"] {
        let t = std::fs::read_to_string(d.join(f)).expect("bench/trip/trip?.in");
        terminal.extend(t.lines().map(str::to_string));
    }
    Session {
        jobs: vec![tex.clone(), tex],
        jobname: "trip".into(),
        terminal,
        interaction: 3,
        limits: Limits::TRIP,
        font_dirs: vec![d.clone()],
        input_dirs: Vec::new(),
        font,
    }
}

/// The five schedules of `tests/dvi.rs`, in `TeX82` mode (TRIP is not run in
/// e-TeX's extended mode), with web2c's main memory.
fn runs() -> [Options; 5] {
    let o = |threads, reads, order| Options { threads, reads, order, dump: false, etex: false, ..Options::default() };
    [
        o(1, Reads::Suspend, Order::Priority),
        o(8, Reads::Suspend, Order::Priority),
        o(8, Reads::Speculate, Order::Priority),
        o(8, Reads::Suspend, Order::Random(1)),
        o(8, Reads::Speculate, Order::Random(7)),
    ]
}

/// `DVItype`'s listing of a DVI file (TRIP's options), if `dvitype` runs.
fn dvitype(dvi: &[u8], tmp: &Path) -> Option<String> {
    std::fs::create_dir_all(tmp).ok()?;
    std::fs::write(tmp.join("trip.dvi"), dvi).ok()?;
    std::fs::copy(dir().join("trip.tfm"), tmp.join("trip.tfm")).ok()?;
    let o = Command::new("dvitype")
        .args(["-output-level=2", "-dpi=72.27", "-page-start=*.*.*.*.*.*.*.*.*.*", "trip.dvi"])
        .env("TFMFONTS", ".")
        .current_dir(tmp)
        .output()
        .ok()?;
    Some(String::from_utf8_lossy(&o.stdout).into_owned())
}

/// A listing with what may differ masked (TeX Live's `trip.diffs`: the
/// version line, the file's dated comment, x86 glue rounding).
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
            let mut l = l.to_string();
            for (pre, want) in [("down4 639342", "down4 639342208"), ("y4 2039217", "y4 203921760"), ("y0 2039217", "y0 203921760")]
            {
                if let Some(at) = l.find(pre) {
                    l.replace_range(at..at + want.len().min(l.len() - at), want);
                }
            }
            l
        })
        .collect()
}

/// Where the logs and the listing go, for navigation.
fn out_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("trip")
}

fn save(o: &SessionOut, typ: Option<&str>) {
    let d = out_dir();
    let _ = std::fs::create_dir_all(&d);
    let names = ["tripin.log", "trip.log"];
    for (k, l) in o.logs.iter().enumerate() {
        if let Some(n) = names.get(k) {
            let _ = std::fs::write(d.join(n), &l.log);
        }
    }
    if let Some(l) = o.logs.get(1) {
        let _ = std::fs::write(d.join("trip.fot"), &l.term);
    }
    let _ = std::fs::write(d.join("trip.dvi"), &o.dvi);
    if let Some(t) = typ {
        let _ = std::fs::write(d.join("trip.typ"), t);
    }
    for (n, t) in &o.writes {
        let _ = std::fs::write(d.join(n), t);
    }
}

#[test]
fn trip() {
    let s = session();
    let want = std::fs::read_to_string(dir().join("trip.typ")).expect("bench/trip/trip.typ");
    let want = masked(&want);
    let runs = runs();
    let first = s.run(runs[0]);
    let tmp = out_dir().join("dvitype");
    let typ = dvitype(&first.dvi, &tmp);
    save(&first, typ.as_deref());
    for &o in &runs[1..] {
        let other = s.run(o);
        assert!(other.dvi == first.dvi, "TRIP ({o:?}): the DVI file differs from one thread's");
        // (the log is not compared: the DVI is the bar)
        assert_eq!(other.writes, first.writes, "TRIP ({o:?}): the written files differ from one thread's");
    }
    let Some(typ) = typ else {
        panic!("dvitype did not run (TeX Live's dvitype is the gate's oracle)");
    };
    let got = masked(&typ);
    let at = got.iter().zip(&want).position(|(a, b)| a != b);
    let Some(i) = at.or_else(|| (got.len() != want.len()).then(|| got.len().min(want.len()))) else {
        eprintln!("TRIP: trip.typ is Knuth's ({} lines)", want.len());
        return;
    };
    panic!(
        "TRIP: trip.typ differs from line {} of {} (ours: {:?}, Knuth's: {:?}); \
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
