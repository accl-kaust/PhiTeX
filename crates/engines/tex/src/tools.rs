//! The programs a job's reads run, as latexmk runs them between
//! pdflatex's runs: BibTeX, biber and makeindex, in process
//! (`phitex-bibtex`, `phitex-biber`, `phitex-makeindex`).
//!
//! A tool is driven by a read, never by a write. When TeX opens a name
//! the job does not write itself ([`crate::run::Run::file`]), each tool's
//! crate is asked whether that name is its default output for an input
//! the job writes (makeindex: `X.ind` from `X.idx`). The read is then the
//! output of a memoised application of [`ToolRun`] (`Ctx::apply`): it
//! reads the input as the job wrote it the round before (`wtext:<name>`,
//! a loop-back the files' node publishes, `ops::Files`) and its support
//! files through the client's lookups for the tool
//! ([`crate::api::ToolSearch`]). So a tool runs only where its output is
//! read, and again only when what it read changed: the engine's
//! memoisation, no step between rounds. A name no tool claims, or one
//! whose input the job does not write, is read from the file system.
//!
//! A `\write18` naming a tool is a node of its own ([`ToolNode`]),
//! placed where the command is in this round (the run making it ends
//! there, its rest a resume after the node, `crate::pages`): it reads the
//! input's stream as the job wrote it up to there, this round, and
//! defines the output's field (`toolout:<name>`, [`out_field`]), which a
//! later read of that name reads.

use phitex_defuse::{Ctx, Field, Locality, Operation, Stop, ValueId, Values};

use crate::api::ToolSearch;
use crate::ops::{Shared, ops};
use crate::val::{list_of, str_of};

/// The output a `\write18`'s tool node made of a name ([`ToolNode`]):
/// its [`ToolRun`] value (`UNIT`: it made none), read where the name is
/// opened after the node.
pub(crate) fn out_field(v: &Values, name: &str) -> Field {
    v.field(&format!("toolout:{name}"))
}

/// The loop-back of a file the job writes, its whole text as the round
/// before wrote it (`UNIT`: not written), published by the files' node
/// for the tools that read it.
pub(crate) fn text_field(v: &Values, name: &str) -> Field {
    v.field(&format!("wtext:{name}"))
}

/// The tools, in the order a name more than one claims is given to (the
/// first whose input the job writes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tool {
    Biber,
    Bibtex,
    Makeindex,
}

impl Tool {
    const ALL: [Tool; 3] = [Tool::Biber, Tool::Bibtex, Tool::Makeindex];

    /// The program's name (a `\write18` command's first word).
    pub(crate) fn name(self) -> &'static str {
        match self {
            Tool::Biber => "biber",
            Tool::Bibtex => "bibtex",
            Tool::Makeindex => "makeindex",
        }
    }

    pub(crate) fn of_name(s: &str) -> Option<Tool> {
        Tool::ALL.into_iter().find(|t| t.name() == s)
    }

    /// The input the tool makes `output` from by default (its crate's
    /// predicate on the output's name).
    fn default_input(self, output: &str) -> Option<String> {
        match self {
            Tool::Biber => phitex_biber::default_input_for(output),
            Tool::Bibtex => phitex_bibtex::default_input_for(output),
            Tool::Makeindex => phitex_makeindex::default_input_for(output),
        }
    }

    /// The output and input of the tool run with `args` (its crate's
    /// command line).
    pub(crate) fn command_files(self, args: &[String]) -> Option<(String, String)> {
        match self {
            Tool::Biber => {
                let f = args.iter().find(|a| !a.starts_with('-'))?;
                let base = f.strip_suffix(".bcf").unwrap_or(f);
                Some((format!("{base}.bbl"), format!("{base}.bcf")))
            }
            Tool::Bibtex => phitex_bibtex::command_files(args),
            Tool::Makeindex => phitex_makeindex::command_files(args),
        }
    }
}

/// The tools that may make `output` by default, each with its input.
pub(crate) fn claims(output: &str) -> Vec<(Tool, String)> {
    Tool::ALL
        .into_iter()
        .filter_map(|t| t.default_input(output).map(|i| (t, i)))
        .collect()
}

/// TeX Live 2026's `shell_escape_commands` (texmf.cnf): what restricted
/// mode allows.
pub(crate) const SHELL_ESCAPE_COMMANDS: &[&str] = &[
    "bibtex",
    "bibtex8",
    "extractbb",
    "gregorio",
    "kpsewhich",
    "makeindex",
    "repstopdf",
    "r-mpost",
    "texosquery-jre8",
];

/// The tools' settings the client gives ([`crate::Options::bibtex`]).
#[derive(Clone, Default)]
pub(crate) struct ToolOptions {
    pub bibtex: phitex_bibtex::Options,
}

