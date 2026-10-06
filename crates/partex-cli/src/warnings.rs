//! A build's diagnostics sorted for the terminal (DESIGN.md, "Command line
//! and terminal"): errors in order, warnings grouped and deduplicated, and
//! the rest of what the document showed on the terminal.
//!
//! Box warnings come from the engine as data. Warnings that LaTeX and its
//! packages write (undefined references and citations, font
//! substitutions, `Package x Warning: …`) are text a macro wrote to the
//! terminal: each arrives as one note (one `\write` or `\message`), and is
//! recognized by its standard form.

use std::fmt::Write as _;

use partex_core::diag::{Diagnostic, FrameKind, Severity};

/// A place in a source file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Loc {
    pub file: String,
    pub line: i32,
}

impl std::fmt::Display for Loc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.file.is_empty() {
            write!(f, "line {}", self.line)
        } else {
            write!(f, "{}:{}", self.file, self.line)
        }
    }
}

/// What a group of warnings is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    OverfullHbox,
    OverfullVbox,
    UnderfullHbox,
    UnderfullVbox,
    TightHbox,
    TightVbox,
    LooseHbox,
    LooseVbox,
    UndefinedReference,
    UndefinedCitation,
    FontSubstitution,
    /// Any other warning a document wrote (grouped by its text).
    Other,
}

impl Kind {
    fn of_code(code: &str) -> Option<Kind> {
        Some(match code {
            "overfull-hbox" => Kind::OverfullHbox,
            "overfull-vbox" => Kind::OverfullVbox,
            "underfull-hbox" => Kind::UnderfullHbox,
            "underfull-vbox" => Kind::UnderfullVbox,
            "tight-hbox" => Kind::TightHbox,
            "tight-vbox" => Kind::TightVbox,
            "loose-hbox" => Kind::LooseHbox,
            "loose-vbox" => Kind::LooseVbox,
            _ => return None,
        })
    }

    fn is_box(self) -> bool {
        self < Kind::UndefinedReference
    }

    fn overfull(self) -> bool {
        matches!(self, Kind::OverfullHbox | Kind::OverfullVbox)
    }

    /// A stable id (`--message-format=json`, `phitex why`).
    pub fn code(self) -> &'static str {
        match self {
            Kind::OverfullHbox => "overfull-hbox",
            Kind::OverfullVbox => "overfull-vbox",
            Kind::UnderfullHbox => "underfull-hbox",
            Kind::UnderfullVbox => "underfull-vbox",
            Kind::TightHbox => "tight-hbox",
            Kind::TightVbox => "tight-vbox",
            Kind::LooseHbox => "loose-hbox",
            Kind::LooseVbox => "loose-vbox",
            Kind::UndefinedReference => "undefined-reference",
            Kind::UndefinedCitation => "undefined-citation",
            Kind::FontSubstitution => "font-substitution",
            Kind::Other => "warning",
        }
    }

    /// `n` of this, in words.
    fn count(self, n: usize) -> String {
        let (one, many) = match self {
            Kind::OverfullHbox => ("overfull \\hbox", "overfull \\hboxes"),
            Kind::OverfullVbox => ("overfull \\vbox", "overfull \\vboxes"),
            Kind::UnderfullHbox => ("underfull \\hbox", "underfull \\hboxes"),
            Kind::UnderfullVbox => ("underfull \\vbox", "underfull \\vboxes"),
            Kind::TightHbox => ("tight \\hbox", "tight \\hboxes"),
            Kind::TightVbox => ("tight \\vbox", "tight \\vboxes"),
            Kind::LooseHbox => ("loose \\hbox", "loose \\hboxes"),
            Kind::LooseVbox => ("loose \\vbox", "loose \\vboxes"),
            Kind::UndefinedReference => ("undefined reference", "undefined references"),
            Kind::UndefinedCitation => ("undefined citation", "undefined citations"),
            Kind::FontSubstitution => ("font substitution", "font substitutions"),
            Kind::Other => ("warning", "warnings"),
        };
        format!("{n} {}", if n == 1 { one } else { many })
    }
}

