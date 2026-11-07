//! latexmk's configuration files (`.latexmkrc`, `latexmkrc`) read as
//! data: never run. latexmk evaluates them as Perl; PhiTeX reads the
//! declarative subset people write (docs/latexmkrc.md) and says, for every
//! line it does not apply, the file, the line and why.
//!
//! - [`order`]: the files latexmk reads, in its order (the system's, the
//!   user's, the working directory's; at most one of each, the first that
//!   exists), as latexmk 4.8x does (its `read_first_rc_file_in_list`).
//! - [`read`]: those files evaluated in order, later settings over
//!   earlier ones, into an [`Rc`]: the search paths they set
//!   (`$ENV{TEXINPUTS}`, `BIBINPUTS`, `BSTINPUTS`, latexmk's
//!   `ensure_path`), `@default_files`, and the settings only
//!   `--exact-latexmk` applies ([`Rc::exact`]).
//!
//! Nothing here touches a disk: a host gives the files' texts
//! ([`RcFiles`]), the environment and the working directory, so the CLI,
//! the editors and the wasm core read the same files the same way.
//!
//! Perl's own meaning is kept where the subset reaches it: `__FILE__` in a
//! file latexmk reads itself is `(eval N)` (latexmk `eval`s the file's
//! text), so `dirname(abs_path(__FILE__))` there is the working directory;
//! in a file `do`ne from one it is the name `do` was given. A relative
//! `do` or `require` path is the working directory's (Perl's `./` and
//! `../` rule; `.` is not in a current Perl's `@INC`).

use std::collections::BTreeMap;
use std::fmt;

/// The search paths an rc file sets that PhiTeX applies by default (they
/// only help the build find files).
pub const SEARCH_VARS: [&str; 3] = ["TEXINPUTS", "BIBINPUTS", "BSTINPUTS"];

/// A place in an rc file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    /// The file as it was named (latexmk's spelling: `~/.latexmkrc`'s
    /// path, `.latexmkrc`).
    pub file: String,
    /// Its line, from 1.
    pub line: usize,
}

impl fmt::Display for Place {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.file, self.line)
    }
}

/// A line not applied, and why: said once, in plain words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Note {
    pub at: Place,
    pub why: String,
}

impl fmt::Display for Note {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.at, self.why)
    }
}

/// A setting only `--exact-latexmk` applies (by default PhiTeX keeps its
/// own rule for it, and says so once).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Exact {
    /// `$pdf_mode`.
    PdfMode(String),
    /// `$out_dir`.
    OutDir(String),
    /// `$aux_dir`.
    AuxDir(String),
    /// `$bibtex_use`.
    BibtexUse(String),
    /// `$max_repeat`.
    MaxRepeat(String),
    /// `$pdflatex`, `$xelatex`, `$lualatex`, `$latex` (the variable's
    /// name), or `set_tex_cmds`'s options (`*`).
    Command(String, String),
}

impl Exact {
    /// The setting as latexmk names it.
    pub fn name(&self) -> String {
        match self {
            Exact::PdfMode(_) => "$pdf_mode".into(),
            Exact::OutDir(_) => "$out_dir".into(),
            Exact::AuxDir(_) => "$aux_dir".into(),
            Exact::BibtexUse(_) => "$bibtex_use".into(),
            Exact::MaxRepeat(_) => "$max_repeat".into(),
            Exact::Command(v, _) if v == "*" => "set_tex_cmds".into(),
            Exact::Command(v, _) => format!("${v}"),
        }
    }

    /// Why PhiTeX does not apply it by default.
    fn kept(&self) -> &'static str {
        match self {
            Exact::PdfMode(_) => "phitex chooses the engine itself",
            Exact::OutDir(_) | Exact::AuxDir(_) => "phitex keeps its own output folder",
            Exact::BibtexUse(_) => "phitex runs BibTeX and biber when the document needs them",
            Exact::MaxRepeat(_) => "phitex runs LaTeX until its files settle",
            Exact::Command(..) => "phitex keeps its own engine options",
        }
    }
}

/// What the rc files say.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Rc {
    /// The files read, in order (an included one after the file that
    /// included it).
    pub read: Vec<String>,
    /// The search paths they set ([`SEARCH_VARS`]): each one's final value
    /// (kpathsea's syntax; relative elements made absolute against the
    /// working directory, as the engine latexmk runs reads them there).
    pub vars: Vec<(String, String)>,
    /// `@default_files`, where it was last set: the main files when the
    /// command line names none.
    pub default_files: Option<(Vec<String>, Place)>,
    /// The settings only `--exact-latexmk` applies, each where it was last
    /// set, in the order first set.
    pub exact: Vec<(Exact, Place)>,
    /// The lines not understood or not applied.
    pub notes: Vec<Note>,
}

/// The rc files' texts, as a host has them.
pub trait RcFiles {
    /// The text of the file at `path` (absolute, `/`-separated); none: no
    /// such file (or a directory, or unreadable).
    fn read(&self, path: &str) -> Option<String>;
    /// `abs_path`'s answer for absolute `path` (symbolic links resolved);
    /// by default `path` with `.` and `..` taken out.
    fn canonical(&self, path: &str) -> String {
        normalize(path)
    }
}

/// Where latexmk looks for its rc files, and what of the environment
/// decides it.
#[derive(Clone, Debug, Default)]
pub struct Where {
    /// The working directory (absolute): latexmk's, where it is run.
    pub cwd: String,
    /// `$HOME` (none or empty: no user file).
    pub home: Option<String>,
    /// `$XDG_CONFIG_HOME`.
    pub xdg_config_home: Option<String>,
    /// Whether `$HOME/.config` is a directory (latexmk's XDG default).
    pub home_config_is_dir: bool,
    /// `$LATEXMKRCSYS`.
    pub latexmkrcsys: Option<String>,
    /// Read the system's and the user's files (the CLI and the editors);
    /// the browser hosts read only the project's, as Overleaf does.
    pub system_and_user: bool,
}

/// latexmk's rc files in its order: each level a list of candidates, the
/// first that exists read (latexmk 4.8x on Unix).
pub fn order(w: &Where) -> Vec<Vec<String>> {
    let mut levels = Vec::new();
    if w.system_and_user {
        match &w.latexmkrcsys {
            Some(f) => levels.push(vec![f.clone()]),
            None => {
                let mut sys = Vec::new();
                for name in ["LatexMk", "latexmkrc"] {
                    for d in [
                        "/etc",
                        "/opt/local/share/latexmk",
                        "/usr/local/share/latexmk",
                        "/usr/local/lib/latexmk",
                    ] {
                        sys.push(format!("{d}/{name}"));
                    }
                }
                levels.push(sys);
            }
        }
        if let Some(home) = w.home.as_deref().filter(|h| !h.is_empty()) {
            let xdg = match &w.xdg_config_home {
                Some(x) => Some(x.clone()),
                None if w.home_config_is_dir => Some(format!("{home}/.config")),
                None => None,
            };
            let mut user = Vec::new();
            if let Some(x) = xdg.filter(|x| !x.is_empty()) {
                user.push(format!("{x}/latexmk/latexmkrc"));
            }
            user.push(format!("{home}/.latexmkrc"));
            levels.push(user);
        }
    }
    levels.push(vec![join(&w.cwd, ".latexmkrc"), join(&w.cwd, "latexmkrc")]);
    levels
}

