//! A native latexdiff: two versions of a LaTeX project in, the new one
//! with the differences marked (latexdiff's markup, `\DIFadd{…}`,
//! `\DIFdel{…}`, …) and the list of changes out.
//!
//! - Both versions are flattened (`\input`, `\include`, as latexdiff
//!   `--flatten`), each following its own inputs: a file the new version no
//!   longer inputs is deleted text where it was, a new one is added text.
//! - The preamble is the new version's, unmarked, with latexdiff's
//!   definitions before `\begin{document}`.
//! - The bodies are read from their CST ([`phitex_syntax`]) into tokens
//!   (words, commands with their arguments, math as a unit; [`tok`]), by a
//!   signature table ([`sig`]) extended from the preambles.
//! - Paragraphs are aligned first; the runs of paragraphs that differ are
//!   diffed by tokens (Myers, [`myers`]); a command whose text changed is
//!   diffed inside (`\section{Intro\DIFadd{duction}}`).
//! - The markup is put only where it compiles: a deleted command that
//!   cannot sit in `\DIFdel{…}` is commented out (`%DIFDELCMD <`), a deleted
//!   `\section` steps its counter back, math is coarse (a changed display
//!   is shown whole, old struck out, new added), floats and alignments get
//!   the `FL` markup, verbatim and pictures are never marked inside.
//!
//! The entry points: [`diff`], or a [`Baseline`] kept to diff new versions
//! against (phase 2, the live diff, re-diffs only what an edit touched).

pub mod flatten;
pub mod live;
pub mod myers;
pub mod sig;
pub mod tok;

mod emit;

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

pub use flatten::{Flat, flatten};
pub use live::{Live, Place, Stats, changes_json, places};
pub use sig::{Class, EnvKind, EnvSig, Sig, Signatures};

/// A set of files: a project, at one version. Paths are relative to the
/// project's root, `/`-separated.
pub trait Files {
    /// The text of file `path`, if there is one.
    fn read(&self, path: &str) -> Option<String>;
}

impl Files for BTreeMap<String, String> {
    fn read(&self, path: &str) -> Option<String> {
        self.get(path).cloned()
    }
}

impl<S: std::hash::BuildHasher> Files for HashMap<String, String, S> {
    fn read(&self, path: &str) -> Option<String> {
        self.get(path).cloned()
    }
}

/// The files under a directory.
#[derive(Clone, Debug)]
pub struct Dir(pub PathBuf);

impl Files for Dir {
    fn read(&self, path: &str) -> Option<String> {
        let p = self.0.join(path);
        let bytes = std::fs::read(p).ok()?;
        Some(String::from_utf8_lossy(&bytes).into_owned())
    }
}

/// How changes look (latexdiff's `--type`): its definitions of `\DIFadd`
/// and `\DIFdel`, as latexdiff.pl has them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Markup {
    /// Added text wavy-underlined in blue, deleted struck out in red
    /// (ulem).
    #[default]
    Underline,
    /// Added text blue and sans serif, deleted red, in a footnote.
    Ctraditional,
    /// Added text sans serif, deleted in a footnote.
    Traditional,
    /// Added text blue and sans serif, deleted red and small.
    Cfont,
    /// Added text sans serif, deleted small and struck out (ulem).
    Fontstrike,
    /// A change bar in the margin, added text blue, deleted red
    /// (changebar).
    Cchangebar,
    /// [`Markup::Cfont`] with change bars.
    Cfontchbar,
    /// [`Markup::Underline`] with change bars.
    Culinechbar,
    /// Change bars only, deleted text not shown.
    Changebar,
    /// Added text as it is, deleted text not shown.
    Invisible,
    /// Added text bold, deleted text not shown.
    Bold,
}

impl Markup {
    /// Every type, in latexdiff's order.
    pub const ALL: [Markup; 11] = [
        Markup::Underline,
        Markup::Ctraditional,
        Markup::Traditional,
        Markup::Cfont,
        Markup::Fontstrike,
        Markup::Cchangebar,
        Markup::Cfontchbar,
        Markup::Culinechbar,
        Markup::Changebar,
        Markup::Invisible,
        Markup::Bold,
    ];

    /// latexdiff's name for it (`UNDERLINE`, …).
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Markup::Underline => "UNDERLINE",
            Markup::Ctraditional => "CTRADITIONAL",
            Markup::Traditional => "TRADITIONAL",
            Markup::Cfont => "CFONT",
            Markup::Fontstrike => "FONTSTRIKE",
            Markup::Cchangebar => "CCHANGEBAR",
            Markup::Cfontchbar => "CFONTCHBAR",
            Markup::Culinechbar => "CULINECHBAR",
            Markup::Changebar => "CHANGEBAR",
            Markup::Invisible => "INVISIBLE",
            Markup::Bold => "BOLD",
        }
    }

    /// The type latexdiff's name (any case) names.
    #[must_use]
    pub fn parse(name: &str) -> Option<Markup> {
        Markup::ALL
            .into_iter()
            .find(|m| m.name().eq_ignore_ascii_case(name.trim()))
    }

    /// Its definitions, as latexdiff.pl has them.
    fn preamble(self) -> &'static str {
        match self {
            Markup::Underline => UNDERLINE,
            Markup::Ctraditional => CTRADITIONAL,
            Markup::Traditional => TRADITIONAL,
            Markup::Cfont => CFONT,
            Markup::Fontstrike => FONTSTRIKE,
            Markup::Cchangebar => CCHANGEBAR,
            Markup::Cfontchbar => CFONTCHBAR,
            Markup::Culinechbar => CULINECHBAR,
            Markup::Changebar => CHANGEBAR,
            Markup::Invisible => INVISIBLE,
            Markup::Bold => BOLD,
        }
    }

    /// Whether it uses ulem (`\sout` then works in math with amsmath).
    #[must_use]
    pub fn ulem(self) -> bool {
        self.preamble().contains("{ulem}")
    }

    /// Whether deleted text goes in a footnote (not in floats: shown small
    /// there).
    fn footnotes(self) -> bool {
        matches!(self, Markup::Ctraditional | Markup::Traditional)
    }
}

