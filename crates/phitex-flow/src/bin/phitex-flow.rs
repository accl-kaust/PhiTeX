//! `phitex-flow FILE.tex [-j N] [--reads suspend|speculate]
//! [--order priority|random:SEED] [--tfm PATH] [-o OUT.dvi] [--dump]
//! [--edit OLD=>NEW] [--repeat N] [--check] [--log]`
//!
//! Builds FILE.tex to DVI; with `--edit`, then applies the edit (the first
//! occurrence of OLD becomes NEW) and builds again incrementally, and with
//! `--check` compares that to a cold build of the edited text. `--repeat N`
//! undoes and redoes the edit N times more before that (for profiling).
//! `--log` prints what `\message` and `\immediate\write` wrote, laid out
//! as TeX's log.

// (work shares and milliseconds for printing)
#![allow(clippy::cast_precision_loss)]

use std::process::ExitCode;
use std::sync::Arc;

use phitex_flow::{Build, Font, Options, Order, Output, Reads};

#[cfg(all(feature = "jemalloc", not(target_family = "wasm")))]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

fn usage() -> ExitCode {
    eprintln!(
        "usage: phitex-flow FILE.tex [-j N] [--reads suspend|speculate] [--order priority|random:SEED] \
         [--tfm PATH] [-o OUT.dvi] [--dump] [--edit OLD=>NEW] [--repeat N] [--check] [--log] [--main-memory WORDS]"
    );
    ExitCode::from(2)
}

fn report(what: &str, o: &Output) {
    let s = &o.stats;
    let busy = s.work.iter().filter(|&&w| w > 0).count();
    let total: u64 = s.work.iter().sum();
    let max = s.work.iter().copied().max().unwrap_or(0);
    println!(
        "{what}: {:.2} ms; {} of {} commands run; {} pieces ({} ran, {} kept, {} re-run after a changed read; {} parks, {} run by the parked thread); \
         {} line breaks run, {} kept; {} pages ({} shipped, {} kept); {} of {} threads worked, the busiest did {:.0}% of the work",
        s.wall.as_secs_f64() * 1000.0,
        s.commands_run,
        s.commands,
        s.pieces,
        s.ran,
        s.kept,
        s.reruns,
        s.parks,
        s.leapfrogs,
        s.breaks_run,
        s.breaks_reused,
        s.pages,
        s.pages_shipped,
        s.pages_reused,
        busy,
        s.work.len(),
        if total == 0 {
            0.0
        } else {
            100.0 * max as f64 / total as f64
        },
    );
    println!(
        "  serial: cut {:.1} ms, setup {:.1} ms, join {:.1} ms; by kind (all threads): reading {:.1} ms, line breaking {:.1} ms, page scan {:.1} ms, DVI {:.1} ms, log {:.1} ms, confirming {:.1} ms ({:.1} ms holding the lock)",
        s.time_cut.as_secs_f64() * 1000.0,
        s.time_setup.as_secs_f64() * 1000.0,
        s.time_join.as_secs_f64() * 1000.0,
        s.time_run.as_secs_f64() * 1000.0,
        s.time_break.as_secs_f64() * 1000.0,
        s.time_scan.as_secs_f64() * 1000.0,
        s.time_ship.as_secs_f64() * 1000.0,
        s.time_log.as_secs_f64() * 1000.0,
        s.time_confirm.as_secs_f64() * 1000.0,
        s.time_held.as_secs_f64() * 1000.0
    );
    for e in o.errors.iter().take(20) {
        eprintln!("! {e}");
    }
}