/// The rc files of `levels` ([`order`]) read in order, the environment
/// `env` (the process's) the start of `$ENV`.
pub fn read(
    levels: &[Vec<String>],
    cwd: &str,
    env: &BTreeMap<String, String>,
    files: &dyn RcFiles,
) -> Rc {
    let mut r = Reader {
        cwd: cwd.to_owned(),
        env: env
            .iter()
            .map(|(k, v)| (k.clone(), Some(v.clone())))
            .collect(),
        start: env.clone(),
        files,
        vars: BTreeMap::new(),
        rc: Rc::default(),
        including: Vec::new(),
        auto_rc_use: true,
    };
    for level in levels {
        if !r.auto_rc_use {
            break;
        }
        for f in level {
            if let Some(text) = files.read(f) {
                r.file(f, &display_name(f, cwd), &text, true);
                break;
            }
        }
    }
    r.finish()
}

/// How a file is named in what is said: relative to `cwd` when inside
/// it, else as it is.
fn display_name(f: &str, cwd: &str) -> String {
    match f.strip_prefix(cwd).and_then(|r| r.strip_prefix('/')) {
        Some(rel) if !rel.is_empty() => rel.to_owned(),
        _ => f.to_owned(),
    }
}

impl Rc {
    /// The value an rc file gave search variable `var`, if any.
    pub fn var(&self, var: &str) -> Option<&str> {
        self.vars
            .iter()
            .find(|(k, _)| k == var)
            .map(|(_, v)| v.as_str())
    }

    /// The lines said by default for the settings only `--exact-latexmk`
    /// applies: one a setting (`how` names the way to turn it on, where
    /// the host has one).
    pub fn not_applied(&self, how: Option<&str>) -> Vec<String> {
        self.exact
            .iter()
            .map(|(e, at)| match how {
                Some(how) => format!(
                    "{at}: {} not applied ({}; {how} applies it)",
                    e.name(),
                    e.kept()
                ),
                None => format!("{at}: {} not applied ({})", e.name(), e.kept()),
            })
            .collect()
    }

    /// What `--exact-latexmk` applies, and the lines it still cannot
    /// (each with its place).
    pub fn exact(&self) -> Applied {
        let mut a = Applied::default();
        let get = |n: &str| {
            self.exact.iter().find(|(e, _)| match e {
                Exact::PdfMode(_) => n == "$pdf_mode",
                _ => e.name() == n,
            })
        };
        let mode = get("$pdf_mode").map(|(e, at)| match e {
            Exact::PdfMode(v) => (v.trim().to_owned(), at.clone()),
            _ => unreachable!(),
        });
        // (the command the mode runs: latexmk's $pdflatex for 1, $xelatex
        // for 5, $lualatex for 4)
        let var = match mode.as_ref().map(|(v, _)| v.as_str()) {
            Some("5") => "xelatex",
            Some("4") => "lualatex",
            _ => "pdflatex",
        };
        if let Some((v, at)) = &mode {
            match v.as_str() {
                "1" => a.engine = Some("pdflatex".into()),
                "5" => a.engine = Some("xelatex".into()),
                "4" => a.notes.push(Note {
                    at: at.clone(),
                    why: "$pdf_mode = 4 (lualatex) not applied: LuaLaTeX is not supported in \
                          PhiTeX 1.0"
                        .into(),
                }),
                v => a.notes.push(Note {
                    at: at.clone(),
                    why: format!(
                        "$pdf_mode = {v} not applied: phitex makes the PDF with pdflatex \
                         ($pdf_mode = 1) or xelatex (5), not by DVI or PostScript"
                    ),
                }),
            }
        }
        let mut commands = Vec::new();
        if let Some((Exact::Command(_, o), at)) = get("set_tex_cmds") {
            commands.push((format!("{var} {o}"), at.clone(), true));
        }
        if let Some((Exact::Command(_, c), at)) = get(&format!("${var}")) {
            commands.push((c.clone(), at.clone(), false));
        }
        for (c, at, from_options) in commands {
            let cmd = command(&c);
            if !from_options {
                match cmd.program.as_str() {
                    "pdflatex" | "xelatex" => {
                        if cmd.program != var {
                            a.engine = Some(cmd.program.clone());
                        }
                    }
                    "lualatex" => a.notes.push(Note {
                        at: at.clone(),
                        why: format!(
                            "${var} runs lualatex: not applied, LuaLaTeX is not supported in \
                             PhiTeX 1.0"
                        ),
                    }),
                    "internal" => a.notes.push(Note {
                        at: at.clone(),
                        why: format!("${var} names a Perl subroutine: not applied (no Perl runs)"),
                    }),
                    p => a.notes.push(Note {
                        at: at.clone(),
                        why: format!("${var} runs `{p}`: not applied, phitex runs its own engine"),
                    }),
                }
            }
            if cmd.shell_escape.is_some() {
                a.shell_escape = cmd.shell_escape.map(|s| (s, at.clone()));
            }
            if cmd.synctex.is_some() {
                a.synctex = cmd.synctex;
            }
            if !cmd.others.is_empty() {
                a.notes.push(Note {
                    at: at.clone(),
                    why: format!(
                        "{} not applied: phitex has no such engine option",
                        cmd.others.join(" ")
                    ),
                });
            }
        }
        for (e, at) in &self.exact {
            match e {
                Exact::OutDir(d) => a.out_dir = Some((d.clone(), at.clone())),
                Exact::AuxDir(d) => a.aux_dir = Some((d.clone(), at.clone())),
                Exact::BibtexUse(v) => match v.trim() {
                    // (as phitex runs it: when the document needs it)
                    "1" | "1.5" | "2" => {}
                    v => a.notes.push(Note {
                        at: at.clone(),
                        why: format!(
                            "$bibtex_use = {v} not applied: phitex runs BibTeX when the \
                             document needs it and has no switch to keep it from running"
                        ),
                    }),
                },
                // (phitex's limit is latexmk's default)
                Exact::MaxRepeat(v) if v.trim() == "5" => {}
                Exact::MaxRepeat(v) => a.notes.push(Note {
                    at: at.clone(),
                    why: format!(
                        "$max_repeat = {} not applied: phitex runs LaTeX at most 5 times, \
                         latexmk's default",
                        v.trim()
                    ),
                }),
                _ => {}
            }
        }
        if let (Some((aux, at)), out) = (&a.aux_dir, &a.out_dir)
            && out.as_ref().is_none_or(|(o, _)| o != aux)
        {
            a.notes.push(Note {
                at: at.clone(),
                why: "$aux_dir not applied: phitex writes the auxiliary files beside the PDF \
                      ($out_dir)"
                    .into(),
            });
        }
        a
    }
}

