//! The engine's failures and warnings said for a reader. The engine
//! reports an evaluation that could not finish (`phitex_tex::Diagnostic`
//! errors at no place: its `EvalError` and the nodes it names, one per
//! line) and one that finished at the round cap (a warning naming the
//! read-back fields still moving). Their text is for a bug report; the
//! terminal gets a sentence: what failed, where in the document, and
//! whose fault it is. The whole text stays for `-v` (`Report::internal`).

/// What the engine's own failures end with: not the document's fault.
const BUG: &str = "This is a bug in PhiTeX, not in the document: please report it with the report file named below";

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

/// The node an engine message names first (its lines `  NodeId(..): op
/// state, parent .., hint "text" (cause), args ..`), as ` at <op> on
/// "<text>"`; empty when it names none.
fn node_of(m: &str) -> String {
    for l in m.lines().skip(1).map(str::trim_start) {
        let Some((_, rest)) = l.split_once("): ").filter(|_| l.starts_with("NodeId(")) else {
            continue;
        };
        let op = rest.split([' ', ',']).next().unwrap_or("");
        let text = rest
            .split_once(", hint \"")
            .and_then(|(_, h)| h.split_once("\" (").map(|(t, _)| t))
            .unwrap_or("")
            .replace("\\n", " ")
            .replace("\\\\", "\\");
        let text = text.trim();
        let short: String = text.chars().take(60).collect();
        let cut = if short.len() < text.len() { "…" } else { "" };
        return if short.is_empty() {
            format!(" at {op}")
        } else {
            format!(" at {op} on \"{short}{cut}\"")
        };
    }
    String::new()
}

/// A read-back field's engine name said as its file (and the key, for
/// one of a file's definitions): `wdef:F\u{1}cs:name` is `F (\name)`,
/// `wtext:F`, `wkeys:F`, `wfile:F`, `wstream:F`, `wunit:F:…` are `F`.
pub fn field(f: &str) -> String {
    let f = f.trim();
    if let Some(d) = f.strip_prefix("wdef:") {
        let (file, key) = d
            .split_once('\u{1}')
            .or_else(|| d.split_once(' '))
            .unwrap_or((d, ""));
        let key = key
            .strip_prefix("cs:")
            .map_or_else(|| key.to_owned(), |k| format!("\\{k}"));
        return if key.is_empty() {
            file.to_owned()
        } else {
            format!("{file} ({key})")
        };
    }
    for p in ["wtext:", "wkeys:", "wfile:", "wstream:"] {
        if let Some(file) = f.strip_prefix(p) {
            return file.to_owned();
        }
    }
    if let Some(u) = f.strip_prefix("wunit:") {
        // (`wunit:<file>:<hash>:<n>`: a line of the file)
        let mut parts = u.rsplitn(3, ':');
        let _n = parts.next();
        let _hash = parts.next();
        return parts.next().unwrap_or(u).to_owned();
    }
    f.to_owned()
}

/// The fields `fs` said as files, each once, keys kept (at most `max`
/// named, then how many more).
fn files(fs: &[String], max: usize) -> String {
    let mut out: Vec<String> = Vec::new();
    for f in fs {
        let r = field(f);
        if !out.contains(&r) {
            out.push(r);
        }
    }
    // (a file named alone says less than the same file with a key)
    let keyed: Vec<String> = out.iter().filter(|r| r.contains(" (")).cloned().collect();
    out.retain(|r| r.contains(" (") || !keyed.iter().any(|k| k.starts_with(&format!("{r} ("))));
    let more = out.len().saturating_sub(max);
    out.truncate(max);
    let mut s = out.join(", ");
    if more > 0 {
        s.push_str(&format!(" and {more} more"));
    }
    s
}

/// An engine failure (a diagnostic's whole message) as one paragraph for
/// the terminal.
#[must_use]
pub fn explain(m: &str) -> String {
    let first = m.lines().next().unwrap_or("").trim();
    let node = node_of(m);
    // (a static hook of an operation panicked: `internal error: the
    // {hook} hook of operation {op} panicked: {message}`)
    if let Some(rest) = first.strip_prefix("internal error: the ") {
        let (what, msg) = rest.split_once(" panicked: ").unwrap_or((rest, ""));
        let at = if node.is_empty() {
            " (before its node was placed)".to_owned()
        } else {
            node
        };
        return format!("PhiTeX's engine failed: the {what} panicked{at}: {msg}. {BUG}");
    }
    let e = first.strip_prefix("evaluation: ").unwrap_or(first);
    // (the build stopped making progress, `EvalError::Stuck`: `stuck: node
    // N (op): why; at place` or `stuck: why`)
    if let Some(rest) = e.strip_prefix("stuck: ") {
        let why = match rest.strip_prefix("node ") {
            Some(r) => r.split_once("): ").map_or(rest, |(_, w)| w),
            None => rest,
        };
        return format!(
            "the build stopped: it made no progress ({why}){node}. The last good output is kept. If pdflatex finishes this document, this is a bug in PhiTeX: please report it with the report file named below"
        );
    }
    // (two token sequences met in one id, `EvalError::SeqCollision`)
    if e.starts_with("two sequences met in one id ") {
        return format!(
            "PhiTeX's engine stopped: two different sequences hashed to one identity{node}, and the build would have read the one for the other. {BUG}"
        );
    }
    if let Some(rest) = e.strip_prefix("no fixpoint after ") {
        let rounds = rest.split(' ').next().unwrap_or("?");
        let written: Vec<String> = m
            .lines()
            .filter_map(|l| l.trim_start().strip_prefix("written file "))
            .map(str::to_owned)
            .collect();
        let named = if written.is_empty() {
            String::new()
        } else {
            format!(" (the files it writes: {})", files(&written, 6))
        };
        return format!(
            "the files the document writes and reads back did not settle: no fixpoint after {rounds} rounds{named}, so no output was made. {BUG}"
        );
    }
    if e.starts_with("final node ") && e.contains(" moved loop-back field ") {
        let moved = m
            .lines()
            .find_map(|l| l.trim_start().strip_prefix("the loop-back moved: "))
            .map_or_else(|| "a file".to_owned(), field);
        return format!(
            "PhiTeX's engine failed: a step that runs only once the rounds have settled{node} changed {moved}, which the document reads back, so the output would not be the fixpoint's. {BUG}"
        );
    }
    // (an operation's run panicked: `node N (op) panicked: message`)
    if let Some((head, msg)) = e.split_once(") panicked: ")
        && let Some((_, op)) = head.rsplit_once(" (")
    {
        let at = if node.is_empty() {
            format!(" in {op}")
        } else {
            node
        };
        return format!("PhiTeX's engine failed: a step panicked{at}: {msg}. {BUG}");
    }
    crate::session::engine_stop(m)
}

