//! A native latexdiff: two versions of a LaTeX project in, the new one
//! with the differences marked (latexdiff's markup, `\DIFadd{…}`,
//! `\DIFdel{…}`, …) and the list of changes out.
//!
//! - Both versions are flattened (`\input`, `\include`, as latexdiff
//!   `--flatten`), each following its own inputs: a file the new version no
//!   longer inputs is deleted text where it was, a new one is added text.
//! - The preamble is the new version's, unmarked, with latexdiff's
//!   definitions before `\begin{document}`.
//! - The bodies are read from their CST ([`phitex_source`]) into tokens
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
mod look;

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

pub use flatten::{Flat, flatten};
pub use live::{Live, Place, Stats, changes_json, places};
pub use look::{dif_body, dif_preamble};
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

    /// Whether it underlines or strikes out (latexdiff's by ulem; ours by
    /// ulem's code, [`look`]).
    #[must_use]
    pub fn ulem(self) -> bool {
        matches!(
            self,
            Markup::Underline | Markup::Fontstrike | Markup::Culinechbar
        )
    }

    /// Whether it shows deleted text (not: change bars only, added text
    /// alone, bold).
    #[must_use]
    pub fn shows_deleted(self) -> bool {
        !matches!(self, Markup::Changebar | Markup::Invisible | Markup::Bold)
    }

    /// Whether it colours changes (the blocks', math's and pictures'
    /// colours: else they are black, told apart by their frames).
    #[must_use]
    pub fn colored(self) -> bool {
        matches!(
            self,
            Markup::Underline
                | Markup::Ctraditional
                | Markup::Cfont
                | Markup::Cchangebar
                | Markup::Cfontchbar
                | Markup::Culinechbar
        )
    }

    /// Whether it draws change bars (changebar).
    #[must_use]
    pub fn bars(self) -> bool {
        matches!(
            self,
            Markup::Cchangebar | Markup::Cfontchbar | Markup::Culinechbar | Markup::Changebar
        )
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

    /// Its red, green and blue, each from 0 to 1: a hex color's, or an
    /// xcolor expression's of xcolor's base colors (`blue!60!black`: 60%
    /// blue, the rest black; `red!50`: the rest white). None for another
    /// name.
    #[must_use]
    pub fn rgb(&self) -> Option<(f64, f64, f64)> {
        if let Some(hex) = self.0.strip_prefix('#') {
            let c = |k: usize| {
                u8::from_str_radix(hex.get(k..k + 2)?, 16)
                    .ok()
                    .map(|x| f64::from(x) / 255.0)
            };
            return Some((c(0)?, c(2)?, c(4)?));
        }
        let base = |n: &str| -> Option<(f64, f64, f64)> {
            Some(match n {
                "red" => (1.0, 0.0, 0.0),
                "green" => (0.0, 1.0, 0.0),
                "blue" => (0.0, 0.0, 1.0),
                "cyan" => (0.0, 1.0, 1.0),
                "magenta" => (1.0, 0.0, 1.0),
                "yellow" => (1.0, 1.0, 0.0),
                "black" => (0.0, 0.0, 0.0),
                "white" => (1.0, 1.0, 1.0),
                "gray" => (0.5, 0.5, 0.5),
                "darkgray" => (0.25, 0.25, 0.25),
                "lightgray" => (0.75, 0.75, 0.75),
                "brown" => (0.75, 0.5, 0.25),
                "lime" => (0.75, 1.0, 0.0),
                "olive" => (0.5, 0.5, 0.0),
                "orange" => (1.0, 0.5, 0.0),
                "pink" => (1.0, 0.75, 0.75),
                "purple" => (0.75, 0.0, 0.25),
                "teal" => (0.0, 0.5, 0.5),
                "violet" => (0.5, 0.0, 0.5),
                _ => return None,
            })
        };
        let mut parts = self.0.split('!');
        let mut c = base(parts.next()?)?;
        while let Some(p) = parts.next() {
            let p: f64 = p.parse().ok().filter(|p: &f64| (0.0..=100.0).contains(p))?;
            let other = match parts.next() {
                Some(n) => base(n)?,
                None => (1.0, 1.0, 1.0),
            };
            let mix = |a: f64, b: f64| a * p / 100.0 + b * (1.0 - p / 100.0);
            c = (mix(c.0, other.0), mix(c.1, other.1), mix(c.2, other.2));
        }
        Some(c)
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

/// The diff: the marked-up, flattened document, the changes, in document
/// order, and the map from the document back to the user's files.
#[derive(Clone, Debug, Default)]
pub struct DiffOut {
    pub tex: String,
    pub changes: Vec<Change>,
    /// The new version's files, the main one first ([`MapRun::file`]).
    pub files: Vec<String>,
    /// The runs of `tex` that are the new version's text as it is, in
    /// order: what a place on the diff's pages is in the user's files
    /// ([`DiffOut::source`]).
    pub map: Vec<MapRun>,
    /// `tex` split back into the new version's files ([`split`]), none
    /// where it can't be (a file read twice, a cut inside a token).
    pub split: Option<Split>,
}

/// The marked-up text as the new version's files: each file its own
/// marked-up text, its `\input` commands kept, so that a build of the
/// document reads the marked-up files where it read the files (an edit
/// of the files changed, the others read as they are).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Split {
    /// Each file's marked-up text, in [`DiffOut::files`]' order.
    pub texts: Vec<String>,
    /// Each file's runs that are the flat marked-up text's
    /// ([`DiffOut::tex`]): (where in the file's text, length, where in
    /// `tex`), in order.
    pub runs: Vec<Vec<(usize, usize, usize)>>,
}

impl Split {
    /// Where offset `at` of file `file`'s marked-up text is in the flat
    /// marked-up text, if it is there (not an `\input` command, nor a
    /// markup command closed or opened again at a cut).
    #[must_use]
    pub fn flat_of(&self, file: usize, at: usize) -> Option<usize> {
        let runs = self.runs.get(file)?;
        let i = runs.partition_point(|r| r.0 + r.1 <= at);
        let r = runs.get(i)?;
        (r.0 <= at).then(|| r.2 + (at - r.0))
    }
}

/// The runs of the marked-up text before the body that are the new
/// text's: the preamble, then (after the `pre` bytes of packages it
/// loads) `\begin{document}`.
fn head_spans(nb: Body, pre: usize) -> [(usize, usize, usize); 2] {
    [
        (0, nb.begin, 0),
        (nb.begin + pre, nb.start - nb.begin, nb.begin),
    ]
}

/// `tex`, the marked-up flat text of `flat`, split back into `flat`'s
/// files ([`Split`]): each file's runs of the flat text marked up, the
/// rest of its text (its `\input` commands, its last line end) as it is.
/// `spans` are the runs of `tex` that are the flat text as it is (where in
/// `tex`, length, where in the flat text), `adds` the places of the added
/// tokens written in the markup ([`emit::Em::adds`]), both in order. A cut
/// between two files inside a run of added text closes its markup at the
/// end of the one and opens it again in the other.
///
/// None where a file's runs are not in order (a file read twice), a cut
/// is inside a token written as markup, or the bytes between two files'
/// runs are not the flat text's own.
#[must_use]
pub fn split(
    tex: &str,
    flat: &Flat,
    spans: &[(usize, usize, usize)],
    adds: &[(usize, usize, Option<&'static str>)],
) -> Option<Split> {
    // (`PHITEX_SPLIT_DEBUG`: why a split failed, on stderr)
    macro_rules! fail {
        ($($a:tt)*) => {{
            if std::env::var_os("PHITEX_SPLIT_DEBUG").is_some() {
                eprintln!("split: {}", format!($($a)*));
            }
            return None;
        }};
    }
    let n = flat.files.len();
    if flat.sources.len() != n {
        fail!("no sources");
    }
    let spans: Vec<_> = spans.iter().copied().filter(|s| s.1 > 0).collect();
    let cut = |b: usize| -> Option<(usize, Option<&'static str>)> {
        let i = spans.partition_point(|s| s.2 + s.1 <= b);
        if let Some(s) = spans.get(i)
            && s.2 <= b
        {
            return Some((s.0 + (b - s.2), None));
        }
        let k = adds.partition_point(|a| a.0 < b);
        if let Some(a) = adds.get(k)
            && a.0 == b
        {
            return Some((a.1, a.2));
        }
        // (the text's end)
        let last = spans.last()?;
        (b == flat.text.len() && last.2 + last.1 == b).then_some((last.0 + last.1, None))
    };
    let mut texts = vec![String::new(); n];
    let mut runs = vec![Vec::new(); n];
    let mut prev = vec![0usize; n];
    let (mut out_end, mut flat_end, mut open, mut last_file) = (0usize, 0usize, None, 0usize);
    let near = |b: usize| {
        flat.text
            .get(b.saturating_sub(20)..(b + 20).min(flat.text.len()))
            .unwrap_or("")
    };
    for s in &flat.segs {
        let Some((a, ao)) = cut(s.flat) else {
            fail!("no cut at {} ({:?})", s.flat, near(s.flat));
        };
        let Some((b, bo)) = cut(s.flat + s.len) else {
            fail!("no cut at {} ({:?})", s.flat + s.len, near(s.flat + s.len));
        };
        if a < out_end
            || b < a
            || ao != open
            || tex.get(out_end..a) != flat.text.get(flat_end..s.flat)
        {
            fail!(
                "a gap at {flat_end}..{}: {:?} for {:?}",
                s.flat,
                tex.get(out_end..a),
                flat.text.get(flat_end..s.flat)
            );
        }
        let src = flat.sources.get(s.file)?;
        if s.src < prev[s.file] {
            fail!("{} read twice", flat.files[s.file]);
        }
        let t = &mut texts[s.file];
        t.push_str(src.get(prev[s.file]..s.src)?);
        if let Some(c) = ao {
            t.push('\\');
            t.push_str(c);
            t.push('{');
        }
        runs[s.file].push((t.len(), b - a, a));
        t.push_str(&tex[a..b]);
        if bo.is_some() {
            t.push('}');
        }
        prev[s.file] = s.src + s.len;
        (out_end, flat_end, open, last_file) = (b, s.flat + s.len, bo, s.file);
    }
    // (after the last run: the flat text's, and a line end owed after a
    // comment written last)
    let (rest, flat_rest) = (tex.get(out_end..)?, flat.text.get(flat_end..)?);
    if open.is_some() {
        fail!("the markup open at the end");
    }
    if rest != flat_rest {
        if rest.strip_suffix('\n') != Some(flat_rest) || !flat_rest.is_empty() {
            fail!("the end: {rest:?} for {flat_rest:?}");
        }
        texts[last_file].push('\n');
    }
    for (k, t) in texts.iter_mut().enumerate() {
        t.push_str(flat.sources[k].get(prev[k]..)?);
    }
    Some(Split { texts, runs })
}

/// A run of the diff's text that is the new version's: `len` bytes at
/// `out` in it are those at `src` in file `file` ([`DiffOut::files`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MapRun {
    pub out: usize,
    pub len: usize,
    pub file: usize,
    pub src: usize,
}

impl DiffOut {
    /// Where offset `out` of the diff's text is in the user's files (the
    /// viewer's follow, selection and double-click on the diff's pages):
    /// in a run copied as it is, its place there; in a change's markup
    /// (added text, a deleted passage), where the change is in the new
    /// version; elsewhere (latexdiff's own markup), the end of the run
    /// before it. None before the first run (the preamble's head).
    #[must_use]
    pub fn source(&self, out: usize) -> Option<(&str, usize)> {
        let i = self.map.partition_point(|r| r.out <= out);
        if let Some(r) = i.checked_sub(1).map(|i| &self.map[i])
            && out < r.out + r.len
        {
            return Some((self.files.get(r.file)?.as_str(), r.src + (out - r.out)));
        }
        if let Some(c) = self.changes.iter().find(|c| c.out.contains(&out)) {
            return Some((c.new.file.as_str(), c.new.start));
        }
        let r = &self.map[i.checked_sub(1)?];
        Some((self.files.get(r.file)?.as_str(), r.src + r.len))
    }
}

/// `spans` (where in the diff's text, length, where in `flat`'s text) as
/// runs of the user's files, split where `flat`'s files change.
pub(crate) fn map_runs(spans: &[(usize, usize, usize)], flat: &Flat) -> Vec<MapRun> {
    let mut out = Vec::new();
    for &(o, len, f) in spans {
        let mut done = 0;
        while done < len {
            let i = flat.segs.partition_point(|s| s.flat <= f + done);
            let Some(s) = i.checked_sub(1).map(|i| &flat.segs[i]) else {
                break;
            };
            let off = f + done - s.flat;
            if off >= s.len {
                // (bytes between runs, from no file)
                done += 1;
                continue;
            }
            let n = (s.len - off).min(len - done);
            out.push(MapRun {
                out: o + done,
                len: n,
                file: s.file,
                src: s.src + off,
            });
            done += n;
        }
    }
    out
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

/// `text`, a main file's, with the markup's definitions for `opts` put
/// where the diff's text has them ([`Baseline::diff`]: packages at the
/// preamble's end, the rest after `\begin{document}`) and nothing else
/// changed: the document as it is, ready for the markup. A build of the
/// document given this first, then the diff's text, runs the definitions
/// alone first, then only the paragraphs the markup changed. None without
/// `\begin{document}` in it.
#[must_use]
pub fn with_definitions(text: &str, opts: &Options) -> Option<String> {
    let b = find_body(text);
    if b.start == 0 {
        return None;
    }
    let pre = &text[..b.begin];
    Some(format!(
        "{pre}{}{}{}{}",
        dif_preamble(opts, pre),
        &text[b.begin..b.start],
        dif_body(opts, pre),
        &text[b.start..]
    ))
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
    use phitex_source::{SyntaxKind as K, Tree};
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
        em.show_del = self.opts.markup.shows_deleted();
        let (h, head) = self.head(new, nb);
        em.out.push_str(&h);
        em.body(&old_toks, &new_toks, ob.end, nb.end);
        em.push(&new.text[nb.end..]);
        let spans = std::mem::take(&mut em.spans);
        let adds = std::mem::take(&mut em.adds);
        let (tex, raw) = em.finish();
        let flat_spans: Vec<_> = head.iter().chain(&spans).copied().collect();
        let split = split(&tex, new, &flat_spans, &adds);
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
        DiffOut {
            tex,
            changes,
            files: new.files.clone(),
            map: map_runs(&spans, new),
            split,
        }
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

    /// The output before the body: the new preamble, the packages the
    /// markup needs at its end (none for most looks), `\begin{document}`,
    /// the markup's definitions after it ([`look`]: in the body, not the
    /// preamble, so that the document's build sees no preamble changed);
    /// and its runs that are the new text's.
    fn head(&self, new: &Flat, nb: Body) -> (String, [(usize, usize, usize); 2]) {
        let preamble = &new.text[..nb.begin];
        let mut s = String::from(preamble);
        let pre = dif_preamble(&self.opts, preamble);
        s.push_str(&pre);
        s.push_str(&new.text[nb.begin..nb.start]);
        // (whatever the body's first line: the live diff keeps the head
        // while the body is edited; a blank line made after it is a `\par`
        // in vertical mode, nothing)
        s.push_str(&look::dif_body(&self.opts, preamble));
        (s, head_spans(nb, pre.len()))
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
        // (a change across two files, a deleted `\include` after an edited
        // file's end: placed in the file it begins in, its text that
        // file's part)
        let old = locate(&self.flat, c.old);
        let old_len = (old.end - old.start).min(c.old.1 - c.old.0);
        let mut new_text = new_text;
        new_text.truncate((new.end - new.start).min(new_text.len()));
        Change {
            kind,
            old,
            new,
            old_text: self.flat.text[c.old.0..c.old.0 + old_len].to_owned(),
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

    fn body(d: &DiffOut) -> String {
        // (the body after the markup's definitions, and the line end
        // after `\begin{document}`)
        let end = "%DIF END PREAMBLE EXTENSION ADDED BY LATEXDIFF\n\n";
        let s = d.tex.find(end).unwrap() + end.len();
        let e = d.tex.rfind("\\end{document}").unwrap();
        d.tex[s..e].to_owned()
    }

    /// The diff's text back to the user's files (the viewer's follow,
    /// selection and double-click on the diff's pages): unchanged text to
    /// its very place, in an `\input` file too; added text to its
    /// change's place; the live diff's map the one-shot diff's.
    #[test]
    fn the_diff_maps_back_to_the_files() {
        let main = |body: &str| {
            format!(
                "\\documentclass{{article}}\n\\begin{{document}}\n{body}\n\n\\input{{ch}}\n\\end{{document}}\n"
            )
        };
        let mut o = BTreeMap::new();
        o.insert("main.tex".to_owned(), main("Hello world."));
        o.insert("ch.tex".to_owned(), "Chapter text here.\n".to_owned());
        let mut n = o.clone();
        n.insert("main.tex".to_owned(), main("Hello brave world."));
        let d = diff(&o, &n, "main.tex", &Options::default()).unwrap();
        let at = |needle: &str| d.tex.find(needle).unwrap();
        let new_main = &n["main.tex"];
        assert_eq!(
            d.source(at("world")),
            Some(("main.tex", new_main.find("world").unwrap()))
        );
        assert_eq!(d.source(at("text here")), Some(("ch.tex", 8)));
        assert_eq!(
            d.source(at("brave")),
            Some(("main.tex", new_main.find("brave").unwrap()))
        );
        let mut live = live::Live::new(Baseline::new(&o, "main.tex", &Options::default()).unwrap());
        let l = live.update(&flatten::flatten(&n, "main.tex").unwrap());
        assert_eq!(l.tex, d.tex);
        for needle in ["world", "text here", "brave", "Hello"] {
            assert_eq!(l.source(at(needle)), d.source(at(needle)), "{needle}");
        }
    }

    #[test]
    fn word_added() {
        let d = run("Hello world.\n", "Hello brave world.\n");
        assert_eq!(
            body(&d),
            "Hello \\DIFaddbegin \\DIFadd{brave} \\DIFaddend world.\n"
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
            "\\section{\\DIFdelbegin \\DIFdel{Intro}\\DIFdelend \\DIFchangesep \\DIFaddbegin \\DIFadd{Overview}\\DIFaddend }\nText.\n"
        );
        assert_eq!(d.changes[0].section.as_deref(), Some("Overview"));
    }

    #[test]
    fn coarse_math() {
        let d = run(
            "See\n\\begin{equation}\nx = y^2 \\label{e}\n\\end{equation}\ndone.\n",
            "See\n\\begin{equation}\nx = y^3 \\label{e}\n\\end{equation}\ndone.\n",
        );
        // (math whole, never cut by markup: the old one unnumbered, its
        // label dropped, coloured; the new one as it is, coloured)
        let b = body(&d);
        assert!(
            b.contains("\\begin{displaymath}\\DIFmathdelbegin \nx = y^2 \n\\DIFmathdelend \\end{displaymath}"),
            "{b}"
        );
        assert!(
            b.contains("\\begin{equation}\\DIFmathaddbegin \nx = y^3 \\label{e}\n\\DIFmathaddend \\end{equation}"),
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
            d.tex.starts_with(
                "\\documentclass{article}\n\\begin{document}%\n%DIF PREAMBLE EXTENSION"
            ),
            "{}",
            d.tex
        );
        assert!(d.tex.contains(
            "\\providecommand{\\DIFadd}[1]{\\phitexdifmark{0 0 1}{\\phitexdifuline{#1}}} %DIF PREAMBLE\n"
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
        let p = dif_preamble(&o, "") + &dif_body(&o, "");
        assert!(
            p.contains("\\RequirePackage[xetex]{changebar} %DIF PREAMBLE\n"),
            "{p}"
        );
        assert!(p.contains("\\zlabel{DIFchgb\\phitexdifcountb}"), "{p}");
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
        let p = dif_preamble(&o, "") + &dif_body(&o, "");
        assert!(
            p.contains(
                "\\providecommand{\\DIFadd}[1]{\\phitexdifmark{0 0.502 0}{\\phitexdifuline{#1}}}"
            ),
            "{p}"
        );
        assert!(p.contains(
            "\\providecommand{\\DIFdel}[1]{\\phitexdifmark{1 0 0}{\\phitexdifsout{#1}}}"
        ));
        assert!(!p.contains("color}") && !p.contains("xcolor"), "{p}");
        // (the subtype colors too)
        let o = Options {
            markup: Markup::Traditional,
            subtype: Subtype::Color,
            del_color: Some(Color::parse("orange").unwrap()),
            ..Options::default()
        };
        let p = dif_preamble(&o, "") + &dif_body(&o, "");
        assert!(
            p.contains("\\providecommand{\\DIFdelbegin}{\\phitexdifpush{1 0.5 0}}"),
            "{p}"
        );
        assert!(
            p.contains("\\providecommand{\\DIFdelFL}[1]{{[..{\\scriptsize {removed: #1}} ]}}"),
            "{p}"
        );
        // (change bars in the added text's colour)
        let o = Options {
            markup: Markup::Culinechbar,
            add_color: Some(Color::parse("#008000").unwrap()),
            ..Options::default()
        };
        let p = dif_preamble(&o, "") + &dif_body(&o, "");
        assert!(
            p.contains("\\definecolor{DIFaddcolor}{HTML}{008000}\\cbcolor{DIFaddcolor}"),
            "{p}"
        );
    }
}