/// What `--exact-latexmk` applies of an [`Rc`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Applied {
    /// The engine (`pdflatex`, `xelatex`), by `$pdf_mode` and its command.
    pub engine: Option<String>,
    /// `$out_dir` (as written: relative to the working directory).
    pub out_dir: Option<(String, Place)>,
    /// `$aux_dir`.
    pub aux_dir: Option<(String, Place)>,
    /// `-shell-escape` (or `-no-shell-escape`) in the command, and where.
    pub shell_escape: Option<(bool, Place)>,
    /// `-synctex=N` in the command: on unless 0.
    pub synctex: Option<bool>,
    /// What it cannot apply.
    pub notes: Vec<Note>,
}

/// A TeX command line as latexmk's `$pdflatex` holds it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Command {
    pub program: String,
    pub shell_escape: Option<bool>,
    pub synctex: Option<bool>,
    /// The options PhiTeX does not apply (nor accepts as its own way).
    pub others: Vec<String>,
}

/// Command `c` (`pdflatex -shell-escape %O %S`) read.
pub fn command(c: &str) -> Command {
    let mut words = split_words(c).into_iter();
    let mut cmd = Command {
        program: words.next().unwrap_or_default(),
        ..Command::default()
    };
    // (a path to the program: its name)
    if let Some(n) = cmd.program.rsplit('/').next() {
        cmd.program = n.to_owned();
    }
    for w in words {
        // (latexmk's placeholders: the options it adds, the source, the
        // destination, the base name, ...)
        if w.starts_with('%') && w.len() == 2 {
            continue;
        }
        let o = w.trim_start_matches('-');
        if !w.starts_with('-') {
            cmd.others.push(w.clone());
            continue;
        }
        let (name, value) = o.split_once('=').unwrap_or((o, ""));
        match name {
            "shell-escape" => cmd.shell_escape = Some(true),
            "no-shell-escape" | "shell-restricted" => cmd.shell_escape = Some(false),
            "synctex" => cmd.synctex = Some(value.trim() != "0"),
            // (accepted, as phitex works: errors shown by phitex, file:line
            // errors in its own report, a build that stops at the first
            // error's place, PDF output)
            "interaction" | "file-line-error" | "no-file-line-error" | "halt-on-error"
            | "recorder" | "pdf" | "output-format" => {}
            _ => cmd.others.push(w.clone()),
        }
    }
    cmd
}

fn split_words(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut any = false;
    for c in s.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => cur.push(c),
            None if c == '"' || c == '\'' => {
                quote = Some(c);
                any = true;
            }
            None if c.is_whitespace() => {
                if any || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            None => cur.push(c),
        }
    }
    if any || !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// `p` absolute against `cwd` (unless it is), `.` and `..` taken out.
pub fn join(cwd: &str, p: &str) -> String {
    if p.starts_with('/') {
        normalize(p)
    } else {
        normalize(&format!("{cwd}/{p}"))
    }
}

/// Absolute path `p` with `.`, `..` and repeated `/` taken out (a
/// trailing `//`, kpathsea's subdirectories mark, is not this function's).
pub fn normalize(p: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for s in p.split('/') {
        match s {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    format!("/{}", parts.join("/"))
}

/// Perl's `File::Basename::dirname`.
fn dirname(p: &str) -> String {
    let t = p.trim_end_matches('/');
    if t.is_empty() {
        return if p.starts_with('/') {
            "/".into()
        } else {
            ".".into()
        };
    }
    match t.rfind('/') {
        Some(i) => {
            let d = t[..i].trim_end_matches('/');
            if d.is_empty() { "/".into() } else { d.into() }
        }
        None => ".".into(),
    }
}

/// Perl's `File::Basename::basename`.
fn basename(p: &str) -> String {
    let t = p.trim_end_matches('/');
    t.rsplit('/').next().unwrap_or("").to_owned()
}

/// A search path's relative elements made absolute against `cwd`
/// (kpathsea's elements: `!!`, a trailing `//`, an empty element, `$VAR`,
/// `~` and braces kept as they are).
pub fn absolute_elements(value: &str, cwd: &str) -> String {
    value
        .split(':')
        .map(|e| {
            let (bang, rest) = match e.strip_prefix("!!") {
                Some(r) => ("!!", r),
                None => ("", e),
            };
            if rest.is_empty()
                || rest.starts_with('/')
                || rest.starts_with('$')
                || rest.starts_with('~')
                || rest.contains('{')
            {
                return e.to_owned();
            }
            let tail = rest.len() - rest.trim_end_matches('/').len();
            let dir = rest.trim_end_matches('/');
            let abs = join(cwd, if dir.is_empty() { "." } else { dir });
            let slashes = if tail >= 2 { "//" } else { "" };
            format!("{bang}{abs}{slashes}")
        })
        .collect::<Vec<_>>()
        .join(":")
}

/// An element of a search path, for a host without kpathsea (Shelf's,
/// the browser's): a folder, its subfolders too after a trailing `//`;
/// or the host's own search (an empty element: kpathsea's default path).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Element {
    Dir { path: String, sub: bool },
    Default,
}

/// The elements of search path `value` (`:` or `;` between them, `!!`
/// dropped: there is no `ls-R` to keep to). A path with no empty element
/// has no default search, as in kpathsea.
pub fn elements(value: &str) -> Vec<Element> {
    value
        .split([':', ';'])
        .map(|e| {
            let e = e.strip_prefix("!!").unwrap_or(e);
            if e.is_empty() {
                return Element::Default;
            }
            let dir = e.trim_end_matches('/');
            Element::Dir {
                path: if dir.is_empty() {
                    "/".into()
                } else {
                    dir.into()
                },
                sub: e.ends_with("//"),
            }
        })
        .collect()
}

// ---------------------------------------------------------------------
// The reader: statements of Perl's declarative subset.

/// latexmk's variables PhiTeX's build has no part in, but which change
/// nothing in the PDF (viewers, cleaning, the run loop's timing): said
/// once, as any line not applied.
const VIEW_AND_CLEAN: [&str; 24] = [
    "pdf_previewer",
    "ps_previewer",
    "ps_previewer_landscape",
    "dvi_previewer",
    "dvi_previewer_landscape",
    "pdf_update_method",
    "pdf_update_command",
    "pdf_update_signal",
    "dvi_update_method",
    "preview_mode",
    "preview_continuous_mode",
    "new_viewer_always",
    "view",
    "sleep_time",
    "clean_ext",
    "clean_full_ext",
    "cleanup_includes_generated",
    "cleanup_includes_cusdep_generated",
    "silent",
    "quote_filenames",
    "show_time",
    "recorder",
    "bibtex_fudge",
    "compiling_cmd",
];

struct Reader<'a> {
    cwd: String,
    /// `%ENV` as the files left it (none: deleted or undef).
    env: BTreeMap<String, Option<String>>,
    /// The environment the reading started from.
    start: BTreeMap<String, String>,
    files: &'a dyn RcFiles,
    /// `my`/`our` variables and other scalars a file set.
    vars: BTreeMap<String, Option<String>>,
    rc: Rc,
    /// The files being read (an include of one of them is a cycle).
    including: Vec<String>,
    auto_rc_use: bool,
}

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Scalar(String),
    Array(String),
    Str(String),
    Num(String),
    Word(String),
    Op(&'static str),
}

