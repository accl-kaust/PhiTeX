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
//! recovery, `ins_list`), lists backed up to be read again, and macros'
//! levels with their arguments.
//!
//! A macro's level here is TeX's macro level and its `<argument>` levels
//! in one: the run reads a macro's body with its arguments in place
//! (`Arena::expand_body`), keeping with the level only the name it was
//! called by and its arguments' lengths, written before the expansion
//! (`Level::name`, `Level::nargs`); nothing else is kept on the hot path.
//! At an error, the macro's definition is its meaning now, taken where it
//! matches the expansion (its plain tokens where they stand, the
//! arguments' lengths): the place read in the expansion is then a place
//! in the body (the macro's level, §314's `\name` and its parameter text,
//! `->`, its body) or in an argument (an `<argument>` level above the
//! macro's, which stands just past that parameter's `#n`). A token list's
//! level stepped back over tokens put back is TeX's levels of those tokens
//! above it (`BackMark`). A node's text taken up from another run's
//! levels (a continuation's rest) keeps what each level was ([`Lv`]:
//! backed up, inserted, a macro's by its name, found in its body from the
//! end); a line held from a place in it on, or a file's line read whole,
//! has its head made from the text the client holds when the log is laid
//! out (`crate::log::HEAD_MARK`). An `\errmessage`'s help is `\errhelp` or
//! TeX's own (`ERR_HELP`, `LONG_HELP`).
//!
//! Not as TeX: a macro defined again while its expansion is still read
//! (its meaning now no longer matches the expansion: the level is left
//! out), and a token another node read ahead and gave back (`\usepackage`'s
//! look for `[`): TeX reads it again from a list (`<recently read>`), the
//! node from its line.
//!
//! [`Lv`]: crate::input::Lv

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

/// In a context event's text, after its levels: `\errhelp`'s tokens,
/// printed (§1283's `use_err_help`: §90 prints them by `give_err_help`
/// after a `print_ln`, the new-line character a line's end).
pub(crate) const ERR_HELP: char = '\u{10FFF6}';