/// How a changed range is set off (latexdiff's `--subtype`): its
/// definitions of `\DIFaddbegin`, `\DIFaddend`, … (latexdiff's `DVIPSCOL`,
/// for dvips only, is not here).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Subtype {
    /// Not at all (the markup alone).
    #[default]
    Safe,
    /// The whole range colored: added blue, deleted red.
    Color,
    /// A marker in the margin where a change begins and ends (`a[`, `d[`,
    /// `]`; latexdiff's `MARGIN`).
    Margin,
    /// A label at each change's ends (`DIFchgb1`, …), from which the pages
    /// with changes can be found (deprecated by latexdiff for `ZLABEL`).
    Label,
    /// The labels by zref, with absolute page numbers.
    Zlabel,
    /// Only the pages with changes shipped (atbegshi).
    OnlyChangedPage,
}

impl Subtype {
    pub const ALL: [Subtype; 6] = [
        Subtype::Safe,
        Subtype::Color,
        Subtype::Margin,
        Subtype::Label,
        Subtype::Zlabel,
        Subtype::OnlyChangedPage,
    ];

    /// latexdiff's name for it (`SAFE`, …).
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Subtype::Safe => "SAFE",
            Subtype::Color => "COLOR",
            Subtype::Margin => "MARGIN",
            Subtype::Label => "LABEL",
            Subtype::Zlabel => "ZLABEL",
            Subtype::OnlyChangedPage => "ONLYCHANGEDPAGE",
        }
    }

    /// The subtype latexdiff's name (any case) names; `MARKER` is
    /// `MARGIN`.
    #[must_use]
    pub fn parse(name: &str) -> Option<Subtype> {
        if name.trim().eq_ignore_ascii_case("marker") {
            return Some(Subtype::Margin);
        }
        Subtype::ALL
            .into_iter()
            .find(|m| m.name().eq_ignore_ascii_case(name.trim()))
    }

    fn preamble(self) -> &'static str {
        match self {
            Subtype::Safe => SAFE,
            Subtype::Color => COLOR,
            Subtype::Margin => MARGIN,
            Subtype::Label => LABEL,
            Subtype::Zlabel => ZLABEL,
            Subtype::OnlyChangedPage => ONLYCHANGEDPAGE,
        }
    }

    /// Whether floats are marked as the text is (latexdiff's `IDENTICAL`
    /// float type: a change in a float is a change for the labels too).
    fn identical_floats(self) -> bool {
        matches!(self, Subtype::Label | Subtype::OnlyChangedPage)
    }
}

/// The output driver changebar is loaded for (latexdiff's `--driver`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Driver {
    #[default]
    Pdftex,
    Xetex,
    Dvips,
}

impl Driver {
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Driver::Pdftex => "pdftex",
            Driver::Xetex => "xetex",
            Driver::Dvips => "dvips",
        }
    }
}

/// A color for added or deleted text: an xcolor name or expression
/// (`blue`, `blue!60!black`: letters, digits, `!` and `.`), or `#RRGGBB`.
/// Nothing else is taken, so nothing but a color reaches the preamble.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Color(String);

impl Color {
    /// The color `s` names.
    ///
    /// # Errors
    ///
    /// If it is neither a hex color (`#` and six hex digits, or the
    /// digits alone) nor a name of letters, digits, `!` and `.`.
    pub fn parse(s: &str) -> Result<Color, String> {
        let s = s.trim();
        let hex = s.strip_prefix('#').unwrap_or(s);
        if hex.len() == 6 && hex.bytes().all(|c| c.is_ascii_hexdigit()) {
            return Ok(Color(format!("#{}", hex.to_ascii_uppercase())));
        }
        if s.starts_with('#') {
            return Err(format!("`{s}` is not a color: #RRGGBB has six hex digits"));
        }
        if !s.is_empty()
            && s.bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'!' || c == b'.')
            && s.as_bytes()[0].is_ascii_alphabetic()
        {
            return Ok(Color(s.to_owned()));
        }
        Err(format!(
            "`{s}` is not a color: an xcolor name (blue, blue!60!black) or #RRGGBB"
        ))
    }

    /// As given: `#RRGGBB` or a name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The xcolor command that defines color `name` as this one.
    fn define(&self, name: &str) -> String {
        match self.0.strip_prefix('#') {
            Some(hex) => format!("\\definecolor{{{name}}}{{HTML}}{{{hex}}}"),
            None => format!("\\colorlet{{{name}}}{{{}}}", self.0),
        }
    }
}

/// What to diff, and how.
#[derive(Clone, Debug, Default)]
pub struct Options {
    pub markup: Markup,
    pub subtype: Subtype,
    /// The colors of added and deleted text, where the type or subtype
    /// colors it (none: latexdiff's blue and red, its preamble as it is).
    /// latexdiff has no option for them.
    pub add_color: Option<Color>,
    pub del_color: Option<Color>,
    /// The driver changebar is loaded for.
    pub driver: Driver,
    /// Commands to take as safe, text (last argument diffed inside) or
    /// unsafe, over the table and the preambles (latexdiff's
    /// `--append-safecmd`, `--append-textcmd`, `--exclude-safecmd`).
    pub safe: Vec<String>,
    pub text: Vec<String>,
    pub unsafe_cmds: Vec<String>,
}