/// makeindex's banner: 2.18 as TeX Live 2026 builds it.
const MAKEINDEX_VERSION: &[u8] = b"version 2.18 [TeX Live 2026] (kpathsea + Thai support)";

/// TeX's characters as bytes (Latin-1; a character past 255 as `?`).
fn bytes(s: &str) -> Vec<u8> {
    s.chars().map(|c| u8::try_from(c).unwrap_or(b'?')).collect()
}

/// Bytes as TeX's characters (each byte one character).
fn chars(b: &[u8]) -> String {
    b.iter().map(|&c| char::from(c)).collect()
}

/// A file a tool asks for that the run has not looked up yet.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
enum Need {
    /// By name as given: a file the job writes (its text the round
    /// before), else the job's own lookup (`file:<name>`).
    File(String),
    /// Through the tool's search.
    Support(ToolSearch, String),
}

/// The files a tool's run is served: what was looked up so far, and what
/// it asked for that was not ([`ToolRun`] looks those up and runs the
/// tool again, until it asks for nothing new).
#[derive(Default)]
struct Served {
    got: crate::FxHashMap<Need, Option<(String, Vec<u8>)>>,
    missing: Vec<Need>,
}

impl Served {
    fn get(&mut self, n: Need) -> Option<(String, Vec<u8>)> {
        if let Some(x) = self.got.get(&n) {
            return x.clone();
        }
        if !self.missing.contains(&n) {
            self.missing.push(n);
        }
        None
    }

    fn file(&mut self, name: &[u8]) -> Option<Vec<u8>> {
        let n = chars(name);
        let n = n.strip_prefix("./").unwrap_or(&n).to_owned();
        self.get(Need::File(n)).map(|(_, b)| b)
    }
}

impl phitex_bibtex::Files for Served {
    fn aux(&mut self, name: &[u8]) -> Option<Vec<u8>> {
        self.file(name)
    }
    fn bst(&mut self, name: &[u8]) -> Option<Vec<u8>> {
        let n = chars(name);
        let n = if n.contains('.') {
            n
        } else {
            format!("{n}.bst")
        };
        self.get(Need::Support(ToolSearch::BibStyle, n))
            .map(|(_, b)| b)
    }
    fn bib(&mut self, name: &[u8]) -> Option<Vec<u8>> {
        let n = chars(name);
        let n = if n.to_ascii_lowercase().ends_with(".bib") {
            n
        } else {
            format!("{n}.bib")
        };
        self.get(Need::Support(ToolSearch::BibData, n))
            .map(|(_, b)| b)
    }
}

impl phitex_biber::Files for Served {
    fn datasource(&mut self, name: &str) -> Option<(String, Vec<u8>)> {
        if let Some((_, b)) = self.get(Need::File(name.to_owned())) {
            return Some((name.to_owned(), b));
        }
        self.get(Need::Support(ToolSearch::BibData, name.to_owned()))
    }
}

impl phitex_makeindex::Files for Served {
    fn read(&mut self, name: &[u8]) -> Option<Vec<u8>> {
        self.file(name)
    }
    fn readable(&mut self, name: &[u8]) -> bool {
        self.file(name).is_some()
    }
    fn style(&mut self, name: &[u8]) -> Option<(Vec<u8>, Option<Vec<u8>>)> {
        let n = chars(name);
        self.get(Need::Support(ToolSearch::IndexStyle, n))
            .map(|(p, b)| (bytes(&p), Some(b)))
    }
    fn out_name_ok(&mut self, _name: &[u8]) -> Result<(), Vec<u8>> {
        Ok(())
    }
    fn stdin(&mut self) -> Vec<u8> {
        Vec::new()
    }
}

/// The environment field a support file is looked up by
/// (`tfile:<search>:<name>`, answered by the client's lookup for tools,
/// [`crate::Options::tool_finder`]).
pub(crate) fn support_field(v: &Values, s: ToolSearch, name: &str) -> Field {
    v.field(&format!("tfile:{}:{name}", s.code()))
}

/// Looks up what a run asked for: a file the job writes, by its text
/// the round before; else through the job's or the tool's lookup.
fn look_up(cx: &mut Ctx<'_>, n: &Need) -> Result<Option<(String, Vec<u8>)>, Stop> {
    let v = cx.values();
    let file = |x: Option<ValueId>| {
        x.and_then(|x| {
            let xs = list_of(v, x);
            match (xs.first(), xs.get(1)) {
                (Some(&p), Some(&t)) => Some((str_of(v, p), bytes(&str_of(v, t)))),
                _ => None,
            }
        })
    };
    match n {
        Need::File(name) => {
            // (a file the job writes: its text the round before, from its
            // loop-back, never the file system's copy the job is writing;
            // one it does not write: the job's lookup)
            let w = text_field(v, name);
            if let Some(x) = cx.use_field(w)? {
                return Ok(
                    (x != ValueId::UNIT).then(|| (format!("./{name}"), bytes(&str_of(v, x))))
                );
            }
            Ok(file(cx.use_env(v.field(&format!("file:{name}")))?))
        }
        Need::Support(s, name) => Ok(file(cx.use_env(support_field(v, *s, name))?)),
    }
}

