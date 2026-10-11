//! `phitex-trace record <main.tex> [--engine pdflatex|pdftex] [--out trace.jsonl] [--scratch DIR] [--conservative]`
//! `phitex-trace analyze <trace.jsonl> [--json report.json] [--samples N]`

use std::io::{self, BufReader};
use std::path::PathBuf;
use std::process::ExitCode;

use phitex_trace::analyze::{Graph, markdown};
use phitex_trace::parse::Options;
use phitex_trace::record::{Engine, parse_log_file, record};

const USAGE: &str = "usage:\n  phitex-trace record <main.tex> [--engine pdflatex|pdftex] [--out trace.jsonl] [--scratch DIR] [--conservative]\n  phitex-trace parse <job.log> --main <main.tex> [--engine NAME] [--out trace.jsonl] [--conservative]\n  phitex-trace analyze <trace.jsonl> [--json report.json] [--samples N]";

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
            let r = parse_log_file(&PathBuf::from(input), &main, &engine, &out, opts)
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
        _ => Err(USAGE.to_owned()),
    }
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
