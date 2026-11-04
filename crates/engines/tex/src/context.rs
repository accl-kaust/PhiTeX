//! What TeX's `error` prints after the message in a run that cannot ask
//! (§90, nonstop and batch mode): the context (`show_context`,
//! §311–§320), the help lines and an empty line; and before an error met
//! while a list was scanned, the list so far (`runaway`, §306).
//!
//! The run gives each level's parts as it holds them, and the log's
//! layout makes the two lines of §317 from them ([`crate::log`]): a line
//! of a document's piece has its number made there
//! ([`crate::log::LINE_MARK`], [`Run::line_for_log`]), and the number's
//! width moves line 2.
//!
//! Shown: the lines of files (`l.N`), the terminal's line when no file is
//! open (`<*>`, the job's command line, after the main file ended), and
//! the token lists the run knows the type of: inserted text (§339's
//! recovery, `ins_list`) and lists backed up to be read again. A macro's
//! level is not shown: the run reads a macro's body with its arguments
//! in place and does not keep its name with the level.

use crate::input::{Line, Seg, kind, ty};
use crate::run::{FileLine, LineMark, R, Run};
use crate::val::{Tok, cmd};

/// In a context event's text: a level's prefix (`l.N `, `<inserted text> `)
/// follows, then [`PART`], the level's text before the place read, [`PART`]
/// and the rest.
pub(crate) const LEVEL: char = '\u{10FFFB}';
pub(crate) const PART: char = '\u{10FFFA}';
/// In a context event's text: `print_nl` of the text that follows (a help
/// line, §311's `...`).
pub(crate) const NL: char = '\u{10FFF9}';

/// In a context event's text: the job's command line (`main.tex`), which
/// the log's layout knows (the name it opens the log with,
/// [`crate::log::LayoutMemo::update`]); the run reads nothing for it.
pub(crate) const CMDLINE: char = '\u{10FFF8}';

/// TeX Live's `error_line` (texmf.cnf), §317's limit for a context line.
const ERROR_LINE: usize = 79;

/// \errorcontextlines's `assign_int` code (`crate::prims`).
const ERROR_CONTEXT_LINES: i32 = 5317;

/// A level of the input stack as §312 displays it.
enum Shown {
    /// A line of a file or a pseudo file: its characters (the
    /// `\endlinechar` TeX put at its end left out), the place read, its
    /// number as the log makes it; whether it is a file's (§311's
    /// `bottom_line`).
    Line(Vec<u32>, usize, LineMark, bool),
    /// A token list: its §314 name, its tokens, the place read.
    Toks(&'static str, Vec<Tok>, usize),
}

impl Run<'_, '_> {
    /// The context, help lines and the empty line `error` ends with (§90),
    /// for a context event ([`crate::log::Log::context`]); the lines' marks
    /// in `marks`.
    pub(crate) fn error_tail(&mut self, help: &[&str], marks: &mut Vec<i64>) -> R<String> {
        let mut out = String::new();
        self.show_context(&mut out, marks)?;
        for h in help {
            out.push(NL);
            out.push_str(h);
        }
        Ok(out)
    }

    /// The levels from the top (§311), each with what §312 needs.
    fn context_levels(&self) -> Vec<Shown> {
        let mut out = Vec::new();
        let line = |l: &Line| {
            let mut buf = l.buf.clone();
            // (§318: the `\endlinechar` TeX put at the line's end is not
            // shown)
            if (0..256).contains(&l.endch) && l.ends && !buf.is_empty() {
                buf.pop();
            }
            let file = !l.rescanned;
            Shown::Line(buf, l.loc, self.line_mark_of(l), file)
        };
        let toks = |ty: u8, all: Vec<Tok>, at: usize| match ty {
            ty::BACKED_UP | ty::BACKED_UP_CHAR => Some(Shown::Toks(
                if at < all.len() {
                    "<to be read again> "
                } else {
                    "<recently read> "
                },
                all,
                at,
            )),
            ty::INSERTED => Some(Shown::Toks("<inserted text> ", all, at)),
            _ => None,
        };
        for l in self.input.levels.iter().rev() {
            match l.kind {
                kind::LINE => out.push(line(self.input.line(*l))),
                kind::TOKS => {
                    let all = self.input.arena.get(crate::input::Range {
                        start: l.c,
                        end: l.b,
                    });
                    if let Some(s) = toks(l.ty, all.to_vec(), (l.a - l.c) as usize) {
                        out.push(s);
                    }
                }
                _ => {
                    // (a file's rest between its lines: its last line read
                    // is not held; its number)
                    let n = i64::from(self.input.file(*l).line);
                    out.push(Shown::Line(Vec::new(), 0, (Some(n), 0, None, None), true));
                }
            }
        }
        // (a run resumed from a park: the levels it parked with, below)
        let parked = self.input.text_at..self.parked_levels.min(self.input.text.len());
        for s in self.input.text.get(parked).unwrap_or_default() {
            match s {
                Seg::Line(l) => out.push(line(l)),
                Seg::Toks(_) => {}
                Seg::File(f) => {
                    let n = i64::from(f.line);
                    out.push(Shown::Line(Vec::new(), 0, (Some(n), 0, None, None), true));
                }
            }
        }
        out
    }