/// A tool's run on the files the job wrote the round before
/// (`Ctx::apply`): `[tool, output, input, [args...]]` (and the input's
/// text where a `\write18`'s node read it, [`ToolNode`]), its output
/// `[path, text]` (TeX's characters), or `UNIT` where the tool makes
/// none (its input not written, a bibliography asked for by none, a
/// failed run). The output is `[path, text, 0]`: read inline where it is
/// opened.
pub(crate) struct ToolRun(pub Shared);

impl Operation for ToolRun {
    fn name(&self) -> &'static str {
        "tool"
    }
    fn defs(&self, _: &[ValueId], _: &Values) -> Option<Vec<(Field, Locality)>> {
        Some(Vec::new())
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let _nt = crate::exec::NodeTimer::new(self.name(), cx.node().raw());
        let v = cx.values();
        let args = cx.args();
        let s = |i: usize| args.get(i).map(|&x| str_of(v, x)).unwrap_or_default();
        let Some(tool) = Tool::of_name(&s(0)) else {
            return Ok(ValueId::UNIT);
        };
        let (output, input) = (s(1), s(2));
        let targs: Vec<String> = args
            .get(3)
            .map(|&l| list_of(v, l).iter().map(|&a| str_of(v, a)).collect())
            .unwrap_or_default();
        let opts = ops(&self.0)
            .tool_opts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let mut served = Served::default();
        // (the input first: as a `\write18`'s node read it, else as the
        // round before wrote it; none written, no run)
        let given = args
            .get(4)
            .map(|&t| (format!("./{input}"), bytes(&str_of(v, t))));
        let Some((_, text)) = (match given {
            Some(x) => Some(x),
            None => look_up(cx, &Need::File(input.clone()))?,
        }) else {
            return Ok(ValueId::UNIT);
        };
        if tool == Tool::Bibtex && !phitex_bibtex::requests_data(&text) {
            return Ok(ValueId::UNIT);
        }
        served.got.insert(
            Need::File(input.clone()),
            Some((format!("./{input}"), text.clone())),
        );
        // (run, then the files it asked for and had not looked up, and
        // again until it asks for none new)
        let (made, said) = loop {
            let (made, said) = match tool {
                Tool::Makeindex => {
                    let a: Vec<Vec<u8>> = targs.iter().map(|a| bytes(a)).collect();
                    let o = phitex_makeindex::run(&a, MAKEINDEX_VERSION, &mut served);
                    let said = makeindex_said(o.ilg.as_ref().map_or(&[][..], |f| &f.contents));
                    (o.ind.map(|f| f.contents), said)
                }
                Tool::Bibtex => {
                    let o = phitex_bibtex::run(input.as_bytes(), &opts.bibtex, &mut served);
                    let said = bibtex_said(o.blg.as_ref().map_or(&[][..], |f| &f.contents));
                    (o.bbl.map(|f| f.contents), said)
                }
                Tool::Biber => {
                    let o = phitex_biber::run(&text, &mut served);
                    let said = (o.errors.iter().map(|e| (true, e.clone())))
                        .chain(o.warnings.iter().map(|w| (false, w.clone())))
                        .collect();
                    (o.bbl.map(String::into_bytes), said)
                }
            };
            let missing = std::mem::take(&mut served.missing);
            if missing.is_empty() {
                break (made, said);
            }
            for n in missing {
                let x = look_up(cx, &n)?;
                served.got.insert(n, x);
            }
        };
        if crate::exec::obs_flag!("PHITEX_AUX_TRACE") {
            eprintln!(
                "tool {} for {output}: read {:?}, made {:?} bytes",
                tool.name(),
                served.got.keys().collect::<Vec<_>>(),
                made.as_ref().map(Vec::len)
            );
        }
        // (`[path, text, 0, said]`: a whole text, read inline where it is
        // opened, `Run::start_input`, which reports what the tool said
        // there: `[error, message]` each, in the tool's order)
        Ok(made.map_or(ValueId::UNIT, |b| {
            let said = said
                .into_iter()
                .map(|(e, m)| {
                    let what = if e { "error" } else { "warning" };
                    v.list(vec![
                        v.int(i64::from(e)),
                        v.str(&format!("{} {what}: {m}", tool.name())),
                    ])
                })
                .collect();
            v.list(vec![
                v.str(&format!("./{output}")),
                v.str(&chars(&b)),
                v.int(0),
                v.list(said),
            ])
        }))
    }
}

