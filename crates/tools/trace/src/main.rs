//! `phitex-trace record <main.tex> [--engine pdflatex|pdftex] [--out trace.jsonl] [--scratch DIR] [--conservative]`
//! `phitex-trace analyze <trace.jsonl> [--json report.json] [--samples N]`

use std::io::{self, BufReader};
use std::path::PathBuf;
use std::process::ExitCode;

use phitex_trace::analyze::{Graph, Report, markdown, table};
use phitex_trace::parse::Options;
use phitex_trace::record::{Engine, parse_log_file, record};

const USAGE: &str = "usage:\n  phitex-trace record <main.tex> [--engine pdflatex|pdftex] [--out trace.jsonl] [--scratch DIR] [--conservative]\n  phitex-trace parse <job.log> --main <main.tex> [--engine NAME] [--out trace.jsonl] [--conservative]\n  phitex-trace analyze <trace.jsonl> [--json report.json] [--samples N]\n  phitex-trace table <report.json>...\n  phitex-trace replay <trace.jsonl> --out-dir DIR [--doc NAME] [--dirs D1:D2] [--commit SHA] [--edits N]\n  phitex-trace validate <replay.jsonl> [<edits.jsonl>]\n  phitex-trace index <replay-dir>";

#[allow(clippy::too_many_lines)]
fn run(args: &[String]) -> Result<(), String> {
    let Some(cmd) = args.first() else {
        return Err(USAGE.to_owned());
    };
    let mut positional = Vec::new();
    let mut flags: Vec<(String, Option<String>)> = Vec::new();
    let mut i = 1;
    while i < args.len() {
        let a = &args[i];
        if let Some(name) = a.strip_prefix("--") {
            if name == "conservative" {
                flags.push((name.to_owned(), None));
            } else {
                let v = args
                    .get(i + 1)
                    .ok_or_else(|| format!("--{name} needs a value"))?;
                flags.push((name.to_owned(), Some(v.clone())));
                i += 1;
            }
        } else {
            positional.push(a.clone());
        }
        i += 1;
    }
    let flag = |n: &str| {
        flags
            .iter()
            .find(|(k, _)| k == n)
            .map(|(_, v)| v.clone().unwrap_or_default())
    };
    let input = positional.first().ok_or_else(|| USAGE.to_owned())?;
    match cmd.as_str() {
        "record" => {
            let main = PathBuf::from(input);
            let engine = Engine::parse(flag("engine").as_deref().unwrap_or("pdflatex"))?;
            let stem = main
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("trace")
                .to_owned();
            let out = flag("out").map_or_else(
                || PathBuf::from(format!("{stem}.trace.jsonl")),
                PathBuf::from,
            );
            let scratch = flag("scratch").map_or_else(
                || {
                    std::env::var_os("CLAUDE_JOB_DIR")
                        .map(PathBuf::from)
                        .map_or_else(|| PathBuf::from("target"), |d| d.join("tmp"))
                        .join("phitex-trace")
                        .join(&stem)
                },
                PathBuf::from,
            );
            let opts = Options {
                conservative: flag("conservative").is_some(),
            };
            let r = record(&main, engine, &scratch, &out, opts).map_err(|e| e.to_string())?;
            eprintln!(
                "{}: {} steps ({} log lines not recognised), engine exit {:?}, log {}",
                out.display(),
                r.steps,
                r.unrecognised,
                r.status,
                r.log.display()
            );
            Ok(())
        }
        "parse" => {
            let main = flag("main").ok_or("--main <main.tex> names the document the log is of")?;
            let out = flag("out").map_or_else(|| PathBuf::from("trace.jsonl"), PathBuf::from);
            let opts = Options {
                conservative: flag("conservative").is_some(),
            };
            let engine = flag("engine").unwrap_or_else(|| "unknown".to_owned());
            let r = parse_log_file(&PathBuf::from(input), &main, &engine, None, &out, opts)
                .map_err(|e| e.to_string())?;
            eprintln!(
                "{}: {} steps ({} log lines not recognised)",
                out.display(),
                r.0,
                r.1
            );
            Ok(())
        }
        "analyze" => {
            let samples = flag("samples")
                .map_or(Ok(200), |s| s.parse::<usize>().map_err(|e| e.to_string()))?;
            let f = std::fs::File::open(input).map_err(|e| format!("{input}: {e}"))?;
            let g = Graph::load(BufReader::with_capacity(1 << 20, f)).map_err(|e| e.to_string())?;
            let report = g.report(samples);
            print!("{}", markdown(&report));
            if let Some(p) = flag("json") {
                let j = serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?;
                std::fs::write(&p, j).map_err(|e| format!("{p}: {e}"))?;
            }
            Ok(())
        }
        "table" => {
            let mut reports = Vec::new();
            for p in &positional {
                let text = std::fs::read_to_string(p).map_err(|e| format!("{p}: {e}"))?;
                reports
                    .push(serde_json::from_str::<Report>(&text).map_err(|e| format!("{p}: {e}"))?);
            }
            print!("{}", table(&reports));
            Ok(())
        }
        "replay" => replay(input, &flag),
        "validate" => {
            let open = |p: &str| {
                std::fs::File::open(p)
                    .map(|f| BufReader::with_capacity(1 << 20, f))
                    .map_err(|e| format!("{p}: {e}"))
            };
            let tops =
                phitex_trace::v1::trace(open(input)?).map_err(|e| format!("{input}: {e}"))?;
            if let Some(e) = positional.get(1) {
                let mut ids = phitex_trace::v1::all_ids(open(input)?)?;
                let k = phitex_trace::v1::edits(open(e)?, tops.clone(), &mut ids)
                    .map_err(|x| format!("{e}: {x}"))?;
                eprintln!("{e}: {k} edits valid");
            }
            eprintln!("{input}: {} top-level steps valid", tops.len());
            Ok(())
        }
        "index" => {
            let dir = PathBuf::from(input);
            let mut metas = Vec::new();
            let mut entries: Vec<_> = std::fs::read_dir(&dir)
                .map_err(|e| format!("{input}: {e}"))?
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.join("meta.json").is_file())
                .collect();
            entries.sort();
            for p in entries {
                let text =
                    std::fs::read_to_string(p.join("meta.json")).map_err(|e| e.to_string())?;
                let v: serde_json::Value =
                    serde_json::from_str(&text).map_err(|e| e.to_string())?;
                let name = p
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("?")
                    .to_owned();
                metas.push((name, v));
            }
            print!("{}", phitex_trace::replay::index(&metas));
            Ok(())
        }
        _ => Err(USAGE.to_owned()),
    }
}