/// One warning, or several that say the same thing (a label referred to
/// in several places).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    /// What it is about: the label, the font, the box report, the text.
    pub subject: String,
    pub locations: Vec<Loc>,
    /// For a box, how much too wide or high (sp), or its badness.
    pub amount: i32,
    /// For an hbox, its contents as TeX shows them.
    pub excerpt: String,
}

/// Warnings of one kind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    pub kind: Kind,
    pub items: Vec<Item>,
}

impl Group {
    /// How many warnings (each location of each item).
    pub fn len(&self) -> usize {
        self.items.iter().map(|i| i.locations.len().max(1)).sum()
    }

    /// The group's headline: `3 overfull \hboxes (worst 12.4pt)`.
    pub fn title(&self) -> String {
        let n = self.len();
        let mut t = self.kind.count(n);
        if self.kind.is_box()
            && let Some(worst) = self.items.iter().map(|i| i.amount).max()
        {
            if self.kind.overfull() {
                let _ = write!(t, " (worst {}pt)", pt(worst));
            } else if n > 1 {
                let _ = write!(t, " (worst badness {worst})");
            }
        } else if matches!(
            self.kind,
            Kind::UndefinedReference | Kind::UndefinedCitation
        ) {
            let names: Vec<String> = self
                .items
                .iter()
                .map(|i| format!("`{}`", i.subject))
                .collect();
            let _ = write!(t, ": {}", names.join(", "));
        }
        t
    }
}

/// A build's diagnostics, sorted.
#[derive(Debug, Default)]
pub struct Summary {
    /// Errors (and a fatal one), in order.
    pub errors: Vec<Diagnostic>,
    pub groups: Vec<Group>,
    /// What else the document showed on the terminal (`\message`,
    /// `\typeout`), in order.
    pub messages: Vec<String>,
    /// Pages shipped out.
    pub pages: usize,
}

impl Summary {
    pub fn warnings(&self) -> usize {
        self.groups.iter().map(Group::len).sum()
    }
}

/// The innermost file frame's name and line.
fn location(d: &Diagnostic) -> Loc {
    d.frames
        .iter()
        .find_map(|f| match &f.kind {
            FrameKind::File { name, line } => Some(Loc {
                file: display_name(&String::from_utf8_lossy(name)),
                line: *line,
            }),
            _ => None,
        })
        .unwrap_or(Loc {
            file: String::new(),
            line: 0,
        })
}

/// A file name as the user wrote it (`./x.tex` is `x.tex`).
fn display_name(name: &str) -> String {
    name.strip_prefix("./").unwrap_or(name).to_owned()
}

/// A LaTeX warning's text on one line: `\MessageBreak` continuations
/// (`(hyperref)       more`) joined with spaces.
fn joined(text: &str) -> String {
    let mut out = String::new();
    for (i, line) in text.trim_matches('\n').split('\n').enumerate() {
        let line = if i > 0 {
            let t = line.trim_start();
            let t = match t.strip_prefix('(').and_then(|r| r.split_once(')')) {
                Some((tag, rest)) if !tag.contains(' ') => rest,
                _ => t,
            };
            out.push(' ');
            t.trim_start()
        } else {
            line
        };
        out.push_str(line.trim_end());
    }
    out
}

/// The text between the first `` ` `` and the next `'` after `after`.
fn quoted(text: &str, after: &str) -> Option<String> {
    let rest = &text[text.find(after)? + after.len()..];
    let rest = rest.trim_start().strip_prefix('`')?;
    Some(rest[..rest.find('\'')?].to_owned())
}

