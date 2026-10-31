use super::*;

const DOC: &str = "\\def\\a{x}\n\n{\\b\n\nc}% hi\n  \n\ntext #1 }\n\n\n";

#[test]
fn lossless() {
    let t = Tree::parse(DOC);
    assert_eq!(t.text(), DOC);
    assert_eq!(t.len(), DOC.len());
}

/// Paragraphs end after their blank lines, never inside a group.
#[test]
fn paragraphs() {
    let t = Tree::parse(DOC);
    let texts: Vec<String> = t.paras().iter().map(|p| p.text().to_string()).collect();
    assert_eq!(
        texts,
        [
            "\\def\\a{x}\n\n",
            "{\\b\n\nc}% hi\n  \n\n",
            "text #1 }\n\n\n"
        ]
    );
}

#[test]
fn red_offsets() {
    let t = Tree::parse(DOC);
    let mut last = 0;
    for r in t.reds() {
        assert_eq!(r.offset, last);
        let mut at = r.offset;
        for c in r.children() {
            assert_eq!(c.offset, at);
            at = c.end();
        }
        assert_eq!(at, r.end());
        last = r.end();
    }
    assert_eq!(last, DOC.len());
}

fn check_edit(src: &str, start: usize, old_len: usize, text: &str) -> Splice {
    let mut t = Tree::parse(src);
    let before: Vec<ParaId> = t.paras().iter().map(|p| p.id).collect();
    let mut new = src.to_string();
    new.replace_range(start..start + old_len, text);
    let s = t.edit(start, old_len, text);
    assert_eq!(t.text(), new);
    let fresh = Tree::parse(&new);
    assert_eq!(t.para_ends(), fresh.para_ends());
    // (the paragraphs outside the splice kept their identity)
    let after: Vec<ParaId> = t.paras().iter().map(|p| p.id).collect();
    assert_eq!(after[..s.at], before[..s.at]);
    assert_eq!(after[s.at + s.inserted..], before[s.at + s.removed..]);
    s
}

#[test]
fn edits() {
    let src = "a\n\nb\n\nc\n\nd\n\ne\n";
    // a word inside `c`: `b` (the one before) is cut again, but its text
    // came back: only `c` is new
    let s = check_edit(src, 6, 0, "cc");
    assert_eq!((s.at, s.removed, s.inserted), (2, 1, 1));
    // the blank line after `b` deleted: `b` and `c` become one
    let s = check_edit(src, 4, 1, "");
    assert_eq!(s.inserted + 1, s.removed);
    // a `{` opened in `b`: the rest is one paragraph
    check_edit(src, 3, 0, "{");
    // a new paragraph
    check_edit(src, 3, 0, "\n\nnew");
    // at the ends
    check_edit(src, 0, 0, "x\n\n");
    check_edit(src, src.len(), 0, "\nf\n");
    check_edit("", 0, 0, "only\n");
    check_edit(src, 0, src.len(), "");
}

/// Edits that open and close verbatim runs: the tree is the fresh
/// parse's, and the paragraphs outside the splice are kept.
#[test]
fn verbatim_edits() {
    let src = "a\n\nb\n\n\\begin{verbatim}\n{\n\nx\n\\end{verbatim}\n\nc\n\nd\n";
    assert_eq!(Tree::parse(src).paras().len(), 5);
    // the verbatim's end removed: it runs to the end
    let at = src.find("\\end").unwrap();
    check_edit(src, at, 1, "");
    // a `\verb|{|` typed in `b`, then its delimiter removed
    check_edit(src, 3, 0, "\\verb|{|");
    check_edit(src, 3, 0, "\\verb|{");
    // a verbatim opened in `a`, before an existing end
    check_edit(src, 0, 0, "\\begin{verbatim}");
    check_edit(src, 0, 1, "\\begin{comment}");
}

/// Random edits of a document made of the pieces that move paragraph
/// boundaries: the edited tree is always the fresh parse's.
#[test]
fn random_edits() {
    const PIECES: &[&str] = &[
        "w",
        " ",
        "\n",
        "\n\n",
        "{",
        "}",
        "%",
        "\\verb|",
        "|",
        "\\begin{verbatim}",
        "\\end{verbatim}",
        "\\url{",
        "\\x",
        "\\begin{lstlisting}[",
        "]",
        "\\end{lstlisting}",
    ];
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let mut rand = |n: usize| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        usize::try_from(seed % (n as u64)).unwrap()
    };
    let mut src = String::new();
    for _ in 0..200 {
        src.push_str(PIECES[rand(PIECES.len())]);
    }
    let mut t = Tree::parse(&src);
    for _ in 0..500 {
        let start = rand(src.len() + 1);
        let old_len = rand((src.len() - start).min(12) + 1);
        let mut text = String::new();
        for _ in 0..rand(3) {
            text.push_str(PIECES[rand(PIECES.len())]);
        }
        let before: Vec<ParaId> = t.paras().iter().map(|p| p.id).collect();
        let s = t.edit(start, old_len, &text);
        src.replace_range(start..start + old_len, &text);
        assert_eq!(t.text(), src);
        let fresh = Tree::parse(&src);
        assert_eq!(t.para_ends(), fresh.para_ends(), "{src:?}");
        for (p, f) in t.paras().iter().zip(fresh.paras()) {
            assert_eq!(p.green, f.green);
        }
        let after: Vec<ParaId> = t.paras().iter().map(|p| p.id).collect();
        assert_eq!(after[..s.at], before[..s.at]);
        assert_eq!(after[s.at + s.inserted..], before[s.at + s.removed..]);
    }
}
