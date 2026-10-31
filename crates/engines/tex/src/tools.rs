//! The programs a job's reads run, as latexmk runs them between
//! pdflatex's runs: BibTeX, biber and makeindex, in process
//! (`phitex-bibtex`, `phitex-biber`, `phitex-makeindex`); and the one
//! `\write18` runs for an EPS figure, `repstopdf` (epstopdf with
//! Ghostscript, `phitex-ps`).
//!
//! A file the job writes is its tools' input: its node
//! (`crate::ops::Files`) asks each tool's crate whether a name is its
//! default output from that file (makeindex: `X.ind` from `X.idx`), runs
//! [`ToolRun`] on the text as the job writes it (`wtext:<name>`) and its
//! support files through the client's lookups for the tool
//! ([`crate::api::ToolSearch`]), and defines what it made
//! ([`made_field`]). Where TeX opens that name
//! ([`crate::run::Run::file`]) it reads that definition at its last
//! (`At::Final`): the reader depends on what the tool makes, not on its
//! input, and the tool runs again only when what it read changed. A name
//! no tool claims, or one whose input the job does not write, is read
//! from the file system.
//!
//! A `\write18` naming a tool is a node of its own ([`ToolNode`]),
//! placed where the command is in this round (the run making it ends
//! there, its rest a resume after the node, `crate::pages`): it reads the
//! input's stream as the job wrote it up to there, this round, and
//! defines the output's field (`toolout:<name>`, [`out_field`]), which a
//! later read of that name reads.

use crate::exec::LockWait as _;
use phitex_defuse::{Ctx, Field, Locality, Operation, Stop, ValueId, Values};

use crate::api::ToolSearch;
use crate::ops::{Shared, ops};
use crate::val::{list_of, str_of};

/// The output a `\write18`'s tool node made of a name ([`ToolNode`]):
/// its [`ToolRun`] value (`UNIT`: it made none), read where the name is
/// opened after the node.
pub(crate) fn out_field(v: &Values, name: &str) -> Field {
    v.field(&out_name(name))
}

/// [`out_field`]'s name.
pub(crate) fn out_name(name: &str) -> String {
    format!("toolout:{name}")
}

/// What a tool makes of a name by default from a file the job writes
/// ([`made_from`]): its [`ToolRun`] value, defined by that file's node
/// (`crate::ops::Files`) and read at its last definition where the name
/// is opened.
pub(crate) fn made_field(v: &Values, name: &str) -> Field {
    v.field(&format!("toolmade:{name}"))
}

/// The names a tool makes by default from `input`, a file the job
/// writes, each with its run's arguments: an output whose first claiming
/// tool with a written input reads `input` (BibTeX's `.bbl` from the
/// `.aux`, biber's from the `.bcf`, makeindex's `.ind` from the `.idx`).
pub(crate) fn made_from(
    cx: &mut Ctx<'_>,
    input: &str,
) -> Result<Vec<(String, Vec<ValueId>)>, Stop> {
    let v = cx.values();
    let mut out = Vec::new();
    for o in made_names(input) {
        if let Some(args) = run_args(cx, &o)?
            && args.get(2).is_some_and(|&i| str_of(v, i) == input)
        {
            out.push((o, args));
        }
    }
    Ok(out)
}

/// The names a tool may make by default from `input`, a file the job
/// writes ([`made_from`] without the job's arguments).
pub(crate) fn made_names(input: &str) -> Vec<String> {
    let base = input.rsplit_once('.').map_or(input, |(b, _)| b);
    [format!("{base}.bbl"), format!("{base}.ind")]
        .into_iter()
        .filter(|o| claims(o).iter().any(|(_, i)| i == input))
        .collect()
}

/// The loop-back of a file the job writes, its whole text as the job
/// writes it (`UNIT`: not written), defined by the files' node for the
/// tools that read it.
pub(crate) fn text_field(v: &Values, name: &str) -> Field {
    v.field(&format!("wtext:{name}"))
}

/// The loop-back of a file the job writes and reads back as its text
/// (`wkeys:` the text mark), its lines as `\\read` takes them
/// ([`crate::expand::split_lines`], a list), published with
/// [`text_field`]: a `\\read` of a line reads that line through a path,
/// so that a reader of a file's first lines depends on those lines, not
/// on every byte (logreq's `\\lrq@writeout` reads three lines of the
/// `.run.xml` it then writes again).
pub(crate) fn lines_field(v: &Values, name: &str) -> Field {
    v.field(&format!("wlines:{name}"))
}

/// The loop-back of a file the job writes, the size of its text in
/// bytes as the round before wrote it (`UNIT`: not written), published
/// with [`text_field`]: what `\\pdffilesize` reads of it, so that a reader
/// of the size depends on the size, not on every byte.
pub(crate) fn size_field(v: &Values, name: &str) -> Field {
    v.field(&format!("wsize:{name}"))
}

