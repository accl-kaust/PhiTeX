//! The corpus of synthetic old/new pairs (`tests/corpus/<case>/{old,new}`),
//! without TeX: every pair diffs; the output's braces balance outside
//! comments and verbatim; every change's ranges are its text in its
//! files; the preamble is the new one's. (`tests/compile.sh` compiles
//! them with pdflatex and compares with latexdiff.)

use phitex_diff::{ChangeKind, Dir, Files, Options, diff};
use phitex_source::{Green, SyntaxKind, lex};
use std::path::Path;

fn corpus() -> Vec<std::path::PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let mut cases: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .collect();
    cases.sort();
    cases
}

/// Whether every group of `g` is closed and no `}` is stray.
fn balanced(g: &Green) -> bool {
    g.children().iter().all(|c| match c.kind() {
        SyntaxKind::Other => false,
        SyntaxKind::Group => {
            c.children()
                .last()
                .is_some_and(|l| l.kind() == SyntaxKind::Brace)
                && c.children().len() >= 2
                && balanced(c)
        }
        _ => true,
    })
}

#[test]
fn every_pair() {
    for case in corpus() {
        let name = case.file_name().unwrap().to_string_lossy().into_owned();
        let old = Dir(case.join("old"));
        let new = Dir(case.join("new"));
        let d = diff(&old, &new, "main.tex", &Options::default()).unwrap();
        assert!(balanced(&lex(&d.tex)), "{name}: braces");
        // (the new preamble, then the definitions)
        let main = new.read("main.tex").unwrap();
        let begin = main.find("\\begin{document}").unwrap();
        assert!(
            d.tex.contains("%DIF PREAMBLE EXTENSION ADDED BY LATEXDIFF"),
            "{name}"
        );
        let pre = &main[..begin];
        let first_line = pre.lines().next().unwrap();
        assert!(d.tex.starts_with(first_line), "{name}: preamble");
        // (look_quiet: comments, a reflow and a label renamed, nothing the
        // pages show: no change, `quiet_edits_are_unmarked`)
        assert!(
            name == "look_quiet" || !d.changes.is_empty(),
            "{name}: no change found"
        );
        for c in &d.changes {
            for (files, loc, text) in [(&old, &c.old, &c.old_text), (&new, &c.new, &c.new_text)] {
                if loc.start == loc.end {
                    continue;
                }
                let src = files.read(&loc.file).unwrap();
                assert_eq!(&src[loc.start..loc.end], text, "{name}: {c:?}");
            }
            match c.kind {
                ChangeKind::Add => assert!(c.old_text.is_empty() && !c.new_text.is_empty()),
                ChangeKind::Del => assert!(!c.old_text.is_empty() && c.new_text.is_empty()),
                ChangeKind::Change => assert!(!c.old_text.is_empty() && !c.new_text.is_empty()),
            }
            assert!(c.out.end <= d.tex.len());
        }
        // (deterministic)
        let again = diff(&old, &new, "main.tex", &Options::default()).unwrap();
        assert_eq!(again.tex, d.tex, "{name}");
    }
}

