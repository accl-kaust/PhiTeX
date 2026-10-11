//! The replay converter on a tiny plain document recorded through
//! `pdftex -ini`: the output validates, and the syntactic pass's view
//! lands on the steps it should.

use std::io::BufReader;
use std::path::PathBuf;

use phitex_trace::analyze::Graph;
use phitex_trace::parse::Options;
use phitex_trace::record::{Engine, record};
use phitex_trace::replay::{Provenance, convert, plan_edits, validate, validate_edits};
use serde_json::{Value, json};

const DOC: &str = "\\def\\x{Hello}\n\\expandafter\\def\\csname e\\the\\count0\\endcsname{y}\n\\def\\m#1{[#1]}\n\\def\\mk#1{\\def#1{z}}\n\\begingroup \\count1=5 \\m{ab}\\endgroup\n\\mk\\y\n\\x\n\\bye\n";

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
    let (meta, edit_values) = convert(open, load, &prov, &edits, &mut buf).expect("converts");
    let n = validate(buf.as_slice()).expect("the output validates");
    let mut edit_text = Vec::new();
    for e in &edit_values {
        serde_json::to_writer(&mut edit_text, e).expect("json");
        edit_text.push(b'\n');
    }
    assert_eq!(
        validate_edits(edit_text.as_slice(), n).expect("the edits validate"),
        edit_values.len() as u64
    );
    let text = String::from_utf8(buf).expect("utf-8");
    let steps: Vec<Value> = text
        .lines()
        .skip(1)
        .map(|l| serde_json::from_str(l).expect("json"))
        .collect();
    let trace: Vec<Value> = std::fs::read_to_string(&out)
        .expect("the trace")
        .lines()
        .skip(1)
        .map(|l| serde_json::from_str(l).expect("json"))
        .collect();
    assert_eq!(steps.len(), trace.len());
    Run {
        meta,
        steps,
        edits: edit_values,
        trace,
    }
}

/// The output step whose trace step has this name and writes this field.
fn writing<'r>(r: &'r Run, name: &str, field: &str) -> (usize, &'r Value) {
    let i = r
        .trace
        .iter()
        .position(|t| {
            t["name"] == name
                && t["writes"]
                    .as_array()
                    .is_some_and(|w| w.iter().any(|w| w["field"] == field))
        })
        .unwrap_or_else(|| panic!("no {name} writing {field}"));
    (i, &r.steps[i])
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
#[allow(clippy::many_single_char_names)]
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

    // `\def\x{Hello}`: a known extent (the body is no step), names `\x`.
    let (_, d) = writing(&r, "\\def", "cs:\\x");
    assert_eq!(d["consumes"], json!({"Known": 0}));
    assert_eq!(d["hints"]["claim"], json!({"Known": 0}));
    assert_eq!(d["may_define"], json!({"Names": ["cs:\\x"]}));
    assert_eq!(d["source"]["position"], "exact");
    assert_eq!(d["source"]["line"], 1);

    // `\expandafter\def\csname e\the\count0\endcsname`: the literal prefix.
    let (_, e) = writing(&r, "\\def", "cs:\\e1");
    assert_eq!(e["may_define"], json!({"Prefixes": ["cs:\\e"]}));
    assert_eq!(e["source"]["line"], 2);
    // ... and `\expandafter` itself: pending, as the pass says.
    let (_, x) = named(&r, "\\expandafter");
    assert_eq!(x["consumes"], "Pending");
    assert_eq!(x["hints"]["claim"], "Pending");
    assert_eq!(x["hints"]["cause"], "\\expandafter");

    // `\mk\y` runs `\def#1{z}` from `\mk`'s body: its target is no
    // literal there, so it may define anything.
    let (_, y) = writing(&r, "\\def", "cs:\\y");
    assert_eq!(y["may_define"], "Any");
    assert_eq!(y["source"]["position"], "call-site");
    // `\mk` the call: its body defines through a parameter.
    let (_, mk) = named(&r, "\\mk");
    assert_eq!(mk["may_define"], "Any");

    // `\m{ab}`: the call produces the steps of `[#1]`.
    let (m, mv) = named(&r, "\\m");
    let produced: Vec<u64> = mv["unfold"]["produced"]
        .as_array()
        .expect("an unfold")
        .iter()
        .map(|v| v.as_u64().expect("an id"))
        .collect();
    assert_eq!(produced.first(), Some(&(m as u64 + 1)));
    assert_eq!(r.trace[m + 1]["ch"], "[");
    assert!(mv["consumes"]["Known"].is_u64());
    assert!(r.steps[m + 1]["unfold"].is_null());

    // Nesting across `\begingroup ... \endgroup`.
    let (b, bv) = named(&r, "\\begingroup");
    assert_eq!(bv["nesting_depth"], 0);
    assert_eq!(bv["hints"]["scope"], json!({"Known": "Open"}));
    let (_, c) = writing(&r, "\\count", "count:1");
    assert_eq!(c["nesting_depth"], 1);
    let (eg, egv) = named(&r, "\\endgroup");
    assert!(eg > b);
    assert_eq!(egv["hints"]["scope"], json!({"Known": "Close"}));
    assert_eq!(r.steps[eg + 1]["nesting_depth"], 0);

    // Text order follows the source where the steps joined it.
    let order = |v: &Value| v["text_order"].as_u64().expect("an order");
    assert!(order(d) < order(e) && order(e) < order(bv) && order(bv) < order(egv));
}