impl Options {
    /// The latexdiff command line that marks up as these options do (for
    /// comparisons; the colors have no latexdiff option).
    #[must_use]
    pub fn latexdiff_args(&self) -> Vec<String> {
        let mut a = vec![
            format!("--type={}", self.markup.name()),
            format!("--subtype={}", self.subtype.name()),
            "--math-markup=coarse".into(),
            "--flatten".into(),
        ];
        if self.driver != Driver::Pdftex {
            a.push(format!("--driver={}", self.driver.name()));
        }
        a
    }
}

/// What a change is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeKind {
    Add,
    Del,
    Change,
}

/// A place in a version: a file and a byte range in it (empty where a
/// change has nothing on that side: where it would be).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Loc {
    pub file: String,
    pub start: usize,
    pub end: usize,
}

/// A change: what it is, where on each side, its text on each side, the
/// section of the new version it is in (the title of the last
/// `\section`-like command before it), and where its markup is in the
/// output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub kind: ChangeKind,
    pub old: Loc,
    pub new: Loc,
    pub old_text: String,
    pub new_text: String,
    pub section: Option<String>,
    pub out: std::ops::Range<usize>,
}

/// The diff: the marked-up, flattened document, and the changes, in
/// document order.
#[derive(Clone, Debug)]
pub struct DiffOut {
    pub tex: String,
    pub changes: Vec<Change>,
}

/// Why a diff failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// The main file is not in a version (`old`: in the old one).
    NoMain { old: bool, path: String },
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NoMain { old, path } => write!(
                f,
                "the {} version has no {path}",
                if *old { "old" } else { "new" }
            ),
        }
    }
}

impl std::error::Error for Error {}

/// Diff `main` in `old` against `main` in `new`.
///
/// # Errors
///
/// If `main` is in neither version (a main file only the new version has is
/// diffed against an empty one: all added).
pub fn diff(
    old: &dyn Files,
    new: &dyn Files,
    main: &str,
    opts: &Options,
) -> Result<DiffOut, Error> {
    let base = Baseline::new(old, main, opts).or_else(|_| {
        // (a new document: everything added)
        Ok::<_, Error>(Baseline::from_flat(Flat::default(), opts))
    })?;
    base.diff(new, main)
}

/// Where a document's body is: where `\begin{document}` begins, where the
/// body begins after it, and where `\end{document}` begins.
#[derive(Clone, Copy, Debug)]
struct Body {
    begin: usize,
    start: usize,
    end: usize,
}

/// Find `\begin{document}` and `\end{document}` at the top level of
/// `text` (comments and verbatim are not read: the CST has them as
/// tokens). Without them, the text is all body.
fn find_body(text: &str) -> Body {
    use phitex_syntax::{SyntaxKind as K, Tree};
    let tree = Tree::parse(text);
    let mut begin = None;
    let mut end = None;
    for r in tree.reds() {
        let kids: Vec<_> = r.children().collect();
        for (i, c) in kids.iter().enumerate() {
            if c.green.kind() != K::Cs {
                continue;
            }
            let cs = &text[c.offset..c.end()];
            let Some(g) = kids[i + 1..]
                .iter()
                .find(|k| k.green.kind() != K::Space)
                .filter(|k| k.green.kind() == K::Group)
            else {
                continue;
            };
            if text[g.offset..g.end()].replace(' ', "") != "{document}" {
                continue;
            }
            match cs {
                "\\begin" if begin.is_none() => begin = Some((c.offset, g.end())),
                "\\end" if begin.is_some() => end = Some(c.offset),
                _ => {}
            }
        }
    }
    match begin {
        Some((b, s)) => Body {
            begin: b,
            start: s,
            end: end.unwrap_or(text.len()).max(s),
        },
        None => Body {
            begin: 0,
            start: 0,
            end: text.len(),
        },
    }
}

/// A version diffed against: the old one, flattened and read (phase 2
/// keeps one per baseline and diffs each new version against it).
pub struct Baseline {
    flat: Flat,
    body: Body,
    learned: sig::Learned,
    opts: Options,
}

impl Baseline {
    /// The old version: `main` in `old`, flattened.
    ///
    /// # Errors
    ///
    /// If `main` is not there.
    pub fn new(old: &dyn Files, main: &str, opts: &Options) -> Result<Baseline, Error> {
        let flat = flatten(old, main).map_err(|_| Error::NoMain {
            old: true,
            path: main.to_owned(),
        })?;
        Ok(Baseline::from_flat(flat, opts))
    }

    /// The old version, already flattened.
    #[must_use]
    pub fn from_flat(flat: Flat, opts: &Options) -> Baseline {
        let body = find_body(&flat.text);
        let mut sigs = Signatures::latexdiff();
        let learned = sigs.learn(&flat.text[..body.begin]);
        Baseline {
            flat,
            body,
            learned,
            opts: opts.clone(),
        }
    }

    /// The old version's flattened text.
    #[must_use]
    pub fn flat(&self) -> &Flat {
        &self.flat
    }

    /// Diff `main` in `new` against the baseline.
    ///
    /// # Errors
    ///
    /// If `main` is not in `new`.
    pub fn diff(&self, new: &dyn Files, main: &str) -> Result<DiffOut, Error> {
        let flat = flatten(new, main).map_err(|_| Error::NoMain {
            old: false,
            path: main.to_owned(),
        })?;
        Ok(self.diff_flat(&flat))
    }