/// The engine's warning that the round cap was reached (`… still
/// changed after N rounds …; still changing: f, g`), for the terminal;
/// any other warning as it is.
#[must_use]
pub fn warning(m: &str) -> String {
    let Some((head, moving)) = m.split_once("still changing: ") else {
        return m.to_owned();
    };
    let rounds = head
        .split("after ")
        .nth(1)
        .and_then(|r| r.split(' ').next())
        .unwrap_or("5");
    let fs: Vec<String> = moving.split(", ").map(str::to_owned).collect();
    format!(
        "the files the document reads back still changed after {rounds} rounds, latexmk's limit: {}. The output is the last round's, as latexmk's would be; a page reference or a count may be off by one round",
        files(&fs, 6)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_panic_names_its_node() {
        let m = "internal error: the reads hook of operation exec panicked: index out of bounds\n\
                 evaluation: exec::reads panicked (node Some(NodeId(7))): index out of bounds\n  \
                 NodeId(7): exec Done, parent Some(NodeId(2)), hint \"Hello \\\\bf world\" (text), args 1 2";
        let s = explain(m);
        assert!(s.starts_with("PhiTeX's engine failed: the reads hook of operation exec panicked at exec on \"Hello \\bf world\": index out of bounds."), "{s}");
        assert!(s.contains("bug in PhiTeX"), "{s}");
        let s = explain(
            "internal error: the claim hook of operation load panicked: boom\nevaluation: load::claim panicked (node None): boom",
        );
        assert!(s.contains("before its node was placed"), "{s}");
    }

    #[test]
    fn no_fixpoint_names_the_files() {
        let m = "evaluation: no fixpoint after 40 rounds\n  written file doc.w1\n  written file doc.w2\n  read-back field doc.w1 cs:x";
        let s = explain(m);
        assert!(
            s.contains("no fixpoint after 40 rounds (the files it writes: doc.w1, doc.w2)"),
            "{s}"
        );
    }

    #[test]
    fn final_moved_loop_names_the_field() {
        let m = "evaluation: final node NodeId(9) moved loop-back field Field(3)\n  the loop-back moved: wdef:doc.w1\u{1}cs:lastpage\n  NodeId(9): link Done, parent None, hint \"\" (none), args";
        let s = explain(m);
        assert!(s.contains("at link changed doc.w1 (\\lastpage)"), "{s}");
    }

    #[test]
    fn op_panic() {
        let s = explain(
            "evaluation: node NodeId(4) (page) panicked: oops\n  NodeId(4): page Running, parent None, hint \"x\" (text), args",
        );
        assert!(
            s.starts_with("PhiTeX's engine failed: a step panicked at page on \"x\": oops."),
            "{s}"
        );
    }

    #[test]
    fn stuck_says_why_and_where() {
        let m = "evaluation: stuck: node 2167 (finish): it read no new line of its input for 60 s; at amsldoc.tex:2174, on \\foo, input 9 levels deep, 3 lists open, 812 commands run\n  NodeId(2167): finish Running, parent None, hint \"\" (none), args";
        let s = explain(m);
        assert!(
            s.starts_with("the build stopped: it made no progress (it read no new line of its input for 60 s; at amsldoc.tex:2174"),
            "{s}"
        );
        assert!(s.contains("last good output is kept"), "{s}");
        let s = explain("evaluation: stuck: every worker idle for 10 s with 3 nodes outstanding: a wake was lost");
        assert!(s.contains("(every worker idle for 10 s with 3 nodes outstanding: a wake was lost)"), "{s}");
    }

    #[test]
    fn a_sequence_collision_is_the_engines_fault() {
        let m = "evaluation: two sequences met in one id 0x1: node 12 (exec)\n  NodeId(12): exec Running, parent None, hint \"\\section\" (cause), args";
        let s = explain(m);
        assert!(s.starts_with("PhiTeX's engine stopped: two different sequences hashed to one identity at exec on \"\\section\""), "{s}");
        assert!(s.ends_with("the report file named below"), "{s}");
    }

    #[test]
    fn cap_warning_is_readable() {
        let m = "the files read back still changed after 5 rounds (latexmk's limit); the output is the last round's; still changing: wdef:doc.w1\u{1}cs:a, wkeys:doc.w1, wtext:doc.w2, wunit:doc.w2:00ff:0";
        let s = warning(m);
        assert_eq!(
            s,
            "the files the document reads back still changed after 5 rounds, latexmk's limit: doc.w1 (\\a), doc.w2. The output is the last round's, as latexmk's would be; a page reference or a count may be off by one round"
        );
    }
}