/// Why a statement was not applied.
type Why = String;

impl Reader<'_> {
    fn finish(mut self) -> Rc {
        for v in SEARCH_VARS {
            let now = self.env.get(v).cloned().flatten();
            if now.as_deref() != self.start.get(v).map(String::as_str)
                && let Some(val) = now
            {
                self.rc
                    .vars
                    .push((v.to_owned(), absolute_elements(&val, &self.cwd)));
            }
        }
        self.rc
    }

    /// File `path` (shown as `shown`), text `text`: each statement.
    /// `direct`: latexmk read it itself (its `__FILE__` is `(eval N)`).
    fn file(&mut self, path: &str, shown: &str, text: &str, direct: bool) {
        self.rc.read.push(shown.to_owned());
        self.including.push(path.to_owned());
        let file_name = if direct {
            "(eval 1)".to_owned()
        } else {
            shown.to_owned()
        };
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        for (line, stmt) in statements(text) {
            let at = Place {
                file: shown.to_owned(),
                line,
            };
            if let Err(why) = self.statement(&stmt, &file_name, &at) {
                self.rc.notes.push(Note { at, why });
            }
        }
        self.including.pop();
    }

    fn statement(&mut self, stmt: &Stmt, file: &str, at: &Place) -> Result<(), Why> {
        let s = match stmt {
            Stmt::Block(head) => {
                return Err(format!(
                    "`{}` not applied: PhiTeX reads latexmk's settings as data and runs no \
                     Perl code (subroutines, conditions, loops)",
                    short(head)
                ));
            }
            Stmt::Unclosed => {
                return Err("not applied: a string or bracket here is not closed".into());
            }
            Stmt::Plain(s) => s,
        };
        let toks = lex(s).map_err(|e| format!("`{}` not understood: {e}", short(s)))?;
        if toks.is_empty() {
            return Ok(());
        }
        let not_understood = || {
            format!(
                "`{}` not understood: PhiTeX reads latexmk's settings as data and runs no Perl \
                 (docs/latexmkrc.md lists what it reads)",
                short(s)
            )
        };
        let mut p = Parser {
            t: &toks,
            i: 0,
            r: self,
            file,
        };
        match &toks[0] {
            Tok::Word(w) if w == "use" || w == "no" => {
                let m = match toks.get(1) {
                    Some(Tok::Word(m)) => m.clone(),
                    _ => return Err(not_understood()),
                };
                if [
                    "File::Basename",
                    "Cwd",
                    "strict",
                    "warnings",
                    "utf8",
                    "File::Spec",
                    "File::Spec::Functions",
                ]
                .contains(&m.as_str())
                {
                    return Ok(());
                }
                Err(format!(
                    "`{}` not applied: PhiTeX loads no Perl module",
                    short(s)
                ))
            }
            Tok::Word(w) if w == "my" || w == "our" || w == "local" => {
                p.i = 1;
                let name = match p.next() {
                    Some(Tok::Scalar(n)) => n,
                    Some(Tok::Array(_)) => {
                        return Err(format!(
                            "`{}` not applied: PhiTeX reads no list of its own",
                            short(s)
                        ));
                    }
                    _ => return Err(not_understood()),
                };
                if p.done() {
                    p.r.vars.insert(name, None);
                    return Ok(());
                }
                if !p.eat("=") {
                    return Err(not_understood());
                }
                let v = p.expr()?;
                p.end()?;
                self.vars.insert(name, v);
                Ok(())
            }
            Tok::Word(w) if w == "do" || w == "require" => {
                p.i = 1;
                let target = p.expr()?.ok_or_else(|| {
                    format!("`{}` not applied: the file it names is not set", short(s))
                })?;
                p.end()?;
                self.include(&target, at)
            }
            Tok::Word(w) if w == "ensure_path" => {
                p.i = 1;
                let args = p.call_args()?;
                p.end()?;
                let mut it = args.into_iter();
                let var = it.next().flatten().ok_or_else(not_understood)?;
                for v in it {
                    let Some(v) = v else { continue };
                    self.ensure_path(&var, &v);
                }
                if !SEARCH_VARS.contains(&var.as_str()) {
                    return Err(format!(
                        "ensure_path('{var}', ...) not applied: phitex applies TEXINPUTS, \
                         BIBINPUTS and BSTINPUTS"
                    ));
                }
                Ok(())
            }
            Tok::Word(w) if w == "set_tex_cmds" => {
                p.i = 1;
                let args = p.call_args()?;
                p.end()?;
                let o = args.into_iter().next().flatten().unwrap_or_default();
                self.set_exact(Exact::Command("*".into(), o), at);
                Ok(())
            }
            Tok::Word(w) if w == "print" || w == "warn" || w == "1" => Ok(()),
            Tok::Num(_) => Ok(()),
            Tok::Word(w) if w == "push" || w == "unshift" => {
                p.i = 1;
                let parens = p.eat("(");
                let name = match p.next() {
                    Some(Tok::Array(n)) => n,
                    _ => return Err(not_understood()),
                };
                if !p.eat(",") {
                    return Err(not_understood());
                }
                let list = p.list()?;
                if parens && !p.eat(")") {
                    return Err(not_understood());
                }
                p.end()?;
                if name != "default_files" {
                    return Err(format!("`@{name}` not applied: phitex has no such setting"));
                }
                let mut files = self
                    .rc
                    .default_files
                    .take()
                    .map(|(f, _)| f)
                    .unwrap_or_default();
                if w == "push" {
                    files.extend(list);
                } else {
                    files.splice(0..0, list);
                }
                self.rc.default_files = Some((files, at.clone()));
                Ok(())
            }
            Tok::Array(name) => {
                p.i = 1;
                if !p.eat("=") {
                    return Err(not_understood());
                }
                let list = p.list()?;
                p.end()?;
                if name != "default_files" {
                    return Err(format!("`@{name}` not applied: phitex has no such setting"));
                }
                self.rc.default_files = Some((list, at.clone()));
                Ok(())
            }
            Tok::Scalar(name) if name == "ENV" => {
                p.i = 1;
                let var = p.hash_key()?;
                let op = if p.eat("=") {
                    "="
                } else if p.eat(".=") {
                    ".="
                } else {
                    return Err(not_understood());
                };
                let v = p.expr()?;
                p.end()?;
                let v = if op == ".=" {
                    let old = self.env.get(&var).cloned().flatten().unwrap_or_default();
                    Some(old + &v.unwrap_or_default())
                } else {
                    v
                };
                self.env.insert(var.clone(), v);
                if SEARCH_VARS.contains(&var.as_str()) {
                    Ok(())
                } else {
                    Err(format!(
                        "$ENV{{{var}}} not applied: phitex applies TEXINPUTS, BIBINPUTS and \
                         BSTINPUTS from a .latexmkrc"
                    ))
                }
            }
            Tok::Scalar(name) => {
                p.i = 1;
                let op = if p.eat("=") {
                    "="
                } else if p.eat(".=") {
                    ".="
                } else {
                    return Err(not_understood());
                };
                let v = p.expr()?;
                p.end()?;
                let name = name.clone();
                let v = if op == ".=" {
                    let old = self.vars.get(&name).cloned().flatten().unwrap_or_default();
                    Some(old + &v.unwrap_or_default())
                } else {
                    v
                };
                self.scalar(&name, v, at)
            }
            _ => Err(not_understood()),
        }
    }

    /// `$name = v` for a variable not declared with `my`.
    fn scalar(&mut self, name: &str, v: Option<String>, at: &Place) -> Result<(), Why> {
        let val = v.clone().unwrap_or_default();
        let exact = match name {
            "pdf_mode" => Some(Exact::PdfMode(val.clone())),
            "out_dir" | "outdir" => Some(Exact::OutDir(val.clone())),
            "aux_dir" | "auxdir" => Some(Exact::AuxDir(val.clone())),
            "bibtex_use" => Some(Exact::BibtexUse(val.clone())),
            "max_repeat" => Some(Exact::MaxRepeat(val.clone())),
            "pdflatex" | "xelatex" | "lualatex" | "latex" => {
                Some(Exact::Command(name.to_owned(), val.clone()))
            }
            _ => None,
        };
        if let Some(e) = exact {
            self.vars.insert(name.to_owned(), v);
            self.set_exact(e, at);
            return Ok(());
        }
        if name == "auto_rc_use" {
            self.auto_rc_use = val.trim() != "0" && !val.trim().is_empty();
            return Ok(());
        }
        let was_mine = self.vars.contains_key(name);
        self.vars.insert(name.to_owned(), v);
        if was_mine {
            return Ok(());
        }
        if VIEW_AND_CLEAN.contains(&name) {
            return Err(format!(
                "${name} not applied: it is latexmk's (viewing, cleaning, its run loop), with \
                 no part in phitex's build"
            ));
        }
        if matches!(name, "dvi_mode" | "postscript_mode" | "xdv_mode") {
            return Err(format!(
                "${name} not applied: phitex makes the PDF directly"
            ));
        }
        Err(format!("${name} not applied: phitex has no such setting"))
    }

    /// Setting `e` (a later one of the same name over an earlier).
    fn set_exact(&mut self, e: Exact, at: &Place) {
        let name = e.name();
        match self.rc.exact.iter_mut().find(|(x, _)| x.name() == name) {
            Some(slot) => *slot = (e, at.clone()),
            None => self.rc.exact.push((e, at.clone())),
        }
    }

    /// latexmk's `ensure_path(var, value)`: `value` before `$ENV{var}`
    /// unless already one of its elements; `value:` when unset.
    fn ensure_path(&mut self, var: &str, value: &str) {
        if value.is_empty() {
            return;
        }
        let cur = self.env.get(var).cloned().flatten();
        let new = match cur {
            Some(c) => {
                if c.split(':').any(|e| !e.is_empty() && e == value) {
                    return;
                }
                format!("{value}:{c}")
            }
            None => format!("{value}:"),
        };
        self.env.insert(var.to_owned(), Some(new));
    }

    /// `do FILE` / `require FILE`: the file read with `__FILE__` its
    /// name; a relative name is the working directory's.
    fn include(&mut self, target: &str, at: &Place) -> Result<(), Why> {
        let rel = target.starts_with("./") || target.starts_with("../");
        if !target.starts_with('/') && !rel {
            return Err(format!(
                "`do \"{target}\"` not applied: Perl looks for such a name in @INC, not the \
                 working directory (write \"./{target}\")"
            ));
        }
        let path = join(&self.cwd, target);
        if self.including.contains(&path) {
            return Err(format!(
                "`do \"{target}\"` not applied: that file is already being read (a cycle)"
            ));
        }
        let Some(text) = self.files.read(&path) else {
            return Err(format!("`do \"{target}\"` not applied: no such file"));
        };
        let _ = at;
        let shown = display_name(&path, &self.cwd);
        self.file(&path, &shown, &text, false);
        Ok(())
    }
}

