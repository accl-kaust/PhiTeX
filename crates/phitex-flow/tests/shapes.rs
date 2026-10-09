#![allow(clippy::pedantic)]

//! The shapes the design bans (AGENTS_BRIEF.md, "Banned"): TeX here is a
//! language evaluated as a graph of commands, not Knuth's machine. This
//! test reads the crate's sources and fails on the machine's shapes, so
//! that drifting back is a failing gate, not a judgement call.
//!
//! - No process-wide state: `static mut`, `thread_local!`, `OnceLock`,
//!   `OnceCell`, `lazy_static`, `std::sync::Once`.
//! - Locks and atomics only where threads meet: the scheduler (build.rs)
//!   and the font file cache (tfm.rs, a cache of file bytes, no TeX state).
//! - Knuth's names for his state: `eqtb`, `save_stack`, `cur_list`,
//!   `input_stack`, `unsave`, `eq_save`, `eq_define`, `geq_define`,
//!   `nest_ptr`, `cur_input`, `main_control` — none in code (comments may
//!   cite tex.web).
//! - No engine/world struct: `Tex`, `Engine`, `World`, `Globals`, `Eqtb`,
//!   `Nest`, `SaveStack`, `InputStack`, `Machine`.
//! - A name -> value table (`FxMap<Key, _>`, `HashMap<Key, _>`,
//!   `BTreeMap<Key, _>`) only in the graph's own indexes (graph.rs: a
//!   chain's definitions, a run's outside reads, a node's reads so far) and
//!   the build's (build.rs: the names index and the edges). Anywhere else
//!   it is eqtb under another name.

use std::path::{Path, PathBuf};

fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            sources(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

fn is_ident_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

/// Whether `line` contains `word` as a whole identifier.
fn has_word(line: &str, word: &str) -> bool {
    let b = line.as_bytes();
    let mut from = 0;
    while let Some(i) = line[from..].find(word) {
        let s = from + i;
        let e = s + word.len();
        let before = s == 0 || !is_ident_char(b[s - 1]);
        let after = e == b.len() || !is_ident_char(b[e]);
        if before && after {
            return true;
        }
        from = e;
    }
    false
}

#[test]
fn no_shape_of_knuths_machine() {
    let root = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/src"));
    let mut files = Vec::new();
    sources(root, &mut files);
    files.sort();
    assert!(files.len() > 5, "sources found: {files:?}");
    let mut bad: Vec<String> = Vec::new();
    for f in &files {
        let name = f.file_name().unwrap().to_string_lossy().to_string();
        let text = std::fs::read_to_string(f).unwrap();
        let mut in_block_comment = false;
        for (n, raw) in text.lines().enumerate() {
            let line = raw.trim_start();
            if in_block_comment {
                if line.contains("*/") {
                    in_block_comment = false;
                }
                continue;
            }
            if line.starts_with("//") {
                continue;
            }
            if line.starts_with("/*") {
                in_block_comment = !line.contains("*/");
                continue;
            }
            let at = |rule: &str| format!("{}:{}: {rule}: {}", f.strip_prefix(root).unwrap().display(), n + 1, raw.trim());
            for pat in ["static mut", "thread_local!", "lazy_static"] {
                if line.contains(pat) {
                    bad.push(at(&format!("process-wide state (`{pat}`)")));
                }
            }
            // (a once-set cache of immutable things — the null font, the
            // TFM directory index — lives in tfm.rs only)
            let once = line.contains("OnceLock") || line.contains("OnceCell") || line.contains("sync::Once");
            if once && name != "tfm.rs" {
                bad.push(at("a once-set process-wide value outside the font file cache (tfm.rs)"));
            }
            let has_lock = line.contains("Mutex<")
                || line.contains("RwLock<")
                || line.contains("Condvar")
                || ["AtomicBool", "AtomicUsize", "AtomicIsize", "AtomicU64", "AtomicU32", "AtomicI64", "AtomicI32"]
                    .iter()
                    .any(|a| line.contains(a));
            if has_lock && name != "build.rs" && name != "tfm.rs" {
                bad.push(at("a lock or atomic outside the scheduler (build.rs) and the font file cache (tfm.rs)"));
            }
            for w in [
                "eqtb",
                "save_stack",
                "cur_list",
                "input_stack",
                "unsave",
                "eq_save",
                "eq_define",
                "geq_define",
                "nest_ptr",
                "cur_input",
                "main_control",
            ] {
                if has_word(line, w) {
                    bad.push(at(&format!("Knuth's state under its own name (`{w}`)")));
                }
            }
            if let Some(rest) = line.strip_prefix("pub struct ").or_else(|| line.strip_prefix("struct ")) {
                let ident: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
                if ["Tex", "Engine", "World", "Globals", "Eqtb", "Nest", "SaveStack", "InputStack", "Machine"]
                    .contains(&ident.as_str())
                {
                    bad.push(at(&format!("an engine struct (`{ident}`)")));
                }
            }
            let key_map = ["FxMap<Key", "HashMap<Key", "BTreeMap<Key"].iter().any(|p| line.contains(p));
            if key_map && name != "graph.rs" && name != "build.rs" {
                bad.push(at("a name -> value table outside the graph's own indexes (eqtb under another name)"));
            }
        }
    }
    assert!(bad.is_empty(), "the shapes of Knuth's machine (AGENTS_BRIEF.md, Banned):\n{}", bad.join("\n"));
}
