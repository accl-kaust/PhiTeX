//! `\write18` through the host's hook (`phitex_tex::ShellHook`,
//! docs/note-shell-hook.md), with a fake hook in the process: the files a
//! command is handed, what it made read after it in TeX's order, its
//! calls memoised by the files it read, the files commands made kept
//! across evaluations as pdflatex's directory across runs, the
//! evaluation going on while a command runs, and a command refused
//! without a hook.

mod common;

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex};

use phitex_tex::{Options, ShellHook, ShellRan};

/// The fake commands (`fakecmd WHAT ARGS`, quoted as restricted mode
/// quotes them):
/// - `up IN OUT`: `OUT` is `\gdef\made{TEXT}` with `IN`'s text in capitals;
///   reads `IN`;
/// - `hl IN OUT`: as minted's `highlight`: `OUT` (a cache file) from
///   `IN`'s text in brackets, `IN` removed; reads `IN`;
/// - `sleep`: 2 s, noting the jobs the build finished meanwhile.
#[derive(Default)]
struct Fake {
    /// Each call: the command and the texts of the files it read.
    calls: Mutex<Vec<String>>,
    progress: Mutex<Option<Arc<phitex_tex::Progress>>>,
    /// The jobs finished while `sleep` slept.
    during: Mutex<Vec<u64>>,
}

impl Fake {
    /// The calls made on their files (a run on files not written yet, a
    /// speculative one thrown away, is not counted).
    fn distinct(&self) -> BTreeSet<String> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| !c.ends_with("<- None"))
            .cloned()
            .collect()
    }
}

fn file<'a>(files: &'a [(String, Arc<[u8]>)], name: &str) -> Option<&'a [u8]> {
    files.iter().find(|(n, _)| n == name).map(|(_, b)| &b[..])
}

impl ShellHook for Fake {
    fn run(&self, command: &str, files: &[(String, Arc<[u8]>)]) -> ShellRan {
        let w: Vec<String> = command
            .split_whitespace()
            .map(|a| a.trim_matches('\'').to_owned())
            .collect();
        let arg = |i: usize| w.get(i).cloned().unwrap_or_default();
        let input = file(files, &arg(2)).map(|b| String::from_utf8_lossy(b).trim_end().to_owned());
        self.calls
            .lock()
            .unwrap()
            .push(format!("{command} <- {input:?}"));
        match arg(1).as_str() {
            "up" => {
                let Some(t) = input else {
                    return ShellRan {
                        status: 1 << 8,
                        ..ShellRan::default()
                    };
                };
                ShellRan {
                    status: 0,
                    wrote: vec![(
                        arg(3),
                        format!("\\gdef\\made{{{}}}\n", t.to_uppercase()).into_bytes(),
                    )],
                    removed: Vec::new(),
                    read: Some(vec![arg(2)]),
                }
            }
            "hl" => {
                let Some(t) = input else {
                    return ShellRan {
                        status: 1 << 8,
                        ..ShellRan::default()
                    };
                };
                ShellRan {
                    status: 0,
                    wrote: vec![(arg(3), format!("\\gdef\\made{{[{t}]}}\n").into_bytes())],
                    removed: vec![arg(2)],
                    read: Some(vec![arg(2)]),
                }
            }
            "sleep" => {
                let p = self.progress.lock().unwrap().clone();
                let before = p.as_ref().map_or(0, |p| p.done());
                std::thread::sleep(std::time::Duration::from_secs(2));
                let after = p.as_ref().map_or(0, |p| p.done());
                self.during
                    .lock()
                    .unwrap()
                    .push(after.saturating_sub(before));
                ShellRan {
                    read: Some(Vec::new()),
                    ..ShellRan::default()
                }
            }
            _ => ShellRan {
                status: 127 << 8,
                ..ShellRan::default()
            },
        }
    }
}