/// Reads a file the log names: absolute, or relative to one of `dirs`.
fn load(dirs: &[PathBuf], name: &str) -> Option<String> {
    let p = std::path::Path::new(name);
    let candidates: Vec<PathBuf> = if p.is_absolute() {
        vec![p.to_path_buf()]
    } else {
        let rel = name.strip_prefix("./").unwrap_or(name);
        dirs.iter().map(|d| d.join(rel)).collect()
    };
    candidates
        .iter()
        .find_map(|c| std::fs::read(c).ok())
        .map(|b| String::from_utf8_lossy(&b).into_owned())
}

fn replay(input: &str, flag: &dyn Fn(&str) -> Option<String>) -> Result<(), String> {
    use phitex_trace::replay::{Provenance, convert, plan_edits};
    let out_dir = PathBuf::from(flag("out-dir").ok_or("--out-dir DIR")?);
    std::fs::create_dir_all(&out_dir).map_err(|e| e.to_string())?;
    let dirs: Vec<PathBuf> = flag("dirs")
        .unwrap_or_default()
        .split(':')
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .collect();
    let samples =
        flag("edits").map_or(Ok(200), |s| s.parse::<usize>().map_err(|e| e.to_string()))?;
    let open = || std::fs::File::open(input).map(|f| BufReader::with_capacity(1 << 20, f));
    // The edits, from the edit-impact analysis's sample; its graph also
    // gives the expectations of edits retargeted to a top-level call.
    let graph =
        Graph::load(open().map_err(|e| format!("{input}: {e}"))?).map_err(|e| e.to_string())?;
    let edits = plan_edits(&graph, samples);
    let prov = Provenance {
        document: flag("doc").unwrap_or_else(|| input.to_owned()),
        trace_commit: flag("commit").unwrap_or_default(),
    };
    let trace_out = out_dir.join("trace.jsonl");
    let mut w = std::io::BufWriter::with_capacity(
        1 << 20,
        std::fs::File::create(&trace_out).map_err(|e| e.to_string())?,
    );
    let c = convert(
        open,
        |n| load(&dirs, n),
        &prov,
        &edits,
        Some(&graph),
        &mut w,
    )
    .map_err(|e| format!("{input}: {e}"))?;
    drop(w);
    drop(graph);
    let lines = |vs: &[serde_json::Value]| -> Result<String, String> {
        let mut text = String::new();
        for e in vs {
            text.push_str(&serde_json::to_string(e).map_err(|e| e.to_string())?);
            text.push('\n');
        }
        Ok(text)
    };
    let edits_out = out_dir.join("edits.jsonl");
    std::fs::write(&edits_out, lines(&c.edits)?).map_err(|e| e.to_string())?;
    std::fs::write(out_dir.join("edits.meta.jsonl"), lines(&c.edits_meta)?)
        .map_err(|e| e.to_string())?;
    std::fs::write(
        out_dir.join("meta.json"),
        serde_json::to_string_pretty(&c.meta).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let rd = |p: &PathBuf| {
        std::fs::File::open(p)
            .map(|f| BufReader::with_capacity(1 << 20, f))
            .map_err(|e| format!("{}: {e}", p.display()))
    };
    let tops = phitex_trace::v1::trace(rd(&trace_out)?)
        .map_err(|e| format!("{}: {e}", trace_out.display()))?;
    let n = tops.len();
    let mut ids = phitex_trace::v1::all_ids(rd(&trace_out)?)?;
    let k = phitex_trace::v1::edits(rd(&edits_out)?, tops, &mut ids)
        .map_err(|e| format!("{}: {e}", edits_out.display()))?;
    eprintln!(
        "{}: {n} top-level steps, {k} edits, valid (format v1)",
        out_dir.display()
    );
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            let _ = io::Write::write_all(&mut io::stderr(), format!("{e}\n").as_bytes());
            ExitCode::FAILURE
        }
    }
}
