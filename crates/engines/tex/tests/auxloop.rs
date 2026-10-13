//! The `.aux` loop-back: a file the job writes and reads back is a set
//! of per-field definitions, each round's read-back the previous
//! round's publication, settled when a round publishes what the one
//! before did.

mod common;

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
/// routine advanced it to 2), so `\ifodd\count0` takes the even branch
/// on the second page. Round one guesses nothing (the text's value, 1:
/// odd); the fed-back value is the number the previous round read at
/// this flush point, replaced, not added to, so a converged round reads
/// the same number twice: at most two rounds beyond the loop-back's
/// declaration.
const FLUSH_DOC: &str = r"\count0=1
Text one.\penalty-10000
\ifodd\count0 odd\else even\fi
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
    assert!(has(b"even") && !has(b"odd"), "the branch after the flush");
    // (one round declares the flush loop-back, one reads it: 2 beyond
    // the first)
    assert!(b.stats.loop_rounds <= 4, "{} loop rounds", b.stats.loop_rounds);
}