    /// Diff a flattened new version against the baseline.
    #[must_use]
    pub fn diff_flat(&self, new: &Flat) -> DiffOut {
        let nb = find_body(&new.text);
        let preamble = &new.text[..nb.begin];
        let sigs = self.signatures(preamble);
        let old_text = &self.flat.text;
        let ob = self.body;
        let read = |text: &str, b: Body| {
            let r = tok::Reader { text, sigs: &sigs };
            r.seq(&tok::lexemes(text, b.start, b.end))
        };
        let old_toks = read(old_text, ob);
        let new_toks = read(&new.text, nb);
        let amsmath = loads(preamble, &["amsmath", "mathtools"]);
        let mut em = emit::Em::new(old_text, &new.text, &sigs, amsmath);
        em.out.push_str(&self.head(new, nb));
        em.body(&old_toks, &new_toks, ob.end, nb.end);
        em.push(&new.text[nb.end..]);
        let (tex, raw) = em.finish();
        let changes = raw
            .iter()
            .map(|c| {
                let new_loc = locate(new, c.new);
                self.change(
                    c,
                    new_loc,
                    new.text[c.new.0..c.new.1].to_owned(),
                    c.section.clone(),
                    c.out.0..c.out.1,
                )
            })
            .collect();
        DiffOut { tex, changes }
    }

    /// The signature table for a new version whose preamble is `preamble`:
    /// latexdiff's, the new preamble's definitions, the caller's; what
    /// only the old preamble defines is unsafe deleted.
    fn signatures(&self, preamble: &str) -> Signatures {
        let mut sigs = Signatures::latexdiff();
        let new_learned = sigs.learn(preamble);
        sigs.set_old_only(&self.learned, &new_learned);
        for n in &self.opts.safe {
            let spec = sigs.cmd(n).map(|s| s.spec).unwrap_or_default();
            sigs.define(n, Sig::new(&spec, Class::Safe));
        }
        for n in &self.opts.text {
            let spec = sigs
                .cmd(n)
                .map(|s| s.spec)
                .filter(|s| s.contains('m'))
                .unwrap_or_else(|| "m".into());
            sigs.define(n, Sig::new(&spec, Class::Text { counter: false }));
        }
        for n in &self.opts.unsafe_cmds {
            let spec = sigs.cmd(n).map(|s| s.spec).unwrap_or_default();
            sigs.define(n, Sig::new(&spec, Class::Unsafe));
        }
        sigs
    }

    /// The output before the body: the new preamble, latexdiff's
    /// definitions at its end, `\begin{document}`.
    fn head(&self, new: &Flat, nb: Body) -> String {
        let mut s = String::from(&new.text[..nb.begin]);
        s.push_str(&dif_preamble(&self.opts, &new.text[..nb.begin]));
        s.push_str(&new.text[nb.begin..nb.start]);
        s
    }

    /// The old side's tokens, read with `sigs`.
    fn old_tokens(&self, sigs: &Signatures) -> Vec<tok::Tok> {
        let r = tok::Reader {
            text: &self.flat.text,
            sigs,
        };
        r.seq(&tok::lexemes(
            &self.flat.text,
            self.body.start,
            self.body.end,
        ))
    }

    /// A change, from its raw form (the old side in flat offsets) and its
    /// new side placed.
    fn change(
        &self,
        c: &emit::RawChange,
        new: Loc,
        new_text: String,
        section: Option<String>,
        out: std::ops::Range<usize>,
    ) -> Change {
        let kind = match (c.old.0 < c.old.1, c.new.0 < c.new.1) {
            (true, true) => ChangeKind::Change,
            (true, false) => ChangeKind::Del,
            _ => ChangeKind::Add,
        };
        Change {
            kind,
            old: locate(&self.flat, c.old),
            new,
            old_text: self.flat.text[c.old.0..c.old.1].to_owned(),
            new_text,
            section,
            out,
        }
    }
}

/// Flat range `r` of `f`, as a file and a range in it.
fn locate(f: &Flat, r: (usize, usize)) -> Loc {
    let (file, start, end) = f.locate_range(r.0, r.1);
    Loc {
        file: f.files.get(file).cloned().unwrap_or_default(),
        start,
        end,
    }
}

/// Whether `preamble` loads one of `packages` (`\usepackage[…]{a,b}`).
fn loads(preamble: &str, packages: &[&str]) -> bool {
    let mut rest = preamble;
    while let Some(k) = rest
        .find("\\usepackage")
        .or_else(|| rest.find("\\RequirePackage"))
    {
        let line_start = rest[..k].rfind('\n').map_or(0, |n| n + 1);
        let commented = rest[line_start..k].contains('%');
        rest = &rest[k + 1..];
        if commented {
            continue;
        }
        let Some(open) = rest.find('{') else { break };
        let Some(close) = rest[open..].find('}') else {
            break;
        };
        if rest[open + 1..open + close]
            .split(',')
            .any(|p| packages.contains(&p.trim()))
        {
            return true;
        }
    }
    false
}

