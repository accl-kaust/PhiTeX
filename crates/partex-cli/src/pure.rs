//! `PARTEX_SSA=1 PHITEX_SSA_PURE=1`: the build on the φ core
//! (`partex_core::pure`, DESIGN 3.17). The cold build, then one rebuild
//! after each line of `PARTEX_SSA_REBUILD` run as a shell command (an
//! edit), each linked from the steps' effects and its files written.

use partex_core::Tex;
use partex_core::host::Host;
use partex_core::params::Params;
use partex_core::pure::{Build, PureTracker, Shared};

use crate::native;

/// Link `b`'s effects and write its files, the terminal's text and the
/// diagnostics.
fn link(b: &mut Build<Shared<native::NativeHost>>, linker: &mut crate::SsaLinker) -> String {
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
    let shared = b.engine().tex.host_mut().clone();
    let mut guard = shared.lock();
    let host: &mut native::NativeHost = &mut guard;
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

fn report(n: usize, b: &mut Build<Shared<native::NativeHost>>, rep: &phi::Report, ms: f64) {
    let s = b.engine().stats;
    let (live, _) = b.g.mem();
    eprintln!(
        "phitex: pure ssa build {n}: {ms:.1} ms; steps run {} (placed {}, frontiers whole {}, since {}, names loaded {}), \
         reads {}, definitions {}, groups {}, effect steps {}, file calls {}; core: steps {} evals {} \
         woken {} created {} removed {} iterations {}; live nodes {live}",
        s.steps,
        s.placed,
        s.runs,
        s.since,
        s.loaded,
        s.reads,
        s.defs,
        s.groups,
        s.effects,
        s.calls,
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
    let workers = ["PHITEX_SSA_WORKERS", "PARTEX_SSA_WORKERS"]
        .iter()
        .find_map(|k| std::env::var(k).ok()?.parse::<usize>().ok())
        .unwrap_or(1);
    let t0 = std::time::Instant::now();
    let host = Shared(std::sync::Arc::new(std::sync::Mutex::new(host)));
    let tex = Tex::new(host.clone(), PureTracker::default(), params);
    let mut b = Build::new(tex, command_line, workers);
    if workers > 1 {
        b.enable_workers(host);
    }
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
        let t0 = std::time::Instant::now();
        b.engine().stats = partex_core::pure::Stats::default();
        let fr = b.refresh();
        eprintln!(
            "phitex: pure ssa build {}: files changed {}, removed {}",
            n + 1,
            fr.changed,
            fr.removed
        );
        let rep = b.run();
        let ms = t0.elapsed().as_secs_f64() * 1e3;
        let how = link(&mut b, &mut linker);
        report(n + 1, &mut b, &rep, ms);
        eprintln!("phitex: pure ssa build {}: {how}", n + 1);
        history = b.history().unwrap_or(3);
    }
    history
}