/// The arguments of the markup's commands in `tex` (`\DIFadd{…}`,
/// `\DIFdel{…}` and their `FL` forms), comments skipped.
fn markup_args(tex: &str) -> Vec<&str> {
    let b = tex.as_bytes();
    let mut out = Vec::new();
    for cmd in ["\\DIFadd{", "\\DIFdel{", "\\DIFaddFL{", "\\DIFdelFL{"] {
        for (k, _) in tex.match_indices(cmd) {
            let line = tex[..k].rfind('\n').map_or(0, |l| l + 1);
            if tex[line..k].contains('%') {
                continue;
            }
            let open = k + cmd.len() - 1;
            let mut depth = 0usize;
            let mut i = open;
            while i < b.len() {
                match b[i] {
                    b'\\' => i += 1,
                    b'{' => depth += 1,
                    b'}' => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
                i += 1;
            }
            out.push(&tex[open + 1..i.min(b.len())]);
        }
    }
    out
}

/// The markup never cuts a unit apart: no markup command's argument holds
/// an environment (a float, an alignment, a display, a verbatim one), an
/// alignment's `&` or `\\`, a paragraph's end or a rule; a citation in it
/// is in short ones (at most three keys an `\mbox`: the markup breaks
/// between them), and no break is put after one (`\hskip0pt`, where a
/// line could begin with the punctuation after it).
#[test]
fn units_are_never_cut() {
    for case in corpus() {
        let name = case.file_name().unwrap().to_string_lossy().into_owned();
        let old = Dir(case.join("old"));
        let new = Dir(case.join("new"));
        let d = diff(&old, &new, "main.tex", &Options::default()).unwrap();
        for arg in markup_args(&d.tex) {
            for bad in [
                "\\begin{",
                "\\end{",
                "\\\\",
                "\n\n",
                "\\par ",
                "rule",
                "\\multicolumn",
            ] {
                assert!(!arg.contains(bad), "{name}: {bad} in the markup of {arg:?}");
            }
            let amp = arg
                .match_indices('&')
                .any(|(k, _)| k == 0 || &arg[k - 1..k] != "\\");
            assert!(!amp, "{name}: & in the markup of {arg:?}");
            for (k, _) in arg.match_indices("\\mbox{\\cite") {
                let keys = arg[k..].split('}').next().unwrap_or("");
                let list = keys.rsplit('{').next().unwrap_or("");
                assert!(list.split(',').count() <= 3, "{name}: {arg:?}");
            }
        }
        let end = "%DIF END PREAMBLE EXTENSION ADDED BY LATEXDIFF";
        let body = &d.tex[d.tex.find(end).unwrap_or(0)..];
        assert!(!body.contains("\\hskip0pt"), "{name}");
        // (each unit's markup closed: as many ends as begins)
        for unit in ["block", "inline", "math", "color", "tabular"] {
            for side in ["add", "del"] {
                let side = if unit == "tabular" { "" } else { side };
                let b = d.tex.matches(&format!("\\DIF{unit}{side}begin")).count();
                let e = d.tex.matches(&format!("\\DIF{unit}{side}end")).count();
                assert_eq!(b, e, "{name}: {unit} {side}");
            }
        }
    }
}

/// A float deleted or added whole is in its environment still, framed,
/// and a deleted one keeps its old number (its counter given back); a
/// changed table's rows of other things are a row deleted and a row
/// added, a changed citation is diffed by key.
#[test]
fn floats_tables_and_citations() {
    let run = |case: &str| {
        let c = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/corpus")
            .join(case);
        // (white space aside)
        diff(
            &Dir(c.join("old")),
            &Dir(c.join("new")),
            "main.tex",
            &Options::default(),
        )
        .unwrap()
        .tex
        .split_whitespace()
        .collect::<String>()
    };
    let t = run("look_table_del");
    assert!(
        t.contains("\\begin{table*}[!tp]\\DIFfloatnumber{0}\\DIFblockdelbegin"),
        "{t}"
    );
    assert!(
        t.contains("\\DIFblockdelend\\DIFfloatrestore\\end{table*}"),
        "{t}"
    );
    assert!(
        t.contains("\\begin{table}[!htbp]\\DIFfloatnumber{1}\\DIFblockdelbegin"),
        "{t}"
    );
    let t = run("look_table_add");
    assert!(t.contains("\\begin{table*}[t]\\DIFblockaddbegin"), "{t}");
    let t = run("look_table_change");
    // (the row of another framework: deleted, the new one added, not
    // their cells paired)
    assert!(t.contains("\\DIFdelFL{Morpher~"), "{t}");
    assert!(t.contains("\\DIFaddFL{Pillars~"), "{t}");
    assert!(t.contains("\\multicolumn{2}{c}{Runs"), "{t}");
    let t = run("look_cite_long");
    assert!(t.contains("k12,k14,"), "{t}");
    // (one citation, the keys removed and added marked inside it; beside
    // it where the citations are not natbib's numbers)
    assert!(t.contains("\\DIFcite{\\cite{k01,"), "{t}");
    assert!(
        t.contains(
            "\\DIFdel{\\mbox{\\cite{k13}}}\\DIFadd{\\mbox{\\cite{k44,k45}}}}{\\citealp{k01,"
        ),
        "{t}"
    );
    assert!(
        t.contains("\\DIFcitesep\\DIFdel{\\mbox{\\citealp{k13}}}\\DIFcitesep\\DIFadd{\\mbox{\\citealp{k44,k45}}}}."),
        "{t}"
    );
}

/// Comments changed, a paragraph's lines broken otherwise and a label
/// renamed with its references: nothing the pages show changed, so no
/// change and no markup.
#[test]
fn quiet_edits_are_unmarked() {
    let c = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/look_quiet");
    let d = diff(
        &Dir(c.join("old")),
        &Dir(c.join("new")),
        "main.tex",
        &Options::default(),
    )
    .unwrap();
    assert!(d.changes.is_empty(), "{:?}", d.changes);
    let end = "%DIF END PREAMBLE EXTENSION ADDED BY LATEXDIFF";
    let body = &d.tex[d.tex.find(end).unwrap() + end.len()..];
    assert!(!body.contains("\\DIF"), "{body}");
}

/// A version diffed against itself has no change, and its markup is its
/// text.
#[test]
fn identity() {
    for case in corpus() {
        let new = Dir(case.join("new"));
        let d = diff(&new, &new, "main.tex", &Options::default()).unwrap();
        assert!(d.changes.is_empty(), "{}", case.display());
        // (the body after the markup's definitions, which follow
        // `\begin{document}`)
        let end = "%DIF END PREAMBLE EXTENSION ADDED BY LATEXDIFF";
        let body = &d.tex[d.tex.find(end).unwrap() + end.len()..];
        assert!(!body.contains("\\DIF"), "{}", case.display());
    }
}

/// A main file only the new version has: all added.
#[test]
fn new_document() {
    let case = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/newdoc");
    let d = diff(
        &Dir(case.join("old")),
        &Dir(case.join("new")),
        "main.tex",
        &Options::default(),
    )
    .unwrap();
    assert!(d.changes.iter().all(|c| c.kind == ChangeKind::Add));
}

/// The multi-file pair: the deleted input's text is deleted where it was
/// input, the new file's added, each change in its own file.
#[test]
fn multifile() {
    let case = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/multifile");
    let d = diff(
        &Dir(case.join("old")),
        &Dir(case.join("new")),
        "main.tex",
        &Options::default(),
    )
    .unwrap();
    let files: Vec<(&str, &str)> = d
        .changes
        .iter()
        .map(|c| (c.old.file.as_str(), c.new.file.as_str()))
        .collect();
    assert!(
        files.contains(&("chapters/methods.tex", "chapters/results.tex")),
        "{files:?}"
    );
    assert!(
        files.contains(&("chapters/intro.tex", "chapters/intro.tex")),
        "{files:?}"
    );
    assert!(
        d.tex
            .contains("\\DIFdel{This file is no longer input by the new version}")
    );
    // (the preamble's own input followed, unmarked)
    assert!(d.tex.contains("\\newcommand{\\C}"));
    assert_eq!(d.changes[0].section.as_deref(), Some("Introduction"));
}

/// Markup closed at a cut and opened again after it taken out, and the
/// white space (a file's last line end is TeX's, kept where the line after
/// its `\input` goes on in the markup): the files flattened again are the
/// marked-up text.
fn unopened(s: &str) -> String {
    let s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    s.replace("}\\DIFaddFL{", "").replace("}\\DIFadd{", "")
}

/// The marked-up text split back into the new version's files
/// (`DiffOut::split`): every pair splits; the files flattened again are
/// the marked-up text; each file's runs are the flat text's, found again
/// by `Split::flat_of`; the live diff splits as the whole one does.
#[test]
fn split_back_into_files() {
    use std::collections::BTreeMap;
    for case in corpus() {
        let name = case.file_name().unwrap().to_string_lossy().into_owned();
        let old = Dir(case.join("old"));
        let new = Dir(case.join("new"));
        let d = diff(&old, &new, "main.tex", &Options::default()).unwrap();
        let s = d
            .split
            .as_ref()
            .unwrap_or_else(|| panic!("{name}: no split"));
        assert_eq!(s.texts.len(), d.files.len(), "{name}");
        let files: BTreeMap<String, String> = d
            .files
            .iter()
            .cloned()
            .zip(s.texts.iter().cloned())
            .collect();
        let again = phitex_diff::flatten(&files, "main.tex").unwrap();
        assert_eq!(unopened(&again.text), unopened(&d.tex), "{name}");
        for (k, runs) in s.runs.iter().enumerate() {
            for &(at, len, out) in runs {
                assert_eq!(&s.texts[k][at..at + len], &d.tex[out..out + len], "{name}");
                assert_eq!(s.flat_of(k, at), Some(out), "{name}");
            }
        }
        let base = phitex_diff::Baseline::new(&old, "main.tex", &Options::default())
            .unwrap_or_else(|_| {
                phitex_diff::Baseline::from_flat(phitex_diff::Flat::default(), &Options::default())
            });
        let mut live = phitex_diff::Live::new(base);
        let l = live.update(&phitex_diff::flatten(&new, "main.tex").unwrap());
        assert_eq!(l.split, d.split, "{name}: live");
    }
}

/// A file added, words added where a file's text begins and ends: each
/// file's marked-up text its own, its `\input` commands kept, the markup
/// of a run of added text cut between two files closed in the one and
/// opened again in the other.
#[test]
fn split_cuts_added_runs() {
    use std::collections::BTreeMap;
    let doc = |body: &str| {
        format!("\\documentclass{{article}}\n\\begin{{document}}\n{body}\\end{{document}}\n")
    };
    let mut o = BTreeMap::new();
    o.insert(
        "main.tex".to_owned(),
        doc("Before it.\n\\input{ch}\nAfter it.\n"),
    );
    o.insert("ch.tex".to_owned(), "Middle words here.\n".to_owned());
    let mut n = BTreeMap::new();
    n.insert(
        "main.tex".to_owned(),
        doc("Before it new\n\\input{ch}\nAfter it.\n\n\\input{added}\n"),
    );
    n.insert(
        "ch.tex".to_owned(),
        "fresh Middle words here more\n".to_owned(),
    );
    n.insert("added.tex".to_owned(), "A whole new file.\n".to_owned());
    let d = diff(&o, &n, "main.tex", &Options::default()).unwrap();
    let s = d.split.clone().expect("split");
    let at = |f: &str| d.files.iter().position(|x| x == f).unwrap();
    let main = &s.texts[at("main.tex")];
    let ch = &s.texts[at("ch.tex")];
    let added = &s.texts[at("added.tex")];
    assert!(
        main.contains("\\input{ch}") && main.contains("\\input{added}"),
        "{main}"
    );
    assert!(
        !main.contains("Middle") && !main.contains("whole new"),
        "{main}"
    );
    assert!(ch.contains("fresh") && ch.contains("more"), "{ch}");
    assert!(
        ch.starts_with("\\DIFadd{") || ch.contains("\\DIFadd{fresh"),
        "{ch}"
    );
    assert!(added.contains("whole new file"), "{added}");
    assert!(added.ends_with('\n'), "{added}");
    for t in [main, ch, added] {
        assert_eq!(t.matches('{').count(), t.matches('}').count(), "{t}");
    }
    let files: BTreeMap<String, String> = d.files.iter().cloned().zip(s.texts.clone()).collect();
    let again = phitex_diff::flatten(&files, "main.tex").unwrap();
    assert_eq!(unopened(&again.text), unopened(&d.tex));
}
