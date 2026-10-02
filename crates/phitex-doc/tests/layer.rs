//! The static layer on small documents: each construct, nested files,
//! verbatim, the document's macros, guards, numbers, and edits (each
//! checked against a fresh reading).

use std::rc::Rc;

use phitex_doc::{Confirmed, EnvProblem, GuardState, Meant, Memory, Project, Views};

fn open(files: &[(&str, &str)]) -> Project {
    Project::open(files[0].0, Memory::new(files))
}

fn labels<'a>(v: &'a Views<'_>) -> Vec<&'a str> {
    v.labels.iter().map(|l| l.key).collect()
}

fn titles<'a>(v: &'a Views<'_>) -> Vec<(i8, Option<&'a str>, &'a str)> {
    v.outline
        .iter()
        .map(|h| (h.level, h.number.as_deref(), h.title))
        .collect()
}

/// Replace the first `from` in `path` with `to`, and check the project
/// against a fresh reading.
fn edit(p: &mut Project, path: &str, from: &str, to: &str) {
    let id = p.file_id(path).expect("a loaded file");
    let at = p.text(id).find(from).expect("the text to edit");
    p.edit(id, at, from.len(), to);
    p.check().expect("the same facts as a fresh reading");
}

#[test]
fn constructs() {
    let p = open(&[(
        "main.tex",
        "\\documentclass{article}\n\
         \\usepackage{amsmath,cleveref}\n\
         \\begin{document}\n\
         \\section{Intro}\\label{sec:intro}\n\
         See \\ref{sec:intro}, \\eqref{eq:a}, \\cref{sec:intro,sec:two} and\n\
         \\pageref*{sec:two}; \\autoref{nowhere}.\n\
         \\begin{equation}\\label{eq:a} x \\end{equation}\n\n\
         \\section*[Short]{Long \\emph{title}}\n\
         \\subsection[Sh]{Two}\\label{sec:two}\n\
         \\citep[p.~3]{knuth,lamport} \\nocite{*} \\parencites[a]{x}[b][c]{y,z}\n\
         \\includegraphics[width=1cm]{fig}\n\
         \\end{document}\n\
         \\section{After the end}\n",
    )]);
    let v = p.views();
    assert_eq!(
        titles(&v),
        [
            (1, Some("1"), "Intro"),
            (1, None, "Long \\emph{title}"),
            (2, Some("1.1"), "Two")
        ]
    );
    assert_eq!(v.outline[1].short, Some("Short"));
    assert_eq!(v.outline[0].label, Some("sec:intro"));
    assert_eq!(v.outline[2].label, Some("sec:two"));
    assert_eq!(labels(&v), ["sec:intro", "eq:a", "sec:two"]);
    let refs: Vec<&str> = v.refs.iter().map(|r| r.key).collect();
    assert_eq!(
        refs,
        [
            "sec:intro",
            "eq:a",
            "sec:intro",
            "sec:two",
            "sec:two",
            "nowhere"
        ]
    );
    let unresolved: Vec<&str> = v.unresolved().map(|r| r.key).collect();
    assert_eq!(unresolved, ["nowhere"]);
    let cites: Vec<&str> = v.cites.iter().map(|c| c.key).collect();
    assert_eq!(cites, ["knuth", "lamport", "*", "x", "y", "z"]);
    assert!(v.nocite_all);
    assert_eq!(v.assets.len(), 1);
    assert!(!v.assets[0].found);
    assert_eq!(v.class, Some("article"));
    assert_eq!(v.envs, []);
    // (lines)
    assert_eq!(v.outline[2].loc.line, 10);
    assert_eq!(v.labels[1].loc.line, 7);
}

