//! Interaction (tex.web part 6, §71–§98; §37, §534–537, §1337): the
//! terminal is a value, and so is the interaction mode.
//!
//! - The terminal is `Key::ReadStream(16)`: the lines the user types (the
//!   build's, `Build::set_terminal`), a stream like `\openin`'s. A node
//!   that reads the terminal — a job's start taking its first line, an
//!   error asking for advice, `\read16` — defines it anew, advanced by the
//!   lines it took. Nothing reads a device while commands run.
//! - The mode is `Key::Interaction` (global; before any definition the
//!   build's `-interaction`, `Build::set_interaction`). Where printing goes
//!   (tex.web 54's `selector`) is a function of it: a node's log entry
//!   carries its selector markers (`show::SEL`), and `print::session`
//!   lays out the transcript and the terminal from them. No second writer.
//! - An error (§82) is a node's log text — the message, its context, the
//!   help — and, in `\errorstopmode`, the advice the next terminal line
//!   gives, acted on inside the node: deletions drop tokens from the
//!   reader's own input, an insertion is a text the reader enters, a mode
//!   change defines `Key::Interaction`, `X` and a fatal error stop the
//!   reader (`Item::Halt`, as the hundredth error does).
//! - A job's start (§1337) is a node at the start of each job of a
//!   session (`graph::JOB`): the terminal's first line, the transcript's
//!   banner, the main file's `(name`, `\everyjob`.

use std::fmt::Write as _;
use std::sync::Arc;

use super::{EOL_UNREAD, Exec, FATAL_ERRORS, Kind, PendingText, TState, TextKind};
use crate::item::Item;
use crate::print::{LOG_ONLY, NO_PRINT, TERM_AND_LOG, TERM_ONLY, sel};
use crate::show::{HALT, JOB, LN, MSG_END, NL, OPEN, RAW, SEL_POP, SEL_PUSH, THAT_MAKES_100, WLOG};
use crate::val::{IntPar, Key, Stream, Tok, TokPar, Val};

pub(super) const BATCH_MODE: i32 = 0;
pub(super) const NONSTOP_MODE: i32 = 1;
pub(super) const SCROLL_MODE: i32 = 2;
pub(super) const ERROR_STOP_MODE: i32 = 3;

/// The terminal's stream number (`Key::ReadStream`).
pub(super) const TERMINAL: u8 = 16;

/// TeX's banner (tex.web 2).
const BANNER: &str = "This is TeX, Version 3.141592653";

/// What an error's help is (§79, §1283): lines, or `\errhelp`'s tokens.
pub(super) enum Help<'h> {
    Lines(&'h [&'h str]),
    ErrHelp(Arc<Vec<Tok>>),
}

/// How the advice loop ended (§83).
enum Advice {
    /// Go on asking.
    Again,
    /// The error is done: no help, no count (an empty line, a mode
    /// change, an insertion).
    Return,
    /// The job ends here (`X`, `E`, the terminal's end).
    JumpOut,
}

/// The months as TeX's banner names them (tex.web 536).
const MONTHS: [&str; 12] = ["JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC"];

