//! The outline as an editor shows it (DESIGN 4.4, "The outline"): each
//! heading with its place in the source, known before any build, and its
//! place in the PDF once a build's glyph origins are given
//! ([`Outline::place`]).
//!
//! A heading's PDF place is that of the first glyph, in page order, whose
//! origin falls inside the heading's title in the source (not
//! synthesized): the heading itself, before any running head or table of
//! contents shows the title again (those come from the `.toc` or a mark,
//! not from the title's bytes). Its point is the glyph's origin on the
//! page, in PDF points from the bottom left, which the caller computes
//! from the PDF (`partex_engine::pdftext` places every code a page shows,
//! in the order glyph origins are given in).

use crate::{Project, text};

/// A heading of the outline.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    /// `\part` −1, `\chapter` 0, `\section` 1, … `\subparagraph` 5.
    pub level: i8,
    /// As written in the source, its markup shown as text.
    pub title: String,
    /// Its number as LaTeX gives it (`None`: unnumbered).
    pub number: Option<String>,
    /// Its file, as the project names it (relative to the main file's
    /// directory).
    pub file: String,
    /// Its bytes in the file: the command, `\section` to the title's
    /// closing brace (or the call of the document's macro it came from).
    pub start: u32,
    pub end: u32,
    /// Its title's bytes (`start == end`: not in the text, read through a
    /// macro).
    pub title_start: u32,
    pub title_end: u32,
    /// Its line (from 1).
    pub line: u32,
    /// Where the PDF shows it ([`Outline::place`]): the page (from 0),
    /// and its first glyph's origin, in points from the page's bottom
    /// left.
    pub page: Option<usize>,
    pub x: Option<f64>,
    pub y: Option<f64>,
}

/// A glyph's origin, as a build gives it (`partex_core::GlyphOrigin`):
/// bytes `start..end` of file `file` (an index into the build's file
/// names), `synthesized` when the range is a call's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GlyphRef {
    pub file: u32,
    pub start: u32,
    pub end: u32,
    pub synthesized: bool,
}

/// The outline: its entries in reading order, and what
/// [`Outline::section_at`] needs.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Outline {
    pub entries: Vec<Entry>,
    /// Each file's first reading: its path, and the entries before it.
    entered: Vec<(String, usize)>,
}

impl Project {
    /// The outline, from the source alone ([`Outline::place`] adds where
    /// the PDF shows each heading).
    #[must_use]
    pub fn outline(&self) -> Outline {
        let v = self.views();
        let entries = v
            .outline
            .iter()
            .map(|h| {
                let (title_start, title_end) = h.title_range.unwrap_or((h.start, h.start));
                Entry {
                    level: h.level,
                    title: text::display(h.title),
                    number: h.number.clone(),
                    file: self.path(h.loc.file).to_owned(),
                    start: h.start,
                    end: h.end,
                    title_start,
                    title_end,
                    line: h.loc.line,
                    page: None,
                    x: None,
                    y: None,
                }
            })
            .collect();
        let mut entered: Vec<(String, usize)> = Vec::new();
        for &(f, n) in &v.entered {
            let path = self.path(f);
            if !entered.iter().any(|(p, _)| p == path) {
                entered.push((path.to_owned(), n));
            }
        }
        Outline { entries, entered }
    }
}

/// Whether a build's file name `build` (as the job asked for it) is the
/// project's file `path`.
fn same_file(build: &str, path: &str) -> bool {
    let b = build.strip_prefix("./").unwrap_or(build);
    let p = path.strip_prefix("./").unwrap_or(path);
    b == p || p.strip_suffix(".tex") == Some(b)
}

impl Outline {
    /// The entry whose section holds byte `offset` of file `file` (the
    /// innermost: the last heading at or before it in reading order), if
    /// any heading comes before it.
    #[must_use]
    pub fn section_at(&self, file: &str, offset: u32) -> Option<usize> {
        let file = text::normalize(file);
        if let Some(i) = self
            .entries
            .iter()
            .rposition(|e| e.file == file && e.start <= offset)
        {
            return Some(i);
        }
        // (none in the file before it: the last before the file is read)
        let (_, before) = self.entered.iter().find(|(p, _)| *p == file)?;
        before.checked_sub(1)
    }

