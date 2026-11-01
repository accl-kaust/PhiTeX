//! Executing the style file (§290–330): the literal stack, the `.bbl`
//! output buffer, and `execute_fn`.

use alloc::vec::Vec;

use crate::table::Loc;
use crate::{
    BUILT_IN, Bib, END_OF_DEF, FIELD, INT_ENTRY_VAR, INT_GLOBAL_VAR, INT_LITERAL, Lit, Piece,
    QUOTE_NEXT_FN, R, STK_FN, STK_INT, STK_STR, STR_ENTRY_VAR, STR_GLOBAL_VAR, STR_LITERAL,
    WHITE_SPACE, WIZ_DEFINED, lex, set,
};

/// §14: the shortest `.bbl` line `add_out_pool` breaks to.
const MIN_PRINT_LINE: usize = 3;

impl Bib<'_> {
    /// §293: `bst_ex_warn_print`.
    pub(crate) fn bst_ex_warn_print(&mut self) {
        if self.mess_with_entries {
            let s = self.cur_cite();
            self.print(&[&" for entry ", &s]);
        }
        self.print_newline();
        self.print(&[&"while executing-"]);
        self.bst_ln_num_print();
        self.mark_error();
    }

    /// §293: `bst_ex_warn`.
    pub(crate) fn bst_ex_warn(&mut self, pieces: &[&dyn Piece]) {
        self.print(pieces);
        self.bst_ex_warn_print();
    }

    /// §294: `bst_mild_ex_warn_print`.
    pub(crate) fn bst_mild_ex_warn_print(&mut self) {
        if self.mess_with_entries {
            let s = self.cur_cite();
            self.print(&[&" for entry ", &s]);
        }
        self.print_newline();
        self.bst_warn(&[&"while executing"]);
    }

    /// §295
    pub(crate) fn bst_cant_mess_with_entries_print(&mut self) {
        self.bst_ex_warn(&[&"You can't mess with entries here"]);
    }

    /// §307
    pub(crate) fn push(&mut self, lit: Lit) {
        self.lit_stack.push(lit);
    }

    pub(crate) fn push_int(&mut self, n: i64) {
        self.push(Lit::Int(n));
    }

    /// Push a string made by the command being executed.
    pub(crate) fn push_new(&mut self, s: Vec<u8>) {
        self.push(Lit::Str(s.into(), true));
    }

    /// Push the null string.
    pub(crate) fn push_null(&mut self) {
        let s = self.s_null.clone();
        self.push(Lit::Str(s, false));
    }

    /// §309
    pub(crate) fn pop(&mut self) -> Lit {
        if let Some(l) = self.lit_stack.pop() {
            l
        } else {
            self.bst_ex_warn(&[&"You can't pop an empty literal stack"]);
            Lit::Empty
        }
    }

    /// §311
    pub(crate) fn print_stk_lit(&mut self, lit: &Lit) -> R {
        match lit {
            Lit::Int(n) => self.print(&[n, &" is an integer literal"]),
            Lit::Str(s, _) => self.print(&[&"\"", s, &"\" is a string literal"]),
            Lit::Fn(loc) => {
                let s = self.t.text(*loc).clone();
                self.print(&[&"`", &s, &"' is a function literal"]);
            }
            Lit::Missing(s) => self.print(&[&"`", s, &"' is a missing field"]),
            Lit::Empty => return Err(self.confusion("Illegal literal type")),
        }
        Ok(())
    }

    /// §312
    pub(crate) fn print_wrong_stk_lit(&mut self, lit: &Lit, want: u8) -> R {
        if !matches!(lit, Lit::Empty) {
            self.print_stk_lit(lit)?;
            match want {
                STK_INT => self.print(&[&", not an integer,"]),
                STK_STR => self.print(&[&", not a string,"]),
                STK_FN => self.print(&[&", not a function,"]),
                _ => return Err(self.confusion("Illegal literal type")),
            }
            self.bst_ex_warn_print();
        }
        Ok(())
    }

    /// §313
    pub(crate) fn print_lit(&mut self, lit: &Lit) -> R {
        match lit {
            Lit::Int(n) => self.print_ln(&[n]),
            Lit::Str(s, _) | Lit::Missing(s) => self.print_ln(&[s]),
            Lit::Fn(loc) => {
                let s = self.t.text(*loc).clone();
                self.print_ln(&[&s]);
            }
            Lit::Empty => return Err(self.confusion("Illegal literal type")),
        }
        Ok(())
    }

    /// §314
    pub(crate) fn pop_top_and_print(&mut self) -> R {
        let lit = self.pop();
        if matches!(lit, Lit::Empty) {
            self.print_ln(&[&"Empty literal"]);
            Ok(())
        } else {
            self.print_lit(&lit)
        }
    }

    /// §315
    pub(crate) fn pop_whole_stack(&mut self) -> R {
        while !self.lit_stack.is_empty() {
            self.pop_top_and_print()?;
        }
        Ok(())
    }

    /// §316
    pub(crate) fn init_command_execution(&mut self) {
        self.lit_stack.clear();
    }

    /// §317
    pub(crate) fn check_command_execution(&mut self) -> R {
        if !self.lit_stack.is_empty() {
            let n = self.lit_stack.len();
            self.print_ln(&[&"ptr=", &n, &", stack="]);
            self.pop_whole_stack()?;
            self.bst_ex_warn(&[&"---the literal stack isn't empty"]);
        }
        Ok(())
    }

    /// §321
    pub(crate) fn output_bbl_line(&mut self) {
        self.out_touch();
        if self.out_buf_length != 0 {
            while self.out_buf_length > 0
                && lex(self.out_buf[self.out_buf_length - 1]) == WHITE_SPACE
            {
                self.out_buf_length -= 1;
            }
            if self.out_buf_length == 0 {
                return;
            }
            self.bbl
                .extend_from_slice(&self.out_buf[..self.out_buf_length]);
        }
        self.bbl.push(b'\n');
        self.bbl_line_num += 1;
        self.out_buf_length = 0;
    }

    /// §322: append `s` to the output buffer, breaking long lines.
    pub(crate) fn add_out_pool(&mut self, s: &[u8]) {
        self.out_touch();
        for (i, &c) in s.iter().enumerate() {
            set(&mut self.out_buf, self.out_buf_length + i, c);
        }
        self.out_buf_length += s.len();
        let max = self.opts.max_print_line;
        let mut unbreakable_tail = false;
        while self.out_buf_length > max && !unbreakable_tail {
            // §323: break that line
            let end_ptr = self.out_buf_length;
            let mut p = max;
            let mut break_pt_found = false;
            let at = |b: &Self, i: usize| b.out_buf.get(i).copied().unwrap_or(0);
            while lex(at(self, p)) != WHITE_SPACE && p >= MIN_PRINT_LINE {
                p -= 1;
            }
            if p == MIN_PRINT_LINE - 1 {
                // §324: break that unbreakably long line
                p = max + 1;
                while p < end_ptr && lex(at(self, p)) != WHITE_SPACE {
                    p += 1;
                }
                if p == end_ptr {
                    unbreakable_tail = true;
                } else {
                    break_pt_found = true;
                    while p + 1 < end_ptr && lex(at(self, p + 1)) == WHITE_SPACE {
                        p += 1;
                    }
                }
            } else {
                break_pt_found = true;
            }
            if break_pt_found {
                self.out_buf_length = p;
                let break_ptr = p + 1;
                self.output_bbl_line();
                self.out_buf[0] = b' ';
                self.out_buf[1] = b' ';
                self.out_buf.copy_within(break_ptr..end_ptr, 2);
                self.out_buf_length = end_ptr - break_ptr + 2;
            }
        }
    }

    /// §325: execute the function at `loc`.
    pub(crate) fn execute_fn(&mut self, loc: Loc) -> R {
        match self.t.fn_type(loc) {
            BUILT_IN => self.execute_built_in(loc),
            WIZ_DEFINED => {
                // §326
                let mut wiz_ptr = self.t.info(loc) as usize;
                loop {
                    let f = self
                        .wiz_functions
                        .get(wiz_ptr)
                        .copied()
                        .unwrap_or(END_OF_DEF);
                    if f == END_OF_DEF {
                        break;
                    }
                    if f == QUOTE_NEXT_FN {
                        wiz_ptr += 1;
                        let f = self.wiz_functions[wiz_ptr];
                        self.push(Lit::Fn(f));
                    } else {
                        self.execute_fn(f)?;
                    }
                    wiz_ptr += 1;
                }
                Ok(())
            }
            INT_LITERAL => {
                let n = self.t.info(loc);
                self.push_int(n);
                Ok(())
            }
            INT_GLOBAL_VAR => {
                let n = self.glb_int(loc);
                self.push_int(n);
                Ok(())
            }
            STR_LITERAL => {
                let s = self.t.text(loc).clone();
                self.push(Lit::Str(s, false));
                Ok(())
            }
            FIELD => {
                // §327
                if self.mess_with_entries {
                    let lit = if let Some(s) = self.field(self.t.info(loc) as usize) {
                        Lit::Str(s, false)
                    } else {
                        Lit::Missing(self.t.text(loc).clone())
                    };
                    self.push(lit);
                } else {
                    self.bst_cant_mess_with_entries_print();
                }
                Ok(())
            }
            INT_ENTRY_VAR => {
                // §328
                if self.mess_with_entries {
                    let n = self.ent_int(self.t.info(loc) as usize);
                    self.push_int(n);
                } else {
                    self.bst_cant_mess_with_entries_print();
                }
                Ok(())
            }
            STR_ENTRY_VAR => {
                // §329
                if self.mess_with_entries {
                    let v = self.ent_str(self.t.info(loc) as usize);
                    self.push_new(v);
                } else {
                    self.bst_cant_mess_with_entries_print();
                }
                Ok(())
            }
            STR_GLOBAL_VAR => {
                // §330
                let (kept, copy) = self.glb_str(self.t.info(loc) as usize);
                let lit = match kept {
                    Some(s) => Lit::Str(s, false),
                    None => Lit::Str(copy, true),
                };
                self.push(lit);
                Ok(())
            }
            _ => Err(self.confusion("Unknown function class")),
        }
    }
}
