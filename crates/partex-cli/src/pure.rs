//! `PARTEX_SSA=1 PHITEX_SSA_PURE=1`: the build on the φ core
//! (`partex_core::pure`, DESIGN 3.17). The cold build, then one rebuild
//! after each line of `PARTEX_SSA_REBUILD` run as a shell command (an
//! edit), each linked from the steps' effects and its files written.

use partex_core::host::Host;
use partex_core::params::Params;
use partex_core::pure::{Build, PureTracker};
use partex_core::Tex;

use crate::native;

/// The main file of `command_line`: its last word that is not a format
/// (`&name`) or TeX code, as given or with `.tex`, and its bytes.
fn main_file(command_line: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    let s = String::from_utf8_lossy(command_line);
    let w = s
        .split_whitespace()
        .rev()
        .find(|w| !w.starts_with('&') && !w.starts_with('\\'))?;
    for name in [w.to_string(), format!("{w}.tex")] {
        if let Ok(b) = std::fs::read(&name) {
            return Some((name.into_bytes(), b));
        }
    }
    None
}

/// Link `b`'s effects and write its files, the terminal's text and the
/// diagnostics.
fn link(b: &mut Build<native::NativeHost>, linker: &mut crate::SsaLinker) -> String {
    let t0 = std::time::Instant::now();
    let fx = b.effects();
    let chunks: Vec<&[partex_core::effects::Effect]> = fx.iter().map(|x| &x[..]).collect();
    let threads = partex_incr::Threads::available();
    let linked = partex_core::effects::link(&chunks, &threads, &mut |level, data| {
        crate::zlib::deflate_stream(level, data)
    });
    let l = match linked {
        Ok(l) => l,
        Err(e) => {
            eprintln!("phitex: pure ssa: the link failed: {e:?}");
            std::process::exit(3);
        }
    };
    let t_link = t0.elapsed();
    let host = b.engine().tex.host_mut();
    let (files, bytes) = linker.write_full(host, &l);
    host.term_write(&l.term);
    for d in &l.diagnostics {
        host.diagnostic(d);
    }
    format!(
        "linked {} chunks in {:.1} ms; {files} files written ({bytes} bytes)",
        chunks.len(),
        t_link.as_secs_f64() * 1e3
    )
}

fn report(n: usize, b: &mut Build<native::NativeHost>, rep: &phi::Report, ms: f64) {
    let s = b.engine().stats;
    let (live, _) = b.g.mem();
    eprintln!(
        "phitex: pure ssa build {n}: {ms:.1} ms; steps run {} (runs {}, names loaded {}), \
         reads {}, definitions {}, groups {}, effect steps {}; core: steps {} evals {} \
         woken {} created {} removed {} iterations {}; live nodes {live}",
        s.steps,
        s.runs,
        s.loaded,
        s.reads,
        s.defs,
        s.groups,
        s.effects,
        rep.steps,
        rep.evals,
        rep.woken,
        rep.created,
        rep.removed,
        rep.iterations,
    );
    let other = b.engine().tex.tracker().others();
    if !other.is_empty() {
        eprintln!("phitex: pure ssa build {n}: families not versioned: {other:?}");
    }
}

/// The build; its history.
pub fn run(mut host: native::NativeHost, params: Params, command_line: &[u8]) -> i32 {
    host.commands = Some(native::Commands::default());
    let Some((_, main)) = main_file(command_line) else {
        eprintln!("phitex: pure ssa: no main file in the command line");
        return 3;
    };
    let workers = ["PHITEX_SSA_WORKERS", "PARTEX_SSA_WORKERS"]
        .iter()
        .find_map(|k| std::env::var(k).ok()?.parse::<usize>().ok())
        .unwrap_or(1);
    let t0 = std::time::Instant::now();
    let tex = Tex::new(host, PureTracker::default(), params);
    let mut b = Build::new(tex, command_line, &main, workers);
    let rep = b.run();
    let ms = t0.elapsed().as_secs_f64() * 1e3;
    let mut linker = crate::SsaLinker::default();
    let how = link(&mut b, &mut linker);
    report(0, &mut b, &rep, ms);
    eprintln!("phitex: pure ssa build 0: {how}");
    let mut history = b.history().unwrap_or(3);
    let rebuild = std::env::var("PARTEX_SSA_REBUILD").ok();
    let lines: Vec<&str> = rebuild.as_deref().unwrap_or_default().lines().collect();
    for (n, cmd) in lines.iter().enumerate() {
        let ok = std::process::Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            eprintln!("phitex: pure ssa: the rebuild command failed");
            return 3;
        }
        let Some((_, main)) = main_file(command_line) else {
            eprintln!("phitex: pure ssa: the main file is gone");
            return 3;
        };
        let t0 = std::time::Instant::now();
        b.engine().stats = partex_core::pure::Stats::default();
        b.edit(&main);
        let rep = b.run();
        let ms = t0.elapsed().as_secs_f64() * 1e3;
        let how = link(&mut b, &mut linker);
        report(n + 1, &mut b, &rep, ms);
        eprintln!("phitex: pure ssa build {}: {how}", n + 1);
        history = b.history().unwrap_or(3);
    }
    history
}
