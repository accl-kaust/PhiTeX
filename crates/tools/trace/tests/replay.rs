//! The replay converter on a tiny plain document recorded through
//! `pdftex -ini`: the output validates, and the syntactic pass's view
//! lands on the steps it should.

use std::io::BufReader;
use std::path::PathBuf;

use phitex_trace::analyze::Graph;
use phitex_trace::parse::Options;
use phitex_trace::record::{Engine, record};
use phitex_trace::replay::{Provenance, convert, plan_edits};
use phitex_trace::v1;
use serde_json::{Value, json};

const DOC: &str = "\\def\\x{Hello}\n\\expandafter\\def\\csname e\\the\\count0\\endcsname{y}\n\\def\\m#1{[#1]}\n\\def\\mk#1{\\def#1{z}}\n\\begingroup \\count1=5 \\m{ab}\\endgroup\n\\mk\\y\n\\x\n\\bye\n";

/// The steps in id order, children after their parent (`children` kept
/// as the count of direct children).
fn flatten(mut v: Value, out: &mut Vec<Value>) {
    let kids = v
        .as_object_mut()
        .and_then(|o| o.remove("children"))
        .and_then(|k| k.as_array().cloned())
        .unwrap_or_default();
    v["n_children"] = kids.len().into();
    out.push(v);
    for k in kids {
        flatten(k, out);
    }
}

fn scratch() -> PathBuf {
    let base = std::env::var_os("CLAUDE_JOB_DIR").map_or_else(
        || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../target"),
        |d| PathBuf::from(d).join("tmp"),
    );
    base.join("phitex-trace-replay-test")
}

struct Run {
    meta: Value,
    steps: Vec<Value>,
    edits: Vec<Value>,
    trace: Vec<Value>,
}

fn run() -> Run {
    let dir = scratch();
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    let main = dir.join("tiny.tex");
    std::fs::write(&main, DOC).expect("the document");
    let out = dir.join("tiny.trace.jsonl");
    let run = dir.join("run");
    let _ = std::fs::remove_dir_all(&run);
    let recorded =
        record(&main, Engine::Pdftex, &run, &out, Options::default()).expect("pdftex runs");
    assert_eq!(recorded.status, Some(0), "see {}", recorded.log.display());
    let open = || std::fs::File::open(&out).map(BufReader::new);
    let g = Graph::load(open().expect("the trace")).expect("a graph");
    let edits = plan_edits(&g, 200);
    let mut buf = Vec::new();
    let dirs = [dir.clone(), run.clone()];
    let load = |name: &str| {
        let p = std::path::Path::new(name);
        let cands: Vec<PathBuf> = if p.is_absolute() {
            vec![p.to_path_buf()]
        } else {
            dirs.iter()
                .map(|d| d.join(name.trim_start_matches("./")))
                .collect()
        };
        cands.iter().find_map(|c| std::fs::read_to_string(c).ok())
    };
    let prov = Provenance {
        document: "tiny".to_owned(),
        trace_commit: "test".to_owned(),
    };
    let c = convert(open, load, &prov, &edits, Some(&g), &mut buf).expect("converts");
    // The engine's loader's rules (format v1), trace and edits.
    let tops = v1::trace(buf.as_slice()).expect("the trace is format v1");
    let mut edit_text = Vec::new();
    for e in &c.edits {
        serde_json::to_writer(&mut edit_text, e).expect("json");
        edit_text.push(b'\n');
    }
    let mut ids = v1::all_ids(buf.as_slice()).expect("ids");
    assert_eq!(
        v1::edits(edit_text.as_slice(), tops, &mut ids).expect("the edits are format v1"),
        c.edits.len() as u64
    );
    let text = String::from_utf8(buf).expect("utf-8");
    let mut steps = Vec::new();
    for l in text.lines().skip(1) {
        flatten(serde_json::from_str(l).expect("json"), &mut steps);
    }
    let trace: Vec<Value> = std::fs::read_to_string(&out)
        .expect("the trace")
        .lines()
        .skip(1)
        .map(|l| serde_json::from_str(l).expect("json"))
        .collect();
    assert_eq!(steps.len(), trace.len());
    Run {
        meta: c.meta,
        steps,
        edits: c.edits,
        trace,
    }
}

fn named<'r>(r: &'r Run, name: &str) -> (usize, &'r Value) {
    let i = r
        .trace
        .iter()
        .position(|t| t["name"] == name)
        .unwrap_or_else(|| panic!("no {name}"));
    (i, &r.steps[i])
}

#[test]
fn tiny_document_replays() {
    let r = run();
    assert_eq!(r.meta["document"], "tiny");
    assert!(
        r.meta["pdftex_version"]
            .as_str()
            .is_some_and(|v| v.starts_with("This is pdfTeX")),
        "{}",
        r.meta["pdftex_version"]
    );
    assert_ne!(r.edits.len(), 0);
    // ids are the trace's, as strings; the flattened order is the trace's.
    for (i, s) in r.steps.iter().enumerate() {
        assert_eq!(s["id"], format!("s{i}"));
    }
    // `\def\x{Hello}`: a primitive, its writes as the log shows them.
    let (_, d) = named(&r, "\\def");
    assert_eq!(d["class"], "\\def");
    assert_eq!(
        d["writes"],
        json!([{"field": "cs:\\x", "locality": "local"}])
    );
    // `\m{ab}`: a call; the steps of `[#1]` are its children.
    let (m, mv) = named(&r, "\\m");
    assert!(mv["n_children"].as_u64().is_some_and(|n| n > 0));
    assert!(mv["writes"].is_null(), "a call's definitions are pending");
    assert!(mv.get("may_define").is_some());
    assert_eq!(r.trace[m + 1]["ch"], "[");
    // `\begingroup ... \endgroup`: open and close.
    let (b, bv) = named(&r, "\\begingroup");
    assert_eq!(bv["scope"], "open");
    let (eg, egv) = named(&r, "\\endgroup");
    assert!(eg > b);
    assert_eq!(egv["scope"], "close");
    // edits name top-level steps (`s...`), inserted steps are `e...`.
    for e in &r.edits {
        assert!(e["step"].as_str().is_some_and(|s| s.starts_with('s')));
        assert!(e["expect"]["steps_reached"].is_u64());
    }
}
