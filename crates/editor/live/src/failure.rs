//! The engine's failures and warnings said for a reader. The engine
//! reports an evaluation that could not finish (`phitex_tex::Diagnostic`
//! errors at no place: its `EvalError` and the nodes it names, one per
//! line) and one that finished at the round cap (a warning naming the
//! read-back fields still moving). Their text is for a bug report; the
//! terminal gets a sentence: what failed, where in the document, and
//! whose fault it is. The whole text stays for `-v` (`Report::internal`).

/// What the engine's own failures end with: not the document's fault.
const BUG: &str = "This is a bug in PhiTeX, not in the document: please report it with the report file named below";

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
/// `wtext:F`, `wlines:F`, `wkeys:F`, `wfile:F`, `wstream:F`, `wunit:F:…` are `F`.
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
    for p in ["wtext:", "wlines:", "wkeys:", "wfile:", "wstream:"] {
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

/// The engine's message that the round cap, latexmk's maximum runs, was
/// reached (`… after N rounds …; still changing: f, g`: an error, the
/// build failed as latexmk's, `Build::no_fixpoint`), for the terminal;
/// any other message (a warning) as it is.
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

/// The packages that run only under XeTeX or LuaTeX, and the engine each
/// asks for (fontspec and unicode-math run under both: XeTeX's is the one
/// PhiTeX runs). LuaTeX's come first: a document with luacode and fontspec
/// needs LuaTeX. The CLI's choice of engine reads them too.
pub const NATIVE_PACKAGES: [(&str, &str); 14] = [
    ("luacode", "luatex"),
    ("luatexja", "luatex"),
    ("luatexja-fontspec", "luatex"),
    ("luaotfload", "luatex"),
    ("luamplib", "luatex"),
    ("fontspec", "xetex"),
    ("unicode-math", "xetex"),
    ("polyglossia", "xetex"),
    ("xeCJK", "xetex"),
    ("xltxtra", "xetex"),
    ("xunicode", "xetex"),
    ("mathspec", "xetex"),
    ("xgreek", "xetex"),
    ("bidi", "xetex"),
];

/// A package's engine check failed (fontspec's "requires either XeTeX or
/// LuaTeX" under pdflatex, unicode-math's "Cannot be run with pdftex!"):
/// the error's help says which engine the document needs and how to name
/// it, before the package's own help. `engine` is the program run
/// (`pdflatex`, `xelatex`, ...).
pub fn engine_needed(ds: &mut [phitex_tex::Diagnostic], engine: &str) {
    let xetex = engine.starts_with("xe");
    let latex = engine.ends_with("latex");
    for d in ds
        .iter_mut()
        .filter(|d| d.severity == phitex_tex::Severity::Error)
    {
        let m = d.message.trim_start_matches("Fatal ");
        let Some(name) = m
            .strip_prefix("Package ")
            .and_then(|r| r.split_once(" Error"))
            .map(|(n, _)| n)
        else {
            continue;
        };
        let Some(&(_, needs)) = NATIVE_PACKAGES.iter().find(|(n, _)| *n == name) else {
            continue;
        };
        let about_engine = ["XeTeX", "LuaTeX", "pdftex", "pdfTeX", "engine"]
            .iter()
            .any(|w| m.contains(w));
        if !about_engine || (needs == "xetex" && xetex) {
            continue;
        }
        let say = if needs == "luatex" {
            format!(
                "{name} runs only under LuaTeX: the document needs {}, which PhiTeX does not support yet",
                if latex { "LuaLaTeX" } else { "LuaTeX" }
            )
        } else {
            let (prog, word) = if latex {
                ("xelatex", "XeLaTeX")
            } else {
                ("xetex", "XeTeX")
            };
            format!(
                "{name} runs only under XeTeX or LuaTeX, and this build ran {engine}: build with {word} (--engine {prog}, or `% !TEX program = {prog}` in the main file); LuaLaTeX is not supported yet"
            )
        };
        d.help = Some(match d.help.take() {
            Some(h) if !h.is_empty() => format!("{say}\n{h}"),
            _ => say,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn error(message: &str) -> phitex_tex::Diagnostic {
        phitex_tex::Diagnostic {
            file: "fontspec.sty".into(),
            range: (0, 0),
            line: 100,
            severity: phitex_tex::Severity::Error,
            message: message.into(),
            context: String::new(),
            help: None,
        }
    }

    #[test]
    fn a_packages_engine_check_says_which_engine_to_name() {
        let mut ds = vec![
            error("Fatal Package fontspec Error: The fontspec package requires either XeTeX or"),
            error("Package unicode-math Error: Cannot be run with pdftex!"),
            error("Package hyperref Error: Wrong DVI mode driver option `dvipdfmx'"),
            error("Undefined control sequence"),
        ];
        engine_needed(&mut ds, "pdflatex");
        let h = ds[0].help.as_deref().unwrap_or("");
        assert!(h.starts_with("fontspec runs only under XeTeX or LuaTeX, and this build ran pdflatex: build with XeLaTeX (--engine xelatex, or `% !TEX program = xelatex` in the main file)"), "{h}");
        assert!(h.contains("LuaLaTeX is not supported yet"), "{h}");
        assert!(
            ds[1]
                .help
                .as_deref()
                .unwrap_or("")
                .starts_with("unicode-math runs only")
        );
        assert_eq!((ds[2].help.as_deref(), ds[3].help.as_deref()), (None, None));
        let mut ds = vec![
            error("Package luacode Error: LuaTeX is required"),
            error("Package fontspec Error: The font \"X\" cannot be found"),
        ];
        engine_needed(&mut ds, "xelatex");
        assert_eq!(
            ds[0].help.as_deref(),
            Some(
                "luacode runs only under LuaTeX: the document needs LuaLaTeX, which PhiTeX does not support yet"
            )
        );
        assert_eq!(ds[1].help, None);
    }

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
        let s = explain(
            "evaluation: stuck: every worker idle for 10 s with 3 nodes outstanding: a wake was lost",
        );
        assert!(
            s.contains("(every worker idle for 10 s with 3 nodes outstanding: a wake was lost)"),
            "{s}"
        );
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
