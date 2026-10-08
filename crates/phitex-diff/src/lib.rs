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
pub mod myers;
pub mod sig;
pub mod tok;

mod emit;

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

pub use flatten::{Flat, flatten};
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

/// How changes look (latexdiff's `--type`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Markup {
    /// Added text wavy-underlined in blue, deleted struck out in red.
    #[default]
    Underline,
    /// Added text blue and sans serif, deleted red and small.
    Cfont,
}

/// How a changed range is set off (latexdiff's `--subtype`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Subtype {
    /// Not at all (the markup alone).
    #[default]
    Safe,
    /// The whole range colored: added blue, deleted red.
    Color,
}

/// What to diff, and how.
#[derive(Clone, Debug, Default)]
pub struct Options {
    pub markup: Markup,
    pub subtype: Subtype,
    /// Commands to take as safe, text (last argument diffed inside) or
    /// unsafe, over the table and the preambles (latexdiff's
    /// `--append-safecmd`, `--append-textcmd`, `--exclude-safecmd`).
    pub safe: Vec<String>,
    pub text: Vec<String>,
    pub unsafe_cmds: Vec<String>,
}

impl Options {
    /// The latexdiff command line that marks up as these options do (for
    /// comparisons).
    #[must_use]
    pub fn latexdiff_args(&self) -> Vec<String> {
        vec![
            format!(
                "--type={}",
                match self.markup {
                    Markup::Underline => "UNDERLINE",
                    Markup::Cfont => "CFONT",
                }
            ),
            format!(
                "--subtype={}",
                match self.subtype {
                    Subtype::Safe => "SAFE",
                    Subtype::Color => "COLOR",
                }
            ),
            "--math-markup=coarse".into(),
            "--flatten".into(),
        ]
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
        // (the table: latexdiff's, the new preamble's definitions, the
        // caller's; what only the old preamble defines is unsafe deleted)
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
        em.out.push_str(&new.text[..nb.begin]);
        em.out.push_str(&dif_preamble(&self.opts, preamble));
        em.out.push_str(&new.text[nb.begin..nb.start]);
        em.body(&old_toks, &new_toks, ob.end, nb.end);
        em.push(&new.text[nb.end..]);
        let (tex, raw) = em.finish();
        let changes = raw
            .iter()
            .map(|c| {
                let kind = match (c.old.0 < c.old.1, c.new.0 < c.new.1) {
                    (true, true) => ChangeKind::Change,
                    (true, false) => ChangeKind::Del,
                    _ => ChangeKind::Add,
                };
                let loc = |f: &Flat, r: (usize, usize)| {
                    let (file, start, end) = f.locate_range(r.0, r.1);
                    Loc {
                        file: f.files.get(file).cloned().unwrap_or_default(),
                        start,
                        end,
                    }
                };
                Change {
                    kind,
                    old: loc(&self.flat, c.old),
                    new: loc(new, c.new),
                    old_text: old_text[c.old.0..c.old.1].to_owned(),
                    new_text: new.text[c.new.0..c.new.1].to_owned(),
                    section: c.section.clone(),
                    out: c.out.0..c.out.1,
                }
            })
            .collect();
        DiffOut { tex, changes }
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
/// `%DIF PREAMBLE` as latexdiff tags them.
#[must_use]
pub fn dif_preamble(opts: &Options, preamble: &str) -> String {
    let hyperref = loads(preamble, &["hyperref"]);
    let amsmath = loads(preamble, &["amsmath", "mathtools"]);
    let (name, ty) = match opts.markup {
        Markup::Underline => ("UNDERLINE", UNDERLINE),
        Markup::Cfont => ("CFONT", CFONT),
    };
    let mut lines = vec![format!("%DIF {name} PREAMBLE")];
    for l in ty.lines() {
        // (with hyperref, the markup is \DIFaddtex, wrapped below)
        lines.push(if hyperref {
            l.replace("{\\DIFadd}", "{\\DIFaddtex}")
                .replace("{\\DIFdel}", "{\\DIFdeltex}")
        } else {
            l.to_owned()
        });
    }
    let (sname, sub) = match opts.subtype {
        Subtype::Safe => ("SAFE", SAFE),
        Subtype::Color => ("COLOR", COLOR),
    };
    lines.push(format!("%DIF {sname} PREAMBLE"));
    lines.extend(sub.lines().map(str::to_owned));
    lines.push("%DIF FLOATSAFE PREAMBLE".into());
    lines.extend(FLOATSAFE.lines().map(str::to_owned));
    if amsmath && opts.markup == Markup::Underline {
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

const CFONT: &str = r"\RequirePackage{color}\definecolor{RED}{rgb}{1,0,0}\definecolor{BLUE}{rgb}{0,0,1}
\DeclareOldFontCommand{\sf}{\normalfont\sffamily}{\mathsf}
\providecommand{\DIFadd}[1]{{\protect\color{blue} \sf #1}}
\providecommand{\DIFdel}[1]{{\protect\color{red} \scriptsize #1}}";

const SAFE: &str = r"\providecommand{\DIFaddbegin}{}
\providecommand{\DIFaddend}{}
\providecommand{\DIFdelbegin}{}
\providecommand{\DIFdelend}{}
\providecommand{\DIFmodbegin}{}
\providecommand{\DIFmodend}{}";

const COLOR: &str = r"\RequirePackage{color}
\providecommand{\DIFaddbegin}{\protect\color{blue}}
\providecommand{\DIFaddend}{\protect\color{black}}
\providecommand{\DIFdelbegin}{\protect\color{red}}
\providecommand{\DIFdelend}{\protect\color{black}}
\providecommand{\DIFmodbegin}{}
\providecommand{\DIFmodend}{}";

const FLOATSAFE: &str = r"\providecommand{\DIFaddFL}[1]{\DIFadd{#1}}
\providecommand{\DIFdelFL}[1]{\DIFdel{#1}}
\providecommand{\DIFaddbeginFL}{}
\providecommand{\DIFaddendFL}{}
\providecommand{\DIFdelbeginFL}{}
\providecommand{\DIFdelendFL}{}";

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
}