impl Exec<'_> {
    /// The interaction mode (tex.web 73).
    pub(super) fn interaction(&mut self) -> i32 {
        self.int(&Key::Interaction).clamp(BATCH_MODE, ERROR_STOP_MODE)
    }

    /// Where printing goes while the log is open (tex.web 75, 1265): the
    /// log only in batch mode, else the terminal too.
    pub(super) fn selector(&mut self) -> u8 {
        if self.interaction() == BATCH_MODE { LOG_ONLY } else { TERM_AND_LOG }
    }

    /// The terminal's next line as typed and as `input_ln` keeps it (its
    /// trailing spaces dropped); the terminal is defined advanced past it.
    /// `None`: the terminal has ended.
    fn term_line(&mut self) -> Option<(String, String)> {
        let key = Key::ReadStream(TERMINAL);
        let Val::Stream(s) = self.get(&key) else { return None };
        if s.next >= s.data.len() {
            return None;
        }
        let end = s.data[s.next..].iter().position(|&b| b == b'\n' || b == b'\r').map_or(s.data.len(), |k| s.next + k);
        let typed: String = s.data[s.next..end].iter().map(|&b| char::from(b)).collect();
        let (line, next) = super::io::next_line(&s.data, s.next);
        let line: String = line.iter().map(|&b| char::from(b)).collect();
        let v = Stream { data: s.data.clone(), next, normal: true };
        self.define(key, Val::Stream(Arc::new(v)), true);
        Some((typed, line))
    }

    /// tex.web 71, `term_input`, with selector `sel`: the line the user
    /// types (shown by the terminal as typed), echoed with the selector
    /// below it (`decr(selector)`: to the log). `None` at the terminal's
    /// end.
    pub(super) fn term_input(&mut self, s: &mut String, cur: u8) -> Option<String> {
        let (typed, line) = self.term_line()?;
        s.push(SEL_PUSH);
        s.push(sel(TERM_ONLY));
        s.push_str(&typed);
        s.push(LN);
        if cur > NO_PRINT {
            s.push(sel(cur - 1));
            s.push_str(&line);
            s.push(LN);
        }
        s.push(SEL_POP);
        Some(line)
    }

    /// tex.web 72–90: an error — `print_err(msg)`, its context, then the
    /// user's advice (`\errorstopmode`), the count, the help on the log.
    /// The message goes to the graph's errors too.
    pub(super) fn error(&mut self, msg: &str, help: &[&str]) {
        self.tex_error_help(msg, Help::Lines(help), false);
    }

    /// [`Exec::error`] with any help; `fatal`: §93's `succumb` after it.
    pub(super) fn tex_error_help(&mut self, msg: &str, help: Help<'_>, fatal: bool) {
        if self.halted() {
            return;
        }
        // (the graph's list holds the message; a fatal error's, its reason)
        match (&help, fatal) {
            (Help::Lines([why]), true) => self.errors.push((*why).to_string()),
            _ => self.errors.push(msg.to_string()),
        }
        let mut s = String::from(RAW);
        let cur = self.selector();
        s.push(sel(cur));
        s.push(NL);
        s.push_str("! ");
        s.push_str(msg);
        if fatal && self.interaction() == ERROR_STOP_MODE {
            // (§93: no more interaction)
            self.define(Key::Interaction, Val::Int(SCROLL_MODE), true);
        }
        self.error_tail(s, cur, help, fatal, true);
    }

    /// The rest of an error whose first line is in `s` (selector `cur`):
    /// `.`, the context, the advice, the count (unless `count` is false:
    /// a `\show`'s in the modes that do not stop, §1293), the help.
    pub(super) fn error_tail(&mut self, mut s: String, mut cur: u8, help: Help<'_>, fatal: bool, count: bool) {
        s.push('.');
        self.show_context(&mut s);
        let mut help = help;
        loop {
            if self.interaction() != ERROR_STOP_MODE {
                break;
            }
            match self.advice(&mut s, &mut cur, &mut help) {
                Advice::Again => {}
                Advice::Return => {
                    self.log_raw(&s);
                    return;
                }
                Advice::JumpOut => {
                    self.jump_out(&mut s);
                    return;
                }
            }
        }
        if count && self.count_error() {
            s.push(NL);
            s.push_str(THAT_MAKES_100);
            s.push(HALT);
            self.log_raw(&s);
            return;
        }
        // (§90: the help, on the transcript only)
        let quiet = self.interaction() > BATCH_MODE;
        if quiet {
            s.push(SEL_PUSH);
            s.push(sel(cur - 1));
        }
        match &help {
            Help::Lines(l) => {
                for h in *l {
                    s.push(NL);
                    s.push_str(h);
                }
            }
            Help::ErrHelp(t) => {
                s.push(LN);
                let t = t.clone();
                let mut x = String::new();
                self.show_toks(&mut x, &t);
                s.push_str(&x);
            }
        }
        s.push(LN);
        if quiet {
            s.push(SEL_POP);
        }
        s.push(LN);
        if fatal {
            self.jump_out(&mut s);
            return;
        }
        self.log_raw(&s);
    }

    /// tex.web 81, `jump_out`: the reader stops here, and so does the
    /// build's page fold (`Item::Halt`); the log ends at this entry.
    fn jump_out(&mut self, s: &mut String) {
        s.push(HALT);
        self.log_raw(s);
        self.st.error_count = FATAL_ERRORS;
        self.halt = true;
    }

    /// tex.web 93, `fatal_error`: "Emergency stop", the reason as its
    /// help, and the job ends.
    pub(super) fn fatal_error(&mut self, why: &str) {
        self.tex_error_help("Emergency stop", Help::Lines(&[why]), true);
    }

    /// tex.web 83–89: one round of asking the user what to do.
    fn advice(&mut self, s: &mut String, cur: &mut u8, help: &mut Help<'_>) -> Advice {
        // (`clear_for_error_prompt`, `prompt_input("? ")`)
        s.push(LN);
        s.push_str("? ");
        let Some(line) = self.term_input(s, *cur) else {
            self.log_raw(s);
            s.clear();
            s.push(RAW);
            self.fatal_error("End of file on the terminal!");
            return Advice::Return;
        };
        let mut chars = line.chars();
        let Some(first) = chars.next() else { return Advice::Return };
        let c = first.to_ascii_uppercase();
        match c {
            '0'..='9' => {
                // (§88: deletions)
                let d = chars.next().and_then(|x| x.to_digit(10));
                let mut n = match d {
                    Some(d) => c.to_digit(10).unwrap_or(0) * 10 + d,
                    None => c.to_digit(10).unwrap_or(0),
                };
                let al = self.st.al.state;
                while n > 0 {
                    if self.next_raw().is_none() {
                        break;
                    }
                    n -= 1;
                }
                self.st.al.state = al;
                *help = Help::Lines(&[
                    "I have just deleted some text, as you asked.",
                    "You can now delete more, or insert, or whatever.",
                ]);
                self.show_context(s);
                Advice::Again
            }
            'E' => {
                // (§84: the file being read and its line)
                let name = self.job_file();
                let line = self.input_line_no();
                s.push(NL);
                let _ = write!(s, "You want to edit file {name} at line {line}");
                self.define(Key::Interaction, Val::Int(SCROLL_MODE), true);
                Advice::JumpOut
            }
            'H' => {
                // (§89)
                match help {
                    Help::ErrHelp(t) => {
                        let t = t.clone();
                        let mut x = String::new();
                        self.show_toks(&mut x, &t);
                        s.push_str(&x);
                    }
                    Help::Lines(l) => {
                        let none: &[&str] =
                            &["Sorry, I don't know how to help in this situation.", "Maybe you should try asking a human?"];
                        for h in if l.is_empty() { none } else { *l } {
                            s.push_str(h);
                            s.push(LN);
                        }
                    }
                }
                *help = Help::Lines(&[
                    "Sorry, I already gave what help I could...",
                    "Maybe you should try asking a human?",
                    "An error might have occurred before I noticed any problems.",
                    "``If all else fails, read the instructions.''",
                ]);
                Advice::Again
            }
            'I' => {
                // (§87: the rest of the line, or the next one, is read now,
                // with no end-of-line character)
                let rest: String = chars.collect();
                let text = if rest.is_empty() {
                    s.push_str("insert>");
                    let Some(l) = self.term_input(s, *cur) else {
                        self.log_raw(s);
                        s.clear();
                        s.push(RAW);
                        self.fatal_error("End of file on the terminal!");
                        return Advice::Return;
                    };
                    l
                } else {
                    rest
                };
                let mut t: Vec<char> = text.chars().collect();
                t.push('\r');
                let at = self.st.pending.len();
                self.st.texts.push(PendingText {
                    text: Arc::from(t),
                    at,
                    pos: 0,
                    tstate: TState::M,
                    eol: -1,
                    eof: false,
                    kind: TextKind::Terminal,
                    endinput: false,
                });
                Advice::Return
            }
            'Q' | 'R' | 'S' => {
                // (§86)
                self.st.error_count = 0;
                let k = BATCH_MODE + (c as i32 - 'Q' as i32);
                self.define(Key::Interaction, Val::Int(k), true);
                s.push_str("OK, entering ");
                let name = ["batchmode", "nonstopmode", "scrollmode"][(c as usize) - ('Q' as usize)];
                self.print_esc(s, name);
                if c == 'Q' {
                    *cur -= 1;
                    s.push(sel(*cur));
                }
                s.push_str("...");
                s.push(LN);
                Advice::Return
            }
            'X' => {
                self.define(Key::Interaction, Val::Int(SCROLL_MODE), true);
                Advice::JumpOut
            }
            _ => {
                // (§85: the menu)
                s.push_str("Type <return> to proceed, S to scroll future error messages,");
                s.push(NL);
                s.push_str("R to run without stopping, Q to run quietly,");
                s.push(NL);
                s.push_str("I to insert something, ");
                s.push_str("E to edit your file,");
                s.push(NL);
                s.push_str("1 or ... or 9 to ignore the next 1 to 9 tokens of input,");
                s.push(NL);
                s.push_str("H for help, X to quit.");
                Advice::Again
            }
        }
    }

    /// The name the job's main file was opened by (tex.web 537).
    fn job_file(&mut self) -> String {
        match self.get(&Key::Job) {
            Val::Name(n) => format!("{n}.tex"),
            _ => format!("{}.tex", super::expand::JOBNAME),
        }
    }

    /// A job's start (tex.web 37, 61, 534–537, 1337): the terminal shows
    /// the banner and takes the first line (`**`; a blank line asks again),
    /// the transcript begins with the banner and that line, the main file
    /// is opened (`(name`), `\everyjob` is read first. The date is the
    /// clock's (`fix_date_and_time`: the build's, `IntPar::initial`); a
    /// first line beginning with `&` names the format, the definitions the
    /// previous job's `\dump` reached (`Key::Format` its identification).
    pub(super) fn job_start(&mut self) {
        self.st.pos = 1;
        self.cmd = self.cs("\u{0}jobstart");
        self.kind = Kind::Other;
        let mut s = String::new();
        s.push(JOB);
        s.push(RAW);
        s.push(sel(TERM_ONLY));
        s.push_str(BANNER);
        s.push_str(" (INITEX)");
        s.push(LN);
        let first = loop {
            s.push_str("**");
            let Some((typed, line)) = self.term_line() else {
                s.push(LN);
                s.push_str("! End of file on the terminal... why?");
                s.push(LN);
                s.push(HALT);
                self.log_push(&s);
                self.st.error_count = FATAL_ERRORS;
                self.halt = true;
                return;
            };
            s.push_str(&typed);
            s.push(LN);
            if line.trim_start_matches(' ').is_empty() {
                s.push_str("Please type the name of your input file.");
                s.push(LN);
                continue;
            }
            break line;
        };
        for p in [IntPar::Time, IntPar::Day, IntPar::Month, IntPar::Year] {
            self.define(Key::IntPar(p), Val::Int(p.initial()), true);
        }
        let ident = match (first.trim_start_matches(' ').starts_with('&'), self.get(&Key::Format)) {
            (true, Val::Name(n)) if !n.is_empty() => n.to_string(),
            _ => " (INITEX)".to_string(),
        };
        // (§536: the transcript's first lines; `wlog(banner)` is not
        // counted in the line's length)
        s.push(sel(LOG_ONLY));
        s.push(WLOG);
        s.push_str(BANNER);
        s.push(WLOG);
        s.push_str(&ident);
        let (y, mo, d, t) = (
            IntPar::Year.initial(),
            IntPar::Month.initial(),
            IntPar::Day.initial(),
            IntPar::Time.initial(),
        );
        let m = MONTHS[(mo.clamp(1, 12) - 1) as usize];
        let _ = write!(s, "  {d} {m} {y} {:02}:{:02}", t / 60, t % 60);
        s.push(NL);
        s.push_str("**");
        s.push_str(&first);
        s.push(LN);
        let cur = self.selector();
        s.push(sel(cur));
        s.push(OPEN);
        let name = self.job_file();
        s.push_str(&name);
        s.push(MSG_END);
        self.log_push(&s);
        self.vemit.push(Item::JobStart);
        // (§1030: `\everyjob` first)
        if let Val::Toks(t) = self.get(&Key::TokPar(TokPar::Everyjob))
            && !t.is_empty()
        {
            self.rewrite(t.to_vec());
        }
        self.st.eol = EOL_UNREAD;
    }
}
