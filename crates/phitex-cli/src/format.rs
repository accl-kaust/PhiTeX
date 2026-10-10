//! The formats: made on the graph from TeX Live's ini files, as fmtutil
//! makes TeX Live's (`latex.ini`'s job, its definitions at the `\dump`:
//! `Build::format`), and kept in the cache directory by the engine, the
//! TeX Live tree and this build of phitex. A document's build starts from
//! one (`Build::set_format`).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use phitex_flow::graph::jobs;
use phitex_flow::print::Limits;
use phitex_flow::{Build, Options};

use crate::tex::{self, Engine, Fnv, Tree};

/// The format engine `e` loads: from the cache, else made now (`note`
/// hears about it). Its bytes and its path.
pub fn ensure(
    e: &Engine,
    tree: &Tree,
    threads: usize,
    note: &mut dyn FnMut(&str),
) -> Result<(Arc<Vec<u8>>, PathBuf), String> {
    let dir = tex::cache_dir().join("formats");
    std::fs::create_dir_all(&dir).map_err(|err| format!("can't make {}: {err}", dir.display()))?;
    let key = Fnv::new()
        .add(e.name.as_bytes())
        .add(&tree.stamp.to_le_bytes())
        .add(&tex::exe_stamp().to_le_bytes())
        .0;
    let path = dir.join(format!("{}-{key:016x}.fmt", e.name));
    if let Ok(b) = std::fs::read(&path) {
        return Ok((Arc::new(b), path));
    }
    // (one process makes it; the others wait for it)
    let lock = std::fs::File::create(dir.join(format!("{}.lock", e.name)))
        .map_err(|err| format!("can't lock the {} format: {err}", e.name))?;
    lock.lock().map_err(|err| format!("can't lock the {} format: {err}", e.name))?;
    if let Ok(b) = std::fs::read(&path) {
        return Ok((Arc::new(b), path));
    }
    note(&format!(
        "making the {} format on the graph from {} (once per TeX Live tree and phitex build)",
        e.name, e.ini
    ));
    let t = Instant::now();
    let tree = tree.for_format(e);
    let Some(ini_path) = tree.input(e.ini) else {
        return Err(format!("{}: not found in TeX Live (TEXINPUTS for {})", e.ini, e.name));
    };
    let ini = std::fs::read(&ini_path).map_err(|err| format!("{}: {err}", ini_path.display()))?;
    let ini = tex::chars(&ini);
    // (`latex.ltx` writes `texsys.aux` and looks for `./texsys.aux` to
    // learn the current directory's syntax: a file written and read back
    // in one run, which a build reads from its directories only)
    let work = dir.join(format!(".work-{}-{}", e.name, std::process::id()));
    let _ = std::fs::create_dir_all(&work);
    let _ = std::fs::write(work.join("texsys.aux"), "1970/01/01:00:00\n\n");
    let mut b = Build::new(phitex_flow::tfm::null_font());
    b.set_kpse(tree.kpse.clone(), tree.prog);
    b.set_input_dirs(vec![work.clone()]);
    b.set_jobname(e.name);
    b.set_limits(Limits::TEX_LIVE);
    b.set_terminal(&[e.ini]);
    b.set_interaction_option(Some(0));
    let opts = Options { threads, etex: e.etex, ..Options::default() };
    let out = b.update(&jobs(&[&ini]), opts);
    let _ = std::fs::remove_dir_all(&work);
    let log = dir.join(format!("{}.log", e.name));
    if let Some(l) = out.job_logs(&Limits::TEX_LIVE, &format!("{}.dvi", e.name)).first() {
        let _ = std::fs::write(&log, tex::bytes(&l.log));
    }
    if !out.errors.is_empty() {
        return Err(format!(
            "the {} format did not build on the graph: {} error{}, the first: {}; its log: {}{}",
            e.name,
            out.errors.len(),
            if out.errors.len() == 1 { "" } else { "s" },
            bare(&out.errors[0]),
            log.display(),
            waits_on(e)
        ));
    }
    let bytes = b
        .format()
        .map_err(|err| format!("the {} format could not be saved: {err}{}", e.name, waits_on(e)))?;
    let tmp = dir.join(format!(".{}-{key:016x}.{}", e.name, std::process::id()));
    std::fs::write(&tmp, &bytes)
        .and_then(|()| std::fs::rename(&tmp, &path))
        .map_err(|err| format!("can't write {}: {err}", path.display()))?;
    remove_others(&dir, e.name, &path);
    note(&format!(
        "the {} format: made in {:.1} s, {} bytes ({})",
        e.name,
        t.elapsed().as_secs_f64(),
        bytes.len(),
        path.display()
    ));
    Ok((Arc::new(bytes), path))
}

/// What a format that fails is waiting on, if it is known.
fn waits_on(e: &Engine) -> &'static str {
    match e.name {
        "tex" | "etex" | "pdftex" => " (plain's formats are not among phitex-flow's tested ones yet)",
        _ => "",
    }
}

/// An error without the piece it was met in (`piece K: …`, the graph's
/// own numbering).
pub fn bare(e: &str) -> &str {
    match e.split_once(": ") {
        Some((p, m)) if p.starts_with("piece ") => m,
        _ => e,
    }
}

/// The formats of engine `name` other than `keep`, made by other builds or
/// trees, removed.
fn remove_others(dir: &Path, name: &str, keep: &Path) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let prefix = format!("{name}-");
    for e in rd.flatten() {
        let p = e.path();
        let n = e.file_name().to_string_lossy().into_owned();
        let rest = n.strip_prefix(&prefix).and_then(|r| r.strip_suffix(".fmt"));
        if rest.is_some_and(|r| r.len() == 16 && r.bytes().all(|c| c.is_ascii_hexdigit())) && p != keep {
            let _ = std::fs::remove_file(p);
        }
    }
}
