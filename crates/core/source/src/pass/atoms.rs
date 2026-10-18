//! Tokens as TeX reads them, one character at a time: what argument
//! readers and keyword scanners work on. Comments go, a line end is a
//! space (or a `\par` after a blank line), spaces after a control word go,
//! and under expl3's regime spaces go altogether. Braces know their mates.

use super::lex::{TokKind, Token};
use super::{GroupKind, GroupPair, Status};

/// What an atom is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AtomKind {
    Cs,
    Char(char),
    Space,
    /// A blank line.
    Par,
    Open,
    Close,
    /// `#n` (`#` alone: 0).
    Param(u8),
    Verb,
}

/// An atom: its kind, its token and bytes, and for a brace its mate's
/// index (`usize::MAX`: none).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Atom {
    pub kind: AtomKind,
    pub tok: usize,
    pub start: usize,
    pub end: usize,
    pub mate: usize,
}

impl Atom {
    /// A control sequence's name.
    #[must_use]
    pub fn name<'t>(&self, text: &'t str) -> &'t str {
        &text[self.start + 1..self.end]
    }
}

pub(crate) fn atoms(text: &str, tokens: &[Token]) -> Vec<Atom> {
    // (at most one atom a byte: never grown)
    let mut out: Vec<Atom> = Vec::with_capacity(text.len() + 1);
    let mut stack: Vec<usize> = Vec::new();
    let mut line_blank = true;
    let mut skip_space = false;
    let mut after_comment = false;
    for (k, t) in tokens.iter().enumerate() {
        let atom = |kind| Atom {
            kind,
            tok: k,
            start: t.start,
            end: t.end,
            mate: usize::MAX,
        };
        match t.kind {
            TokKind::Comment => {
                line_blank = false;
                after_comment = true;
            }
            TokKind::Newline => {
                if after_comment {
                    after_comment = false;
                } else if line_blank && !t.expl {
                    out.push(atom(AtomKind::Par));
                } else if !skip_space && !t.expl {
                    out.push(atom(AtomKind::Space));
                }
                skip_space = true;
                line_blank = true;
            }
            TokKind::Space => {
                if !skip_space && !t.expl {
                    out.push(atom(AtomKind::Space));
                    skip_space = true;
                }
            }
            TokKind::Text => {
                line_blank = false;
                skip_space = false;
                let run = &text[t.start..t.end];
                if !t.expl && run.is_ascii() {
                    // (a byte a character: no decoding, no `~` to look for)
                    out.extend(run.bytes().enumerate().map(|(o, c)| Atom {
                        kind: AtomKind::Char(char::from(c)),
                        tok: k,
                        start: t.start + o,
                        end: t.start + o + 1,
                        mate: usize::MAX,
                    }));
                    continue;
                }
                for (o, c) in run.char_indices() {
                    if t.expl && c == '~' {
                        out.push(Atom {
                            start: t.start + o,
                            end: t.start + o + 1,
                            ..atom(AtomKind::Space)
                        });
                        continue;
                    }
                    out.push(Atom {
                        start: t.start + o,
                        end: t.start + o + c.len_utf8(),
                        ..atom(AtomKind::Char(c))
                    });
                }
            }
            TokKind::Cs => {
                line_blank = false;
                let name = &text[t.start + 1..t.end];
                // (a control word, not a control symbol, skips the spaces
                // after it)
                skip_space =
                    name.len() > 1 || name.chars().all(char::is_alphabetic) && !name.is_empty();
                out.push(atom(AtomKind::Cs));
            }
            TokKind::Open => {
                line_blank = false;
                skip_space = false;
                stack.push(out.len());
                out.push(atom(AtomKind::Open));
            }
            TokKind::Close => {
                line_blank = false;
                skip_space = false;
                let me = out.len();
                let mut a = atom(AtomKind::Close);
                if let Some(o) = stack.pop() {
                    out[o].mate = me;
                    a.mate = o;
                }
                out.push(a);
            }
            TokKind::Param => {
                line_blank = false;
                skip_space = false;
                let d = text
                    .as_bytes()
                    .get(t.start + 1)
                    .filter(|c| c.is_ascii_digit())
                    .map_or(0, |c| c - b'0');
                out.push(atom(AtomKind::Param(d)));
            }
            TokKind::Verbatim => {
                line_blank = false;
                skip_space = false;
                out.push(atom(AtomKind::Verb));
            }
        }
    }
    out
}

/// The text of a brace group's characters (an environment's name).
pub(crate) fn group_text(atoms: &[Atom], open: usize) -> Option<String> {
    let close = atoms.get(open)?.mate;
    if close == usize::MAX || close < open {
        return None;
    }
    let mut s = String::new();
    for a in &atoms[open + 1..close] {
        match a.kind {
            AtomKind::Char(c) => s.push(c),
            AtomKind::Space => {}
            _ => return None,
        }
    }
    Some(s)
}

/// The group and environment pairs written as commands, matched within
/// their own brace group.
pub(crate) fn group_pairs(text: &str, atoms: &[Atom], tokens: &[Token]) -> Vec<GroupPair> {
    // (per open brace group, the commands open in it)
    let mut levels: Vec<Vec<usize>> = vec![Vec::new()];
    let mut out: Vec<GroupPair> = Vec::new();
    let next_open = |a: usize| {
        let mut j = a + 1;
        while atoms.get(j).is_some_and(|x| x.kind == AtomKind::Space) {
            j += 1;
        }
        (atoms.get(j).map(|x| x.kind) == Some(AtomKind::Open)).then_some(j)
    };
    let close_level = |level: Vec<usize>, out: &mut Vec<GroupPair>| {
        for p in level {
            out[p].status = Status::Pending;
        }
    };
    for (a, at) in atoms.iter().enumerate() {
        match at.kind {
            AtomKind::Open => levels.push(Vec::new()),
            AtomKind::Close => {
                if levels.len() > 1 {
                    let l = levels.pop().unwrap_or_default();
                    close_level(l, &mut out);
                }
            }
            AtomKind::Cs => {
                let name = at.name(text);
                let opening = match name {
                    "begingroup" => Some(GroupKind::BeginGroup),
                    "bgroup" => Some(GroupKind::BGroup),
                    "begin" => next_open(a)
                        .and_then(|o| group_text(atoms, o))
                        .map(GroupKind::Env),
                    _ => None,
                };
                let closing = match name {
                    "endgroup" => Some(GroupKind::BeginGroup),
                    "egroup" => Some(GroupKind::BGroup),
                    "end" => next_open(a)
                        .and_then(|o| group_text(atoms, o))
                        .map(GroupKind::Env),
                    _ => None,
                };
                let tz = tokens[at.tok].tz;
                let level = levels.last_mut().expect("a level");
                if let Some(kind) = opening {
                    level.push(out.len());
                    out.push(GroupPair {
                        kind,
                        open: Some(a),
                        close: None,
                        status: tz,
                    });
                } else if let Some(kind) = closing {
                    if let Some(pos) = level.iter().rposition(|&p| out[p].kind == kind) {
                        // (anything opened after it and still open is not
                        // closed in this text)
                        let p = level[pos];
                        for q in level.drain(pos..).skip(1) {
                            out[q].status = Status::Pending;
                        }
                        out[p].close = Some(a);
                        out[p].status = out[p].status.max(tz);
                    } else {
                        out.push(GroupPair {
                            kind,
                            open: None,
                            close: Some(a),
                            status: Status::Pending,
                        });
                    }
                }
            }
            _ => {}
        }
    }
    for l in levels {
        close_level(l, &mut out);
    }
    out
}
