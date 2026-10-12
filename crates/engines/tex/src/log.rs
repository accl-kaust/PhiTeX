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
        let mut s = String::new();
        crate::print::push_code(&mut s, c);
        for ch in s.chars() {
            self.raw(ch);
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
            "println" => self.print_ln(),
            _ => self.print(text, nl),
        }
    }
}

/// The log of a document: the file opened, the events, the end
/// (`final_cleanup`, §1335).
pub(crate) fn layout(v: &Values, name: &str, events: Option<ValueId>) -> String {
    let mut log = Log::default();
    log.raw('(');
    log.print(name, -1);
    if let Some(e) = events {
        for x in v.seq_to_vec(e) {
            let xs = list_of(v, x);
            let kind = xs.first().map(|&k| str_of(v, k)).unwrap_or_default();
            let text = xs.get(1).map(|&t| str_of(v, t)).unwrap_or_default();
            let nl = xs.get(2).map_or(-1, |&n| int_of(v, n));
            log.event(&kind, &text, nl);
        }
    }
    log.print(" )", -1);
    log.text
}