fn options(name: &str, hook: Option<Arc<Fake>>, threads: usize) -> Options {
    let dir = common::fixtures().join("plain");
    let mut cmds = phitex_tex::texlive_commands();
    cmds.push("fakecmd".to_owned());
    Options {
        threads: Some(threads),
        shell_commands: cmds,
        shell_hook: hook.map(|h| h as Arc<dyn ShellHook>),
        ..common::options(&format!("./{name}.tex"), name, "", &dir)
    }
}

/// `\made` written to the log, as `GOT:<made>`.
const SHOW: &str = "\\immediate\\write16{GOT:\\made}";

/// A job writes a file, closes it, runs a command over it and reads what
/// the command wrote, all in one run.
fn up_doc(a: &str, b: &str) -> String {
    format!(
        r"\def\made{{none}}
\immediate\openout3=ina.txt \immediate\write3{{{a}}}\immediate\closeout3
\immediate\openout4=inb.txt \immediate\write4{{{b}}}\immediate\closeout4
Before the commands.
\immediate\write18{{fakecmd up ina.txt outa.tex}}
\openin1=outa.tex \ifeof1 \else\closein1 \input outa.tex \fi
{SHOW}
\par Between them.
\immediate\write18{{fakecmd up inb.txt outb.tex}}
\openin1=outb.tex \ifeof1 \else\closein1 \input outb.tex \fi
{SHOW}
\bye
"
    )
}

fn hits(log: &str) -> Vec<String> {
    log.lines()
        .filter_map(|l| l.strip_prefix("HIT:").map(str::to_owned))
        .collect()
}

fn got(log: &str) -> Vec<String> {
    log.lines()
        .filter_map(|l| l.strip_prefix("GOT:").map(str::to_owned))
        .collect()
}

/// What a command made is read right after it in TeX's order; a call is
/// made again only when a file it read changed: an edit of one input
/// calls the one command that read it (the other command, the same
/// words over other files, is answered from the memo), and an edit of
/// the text around calls nothing. The edited build equals a cold build
/// of its text. At -j1 and -j8.
#[test]
fn a_command_reads_what_the_job_wrote_and_its_output_is_read_after_it() {
    for threads in [1, 8] {
        let fake = Arc::new(Fake::default());
        let mut b = common::build(
            options("up", Some(fake.clone()), threads),
            Some(&common::plain()),
            &up_doc("alpha", "beta"),
        );
        let (_, log) = b.output();
        assert_eq!(got(&log), ["ALPHA", "BETA"], "-j{threads}: {log}");
        let n0 = fake.distinct().len();
        assert_eq!(n0, 2, "-j{threads}: {:?}", fake.distinct());
        // (an input edited: one call more)
        b.edit(&up_doc("gamma", "beta"));
        let (dvi, log) = b.output();
        assert_eq!(got(&log), ["GAMMA", "BETA"], "-j{threads}: {log}");
        assert_eq!(
            fake.distinct().len(),
            n0 + 1,
            "-j{threads}: {:?}",
            fake.distinct()
        );
        let cold = common::build(
            options("up", Some(Arc::new(Fake::default())), threads),
            Some(&common::plain()),
            &up_doc("gamma", "beta"),
        );
        assert_eq!(
            common::first_difference(&dvi, &cold.output().0),
            None,
            "-j{threads}"
        );
        // (the text around edited: no call)
        let n1 = fake.distinct().len();
        b.edit(&up_doc("gamma", "beta").replace("Between them.", "Between the two."));
        assert_eq!(got(&b.output().1), ["GAMMA", "BETA"]);
        assert_eq!(
            fake.distinct().len(),
            n1,
            "-j{threads}: {:?}",
            fake.distinct()
        );
    }
}