#[test]
fn files_and_paths() {
    let p = open(&[
        (
            "book.tex",
            "\\input{parts/a}\n\\include{b}\n\\input c.tex\n\\InputIfFileExists{d}{\\label{d-there}}{\\label{d-not}}\n\\IfFileExists{b.tex}{\\label{b-there}}{}\n\\input{missing}\n",
        ),
        ("parts/a.tex", "\\section{A}\\label{a}\n\\input{parts/aa}\n"),
        ("parts/aa.tex", "\\subsection{AA}\n"),
        ("b.tex", "\\section{B}\n"),
        ("c.tex", "\\section{C}\\input{book}\n"),
    ]);
    let v = p.views();
    let files: Vec<(usize, &str, bool)> = v
        .files
        .iter()
        .map(|f| (f.depth, f.path.as_str(), f.found.is_some()))
        .collect();
    assert_eq!(
        files,
        [
            (0, "book.tex", true),
            (1, "parts/a.tex", true),
            (2, "parts/aa.tex", true),
            (1, "b.tex", true),
            (1, "c.tex", true),
            (2, "book.tex", true),
            // (`d` does not exist: \InputIfFileExists reads its else branch)
            (1, "missing.tex", false),
        ]
    );
    // (an \input of a file being read is a cycle, not read again)
    assert!(v.files[5].cycle);
    assert_eq!(labels(&v), ["a", "d-not", "b-there"]);
    assert_eq!(
        titles(&v),
        [
            (1, Some("1"), "A"),
            (2, Some("1.1"), "AA"),
            (1, Some("2"), "B"),
            (1, Some("3"), "C")
        ]
    );
}

#[test]
fn verbatim_is_not_read() {
    let p = open(&[(
        "v.tex",
        "\\section{V}\n\\verb|\\label{no1}| \\url{http://x.org/%7E}\n\
         \\begin{verbatim}\n\\label{no2}\n\n\\section{No}\n\\end{verbatim}\n\
         \\begin{lstlisting}[caption={A},label=lst:yes]\n\\label{no3} {\n\\end{lstlisting}\n\
         \\begin{comment}\n\\label{no4}\n\\end{comment}\n\
         % \\label{no5}\n\
         \\iffalse \\label{no6}\n\n\\section{No} \\fi \\label{yes}\n\
         \\iffalse a \\else \\label{else} \\fi\n",
    )]);
    let v = p.views();
    assert_eq!(labels(&v), ["lst:yes", "yes", "else"]);
    assert_eq!(titles(&v), [(1, Some("1"), "V")]);
    assert_eq!(v.envs, []);
}

#[test]
fn environments_that_do_not_nest() {
    let p = open(&[(
        "e.tex",
        "\\begin{document}\n\\begin{figure}\n\\begin{center}\n\\end{figure}\n\\end{itemize}\n\\begin{table}\n",
    )]);
    let v = p.views();
    let line = |e: &EnvProblem<'_>| match e {
        EnvProblem::Unclosed { loc, .. }
        | EnvProblem::Unopened { loc, .. }
        | EnvProblem::Mismatch { loc, .. } => loc.line,
    };
    // (those never ended: the innermost first)
    assert_eq!(v.envs.iter().map(line).collect::<Vec<_>>(), [4, 5, 6, 1]);
    assert!(matches!(
        v.envs[0],
        EnvProblem::Mismatch {
            name: "figure",
            open: "center",
            ..
        }
    ));
    assert!(matches!(
        v.envs[1],
        EnvProblem::Unopened {
            name: "itemize",
            ..
        }
    ));
    assert!(matches!(
        v.envs[2],
        EnvProblem::Unclosed { name: "table", .. }
    ));
}

