//! The transcript (§54–§71): log events laid out as `print` lays them
//! out in the `.log` file (`selector=log_only`, batch mode), lines of
//! `max_print_line` (79) characters.
//!
//! The layout is the client's: it is a fold over the whole event stream
//! (`file_offset` is the only state), done once on the stream the graph
//! made.

use phitex_defuse::{ValueId, Values};

use crate::val::{int_of, list_of, str_of};

const MAX_PRINT_LINE: usize = 79;

/// The line break of a `box` event's text: no character TeX prints.
pub(crate) const BOX_LINE: char = '\u{10FFFD}';

/// The `.log` writer: `file_offset` and the text.
#[derive(Default)]
pub struct Log {
    pub text: String,
    offset: usize,
}

impl Log {
    /// `print_ln` (§57).
    pub fn print_ln(&mut self) {
        self.text.push('\n');
        self.offset = 0;
    }

    /// `print_raw_char` (§58).
    fn raw(&mut self, c: char) {
        self.text.push(c);
        self.offset += 1;
        if self.offset == MAX_PRINT_LINE {
            self.print_ln();
        }
    }

    /// `print` of one character (§59): the newline character ends the
    /// line, others print as themselves or in `^^` form.
    pub fn print_char(&mut self, c: u32, nl: i64) {
        if i64::from(c) == nl {
            self.print_ln();
            return;
        }
        // §49: characters 32..126 print as themselves, others in `^^`
        // form
        if (32..127).contains(&c) {
            self.raw(char::from_u32(c).unwrap_or('?'));
        } else if c < 128 {
            self.raw('^');
            self.raw('^');
            self.raw(char::from_u32(if c < 64 { c + 64 } else { c - 64 }).unwrap_or('?'));
        } else if c < 256 {
            let hex = |d: u32| char::from_digit(d, 16).unwrap_or('0');
            self.raw('^');
            self.raw('^');
            self.raw(hex(c / 16));
            self.raw(hex(c % 16));
        } else {
            self.raw(char::from_u32(c).unwrap_or('?'));
        }
    }

    /// `print` of a string (§59).
    pub fn print(&mut self, s: &str, nl: i64) {
        for c in s.chars() {
            self.print_char(u32::from(c), nl);
        }
    }

    /// Characters printed as they are (`print_char` directly, §58).
    fn print_plain(&mut self, s: &str) {
        for c in s.chars() {
            self.raw(c);
        }
    }

    /// `print_nl` (§62).
    pub fn print_nl(&mut self, s: &str, nl: i64) {
        if self.offset > 0 {
            self.print_ln();
        }
        self.print(s, nl);
    }

    /// One event.
    pub fn event(&mut self, kind: &str, text: &str, nl: i64) {
        let len = text.chars().count();
        match kind {
            "msg" => {
                // §1280
                if len > MAX_PRINT_LINE - 2 {
                    self.print_ln();
                } else if self.offset > 0 {
                    self.raw(' ');
                }
                self.print(text, nl);
            }
            "errmsg" => {
                self.print_nl("! ", nl);
                self.print(text, nl);
                self.raw('.');
                self.print_ln();
            }
            "error" => {
                self.print_nl("! ", nl);
                self.print_plain(text);
                self.raw('.');
                self.print_ln();
            }
            "wlog" => {
                // §1370
                self.print_nl("", nl);
                self.print(text, nl);
                self.print_ln();
            }
            "show" => {
                // §1294
                self.print_nl("> ", nl);
                self.print(text, nl);
            }
            "box" => {
                // begin_diagnostic … end_diagnostic(true) (§245) around
                // lines each begun by `print_ln` (`show_box`, §182, and
                // `show_activities`, §218)
                let mut lines = text.split(BOX_LINE);
                self.print_nl(lines.next().unwrap_or_default(), nl);
                for l in lines {
                    self.print_ln();
                    self.print(l, nl);
                }
                self.print_nl("", nl);
                self.print_ln();
            }
            "diag" => {
                // begin_diagnostic … end_diagnostic(false) (§245)
                self.print_nl(text, nl);
                self.print_nl("", nl);
            }
            "open" => {
                // §537
                if len > MAX_PRINT_LINE - 2 {
                    self.print_ln();
                } else if self.offset > 0 {
                    self.raw(' ');
                }
                self.raw('(');
                self.print(text, nl);
            }
            "close" => self.raw(')'),
            "ship" => {
                // §638
                if self.offset > 0 {
                    self.raw(' ');
                }
                self.print_plain(text);
            }
            "shipend" => self.raw(']'),
            "openout" => {
                // web2c's §1374: `print_nl`, the name, `print_nl("")`,
                // `print_ln`
                self.print_nl(text, nl);
                self.print_nl("", nl);
                self.print_ln();
            }
            "println" => self.print_ln(),
            "fatal" => {}
            _ => self.print(text, nl),
        }
    }
}

