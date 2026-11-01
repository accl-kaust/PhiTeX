//! A document as paragraphs with their syntax trees: a [`Source`] cut by
//! [`Paragraphs`], and each unit's green tree ([`lex`]). An edit cuts
//! again what it touched ([`Source::edit`]) and lexes only the new
//! paragraphs; a paragraph whose text came back keeps its identity and its
//! tree.
//!
//! What reads the source without running TeX (the outline, latexdiff)
//! keeps per-paragraph data in arrays parallel to [`Tree::paras`]; an edit
//! says which run of them changed ([`Splice`]).

use std::rc::Rc;
use std::sync::Arc;

use crate::syntax::{Green, Paragraphs, Red, lex};
use crate::{Pos, Source, UnitId};

/// A paragraph's identity: its unit's.
pub type ParaId = UnitId;

/// A paragraph: its identity, its text and its green tree.
#[derive(Clone, Debug)]
pub struct Para {
    pub id: ParaId,
    text: Arc<str>,
    pub green: Rc<Green>,
}

impl Para {
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }
}

/// What an edit did to the paragraphs: `removed` of them, from index `at`,
/// were replaced by `inserted` new ones.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Splice {
    pub at: usize,
    pub removed: usize,
    pub inserted: usize,
}

/// The document: its source, cut into paragraphs, and their trees in order.
#[derive(Clone, Debug)]
pub struct Tree {
    src: Source,
    paras: Vec<Para>,
    /// Where each paragraph ends.
    ends: Vec<usize>,
}

impl Default for Tree {
    fn default() -> Tree {
        Tree::parse("")
    }
}

impl Tree {
    #[must_use]
    pub fn parse(text: &str) -> Tree {
        let src = Source::with_split(text, Arc::new(Paragraphs));
        let paras = src.units().map(|u| para(&src, u)).collect();
        let mut t = Tree {
            src,
            paras,
            ends: Vec::new(),
        };
        t.recount(0);
        t
    }

    /// The source the paragraphs are units of.
    #[must_use]
    pub fn source(&self) -> &Source {
        &self.src
    }

    #[must_use]
    pub fn paras(&self) -> &[Para] {
        &self.paras
    }

    /// The document's length in bytes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.ends.last().copied().unwrap_or(0)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.paras.is_empty()
    }

    /// The document's text (the tree is lossless: it is the source).
    #[must_use]
    pub fn text(&self) -> String {
        self.src.to_text()
    }

    /// Where each paragraph ends.
    #[must_use]
    pub fn para_ends(&self) -> &[usize] {
        &self.ends
    }

    /// The paragraphs as red nodes.
    pub fn reds(&self) -> impl Iterator<Item = Red<'_>> {
        let mut at = 0;
        self.paras.iter().map(move |p| {
            let r = Red {
                green: &p.green,
                offset: at,
            };
            at += p.green.len();
            r
        })
    }

    /// Replace `old_len` bytes at `start` with `text`: the source cuts
    /// again what the edit touched, and the paragraphs it made are lexed;
    /// the others keep their identity and their trees.
    ///
    /// # Panics
    ///
    /// If the bytes are not within the document, on character boundaries.
    pub fn edit(&mut self, start: usize, old_len: usize, text: &str) -> Splice {
        if self.paras.is_empty() {
            // (an empty text has no unit to edit in)
            assert!(start == 0 && old_len == 0, "a range within the document");
            *self = Tree::parse(text);
            return Splice {
                at: 0,
                removed: 0,
                inserted: self.paras.len(),
            };
        }
        let (a, b) = (self.pos_at(start), self.pos_at(start + old_len));
        let s = self.src.edit(a, b, text);
        let at = match (s.removed.first(), s.inserted.first()) {
            (Some(r), _) => self
                .paras
                .iter()
                .position(|p| p.id == *r)
                .expect("a removed unit was a paragraph"),
            (None, Some(&i)) => self.src.index(i),
            (None, None) => return Splice::default(),
        };
        let made: Vec<Para> = s.inserted.iter().map(|&u| para(&self.src, u)).collect();
        let (removed, inserted) = (s.removed.len(), made.len());
        self.paras.splice(at..at + removed, made);
        self.recount(at);
        Splice {
            at,
            removed,
            inserted,
        }
    }

    /// The position at byte `b` (the end of the document: the end of its
    /// last paragraph).
    fn pos_at(&self, b: usize) -> Pos {
        let i = self
            .ends
            .partition_point(|&e| e <= b)
            .min(self.paras.len() - 1);
        let begin = if i == 0 { 0 } else { self.ends[i - 1] };
        self.src.pos(self.paras[i].id, b - begin)
    }

    /// `ends` again from paragraph `at` on.
    fn recount(&mut self, at: usize) {
        self.ends.truncate(at);
        let mut e = at.checked_sub(1).map_or(0, |i| self.ends[i]);
        for p in &self.paras[at..] {
            e += p.text.len();
            self.ends.push(e);
        }
    }
}

fn para(src: &Source, u: UnitId) -> Para {
    let text = Arc::clone(src.text(u));
    Para {
        id: u,
        green: Rc::new(lex(&text)),
        text,
    }
}

#[cfg(test)]
mod tests;