fn short(s: &str) -> String {
    let one: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() > 60 {
        let cut: String = one.chars().take(57).collect();
        format!("{cut}...")
    } else {
        one
    }
}

#[derive(Debug, PartialEq)]
enum Stmt {
    Plain(String),
    /// A block statement (`sub`, `if`, `foreach`...): its head.
    Block(String),
    /// A string or bracket never closed.
    Unclosed,
}

/// The statements of `text`, each with the line it starts on: split at
/// `;` outside strings and brackets; a `{ ... }` block ends its statement
/// (a `sub` or a condition); comments and POD left out; reading stops at
/// `__END__`.
fn statements(text: &str) -> Vec<(usize, Stmt)> {
    let c: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let (mut i, mut line) = (0usize, 1usize);
    let mut cur = String::new();
    let mut start = 0usize;
    let mut depth = 0i32;
    let mut braced = false;
    let at_line_start = |c: &[char], i: usize| i == 0 || c[i - 1] == '\n';
    while i < c.len() {
        let ch = c[i];
        // (POD: `=word` at a line's start, to `=cut`)
        if ch == '=' && at_line_start(&c, i) && c.get(i + 1).is_some_and(char::is_ascii_alphabetic)
        {
            while i < c.len() {
                let ls = i;
                while i < c.len() && c[i] != '\n' {
                    i += 1;
                }
                let l: String = c[ls..i].iter().collect();
                if i < c.len() {
                    i += 1;
                    line += 1;
                }
                if l.starts_with("=cut") {
                    break;
                }
            }
            continue;
        }
        if cur.trim().is_empty() && (ch == '_') {
            let rest: String = c[i..c.len().min(i + 8)].iter().collect();
            if rest.starts_with("__END__") || rest.starts_with("__DATA__") {
                break;
            }
        }
        match ch {
            '#' => {
                while i < c.len() && c[i] != '\n' {
                    i += 1;
                }
                continue;
            }
            '\n' => {
                line += 1;
                cur.push(' ');
                i += 1;
                continue;
            }
            '\'' | '"' => {
                if cur.trim().is_empty() {
                    start = line;
                }
                let q = ch;
                cur.push(ch);
                i += 1;
                let mut closed = false;
                while i < c.len() {
                    let d = c[i];
                    cur.push(d);
                    i += 1;
                    if d == '\n' {
                        line += 1;
                    }
                    if d == '\\' && i < c.len() {
                        cur.push(c[i]);
                        if c[i] == '\n' {
                            line += 1;
                        }
                        i += 1;
                        continue;
                    }
                    if d == q {
                        closed = true;
                        break;
                    }
                }
                if !closed {
                    out.push((start, Stmt::Unclosed));
                    return out;
                }
                continue;
            }
            _ => {}
        }
        // (q{...}, qq{...}, qw(...): a quoted string with any brackets)
        if (ch == 'q')
            && (i == 0 || !(c[i - 1].is_alphanumeric() || c[i - 1] == '_' || c[i - 1] == '$'))
        {
            let mut j = i + 1;
            if c.get(j) == Some(&'q') || c.get(j) == Some(&'w') {
                j += 1;
            }
            if let Some(&open) = c.get(j)
                && "{([</|!".contains(open)
            {
                let close = match open {
                    '{' => '}',
                    '(' => ')',
                    '[' => ']',
                    '<' => '>',
                    o => o,
                };
                if cur.trim().is_empty() {
                    start = line;
                }
                let mut k = j + 1;
                let mut d = 1;
                while k < c.len() {
                    if c[k] == '\\' {
                        k += 2;
                        continue;
                    }
                    if c[k] == close && open != close {
                        d -= 1;
                        if d == 0 {
                            break;
                        }
                    } else if c[k] == open {
                        if open == close {
                            break;
                        }
                        d += 1;
                    }
                    k += 1;
                }
                if k >= c.len() {
                    out.push((start, Stmt::Unclosed));
                    return out;
                }
                for &x in &c[i..=k] {
                    if x == '\n' {
                        line += 1;
                    }
                    cur.push(x);
                }
                i = k + 1;
                continue;
            }
        }
        if !ch.is_whitespace() && cur.trim().is_empty() {
            start = line;
            braced = false;
        }
        match ch {
            '(' | '[' => depth += 1,
            ')' | ']' => depth -= 1,
            '{' => {
                // (a block: after `)` or a word that starts one; a hash
                // subscript `$ENV{...}` is not)
                let head = cur.trim_end();
                let block = depth == 0
                    && (head.ends_with(')')
                        || head.ends_with("else")
                        || head.ends_with("do")
                        || head.ends_with("BEGIN")
                        || head.ends_with("END")
                        || head.split_whitespace().next() == Some("sub"))
                    && !head.ends_with("->");
                if block {
                    let head_s = head.to_owned();
                    let mut d = 1;
                    i += 1;
                    while i < c.len() && d > 0 {
                        match c[i] {
                            '{' => d += 1,
                            '}' => d -= 1,
                            '\n' => line += 1,
                            '#' => {
                                while i < c.len() && c[i] != '\n' {
                                    i += 1;
                                }
                                continue;
                            }
                            q @ ('\'' | '"') => {
                                i += 1;
                                while i < c.len() && c[i] != q {
                                    if c[i] == '\\' {
                                        i += 1;
                                    }
                                    if i < c.len() && c[i] == '\n' {
                                        line += 1;
                                    }
                                    i += 1;
                                }
                            }
                            _ => {}
                        }
                        i += 1;
                    }
                    if d > 0 {
                        out.push((start, Stmt::Unclosed));
                        return out;
                    }
                    out.push((start, Stmt::Block(head_s)));
                    cur.clear();
                    braced = true;
                    // (`sub name {...};` or `if (...) {...} else {...}`)
                    continue;
                }
                depth += 1;
            }
            '}' => depth -= 1,
            ';' if depth <= 0 => {
                if !cur.trim().is_empty() {
                    out.push((start, Stmt::Plain(cur.trim().to_owned())));
                } else if braced {
                    // (the `;` after a block)
                }
                cur.clear();
                depth = 0;
                i += 1;
                continue;
            }
            _ => {}
        }
        // (`else`/`elsif` after a block: part of it)
        if braced && cur.trim().is_empty() && ch.is_alphabetic() {
            let rest: String = c[i..c.len().min(i + 5)].iter().collect();
            if rest.starts_with("else") {
                if let Some((_, Stmt::Block(h))) = out.last_mut() {
                    h.push_str(" ...");
                }
            }
        }
        cur.push(ch);
        i += 1;
    }
    if !cur.trim().is_empty() {
        if depth != 0 {
            out.push((start, Stmt::Unclosed));
        } else {
            out.push((start, Stmt::Plain(cur.trim().to_owned())));
        }
    }
    // (an `else { ... }` block after an `if` block came out as its own
    // block: one note for the whole condition)
    let mut merged: Vec<(usize, Stmt)> = Vec::new();
    for (l, s) in out {
        if let (Stmt::Block(h), Some((_, Stmt::Block(_)))) = (&s, merged.last())
            && (h.starts_with("else") || h.starts_with("elsif"))
        {
            continue;
        }
        merged.push((l, s));
    }
    merged
}