/// What BibTeX's `.blg` says a user should see: its warnings
/// (`Warning--…`) and its errors (a message followed by its place,
/// `---line N of file F`, joined into one), each `(error, message)`.
fn bibtex_said(blg: &[u8]) -> Vec<(bool, String)> {
    let t = chars(blg);
    let lines: Vec<&str> = t.lines().collect();
    let mut out = Vec::new();
    for (k, l) in lines.iter().enumerate() {
        if let Some(w) = l.strip_prefix("Warning--") {
            out.push((false, w.trim().to_owned()));
        } else if l.starts_with("---") && k > 0 {
            out.push((true, format!("{}{l}", lines[k - 1].trim_end())));
        }
    }
    out
}

/// What makeindex's `.ilg` says a user should see: each `## Warning`
/// and `!! Input index error` with the line after it (its reason), as
/// `(error, message)`.
fn makeindex_said(ilg: &[u8]) -> Vec<(bool, String)> {
    let t = chars(ilg);
    let lines: Vec<&str> = t.lines().collect();
    let mut out = Vec::new();
    for (k, l) in lines.iter().enumerate() {
        let error = l.starts_with("!!");
        if error || l.starts_with("##") {
            let why = lines
                .get(k + 1)
                .map_or("", |n| n.trim().trim_start_matches("-- "));
            out.push((
                error,
                format!("{} {why}", l.trim_start_matches(['#', '!', ' ']).trim_end()),
            ));
        }
    }
    out
}

/// The arguments of the tool run that makes `output` by default, if one
/// does: the first tool claiming it by name whose input the job writes,
/// with the input's text as the round before wrote it (its loop-back,
/// read here, by the reader: a later publication runs the reader again,
/// and the run is memoised on the text). `None`: no tool makes it, or
/// the job wrote none of its input the round before (the name is read
/// from the file system).
pub(crate) fn run_args(cx: &mut Ctx<'_>, output: &str) -> Result<Option<Vec<ValueId>>, Stop> {
    let v = cx.values();
    for (t, input) in claims(output) {
        let w = text_field(v, &input);
        // (a job that writes the input declares its loop-back; bound
        // otherwise, so the read runs again once it does)
        if !cx.is_loopback(w) {
            let _ = cx.use_field(w)?;
            continue;
        }
        let Some(text) = cx.use_field(w)?.filter(|&x| x != ValueId::UNIT) else {
            return Ok(None);
        };
        let mut a = command_args(v, t, output, &input, std::slice::from_ref(&input));
        a.push(text);
        return Ok(Some(a));
    }
    Ok(None)
}

/// A tool run's arguments (`[tool, output, input, [args...]]`).
pub(crate) fn command_args(
    v: &Values,
    t: Tool,
    output: &str,
    input: &str,
    args: &[String],
) -> Vec<ValueId> {
    vec![
        v.str(t.name()),
        v.str(output),
        v.str(input),
        v.list(args.iter().map(|a| v.str(a)).collect()),
    ]
}

/// A `\write18` naming a tool, where the command is in the round
/// (`[tool, output, input, [args...]]`, placed by the run making it,
/// [`crate::run::Run::tool_command`]): the input as the job wrote it up
/// to here this round, from its stream's accumulator alone
/// ([`crate::run::stream_bucket`]; a file not written before here: as
/// [`ToolRun`] reads it), the tool's memoised run on it, its output the
/// output's field ([`out_field`]).
pub(crate) struct ToolNode(pub Shared);

impl Operation for ToolNode {
    fn name(&self) -> &'static str {
        "toolnode"
    }
    fn defs(&self, args: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        let out = args.get(1).map(|&x| str_of(v, x))?;
        Some(vec![(out_field(v, &out), Locality::Global)])
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let _nt = crate::exec::NodeTimer::new(self.name(), cx.node().raw());
        let v = cx.values();
        let args = cx.args();
        let (Some(&out), Some(&input)) = (args.get(1), args.get(2)) else {
            return Ok(ValueId::UNIT);
        };
        let (out, input) = (str_of(v, out), str_of(v, input));
        let acc = cx.acc_range(crate::run::stream_bucket(v, &input), None)?;
        let text = crate::api::written_texts(v, Some(acc))
            .into_iter()
            .find(|(n, _)| *n == input)
            .map(|(_, t)| t);
        let mut a: Vec<ValueId> = args.iter().take(4).copied().collect();
        a.extend(text.map(|t| v.str(&t)));
        let made = cx.apply(ops(&self.0).tool, a)?;
        if crate::exec::obs_flag!("PHITEX_AUX_TRACE") {
            eprintln!(
                "tool node {} for {out}: made {}",
                cx.node().raw(),
                if made == ValueId::UNIT {
                    "none"
                } else {
                    "output"
                }
            );
        }
        cx.define_with(out_field(v, &out), made, Locality::Global);
        Ok(ValueId::UNIT)
    }
}