/// [`size_field`]'s value of text `t` (`UNIT`: none).
pub(crate) fn size_value(v: &Values, t: ValueId) -> ValueId {
    if t == ValueId::UNIT {
        return t;
    }
    let n = v.with_str(t, |s| s.chars().count()).unwrap_or(0);
    v.int(i64::try_from(n).unwrap_or(i64::MAX))
}

/// The tools, in the order a name more than one claims is given to (the
/// first whose input the job writes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tool {
    Biber,
    Bibtex,
    Makeindex,
    /// epstopdf-base's `repstopdf --outfile=X-eps-converted-to.pdf X.eps`.
    Repstopdf,
    /// kpathsea's `mktextfm` and `mktexpk` where the finder misses a TFM
    /// or PK file (`phitex_mf::mktex::make`, `Run::font_made`): METAFONT
    /// on the font's source, one the job writes first (feynmf's), as
    /// `MFINPUTS` has the current directory first. No command's: not in
    /// [`Tool::ALL`].
    Mktex,
}

impl Tool {
    const ALL: [Tool; 4] = [Tool::Biber, Tool::Bibtex, Tool::Makeindex, Tool::Repstopdf];

    /// The program's name (a `\write18` command's first word).
    pub(crate) fn name(self) -> &'static str {
        match self {
            Tool::Biber => "biber",
            Tool::Bibtex => "bibtex",
            Tool::Makeindex => "makeindex",
            Tool::Repstopdf => "repstopdf",
            Tool::Mktex => "mktex",
        }
    }

    pub(crate) fn of_name(s: &str) -> Option<Tool> {
        Tool::ALL.into_iter().find(|t| t.name() == s)
    }

    /// The tool of a [`ToolRun`]'s first argument: a command's, or
    /// [`Tool::Mktex`].
    fn of_run(s: &str) -> Option<Tool> {
        Tool::of_name(s).or_else(|| (s == Tool::Mktex.name()).then_some(Tool::Mktex))
    }

    /// The input the tool makes `output` from by default (its crate's
    /// predicate on the output's name).
    fn default_input(self, output: &str) -> Option<String> {
        match self {
            Tool::Biber => phitex_biber::default_input_for(output),
            Tool::Bibtex => phitex_bibtex::default_input_for(output),
            Tool::Makeindex => phitex_makeindex::default_input_for(output),
            Tool::Repstopdf | Tool::Mktex => None,
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
            Tool::Repstopdf => epstopdf_files(args),
            Tool::Mktex => None,
        }
    }
}

/// epstopdf's output and input from its arguments: `--outfile=O` (or
/// `-o O`) and the one non-option argument, the output by default the
/// input with `.pdf` for its suffix.
fn epstopdf_files(args: &[String]) -> Option<(String, String)> {
    let mut out = None;
    let mut input = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if let Some(o) = a
            .strip_prefix("--outfile=")
            .or_else(|| a.strip_prefix("-outfile="))
        {
            out = Some(o.to_owned());
        } else if a == "--outfile" || a == "-o" {
            out = it.next().cloned();
        } else if a.starts_with('-') {
        } else {
            input = Some(a.clone());
        }
    }
    let input = input?;
    let out = out.unwrap_or_else(|| {
        let b = input.rsplit_once('.').map_or(input.as_str(), |(b, _)| b);
        format!("{b}.pdf")
    });
    Some((out, input))
}

/// The base 35 fonts Ghostscript's `Resource/Font` gives the converter,
/// by file name, through the tool's lookup ([`crate::ToolSearch::GsFont`]).
struct GsFonts<'a>(std::cell::RefCell<&'a mut Served>);

impl phitex_ps::Resources for GsFonts<'_> {
    fn font(&self, name: &[u8]) -> Option<std::rc::Rc<Vec<u8>>> {
        let n = chars(name);
        self.0
            .borrow_mut()
            .get(Need::Support(ToolSearch::GsFont, n))
            .map(|(_, b)| std::rc::Rc::new(b))
    }
}

/// METAFONT's files for [`Tool::Mktex`]: its inputs as `MFINPUTS` has
/// them, a file the job writes first ([`mf_need`]); web2c's through
/// their search.
#[derive(Clone)]
struct MfFiles(std::rc::Rc<std::cell::RefCell<Served>>);

/// How METAFONT's input `name` is looked up: the current directory
/// first, `MFINPUTS`' first element, where the job's written files are.
fn mf_need(name: &str) -> Need {
    Need::Data(ToolSearch::Mf, name.to_owned())
}