/// `phitex-flow --trip DIR [THREADS]`: Knuth's TRIP test (tripman.tex,
/// Appendix A) as one session: DIR holds `trip.tex`, `trip.tfm`, `trip1.in`
/// and `trip2.in`; `tripin.log`, `trip.log`, `trip.fot`, `trip.dvi` and the
/// files TRIP writes go there.
fn trip(dir: &str, threads: usize) -> ExitCode {
    let d = std::path::Path::new(dir);
    let read = |n: &str| std::fs::read(d.join(n)).unwrap_or_else(|e| panic!("{dir}/{n}: {e}"));
    let tex: String = read("trip.tex").iter().map(|&b| char::from(b)).collect();
    let font = Arc::new(Font::parse("trip", &read("trip.tfm")).expect("trip.tfm"));
    let mut terminal = Vec::new();
    for f in ["trip1.in", "trip2.in"] {
        terminal.extend(String::from_utf8_lossy(&read(f)).lines().map(str::to_string));
    }
    let s = phitex_flow::session::Session {
        // (a `pass2.tex` beside it is the second job instead: for looking
        // at a part of pass 2)
        // (STOPGAP, waiting on agent A's `Key::Ended`: pass 1 cut after its
        // `\next` (= `\dump`) line, as in tests/trip.rs)
        jobs: vec![
            tex.split_inclusive('\n')
                .scan(false, |done, l| {
                    if *done {
                        return None;
                    }
                    *done = l.starts_with("\\next ");
                    Some(l)
                })
                .collect(),
            std::fs::read(d.join("pass2.tex")).map_or(tex, |b| b.iter().map(|&c| char::from(c)).collect()),
        ],
        jobname: "trip".into(),
        terminal,
        interaction: 3,
        limits: phitex_flow::print::Limits::TRIP,
        font_dirs: vec![d.to_path_buf()],
        input_dirs: Vec::new(),
        font,
    };
    let o = s.run(Options { threads, etex: false, ..Options::default() });
    let put = |n: &str, b: &[u8]| std::fs::write(d.join(n), b).unwrap_or_else(|e| panic!("{dir}/{n}: {e}"));
    for (k, l) in o.logs.iter().enumerate() {
        put(if k == 0 { "tripin.log" } else { "trip.log" }, l.log.as_bytes());
        if k == 1 {
            put("trip.fot", l.term.as_bytes());
        }
    }
    put("trip.dvi", &o.dvi);
    for (n, t) in &o.writes {
        put(n, &t.chars().map(|c| u8::try_from(c).unwrap_or(b'?')).collect::<Vec<u8>>());
    }
    println!("trip: {} bytes of DVI, {} runs, {} errors on the graph", o.dvi.len(), o.runs, o.errors.len());
    ExitCode::SUCCESS
}