/// The course's pattern: chapters reached only through a macro of the
/// document's, numbered by `\setcounter` in it.
#[test]
fn macros_that_make_structure() {
    let mut p = open(&[
        (
            "course.tex",
            "\\documentclass{book}\n\\input{pre}\n\\begin{document}\n\\mainmatter\n\
             \\newcommand{\\chapterfile}[2]{\\IfFileExists{#2.tex}{\\setcounter{chapter}{\\numexpr#1-1\\relax}\\input{#2}}{}}\n\n\
             \\chapterfile{0}{ch0}\n\\part{One}\n\\chapterfile{1}{ch1}\n\\chapterfile{2}{ch2}\n\\chapterfile{5}{ch5}\n\\end{document}\n",
        ),
        (
            "pre.tex",
            "\\newcommand{\\chref}[1]{Chapter~\\ref{#1}}\n\\newcommand{\\doc}[1]{\\textsf{#1}}\n\\let\\sect\\section\n",
        ),
        (
            "ch0.tex",
            "\\chapter{Map}\\label{ch:map}\nSee \\chref{ch:one}.\n",
        ),
        (
            "ch1.tex",
            "\\chapter{One}\\label{ch:one}\n\\sect{The \\doc{x}}\n",
        ),
        ("ch5.tex", "\\chapter{Five}\n"),
    ]);
    let v = p.views();
    assert_eq!(
        titles(&v),
        [
            (0, Some("0"), "Map"),
            (-1, Some("I"), "One"),
            (0, Some("1"), "One"),
            (1, Some("1.1"), "The \\doc{x}"),
            (0, Some("5"), "Five"),
        ]
    );
    let refs: Vec<(&str, Option<&[phitex_doc::Name]>)> = v
        .refs
        .iter()
        .map(|r| (r.key, r.fact.via.as_deref()))
        .collect();
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0].0, "ch:one");
    assert_eq!(&*refs[0].1.unwrap()[0], "chref");
    // (the file graph says through which macro)
    assert_eq!(v.files.len(), 5);
    assert!(
        v.files
            .iter()
            .all(|f| f.path != "ch2.tex" || f.found.is_none())
    );
    // (the toc as LaTeX writes it: the document's macros expanded)
    assert_eq!(p.written_title(v.outline[3].title), "The \\textsf {x}");
    drop(v);

    // (an edit to the macro's definition reads its callers again)
    edit(
        &mut p,
        "pre.tex",
        "Chapter~\\ref{#1}",
        "Chapter~\\pageref{#1}",
    );
    let v = p.views();
    assert_eq!(v.refs[0].fact.cs, "pageref");
    drop(v);
    // (a definition removed: its calls are calls of nothing)
    edit(
        &mut p,
        "pre.tex",
        "\\newcommand{\\chref}[1]{Chapter~\\pageref{#1}}\n",
        "",
    );
    assert!(p.views().refs.is_empty());
    // (and added back, in another file, after its uses)
    edit(
        &mut p,
        "ch5.tex",
        "\\chapter{Five}\n",
        "\\chapter{Five}\n\\newcommand{\\chref}[1]{\\ref{#1}}\n",
    );
    assert_eq!(p.views().refs.len(), 1);
    // (a file the macro now names is loaded)
    edit(
        &mut p,
        "course.tex",
        "\\chapterfile{5}{ch5}",
        "\\chapterfile{5}{ch5}\\input{late}",
    );
    assert!(
        p.views()
            .files
            .iter()
            .any(|f| f.path == "late.tex" && f.found.is_none())
    );
}