impl phitex_mf::mktex::Files for MfFiles {
    fn find(&mut self, name: &[u8], kind: phitex_mf::mktex::Kind) -> Option<(Vec<u8>, Vec<u8>)> {
        let n = chars(name);
        let n = n.strip_prefix("./").unwrap_or(&n);
        let mut served = self.0.borrow_mut();
        match kind {
            // (kpathsea's `mf` format: the name with its suffix `.mf`
            // first, then as it is, METAFONT asking `plain`, `fgraphs`)
            phitex_mf::mktex::Kind::Mf => {
                let suffixed = (std::path::Path::new(n).extension()
                    != Some(std::ffi::OsStr::new("mf")))
                .then(|| format!("{n}.mf"));
                suffixed
                    .into_iter()
                    .chain(std::iter::once(n.to_owned()))
                    .find_map(|c| served.get(mf_need(&c)))
            }
            phitex_mf::mktex::Kind::Web2c => {
                served.get(Need::Support(ToolSearch::Web2c, n.to_owned()))
            }
        }
        .map(|(p, b)| (p.into_bytes(), b))
    }

    fn pending(&self) -> bool {
        !self.0.borrow().missing.is_empty()
    }
}

/// The runs of METAFONT kept in the process's memory, for a client that
/// gives no cache ([`crate::Options::mktex`]).
fn memory_cache() -> std::sync::Arc<dyn phitex_mf::mktex::Cache> {
    static C: std::sync::OnceLock<std::sync::Arc<dyn phitex_mf::mktex::Cache>> =
        std::sync::OnceLock::new();
    C.get_or_init(|| std::sync::Arc::new(phitex_mf::mktex::MemoryCache::default()))
        .clone()
}

/// [`Tool::Mktex`]'s run: the file kpathsea makes of a miss of `output`
/// (`NAME.tfm`, `NAME.DPIpk@BDPI:MODE`), none where METAFONT made none
/// (as kpathsea's mktex scripts fail: the font not found).
fn mktex_run(output: &str, opts: &ToolOptions, served: &mut Served) -> Option<Vec<u8>> {
    let req = phitex_mf::mktex::Request::of(output)?;
    let (cache, settings) = opts.mktex.as_ref().map_or_else(
        || (memory_cache(), phitex_mf::mktex::Settings::default()),
        |m| (m.cache.clone(), m.settings.clone()),
    );
    let files = MfFiles(std::rc::Rc::new(std::cell::RefCell::new(std::mem::take(
        served,
    ))));
    let made = phitex_mf::mktex::make(&req, &files, &*cache, &settings);
    *served = std::mem::take(&mut *files.0.borrow_mut());
    made.ok().flatten()
}

/// The tools that may make `output` by default, each with its input.
pub(crate) fn claims(output: &str) -> Vec<(Tool, String)> {
    Tool::ALL
        .into_iter()
        .filter_map(|t| t.default_input(output).map(|i| (t, i)))
        .collect()
}