/// A document's log kept from one build to the next ([`LayoutMemo::update`],
/// [`LayoutMemo::finish`]): its events, the log's length and column after
/// each, and the fatal errors among them (`(event, whether pdfTeX's)`).
#[derive(Default)]
pub(crate) struct LayoutMemo {
    name: String,
    events: Vec<ValueId>,
    after: Vec<(usize, usize)>,
    fatal: Vec<(usize, bool)>,
    text: String,
    offset: usize,
}

impl LayoutMemo {
    /// The log of a document: the file opened, the events (laid out
    /// again from the first that differs from the last ones'); whether a
    /// fatal error is among them, and whether one is pdfTeX's.
    pub(crate) fn update(
        &mut self,
        v: &Values,
        name: &str,
        events: Option<ValueId>,
    ) -> (bool, bool) {
        let evs = events.map(|e| v.seq_to_vec(e)).unwrap_or_default();
        let same = if self.name == name {
            self.events
                .iter()
                .zip(&evs)
                .take_while(|(a, b)| a == b)
                .count()
        } else {
            0
        };
        let mut log = Log::default();
        if same == 0 {
            log.raw('(');
            log.print(name, -1);
            self.after.clear();
        } else {
            let (len, offset) = self.after[same - 1];
            self.text.truncate(len);
            log.text = std::mem::take(&mut self.text);
            log.offset = offset;
            self.after.truncate(same);
        }
        self.fatal.retain(|f| f.0 < same);
        let (fatal, pdf) = (v.str("fatal"), v.str("pdf"));
        for (k, &x) in evs.iter().enumerate().skip(same) {
            let xs = list_of(v, x);
            if xs.first() == Some(&fatal) {
                self.fatal.push((k, xs.get(1) == Some(&pdf)));
            }
            let kind = xs.first().map(|&k| str_of(v, k)).unwrap_or_default();
            let text = xs.get(1).map(|&t| str_of(v, t)).unwrap_or_default();
            let nl = xs.get(2).map_or(-1, |&n| int_of(v, n));
            log.event(&kind, &text, nl);
            self.after.push((log.text.len(), log.offset));
        }
        name.clone_into(&mut self.name);
        self.events = evs;
        self.text = log.text;
        self.offset = log.offset;
        (!self.fatal.is_empty(), self.fatal.iter().any(|f| f.1))
    }

    /// The log [`LayoutMemo::update`] laid out, then the end
    /// (`final_cleanup`, §1335), then `end` on a line of its own (§642).
    pub(crate) fn finish(&mut self, end: &str) -> String {
        let mut log = Log {
            text: std::mem::take(&mut self.text),
            offset: self.offset,
        };
        let len = log.text.len();
        log.print(" )", -1);
        log.print_nl(end, -1);
        let out = log.text.clone();
        log.text.truncate(len);
        self.text = log.text;
        out
    }
}
