//! The views against a build's own files: its `.aux`'s labels and its
//! `.toc`'s entries. What a static reading of the source says and what
//! TeX wrote should agree; each difference has a cause to report.

use crate::views::{Loc, Views};
use crate::{Project, text};

/// Labels: the source's against the `.aux`'s `\newlabel`s.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LabelComparison {
    /// Keys in both.
    pub matched: usize,
    /// cleveref's `key@cref` twins, not compared.
    pub cleveref: usize,
    /// In the `.aux` only (a package's own labels, or a stale `.aux`).
    pub only_aux: Vec<String>,
    /// In the source only.
    pub only_source: Vec<(String, Loc)>,
}

/// The `\newlabel`s of an `.aux` against the views' labels.
#[must_use]
pub fn compare_labels(v: &Views<'_>, aux: &str) -> LabelComparison {
    let mut c = LabelComparison::default();
    let mut seen = std::collections::HashSet::new();
    for line in aux.lines() {
        let Some(rest) = line.strip_prefix("\\newlabel{") else {
            continue;
        };
        let Some(key) = group_text(&format!("{{{rest}")) else {
            continue;
        };
        if key.ends_with("@cref") {
            c.cleveref += 1;
            continue;
        }
        seen.insert(key.clone());
        if v.has_label(&key) {
            c.matched += 1;
        } else {
            c.only_aux.push(key);
        }
    }
    for l in &v.labels {
        if !seen.contains(l.key) {
            c.only_source.push((l.key.to_owned(), l.loc));
        }
    }
    c
}

/// An entry of a table of contents: its level, number and title.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub level: String,
    pub number: Option<String>,
    pub title: String,
    /// Where the source has it (the views' entries).
    pub loc: Option<Loc>,
}

/// The table of contents: the source's against the `.toc`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TocComparison {
    pub matched: usize,
    /// Entries that differ, in order: the source's and the `.toc`'s.
    pub diffs: Vec<(Option<Entry>, Option<Entry>)>,
}

/// The `\contentsline`s of a `.toc` against the views' `toc` entries,
/// titles compared as LaTeX writes them (the document's macros expanded,
/// white space and `\protect` aside).
#[must_use]
pub fn compare_toc(project: &Project, views: &Views<'_>, toc: &str) -> TocComparison {
    let ours: Vec<Entry> = views
        .toc
        .iter()
        .filter(|t| t.list == "toc")
        .map(|t| Entry {
            level: t.level.to_owned(),
            number: t.number.clone(),
            title: project.written_title(t.title),
            loc: Some(t.loc),
        })
        .collect();
    let theirs: Vec<Entry> = toc.lines().filter_map(contentsline).collect();
    let same = |x: &Entry, y: &Entry| {
        x.level == y.level && x.number == y.number && squash(&x.title) == squash(&y.title)
    };
    let mut out = TocComparison::default();
    let (mut mine, mut its) = (0, 0);
    while mine < ours.len() || its < theirs.len() {
        match (ours.get(mine), theirs.get(its)) {
            (Some(x), Some(y)) if same(x, y) => {
                out.matched += 1;
                mine += 1;
                its += 1;
            }
            (Some(x), Some(y)) => {
                // (one side has entries the other has not: the nearest
                // match ahead decides which)
                let skip_ours = (1..8).find(|&n| ours.get(mine + n).is_some_and(|e| same(e, y)));
                let skip_theirs = (1..8).find(|&n| theirs.get(its + n).is_some_and(|e| same(x, e)));
                match (skip_ours, skip_theirs) {
                    (Some(n), other) if other.is_none_or(|m| n <= m) => {
                        for e in &ours[mine..mine + n] {
                            out.diffs.push((Some(e.clone()), None));
                        }
                        mine += n;
                    }
                    (_, Some(n)) => {
                        for e in &theirs[its..its + n] {
                            out.diffs.push((None, Some(e.clone())));
                        }
                        its += n;
                    }
                    _ => {
                        out.diffs.push((Some(x.clone()), Some(y.clone())));
                        mine += 1;
                        its += 1;
                    }
                }
            }
            (Some(x), None) => {
                out.diffs.push((Some(x.clone()), None));
                mine += 1;
            }
            (None, Some(y)) => {
                out.diffs.push((None, Some(y.clone())));
                its += 1;
            }
            (None, None) => break,
        }
    }
    out
}

/// A title compared: no white space, no `\protect`.
fn squash(s: &str) -> String {
    s.replace("\\protect", "")
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect()
}

/// `\contentsline {level}{\numberline {N}title}{page}{anchor}%` (a part's
/// entry is `N\hspace {1em}title`).
fn contentsline(line: &str) -> Option<Entry> {
    let rest = line.strip_prefix("\\contentsline ")?;
    let level = group_text(rest)?;
    let rest = &rest[level.len() + 2..];
    let entry = group_text(rest)?;
    let (number, title) = if let Some(r) = entry.strip_prefix("\\numberline {") {
        let n = group_text(&format!("{{{r}"))?;
        (Some(n.clone()), r[n.len() + 1..].to_owned())
    } else if level == "part"
        && let Some((n, t)) = entry.split_once("\\hspace {1em}")
    {
        (Some(n.to_owned()), t.to_owned())
    } else {
        (None, entry)
    };
    Some(Entry {
        level,
        number,
        title,
        loc: None,
    })
}

/// The text of the brace group `s` begins with.
fn group_text(s: &str) -> Option<String> {
    let s = s.strip_prefix('{')?;
    let mut depth = 1usize;
    for (i, c) in s.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(s[..i].to_owned());
                }
            }
            _ => {}
        }
    }
    None
}

impl Project {
    /// A title as LaTeX writes it to the `.toc`: the document's macros
    /// expanded (not the robust ones), then as `\write` prints it.
    #[must_use]
    pub fn written_title(&self, title: &str) -> String {
        text::written(&crate::scan::expand_text(&self.defs, title, 0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toc_lines() {
        let e = contentsline(
            "\\contentsline {section}{\\numberline {0.1}Two {problems}}{1}{section.0.1}%",
        )
        .unwrap();
        assert_eq!(
            (e.level.as_str(), e.number.as_deref(), e.title.as_str()),
            ("section", Some("0.1"), "Two {problems}")
        );
        let e = contentsline("\\contentsline {part}{I\\hspace {1em}The language}{11}{part.1}%")
            .unwrap();
        assert_eq!(
            (e.number.as_deref(), e.title.as_str()),
            (Some("I"), "The language")
        );
        let e = contentsline("\\contentsline {paragraph}{Why?}{9}{paragraph*.14}%").unwrap();
        assert_eq!((e.number, e.title.as_str()), (None, "Why?"));
    }
}