/// The tokens of statement `s`.
fn lex(s: &str) -> Result<Vec<Tok>, String> {
    let c: Vec<char> = s.chars().collect();
    let mut t = Vec::new();
    let mut i = 0;
    let ident = |c: &[char], mut i: usize| {
        let st = i;
        while i < c.len() && (c[i].is_alphanumeric() || c[i] == '_' || c[i] == ':') {
            // (`::` only, in a package name)
            if c[i] == ':' && c.get(i + 1) != Some(&':') && (i == st || c[i - 1] != ':') {
                break;
            }
            i += 1;
        }
        (c[st..i].iter().collect::<String>(), i)
    };
    while i < c.len() {
        let ch = c[i];
        if ch.is_whitespace() {
            i += 1;
            continue;
        }
        if ch == '$' || ch == '@' {
            let (j, braced) = if c.get(i + 1) == Some(&'{') {
                (i + 2, true)
            } else {
                (i + 1, false)
            };
            let (name, mut k) = ident(&c, j);
            if name.is_empty() {
                return Err(format!("`{ch}` without a name"));
            }
            if braced {
                if c.get(k) != Some(&'}') {
                    return Err("`${` not closed".into());
                }
                k += 1;
            }
            t.push(if ch == '$' {
                Tok::Scalar(name)
            } else {
                Tok::Array(name)
            });
            i = k;
            continue;
        }
        if ch == '\'' || ch == '"' {
            let (v, k) = quoted(&c, i + 1, ch)?;
            t.push(Tok::Str(if ch == '"' { format!("\u{1}{v}") } else { v }));
            i = k;
            continue;
        }
        if ch.is_ascii_digit() {
            let st = i;
            while i < c.len() && (c[i].is_ascii_digit() || c[i] == '.' || c[i] == '_') {
                i += 1;
            }
            t.push(Tok::Num(c[st..i].iter().collect()));
            continue;
        }
        if ch.is_alphabetic() || ch == '_' {
            let (w, k) = ident(&c, i);
            // (q{}, qq{}, qw())
            if matches!(w.as_str(), "q" | "qq" | "qw")
                && let Some(&open) = c[k..].iter().find(|x| !x.is_whitespace())
                && "{([</|!".contains(open)
            {
                let o = k + c[k..].iter().position(|&x| x == open).unwrap_or(0);
                let close = match open {
                    '{' => '}',
                    '(' => ')',
                    '[' => ']',
                    '<' => '>',
                    o => o,
                };
                let mut d = 1;
                let mut e = o + 1;
                while e < c.len() {
                    if c[e] == '\\' {
                        e += 2;
                        continue;
                    }
                    if c[e] == close && (open != close) {
                        d -= 1;
                        if d == 0 {
                            break;
                        }
                    } else if c[e] == open {
                        if open == close {
                            break;
                        }
                        d += 1;
                    }
                    e += 1;
                }
                if e >= c.len() {
                    return Err(format!("`{w}{open}` not closed"));
                }
                let body: String = c[o + 1..e].iter().collect();
                match w.as_str() {
                    "q" => t.push(Tok::Str(body)),
                    "qq" => t.push(Tok::Str(format!("\u{1}{body}"))),
                    _ => {
                        t.push(Tok::Op("("));
                        let mut first = true;
                        for word in body.split_whitespace() {
                            if !first {
                                t.push(Tok::Op(","));
                            }
                            first = false;
                            t.push(Tok::Str(word.to_owned()));
                        }
                        t.push(Tok::Op(")"));
                    }
                }
                i = e + 1;
                continue;
            }
            t.push(Tok::Word(w));
            i = k;
            continue;
        }
        let two: String = c[i..c.len().min(i + 2)].iter().collect();
        let op: Option<&'static str> = match two.as_str() {
            ".=" => Some(".="),
            "||" => Some("||"),
            "//" => Some("//"),
            "=>" => Some("=>"),
            "->" => Some("->"),
            "==" | "!=" | "=~" | "&&" | "<=" | ">=" | "+=" | "-=" => {
                return Err(format!("`{two}`: an expression PhiTeX does not evaluate"));
            }
            _ => None,
        };
        if let Some(o) = op {
            t.push(Tok::Op(o));
            i += 2;
            continue;
        }
        let one: &'static str = match ch {
            '=' => "=",
            '.' => ".",
            ',' => ",",
            '(' => "(",
            ')' => ")",
            '{' => "{",
            '}' => "}",
            _ => return Err(format!("`{ch}`: an expression PhiTeX does not evaluate")),
        };
        t.push(Tok::Op(one));
        i += 1;
    }
    Ok(t)
}

