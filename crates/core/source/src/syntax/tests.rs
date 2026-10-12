use super::*;
use crate::tree::{Para, Tree};

fn texts(t: &Tree) -> Vec<&str> {
    t.paras().iter().map(Para::text).collect()
}

/// The tokens of a text, depth first, with their text (groups by
/// their braces).
fn tokens(text: &str) -> Vec<(SyntaxKind, &str)> {
    fn walk<'a>(g: &Green, text: &'a str, at: &mut usize, out: &mut Vec<(SyntaxKind, &'a str)>) {
        match g {
            Green::Token { kind, len } => {
                out.push((*kind, &text[*at..*at + len]));
                *at += len;
            }
            Green::Node { children, .. } => {
                for c in children {
                    walk(c, text, at, out);
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(&lex(text), text, &mut 0, &mut out);
    out
}

/// `\verb`'s braces and `%` are characters: no group, no comment, and
/// the paragraph ends where it would without them.
#[test]
fn verb() {
    use SyntaxKind::{Brace, Comment, Cs, Newline, Space, Text, Verbatim};
    let src = "a \\verb|{%| b\n\nc \\verb*+}+\n";
    assert_eq!(
        texts(&Tree::parse(src)),
        ["a \\verb|{%| b\n\n", "c \\verb*+}+\n"]
    );
    assert_eq!(
        tokens("a \\verb|{%| b"),
        [
            (Text, "a"),
            (Space, " "),
            (Cs, "\\verb"),
            (Verbatim, "|{%|"),
            (Space, " "),
            (Text, "b")
        ]
    );
    // (no closing delimiter: to the end of the line, as LaTeX reads it
    // before its error)
    assert_eq!(
        tokens("\\verb|{x\n}"),
        [
            (Cs, "\\verb"),
            (Verbatim, "|{x"),
            (Newline, "\n"),
            (Text, "}")
        ]
        .map(|(k, t)| (if t == "}" { SyntaxKind::Other } else { k }, t))
    );
    // (a URL's `%` and `#`; `\href`'s text is read as usual)
    assert_eq!(
        tokens("\\href{a%20#b}{x}%c"),
        [
            (Cs, "\\href"),
            (Verbatim, "{a%20#b}"),
            (Brace, "{"),
            (Text, "x"),
            (Brace, "}"),
            (Comment, "%c")
        ]
    );
    assert_eq!(
        tokens("\\lstinline[language=C]{a{b}%}"),
        [(Cs, "\\lstinline"), (Verbatim, "[language=C]{a{b}%}")]
    );
    // (the lists say what is read)
    assert!(
        VERBATIM_COMMANDS
            .iter()
            .all(|c| verbatim_cmd(c.as_bytes()).is_some())
    );
    assert!(
        VERBATIM_ENVIRONMENTS
            .iter()
            .all(|e| verbatim_env_args(e.as_bytes()).is_some())
    );
    // (a control word that only begins like one is not)
    assert_eq!(tokens("\\verbatim{x}")[0], (Cs, "\\verbatim"));
}

/// A verbatim environment's body is one run: its blank lines do not
/// end the paragraph, its braces open nothing; its arguments are read
/// as usual.
#[test]
fn verbatim_environments() {
    use SyntaxKind::{Brace, Cs, Newline, Text, Verbatim};
    let src = "x\n\\begin{verbatim}\n{\n\n%}\n\\end{verbatim}\ny\n\nz\n";
    assert_eq!(
        texts(&Tree::parse(src)),
        [
            "x\n\\begin{verbatim}\n{\n\n%}\n\\end{verbatim}\ny\n\n",
            "z\n"
        ]
    );
    let toks = tokens("\\begin{lstlisting}[caption={a}]\nint x;{\n\\end{lstlisting}");
    assert_eq!(
        toks,
        [
            (Cs, "\\begin"),
            (Brace, "{"),
            (Text, "lstlisting"),
            (Brace, "}"),
            (Text, "[caption="),
            (Brace, "{"),
            (Text, "a"),
            (Brace, "}"),
            (Text, "]"),
            (Verbatim, "\nint x;{\n"),
            (Cs, "\\end"),
            (Brace, "{"),
            (Text, "lstlisting"),
            (Brace, "}"),
        ]
    );
    assert_eq!(
        tokens("\\begin{minted}{python}\nx\\end{minted}")[6..8],
        [(Brace, "}"), (Verbatim, "\nx")]
    );
    // (an empty body; a body never closed runs to the end)
    assert!(
        tokens("\\begin{comment}\\end{comment}")
            .iter()
            .all(|t| t.0 != Verbatim)
    );
    let src = "a\n\n\\begin{comment}\n\nb\n\nc\n";
    assert_eq!(
        texts(&Tree::parse(src)),
        ["a\n\n", "\\begin{comment}\n\nb\n\nc\n"]
    );
    // (`\begin{verbatim}` in a comment is a comment)
    let src = "% \\begin{verbatim}\n\nb\n";
    assert_eq!(texts(&Tree::parse(src)), ["% \\begin{verbatim}\n\n", "b\n"]);
    assert_eq!(tokens("a\n").last(), Some(&(Newline, "\n")));
}

/// The paragraph cutter as first written, line by line: the one-pass
/// cutter must agree with it.
fn para_end_by_lines(s: &[u8], start: usize) -> usize {
    let blank = |l: &[u8]| l.iter().all(|&c| matches!(c, b' ' | b'\t' | b'\r'));
    let mut i = start;
    let mut depth = 0usize;
    let mut text = false;
    loop {
        if i >= s.len() {
            return s.len();
        }
        let mut e = line_end(s, i);
        if blank(&s[i..e]) {
            if depth == 0 && text {
                let mut j = (e + 1).min(s.len());
                while j < s.len() {
                    let e2 = line_end(s, j);
                    if !blank(&s[j..e2]) {
                        break;
                    }
                    j = (e2 + 1).min(s.len());
                }
                return j;
            }
        } else {
            text = true;
            let mut c = i;
            while c < e {
                match s[c] {
                    b'\\' => {
                        if let Some(v) = verbatim_at(s, c) {
                            c = v.end;
                            if c > e {
                                e = line_end(s, c);
                            }
                            continue;
                        }
                        c += 1;
                    }
                    b'%' => break,
                    b'{' => depth += 1,
                    b'}' => depth = depth.saturating_sub(1),
                    _ => {}
                }
                c += 1;
            }
        }
        if e >= s.len() {
            return s.len();
        }
        i = e + 1;
    }
}

#[test]
fn one_pass_cutter() {
    const PIECES: &[&str] = &[
        "w",
        " ",
        "\t",
        "\n",
        "\n\n",
        " \n",
        "{",
        "}",
        "%",
        "\\",
        "\\verb|",
        "|",
        "\\begin{verbatim}",
        "\\end{verbatim}",
        "\\x",
        "\\{",
    ];
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    for _ in 0..2000 {
        let mut src = String::new();
        for _ in 0..40 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            src.push_str(PIECES[usize::try_from(seed % PIECES.len() as u64).unwrap()]);
        }
        let b = src.as_bytes();
        let mut pos = 0;
        while pos < b.len() {
            let (e, _) = para_end(b, pos);
            assert_eq!(e, para_end_by_lines(b, pos), "{src:?} from {pos}");
            pos = e;
        }
    }
}