/// The `on input line N` of a LaTeX warning.
fn input_line(text: &str) -> Option<i32> {
    let rest = &text[text.rfind("on input line ")? + 14..];
    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

/// A warning a document wrote, recognized: its kind and subject.
fn classify(text: &str) -> Option<(Kind, String)> {
    let t = joined(text);
    let warning = t
        .split_once(" Warning: ")
        .filter(|(who, _)| {
            *who == "LaTeX"
                || *who == "LaTeX Font"
                || who.starts_with("Package ")
                || who.starts_with("Class ")
        })
        .map(|(_, what)| what);
    let what = warning?;
    if what.starts_with("Reference `") && what.contains("undefined") {
        return Some((Kind::UndefinedReference, quoted(what, "Reference")?));
    }
    if what.starts_with("Citation `") && what.contains("undefined") {
        return Some((Kind::UndefinedCitation, quoted(what, "Citation")?));
    }
    if what.starts_with("Font shape `") && what.contains(" using `") {
        let from = quoted(what, "Font shape")?;
        let to = quoted(what, " using")?;
        return Some((Kind::FontSubstitution, format!("{from} -> {to}")));
    }
    // (the end-of-run summaries of the warnings above)
    let summary = [
        "There were undefined references",
        "There were undefined citations",
        "Some font shapes were not available",
    ];
    if summary.iter().any(|s| what.starts_with(s)) {
        return Some((Kind::Other, String::new()));
    }
    Some((Kind::Other, t))
}

/// Sort a build's diagnostics.
#[must_use]
pub fn summarize(diags: &[Diagnostic]) -> Summary {
    let mut sum = Summary::default();
    for d in diags {
        match d.severity {
            Severity::Error | Severity::Fatal => sum.errors.push(d.clone()),
            Severity::Warning => {
                let Some(kind) = Kind::of_code(d.code) else {
                    continue;
                };
                let b = d.boxed.as_ref();
                let loc = location(d);
                add(
                    &mut sum.groups,
                    kind,
                    Item {
                        subject: String::from_utf8_lossy(&d.message).into_owned(),
                        locations: vec![Loc {
                            line: b.map_or(loc.line, |b| b.lines.0),
                            ..loc
                        }],
                        amount: b.map_or(0, |b| b.amount),
                        excerpt: b.map_or_else(String::new, |b| {
                            String::from_utf8_lossy(&b.excerpt).into_owned()
                        }),
                    },
                );
            }
            Severity::Note if d.code == crate::events::PAGE => sum.pages += 1,
            Severity::Note => {
                let text = String::from_utf8_lossy(&d.message).into_owned();
                match classify(&text) {
                    Some((_, subject)) if subject.is_empty() => {}
                    Some((kind, subject)) => {
                        let mut loc = location(d);
                        if let Some(line) = input_line(&text) {
                            loc.line = line;
                        }
                        let item = Item {
                            subject,
                            locations: vec![loc],
                            amount: 0,
                            excerpt: String::new(),
                        };
                        add(&mut sum.groups, kind, item);
                    }
                    None => sum.messages.push(text.trim_matches('\n').to_owned()),
                }
            }
        }
    }
    sum.groups.sort_by_key(|g| g.kind);
    sum
}

/// Add `item` to its group: box warnings each count, other warnings with
/// the same subject are one item with several locations.
fn add(groups: &mut Vec<Group>, kind: Kind, item: Item) {
    let g = if let Some(g) = groups.iter_mut().find(|g| g.kind == kind) {
        g
    } else {
        groups.push(Group {
            kind,
            items: Vec::new(),
        });
        groups.last_mut().expect("just pushed")
    };
    if !kind.is_box()
        && let Some(same) = g.items.iter_mut().find(|i| i.subject == item.subject)
    {
        for l in item.locations {
            if !same.locations.contains(&l) {
                same.locations.push(l);
            }
        }
        return;
    }
    g.items.push(item);
}

/// `sp` in points as TeX prints them (§103).
#[must_use]
pub fn pt(sp: i32) -> String {
    const UNITY: i64 = 65536;
    let mut out = String::new();
    let mut s = i64::from(sp);
    if s < 0 {
        out.push('-');
        s = -s;
    }
    out.push_str(&(s / UNITY).to_string());
    out.push('.');
    s = 10 * (s % UNITY) + 5;
    let mut delta = 10;
    loop {
        if delta > UNITY {
            s += 0o100_000 - 50000; // round the last digit
        }
        out.push(char::from(b'0' + u8::try_from(s / UNITY).unwrap_or(0)));
        s = 10 * (s % UNITY);
        delta *= 10;
        if s <= delta {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use partex_core::diag::{BoxWarning, Frame};

    fn file(name: &str, line: i32) -> Frame {
        Frame {
            kind: FrameKind::File {
                name: name.as_bytes().to_vec(),
                line,
            },
            before: Vec::new(),
            after: Vec::new(),
        }
    }

    fn diag(severity: Severity, code: &'static str, message: &str, at: Frame) -> Diagnostic {
        Diagnostic {
            severity,
            code,
            message: message.as_bytes().to_vec(),
            help: Vec::new(),
            frames: vec![at],
            suggestions: Vec::new(),
            boxed: None,
        }
    }

    fn overfull(excess: i32, line: i32) -> Diagnostic {
        let mut d = diag(
            Severity::Warning,
            "overfull-hbox",
            "Overfull \\hbox",
            file("./ch02.tex", line + 2),
        );
        d.boxed = Some(BoxWarning {
            lines: (line, line + 2),
            amount: excess,
            excerpt: b"\\tenrm the Nesterov".to_vec(),
        });
        d
    }

    #[test]
    fn points_as_tex_prints_them() {
        assert_eq!(pt(0), "0.0");
        assert_eq!(pt(65536), "1.0");
        assert_eq!(pt(812_646), "12.4");
        assert_eq!(pt(812_600), "12.39929");
        assert_eq!(pt(-32768), "-0.5");
    }

    #[test]
    fn box_warnings_group_with_the_worst() {
        let s = summarize(&[overfull(100_000, 88), overfull(812_646, 140)]);
        assert_eq!(s.groups.len(), 1);
        let g = &s.groups[0];
        assert_eq!(g.title(), "2 overfull \\hboxes (worst 12.4pt)");
        assert_eq!(
            g.items[0].locations,
            [Loc {
                file: "ch02.tex".into(),
                line: 88
            }]
        );
    }

    #[test]
    fn latex_warnings_are_recognized_and_deduplicated() {
        let note = |text: &str, line| diag(Severity::Note, "write", text, file("./a.tex", line));
        let s = summarize(&[
            note(
                "\nLaTeX Warning: Reference `fig:x' on page 1 undefined on input line 57.\n",
                57,
            ),
            note(
                "LaTeX Warning: Reference `fig:x' on page 2 undefined on input line 80.",
                80,
            ),
            note(
                "LaTeX Warning: Citation `knuth84' on page 1 undefined on input line 3.",
                3,
            ),
            note(
                "LaTeX Font Warning: Font shape `OT1/cmr/bx/sc' undefined\n(Font)              using `OT1/cmr/bx/n' instead on input line 9.",
                9,
            ),
            note(
                "Package hyperref Warning: Token not allowed in a PDF string\n(hyperref)                removing `math shift' on input line 12.",
                12,
            ),
            note("LaTeX Warning: There were undefined references.", 99),
            note("(./a.aux)", 1),
            diag(Severity::Note, "message", "Hello", file("./a.tex", 4)),
            diag(Severity::Note, crate::events::PAGE, "1", file("./a.tex", 4)),
        ]);
        let titles: Vec<String> = s.groups.iter().map(Group::title).collect();
        assert_eq!(
            titles,
            [
                "2 undefined references: `fig:x`",
                "1 undefined citation: `knuth84`",
                "1 font substitution",
                "1 warning",
            ]
        );
        assert_eq!(
            s.groups[0].items[0].locations,
            [
                Loc {
                    file: "a.tex".into(),
                    line: 57
                },
                Loc {
                    file: "a.tex".into(),
                    line: 80
                }
            ]
        );
        assert_eq!(
            s.groups[2].items[0].subject,
            "OT1/cmr/bx/sc -> OT1/cmr/bx/n"
        );
        assert_eq!(
            s.groups[3].items[0].subject,
            "Package hyperref Warning: Token not allowed in a PDF string removing `math shift' on input line 12."
        );
        assert_eq!(s.messages, ["(./a.aux)", "Hello"]);
        assert_eq!(s.pages, 1);
    }
}