#[test]
fn edits_add_and_remove_a_section_and_a_label() {
    let mut p = open(&[(
        "a.tex",
        "\\documentclass{article}\n\\begin{document}\n\\section{One}\\label{one}\nText \\ref{two}.\n\n\\section{Three}\nMore.\n\n\\end{document}\n",
    )]);
    assert_eq!(p.views().unresolved().count(), 1);
    edit(
        &mut p,
        "a.tex",
        "\\section{Three}",
        "\\section{Two}\\label{two}\n\n\\section{Three}",
    );
    let v = p.views();
    assert_eq!(
        titles(&v),
        [
            (1, Some("1"), "One"),
            (1, Some("2"), "Two"),
            (1, Some("3"), "Three")
        ]
    );
    assert_eq!(v.unresolved().count(), 0);
    drop(v);
    // (a label defined twice)
    edit(&mut p, "a.tex", "More.", "More.\\label{one}");
    let v = p.views();
    let dups = v.duplicates();
    assert_eq!(dups.len(), 1);
    assert_eq!(dups[0].0, "one");
    drop(v);
    edit(&mut p, "a.tex", "\\section{Two}\\label{two}\n\n", "");
    let v = p.views();
    assert_eq!(titles(&v), [(1, Some("1"), "One"), (1, Some("2"), "Three")]);
    assert_eq!(v.unresolved().count(), 1);
    drop(v);
    // (an \iffalse opened: the paragraphs after it are read again)
    edit(&mut p, "a.tex", "Text", "\\iffalse Text");
    assert_eq!(titles(&p.views()).len(), 1);
    edit(&mut p, "a.tex", "More.", "More.\\fi");
    // (the label after the \fi is read)
    assert_eq!(p.views().labels.len(), 2);
    edit(&mut p, "a.tex", "\\iffalse ", "");
    assert_eq!(titles(&p.views()).len(), 2);
    // (a verbatim run opened and closed across paragraphs)
    edit(&mut p, "a.tex", "Text", "\\begin{verbatim}Text");
    assert_eq!(titles(&p.views()).len(), 1);
    edit(
        &mut p,
        "a.tex",
        "\\end{document}",
        "\\end{verbatim}\\end{document}",
    );
    assert_eq!(titles(&p.views()).len(), 1);
    edit(&mut p, "a.tex", "\\begin{verbatim}", "");
    assert_eq!(titles(&p.views()).len(), 2);
}

#[test]
fn numbers() {
    let p = open(&[(
        "b.tex",
        "\\documentclass{book}\n\\begin{document}\n\\frontmatter\n\\chapter{Preface}\n\\mainmatter\n\
         \\part{P}\n\\chapter{A}\n\\section{A1}\n\\subsection{A11}\n\\subsubsection{A111}\n\\paragraph{Pa}\n\
         \\setcounter{secnumdepth}{3}\n\\subsubsection{A112}\n\\chapter*{Star}\n\\addcontentsline{toc}{chapter}{Star}\n\
         \\appendix\n\\chapter{X}\n\\section{X1}\n\\backmatter\n\\chapter{Index}\n\\end{document}\n",
    )]);
    let v = p.views();
    let numbers: Vec<Option<&str>> = v.outline.iter().map(|h| h.number.as_deref()).collect();
    assert_eq!(
        numbers,
        [
            None,
            Some("I"),
            Some("1"),
            Some("1.1"),
            Some("1.1.1"),
            None,
            None,
            Some("1.1.1.1"),
            None,
            Some("A"),
            Some("A.1"),
            None
        ]
    );
    // (the toc: the starred chapter by \addcontentsline, not by itself)
    let toc: Vec<(&str, &str)> = v.toc.iter().map(|t| (t.level, t.title)).collect();
    assert_eq!(toc.len(), 12);
    assert_eq!(toc[8], ("chapter", "Star"));
}

