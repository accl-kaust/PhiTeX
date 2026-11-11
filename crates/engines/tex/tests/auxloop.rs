//! The read-back loop: a file the job writes and reads back is a set
//! of per-field definitions, each round's read-back the previous
//! round's publication, settled when a round publishes what the one
//! before did.

use crate::common;

use phitex_tex::Options;

/// A plain job that reads its `.aux` as LaTeX does (absent on the first
/// run) and writes two lines: a `\gdef`, and a line whose effect depends
/// on the state it is read in (biblatex's `\xifinlist`-guarded
/// `\listxadd` has this shape): defined only when `\foo` is still empty.
const DOC: &str = r"\def\foo{}\def\bar{}
\def\addfoo#1{\ifx\foo\empty\gdef\foo{#1}\fi}
\openin15=\jobname.aux \ifeof15 \else\closein15 \input\jobname.aux \fi
\immediate\openout1=\jobname.aux
\immediate\write1{\noexpand\addfoo{abc}}
\immediate\write1{\noexpand\gdef\noexpand\bar{xyz}}
Value: \foo/\bar.
\bye
";

fn build() -> phitex_tex::Build {
    let dir = common::fixtures().join("plain");
    let opts = Options {
        fixpoint: true,
        ..common::options("./t.tex", "t", "", &dir)
    };
    common::build(opts, Some(&common::plain()), DOC)
}