/// In a context event's text, after its levels: an `\errmessage` with
/// `\errhelp` empty (§1283): its four help lines the first time in the
/// job, then `(That was another \errmessage.)`; the log's layout knows
/// which (TeX's `long_help_seen`, [`crate::log::Log`]).
pub(crate) const LONG_HELP: char = '\u{10FFF5}';

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
    /// `bottom_line`); where it holds the line from a place in it on, the
    /// line's node and place there, and whether its units are characters
    /// (its head the log's layout makes, [`crate::log::HEAD_MARK`]).
    Line(Vec<u32>, usize, LineMark, bool, Option<String>),
    /// A token list: its §314 name, its tokens, the place read.
    Toks(&'static str, Vec<Tok>, usize),
    /// A macro's level (§314: `print_cs(name)`): its name, its parameter
    /// text, `end_match` and its body, the place read.
    Macro(Tok, Vec<Tok>, usize),
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
        // (an `\errmessage`'s: `\errhelp`, or TeX's own, §1283)
        match self.err_help.take() {
            Some(t) if t.is_empty() => out.push(LONG_HELP),
            Some(t) => {
                let nl = self.ipar(crate::fields::Par::newlinechar)?;
                out.push(ERR_HELP);
                out.push_str(&printed_nl(&t, nl));
            }
            None => {}
        }
        Ok(out)
    }

    /// `\errhelp`'s tokens printed, for the `\errmessage` about to be
    /// logged ([`Run::error_tail`], §1283).
    pub(crate) fn note_err_help(&mut self) -> R<()> {
        /// `\errhelp`'s code (`crate::prims`).
        const ERR_HELP_LOC: i32 = 3421;
        let l = self.quantity_loc(crate::val::cmd::ASSIGN_TOKS, ERR_HELP_LOC);
        let toks = self.toks_loc(l)?;
        let mut s = String::new();
        self.show_toks_into(&mut s, &toks)?;
        self.err_help = Some(s);
        Ok(())
    }

    /// The levels from the top (§311), each with what §312 needs.
    fn context_levels(&mut self) -> R<Vec<Shown>> {
        let mut out = Vec::new();
        let mark = self.input.back_mark;
        let levels = self.input.levels.clone();
        for (i, l) in levels.iter().enumerate().rev() {
            match l.kind {
                kind::LINE => out.push(self.shown_line(self.input.line(*l), None)),
                kind::TOKS => {
                    let all = self
                        .input
                        .arena
                        .get(crate::input::Range {
                            start: l.c,
                            end: l.b,
                        })
                        .to_vec();
                    // (tokens put back by stepping this level back: TeX's
                    // own lists above it, §325; read again, the last is
                    // read to its end, the ones before it gone, §324)
                    let mut at = l.a;
                    if mark.depth as usize == i
                        && mark.c == l.c
                        && (mark.lo..=mark.hi).contains(&l.a)
                    {
                        if l.a > mark.lo {
                            let t = self.input.arena.at(l.a - 1);
                            out.push(Shown::Toks("<recently read> ", vec![t], 1));
                        }
                        for k in l.a..mark.hi {
                            let t = self.input.arena.at(k);
                            out.push(Shown::Toks("<to be read again> ", vec![t], 0));
                        }
                        at = mark.hi;
                    }
                    let at = (at - l.c) as usize;
                    if l.ty == ty::MACRO && l.name != 0 && l.nargs == crate::input::REST_ONLY {
                        self.macro_levels(&mut out, Tok::from_raw(l.name), &all, None, at)?;
                    } else if l.ty == ty::MACRO && l.name != 0 {
                        let lens: Vec<u32> = self
                            .input
                            .arena
                            .get(crate::input::Range {
                                start: l.c - u32::from(l.nargs),
                                end: l.c,
                            })
                            .iter()
                            .map(|t| t.raw())
                            .collect();
                        self.macro_levels(&mut out, Tok::from_raw(l.name), &all, Some(&lens), at)?;
                    } else if let Some(s) = shown_toks(l.ty, all, at) {
                        out.push(s);
                    }
                }
                _ => {
                    // (a file's rest between its lines: its last line read
                    // is not held; its number, and its text as the log's
                    // layout finds it, [`crate::log::HEAD_MARK`])
                    let f = self.input.file(*l);
                    let n = i64::from(f.line);
                    let head = (n > 0).then(|| {
                        let name = crate::val::str_of(self.v, f.name);
                        format!("{}:f:{n}:{name}", u8::from(self.xe))
                    });
                    out.push(Shown::Line(
                        Vec::new(),
                        0,
                        (Some(n), 0, None, None),
                        true,
                        head,
                    ));
                }
            }
        }
        // (no line of a file on the stack, the run reading a node's text
        // that is a continuation's: the line TeX reads below its tokens
        // is the line its text goes on in, not reached yet, its part read
        // before the node not held: shown from where the node's text
        // takes it up)
        if !out.iter().any(|s| matches!(s, Shown::Line(..)))
            && let Some(Seg::Line(l)) = self.input.text_rest().first()
            && !l.starts
        {
            let l = l.clone();
            let e = u32::try_from(self.ipar(crate::fields::Par::endlinechar)?).ok();
            out.push(self.shown_line(&l, e));
        }
        // (a run resumed from a park: the levels it parked with, below)
        let parked = self.input.text_at..self.parked_levels.min(self.input.text.len());
        for s in self.input.text.get(parked).unwrap_or_default() {
            match s {
                Seg::Line(l) => out.push(self.shown_line(l, None)),
                Seg::Toks(..) | Seg::Chunk(..) => {}
                Seg::File(f) => {
                    let n = i64::from(f.line);
                    out.push(Shown::Line(
                        Vec::new(),
                        0,
                        (Some(n), 0, None, None),
                        true,
                        None,
                    ));
                }
            }
        }
        Ok(out)
    }

    /// A line's level (§318: the `\endlinechar` TeX put at its end is
    /// not shown).
    fn shown_line(&self, l: &Line, endline: Option<u32>) -> Shown {
        let mut buf = l.buf.clone();
        if (0..256).contains(&l.endch) && l.ends && !buf.is_empty() {
            buf.pop();
        }
        // (a continuation's rest holds its `\endlinechar`, put in where
        // the line was read: not shown)
        if !l.ends && endline.is_some() && buf.last().copied() == endline {
            buf.pop();
        }
        let file = !l.rescanned;
        let head = if l.starts {
            None
        } else {
            l.owner
                .map(|(n, k)| format!("{}:n:{n}:{k}", u8::from(l.chars)))
        };
        Shown::Line(buf, l.loc, self.line_mark_of(l), file, head)
    }

    /// A macro's level as TeX's levels (top first): the macro called by
    /// `name`, its expansion `all` read to `at`, its arguments' lengths
    /// `lens` (none: a macro without parameters, or one whose body uses
    /// none of them read in place). Its definition is its meaning now
    /// where that matches the expansion; a macro defined again since its
    /// call, its body otherwise, is not shown.
    fn macro_levels(
        &mut self,
        out: &mut Vec<Shown>,
        name: Tok,
        all: &[Tok],
        lens: Option<&[u32]>,
        at: usize,
    ) -> R<()> {
        let m = match self.mean_of(name)? {
            crate::val::Mean::Macro(m) => Some(m),
            _ => None,
        };
        let whole = |params: &[Tok], body: &[Tok], loc: usize| {
            let mut t = params.to_vec();
            t.push(crate::val::end_match());
            t.extend_from_slice(body);
            Shown::Macro(name, t, params.len() + 1 + loc)
        };
        let Some(lens) = lens else {
            // (the rest of the expansion alone, a node's text: its place
            // in the body found from the end, as far as the body's plain
            // tokens match it; a place in an argument is not found)
            let Some(m) = m else { return Ok(()) };
            let (bl, n) = (m.body.len(), all.len());
            let mut matched = 0;
            while matched < n && matched < bl {
                let b = m.body[bl - 1 - matched];
                let param = b
                    .as_char()
                    .is_some_and(|x| x >> 21 == u32::from(crate::val::OUT_PARAM_CAT));
                if param || b != all[n - 1 - matched] {
                    break;
                }
                matched += 1;
            }
            if at + matched >= n && (at > 0 || matched == n) {
                out.push(whole(&m.params, &m.body, bl - (n - at)));
            }
            return Ok(());
        };
        if lens.is_empty() {
            // (the body read in place: the expansion is the body; its
            // parameter text the meaning's whose body it is)
            let params = m
                .as_ref()
                .filter(|m| &m.body[..] == all)
                .map_or(&[][..], |m| &m.params[..]);
            out.push(whole(params, all, at));
            return Ok(());
        }
        let Some(m) = m.filter(|m| !m.segs.is_empty()) else {
            return Ok(());
        };
        // (the expansion's pieces: each segment's plain tokens where they
        // stand, then its argument; the meaning is the call's where they
        // all match and fill the expansion)
        let mut spans = Vec::with_capacity(m.segs.len());
        let mut off = 0usize;
        for sg in &m.segs {
            let lit = &m.body[sg.lo as usize..sg.hi as usize];
            if all.get(off..off + lit.len()) != Some(lit) {
                return Ok(());
            }
            off += lit.len();
            let n = match sg.param {
                0 => 0,
                p => match lens.get(usize::from(p) - 1) {
                    Some(&n) => n as usize,
                    None => return Ok(()),
                },
            };
            spans.push((off - lit.len(), off, off + n));
            off += n;
        }
        if off != all.len() {
            return Ok(());
        }
        // (the token read last: in a segment's plain tokens, the body's
        // place after it; in an argument, the argument's place after it
        // and the body's after its `#n`)
        let Some(q) = at.checked_sub(1) else {
            out.push(whole(&m.params, &m.body, 0));
            return Ok(());
        };
        for (sg, &(lo, mid, hi)) in m.segs.iter().zip(&spans) {
            if q < mid {
                out.push(whole(&m.params, &m.body, sg.lo as usize + (q - lo) + 1));
                return Ok(());
            }
            if q < hi {
                out.push(Shown::Toks(
                    "<argument> ",
                    all[mid..hi].to_vec(),
                    q - mid + 1,
                ));
                out.push(whole(&m.params, &m.body, sg.hi as usize + 1));
                return Ok(());
            }
        }
        out.push(whole(&m.params, &m.body, m.body.len()));
        Ok(())
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

    /// The context's levels above TeX's line (§311's `bottom_line`), as a
    /// context event's text: what a diagnostic shows above the line it
    /// is placed at ([`crate::api`]); empty where there are none.
    pub(crate) fn context_above_line(&mut self) -> R<String> {
        let mut out = String::new();
        let mut marks = Vec::new();
        let at = self.show_context(&mut out, &mut marks)?;
        out.truncate(at);
        Ok(out)
    }

    /// `show_context` (§311): the levels from the top to the first line
    /// of a file, the top one, that line and `\errorcontextlines` others
    /// shown, the rest said by `...` once; the terminal's line (the job's
    /// command line, `<*>`) where no file is open. Where in `out` the
    /// file's line begins (its end where none is shown).
    fn show_context(&mut self, out: &mut String, marks: &mut Vec<i64>) -> R<usize> {
        let l = self.quantity_loc(cmd::ASSIGN_INT, ERROR_CONTEXT_LINES);
        let ecl = self.int_loc(l)?;
        let levels = self.context_levels()?;
        let nl = self.ipar(crate::fields::Par::newlinechar)?;
        let mut nn = 0i32;
        let mut bottom = false;
        let mut line_at = None;
        for (i, s) in levels.into_iter().enumerate() {
            let top = i == 0;
            bottom = matches!(s, Shown::Line(_, _, _, true, _));
            // (§312: a list backed up and read to its end is not shown
            // below the top)
            if !top && matches!(s, Shown::Toks("<recently read> ", ..)) {
                continue;
            }
            if bottom {
                line_at = Some(out.len());
            }
            if top || bottom || nn < ecl {
                match s {
                    Shown::Line(buf, loc, m, _, head) => {
                        let num = self.line_for_log(m, marks)?;
                        let (mut a, b) = split_printed(&buf, loc);
                        if let Some(r) = head {
                            let h = crate::log::HEAD_MARK;
                            a = format!("{h}{r}{h}{a}");
                        }
                        level(out, &format!("l.{num} "), &a, &b);
                    }
                    Shown::Toks(name, all, at) => {
                        let (a, b) = self.show_toks_split(&all, at)?;
                        level(out, name, &printed_nl(&a, nl), &printed_nl(&b, nl));
                    }
                    Shown::Macro(name, all, at) => {
                        let mut p = String::new();
                        self.show_toks_into(&mut p, &[name])?;
                        let (a, b) = self.show_toks_split(&all, at)?;
                        level(out, &printed(&p), &printed_nl(&a, nl), &printed_nl(&b, nl));
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
            line_at = Some(out.len());
            level(out, "<*> ", &CMDLINE.to_string(), "");
        }
        Ok(line_at.unwrap_or(out.len()))
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

/// A token list's level of type `ty` (§314) the context shows: backed-up
/// and inserted tokens.
fn shown_toks(ty: u8, all: Vec<Tok>, at: usize) -> Option<Shown> {
    match ty {
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

/// [`printed`] of a token list's text, the new-line character kept as
/// itself where the format's translation prints it (TeX Live's cp227:
/// 9, 10 and 11, `tcx`): the log's layout makes it a line's end (§58),
/// as the display of `trick_buf` does (§317).
fn printed_nl(s: &str, nl: i32) -> String {
    let keep = u32::try_from(nl).ok().filter(|c| (9..=11).contains(c));
    let mut out = String::new();
    for c in s.chars() {
        if Some(u32::from(c)) == keep {
            out.push(c);
        } else {
            crate::log::printable(u32::from(c), &mut out);
        }
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
