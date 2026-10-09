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

#[allow(clippy::too_many_lines)]
fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
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
    // (the `.tex` files beside it are what `\input` reads)
    let files = input_files(&file);
    let new_build = || {
        let mut b = Build::new(font.clone());
        for (name, text) in &files {
            b.set_file(name, text);
        }
        b
    };
    let mut b = new_build();
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
            let fresh = new_build().update(&edited, opts);
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
    println!("wrote {out} ({} bytes)", last.dvi.len());
    ExitCode::SUCCESS
}

/// The `.tex` files in the directory of `file` (by name), for `\input`.
fn input_files(file: &str) -> Vec<(String, String)> {
    let dir = std::path::Path::new(file).parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(std::path::Path::new("."));
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out = Vec::new();
    for e in rd.flatten() {
        let path = e.path();
        if path.extension().is_some_and(|x| x == "tex")
            && let (Some(name), Ok(text)) = (path.file_name().and_then(|n| n.to_str()), std::fs::read_to_string(&path))
        {
            out.push((name.to_string(), text));
        }
    }
    out
}