#[allow(clippy::too_many_lines)]
fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--trip") {
        return trip(args.get(1).map_or(".", String::as_str), args.get(2).and_then(|j| j.parse().ok()).unwrap_or(1));
    }
    let mut file = None;
    let mut opts = Options::default();
    let mut tfm = std::env::var("PHITEX_FLOW_TFM").ok();
    let mut out = None;
    let mut dump = false;
    let mut edit = None;
    let mut check = false;
    let mut repeat = 0usize;
    let mut log = false;
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        let mut val = || {
            i += 1;
            args.get(i).cloned()
        };
        match a {
            "-j" => match val().and_then(|v| v.parse().ok()) {
                Some(n) => opts.threads = n,
                None => return usage(),
            },
            "--reads" => match val().as_deref() {
                Some("suspend") => opts.reads = Reads::Suspend,
                Some("speculate") => opts.reads = Reads::Speculate,
                _ => return usage(),
            },
            "--order" => match val() {
                Some(v) if v == "priority" => opts.order = Order::Priority,
                Some(v) if v.starts_with("random:") => match v["random:".len()..].parse() {
                    Ok(s) => opts.order = Order::Random(s),
                    Err(_) => return usage(),
                },
                _ => return usage(),
            },
            "--tfm" => tfm = val(),
            "-o" => out = val(),
            "--dump" => {
                dump = true;
                opts.dump = true;
            }
            "--check" => check = true,
            "--repeat" => match val().and_then(|v| v.parse().ok()) {
                Some(n) => repeat = n,
                None => return usage(),
            },
            "--log" => log = true,
            "--main-memory" => match val().and_then(|v| v.parse().ok()) {
                Some(n) => opts.main_memory = n,
                None => return usage(),
            },
            "--edit" => match val() {
                Some(v) if v.contains("=>") => {
                    let (a, b) = v.split_once("=>").expect("=>");
                    edit = Some((a.to_string(), b.to_string()));
                }
                _ => return usage(),
            },
            _ if file.is_none() && !a.starts_with('-') => file = Some(a.to_string()),
            _ => return usage(),
        }
        i += 1;
    }
    let Some(file) = file else { return usage() };
    let tfm = tfm.or_else(|| {
        [
            "crates/partex-core/testdata/cmr10.tfm",
            "/usr/share/texmf-dist/fonts/tfm/public/cm/cmr10.tfm",
            "/usr/share/texlive/texmf-dist/fonts/tfm/public/cm/cmr10.tfm",
        ]
        .iter()
        .find(|p| std::path::Path::new(p).exists())
        .map(|p| (*p).to_string())
    });
    let Some(tfm) = tfm else {
        eprintln!("no cmr10.tfm found: pass --tfm PATH");
        return ExitCode::FAILURE;
    };
    let font = match std::fs::read(&tfm)
        .map_err(|e| e.to_string())
        .and_then(|b| Font::parse("cmr10", &b))
    {
        Ok(f) => Arc::new(f),
        Err(e) => {
            eprintln!("{tfm}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let src = match std::fs::read_to_string(&file) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{file}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let out = out.unwrap_or_else(|| format!("{}.dvi", file.trim_end_matches(".tex")));
    // (`\input` and `\openin` find files beside the document; the job is
    // its name)
    let path = std::path::Path::new(&file);
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).map_or_else(|| ".".into(), std::path::Path::to_path_buf);
    let job = path.file_stem().map_or_else(|| "texput".to_string(), |s| s.to_string_lossy().into_owned());
    let mut b = Build::new(font.clone());
    b.set_input_dirs(vec![dir.clone()]);
    b.set_jobname(&job);
    let cold = b.update(&src, opts);
    report("cold", &cold);
    if dump {
        print!("{}", cold.dump);
    }
    if log {
        println!("{}", phitex_flow::print::log("", &cold.log, ""));
    }
    let mut last = cold;
    if let Some((from, to)) = edit {
        let Some(at) = src.find(&from) else {
            eprintln!("--edit: {from:?} is not in the file");
            return ExitCode::FAILURE;
        };
        let edited = format!("{}{}{}", &src[..at], to, &src[at + from.len()..]);
        for _ in 0..repeat {
            b.update(&edited, opts);
            b.update(&src, opts);
        }
        let e = b.update(&edited, opts);
        report("edit", &e);
        if dump {
            print!("{}", e.dump);
        }
        if check {
            let mut fb = Build::new(font);
            fb.set_input_dirs(vec![dir.clone()]);
            fb.set_jobname(&job);
            let fresh = fb.update(&edited, opts);
            report("cold of the edited text", &fresh);
            println!(
                "edit == cold: {}",
                if fresh.dvi == e.dvi { "yes" } else { "NO" }
            );
        }
        last = e;
    }
    if let Err(e) = std::fs::write(&out, &last.dvi) {
        eprintln!("{out}: {e}");
        return ExitCode::FAILURE;
    }
    // (what `\openout` and `\write` made, beside the document)
    for (name, text) in &last.writes {
        let p = dir.join(name);
        if let Err(e) = std::fs::write(&p, text.chars().map(|c| u32::from(c).min(255) as u8).collect::<Vec<u8>>()) {
            eprintln!("{}: {e}", p.display());
            return ExitCode::FAILURE;
        }
    }
    println!("wrote {out} ({} bytes)", last.dvi.len());
    ExitCode::SUCCESS
}