    /// Where the PDF shows each heading: `files`, the build's file names
    /// (`Tex::origin_files`); `pages`, each page's glyphs' origins
    /// (`Tex::origins`); `position(page, glyph)`, that glyph's origin on
    /// its page, in points from the bottom left. An entry whose title
    /// no glyph comes from has no place.
    pub fn place(
        &mut self,
        files: &[String],
        pages: &[Vec<GlyphRef>],
        position: &mut dyn FnMut(usize, usize) -> Option<(f64, f64)>,
    ) {
        // (each build file's titles, by their start: entry indices)
        let mut titles: Vec<Vec<(u32, u32, usize)>> = vec![Vec::new(); files.len()];
        for (i, e) in self.entries.iter_mut().enumerate() {
            (e.page, e.x, e.y) = (None, None, None);
            if e.title_start == e.title_end {
                continue;
            }
            if let Some(f) = files.iter().position(|b| same_file(b, &e.file)) {
                titles[f].push((e.title_start, e.title_end, i));
            }
        }
        for t in &mut titles {
            t.sort_unstable();
        }
        let mut left: usize = titles.iter().map(Vec::len).sum();
        for (n, glyphs) in pages.iter().enumerate() {
            if left == 0 {
                break;
            }
            for (k, g) in glyphs.iter().enumerate() {
                if g.synthesized {
                    continue;
                }
                let Some(t) = titles.get(g.file as usize) else {
                    continue;
                };
                let at = t.partition_point(|&(s, _, _)| s <= g.start);
                let Some(&(_, end, i)) = at.checked_sub(1).and_then(|j| t.get(j)) else {
                    continue;
                };
                let e = &mut self.entries[i];
                if g.end <= end && e.page.is_none() {
                    e.page = Some(n);
                    if let Some((x, y)) = position(n, k) {
                        (e.x, e.y) = (Some(x), Some(y));
                    }
                    left -= 1;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use super::*;
    use crate::Files;

    struct Mem(Vec<(&'static str, &'static str)>);
    impl Files for Mem {
        fn read(&self, path: &str) -> Option<String> {
            self.0
                .iter()
                .find(|(p, _)| *p == path)
                .map(|(_, t)| (*t).to_owned())
        }
        fn exists(&self, path: &str) -> bool {
            self.0.iter().any(|(p, _)| *p == path)
        }
    }

    const MAIN: &str = "\\documentclass{article}\n\\begin{document}\n\\section{One}\nText.\n\n\\input{part}\n\\section*{Three}\n\\end{document}\n";
    const PART: &str = "Before.\n\\subsection{Two}\nAfter.\n";

    fn project() -> Project {
        Project::open(
            "main.tex",
            Rc::new(Mem(vec![("main.tex", MAIN), ("part.tex", PART)])),
        )
    }

    #[test]
    fn entries_and_sections() {
        let o = project().outline();
        let titles: Vec<&str> = o.entries.iter().map(|e| e.title.as_str()).collect();
        assert_eq!(titles, ["One", "Two", "Three"]);
        let e = &o.entries[0];
        assert_eq!(&MAIN[e.start as usize..e.end as usize], "\\section{One}");
        assert_eq!(&MAIN[e.title_start as usize..e.title_end as usize], "One");
        assert_eq!((e.line, e.number.as_deref()), (3, Some("1")));
        let e = &o.entries[1];
        assert_eq!(e.file, "part.tex");
        assert_eq!(&PART[e.title_start as usize..e.title_end as usize], "Two");
        assert_eq!(e.number.as_deref(), Some("1.1"));
        assert_eq!(o.entries[2].number, None);
        // (the main file before the first heading: none; in its section)
        assert_eq!(o.section_at("main.tex", 0), None);
        let at = |s: &str, what: &str| u32::try_from(s.find(what).unwrap_or(0)).unwrap_or(0);
        let text = at(MAIN, "Text.");
        assert_eq!(o.section_at("main.tex", text), Some(0));
        // (the part's text before its heading: the section it is read in)
        assert_eq!(o.section_at("part.tex", 0), Some(0));
        let end = u32::try_from(PART.len()).unwrap_or(0) - 1;
        assert_eq!(o.section_at("part.tex", end), Some(1));
        let three = at(MAIN, "\\section*");
        assert_eq!(o.section_at("main.tex", three), Some(2));
    }

    #[test]
    fn places_titles() {
        let mut o = project().outline();
        let at = |s: &str, what: &str| u32::try_from(s.find(what).unwrap_or(0)).unwrap_or(0);
        let (one, two) = (at(MAIN, "One"), at(PART, "Two"));
        let g = |file, start, end, synthesized| GlyphRef {
            file,
            start,
            end,
            synthesized,
        };
        let files = vec!["main".to_owned(), "part.tex".to_owned()];
        let pages = vec![
            // (the number, synthesized; then "One")
            vec![
                g(0, one - 9, one + 4, true),
                g(0, one, one + 1, false),
                g(0, one + 1, one + 2, false),
            ],
            vec![g(1, 0, 1, false), g(1, two + 1, two + 2, false)],
        ];
        o.place(&files, &pages, &mut |n, k| {
            Some((f64::from(u32::try_from(n * 100 + k).unwrap_or(0)), 0.0))
        });
        let at: Vec<(Option<usize>, Option<f64>)> =
            o.entries.iter().map(|e| (e.page, e.x)).collect();
        assert_eq!(
            at,
            [(Some(0), Some(1.0)), (Some(1), Some(101.0)), (None, None)]
        );
    }
}
