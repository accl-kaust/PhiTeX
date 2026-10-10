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
         [--tfm PATH] [-o OUT.dvi] [--dump] [--etex] [--input-dir DIR]... [--jobname NAME] [--before FILE]... [--terminal LINE]... [--font-dir DIR]... [--fmt FILE] [--dump-fmt FILE] [--interaction MODE] [--edit OLD=>NEW] [--repeat N] \
         [--check] [--log] [--main-memory WORDS]"
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
    if s.rounds > 1 {
        let walls: Vec<String> = s
            .round_walls
            .iter()
            .map(|d| format!("{:.0}", d.as_secs_f64() * 1000.0))
            .collect();
        println!("  {} rounds (ms): {}", s.rounds, walls.join(" "));
    }
    for e in o.errors.iter().take(20) {
        eprintln!("! {e}");
    }
}

/// `phitex-flow --trip DIR [THREADS]`: Knuth's TRIP test (tripman.tex,
/// Appendix A) as one session: DIR holds `trip.tex`, `trip.tfm`, `trip1.in`
/// and `trip2.in`; `tripin.log`, `trip.log`, `trip.fot`, `trip.dvi` and the
/// files TRIP writes go there. A DIR holding `etrip.tex` runs e-TRIP
/// instead (etripman.tex, part 2: `etrip.tfm`, `etrip2.in`, `etrip3.in`,
/// extended mode), its outputs named `etrip`.
fn trip(dir: &str, threads: usize) -> ExitCode {
    let d = std::path::Path::new(dir);
    let read = |n: &str| std::fs::read(d.join(n)).unwrap_or_else(|e| panic!("{dir}/{n}: {e}"));
    let e = d.join("etrip.tex").exists();
    let (job, ins) = if e { ("etrip", ["etrip2.in", "etrip3.in"]) } else { ("trip", ["trip1.in", "trip2.in"]) };
    let tex: String = read(&format!("{job}.tex")).iter().map(|&b| char::from(b)).collect();
    let font = Arc::new(Font::parse(job, &read(&format!("{job}.tfm"))).expect("the TFM file"));
    let mut terminal = Vec::new();
    for f in ins {
        terminal.extend(String::from_utf8_lossy(&read(f)).lines().map(str::to_string));
    }
    let s = phitex_flow::session::Session {
        // (a `pass2.tex` beside it is the second job instead: for looking
        // at a part of pass 2)
        jobs: vec![
            tex.clone(),
            std::fs::read(d.join("pass2.tex")).map_or(tex, |b| b.iter().map(|&c| char::from(c)).collect()),
        ],
        jobname: job.into(),
        terminal,
        interaction: 3,
        limits: phitex_flow::print::Limits::TRIP,
        font_dirs: vec![d.to_path_buf()],
        input_dirs: Vec::new(),
        font,
    };
    let o = s.run(Options { threads, etex: e, ..Options::default() });
    let put = |n: &str, b: &[u8]| std::fs::write(d.join(n), b).unwrap_or_else(|e| panic!("{dir}/{n}: {e}"));
    for (k, l) in o.logs.iter().enumerate() {
        put(&if k == 0 { format!("{job}in.log") } else { format!("{job}.log") }, l.log.as_bytes());
        if k == 1 {
            put(&format!("{job}.fot"), l.term.as_bytes());
        }
    }
    put(&format!("{job}.dvi"), &o.dvi);
    for (n, t) in &o.writes {
        put(n, &t.chars().map(|c| u8::try_from(c).unwrap_or(b'?')).collect::<Vec<u8>>());
    }
    println!("{job}: {} bytes of DVI, {} runs, {} errors on the graph", o.dvi.len(), o.runs, o.errors.len());
    ExitCode::SUCCESS
}