/// Round one reads no `.aux` and publishes both fields; round two reads
/// them back and, interpreting the lines it writes as the next read
/// will (before its own read-back took effect), publishes the same
/// content: nothing changes and the evaluation settles (the rounds
/// beyond two declare the fields' loop-backs and the flush point's
/// once each; without the reverted read the state-dependent line
/// toggles until the engine gives up after 64).
#[test]
fn equal_content_publishes_nothing_the_second_time() {
    let b = build();
    let errors: Vec<String> = b
        .diagnostics()
        .iter()
        .filter(|d| d.severity == phitex_tex::Severity::Error)
        .map(|d| d.message.clone())
        .filter(|m| m.contains("fixpoint") || m.contains("rounds"))
        .collect();
    assert!(errors.is_empty(), "{errors:?}");
    assert!(
        b.stats.loop_rounds <= 5,
        "{} loop rounds",
        b.stats.loop_rounds
    );
    let files = b.files();
    let aux = files
        .iter()
        .find(|(n, _)| n == "t.aux")
        .map(|(_, t)| t.as_str())
        .unwrap_or_default();
    assert_eq!(aux, "\\addfoo {abc}\n\\gdef \\bar {xyz}\n");
    // the page typeset with the read-back values: `abc/xyz` set in the
    // DVI (set_char opcodes are the ASCII codes)
    let (dvi, _) = b.output();
    let shown: String = dvi
        .iter()
        .map(|&c| {
            if c.is_ascii_graphic() {
                char::from(c)
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .filter(|w| w.len() > 1)
        .collect::<Vec<_>>()
        .join(" ");
    // (cmr10 kerns b–c: the words as the DVI sets them)
    for word in [&b"ab"[..], b"c/xyz"] {
        assert!(
            dvi.windows(word.len()).any(|w| w == word),
            "the page does not show the read-back value {}: {shown}",
            String::from_utf8_lossy(word)
        );
    }
}

/// A flush point in the text (`\penalty-10000` on the main list, as
/// `\clearpage` makes one): the text after it reads `\count0` as the
/// page chain leaves it after the page that breaks there (plain's
/// routine advanced it to 2), so `\ifodd\count0` takes the even branch (`ccc`)
/// on the second page. Round one guesses nothing (the text's value, 1:
/// odd); the fed-back value is the number the previous round read at
/// this flush point, replaced, not added to, so a converged round reads
/// the same number twice: at most two rounds beyond the loop-back's
/// declaration.
const FLUSH_DOC: &str = r"\count0=1
Text one.\par\penalty-10000
\ifodd\count0 aaa\else ccc\fi
\bye
";

#[test]
fn a_flush_point_reads_the_page_number_after_its_page() {
    let dir = common::fixtures().join("plain");
    let opts = Options {
        fixpoint: true,
        ..common::options("./f.tex", "f", "", &dir)
    };
    let b = common::build(opts, Some(&common::plain()), FLUSH_DOC);
    let errors: Vec<String> = b
        .diagnostics()
        .iter()
        .filter(|d| d.severity == phitex_tex::Severity::Error)
        .map(|d| d.message.clone())
        .filter(|m| m.contains("fixpoint") || m.contains("rounds"))
        .collect();
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(b.pages().len(), 2);
    let (dvi, _) = b.output();
    let has = |w: &[u8]| dvi.windows(w.len()).any(|x| x == w);
    // (words with no kerns or ligatures in cmr10: their bytes in a row)
    assert!(
        has(b"ccc") && !has(b"aaa"),
        "the branch after the flush: {}",
        String::from_utf8_lossy(&dvi)
            .chars()
            .filter(char::is_ascii_lowercase)
            .collect::<String>()
    );
    // the pages' `\count0`, from their `bop`s (DVI: the postamble's
    // pointer to the last, each to the one before): 1 and 2, as the
    // routine numbered them
    assert_eq!(bop_counts(&dvi), [1, 2]);
    // (one round declares the flush loop-back, one reads it: 2 beyond
    // the first)
    assert!(
        b.stats.loop_rounds <= 4,
        "{} loop rounds",
        b.stats.loop_rounds
    );
}

/// `\immediate` before a macro that expands to `\immediate\write` (§1375:
/// the token read is not `\write`, so it is backed up and `\immediate`
/// has done nothing; the macro's own `\immediate\write` then writes at
/// once). LaTeX's `\enddocument` makes `\write` such a macro
/// (`\__shipout_force_immediate_writes:`), and the page steps interpret
/// the `.aux`'s `\@writefile` lines in that state.
const IMMEDIATE_DOC: &str = r"\immediate\openout1=\jobname.aux
\def\w{\immediate\write}
\immediate\w1{hi}
\immediate\write1{there}
Text.
\bye
";

#[test]
fn immediate_before_a_macro_for_immediate_write() {
    let dir = common::fixtures().join("plain");
    let opts = common::options("./i.tex", "i", "", &dir);
    let b = common::build(opts, Some(&common::plain()), IMMEDIATE_DOC);
    let files = b.files();
    let aux = files
        .iter()
        .find(|(n, _)| n == "i.aux")
        .map(|(_, t)| t.as_str())
        .unwrap_or_default();
    assert_eq!(aux, "hi\nthere\n");
}

/// Each page's `\count0` in a DVI file, in order (the `bop` chain from
/// the postamble).
fn bop_counts(dvi: &[u8]) -> Vec<i32> {
    let word = |i: usize| {
        dvi.get(i..i + 4)
            .map_or(-1, |b| i32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    };
    let mut end = dvi.len();
    while end > 0 && dvi[end - 1] == 223 {
        end -= 1;
    }
    // (`post_post`, q, id, then the 223s)
    let Some(q) = end
        .checked_sub(5)
        .and_then(|i| usize::try_from(word(i)).ok())
    else {
        return Vec::new();
    };
    let mut p = word(q + 1);
    let mut cs = Vec::new();
    while let Ok(i) = usize::try_from(p) {
        if dvi.get(i) != Some(&139) {
            break;
        }
        cs.push(word(i + 1));
        p = word(i + 41);
    }
    cs.reverse();
    cs
}

/// A job-written file is read back per field because every line written
/// to it only defines, not because of its name: [`DOC`] writing to a
/// file of another name reads the same values back, and the file is one
/// of the per-field read-backs the build exports.
#[test]
fn a_file_of_definitions_is_read_back_per_field_whatever_its_name() {
    let dir = common::fixtures().join("plain");
    let opts = Options {
        fixpoint: true,
        ..common::options("./t.tex", "t", "", &dir)
    };
    let doc = DOC.replace("\\jobname.aux", "\\jobname.dat");
    let b = common::build(opts, Some(&common::plain()), &doc);
    let (dvi, _) = b.output();
    for word in [&b"ab"[..], b"c/xyz"] {
        assert!(
            dvi.windows(word.len()).any(|w| w == word),
            "the page does not show the read-back value {}",
            String::from_utf8_lossy(word)
        );
    }
    assert!(
        b.wkeys().iter().any(|(n, _)| n == "t.dat"),
        "read back per field: {:?}",
        b.wkeys()
    );
}

/// A line that typesets makes its file one read back as its text: the
/// read-back typesets the line and defines what the other lines define.
const TEXT_DOC: &str = r"\def\bar{}
\openin15=\jobname.aux \ifeof15 \else\closein15 \input\jobname.aux \fi
\immediate\openout1=\jobname.aux
\immediate\write1{\noexpand\gdef\noexpand\bar{xyz}}
\immediate\write1{qqq}
Value: \bar.
\bye
";

#[test]
fn a_line_that_typesets_reads_the_file_back_as_text() {
    let dir = common::fixtures().join("plain");
    let opts = Options {
        fixpoint: true,
        ..common::options("./u.tex", "u", "", &dir)
    };
    let b = common::build(opts, Some(&common::plain()), TEXT_DOC);
    let (dvi, _) = b.output();
    let has = |w: &[u8]| dvi.windows(w.len()).any(|x| x == w);
    assert!(
        has(b"qqq") && has(b"xyz"),
        "{}",
        String::from_utf8_lossy(&dvi)
    );
    assert!(
        !b.wkeys().iter().any(|(n, _)| n == "u.aux"),
        "read back as text: {:?}",
        b.wkeys()
    );
}

/// A line that measures a box and sets a dimension from it only defines
/// (hypdoc's `\\HD@SetMacroIndent` in the `.aux` of every manual with its
/// source): the file is read back per field, the value as TeX's. A box left
/// on the page by a line, or kept in a register by it (abspos's `\\absput`,
/// a box kept for shipout), still reads it back as text.
#[test]
fn a_line_that_measures_a_box_only_defines() {
    let doc = |line: &str| {
        format!(
            "\\newdimen\\ind \\newbox\\tmp \\newbox\\kept\n\\def\\set#1{{\\begingroup\\setbox\\tmp\\hbox{{{{#1}}}}\\global\\ind=\\wd\\tmp\\endgroup}}\n\\def\\keep#1{{\\global\\setbox\\kept\\hbox{{#1}}}}\n\
             \\openin15=\\jobname.aux \\ifeof15 \\else\\closein15 \\input\\jobname.aux \\fi\n\
             \\immediate\\openout1=\\jobname.aux\n\\immediate\\write1{{{line}}}\n\\immediate\\closeout1\n\
             Width \\the\\ind.\\par\\ifvoid\\kept\\else\\box\\kept\\fi\n\\bye\n"
        )
    };
    let dir = common::fixtures().join("plain");
    let build = |name: &str, text: &str| {
        let opts = Options {
            fixpoint: true,
            ..common::options(&format!("./{name}.tex"), name, "", &dir)
        };
        common::build(opts, Some(&common::plain()), text)
    };
    let b = build("m", &doc("\\noexpand\\set{abc}"));
    assert!(
        b.wkeys().iter().any(|(n, _)| n == "m.aux"),
        "read back per field: {:?}",
        b.wkeys()
    );
    // (cmr10's "abc": 5pt, 5.55557pt, 4.44444pt less a kern: not zero)
    let (dvi, _) = b.output();
    let text = String::from_utf8_lossy(&dvi);
    assert!(!text.contains("0.0pt"), "the width reached the text");
    let c = build("n", &doc("\\noexpand\\hbox{abc}"));
    assert!(
        !c.wkeys().iter().any(|(n, _)| n == "n.aux"),
        "a box left by the line: as text: {:?}",
        c.wkeys()
    );
    let k = build("k", &doc("\\noexpand\\keep{abc}"));
    assert!(
        !k.wkeys().iter().any(|(n, _)| n == "k.aux"),
        "a box kept by the line: as text: {:?}",
        k.wkeys()
    );
}

/// A file the job writes only where it is missing, then reads (a package
/// a document carries in its own text): TeX's next run finds the file the
/// run before wrote, so it does not write it again and reads it; the
/// file stays as that run left it, and the rounds settle on the second
/// (the next round's file is not absent for want of a write, which would
/// write it again in the third, and so on to the cap).
const KEPT_DOC: &str = r"\openin15=kept.tex
\ifeof15 \immediate\openout2=kept.tex
\immediate\write2{\noexpand\def\noexpand\word{12345}}
\immediate\closeout2 \else\closein15 \fi
\input kept.tex
Word: \word.
\bye
";

#[test]
fn a_file_written_where_missing_stays_for_the_next_round() {
    let dir = common::fixtures().join("plain");
    let opts = Options {
        fixpoint: true,
        ..common::options("./k.tex", "k", "", &dir)
    };
    let b = common::build(opts, Some(&common::plain()), KEPT_DOC);
    eprintln!("kept: {} loop rounds", b.stats.loop_rounds);
    let (dvi, _) = b.output();
    assert!(
        dvi.windows(5).any(|w| w == b"12345"),
        "the page does not show the file's definition"
    );
    assert!(
        b.stats.loop_rounds <= 2,
        "{} loop rounds (the file written in round one, found in round two)",
        b.stats.loop_rounds
    );
}

/// A written file read back twice by one macro, its lines meaning a
/// definition the first time and text the second (a list read once to
/// note its entries, then to set them): read back as its text, both
/// reads see its lines, and the page shows the entries (`mmm`, `nnn`)
/// after a build that read the file back. Read back per field, by the
/// definitions the lines make at the first read, the second read would
/// find those definitions instead of the lines and set nothing.
#[test]
fn a_file_read_back_twice_by_one_run_is_read_as_text() {
    let doc = r"\def\LST#1{\ENTRY{#1}}
\def\readtwice{%
  \gdef\ENTRY##1{\expandafter\gdef\csname seen##1\endcsname{}}%
  \openin15=\jobname.lst \ifeof15 \let\next\relax\else\closein15 \def\next{\input\jobname.lst }\fi\next
  \gdef\ENTRY##1{##1 }%
  \openin15=\jobname.lst \ifeof15 \let\next\relax\else\closein15 \def\next{\input\jobname.lst }\fi\next
  }
\readtwice\par
\immediate\openout2=\jobname.lst
\immediate\write2{\string\LST{mmm}}
\immediate\write2{\string\LST{nnn}}
Entries.
\bye
";
    let dir = common::fixtures().join("plain");
    let opts = Options {
        fixpoint: true,
        ..common::options("./r.tex", "r", "", &dir)
    };
    let b = common::build(opts, Some(&common::plain()), doc);
    let (dvi, _) = b.output();
    let has = |w: &[u8]| dvi.windows(w.len()).any(|x| x == w);
    assert!(
        has(b"mmm") && has(b"nnn"),
        "the second read's text: {}",
        String::from_utf8_lossy(&dvi)
            .chars()
            .filter(char::is_ascii_lowercase)
            .collect::<String>()
    );
}

/// A lookup of a name a tool makes from a file the job writes (here the
/// bibliography from the `.aux`, which asks for no data, so the tool
/// makes nothing): the reader depends on what the tool makes, not on the
/// whole text of its input. A line written to the input changed runs
/// the tool's memoised run again, not the node that looked the name up.
#[test]
fn a_tool_output_lookup_does_not_read_the_tool_input() {
    let doc = |word: &str| {
        format!(
            r"\openin15=\jobname.bbl \ifeof15 \gdef\st{{none}}\else\closein15 \gdef\st{{some}}\fi

\immediate\openout1=\jobname.aux


\immediate\write1{{\relax {word}}}
Status \st.
\bye
"
        )
    };
    let dir = common::fixtures().join("plain");
    let opts = || Options {
        fixpoint: true,
        ..common::options("./b.tex", "b", "", &dir)
    };
    let mut b = common::build(opts(), Some(&common::plain()), &doc("one"));
    b.edit(&doc("two"));
    let reran: Vec<String> = b
        .stats
        .reruns
        .iter()
        .map(|(n, op, why)| format!("{op} {why} [{}]", b.describe_node(*n)))
        .collect();
    for r in &reran {
        eprintln!("  rerun {r}");
    }
    let cold = common::build(opts(), Some(&common::plain()), &doc("two"));
    let d = common::first_difference(&b.output().0, &cold.output().0);
    assert!(d.is_none(), "edit differs from cold: {d:?}");
    assert!(
        !reran.iter().any(|r| r.contains("openin")),
        "the node that looked the tool's output up ran again: {reran:?}"
    );
}

/// A `\read` of a file the job writes and reads back as its text (here
/// its first line, as logreq's `\lrq@writeout` reads the `.run.xml`'s
/// first three before writing it again): the reader depends on the
/// lines it read, not the whole text. A later line changed runs the
/// file's node again, not the node that read the first line; the first
/// line changed runs it again.
#[test]
fn a_read_of_a_written_file_depends_on_the_lines_read() {
    let doc = |head: &str, word: &str| {
        format!(
            r"\openin15=\jobname.xml \ifeof15 \gdef\st{{none}}\else\readline15 to\st \closein15 \fi

\immediate\openout1=\jobname.xml


\immediate\write1{{{head}}}\immediate\write1{{{word}}}
Status \st.
\bye
"
        )
    };
    let dir = common::fixtures().join("plain");
    let opts = || Options {
        fixpoint: true,
        ..common::options("./x.tex", "x", "", &dir)
    };
    let reran = |b: &phitex_tex::Build| -> Vec<String> {
        b.stats
            .reruns
            .iter()
            .map(|(n, op, why)| format!("{op} {why} [{}]", b.describe_node(*n)))
            .collect()
    };
    let mut b = common::build(opts(), Some(&common::plain()), &doc("head", "one"));
    eprintln!("cold: {} loop rounds", b.stats.loop_rounds);
    b.edit(&doc("head", "two"));
    let r = reran(&b);
    for x in &r {
        eprintln!("  rerun {x}");
    }
    let cold = common::build(opts(), Some(&common::plain()), &doc("head", "two"));
    let d = common::first_difference(&b.output().0, &cold.output().0);
    assert!(d.is_none(), "edit differs from cold: {d:?}");
    assert!(
        !r.iter().any(|x| x.contains("openin")),
        "the node that read the file's first line ran again: {r:?}"
    );
    b.edit(&doc("tail", "two"));
    let r = reran(&b);
    let cold = common::build(opts(), Some(&common::plain()), &doc("tail", "two"));
    let d = common::first_difference(&b.output().0, &cold.output().0);
    assert!(d.is_none(), "first line's edit differs from cold: {d:?}");
    assert!(
        r.iter().any(|x| x.contains("openin")),
        "the node that read the file's first line did not run again: {r:?}"
    );
}

/// A run that reads the `.aux` inline and goes on past a page candidate
/// (LaTeX's `\begin{document}`: the `.aux` read, then the hooks): its
/// rest is a resume reading the run's continuation at the cut (the
/// file read in a group: inline, its units not pieces of their own). The
/// `.aux` one line longer (a `\citation` more) runs the reader again,
/// and the continuation it leaves is the same: the number of lines the
/// run began is no part of it, so the rest does not run again (on
/// Grail every `\cite` added and every heading edited ran the rest of
/// `\begin{document}` again, 150 ms).
#[test]
fn a_longer_aux_does_not_run_the_rest_after_its_reader_again() {
    let doc = |n: usize| {
        let writes = "\\immediate\\write1{a}\n".repeat(n);
        format!(
            r"\def\start{{\begingroup\openin15=\jobname.aux \ifeof15 \else\closein15 \input\jobname.aux \fi\endgroup
  Before the cut.\par After the cut, the rest of the run.\par}}
\start

Text between.

\immediate\openout1=\jobname.aux

{writes}
\bye
"
        )
    };
    let dir = common::fixtures().join("plain");
    for threads in [1, 4] {
        let opts = || Options {
            fixpoint: true,
            threads: Some(threads),
            ..common::options("./c.tex", "c", "", &dir)
        };
        let mut b = common::build(opts(), Some(&common::plain()), &doc(2));
        // (one line more each time, then fewer)
        for n in [3, 4, 5, 2] {
            b.edit(&doc(n));
            let r: Vec<String> = b
                .stats
                .reruns
                .iter()
                .map(|(n, op, why)| format!("{op} {why} [{}]", b.describe_node(*n)))
                .collect();
            for x in &r {
                eprintln!("  -j{threads} {n} lines: rerun {x}");
            }
            let cold = common::build(opts(), Some(&common::plain()), &doc(n));
            let d = common::first_difference(&b.output().0, &cold.output().0);
            assert!(
                d.is_none(),
                "-j{threads} {n}: edit differs from cold: {d:?}"
            );
            assert!(
                r.iter()
                    .any(|x| x.starts_with("exec") && x.contains("\\start")),
                "-j{threads} {n}: the reader of the .aux did not run again: {r:?}"
            );
            // (the file's node runs once: it reads no definition of its
            // own file's text or size, which it makes itself)
            assert!(
                !r.iter().any(|x| x.starts_with("files Changed wsize:")
                    || x.starts_with("files Changed wtext:")),
                "-j{threads} {n}: the .aux's node ran again on its own text: {r:?}"
            );
            // (the rest is a resume reading `cont:<reader>:<k>`)
            assert!(
                !r.iter().any(|x| x.contains("Changed cont:")),
                "-j{threads} {n}: the rest after the reader ran again: {r:?}"
            );
        }
    }
}

/// Text after a page defining, in the macro that looked for a file in
/// vain, a field the page's output routine defines: keeping the value
/// there for the lines written later reads the field, a read held while
/// the pages have not reached the text. The run stops there and runs
/// again; it used to go on without the value and commit, with what it
/// published depending on when the pages got there (a debug build's
/// assertion in the scheduler, a run ended past a stop it did not
/// return; Grail's `\inputminted`s under `PHITEX_RESUME_CHECK`).
#[test]
fn a_definition_kept_for_a_read_back_stops_at_the_frontier() {
    use std::fmt::Write as _;
    let mut doc = String::from(
        r"\newcount\n
\output={\global\def\mine{routine}\shipout\box255 }
\def\window{\openin1=nosuchfile.xyz \global\def\mine{text}}
\immediate\openout1=f.aux
",
    );
    for i in 0..400 {
        writeln!(
            doc,
            "Paragraph {i} of the text, long enough for the pages to fire.\n"
        )
        .unwrap();
        if i % 40 == 39 {
            doc.push_str("\\window\n\n");
        }
    }
    doc.push_str("\\immediate\\write1{\\noexpand\\gdef\\noexpand\\seen{\\mine}}\n\\bye\n");
    let dir = common::fixtures().join("plain");
    let opts = Options {
        fixpoint: true,
        ..common::options("./w.tex", "w", "", &dir)
    };
    let b = common::build(opts, Some(&common::plain()), &doc);
    assert!(!b.output().0.is_empty(), "no output");
}

/// A read-back line that defines a register past the fixed ones (a spill
/// location numbered by the line), then a long run of commands in the same
/// node: the line's changes are rolled back after it, its location with
/// them, and the checkpoints after it name the registers the run defined by
/// their fields. Kept from the line, the register's location had no field
/// any more (`Fields::field_of` of a spill location, a debug assertion), and
/// in a release build that location was cached as the empty name's and a
/// later resume wrote the empty name's slot over another field's
/// (simpledarkblue -j4: `\fontdimen6` of the current font read as 0, a
/// `0.25em` skip as 0pt, about 1 build in 5).
#[test]
fn a_read_back_line_s_register_location_goes_with_its_rollback() {
    const DOC: &str = r"\openin15=\jobname.aux \ifeof15 \else\closein15 \input\jobname.aux \fi
\immediate\openout1=\jobname.aux
\immediate\write1{\global\count300=7 }
\immediate\closeout1
\count1=0 \loop\advance\count1 by1 \ifnum\count1<4000 \repeat
Value: \the\count300.
\bye
";
    let dir = common::fixtures().join("plain");
    let opts = Options {
        fixpoint: true,
        ..common::options("./r.tex", "r", "", &dir)
    };
    let b = common::build(opts, Some(&common::plain()), DOC);
    let (dvi, _) = b.output();
    assert!(dvi.windows(4).any(|w| w == b"alue"), "no page text");
}

/// A line that adds to what a line before it in the same file defined
/// (biblatex's `\abx@aux@cite` adding a key to a list, `\blx@pagesum@old`
/// taking a digit per page line): the file is a fold, its definitions
/// those of its whole text read at once from the read-back point, so it
/// is still read back per field. The list holds every line's item, as
/// TeX's read of the file leaves it; a line added to the list changes
/// that field alone, and the run that reads the file back, which uses
/// another field, does not run again (read as its text, it ran again on
/// every change of the file); every edit's output equals a cold build's.
#[test]
fn a_line_adding_to_an_earlier_line_s_list_reads_back_per_field() {
    let doc = |extra: &str| {
        format!(
            r"\def\lst{{}}\def\addto#1{{\xdef\lst{{\lst#1,}}}}\def\other{{}}

\openin15=\jobname.aux \ifeof15 \else\closein15 \input\jobname.aux \fi Other: \other.

\immediate\openout1=\jobname.aux
\immediate\write1{{\noexpand\addto{{a}}}}\immediate\write1{{\noexpand\gdef\noexpand\other{{xyz}}}}

Middle{extra}.

List: \lst.
\bye
"
        )
    };
    let more = "\\immediate\\write1{\\noexpand\\addto{b}}";
    let dir = common::fixtures().join("plain");
    for threads in [1, 4] {
        let opts = || Options {
            fixpoint: true,
            threads: Some(threads),
            ..common::options("./fold.tex", "fold", "", &dir)
        };
        let mut b = common::build(opts(), Some(&common::plain()), &doc(more));
        let (dvi, _) = b.output();
        let has = |d: &[u8], w: &[u8]| d.windows(w.len()).any(|x| x == w);
        assert!(
            has(&dvi, b"a,b,") && has(&dvi, b"xyz"),
            "-j{threads}: the list after the read-back: {}",
            String::from_utf8_lossy(&dvi)
        );
        for extra in ["", more, " and more", ""] {
            b.edit(&doc(extra));
            let r: Vec<String> = b
                .stats
                .reruns
                .iter()
                .map(|(n, op, why)| format!("{op} {why} [{}]", b.describe_node(*n)))
                .collect();
            for x in &r {
                eprintln!("  -j{threads} {extra:?}: rerun {x}");
            }
            let cold = common::build(opts(), Some(&common::plain()), &doc(extra));
            let d = common::first_difference(&b.output().0, &cold.output().0);
            assert!(
                d.is_none(),
                "-j{threads} {extra:?}: edit differs from cold: {d:?}"
            );
            assert!(
                !r.iter()
                    .any(|x| x.starts_with("exec") && x.contains("openin")),
                "-j{threads} {extra:?}: the run reading the file back ran again: {r:?}"
            );
        }
    }
}