    /// Line `l`'s number as [`Run::line_for_log`] takes it.
    fn line_mark_of(&self, l: &Line) -> LineMark {
        match crate::run::line_of(l) {
            Some(FileLine::Known(n)) => (Some(n), 0, None, None),
            Some(FileLine::Piece(name, own)) => (
                None,
                self.doc_lines
                    .iter()
                    .find(|(n, _)| *n == name)
                    .map_or(0, |&(_, k)| k),
                Some(name),
                own,
            ),
            None => (Some(0), 0, None, None),
        }
    }

    /// `show_context` (§311): the levels from the top to the first line
    /// of a file, the top one, that line and `\errorcontextlines` others
    /// shown, the rest said by `...` once; the terminal's line (the job's
    /// command line, `<*>`) where no file is open.
    fn show_context(&mut self, out: &mut String, marks: &mut Vec<i64>) -> R<()> {
        let l = self.quantity_loc(cmd::ASSIGN_INT, ERROR_CONTEXT_LINES);
        let ecl = self.int_loc(l)?;
        let levels = self.context_levels();
        let mut nn = 0i32;
        let mut bottom = false;
        for (i, s) in levels.into_iter().enumerate() {
            let top = i == 0;
            bottom = matches!(s, Shown::Line(.., true));
            // (§312: a list backed up and read to its end is not shown
            // below the top)
            if !top && matches!(s, Shown::Toks("<recently read> ", ..)) {
                continue;
            }
            if top || bottom || nn < ecl {
                match s {
                    Shown::Line(buf, loc, m, _) => {
                        let num = self.line_for_log(m, marks)?;
                        let (a, b) = split_printed(&buf, loc);
                        level(out, &format!("l.{num} "), &a, &b);
                    }
                    Shown::Toks(name, all, at) => {
                        let mut a = String::new();
                        let mut b = String::new();
                        self.show_toks_into(&mut a, &all[..at.min(all.len())])?;
                        self.show_toks_into(&mut b, &all[at.min(all.len())..])?;
                        level(out, name, &printed(&a), &printed(&b));
                    }
                }
                nn += 1;
            } else if nn == ecl {
                out.push(NL);
                out.push_str("...");
                nn += 1;
            }
            if bottom {
                break;
            }
        }
        if !bottom && self.doc_ended {
            // (the main file ended: the terminal's line, read, is below,
            // §313's `<*>`: what the job was given, `main.tex`)
            level(out, "<*> ", &CMDLINE.to_string(), "");
        }
        Ok(())
    }

    /// `runaway` (§306): "Runaway argument?" (or definition, text), then
    /// the list so far, as far as `error_line - 10` characters, `\ETC.`
    /// for the rest; for a runaway event ([`crate::log::Log::event`]).
    pub(crate) fn runaway_text(&mut self, what: &str, toks: &[Tok]) -> R<String> {
        let mut out = format!("Runaway {what}?\n");
        let mut tally = 0usize;
        for t in toks {
            if tally >= ERROR_LINE - 10 {
                let esc = self.ipar(crate::fields::Par::escapechar)?;
                if let Ok(e) = u32::try_from(esc)
                    && e < 256
                {
                    out.push_str(&printed(&char::from_u32(e).unwrap_or('\\').to_string()));
                }
                out.push_str("ETC.");
                break;
            }
            let mut s = String::new();
            self.show_toks_into(&mut s, std::slice::from_ref(t))?;
            let p = printed(&s);
            tally += p.chars().count();
            out.push_str(&p);
        }
        Ok(out)
    }
}

/// One level: its prefix, then its text before and after the place read.
fn level(out: &mut String, prefix: &str, before: &str, after: &str) {
    out.push(LEVEL);
    out.push_str(prefix);
    out.push(PART);
    out.push_str(before);
    out.push(PART);
    out.push_str(after);
}

/// `s` (character codes) as `print` puts them in the log (§59, as
/// [`crate::log::Log::print_char`]: `^^` forms for the unprintable).
fn printed(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        crate::log::printable(u32::from(c), &mut out);
    }
    out
}

/// A line's characters printed before the place read and after it (§318).
fn split_printed(buf: &[u32], loc: usize) -> (String, String) {
    let (mut a, mut b) = (String::new(), String::new());
    for (i, &c) in buf.iter().enumerate() {
        crate::log::printable(c, if i < loc { &mut a } else { &mut b });
    }
    (a, b)
}