/// `phitex-flow --fuzz SEED COUNT [SEEDS] [--threads N,..] [--doc NAME]
/// [--out DIR] [--no-over] [--strict-over] [--timeout SECS]`: the incremental fuzz gate
/// (`phitex_flow::fuzz`): SEEDS seeds from SEED (1), COUNT edits each; a
/// failing seed's reproduction goes to DIR (`fuzz-out`).
fn fuzz(args: &[String]) -> ExitCode {
    use phitex_flow::fuzz;
    let num = |i: usize| args.get(i).and_then(|a| a.parse::<u64>().ok());
    let (Some(seed), Some(count)) = (num(0), num(1)) else {
        eprintln!("usage: phitex-flow --fuzz SEED COUNT [SEEDS] [--threads N,..] [--doc NAME] [--out DIR] [--no-over] [--timeout SECS]");
        return ExitCode::FAILURE;
    };
    let mut seeds = 1;
    let mut cfg = fuzz::Config::default();
    let mut out = std::path::PathBuf::from("fuzz-out");
    let mut i = 2;
    if let Some(n) = num(2) {
        seeds = n;
        i = 3;
    }
    while i < args.len() {
        let val = args.get(i + 1).cloned().unwrap_or_default();
        match args[i].as_str() {
            "--threads" => cfg.threads = val.split(',').filter_map(|t| t.parse().ok()).collect(),
            "--doc" => cfg.doc = Some(val),
            "--out" => out = val.into(),
            "--timeout" => cfg.timeout = std::time::Duration::from_secs(val.parse().unwrap_or(600)),
            "--strict-over" => {
                cfg.strict = true;
                i += 1;
                continue;
            }
            "--no-over" => {
                cfg.over = false;
                i += 1;
                continue;
            }
            a => {
                eprintln!("--fuzz: unknown option {a}");
                return ExitCode::FAILURE;
            }
        }
        i += 2;
    }
    let corpus = Arc::new(fuzz::corpus());
    let (mut fails, mut over, mut ok) = (0, 0, 0);
    for s in seed..seed + seeds {
        match fuzz::run_seed_timed(&corpus, s, usize::try_from(count).unwrap_or(usize::MAX), &cfg) {
            None => ok += 1,
            Some(r) => {
                let p = fuzz::save(&out, &r);
                if r.fail.is_over() {
                    over += 1;
                } else {
                    fails += 1;
                }
                // (a failure a no-op edit has too is the update's, not the edit's)
                let noop = !r.fail.is_over() && r.step > 0 && fuzz::replay(&r.noop()).is_some();
                println!(
                    "seed {s} ({}, edit {}: {}): {:?}{} -> {}",
                    r.doc,
                    r.step,
                    r.edit.what,
                    r.fail,
                    if noop { " (a no-op edit fails too)" } else { "" },
                    p.display()
                );
            }
        }
    }
    println!("fuzz: {seeds} seeds of {count} edits from {seed}: {ok} passed, {fails} failed, {over} over-invalidated");
    // (a seed still running past its time is left behind: exit at once)
    std::process::exit(i32::from(fails + over > 0));
}

/// `phitex-flow --fuzz-orders DOC FILE A B [DIR]`: two cold builds of FILE's
/// text, as corpus document DOC is built, at 1 worker in orders A and B
/// (`priority`, `random:N`): "same" or "differs: WHAT" (exit 1); DIR gets
/// both graphs and logs (a.dump, b.dump, a.log, b.log).
fn fuzz_orders(args: &[String]) -> ExitCode {
    use phitex_flow::fuzz;
    let (Some(doc), Some(file), Some(first), Some(second)) = (
        args.first(),
        args.get(1),
        args.get(2).and_then(|s| fuzz::parse_order(s)),
        args.get(3).and_then(|s| fuzz::parse_order(s)),
    ) else {
        eprintln!("usage: phitex-flow --fuzz-orders DOC FILE ORDER ORDER [DIR]");
        return ExitCode::FAILURE;
    };
    let corpus = fuzz::corpus();
    let Some(found) = corpus.iter().find(|x| x.name == *doc) else {
        eprintln!("--fuzz-orders: no document {doc}");
        return ExitCode::FAILURE;
    };
    // (`-`: the document's own text)
    let read = if file == "-" { Ok(found.text.clone()) } else { std::fs::read_to_string(file) };
    let Ok(text) = read else {
        eprintln!("--fuzz-orders: {file}: unreadable");
        return ExitCode::FAILURE;
    };
    let dir = args.get(4);
    let (w, x, y) = fuzz::orders_differ(found, &text, first, second, dir.is_some());
    if let Some(dir) = dir {
        let dir = std::path::Path::new(dir);
        let _ = std::fs::create_dir_all(dir);
        let _ = std::fs::write(dir.join("a.dump"), &x.dump);
        let _ = std::fs::write(dir.join("b.dump"), &y.dump);
        let _ = std::fs::write(dir.join("a.log"), x.log.join("\n"));
        let _ = std::fs::write(dir.join("b.log"), y.log.join("\n"));
    }
    match w {
        None => {
            println!("same");
            ExitCode::SUCCESS
        }
        Some(w) => {
            println!("differs: {w}");
            ExitCode::FAILURE
        }
    }
}

