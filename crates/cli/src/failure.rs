//! The engine's failures written out for a bug report, in the cache's
//! `reports` directory (the sentences a reader is shown are
//! `phitex_live::failure`'s).

/// An engine failure written out for a bug report: what PhiTeX was, the
/// command, the document's name (not its text), the build, the engine's
/// whole account (`internal`) and the errors shown. In the cache's
/// `reports` directory; its path, or none where it could not be written.
pub fn report_file(file: &str, why: &str, threads: usize, internal: &str, errors: &[String]) -> Option<std::path::PathBuf> {
    let dir = crate::tex::cache_dir().join("reports");
    std::fs::create_dir_all(&dir).ok()?;
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let exe = std::env::current_exe().ok();
    let built = exe
        .as_ref()
        .and_then(|p| std::fs::metadata(p).ok())
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs());
    let mut s = String::new();
    s.push_str("PhiTeX failure report\n");
    s.push_str(&format!("version: {} (binary {}, built at unix {built})\n", env!("CARGO_PKG_VERSION"), exe.map_or_else(String::new, |p| p.display().to_string())));
    s.push_str(&format!("time: unix {secs}\n"));
    s.push_str(&format!("command: {}\n", std::env::args().collect::<Vec<_>>().join(" ")));
    s.push_str(&format!("document: {file}\n"));
    s.push_str(&format!("build: {why}, {threads} threads\n"));
    s.push_str("errors shown:\n");
    for e in errors {
        s.push_str(&format!("  {e}\n"));
    }
    s.push_str("engine:\n");
    s.push_str(internal);
    s.push('\n');
    let p = dir.join(format!("report-{secs}-{}.txt", std::process::id()));
    std::fs::write(&p, s).ok()?;
    Some(p)
}