/// The definitions latexdiff puts in the preamble, for `opts` (and a
/// preamble that loads amsmath or hyperref), each line tagged
/// `%DIF PREAMBLE` as latexdiff tags them. With colors chosen, xcolor is
/// loaded where latexdiff loads color, and the type's and subtype's blue
/// and red are `DIFaddcolor` and `DIFdelcolor`.
#[must_use]
pub fn dif_preamble(opts: &Options, preamble: &str) -> String {
    let hyperref = loads(preamble, &["hyperref"]);
    let amsmath = loads(preamble, &["amsmath", "mathtools"]);
    let colored = opts.add_color.is_some() || opts.del_color.is_some();
    let recolor = |l: &str| -> String {
        let l = l.replace(
            "[pdftex]{changebar}",
            &format!("[{}]{{changebar}}", opts.driver.name()),
        );
        if !colored {
            return l;
        }
        l.replace("\\RequirePackage{color}", "\\RequirePackage{xcolor}")
            .replace("\\color{blue}", "\\color{DIFaddcolor}")
            .replace("\\color{red}", "\\color{DIFdelcolor}")
    };
    let mut lines = Vec::new();
    if colored {
        let add = opts.add_color.clone().unwrap_or(Color("blue".into()));
        let del = opts.del_color.clone().unwrap_or(Color("red".into()));
        lines.push("%DIF COLORS PREAMBLE".to_owned());
        lines.push("\\RequirePackage{xcolor}".to_owned());
        lines.push(add.define("DIFaddcolor"));
        lines.push(del.define("DIFdelcolor"));
    }
    lines.push(format!("%DIF {} PREAMBLE", opts.markup.name()));
    for l in opts.markup.preamble().lines() {
        // (with hyperref, the markup is \DIFaddtex, wrapped below)
        let l = recolor(l);
        lines.push(if hyperref {
            l.replace("{\\DIFadd}", "{\\DIFaddtex}")
                .replace("{\\DIFdel}", "{\\DIFdeltex}")
        } else {
            l
        });
    }
    lines.push(format!("%DIF {} PREAMBLE", opts.subtype.name()));
    lines.extend(opts.subtype.preamble().lines().map(recolor));
    if opts.subtype.identical_floats() {
        lines.push("%DIF IDENTICAL PREAMBLE".into());
        lines.extend(IDENTICAL.lines().map(str::to_owned));
    } else {
        lines.push("%DIF FLOATSAFE PREAMBLE".into());
        if opts.markup.footnotes() {
            // (a footnote cannot be in a float's caption or cell: deleted
            // text there shown small, as latexdiff's TRADITIONALSAFE shows
            // it)
            lines.push("\\providecommand{\\color}[1]{}".into());
            lines.push(recolor(
                "\\providecommand{\\DIFdelFL}[1]{{\\protect\\color{red}[..{\\scriptsize {removed: #1}} ]}}",
            ));
        }
        lines.extend(FLOATSAFE.lines().map(str::to_owned));
    }
    if amsmath && opts.markup.ulem() {
        lines.push("%DIF AMSMATHULEM PREAMBLE".into());
        lines.extend(AMSMATHULEM.lines().map(str::to_owned));
    }
    if hyperref {
        lines.push("%DIF HYPERREF PREAMBLE".into());
        lines.extend(HYPERREF.lines().map(str::to_owned));
    }
    let mut s = String::from("%DIF PREAMBLE EXTENSION ADDED BY LATEXDIFF\n");
    for l in lines {
        s.push_str(&l);
        s.push_str(" %DIF PREAMBLE\n");
    }
    s.push_str("%DIF END PREAMBLE EXTENSION ADDED BY LATEXDIFF\n");
    s
}

// (latexdiff.pl's own definitions, as it has them)

