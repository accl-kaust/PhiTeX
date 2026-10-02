//! Part 6: Reporting errors (§71–§98). `term_input` (§71) is in `input.rs`
//! with the rest of line input.

use crate::diag::Severity;
use crate::host::Host;
use crate::print::{LOG_ONLY, NO_PRINT, TERM_AND_LOG, TERM_ONLY};
use crate::tex::{Jump, Tex};
use crate::track::Tracker;

/// §73: omits all stops and omits terminal output.
pub const BATCH_MODE: i32 = 0;
/// §73: omits all stops.
pub const NONSTOP_MODE: i32 = 1;
/// §73: omits error stops.
pub const SCROLL_MODE: i32 = 2;
/// §73: stops at every opportunity to interact.
pub const ERROR_STOP_MODE: i32 = 3;
/// §73 (web2c): extra value for the command-line switch.
pub const UNSPECIFIED_MODE: i32 = 4;

/// §76: nothing has been amiss yet.
pub const SPOTLESS: i32 = 0;
/// §76: `begin_diagnostic` has been called.
pub const WARNING_ISSUED: i32 = 1;
/// §76: `error` has been called.
pub const ERROR_MESSAGE_ISSUED: i32 = 2;
/// §76: termination was premature.
pub const FATAL_ERROR_STOP: i32 = 3;

impl<H: Host, T: Tracker> Tex<H, T> {
    /// §73: `print_err(s)`: begin an error message.
    pub(crate) fn print_err(&mut self, s: &[u8]) {
        if self.params.file_line_error_style {
            self.print_file_line();
        } else {
            self.print_nl(b"! ");
        }
        self.diag_begin_message();
        self.print_str(s);
    }