/// `phitex-flow --budgets [REPS] [DOC]`: the edit budgets' table
/// (`phitex_flow::budgets`), each edit kind REPS times (5).
fn budgets(args: &[String]) -> ExitCode {
    let reps = args.first().and_then(|r| r.parse().ok()).unwrap_or(5);
    let rows = phitex_flow::budgets::run_all(reps, args.get(1).map(String::as_str));
    print!("{}", phitex_flow::budgets::table(&rows));
    ExitCode::SUCCESS
}

/// `phitex-flow --fuzz-classify FILE...`: each reproduction's class
/// (`fuzz::classify`), a line `CLASS<TAB>FILE`.
fn fuzz_classify(files: &[String]) -> ExitCode {
    for f in files {
        match std::fs::read_to_string(f).ok().and_then(|s| phitex_flow::fuzz::Repro::parse(&s)) {
            Some(r) => println!("{}\t{f}", phitex_flow::fuzz::classify(&r)),
            None => println!("unreadable\t{f}"),
        }
    }
    ExitCode::SUCCESS
}

/// `phitex-flow --fuzz-replay FILE`: a reproduction run again.
fn fuzz_replay(file: &str) -> ExitCode {
    let Some(r) = std::fs::read_to_string(file).ok().and_then(|s| phitex_flow::fuzz::Repro::parse(&s)) else {
        eprintln!("--fuzz-replay: {file}: not a reproduction");
        return ExitCode::FAILURE;
    };
    if let Some(f) = phitex_flow::fuzz::replay(&r) {
        println!("{file}: fails: {f:?}");
        return ExitCode::FAILURE;
    }
    // (the last edit alone passes: the seed again, with what came before it)
    match phitex_flow::fuzz::replay_history(&r) {
        None => {
            println!("{file}: passes now");
            ExitCode::SUCCESS
        }
        Some(h) => {
            println!("{file}: the last edit alone passes; with its history, edit {} fails: {:?}", h.step, h.fail);
            ExitCode::FAILURE
        }
    }
}