const UNDERLINE: &str = r"\RequirePackage[normalem]{ulem}
\RequirePackage{color}\definecolor{RED}{rgb}{1,0,0}\definecolor{BLUE}{rgb}{0,0,1}
\providecommand{\DIFadd}[1]{{\protect\color{blue}\uwave{#1}}}
\providecommand{\DIFdel}[1]{{\protect\color{red}\sout{#1}}}";

const CTRADITIONAL: &str = r"\RequirePackage{color}\definecolor{RED}{rgb}{1,0,0}\definecolor{BLUE}{rgb}{0,0,1}
\RequirePackage[stable]{footmisc}
\DeclareOldFontCommand{\sf}{\normalfont\sffamily}{\mathsf}
\providecommand{\DIFadd}[1]{{\protect\color{blue} \sf #1}}
\providecommand{\DIFdel}[1]{{\protect\color{red} [..\footnote{removed: #1} ]}}";

const TRADITIONAL: &str = r"\RequirePackage[stable]{footmisc}
\DeclareOldFontCommand{\sf}{\normalfont\sffamily}{\mathsf}
\providecommand{\DIFadd}[1]{{\sf #1}}
\providecommand{\DIFdel}[1]{{[..\footnote{removed: #1} ]}}";

const CFONT: &str = r"\RequirePackage{color}\definecolor{RED}{rgb}{1,0,0}\definecolor{BLUE}{rgb}{0,0,1}
\DeclareOldFontCommand{\sf}{\normalfont\sffamily}{\mathsf}
\providecommand{\DIFadd}[1]{{\protect\color{blue} \sf #1}}
\providecommand{\DIFdel}[1]{{\protect\color{red} \scriptsize #1}}";

const FONTSTRIKE: &str = r"\RequirePackage[normalem]{ulem}
\DeclareOldFontCommand{\sf}{\normalfont\sffamily}{\mathsf}
\providecommand{\DIFadd}[1]{{\sf #1}}
\providecommand{\DIFdel}[1]{{\footnotesize \sout{#1}}}";

const CCHANGEBAR: &str = r"\RequirePackage[pdftex]{changebar}
\RequirePackage{color}\definecolor{RED}{rgb}{1,0,0}\definecolor{BLUE}{rgb}{0,0,1}
\providecommand{\DIFadd}[1]{\protect\cbstart{\protect\color{blue}#1}\protect\cbend}
\providecommand{\DIFdel}[1]{\protect\cbdelete{\protect\color{red}#1}\protect\cbdelete}";

const CFONTCHBAR: &str = r"\RequirePackage[pdftex]{changebar}
\RequirePackage{color}\definecolor{RED}{rgb}{1,0,0}\definecolor{BLUE}{rgb}{0,0,1}
\providecommand{\DIFadd}[1]{\protect\cbstart{\protect\color{blue}\sf #1}\protect\cbend}
\providecommand{\DIFdel}[1]{\protect\cbdelete{\protect\color{red}\scriptsize #1}\protect\cbdelete}";

const CULINECHBAR: &str = r"\RequirePackage[normalem]{ulem}
\RequirePackage[pdftex]{changebar}
\RequirePackage{color}\definecolor{RED}{rgb}{1,0,0}\definecolor{BLUE}{rgb}{0,0,1}
\providecommand{\DIFadd}[1]{\protect\cbstart{\protect\color{blue}\uwave{#1}}\protect\cbend}
\providecommand{\DIFdel}[1]{\protect\cbdelete{\protect\color{red}\sout{#1}}\protect\cbdelete}";

const CHANGEBAR: &str = r"\RequirePackage[pdftex]{changebar}
\providecommand{\DIFadd}[1]{\protect\cbstart{#1}\protect\cbend}
\providecommand{\DIFdel}[1]{\protect\cbdelete}";

const INVISIBLE: &str = r"\providecommand{\DIFadd}[1]{#1}
\providecommand{\DIFdel}[1]{}";

const BOLD: &str = r"\DeclareOldFontCommand{\bf}{\normalfont\bfseries}{\mathbf}
\providecommand{\DIFadd}[1]{{\bf #1}}
\providecommand{\DIFdel}[1]{}";

const SAFE: &str = r"\providecommand{\DIFaddbegin}{}
\providecommand{\DIFaddend}{}
\providecommand{\DIFdelbegin}{}
\providecommand{\DIFdelend}{}
\providecommand{\DIFmodbegin}{}
\providecommand{\DIFmodend}{}";

const MARGIN: &str = r"\providecommand{\DIFaddbegin}{\protect\marginpar{a[}}
\providecommand{\DIFaddend}{\protect\marginpar{]}}
\providecommand{\DIFdelbegin}{\protect\marginpar{d[}}
\providecommand{\DIFdelend}{\protect\marginpar{]}}
\providecommand{\DIFmodbegin}{\protect\marginpar{m[}}
\providecommand{\DIFmodend}{\protect\marginpar{]}}";

const COLOR: &str = r"\RequirePackage{color}
\providecommand{\DIFaddbegin}{\protect\color{blue}}
\providecommand{\DIFaddend}{\protect\color{black}}
\providecommand{\DIFdelbegin}{\protect\color{red}}
\providecommand{\DIFdelend}{\protect\color{black}}
\providecommand{\DIFmodbegin}{}
\providecommand{\DIFmodend}{}";

const LABEL: &str = r#"% To show only pages with changes (pdf) (external program pdftk needs to be installed)
% (only works for simple documents with non-repeated page numbers, otherwise use ZLABEL)
% pdflatex diff.tex
% pdflatex diff.tex
%pdftk diff.pdf cat \
%`perl -lne '\
% if (m/\\newlabel{DIFchg[b](\d*)}{{.*}{(.*)}}/) { $start{$1}=$2; print $2}\
% if (m/\\newlabel{DIFchg[e](\d*)}{{.*}{(.*)}}/) { \
%      if (defined($start{$1})) { \
%         for ($j=$start{$1}; $j<=$2; $j++) {print "$j";}\
%      } else { \
%         print "$2"\
%      }\
% }' diff.aux \
% | uniq \
% | tr  \\n ' '` \
% output diff-changedpages.pdf
% To show only pages with changes (dvips/dvipdf)
% dvips -pp `\
% [ put here the perl script from above]
% | uniq | tr -s \\n ','`
\typeout{Check comments in preamble of output for instructions how to show only pages where changes have been made}
\newcount\DIFcounterb
\global\DIFcounterb 0\relax
\newcount\DIFcountere
\global\DIFcountere 0\relax
\providecommand{\DIFaddbegin}{\global\advance\DIFcounterb 1\relax\label{DIFchgb\the\DIFcounterb}}
\providecommand{\DIFaddend}{\global\advance\DIFcountere 1\relax\label{DIFchge\the\DIFcountere}}
\providecommand{\DIFdelbegin}{\global\advance\DIFcounterb 1\relax\label{DIFchgb\the\DIFcounterb}}
\providecommand{\DIFdelend}{\global\advance\DIFcountere 1\relax\label{DIFchge\the\DIFcountere}}
\providecommand{\DIFmodbegin}{\global\advance\DIFcounterb 1\relax\label{DIFchgb\the\DIFcounterb}}
\providecommand{\DIFmodend}{\global\advance\DIFcountere 1\relax\label{DIFchge\the\DIFcountere}}"#;

const ZLABEL: &str = r#"% To show only pages with changes (pdf) (external program pdftk needs to be installed)
% (uses zref for reference to absolute page numbers)
% pdflatex diff.tex
% pdflatex diff.tex
%pdftk diff.pdf cat \
%`perl -lne 'if (m/\\zref\@newlabel{DIFchgb(\d*)}{.*\\abspage{(\d*)}}/ ) { $start{$1}=$2; print $2 } \
%  if (m/\\zref\@newlabel{DIFchge(\d*)}{.*\\abspage{(\d*)}}/) { \
%      if (defined($start{$1})) { \
%         for ($j=$start{$1}; $j<=$2; $j++) {print "$j";}\
%      } else { \
%         print "$2"\
%      }\
% }' diff.aux \
% | uniq \
% | tr  \\n ' '` \
% output diff-changedpages.pdf
% To show only pages with changes (dvips/dvipdf)
% latex diff.tex
% latex diff.tex
% dvips -pp `perl -lne 'if (m/\\newlabel{DIFchg[be]\d*}{{.*}{(.*)}}/) { print $1 }' diff.aux | uniq | tr -s \\n ','` diff.dvi
\typeout{Check comments in preamble of output for instructions how to show only pages where changes have been made}
\usepackage[user,abspage]{zref}
\newcount\DIFcounterb
\global\DIFcounterb 0\relax
\newcount\DIFcountere
\global\DIFcountere 0\relax
\providecommand{\DIFaddbegin}{\global\advance\DIFcounterb 1\relax\zlabel{DIFchgb\the\DIFcounterb}}
\providecommand{\DIFaddend}{\global\advance\DIFcountere 1\relax\zlabel{DIFchge\the\DIFcountere}}
\providecommand{\DIFdelbegin}{\global\advance\DIFcounterb 1\relax\zlabel{DIFchgb\the\DIFcounterb}}
\providecommand{\DIFdelend}{\global\advance\DIFcountere 1\relax\zlabel{DIFchge\the\DIFcountere}}
\providecommand{\DIFmodbegin}{\global\advance\DIFcounterb 1\relax\zlabel{DIFchgb\the\DIFcounterb}}
\providecommand{\DIFmodend}{\global\advance\DIFcountere 1\relax\zlabel{DIFchge\the\DIFcountere}}"#;

const ONLYCHANGEDPAGE: &str = r"\RequirePackage{atbegshi}
\RequirePackage{etoolbox}
\RequirePackage{zref}
% redefine label command to write immediately to aux file - page references will be lost
\makeatletter \let\oldlabel\label% Store \label
\renewcommand{\label}[1]{% Update \label to write to the .aux immediately
\zref@wrapper@immediate{\oldlabel{#1}}}
\makeatother
\newbool{DIFkeeppage}
\newbool{DIFchange}
\boolfalse{DIFkeeppage}
\boolfalse{DIFchange}
\AtBeginShipout{%
  \ifbool{DIFkeeppage}
        {\global\boolfalse{DIFkeeppage}}  % True DIFkeeppage
         {\ifbool{DIFchange}{\global\boolfalse{DIFkeeppage}}{\global\boolfalse{DIFkeeppage}\AtBeginShipoutDiscard}} % False DIFkeeppage
}
\providecommand{\DIFaddbegin}{\global\booltrue{DIFkeeppage}\global\booltrue{DIFchange}}
\providecommand{\DIFaddend}{\global\booltrue{DIFkeeppage}\global\boolfalse{DIFchange}}
\providecommand{\DIFdelbegin}{\global\booltrue{DIFkeeppage}\global\booltrue{DIFchange}}
\providecommand{\DIFdelend}{\global\booltrue{DIFkeeppage}\global\boolfalse{DIFchange}}
\providecommand{\DIFmodbegin}{\global\booltrue{DIFkeeppage}\global\booltrue{DIFchange}}
\providecommand{\DIFmodend}{\global\booltrue{DIFkeeppage}\global\boolfalse{DIFchange}}";

const FLOATSAFE: &str = r"\providecommand{\DIFaddFL}[1]{\DIFadd{#1}}
\providecommand{\DIFdelFL}[1]{\DIFdel{#1}}
\providecommand{\DIFaddbeginFL}{}
\providecommand{\DIFaddendFL}{}
\providecommand{\DIFdelbeginFL}{}
\providecommand{\DIFdelendFL}{}";

const IDENTICAL: &str = r"\providecommand{\DIFaddFL}[1]{\DIFadd{#1}}
\providecommand{\DIFdelFL}[1]{\DIFdel{#1}}
\providecommand{\DIFaddbeginFL}{\DIFaddbegin}
\providecommand{\DIFaddendFL}{\DIFaddend}
\providecommand{\DIFdelbeginFL}{\DIFdelbegin}
\providecommand{\DIFdelendFL}{\DIFdelend}";

const AMSMATHULEM: &str = r"\makeatletter
\let\sout@orig\sout
\renewcommand{\sout}[1]{\ifmmode\text{\sout@orig{\ensuremath{#1}}}\else\sout@orig{#1}\fi}
\makeatother";

const HYPERREF: &str = r"\providecommand{\DIFadd}[1]{\texorpdfstring{\DIFaddtex{#1}}{#1}}
\providecommand{\DIFdel}[1]{\texorpdfstring{\DIFdeltex{#1}}{}}";

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(body: &str) -> String {
        format!("\\documentclass{{article}}\n\\begin{{document}}\n{body}\\end{{document}}\n")
    }

    fn run(old: &str, new: &str) -> DiffOut {
        let mut o = BTreeMap::new();
        o.insert("main.tex".to_owned(), doc(old));
        let mut n = BTreeMap::new();
        n.insert("main.tex".to_owned(), doc(new));
        diff(&o, &n, "main.tex", &Options::default()).unwrap()
    }

    fn body(d: &DiffOut) -> &str {
        let s = d.tex.find("\\begin{document}\n").unwrap() + 17;
        let e = d.tex.rfind("\\end{document}").unwrap();
        &d.tex[s..e]
    }

    #[test]
    fn word_added() {
        let d = run("Hello world.\n", "Hello brave world.\n");
        assert_eq!(
            body(&d),
            "Hello \\DIFaddbegin \\DIFadd{brave }\\DIFaddend world.\n"
        );
        assert_eq!(d.changes.len(), 1);
        let c = &d.changes[0];
        assert_eq!(c.kind, ChangeKind::Add);
        assert_eq!(c.new_text, "brave");
        assert_eq!(c.new.file, "main.tex");
        assert_eq!(c.old.start, c.old.end);
    }

    #[test]
    fn text_command_inside() {
        let d = run("\\section{Intro}\nText.\n", "\\section{Overview}\nText.\n");
        assert_eq!(
            body(&d),
            "\\section{\\DIFdelbegin \\DIFdel{Intro}\\DIFdelend \\DIFaddbegin \\DIFadd{Overview}\\DIFaddend }\nText.\n"
        );
        assert_eq!(d.changes[0].section.as_deref(), Some("Overview"));
    }

    #[test]
    fn coarse_math() {
        let d = run(
            "See\n\\begin{equation}\nx = y^2 \\label{e}\n\\end{equation}\ndone.\n",
            "See\n\\begin{equation}\nx = y^3 \\label{e}\n\\end{equation}\ndone.\n",
        );
        let b = body(&d);
        assert!(
            b.contains("\\begin{displaymath}\\DIFdel{\nx = y^2 }%DIFDELCMD < \\label{e}"),
            "{b}"
        );
        assert!(
            b.contains("\\begin{equation}\\DIFadd{\nx = y^3 }\\label{e}"),
            "{b}"
        );
    }

    #[test]
    fn deleted_unsafe_commented() {
        let d = run("A \\foo{x} B.\n", "A B.\n");
        assert!(
            body(&d).contains("%DIFDELCMD < \\foo{x}\n%DIFDELCMD < %%%\n\\DIFdelend B."),
            "{}",
            body(&d)
        );
    }

    #[test]
    fn deleted_section_counter() {
        let d = run(
            "\\section{Gone}\nText.\n\n\\section{Kept}\n",
            "\\section{Kept}\n",
        );
        assert!(
            body(&d).contains("\\section{\\DIFdel{Gone}}\\addtocounter{section}{-1}"),
            "{}",
            body(&d)
        );
    }

    #[test]
    fn preamble_block() {
        let d = run("a\n", "b\n");
        assert!(
            d.tex
                .starts_with("\\documentclass{article}\n%DIF PREAMBLE EXTENSION")
        );
        assert!(d.tex.contains(
            "\\providecommand{\\DIFadd}[1]{{\\protect\\color{blue}\\uwave{#1}}} %DIF PREAMBLE\n"
        ));
    }

    #[test]
    fn types_by_name() {
        for m in Markup::ALL {
            assert_eq!(Markup::parse(&m.name().to_lowercase()), Some(m));
        }
        for t in Subtype::ALL {
            assert_eq!(Subtype::parse(t.name()), Some(t));
        }
        assert_eq!(Subtype::parse("marker"), Some(Subtype::Margin));
        assert_eq!(Markup::parse("LUAUNDERLINE"), None);
        assert!(Markup::Underline.ulem() && Markup::Culinechbar.ulem() && !Markup::Cfont.ulem());
        let o = Options {
            markup: Markup::Cchangebar,
            subtype: Subtype::Zlabel,
            driver: Driver::Xetex,
            ..Options::default()
        };
        assert_eq!(
            o.latexdiff_args()[..2],
            [
                "--type=CCHANGEBAR".to_owned(),
                "--subtype=ZLABEL".to_owned()
            ]
        );
        assert!(o.latexdiff_args().contains(&"--driver=xetex".to_owned()));
        let p = dif_preamble(&o, "");
        assert!(
            p.contains("\\RequirePackage[xetex]{changebar} %DIF PREAMBLE\n"),
            "{p}"
        );
        assert!(p.contains("\\zlabel{DIFchgb\\the\\DIFcounterb}"));
    }

    #[test]
    fn colors() {
        assert_eq!(Color::parse("#1a2B3c").unwrap().as_str(), "#1A2B3C");
        assert_eq!(Color::parse("1a2b3c").unwrap().as_str(), "#1A2B3C");
        assert_eq!(
            Color::parse(" blue!60!black ").unwrap().as_str(),
            "blue!60!black"
        );
        for bad in [
            "",
            "#12345",
            "#12345g",
            "red}\\input{x}",
            "re d",
            "a{b}",
            "!red",
            "x%y",
        ] {
            assert!(Color::parse(bad).is_err(), "{bad}");
        }
        let o = Options {
            add_color: Some(Color::parse("#008000").unwrap()),
            ..Options::default()
        };
        let p = dif_preamble(&o, "");
        let at = |s: &str| p.find(s).unwrap_or_else(|| panic!("{s} in {p}"));
        assert!(at("\\RequirePackage{xcolor}") < at("\\definecolor{DIFaddcolor}{HTML}{008000}"));
        assert!(at("\\colorlet{DIFdelcolor}{red}") < at("%DIF UNDERLINE PREAMBLE"));
        assert!(
            p.contains("\\providecommand{\\DIFadd}[1]{{\\protect\\color{DIFaddcolor}\\uwave{#1}}}")
        );
        assert!(
            p.contains("\\providecommand{\\DIFdel}[1]{{\\protect\\color{DIFdelcolor}\\sout{#1}}}")
        );
        assert!(!p.contains("\\RequirePackage{color}"));
        // (no colors: latexdiff's preamble as it is)
        let plain = dif_preamble(&Options::default(), "");
        assert!(!plain.contains("xcolor") && !plain.contains("DIFaddcolor"));
        // (the subtype colors too)
        let o = Options {
            markup: Markup::Traditional,
            subtype: Subtype::Color,
            del_color: Some(Color::parse("orange").unwrap()),
            ..Options::default()
        };
        let p = dif_preamble(&o, "");
        assert!(p.contains("\\providecommand{\\DIFdelbegin}{\\protect\\color{DIFdelcolor}}"));
        assert!(p.contains("\\providecommand{\\DIFdelFL}[1]{{\\protect\\color{DIFdelcolor}[..{\\scriptsize {removed: #1}} ]}}"));
    }
}