/// The tools' settings the client gives ([`crate::Options::bibtex`]).
#[derive(Clone, Default)]
pub(crate) struct ToolOptions {
    pub bibtex: phitex_bibtex::Options,
    /// Where METAFONT's runs are kept ([`crate::Options::mktex`]).
    pub mktex: Option<crate::api::Mktex>,
    /// The host's runner of other commands ([`crate::Options::shell_hook`]).
    pub shell_hook: Option<std::sync::Arc<dyn crate::shell::ShellHook>>,
    /// What restricted mode allows ([`crate::Options::shell_commands`]).
    pub shell_commands: Vec<String>,
    /// The hook's calls made, by command and the files each read
    /// ([`crate::shell::Memo`]).
    pub shell_memo: std::sync::Arc<std::sync::Mutex<crate::shell::Memo>>,
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
    /// A file of a search whose first element is the current directory:
    /// a file the job writes, by its text the round before, else through
    /// the search. A bibliography database (a class's control entries,
    /// revtex's notes: BibTeX runs after the run that wrote it, and finds
    /// it there, `BIBINPUTS`' first element); METAFONT's input (feynmf's
    /// graphs, `MFINPUTS`).
    Data(ToolSearch, String),
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
        let n = n.strip_prefix("./").unwrap_or(&n).to_owned();
        self.get(Need::Data(ToolSearch::BibData, n)).map(|(_, b)| b)
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
            // (a file the job writes: its text as the job writes it, from
            // its loop-back field, never the file system's copy the job
            // is writing; one it does not write: the job's lookup)
            let w = text_field(v, name);
            if let Some(x) = crate::ops::lb_read(cx, w)? {
                return Ok(
                    (x != ValueId::UNIT).then(|| (format!("./{name}"), bytes(&str_of(v, x))))
                );
            }
            Ok(file(cx.env(v.field(&format!("file:{name}")))?))
        }
        Need::Support(s, name) => Ok(file(cx.env(support_field(v, *s, name))?)),
        Need::Data(s, name) => {
            if let Some(x) = crate::ops::lb_read(cx, text_field(v, name))?
                && x != ValueId::UNIT
            {
                return Ok(Some((format!("./{name}"), bytes(&str_of(v, x)))));
            }
            Ok(file(cx.env(support_field(v, *s, name))?))
        }
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
    #[allow(clippy::too_many_lines)]
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let _nt = crate::exec::NodeTimer::new(self.name(), cx.node().raw());
        let v = cx.values();
        let args = cx.args().to_vec();
        let s = |i: usize| args.get(i).map(|&x| str_of(v, x)).unwrap_or_default();
        let Some(tool) = Tool::of_run(&s(0)) else {
            return Ok(ValueId::UNIT);
        };
        let (output, input) = (s(1), s(2));
        let targs: Vec<String> = args
            .get(3)
            .map(|&l| list_of(v, l).iter().map(|&a| str_of(v, a)).collect())
            .unwrap_or_default();
        let opts = ops(&self.0).tool_opts.locked().clone();
        let mut served = Served::default();
        // (the input first: as a `\write18`'s node read it, else as the
        // round before wrote it; none written, no run)
        let given = args
            .get(4)
            .map(|&t| (format!("./{input}"), bytes(&str_of(v, t))));
        // (METAFONT's source as `MFINPUTS` finds it: the run goes on
        // without one, mktexmf writing one where it can)
        if tool == Tool::Mktex {
            let need = mf_need(&input);
            let found = look_up(cx, &need)?;
            served.got.insert(need, found);
        }
        let Some((_, text)) = (match given {
            Some(x) => Some(x),
            None if tool == Tool::Mktex => Some((String::new(), Vec::new())),
            None => look_up(cx, &Need::File(input.clone()))?,
        }) else {
            return Ok(ValueId::UNIT);
        };
        if tool == Tool::Bibtex && !phitex_bibtex::requests_data(&text) {
            return Ok(ValueId::UNIT);
        }
        let epoch = match cx.env(v.field("env:SOURCE_DATE_EPOCH"))? {
            Some(x) => crate::val::int_of(v, x),
            None => crate::pdfprim::source_date_epoch(),
        };
        if tool != Tool::Mktex {
            served.got.insert(
                Need::File(input.clone()),
                Some((format!("./{input}"), text.clone())),
            );
        }
        // (run, then the files it asked for and had not looked up, and
        // again until it asks for none new)
        let mut fail: Option<String> = None;
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
                Tool::Repstopdf => {
                    // (epstopdf.pl's rewrite of the header, then gs reading
                    // it from stdin; gs's error, if any, ends its run, the
                    // page so far written)
                    let feed = phitex_ps::epstopdf_feed(&text);
                    let o = phitex_ps::convert(
                        &feed,
                        phitex_ps::Settings::epstopdf(epoch),
                        &GsFonts(std::cell::RefCell::new(&mut served)),
                    );
                    // (what the converter does not write as gs does: no
                    // file, as a failed repstopdf leaves, and an error
                    // where its output is looked for, `Run::tool_output`)
                    fail = o.error.map(|e| format!("repstopdf: {input}: {e}"));
                    (Some(o.pdf), Vec::new())
                }
                Tool::Mktex => (mktex_run(&output, &opts, &mut served), Vec::new()),
            };
            let missing = std::mem::take(&mut served.missing);
            if missing.is_empty() {
                break (made, said);
            }
            fail = None;
            for n in missing {
                let x = look_up(cx, &n)?;
                served.got.insert(n, x);
            }
        };
        if let Some(m) = fail {
            return Ok(v.list(vec![v.str(&m)]));
        }
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
/// does: the first tool claiming it by name whose input the job writes
/// (its size defined, `crate::ops::Files`). `None`: no tool makes it.
pub(crate) fn run_args(cx: &mut Ctx<'_>, output: &str) -> Result<Option<Vec<ValueId>>, Stop> {
    let v = cx.values();
    for (t, input) in claims(output) {
        // (a job that writes the input defines its size, the files'
        // node; read either way, so this runs again once it does)
        if crate::ops::lb_read(cx, size_field(v, &input))?.is_none() {
            continue;
        }
        return Ok(Some(command_args(
            v,
            t,
            output,
            &input,
            std::slice::from_ref(&input),
        )));
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
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let _nt = crate::exec::NodeTimer::new(self.name(), cx.node().raw());
        let v = cx.values();
        let args = cx.args().to_vec();
        let (Some(&out), Some(&input)) = (args.get(1), args.get(2)) else {
            return Ok(ValueId::UNIT);
        };
        let (out, input) = (str_of(v, out), str_of(v, input));
        let acc = cx.fold(crate::run::stream_bucket(v, &input), None, None)?;
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