/// A quoted string's body from `i` to its closing `q`, escapes kept for
/// a double-quoted one (interpolated later), a single-quoted one's
/// `\\` and `\'` undone.
fn quoted(c: &[char], mut i: usize, q: char) -> Result<(String, usize), String> {
    let mut v = String::new();
    while i < c.len() {
        let d = c[i];
        if d == '\\' && i + 1 < c.len() {
            let n = c[i + 1];
            if q == '\'' {
                if n == '\\' || n == '\'' {
                    v.push(n);
                } else {
                    v.push('\\');
                    v.push(n);
                }
            } else {
                v.push('\\');
                v.push(n);
            }
            i += 2;
            continue;
        }
        if d == q {
            return Ok((v, i + 1));
        }
        v.push(d);
        i += 1;
    }
    Err("a string not closed".into())
}

struct Parser<'p, 'r, 'a> {
    t: &'p [Tok],
    i: usize,
    r: &'r mut Reader<'a>,
    /// `__FILE__`.
    file: &'p str,
}

impl Parser<'_, '_, '_> {
    fn next(&mut self) -> Option<Tok> {
        let t = self.t.get(self.i).cloned();
        self.i += 1;
        t
    }

    fn peek(&self) -> Option<&Tok> {
        self.t.get(self.i)
    }

    fn eat(&mut self, op: &str) -> bool {
        if matches!(self.peek(), Some(Tok::Op(o)) if *o == op) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    fn done(&self) -> bool {
        self.i >= self.t.len()
    }

    fn end(&self) -> Result<(), Why> {
        if self.done() {
            Ok(())
        } else {
            Err(format!(
                "not understood from `{}` on: PhiTeX reads latexmk's settings as data and runs \
                 no Perl",
                tok_text(&self.t[self.i])
            ))
        }
    }

    /// `{KEY}` or `{'KEY'}` after `$ENV`.
    fn hash_key(&mut self) -> Result<String, Why> {
        if !self.eat("{") {
            return Err("`$ENV` without a key".into());
        }
        let k = match self.next() {
            Some(Tok::Word(w)) => w,
            Some(Tok::Str(s)) => self.string(&s)?,
            _ => return Err("`$ENV{...}`: a key PhiTeX does not evaluate".into()),
        };
        if !self.eat("}") {
            return Err("`$ENV{` not closed".into());
        }
        Ok(k)
    }

    /// A string token's value (a double-quoted one interpolated).
    fn string(&self, s: &str) -> Result<String, Why> {
        match s.strip_prefix('\u{1}') {
            Some(body) => self.interpolate(body),
            None => Ok(s.to_owned()),
        }
    }

    fn interpolate(&self, body: &str) -> Result<String, Why> {
        let c: Vec<char> = body.chars().collect();
        let mut out = String::new();
        let mut i = 0;
        while i < c.len() {
            let ch = c[i];
            if ch == '\\' && i + 1 < c.len() {
                match c[i + 1] {
                    'n' => out.push('\n'),
                    't' => out.push('\t'),
                    n => out.push(n),
                }
                i += 2;
                continue;
            }
            if ch == '$' && i + 1 < c.len() {
                let (j, braced) = if c[i + 1] == '{' {
                    (i + 2, true)
                } else {
                    (i + 1, false)
                };
                let st = j;
                let mut k = j;
                while k < c.len() && (c[k].is_alphanumeric() || c[k] == '_') {
                    k += 1;
                }
                let name: String = c[st..k].iter().collect();
                if name.is_empty() {
                    out.push(ch);
                    i += 1;
                    continue;
                }
                if braced {
                    if c.get(k) != Some(&'}') {
                        return Err("`${` not closed in a string".into());
                    }
                    k += 1;
                }
                if name == "ENV" && c.get(k) == Some(&'{') {
                    let e = c[k..]
                        .iter()
                        .position(|&x| x == '}')
                        .ok_or("`$ENV{` not closed in a string")?;
                    let key: String = c[k + 1..k + e].iter().collect();
                    let key = key.trim_matches(|x| x == '\'' || x == '"').to_owned();
                    out.push_str(&self.r.env.get(&key).cloned().flatten().unwrap_or_default());
                    i = k + e + 1;
                    continue;
                }
                out.push_str(&self.scalar_value(&name)?.unwrap_or_default());
                i = k;
                continue;
            }
            out.push(ch);
            i += 1;
        }
        Ok(out)
    }

    fn scalar_value(&self, name: &str) -> Result<Option<String>, Why> {
        if let Some(v) = self.r.vars.get(name) {
            return Ok(v.clone());
        }
        match name {
            "search_path_separator" => Ok(Some(":".into())),
            "HOME" => Ok(self.r.env.get("HOME").cloned().flatten()),
            _ => Err(format!(
                "${name} is not set here (PhiTeX knows only the variables the files set)"
            )),
        }
    }

    /// `a, b, ...` up to the end or a `)`: each a string.
    fn list(&mut self) -> Result<Vec<String>, Why> {
        let paren = self.eat("(");
        let mut out = Vec::new();
        loop {
            if paren && self.eat(")") {
                break;
            }
            if self.done() || matches!(self.peek(), Some(Tok::Op(")"))) {
                break;
            }
            if let Some(v) = self.expr()? {
                out.push(v);
            }
            if !self.eat(",") && !self.eat("=>") {
                if paren && !self.eat(")") {
                    return Err("a list not closed".into());
                }
                break;
            }
        }
        Ok(out)
    }

    /// A call's arguments: `(a, b)` or `a, b`.
    fn call_args(&mut self) -> Result<Vec<Option<String>>, Why> {
        let paren = self.eat("(");
        let mut out = Vec::new();
        if paren && self.eat(")") {
            return Ok(out);
        }
        loop {
            out.push(self.expr()?);
            if !self.eat(",") {
                break;
            }
        }
        if paren && !self.eat(")") {
            return Err("a call's `(` not closed".into());
        }
        Ok(out)
    }

    fn expr(&mut self) -> Result<Option<String>, Why> {
        let mut v = self.cat()?;
        loop {
            if self.eat("||") {
                let w = self.cat()?;
                if v.as_deref().is_none_or(|s| s.is_empty() || s == "0") {
                    v = w;
                }
            } else if self.eat("//") {
                let w = self.cat()?;
                if v.is_none() {
                    v = w;
                }
            } else {
                return Ok(v);
            }
        }
    }

    fn cat(&mut self) -> Result<Option<String>, Why> {
        let mut v = self.term()?;
        while self.eat(".") {
            let w = self.term()?;
            v = Some(v.unwrap_or_default() + &w.unwrap_or_default());
        }
        Ok(v)
    }

    fn term(&mut self) -> Result<Option<String>, Why> {
        match self.next() {
            Some(Tok::Str(s)) => self.string(&s).map(Some),
            Some(Tok::Num(n)) => Ok(Some(n)),
            Some(Tok::Scalar(n)) if n == "ENV" => {
                let k = self.hash_key()?;
                Ok(self.r.env.get(&k).cloned().flatten())
            }
            Some(Tok::Scalar(n)) => self.scalar_value(&n),
            Some(Tok::Op("(")) => {
                let v = self.expr()?;
                if !self.eat(")") {
                    return Err("a `(` not closed".into());
                }
                Ok(v)
            }
            Some(Tok::Word(w)) => self.word(&w),
            Some(t) => Err(format!(
                "`{}`: an expression PhiTeX does not evaluate",
                tok_text(&t)
            )),
            None => Err("an expression ends early".into()),
        }
    }

    fn word(&mut self, w: &str) -> Result<Option<String>, Why> {
        let cwd = self.r.cwd.clone();
        match w {
            "__FILE__" => Ok(Some(self.file.to_owned())),
            "undef" => Ok(None),
            "dirname" | "File::Basename::dirname" => {
                let a = self.one_arg(w)?;
                Ok(Some(dirname(&a)))
            }
            "basename" | "File::Basename::basename" => {
                let a = self.one_arg(w)?;
                Ok(Some(basename(&a)))
            }
            "abs_path" | "realpath" | "Cwd::abs_path" | "Cwd::realpath" | "rel2abs" => {
                let a = self.one_arg(w)?;
                Ok(Some(self.r.files.canonical(&join(&cwd, &a))))
            }
            "getcwd" | "cwd" | "Cwd::getcwd" | "Cwd::cwd" => {
                if self.eat("(") && !self.eat(")") {
                    return Err(format!("{w}() takes no argument"));
                }
                Ok(Some(cwd))
            }
            "File::Spec" if self.eat("->") => {
                let m = match self.next() {
                    Some(Tok::Word(m)) => m,
                    _ => return Err("File::Spec-> without a method".into()),
                };
                let args = self.call_args()?;
                let parts: Vec<String> = args.into_iter().map(Option::unwrap_or_default).collect();
                match m.as_str() {
                    "catfile" | "catdir" | "join" => Ok(Some(parts.join("/"))),
                    "rel2abs" => Ok(Some(join(&cwd, parts.first().map_or("", |s| s)))),
                    _ => Err(format!(
                        "File::Spec->{m}: a method PhiTeX does not evaluate"
                    )),
                }
            }
            "catfile" | "catdir" => {
                let args = self.call_args()?;
                let parts: Vec<String> = args.into_iter().map(Option::unwrap_or_default).collect();
                Ok(Some(parts.join("/")))
            }
            _ => Err(format!("`{w}`: a function PhiTeX does not evaluate")),
        }
    }

    fn one_arg(&mut self, f: &str) -> Result<String, Why> {
        let args = self.call_args()?;
        match args.as_slice() {
            [Some(a)] => Ok(a.clone()),
            [None] => Ok(String::new()),
            _ => Err(format!("{f} takes one argument")),
        }
    }
}

fn tok_text(t: &Tok) -> String {
    match t {
        Tok::Scalar(n) => format!("${n}"),
        Tok::Array(n) => format!("@{n}"),
        Tok::Str(s) => format!("\"{}\"", s.trim_start_matches('\u{1}')),
        Tok::Num(n) | Tok::Word(n) => n.clone(),
        Tok::Op(o) => (*o).to_owned(),
    }
}

#[cfg(test)]
mod tests;