#[test]
fn guards() {
    let mut p = open(&[(
        "g.tex",
        "\\documentclass{article}\n\\usepackage{xr}\n\\renewcommand{\\section}[1]{\\textbf{#1}}\n\
         \\begin{document}\n\\section{S}\\label{s}\n\\subsection{T}\n\\ref{s} \\cite{k}\n\\end{document}\n",
    )]);
    let v = p.views();
    let broken: Vec<&str> = v.broken.iter().map(|b| b.cs.as_str()).collect();
    assert!(broken.contains(&"section"), "{broken:?}");
    assert!(broken.contains(&"ref"), "{broken:?}");
    assert!(!v.outline[0].guarded);
    assert!(v.outline[1].guarded);
    assert!(!v.refs[0].guarded);
    assert!(v.labels[0].guarded);
    drop(v);
    let a = p.assumptions();
    let state = |cs: &str| a.iter().find(|x| x.cs == cs).map(|x| x.state.clone());
    assert!(matches!(state("section"), Some(GuardState::Broken(_))));
    assert_eq!(state("label"), Some(GuardState::Assumed));
    // (a build confirms one and breaks another)
    p.confirm("label", Confirmed::Kept);
    p.confirm("cite", Confirmed::Broken("\\cite was \\relax".to_owned()));
    let a = p.assumptions();
    let state = |cs: &str| a.iter().find(|x| x.cs == cs).map(|x| x.state.clone());
    assert_eq!(state("label"), Some(GuardState::Kept));
    assert!(matches!(state("cite"), Some(GuardState::Broken(_))));
    assert!(!p.views().cites[0].guarded);

    // (the document's macros the facts went through are assumptions too)
    let p = open(&[(
        "m.tex",
        "\\newcommand{\\mysec}[1]{\\section{#1}}\n\\mysec{A}\n",
    )]);
    let found = p.assumptions();
    let mysec = found.iter().find(|x| x.cs == "mysec").expect("the macro");
    assert!(matches!(mysec.meant, Meant::Document(l) if l.line == 1));
    assert_eq!(mysec.facts, 1);
}

#[test]
fn citations_against_the_database() {
    let p = open(&[
        (
            "c.tex",
            "\\documentclass{article}\n\\begin{document}\n\\cite{a,b}\\citet*[x]{c}\n\\bibliography{refs,more}\n\
             \\begin{filecontents*}{more.bib}\n@book{c, title={x}}\n\\end{filecontents*}\n\\end{document}\n",
        ),
        ("refs.bib", "@article{a,\n title = {T}}\n"),
    ]);
    let v = p.views();
    let missing: Vec<&str> = v.missing_cites().map(|c| c.key).collect();
    assert_eq!(missing, ["b"]);
    assert_eq!(v.bibs.len(), 2);
    assert_eq!(v.bibs[0].keys, Some(1));
    assert_eq!(v.bibs[1].keys, Some(1));
}

/// The LaTeX documents of the e2e tests.
#[test]
fn e2e_documents() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/e2e");
    let fs = Rc::new(Dir(dir.into()));
    let p = Project::open("latexdoc.tex", fs.clone());
    let v = p.views();
    assert_eq!(
        titles(&v),
        [(1, Some("1"), "Introduction"), (2, Some("1.1"), "Lists")]
    );
    assert_eq!(labels(&v), ["sec:intro", "eq:e"]);
    assert_eq!(v.unresolved().count(), 0);
    assert_eq!(v.envs, []);

    let p = Project::open("modern.tex", fs.clone());
    let v = p.views();
    let unresolved: Vec<&str> = v.unresolved().map(|r| r.key).collect();
    assert_eq!(unresolved, ["sec:nowhere"]);
    assert_eq!(v.cites.len(), 2);
    // (xampl.bib is TeX Live's, not beside the document: nothing is said
    // of the keys)
    assert_eq!(v.missing_cites().count(), 0);

    let p = Project::open("bibdoc.tex", fs.clone());
    let v = p.views();
    assert_eq!(v.cites.len(), 10);
    assert_eq!(v.bibs[0].path, "xampl.bib");

    let p = Project::open("edits.tex", fs);
    let v = p.views();
    assert!(v.outline.len() > 20);
    assert_eq!(v.envs, []);
    // (every \ref names a label defined, or is reported)
    for r in v.unresolved() {
        assert!(!v.has_label(r.key));
    }
}

/// A directory, for the e2e documents.
struct Dir(std::path::PathBuf);

impl phitex_doc::Files for Dir {
    fn read(&self, path: &str) -> Option<String> {
        std::fs::read_to_string(self.0.join(path)).ok()
    }

    fn exists(&self, path: &str) -> bool {
        self.0.join(path).is_file()
    }
}
