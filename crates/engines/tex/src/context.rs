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
//! recovery, `ins_list`) and lists backed up to be read again, and a
//! macro's level (§314's name, the parameter text and body with the place
//! read in the body) from the call its level keeps
//! ([`crate::input::Call`]): the run reads a macro's body with its
//! arguments in place, so an argument being read is shown as TeX's own
//! level ("<argument>") from the arguments' lengths the call kept.

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

/// In a context event's text: `\errhelp`'s list, shown as `error` shows
/// it (§90 `give_err_help`: a line ended, then the list printed as
/// `print` prints, `\newlinechar` a line's end).
pub(crate) const ERRHELP: char = '\u{10FFF7}';

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
    /// `bottom_line`); whether the node's text was cut inside it (its
    /// rest is the next text's, [`Run::line_rest`]).
    Line(Vec<u32>, usize, LineMark, bool, bool),
    /// A token list: its §314 name, its tokens, the place read.
    Toks(&'static str, Vec<Tok>, usize),
    /// A macro's level (§314, §319): its name, its definition (the
    /// parameter text, `->` and the body), the place read in it.
    Macro(Tok, Vec<Tok>, usize),
}

/// Where a macro's level stands in its definition ([`Shown::Macro`]):
/// the place `at` tokens into its expansion (the body with arguments of
/// `lens` tokens in place of its parameters) is the place in the body,
/// counted in the definition (the parameter text and `->` before the
/// body), and, inside an argument, the argument's start in the expansion,
/// its length and the place in it (TeX reads an argument as a level of
/// its own on top of the macro's, §390, that level still there once its
/// last token is read).
fn place_in(
    m: &crate::val::Macro,
    lens: &[u32],
    at: usize,
) -> (usize, Option<(usize, usize, usize)>) {
    let off = m.params.len() + 1;
    if m.segs.is_empty() {
        return (off + at.min(m.body.len()), None);
    }
    let (mut p, mut pos) = (at, 0usize);
    for s in &m.segs {
        let lit = (s.hi - s.lo) as usize;
        if p <= lit {
            return (off + s.lo as usize + p, None);
        }
        p -= lit;
        pos += lit;
        if s.param != 0 {
            let len = lens
                .get(usize::from(s.param) - 1)
                .map_or(0, |&n| n as usize);
            if p <= len {
                // (after the parameter's `#n` in the body)
                return (off + s.hi as usize + 1, Some((pos, len, p)));
            }
            p -= len;
            pos += len;
        }
    }
    (off + m.body.len(), None)
}

impl Run<'_, '_> {
    /// The context, help lines and the empty line `error` ends with (§90),
    /// for a context event ([`crate::log::Log::context`]); the lines' marks
    /// in `marks`.
    pub(crate) fn error_tail(&mut self, help: &[&str], marks: &mut Vec<i64>) -> R<String> {
        let mut out = String::new();
        self.show_context(&mut out, marks)?;
        for h in help {
            if h.starts_with(ERRHELP) {
                out.push_str(h);
                continue;
            }
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
            Shown::Line(buf, l.loc, self.line_mark_of(l), file, file && !l.ends)
        };
        for (i, l) in self.input.levels.iter().enumerate().rev() {
            match l.kind {
                kind::LINE => out.push(line(self.input.line(*l))),
                kind::TOKS => self.toks_shown(i, l, &mut out),
                _ => {
                    // (a file's rest between its lines: its last line read
                    // is not held; its number)
                    let n = i64::from(self.input.file(*l).line);
                    out.push(Shown::Line(
                        Vec::new(),
                        0,
                        (Some(n), 0, None, None),
                        true,
                        false,
                    ));
                }
            }
        }
        // (a run resumed from a park: the levels it parked with, below)
        let parked = self.input.text_at..self.parked_levels.min(self.input.text.len());
        for s in self.input.text.get(parked).unwrap_or_default() {
            match s {
                Seg::Line(l) => out.push(match line(l) {
                    Shown::Line(b, at, m, f, _) => Shown::Line(b, at, m, f, false),
                    s => s,
                }),
                Seg::Toks(_) => {}
                Seg::File(f) => {
                    let n = i64::from(f.line);
                    out.push(Shown::Line(
                        Vec::new(),
                        0,
                        (Some(n), 0, None, None),
                        true,
                        false,
                    ));
                }
            }
        }
        out
    }