#[allow(clippy::too_many_lines)]
fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--fuzz") {
        return fuzz(&args[1..]);
    }
    if args.first().map(String::as_str) == Some("--budgets") {
        return budgets(&args[1..]);
    }
    if args.first().map(String::as_str) == Some("--fuzz-classify") {
        return fuzz_classify(&args[1..]);
    }
    if args.first().map(String::as_str) == Some("--fuzz-orders") {
        return fuzz_orders(&args[1..]);
    }
    if args.first().map(String::as_str) == Some("--fuzz-replay") {
        return fuzz_replay(args.get(1).map_or("", String::as_str));
    }
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
    let mut dirs: Vec<std::path::PathBuf> = Vec::new();
    let mut jobname = None;
    let mut before: Vec<String> = Vec::new();
    let mut terminal: Vec<String> = Vec::new();
    let mut font_dirs: Vec<std::path::PathBuf> = Vec::new();
    let mut fmt: Option<String> = None;
    let mut dump_fmt: Option<String> = None;
    let mut interaction: Option<i32> = None;
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
            "--etex" => opts.etex = true,
            "--input-dir" => match val() {
                Some(d) => dirs.push(d.into()),
                None => return usage(),
            },
            "--jobname" => jobname = val(),
            "--before" => match val() {
                Some(f) => before.push(f),
                None => return usage(),
            },
            "--terminal" => match val() {
                Some(l) => terminal.push(l),
                None => return usage(),
            },
            "--font-dir" => match val() {
                Some(d) => font_dirs.push(d.into()),
                None => return usage(),
            },
            "--fmt" => match val() {
                Some(f) => fmt = Some(f),
                None => return usage(),
            },
            "--dump-fmt" => match val() {
                Some(f) => dump_fmt = Some(f),
                None => return usage(),
            },
            "--interaction" => match val().as_deref() {
                Some("batchmode") => interaction = Some(0),
                Some("nonstopmode") => interaction = Some(1),
                Some("scrollmode") => interaction = Some(2),
                Some("errorstopmode") => interaction = Some(3),
                _ => return usage(),
            },
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
    // (files read as bytes, a character each; with `--before`, a session:
    // those files' jobs, then this one's; with `--terminal`, a job reading
    // the terminal's first line)
    let read = |f: &str| std::fs::read(f).map(|b| b.iter().map(|&c| char::from(c)).collect::<String>());
    let src = match read(&file) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{file}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let src = if before.is_empty() && terminal.is_empty() {
        src
    } else {
        let mut texts = Vec::new();
        for f in &before {
            match read(f) {
                Ok(s) => texts.push(s),
                Err(e) => {
                    eprintln!("{f}: {e}");
                    return ExitCode::FAILURE;
                }
            }
        }
        texts.push(src);
        let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
        phitex_flow::graph::jobs(&refs)
    };
    let out = out.unwrap_or_else(|| format!("{}.dvi", file.trim_end_matches(".tex")));
    // (`\input` and `\openin` find files beside the document; the job is
    // its name)
    let path = std::path::Path::new(&file);
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).map_or_else(|| ".".into(), std::path::Path::to_path_buf);
    let job = jobname
        .unwrap_or_else(|| path.file_stem().map_or_else(|| "texput".to_string(), |s| s.to_string_lossy().into_owned()));
    let inputs: Vec<std::path::PathBuf> = std::iter::once(dir.clone()).chain(dirs).collect();
    let mut b = Build::new(font.clone());
    b.set_input_dirs(inputs.clone());
    b.set_jobname(&job);
    if !terminal.is_empty() {
        let lines: Vec<&str> = terminal.iter().map(String::as_str).collect();
        b.set_terminal(&lines);
    }
    if !font_dirs.is_empty() {
        font_dirs.extend(phitex_flow::tfm::default_dirs());
        b.set_font_dirs(font_dirs.clone());
    }
    // (`-interaction`: the mode before any definition, and at each job's
    // start over the format's)
    if let Some(m) = interaction {
        b.set_interaction(m);
        b.set_interaction_option(Some(m));
    }
    // (the format the build starts from: its definitions before the first
    // piece)
    if let Some(f) = &fmt {
        let t = std::time::Instant::now();
        let bytes = match std::fs::read(f) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("{f}: {e}");
                return ExitCode::FAILURE;
            }
        };
        if let Err(e) = b.set_format(&bytes) {
            eprintln!("{f}: {e}");
            return ExitCode::FAILURE;
        }
        println!("format: {f} ({} bytes) read in {:.2} ms", bytes.len(), t.elapsed().as_secs_f64() * 1e3);
    }
    let cold = b.update(&src, opts);
    report("cold", &cold);
    if let Some(f) = &dump_fmt {
        let t = std::time::Instant::now();
        match b.format().map(|bytes| std::fs::write(f, &bytes).map(|()| bytes.len())) {
            Ok(Ok(n)) => println!("format: wrote {f} ({n} bytes) in {:.2} ms", t.elapsed().as_secs_f64() * 1e3),
            Ok(Err(e)) => {
                eprintln!("{f}: {e}");
                return ExitCode::FAILURE;
            }
            Err(e) => {
                eprintln!("--dump-fmt: {e}");
                return ExitCode::FAILURE;
            }
        }
    }
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
            fb.set_input_dirs(inputs.clone());
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