/// A command whose output the job reads only where it is missing, as
/// minted reads its cache: the first evaluation runs the commands (the
/// cache is empty), they remove their input, and the build evaluates
/// again from the files they made, as pdflatex's second run reads the
/// directory its first left (`Stats::shell_passes` 1); that evaluation
/// runs none. One listing edited: one call. At -j1 and -j8.
fn cache_doc(keys: &[(&str, &str)]) -> String {
    let mut s = String::from(
        r"\def\made{none}
\def\listing#1#2{\openin1=cache/#1.tex
  \ifeof1
    \immediate\openout3=data.tmp \immediate\write3{#2}\immediate\closeout3
    \immediate\write18{fakecmd hl data.tmp cache/#1.tex}%
  \else \closein1 \immediate\write16{HIT:#1}\fi
  \openin1=cache/#1.tex \ifeof1 \gdef\made{MISSING}\else\closein1 \input cache/#1.tex \fi
  \immediate\write16{GOT:\made}\par}
",
    );
    for (k, t) in keys {
        let _ = write!(s, "Text before {k}.\n\\listing{{{k}}}{{{t}}}\n\n");
    }
    s.push_str("\\bye\n");
    s
}

#[test]
fn files_commands_make_stay_for_the_next_evaluation() {
    let keys = [("k1", "one"), ("k2", "two"), ("k3", "three")];
    // (with the files the job writes read back at their fixpoint, as
    // latexmk runs it, and without)
    for (threads, fixpoint) in [(1, false), (8, false), (1, true), (8, true)] {
        let opts = |h: Arc<Fake>| Options {
            fixpoint,
            ..options("cache", Some(h), threads)
        };
        let fake = Arc::new(Fake::default());
        let mut b = common::build(
            opts(fake.clone()),
            Some(&common::plain()),
            &cache_doc(&keys),
        );
        let (_, log) = b.output();
        assert_eq!(
            got(&log),
            ["[one]", "[two]", "[three]"],
            "-j{threads}: {log}"
        );
        // (at a fixpoint the last evaluation read the files the first
        // made, as pdflatex's second run; one run from nothing reads none
        // before its command, as TeX's first run)
        let want: &[&str] = if fixpoint { &["k1", "k2", "k3"] } else { &[] };
        assert_eq!(hits(&log), want, "-j{threads} fixpoint {fixpoint}: {log}");
        assert_eq!(
            b.stats.shell_passes,
            u64::from(fixpoint),
            "-j{threads} fixpoint {fixpoint}"
        );
        assert_eq!(
            fake.distinct().len(),
            3,
            "-j{threads}: {:?}",
            fake.distinct()
        );
        let files = b.shell_files();
        assert!(
            files
                .iter()
                .any(|(n, t)| n == "cache/k2.tex" && t.is_some()),
            "{files:?}"
        );
        // (one listing edited: its call alone, then read from the store)
        let edited = [("k1", "one"), ("k2b", "deux"), ("k3", "three")];
        b.edit(&cache_doc(&edited));
        let (dvi, log) = b.output();
        assert_eq!(
            got(&log),
            ["[one]", "[deux]", "[three]"],
            "-j{threads}: {log}"
        );
        let want: &[&str] = if fixpoint { &["k1", "k2b", "k3"] } else { &[] };
        assert_eq!(hits(&log), want, "-j{threads} fixpoint {fixpoint}: {log}");
        assert_eq!(
            b.stats.shell_passes,
            u64::from(fixpoint),
            "-j{threads} fixpoint {fixpoint}"
        );
        assert_eq!(
            fake.distinct().len(),
            4,
            "-j{threads}: {:?}",
            fake.distinct()
        );
        let cold = common::build(
            opts(Arc::new(Fake::default())),
            Some(&common::plain()),
            &cache_doc(&edited),
        );
        assert_eq!(
            common::first_difference(&dvi, &cold.output().0),
            None,
            "-j{threads}"
        );
    }
}

/// A read of a name before the command that makes it, in TeX's order,
/// finds no file in one run from nothing, as TeX's first run does
/// (`pdftex -shell-restricted` once: minted's placeholders before its
/// `batch` at the end); a read after it finds the file. At a fixpoint
/// (latexmk's runs) the read before it finds what the run before made,
/// as pdflatex's second run finds it in the directory. At -j1 and -j8.
#[test]
fn a_read_before_the_command_finds_its_file_only_in_the_next_run() {
    let doc = r"\def\made{none}
\openin1=outa.tex \ifeof1 \immediate\write16{EARLY:missing}\else\closein1 \immediate\write16{EARLY:found}\fi
Text before.\par
\immediate\openout3=ina.txt \immediate\write3{alpha}\immediate\closeout3
\immediate\write18{fakecmd up ina.txt outa.tex}
\openin1=outa.tex \ifeof1 \immediate\write16{LATE:missing}\else\closein1 \immediate\write16{LATE:found}\fi
\bye
";
    let lines = |log: &str, p: &str| -> Vec<String> {
        log.lines()
            .filter_map(|l| l.strip_prefix(p).map(str::to_owned))
            .collect()
    };
    for threads in [1, 8] {
        for fixpoint in [false, true] {
            let opts = Options {
                fixpoint,
                ..options("early", Some(Arc::new(Fake::default())), threads)
            };
            let b = common::build(opts, Some(&common::plain()), doc);
            let (_, log) = b.output();
            let early = if fixpoint { "found" } else { "missing" };
            assert_eq!(
                lines(&log, "EARLY:"),
                [early],
                "-j{threads} fixpoint {fixpoint}: {log}"
            );
            assert_eq!(
                lines(&log, "LATE:"),
                ["found"],
                "-j{threads} fixpoint {fixpoint}: {log}"
            );
            assert!(
                b.shell_files()
                    .iter()
                    .any(|(n, t)| n == "outa.tex" && t.is_some()),
                "-j{threads} fixpoint {fixpoint}: {:?}",
                b.shell_files()
            );
        }
    }
}

/// Without a hook, a command the engine has no in-process version of is
/// refused with a diagnostic, and nothing it would make is read.
#[test]
fn a_command_is_refused_without_a_hook() {
    let b = common::build(
        options("up", None, 1),
        Some(&common::plain()),
        &up_doc("alpha", "beta"),
    );
    let (_, log) = b.output();
    assert_eq!(got(&log), ["none", "none"], "{log}");
    assert!(
        b.diagnostics()
            .iter()
            .any(|d| d.message.contains("fakecmd") && d.message.contains("not run")),
        "{:?}",
        b.diagnostics()
            .iter()
            .map(|d| &d.message)
            .collect::<Vec<_>>()
    );
}

/// A command not in the restricted list is not run, hook or not.
#[test]
fn a_command_not_allowed_is_not_run() {
    let fake = Arc::new(Fake::default());
    let opts = Options {
        shell_commands: phitex_tex::texlive_commands(),
        ..options("up", Some(fake.clone()), 1)
    };
    let b = common::build(opts, Some(&common::plain()), &up_doc("alpha", "beta"));
    assert_eq!(got(&b.output().1), ["none", "none"]);
    assert!(fake.distinct().is_empty());
}

/// A command that takes 2 s holds nothing the other workers need: the
/// 300 paragraphs after it are run meanwhile, at -j8.
#[test]
fn the_evaluation_goes_on_while_a_command_runs() {
    let (src, _) = common::plain_paragraphs(300, false);
    let src = src.replacen(
        "\\parindent=20pt\n",
        "\\parindent=20pt\nFirst.\\immediate\\write18{fakecmd sleep}\\par\n",
        1,
    );
    let fake = Arc::new(Fake::default());
    let progress = Arc::new(phitex_tex::Progress::new(8));
    *fake.progress.lock().unwrap() = Some(progress.clone());
    let opts = Options {
        progress: Some(progress),
        ..options("sleep", Some(fake.clone()), 8)
    };
    let t = std::time::Instant::now();
    let b = common::build(opts, Some(&common::plain()), &src);
    let wall = t.elapsed();
    let during = fake.during.lock().unwrap().clone();
    eprintln!(
        "sleep: wall {wall:?}, jobs finished while the command ran {during:?}, {} pages",
        b.pages().len()
    );
    assert!(!during.is_empty(), "the command never ran");
    assert!(
        during.iter().any(|&n| n >= 300),
        "jobs finished while it slept: {during:?}"
    );
}