    /// The rest of the line the node's text was cut in (§318 shows the
    /// whole line): the text after it, the node's own or its next
    /// siblings' (`Ctx::following`, a read of their
    /// text), while each goes on with the line.
    fn line_rest(&mut self) -> R<Vec<u32>> {
        let mut out = Vec::new();
        let mut segs: Vec<Seg> = self
            .input
            .text
            .get(self.input.text_at..)
            .unwrap_or_default()
            .to_vec();
        let mut next = self.pulled;
        for _ in 0..16 {
            if segs.is_empty() {
                match self.cx.following(next) {
                    Some((op, args)) if op == self.ops.exec => {
                        let args = self.resolve_units(&args)?;
                        segs = crate::run::decode_segs(self.v, &args);
                        next += 1;
                    }
                    _ => break,
                }
            }
            match segs.first() {
                Some(Seg::Line(l)) if !l.starts => {
                    out.extend_from_slice(&l.buf);
                    if l.ends {
                        break;
                    }
                    segs.remove(0);
                }
                _ => break,
            }
        }
        Ok(out)
    }

    /// Token list level `i`, `l`, as §312 shows it, into `out`: the
    /// tokens put back in its place each a level above it, a macro's call
    /// (an argument being read on top of it), the list types §314 names.
    fn toks_shown(&self, i: usize, l: &crate::input::Level, out: &mut Vec<Shown>) {
        let n = self.input.levels.len();
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
            // (a token TeX keeps in hand: none of its levels once read)
            ty::REREAD if at < all.len() => Some(Shown::Toks("<to be read again> ", all, at)),
            _ => None,
        };
        let all = self.input.arena.get(crate::input::Range {
            start: l.c,
            end: l.b,
        });
        let mut at = (l.a - l.c) as usize;
        // (tokens put back in place, [`crate::input::Unread`]:
        // each TeX's level of its own above this one, the last
        // put back on top; one read again is the top's
        // "<recently read>" while nothing was read after it)
        if let Some(u) = self.input.unread
            && u.of(i, l)
            && u.from < u.to
            && (u.from..=u.to).contains(&l.a)
        {
            let (from, to) = ((u.from - l.c) as usize, (u.to - l.c) as usize);
            if at > from && i + 1 == n {
                out.push(Shown::Toks("<recently read> ", all[at - 1..at].to_vec(), 1));
            }
            for k in at..to {
                out.push(Shown::Toks("<to be read again> ", all[k..=k].to_vec(), 0));
            }
            at = to;
            // (put back from its end: TeX's `back_input` ended
            // the level first, §325)
            if u.to == l.b {
                return;
            }
        }
        // (a macro without parameters: its body is the list)
        if l.ty == ty::MACRO
            && let Some(name) = crate::input::Input::name_of(l)
        {
            let mut def = vec![crate::val::end_match()];
            def.extend_from_slice(all);
            out.push(Shown::Macro(name, def, 1 + at));
            return;
        }
        if l.ty == ty::MACRO
            && let Some((c, lens)) = self.input.call_of(l)
        {
            let (k, arg) = place_in(&c.mac, lens, at);
            if let Some((from, len, p)) = arg {
                out.push(Shown::Toks(
                    "<argument> ",
                    all.get(from..from + len).unwrap_or_default().to_vec(),
                    p,
                ));
            }
            let mut def = c.mac.params.clone();
            def.push(crate::val::end_match());
            def.extend(c.mac.body.iter().copied());
            out.push(Shown::Macro(c.name, def, k));
            return;
        }
        if let Some(s) = toks(l.ty, all.to_vec(), at) {
            out.push(s);
        }
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
            bottom = matches!(s, Shown::Line(_, _, _, true, _));
            // (§312: a list backed up and read to its end is not shown
            // below the top)
            if !top && matches!(s, Shown::Toks("<recently read> ", ..)) {
                continue;
            }
            if top || bottom || nn < ecl {
                match s {
                    Shown::Line(mut buf, loc, m, _, cut) => {
                        if cut {
                            buf.extend(self.line_rest()?);
                        }
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
                    Shown::Macro(name, def, at) => {
                        // (§314 `print_cs(name)`, then §319's list as one:
                        // the match character and the parameters' numbers
                        // run on through the body)
                        let mut cs = String::new();
                        let esc = i64::from(self.ipar(crate::fields::Par::escapechar)?);
                        let single = name.as_cs().and_then(|n| {
                            let mut it = n.chars();
                            it.next().filter(|_| it.next().is_none()).map(u32::from)
                        });
                        let cat = match single {
                            Some(c) => Some((c, i64::from(self.cat(c)?))),
                            None => None,
                        };
                        crate::print::print_cs(&mut cs, name, esc, &mut |c| {
                            cat.filter(|x| x.0 == c).map_or(12, |x| x.1)
                        });
                        let mut a = String::new();
                        let mut whole = String::new();
                        self.show_toks_into(&mut a, &def[..at.min(def.len())])?;
                        self.show_toks_into(&mut whole, &def)?;
                        let b = if let Some(b) = whole.strip_prefix(a.as_str()) {
                            b.to_owned()
                        } else {
                            let mut b = String::new();
                            self.show_toks_into(&mut b, &def[at.min(def.len())..])?;
                            b
                        };
                        level(out, &printed(&cs), &printed(&a), &printed(&b));
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