    /// §79: `help0`…`help6`. Lines are given in reading order; as in WEB,
    /// `help_line[0]` is the last one.
    pub(crate) fn help(&mut self, lines: &[&'static [u8]]) {
        self.help_ptr = lines.len();
        for (i, l) in lines.iter().enumerate() {
            self.help_line[lines.len() - 1 - i] = l;
        }
    }

    /// web2c: the `file:line:` prefix for `-file-line-error`.
    pub(crate) fn print_file_line(&mut self) {
        let mut level = self.in_open;
        while level > 0 && self.full_source_filename_stack[level] == 0 {
            level -= 1;
        }
        if level == 0 {
            self.print_nl(b"! ");
        } else {
            self.print_nl(b"");
            self.print(self.full_source_filename_stack[level]);
            self.print_char(b':');
            if level == self.in_open {
                self.print_line_no(level, self.line);
            } else {
                self.print_line_no(level, self.line_stack[level + 1]);
            }
            self.print_str(b": ");
        }
    }

    /// §81: `jump_out`: close files and terminate, which `run` does when
    /// `Jump::JumpOut` reaches it.
    #[allow(clippy::unused_self)] // a method, like the rest of the error code
    pub(crate) fn jump_out<R>(&mut self) -> Result<R, Jump> {
        Err(Jump::JumpOut)
    }

    /// §82: complete the job of error reporting.
    pub(crate) fn error(&mut self) -> Result<(), Jump> {
        self.diag_report(Severity::Error);
        if self.history() < ERROR_MESSAGE_ISSUED {
            self.set_history(ERROR_MESSAGE_ISSUED);
        }
        self.print_char(b'.');
        self.show_context();
        if self.params.halt_on_error {
            // If `close_files_and_terminate` generates an error, we'll end
            // up back here; just give up in that case.
            if self.halting_on_error {
                self.update_terminal();
                return Err(Jump::EndOfTex); // `do_final_end`
            }
            self.halting_on_error = true;
            self.put_help_message();
            self.set_history(FATAL_ERROR_STOP);
            return self.jump_out();
        }
        if self.interaction() == ERROR_STOP_MODE {
            // §83: get user's advice and return.
            loop {
                if self.interaction() != ERROR_STOP_MODE {
                    return Ok(());
                }
                self.clear_for_error_prompt();
                self.prompt_input(b"? ")?;
                if self.last == self.first {
                    return Ok(());
                }
                let mut c = self.buffer[self.first];
                if c >= b'a' {
                    c = c.wrapping_add(b'A').wrapping_sub(b'a'); // convert to uppercase
                }
                // §84: interpret code `c` and return if done.
                match c {
                    b'0'..=b'9' if self.deletions_allowed => {
                        self.delete_tokens(c)?;
                        continue;
                    }
                    b'E' if self.base_ptr > 0 && self.input_stack[self.base_ptr].name >= 256 => {
                        let name = self.input_stack[self.base_ptr].name;
                        self.edit_request = Some((name, self.line));
                        return self.jump_out();
                    }
                    b'H' => {
                        self.print_help_info();
                        continue;
                    }
                    b'I' => {
                        // §87: introduce new material from the terminal.
                        self.begin_file_reading()?;
                        // now `state=mid_line`, so an initial blank space
                        // will count as a blank
                        if self.last > self.first + 1 {
                            self.cur_input.loc = i32::try_from(self.first + 1).unwrap_or(0);
                            self.buffer[self.first] = b' ';
                        } else {
                            self.prompt_input(b"insert>")?;
                            self.cur_input.loc = i32::try_from(self.first).unwrap_or(0);
                        }
                        self.first = self.last;
                        // no `end_line_char` ends this line
                        self.cur_input.limit = i32::try_from(self.last).unwrap_or(0) - 1;
                        return Ok(());
                    }
                    b'Q' | b'R' | b'S' => {
                        // §86: change the interaction level and return.
                        self.set_error_count(0);
                        self.set_interaction(BATCH_MODE + i32::from(c - b'Q'));
                        self.print_str(b"OK, entering ");
                        match c {
                            b'Q' => {
                                self.print_esc(b"batchmode");
                                self.set_selector(self.selector() - 1);
                            }
                            b'R' => self.print_esc(b"nonstopmode"),
                            _ => self.print_esc(b"scrollmode"),
                        }
                        self.print_str(b"...");
                        self.print_ln();
                        self.update_terminal();
                        return Ok(());
                    }
                    b'X' => {
                        self.set_interaction(SCROLL_MODE);
                        return self.jump_out();
                    }
                    _ => {}
                }
                // §85: print the menu of available options.
                self.print_str(b"Type <return> to proceed, S to scroll future error messages,");
                self.print_nl(b"R to run without stopping, Q to run quietly,");
                self.print_nl(b"I to insert something, ");
                if self.base_ptr > 0 && self.input_stack[self.base_ptr].name >= 256 {
                    self.print_str(b"E to edit your file,");
                }
                if self.deletions_allowed {
                    self.print_nl(b"1 or ... or 9 to ignore the next 1 to 9 tokens of input,");
                }
                self.print_nl(b"H for help, X to quit.");
            }
        }
        self.set_error_count(self.error_count() + 1);
        if self.error_count() == 100 {
            self.print_nl(b"(That makes 100 errors; please try again.)");
            self.set_history(FATAL_ERROR_STOP);
            return self.jump_out();
        }
        self.put_help_message();
        Ok(())
    }

    /// §88: delete `c-"0"` tokens and `goto continue`.
    fn delete_tokens(&mut self, c: u8) -> Result<(), Jump> {
        let s1 = self.cur_tok;
        let s2 = self.cur_cmd;
        let s3 = self.cur_chr;
        let s4 = self.align_state();
        self.set_align_state(1_000_000);
        self.ok_to_interrupt = false;
        let d = self.buffer[self.first + 1];
        let mut n = if self.last > self.first + 1 && d.is_ascii_digit() {
            i32::from(c) * 10 + i32::from(d) - i32::from(b'0') * 11
        } else {
            i32::from(c - b'0')
        };
        while n > 0 {
            self.get_token()?; // one-level recursive call of `error` is possible
            n -= 1;
        }
        self.cur_tok = s1;
        self.cur_cmd = s2;
        self.cur_chr = s3;
        self.set_align_state(s4);
        self.ok_to_interrupt = true;
        self.help(&[
            b"I have just deleted some text, as you asked.",
            b"You can now delete more, or insert, or whatever.",
        ]);
        self.show_context();
        Ok(())
    }

    /// §89: print the help information and `goto continue`.
    fn print_help_info(&mut self) {
        if self.use_err_help {
            self.give_err_help();
            self.use_err_help = false;
        } else {
            if self.help_ptr == 0 {
                self.help(&[
                    b"Sorry, I don't know how to help in this situation.",
                    b"Maybe you should try asking a human?",
                ]);
            }
            loop {
                self.help_ptr -= 1;
                self.print_str(self.help_line[self.help_ptr]);
                self.print_ln();
                if self.help_ptr == 0 {
                    break;
                }
            }
        }
        self.help(&[
            b"Sorry, I already gave what help I could...",
            b"Maybe you should try asking a human?",
            b"An error might have occurred before I noticed any problems.",
            b"``If all else fails, read the instructions.''",
        ]);
    }

    /// §90: put help message on the transcript file.
    fn put_help_message(&mut self) {
        if self.interaction() > BATCH_MODE {
            self.set_selector(self.selector() - 1); // avoid terminal output
        }
        if self.use_err_help {
            self.print_ln();
            self.give_err_help();
        } else {
            while self.help_ptr > 0 {
                self.help_ptr -= 1;
                self.print_nl(self.help_line[self.help_ptr]);
            }
        }
        self.print_ln();
        if self.interaction() > BATCH_MODE {
            self.set_selector(self.selector() + 1); // re-enable terminal output
        }
        self.print_ln();
    }

    /// §91: an error message with an integer at the end.
    pub(crate) fn int_error(&mut self, n: i32) -> Result<(), Jump> {
        self.print_str(b" (");
        self.print_int(n);
        self.print_char(b')');
        self.error()
    }

    /// §92: restore the selector after an interruption.
    pub(crate) fn normalize_selector(&mut self) -> Result<(), Jump> {
        let sel = if self.log_opened() {
            TERM_AND_LOG
        } else {
            TERM_ONLY
        };
        self.set_selector(sel);
        if self.job_name() == 0 {
            self.open_log_file()?;
        }
        if self.interaction() == BATCH_MODE {
            self.set_selector(self.selector() - 1);
        }
        Ok(())
    }

    /// §93: `succumb`: an irrecoverable error.
    pub(crate) fn succumb<R>(&mut self) -> Result<R, Jump> {
        self.diag_report(Severity::Fatal);
        if self.interaction() == ERROR_STOP_MODE {
            self.set_interaction(SCROLL_MODE); // no more interaction
        }
        if self.log_opened() {
            self.error()?;
        }
        self.set_history(FATAL_ERROR_STOP);
        self.jump_out()
    }

    /// §93: print `s`, and that's it.
    pub(crate) fn fatal_error<R>(&mut self, s: &'static [u8]) -> Result<R, Jump> {
        self.normalize_selector()?;
        self.print_err(b"Emergency stop");
        self.help(&[s]);
        self.succumb()
    }

    /// §94: stop due to finiteness.
    pub(crate) fn overflow<R>(&mut self, s: &[u8], n: i32) -> Result<R, Jump> {
        self.normalize_selector()?;
        self.print_err(b"TeX capacity exceeded, sorry [");
        self.print_str(s);
        self.print_char(b'=');
        self.print_int(n);
        self.print_char(b']');
        self.help(&[
            b"If you really absolutely need more capacity,",
            b"you can ask a wizard to enlarge me.",
        ]);
        self.succumb()
    }

    /// §95: consistency check violated; `s` tells where.
    pub(crate) fn confusion<R>(&mut self, s: &[u8]) -> Result<R, Jump> {
        self.normalize_selector()?;
        if self.history() < ERROR_MESSAGE_ISSUED {
            self.print_err(b"This can't happen (");
            self.print_str(s);
            self.print_char(b')');
            // sic: web2c's tex.ch doubles "can fix".
            self.help(&[b"I'm broken. Please show this to someone who can fix can fix"]);
        } else {
            self.print_err(b"I can't go on meeting you like this");
            self.help(&[
                b"One of your faux pas seems to have wounded me deeply...",
                b"in fact, I'm barely conscious. Please fix it and try again.",
            ]);
        }
        self.succumb()
    }

    /// §96: `check_interrupt`.
    #[inline]
    pub(crate) fn check_interrupt(&mut self) -> Result<(), Jump> {
        if self.interrupt != 0 {
            self.pause_for_instructions()?;
        }
        Ok(())
    }

    /// §98
    pub(crate) fn pause_for_instructions(&mut self) -> Result<(), Jump> {
        if self.ok_to_interrupt {
            self.set_interaction(ERROR_STOP_MODE);
            if self.selector() == LOG_ONLY || self.selector() == NO_PRINT {
                self.set_selector(self.selector() + 1);
            }
            self.print_err(b"Interruption");
            self.help(&[
                b"You rang?",
                b"Try to insert an instruction for me (e.g., `I\\showlists'),",
                b"unless you just want to quit by typing `X'.",
            ]);
            self.deletions_allowed = false;
            self.error()?;
            self.deletions_allowed = true;
            self.interrupt = 0;
        }
        Ok(())
    }

    /// §1284: `give_err_help`.
    pub(crate) fn give_err_help(&mut self) {
        let p = self
            .equiv_toks(crate::web::ERR_HELP_LOC)
            .cloned()
            .unwrap_or_default();
        self.token_show(&p);
    }
}
