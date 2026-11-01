//! The built-in functions (§331–454) and the pre-defined strings.

use alloc::vec::Vec;

use crate::aux::{N_AUX_BIBDATA, N_AUX_BIBSTYLE, N_AUX_CITATION, N_AUX_INPUT};
use crate::bib::{N_BIB_COMMENT, N_BIB_PREAMBLE, N_BIB_STRING};
use crate::bst::{
    N_BST_ENTRY, N_BST_EXECUTE, N_BST_FUNCTION, N_BST_INTEGERS, N_BST_ITERATE, N_BST_MACRO,
    N_BST_READ, N_BST_REVERSE, N_BST_SORT, N_BST_STRINGS,
};
use crate::table::{
    AUX_COMMAND_ILK, BIB_COMMAND_ILK, BST_COMMAND_ILK, BST_FN_ILK, CONTROL_SEQ_ILK, FILE_AREA_ILK,
    FILE_EXT_ILK, Loc, Str, TEXT_ILK,
};
use crate::{
    ALPHA, BUILT_IN, Bib, EMPTY, FIELD, INT_ENTRY_VAR, INT_GLOBAL_VAR, Lit, NUMERIC, R, SEP_CHAR,
    STK_FN, STK_INT, STK_STR, STR_ENTRY_VAR, STR_GLOBAL_VAR, STR_LITERAL, UNDEFINED, WHITE_SPACE,
    lex, set,
};

/// §333: the built-in functions, in the order of their numbers.
const BUILT_INS: [&str; NUM_BLT_IN_FNS] = [
    "=",
    ">",
    "<",
    "+",
    "-",
    "*",
    ":=",
    "add.period$",
    "call.type$",
    "change.case$",
    "chr.to.int$",
    "cite$",
    "duplicate$",
    "empty$",
    "format.name$",
    "if$",
    "int.to.chr$",
    "int.to.str$",
    "missing$",
    "newline$",
    "num.names$",
    "pop$",
    "preamble$",
    "purify$",
    "quote$",
    "skip$",
    "stack$",
    "substring$",
    "swap$",
    "text.length$",
    "text.prefix$",
    "top$",
    "type$",
    "warning$",
    "while$",
    "width$",
    "write$",
];
pub const NUM_BLT_IN_FNS: usize = 37;
const N_SKIP: usize = 25;

// §338: the control sequences for foreign characters
const N_I: i64 = 0;
const N_J: i64 = 1;
const N_OE: i64 = 2;
const N_OE_UPPER: i64 = 3;
const N_AE: i64 = 4;
const N_AE_UPPER: i64 = 5;
const N_AA: i64 = 6;
const N_AA_UPPER: i64 = 7;
const N_O: i64 = 8;
const N_O_UPPER: i64 = 9;
const N_L: i64 = 10;
const N_L_UPPER: i64 = 11;
const N_SS: i64 = 12;

// §365: case conversions
const TITLE_LOWERS: u8 = 0;
const ALL_LOWERS: u8 = 1;
const ALL_UPPERS: u8 = 2;
const BAD_CONVERSION: u8 = 3;

/// §35: character widths in cmr10, in units of 1/1000 em (`@'40`–`@'176`).
const CHAR_WIDTH: [i64; 95] = [
    278, 278, 500, 833, 500, 833, 778, 278, 389, 389, 500, 778, 278, 333, 278, 500, // 40–57
    500, 500, 500, 500, 500, 500, 500, 500, 500, 500, 278, 278, 278, 778, 472, 472, // 60–77
    778, 750, 708, 722, 764, 681, 653, 785, 750, 361, 514, 778, 625, 917, 750,
    778, // 100–117
    681, 778, 736, 556, 722, 750, 750, 1028, 750, 750, 611, 278, 500, 278, 500,
    278, // 120–137
    278, 500, 556, 444, 556, 444, 306, 500, 556, 278, 306, 528, 278, 833, 556,
    500, // 140–157
    556, 528, 392, 394, 389, 556, 528, 722, 528, 528, 444, 500, 1000, 500, 500, // 160–176
];

fn char_width(c: u8) -> i64 {
    match c {
        0x20..=0x7e => CHAR_WIDTH[(c - 0x20) as usize],
        _ => 0,
    }
}

/// The parts of a name found by `format.name$` (§395), as token indices.
#[derive(Default, Clone, Copy)]
struct Parts {
    first_start: i64,
    first_end: i64,
    last_end: i64,
    von_start: i64,
    von_end: i64,
    jr_end: i64,
}

impl Bib<'_> {
    /// §75, §79, §334, §339, §340: pre-define certain strings.
    pub(crate) fn pre_def_certain_strings(&mut self) {
        for ext in [".aux", ".bbl", ".blg", ".bst", ".bib"] {
            self.t.insert(FILE_EXT_ILK, ext.as_bytes());
        }
        for area in ["texinputs:", "texbib:"] {
            self.t.insert(FILE_AREA_ILK, area.as_bytes());
        }
        let commands: [(&str, u8, i64); 17] = [
            ("\\citation", AUX_COMMAND_ILK, N_AUX_CITATION),
            ("\\bibdata", AUX_COMMAND_ILK, N_AUX_BIBDATA),
            ("\\bibstyle", AUX_COMMAND_ILK, N_AUX_BIBSTYLE),
            ("\\@input", AUX_COMMAND_ILK, N_AUX_INPUT),
            ("entry", BST_COMMAND_ILK, N_BST_ENTRY),
            ("execute", BST_COMMAND_ILK, N_BST_EXECUTE),
            ("function", BST_COMMAND_ILK, N_BST_FUNCTION),
            ("integers", BST_COMMAND_ILK, N_BST_INTEGERS),
            ("iterate", BST_COMMAND_ILK, N_BST_ITERATE),
            ("macro", BST_COMMAND_ILK, N_BST_MACRO),
            ("read", BST_COMMAND_ILK, N_BST_READ),
            ("reverse", BST_COMMAND_ILK, N_BST_REVERSE),
            ("sort", BST_COMMAND_ILK, N_BST_SORT),
            ("strings", BST_COMMAND_ILK, N_BST_STRINGS),
            ("comment", BIB_COMMAND_ILK, N_BIB_COMMENT),
            ("preamble", BIB_COMMAND_ILK, N_BIB_PREAMBLE),
            ("string", BIB_COMMAND_ILK, N_BIB_STRING),
        ];
        for (name, ilk, n) in commands {
            let (loc, _) = self.t.insert(ilk, name.as_bytes());
            self.t.set_info(loc, n);
        }
        // §334–335
        for (n, name) in BUILT_INS.iter().enumerate() {
            let (loc, _) = self.t.insert(BST_FN_ILK, name.as_bytes());
            self.t.set_fn_type(loc, BUILT_IN);
            self.t.set_info(loc, n as i64);
            self.blt_in_loc[n] = loc;
        }
        self.b_skip = self.blt_in_loc[N_SKIP];
        // §339
        let (loc, _) = self.t.insert(TEXT_ILK, b"");
        self.s_null = self.t.text(loc).clone();
        self.t.set_fn_type(loc, STR_LITERAL);
        let (loc, _) = self.t.insert(TEXT_ILK, b"default.type");
        self.t.set_fn_type(loc, STR_LITERAL);
        self.b_default = self.b_skip;
        let seqs = [
            "i", "j", "oe", "OE", "ae", "AE", "aa", "AA", "o", "O", "l", "L", "ss",
        ];
        for (n, name) in seqs.iter().enumerate() {
            let (loc, _) = self.t.insert(CONTROL_SEQ_ILK, name.as_bytes());
            self.t.set_info(loc, n as i64);
        }
        // §340
        let (loc, _) = self.t.insert(BST_FN_ILK, b"crossref");
        self.t.set_fn_type(loc, FIELD);
        self.t.set_info(loc, self.num_fields as i64);
        self.crossref_num = self.num_fields;
        self.num_fields += 1;
        self.num_pre_defined_fields = self.num_fields;
        let (loc, _) = self.t.insert(BST_FN_ILK, b"sort.key$");
        self.t.set_fn_type(loc, STR_ENTRY_VAR);
        self.t.set_info(loc, self.num_ent_strs as i64);
        self.sort_key_num = self.num_ent_strs;
        self.num_ent_strs += 1;
        let (loc, _) = self.t.insert(BST_FN_ILK, b"entry.max$");
        self.t.set_fn_type(loc, INT_GLOBAL_VAR);
        self.t.set_info(loc, self.opts.ent_str_size as i64);
        let (loc, _) = self.t.insert(BST_FN_ILK, b"global.max$");
        self.t.set_fn_type(loc, INT_GLOBAL_VAR);
        self.t.set_info(loc, self.opts.glob_str_size as i64);
    }

    /// §341: execute a built-in function.
    pub(crate) fn execute_built_in(&mut self, loc: Loc) -> R {
        let n = self.t.info(loc) as usize;
        self.execution_count[n] += 1;
        match BUILT_INS[n] {
            "=" => self.x_equals(),
            ">" => self.x_compare(|a, b| a > b),
            "<" => self.x_compare(|a, b| a < b),
            "+" => self.x_arith(i64::wrapping_add),
            "-" => self.x_arith(i64::wrapping_sub),
            "*" => self.x_concatenate(),
            ":=" => self.x_gets(),
            "add.period$" => self.x_add_period(),
            "call.type$" => {
                // §363
                if !self.mess_with_entries {
                    self.bst_cant_mess_with_entries_print();
                    return Ok(());
                }
                match self.entry_type() {
                    UNDEFINED => self.execute_fn(self.b_default),
                    EMPTY => Ok(()),
                    t => self.execute_fn(t),
                }
            }
            "change.case$" => self.x_change_case(),
            "chr.to.int$" => self.x_chr_to_int(),
            "cite$" => {
                // §378
                if self.mess_with_entries {
                    let s = self.cur_cite();
                    self.push(Lit::Str(s, false));
                } else {
                    self.bst_cant_mess_with_entries_print();
                }
                Ok(())
            }
            "duplicate$" => {
                // §379
                let l = self.pop();
                self.push(l.clone());
                self.push(l);
                Ok(())
            }
            "empty$" => self.x_empty(),
            "format.name$" => self.x_format_name(),
            "if$" => {
                // §421
                let (l1, l2, l3) = (self.pop(), self.pop(), self.pop());
                match (&l1, &l2, &l3) {
                    (Lit::Fn(f1), Lit::Fn(f2), Lit::Int(n)) => {
                        self.execute_fn(if *n > 0 { *f2 } else { *f1 })
                    }
                    (Lit::Fn(_), Lit::Fn(_), _) => self.print_wrong_stk_lit(&l3, STK_INT),
                    (Lit::Fn(_), _, _) => self.print_wrong_stk_lit(&l2, STK_FN),
                    _ => self.print_wrong_stk_lit(&l1, STK_FN),
                }
            }
            "int.to.chr$" => {
                // §422
                match self.pop() {
                    Lit::Int(n) if (0..=127).contains(&n) => self.push_new(alloc::vec![n as u8]),
                    Lit::Int(n) => {
                        self.bst_ex_warn(&[&n, &" isn't valid ASCII"]);
                        self.push_null();
                    }
                    l => {
                        self.print_wrong_stk_lit(&l, STK_INT)?;
                        self.push_null();
                    }
                }
                Ok(())
            }
            "int.to.str$" => {
                // §423
                match self.pop() {
                    Lit::Int(n) => {
                        let mut v = Vec::new();
                        crate::Piece::put(&n, &mut v);
                        self.push_new(v);
                    }
                    l => {
                        self.print_wrong_stk_lit(&l, STK_INT)?;
                        self.push_null();
                    }
                }
                Ok(())
            }
            "missing$" => {
                // §424
                let l = self.pop();
                if !self.mess_with_entries {
                    self.bst_cant_mess_with_entries_print();
                } else if let Lit::Missing(_) = l {
                    self.push_int(1);
                } else if let Lit::Str(..) = l {
                    self.push_int(0);
                } else {
                    if !matches!(l, Lit::Empty) {
                        self.print_stk_lit(&l)?;
                        self.bst_ex_warn(&[&", not a string or missing field,"]);
                    }
                    self.push_int(0);
                }
                Ok(())
            }
            "newline$" => {
                self.output_bbl_line();
                Ok(())
            }
            "num.names$" => {
                // §426–427
                match self.pop() {
                    Lit::Str(s, _) => {
                        self.ex_buf = s.to_vec();
                        let mut ptr = 0;
                        let mut n = 0;
                        while ptr < s.len() {
                            self.name_scan_for_and(&mut ptr, &s);
                            n += 1;
                        }
                        self.push_int(n);
                    }
                    l => {
                        self.print_wrong_stk_lit(&l, STK_STR)?;
                        self.push_int(0);
                    }
                }
                Ok(())
            }
            "pop$" => {
                self.pop();
                Ok(())
            }
            "preamble$" => {
                // §429
                let v = self.preamble();
                self.push_new(v);
                Ok(())
            }
            "purify$" => self.x_purify(),
            "quote$" => {
                self.push_new(alloc::vec![b'"']);
                Ok(())
            }
            "skip$" => Ok(()),
            "stack$" => self.pop_whole_stack(),
            "substring$" => self.x_substring(),
            "swap$" => {
                // §439
                let (l1, l2) = (self.pop(), self.pop());
                self.push(l1);
                self.push(l2);
                Ok(())
            }
            "text.length$" => {
                // §441–442
                match self.pop() {
                    Lit::Str(s, _) => {
                        let (_, n, _) = text_chars(&s, i64::MAX);
                        self.push_int(n);
                    }
                    l => {
                        self.print_wrong_stk_lit(&l, STK_STR)?;
                        self.push_null();
                    }
                }
                Ok(())
            }
            "text.prefix$" => self.x_text_prefix(),
            "top$" => self.pop_top_and_print(),
            "type$" => {
                // §447
                if self.mess_with_entries {
                    match self.entry_type() {
                        UNDEFINED | EMPTY => self.push_null(),
                        t => {
                            let s = self.t.text(t).clone();
                            self.push(Lit::Str(s, false));
                        }
                    }
                } else {
                    self.bst_cant_mess_with_entries_print();
                }
                Ok(())
            }
            "warning$" => {
                // §448
                let l = self.pop();
                if let Lit::Str(..) = l {
                    self.print(&[&"Warning--"]);
                    self.print_lit(&l)?;
                    self.mark_warning();
                    Ok(())
                } else {
                    self.print_wrong_stk_lit(&l, STK_STR)
                }
            }
            "while$" => {
                // §449
                let (r1, r2) = (self.pop(), self.pop());
                let (Lit::Fn(body), Lit::Fn(test)) = (&r1, &r2) else {
                    return if let Lit::Fn(_) = r1 {
                        self.print_wrong_stk_lit(&r2, STK_FN)
                    } else {
                        self.print_wrong_stk_lit(&r1, STK_FN)
                    };
                };
                let (body, test) = (*body, *test);
                loop {
                    self.execute_fn(test)?;
                    match self.pop() {
                        Lit::Int(n) if n > 0 => self.execute_fn(body)?,
                        Lit::Int(_) => break,
                        l => {
                            self.print_wrong_stk_lit(&l, STK_INT)?;
                            break;
                        }
                    }
                }
                Ok(())
            }
            "width$" => self.x_width(),
            "write$" => {
                // §454
                match self.pop() {
                    Lit::Str(s, _) => {
                        self.add_out_pool(&s);
                        Ok(())
                    }
                    l => self.print_wrong_stk_lit(&l, STK_STR),
                }
            }
            _ => Err(self.confusion("Unknown built-in function")),
        }
    }

    /// §345
    fn x_equals(&mut self) -> R {
        let (l1, l2) = (self.pop(), self.pop());
        let v = match (&l1, &l2) {
            (Lit::Int(a), Lit::Int(b)) => a == b,
            (Lit::Str(a, _), Lit::Str(b, _)) => a == b,
            _ if core::mem::discriminant(&l1) != core::mem::discriminant(&l2) => {
                if !matches!(l1, Lit::Empty) && !matches!(l2, Lit::Empty) {
                    self.print_stk_lit(&l1)?;
                    self.print(&[&", "]);
                    self.print_stk_lit(&l2)?;
                    self.print_newline();
                    self.bst_ex_warn(&[&"---they aren't the same literal types"]);
                }
                false
            }
            _ => {
                if !matches!(l1, Lit::Empty) {
                    self.print_stk_lit(&l1)?;
                    self.bst_ex_warn(&[&", not an integer or a string,"]);
                }
                false
            }
        };
        self.push_int(i64::from(v));
        Ok(())
    }

    /// Pop two integers (§346–349): `(second, first)`, or `None` after
    /// complaining and pushing 0.
    fn pop_two_ints(&mut self) -> R<Option<(i64, i64)>> {
        let (l1, l2) = (self.pop(), self.pop());
        match (&l1, &l2) {
            (Lit::Int(a), Lit::Int(b)) => Ok(Some((*b, *a))),
            (Lit::Int(_), _) => {
                self.print_wrong_stk_lit(&l2, STK_INT)?;
                self.push_int(0);
                Ok(None)
            }
            _ => {
                self.print_wrong_stk_lit(&l1, STK_INT)?;
                self.push_int(0);
                Ok(None)
            }
        }
    }

    /// §346–347
    fn x_compare(&mut self, f: fn(i64, i64) -> bool) -> R {
        if let Some((a, b)) = self.pop_two_ints()? {
            self.push_int(i64::from(f(a, b)));
        }
        Ok(())
    }

    /// §348–349
    fn x_arith(&mut self, f: fn(i64, i64) -> i64) -> R {
        if let Some((a, b)) = self.pop_two_ints()? {
            self.push_int(f(a, b));
        }
        Ok(())
    }

    /// §350–353
    fn x_concatenate(&mut self) -> R {
        let (l1, l2) = (self.pop(), self.pop());
        match (&l1, &l2) {
            (Lit::Str(s1, _), Lit::Str(s2, _)) => {
                if s2.is_empty() {
                    self.push(l1);
                } else if s1.is_empty() {
                    self.push(l2);
                } else {
                    let mut v = s2.to_vec();
                    v.extend_from_slice(s1);
                    self.push_new(v);
                }
            }
            (Lit::Str(..), _) => {
                self.print_wrong_stk_lit(&l2, STK_STR)?;
                self.push_null();
            }
            _ => {
                self.print_wrong_stk_lit(&l1, STK_STR)?;
                self.push_null();
            }
        }
        Ok(())
    }

    /// §356: `bst_string_size_exceeded`.
    fn bst_string_size_exceeded(&mut self, size: usize, what: &str) {
        self.print(&[&"Warning--you've exceeded ", &size, &what]);
        self.print(&[&"-string-size,"]);
        self.bst_mild_ex_warn_print();
        self.print_ln(&[&"*Please notify the bibstyle designer*"]);
    }

    /// §354–359
    fn x_gets(&mut self) -> R {
        let (l1, l2) = (self.pop(), self.pop());
        let Lit::Fn(f) = l1 else {
            return self.print_wrong_stk_lit(&l1, STK_FN);
        };
        let class = self.t.fn_type(f);
        if !self.mess_with_entries && (class == STR_ENTRY_VAR || class == INT_ENTRY_VAR) {
            self.bst_cant_mess_with_entries_print();
            return Ok(());
        }
        let idx = self.t.info(f) as usize;
        match (class, &l2) {
            (INT_ENTRY_VAR, Lit::Int(n)) => {
                self.set_ent_int(idx, *n);
            }
            (STR_ENTRY_VAR, Lit::Str(s, _)) => {
                // §357
                let mut v = s.to_vec();
                if v.len() > self.opts.ent_str_size {
                    self.bst_string_size_exceeded(self.opts.ent_str_size, ", the entry");
                    v.truncate(self.opts.ent_str_size);
                }
                self.set_ent_str(idx, v);
            }
            (INT_GLOBAL_VAR, Lit::Int(n)) => self.set_glb_int(f, *n),
            (STR_GLOBAL_VAR, Lit::Str(s, made)) => {
                // §359: a string of the database or style is kept as it
                // is; one computed by this command is copied
                if *made {
                    let mut v = s.to_vec();
                    if v.len() > self.opts.glob_str_size {
                        self.bst_string_size_exceeded(self.opts.glob_str_size, ", the global");
                        v.truncate(self.opts.glob_str_size);
                    }
                    self.set_glb_str(idx, None, v);
                } else {
                    self.set_glb_str(idx, Some(s.clone()), Vec::new());
                }
            }
            (INT_ENTRY_VAR | INT_GLOBAL_VAR, _) => self.print_wrong_stk_lit(&l2, STK_INT)?,
            (STR_ENTRY_VAR | STR_GLOBAL_VAR, _) => self.print_wrong_stk_lit(&l2, STK_STR)?,
            _ => {
                self.print(&[&"You can't assign to type "]);
                self.print_fn_class(f);
                self.bst_ex_warn(&[&", a nonvariable function class"]);
            }
        }
        Ok(())
    }

    /// §360–362
    fn x_add_period(&mut self) -> R {
        let l = self.pop();
        let Lit::Str(s, _) = &l else {
            self.print_wrong_stk_lit(&l, STK_STR)?;
            self.push_null();
            return Ok(());
        };
        if s.is_empty() {
            self.push_null();
            return Ok(());
        }
        let last = s.iter().rposition(|&c| c != b'}').unwrap_or(0);
        if matches!(s[last], b'.' | b'?' | b'!') {
            self.push(l);
        } else {
            let mut v = s.to_vec();
            v.push(b'.');
            self.push_new(v);
        }
        Ok(())
    }

    /// §367: `decr_brace_level`.
    fn decr_brace_level(&mut self, s: &Str) {
        if self.brace_level == 0 {
            self.braces_unbalanced_complaint(s);
        } else {
            self.brace_level -= 1;
        }
    }

    /// §368
    fn braces_unbalanced_complaint(&mut self, s: &Str) {
        self.print(&[&"Warning--\"", s]);
        self.print(&[&"\" isn't a brace-balanced string"]);
        self.bst_mild_ex_warn_print();
    }

    /// §369
    fn check_brace_level(&mut self, s: &Str) {
        if self.brace_level > 0 {
            self.braces_unbalanced_complaint(s);
        }
    }

    fn control_seq(&self, from: usize, to: usize) -> Option<i64> {
        self.t
            .find(CONTROL_SEQ_ILK, &self.ex_buf[from..to])
            .map(|loc| self.t.info(loc))
    }

    /// §364–376
    fn x_change_case(&mut self) -> R {
        let (l1, l2) = (self.pop(), self.pop());
        let (Lit::Str(s1, _), Lit::Str(s2, _)) = (&l1, &l2) else {
            if let Lit::Str(..) = l1 {
                self.print_wrong_stk_lit(&l2, STK_STR)?;
            } else {
                self.print_wrong_stk_lit(&l1, STK_STR)?;
            }
            self.push_null();
            return Ok(());
        };
        let (s1, s2) = (s1.clone(), s2.clone());
        // §366
        let mut conversion = match s1.first() {
            Some(b't' | b'T') => TITLE_LOWERS,
            Some(b'l' | b'L') => ALL_LOWERS,
            Some(b'u' | b'U') => ALL_UPPERS,
            _ => BAD_CONVERSION,
        };
        if s1.len() != 1 || conversion == BAD_CONVERSION {
            conversion = BAD_CONVERSION;
            self.print(&[&s1]);
            self.bst_ex_warn(&[&" is an illegal case-conversion string"]);
        }
        self.ex_buf = s2.to_vec();
        let mut len = self.ex_buf.len();
        // §370
        self.brace_level = 0;
        let mut ptr = 0;
        while ptr < len {
            let c = self.ex_buf[ptr];
            if c == b'{' {
                self.brace_level += 1;
                let give_up = self.brace_level != 1
                    || ptr + 4 > len
                    || self.ex_buf[ptr + 1] != b'\\'
                    || (conversion == TITLE_LOWERS
                        && (ptr == 0
                            || (self.prev_colon && lex(self.ex_buf[ptr - 1]) == WHITE_SPACE)));
                if !give_up {
                    // §371: convert a special character
                    ptr += 1;
                    while ptr < len && self.brace_level > 0 {
                        ptr += 1;
                        let mut xptr = ptr;
                        while ptr < len && lex(self.ex_buf[ptr]) == ALPHA {
                            ptr += 1;
                        }
                        if let Some(cs) = self.control_seq(xptr, ptr) {
                            // §372
                            match conversion {
                                TITLE_LOWERS | ALL_LOWERS => {
                                    if matches!(
                                        cs,
                                        N_L_UPPER
                                            | N_O_UPPER
                                            | N_OE_UPPER
                                            | N_AE_UPPER
                                            | N_AA_UPPER
                                    ) {
                                        self.ex_buf[xptr..ptr].make_ascii_lowercase();
                                    }
                                }
                                ALL_UPPERS => match cs {
                                    N_L | N_O | N_OE | N_AE | N_AA => {
                                        self.ex_buf[xptr..ptr].make_ascii_uppercase();
                                    }
                                    N_I | N_J | N_SS => {
                                        // §374: convert, then remove the control sequence
                                        self.ex_buf[xptr..ptr].make_ascii_uppercase();
                                        self.ex_buf.copy_within(xptr..ptr, xptr - 1);
                                        xptr = ptr - 1;
                                        while ptr < len && lex(self.ex_buf[ptr]) == WHITE_SPACE {
                                            ptr += 1;
                                        }
                                        self.ex_buf.copy_within(ptr..len, xptr);
                                        len -= ptr - xptr;
                                        ptr = xptr;
                                    }
                                    _ => {}
                                },
                                _ => {}
                            }
                        }
                        let xptr = ptr;
                        while ptr < len && self.brace_level > 0 && self.ex_buf[ptr] != b'\\' {
                            match self.ex_buf[ptr] {
                                b'}' => self.brace_level -= 1,
                                b'{' => self.brace_level += 1,
                                _ => {}
                            }
                            ptr += 1;
                        }
                        // §375: convert a noncontrol sequence
                        match conversion {
                            TITLE_LOWERS | ALL_LOWERS => {
                                self.ex_buf[xptr..ptr].make_ascii_lowercase();
                            }
                            ALL_UPPERS => self.ex_buf[xptr..ptr].make_ascii_uppercase(),
                            _ => {}
                        }
                    }
                    ptr -= 1;
                }
                self.prev_colon = false;
            } else if c == b'}' {
                self.decr_brace_level(&s2);
                self.prev_colon = false;
            } else if self.brace_level == 0 {
                // §376
                match conversion {
                    TITLE_LOWERS => {
                        if ptr != 0
                            && !(self.prev_colon && lex(self.ex_buf[ptr - 1]) == WHITE_SPACE)
                        {
                            self.ex_buf[ptr].make_ascii_lowercase();
                        }
                        if self.ex_buf[ptr] == b':' {
                            self.prev_colon = true;
                        } else if lex(self.ex_buf[ptr]) != WHITE_SPACE {
                            self.prev_colon = false;
                        }
                    }
                    ALL_LOWERS => self.ex_buf[ptr].make_ascii_lowercase(),
                    ALL_UPPERS => self.ex_buf[ptr].make_ascii_uppercase(),
                    _ => {}
                }
            }
            ptr += 1;
        }
        self.check_brace_level(&s2);
        let v = self.ex_buf[..len].to_vec();
        self.push_new(v);
        Ok(())
    }

    /// §377
    fn x_chr_to_int(&mut self) -> R {
        match self.pop() {
            Lit::Str(s, _) if s.len() == 1 => self.push_int(i64::from(s[0])),
            Lit::Str(s, _) => {
                self.print(&[&"\"", &s]);
                self.bst_ex_warn(&[&"\" isn't a single character"]);
                self.push_int(0);
            }
            l => {
                self.print_wrong_stk_lit(&l, STK_STR)?;
                self.push_int(0);
            }
        }
        Ok(())
    }

    /// §380–381
    fn x_empty(&mut self) -> R {
        match self.pop() {
            Lit::Str(s, _) => {
                let blank = s.iter().all(|&c| lex(c) == WHITE_SPACE);
                self.push_int(i64::from(blank));
            }
            Lit::Missing(_) => self.push_int(1),
            Lit::Empty => self.push_int(0),
            l => {
                self.print_stk_lit(&l)?;
                self.bst_ex_warn(&[&", not a string or missing field,"]);
                self.push_int(0);
            }
        }
        Ok(())
    }

    /// §384: advance `ptr` in `ex_buf` past the next name and its "and".
    fn name_scan_for_and(&mut self, ptr: &mut usize, s: &Str) {
        let len = s.len();
        self.brace_level = 0;
        let mut preceding_white = false;
        let mut and_found = false;
        while !and_found && *ptr < len {
            match self.ex_buf[*ptr] {
                b'a' | b'A' => {
                    *ptr += 1;
                    if preceding_white
                        && *ptr + 3 <= len
                        && matches!(self.ex_buf[*ptr], b'n' | b'N')
                        && matches!(self.ex_buf[*ptr + 1], b'd' | b'D')
                        && lex(self.ex_buf[*ptr + 2]) == WHITE_SPACE
                    {
                        // §386
                        *ptr += 2;
                        and_found = true;
                    }
                    preceding_white = false;
                }
                b'{' => {
                    self.brace_level += 1;
                    *ptr += 1;
                    // §385
                    while self.brace_level > 0 && *ptr < len {
                        match self.ex_buf[*ptr] {
                            b'}' => self.brace_level -= 1,
                            b'{' => self.brace_level += 1,
                            _ => {}
                        }
                        *ptr += 1;
                    }
                    preceding_white = false;
                }
                b'}' => {
                    self.decr_brace_level(s);
                    *ptr += 1;
                    preceding_white = false;
                }
                c => {
                    *ptr += 1;
                    preceding_white = lex(c) == WHITE_SPACE;
                }
            }
        }
        self.check_brace_level(s);
    }

    /// The start of name token `i` in `name_buf` (§387).
    fn tok(&self, i: i64) -> usize {
        usize::try_from(i).map_or(0, |i| self.name_tok.get(i).copied().unwrap_or(0))
    }

    /// §397–400: whether the token in `name_buf[bf..xb]` starts with a
    /// lower-case letter (at brace level 0 or inside a special character).
    fn von_token_found(&self, mut bf: usize, xb: usize) -> bool {
        let nb = &self.name_buf;
        let mut nm_brace_level = 0i64;
        while bf < xb {
            let c = nb[bf];
            if c.is_ascii_uppercase() {
                return false;
            } else if c.is_ascii_lowercase() {
                return true;
            } else if c == b'{' {
                nm_brace_level += 1;
                bf += 1;
                if bf + 2 < xb && nb[bf] == b'\\' {
                    // §398: check the special character
                    bf += 1;
                    let yb = bf;
                    while bf < xb && lex(nb[bf]) == ALPHA {
                        bf += 1;
                    }
                    if let Some(loc) = self.t.find(CONTROL_SEQ_ILK, &nb[yb..bf]) {
                        // §399
                        return matches!(
                            self.t.info(loc),
                            N_I | N_J | N_OE | N_AE | N_AA | N_O | N_L | N_SS
                        );
                    }
                    while bf < xb && nm_brace_level > 0 {
                        let c = nb[bf];
                        if c.is_ascii_uppercase() {
                            return false;
                        } else if c.is_ascii_lowercase() {
                            return true;
                        } else if c == b'}' {
                            nm_brace_level -= 1;
                        } else if c == b'{' {
                            nm_brace_level += 1;
                        }
                        bf += 1;
                    }
                    return false;
                }
                // §400
                while nm_brace_level > 0 && bf < xb {
                    match nb[bf] {
                        b'}' => nm_brace_level -= 1,
                        b'{' => nm_brace_level += 1,
                        _ => {}
                    }
                    bf += 1;
                }
            } else {
                bf += 1;
            }
        }
        false
    }

    /// §401
    fn von_name_ends_and_last_name_starts_stuff(&self, p: &mut Parts) {
        p.von_end = p.last_end - 1;
        while p.von_end > p.von_start {
            if self.von_token_found(self.tok(p.von_end - 1), self.tok(p.von_end)) {
                return;
            }
            p.von_end -= 1;
        }
    }

    /// §382–420
    fn x_format_name(&mut self) -> R {
        let (l1, l2, l3) = (self.pop(), self.pop(), self.pop());
        let (Lit::Str(fmt, _), Lit::Int(n), Lit::Str(names, _)) = (&l1, &l2, &l3) else {
            match (&l1, &l2) {
                (Lit::Str(..), Lit::Int(_)) => self.print_wrong_stk_lit(&l3, STK_STR)?,
                (Lit::Str(..), _) => self.print_wrong_stk_lit(&l2, STK_INT)?,
                _ => self.print_wrong_stk_lit(&l1, STK_STR)?,
            }
            self.push_null();
            return Ok(());
        };
        let (fmt, n, names) = (fmt.clone(), *n, names.clone());
        self.ex_buf = names.to_vec();
        let len = names.len();
        // §383: isolate the desired name
        let mut ptr = 0;
        let mut xptr = 0;
        let mut num_names = 0;
        while num_names < n && ptr < len {
            num_names += 1;
            xptr = ptr;
            self.name_scan_for_and(&mut ptr, &names);
        }
        if num_names == 0 {
            // (nothing was scanned: no tokens, however bibtex.web's
            // pointers are left)
            ptr = 0;
            xptr = 0;
        } else if ptr < len {
            ptr -= 4;
        }
        if num_names < n {
            if n == 1 {
                self.print(&[&"There is no name in \""]);
            } else {
                self.print(&[&"There aren't ", &n, &" names in \""]);
            }
            self.print(&[&names]);
            self.bst_ex_warn(&[&"\""]);
        }
        // §388: remove trailing junk, complaining if necessary
        while ptr > xptr {
            match lex(self.ex_buf[ptr - 1]) {
                WHITE_SPACE | SEP_CHAR => ptr -= 1,
                _ if self.ex_buf[ptr - 1] == b',' => {
                    self.print(&[
                        &"Name ",
                        &n,
                        &" in \"",
                        &names,
                        &"\" has a comma at the end",
                    ]);
                    self.bst_ex_warn_print();
                    ptr -= 1;
                }
                _ => break,
            }
        }
        // §387: copy name and count commas to determine syntax
        let mut bf = 0;
        let mut num_commas = 0;
        let mut num_tokens = 0usize;
        let (mut comma1, mut comma2) = (0, 0);
        let mut token_starting = true;
        while xptr < ptr {
            let c = self.ex_buf[xptr];
            match c {
                b',' => {
                    // §389
                    if num_commas == 2 {
                        self.print(&[&"Too many commas in name ", &n, &" of \"", &names, &"\""]);
                        self.bst_ex_warn_print();
                    } else {
                        num_commas += 1;
                        if num_commas == 1 {
                            comma1 = num_tokens;
                        } else {
                            comma2 = num_tokens;
                        }
                        set(&mut self.name_sep_char, num_tokens, b',');
                    }
                    xptr += 1;
                    token_starting = true;
                }
                b'{' => {
                    // §390
                    self.brace_level += 1;
                    if token_starting {
                        set(&mut self.name_tok, num_tokens, bf);
                        num_tokens += 1;
                    }
                    set(&mut self.name_buf, bf, c);
                    bf += 1;
                    xptr += 1;
                    while self.brace_level > 0 && xptr < ptr {
                        match self.ex_buf[xptr] {
                            b'}' => self.brace_level -= 1,
                            b'{' => self.brace_level += 1,
                            _ => {}
                        }
                        set(&mut self.name_buf, bf, self.ex_buf[xptr]);
                        bf += 1;
                        xptr += 1;
                    }
                    token_starting = false;
                }
                b'}' => {
                    // §391
                    if token_starting {
                        set(&mut self.name_tok, num_tokens, bf);
                        num_tokens += 1;
                    }
                    self.print(&[&"Name ", &n, &" of \"", &names]);
                    self.bst_ex_warn(&[&"\" isn't brace balanced"]);
                    xptr += 1;
                    token_starting = false;
                }
                _ => match lex(c) {
                    WHITE_SPACE | SEP_CHAR => {
                        // §392–393
                        if !token_starting {
                            let sep = if lex(c) == WHITE_SPACE { b' ' } else { c };
                            set(&mut self.name_sep_char, num_tokens, sep);
                        }
                        xptr += 1;
                        token_starting = true;
                    }
                    _ => {
                        // §394
                        if token_starting {
                            set(&mut self.name_tok, num_tokens, bf);
                            num_tokens += 1;
                        }
                        set(&mut self.name_buf, bf, c);
                        bf += 1;
                        xptr += 1;
                        token_starting = false;
                    }
                },
            }
        }
        set(&mut self.name_tok, num_tokens, bf);
        // §395: find the parts of the name
        let num_tokens = num_tokens as i64;
        let mut p = Parts::default();
        if num_commas == 0 {
            {
                p.first_start = 0;
                p.last_end = num_tokens;
                p.jr_end = p.last_end;
                // §396: where the first name ends and the von name starts and ends
                p.von_start = 0;
                let mut found = false;
                while p.von_start < p.last_end - 1 {
                    if self.von_token_found(self.tok(p.von_start), self.tok(p.von_start + 1)) {
                        self.von_name_ends_and_last_name_starts_stuff(&mut p);
                        found = true;
                        break;
                    }
                    p.von_start += 1;
                }
                if !found {
                    while p.von_start > 0 {
                        let c = crate::get(&self.name_sep_char, p.von_start as usize);
                        if lex(c) != SEP_CHAR || c == b'~' {
                            break;
                        }
                        p.von_start -= 1;
                    }
                    p.von_end = p.von_start;
                }
                p.first_end = p.von_start;
            }
        } else {
            {
                p.von_start = 0;
                p.last_end = comma1 as i64;
                p.jr_end = if num_commas == 1 {
                    p.last_end
                } else {
                    comma2 as i64
                };
                p.first_start = p.jr_end;
                p.first_end = num_tokens;
                self.von_name_ends_and_last_name_starts_stuff(&mut p);
            }
        }
        // §402: figure out the formatted name
        self.ex_buf = fmt.to_vec();
        self.format_the_name(&fmt, &p);
        let v = self.ex_buf[..self.ex_buf_length].to_vec();
        self.push_new(v);
        Ok(())
    }

    /// Append to the formatted name (`append_ex_buf_char`).
    fn out(&mut self, c: u8) {
        set(&mut self.ex_buf, self.ex_buf_length, c);
        self.ex_buf_length += 1;
    }

    /// §402–419: write the name parts `p` as the format `fmt` says; the
    /// result is `ex_buf[..ex_buf_length]`.
    fn format_the_name(&mut self, fmt: &Str, p: &Parts) {
        let f = |i: usize| fmt.get(i).copied().unwrap_or(0);
        let sp_end = fmt.len();
        self.ex_buf_length = 0;
        let mut sp_brace_level = 0i64;
        let mut sp_ptr = 0;
        let skip_deeper = |sp_ptr: &mut usize, level: &mut i64| {
            // §404
            while *level > 1 && *sp_ptr < sp_end {
                match f(*sp_ptr) {
                    b'}' => *level -= 1,
                    b'{' => *level += 1,
                    _ => {}
                }
                *sp_ptr += 1;
            }
        };
        while sp_ptr < sp_end {
            match f(sp_ptr) {
                b'{' => {
                    sp_brace_level += 1;
                    sp_ptr += 1;
                    // §403: format this part of the name
                    let mut sp_xptr1 = sp_ptr;
                    let mut alpha_found = false;
                    let mut double_letter = false;
                    let mut end_of_group = false;
                    let mut to_be_written = true;
                    let (mut cur_token, mut last_token) = (0i64, 0i64);
                    while !end_of_group && sp_ptr < sp_end {
                        let c = f(sp_ptr);
                        if lex(c) == ALPHA {
                            sp_ptr += 1;
                            // §405: figure out what this letter means
                            if alpha_found {
                                self.brace_lvl_one_letters_complaint(fmt);
                                to_be_written = false;
                            } else {
                                let next = f(sp_ptr).to_ascii_lowercase();
                                let part = match c.to_ascii_lowercase() {
                                    b'f' => Some((p.first_start, p.first_end, b'f')),
                                    b'v' => Some((p.von_start, p.von_end, b'v')),
                                    b'l' => Some((p.von_end, p.last_end, b'l')),
                                    b'j' => Some((p.last_end, p.jr_end, b'j')),
                                    _ => None,
                                };
                                if let Some((a, b, letter)) = part {
                                    // §407–410
                                    cur_token = a;
                                    last_token = b;
                                    if cur_token == last_token {
                                        to_be_written = false;
                                    }
                                    if next == letter {
                                        double_letter = true;
                                    }
                                } else {
                                    self.brace_lvl_one_letters_complaint(fmt);
                                    to_be_written = false;
                                }
                                if double_letter {
                                    sp_ptr += 1;
                                }
                            }
                            alpha_found = true;
                        } else if c == b'}' {
                            sp_brace_level -= 1;
                            sp_ptr += 1;
                            end_of_group = true;
                        } else if c == b'{' {
                            sp_brace_level += 1;
                            sp_ptr += 1;
                            skip_deeper(&mut sp_ptr, &mut sp_brace_level);
                        } else {
                            sp_ptr += 1;
                        }
                    }
                    if end_of_group && to_be_written {
                        // §411: finally format this part of the name
                        let group_start = self.ex_buf_length;
                        sp_ptr = sp_xptr1;
                        sp_brace_level = 1;
                        while sp_brace_level > 0 && sp_ptr < sp_end {
                            let c = f(sp_ptr);
                            if lex(c) == ALPHA && sp_brace_level == 1 {
                                sp_ptr += 1;
                                // §412: figure out how to output the name tokens
                                if double_letter {
                                    sp_ptr += 1;
                                }
                                let mut use_default = true;
                                let mut sp_xptr2 = sp_ptr;
                                if f(sp_ptr) == b'{' {
                                    use_default = false;
                                    sp_brace_level += 1;
                                    sp_ptr += 1;
                                    sp_xptr1 = sp_ptr;
                                    skip_deeper(&mut sp_ptr, &mut sp_brace_level);
                                    sp_xptr2 = sp_ptr - 1;
                                }
                                // §413: finally output the name tokens
                                while cur_token < last_token {
                                    let (mut nb, xb) =
                                        (self.tok(cur_token), self.tok(cur_token + 1));
                                    if double_letter {
                                        // §414
                                        for i in nb..xb {
                                            let c = self.name_buf[i];
                                            self.out(c);
                                        }
                                    } else {
                                        // §415: an abbreviated token
                                        while nb < xb {
                                            let c = self.name_buf[nb];
                                            if lex(c) == ALPHA {
                                                self.out(c);
                                                break;
                                            } else if c == b'{'
                                                && nb + 1 < xb
                                                && self.name_buf[nb + 1] == b'\\'
                                            {
                                                // §416: a special character
                                                self.out(b'{');
                                                self.out(b'\\');
                                                nb += 2;
                                                let mut nm_brace_level = 1;
                                                while nb < xb && nm_brace_level > 0 {
                                                    let c = self.name_buf[nb];
                                                    if c == b'}' {
                                                        nm_brace_level -= 1;
                                                    } else if c == b'{' {
                                                        nm_brace_level += 1;
                                                    }
                                                    self.out(c);
                                                    nb += 1;
                                                }
                                                break;
                                            }
                                            nb += 1;
                                        }
                                    }
                                    cur_token += 1;
                                    if cur_token < last_token {
                                        // §417: the inter-token string
                                        if use_default {
                                            if !double_letter {
                                                self.out(b'.');
                                            }
                                            let sep =
                                                crate::get(&self.name_sep_char, cur_token as usize);
                                            if lex(sep) == SEP_CHAR {
                                                self.out(sep);
                                            } else if cur_token == last_token - 1
                                                || !self.enough_text_chars(3, group_start)
                                            {
                                                self.out(b'~');
                                            } else {
                                                self.out(b' ');
                                            }
                                        } else {
                                            for i in sp_xptr1..sp_xptr2 {
                                                self.out(f(i));
                                            }
                                        }
                                    }
                                }
                                if !use_default {
                                    sp_ptr = sp_xptr2 + 1;
                                }
                            } else if c == b'}' {
                                sp_brace_level -= 1;
                                sp_ptr += 1;
                                if sp_brace_level > 0 {
                                    self.out(b'}');
                                }
                            } else if c == b'{' {
                                sp_brace_level += 1;
                                sp_ptr += 1;
                                self.out(b'{');
                            } else {
                                self.out(c);
                                sp_ptr += 1;
                            }
                        }
                        if self.ex_buf_length > 0 && self.ex_buf[self.ex_buf_length - 1] == b'~' {
                            // §419: handle a discretionary tie
                            self.ex_buf_length -= 1;
                            let before = self.ex_buf_length.checked_sub(1).map(|i| self.ex_buf[i]);
                            if before == Some(b'~') {
                            } else if !self.enough_text_chars(3, group_start) {
                                self.ex_buf_length += 1;
                            } else {
                                self.out(b' ');
                            }
                        }
                    }
                }
                b'}' => {
                    self.braces_unbalanced_complaint(fmt);
                    sp_ptr += 1;
                }
                c => {
                    self.out(c);
                    sp_ptr += 1;
                }
            }
        }
        if sp_brace_level > 0 {
            self.braces_unbalanced_complaint(fmt);
        }
    }

    /// §406
    fn brace_lvl_one_letters_complaint(&mut self, fmt: &Str) {
        self.print(&[&"The format string \"", fmt]);
        self.bst_ex_warn(&[&"\" has an illegal brace-level-1 letter"]);
    }

    /// §418: whether the formatted text from `from` has at least `enough`
    /// text characters (special characters count as one).
    fn enough_text_chars(&mut self, enough: i64, from: usize) -> bool {
        let end = self.ex_buf_length;
        let mut num = 0;
        let mut y = from;
        while y < end && num < enough {
            y += 1;
            match self.ex_buf[y - 1] {
                b'{' => {
                    self.brace_level += 1;
                    if self.brace_level == 1 && y < end && self.ex_buf[y] == b'\\' {
                        y += 1;
                        while y < end && self.brace_level > 0 {
                            match self.ex_buf[y] {
                                b'}' => self.brace_level -= 1,
                                b'{' => self.brace_level += 1,
                                _ => {}
                            }
                            y += 1;
                        }
                    }
                }
                b'}' => self.brace_level -= 1,
                _ => {}
            }
            num += 1;
        }
        num >= enough
    }

    /// §430–433
    fn x_purify(&mut self) -> R {
        let l = self.pop();
        let Lit::Str(s, _) = &l else {
            self.print_wrong_stk_lit(&l, STK_STR)?;
            self.push_null();
            return Ok(());
        };
        let mut ex = s.to_vec();
        let len = ex.len();
        self.brace_level = 0;
        let mut xptr = 0;
        let mut ptr = 0;
        while ptr < len {
            match lex(ex[ptr]) {
                WHITE_SPACE | SEP_CHAR => {
                    ex[xptr] = b' ';
                    xptr += 1;
                }
                ALPHA | NUMERIC => {
                    ex[xptr] = ex[ptr];
                    xptr += 1;
                }
                _ => {
                    if ex[ptr] == b'{' {
                        self.brace_level += 1;
                        if self.brace_level == 1 && ptr + 1 < len && ex[ptr + 1] == b'\\' {
                            // §432: purify a special character
                            ptr += 1;
                            while ptr < len && self.brace_level > 0 {
                                ptr += 1;
                                let yptr = ptr;
                                while ptr < len && lex(ex[ptr]) == ALPHA {
                                    ptr += 1;
                                }
                                if let Some(loc) = self.t.find(CONTROL_SEQ_ILK, &ex[yptr..ptr]) {
                                    // §433
                                    ex[xptr] = ex[yptr];
                                    xptr += 1;
                                    if matches!(
                                        self.t.info(loc),
                                        N_OE | N_OE_UPPER | N_AE | N_AE_UPPER | N_SS
                                    ) {
                                        ex[xptr] = ex[yptr + 1];
                                        xptr += 1;
                                    }
                                }
                                while ptr < len && self.brace_level > 0 && ex[ptr] != b'\\' {
                                    match lex(ex[ptr]) {
                                        ALPHA | NUMERIC => {
                                            ex[xptr] = ex[ptr];
                                            xptr += 1;
                                        }
                                        _ => {
                                            if ex[ptr] == b'}' {
                                                self.brace_level -= 1;
                                            } else if ex[ptr] == b'{' {
                                                self.brace_level += 1;
                                            }
                                        }
                                    }
                                    ptr += 1;
                                }
                            }
                            ptr -= 1;
                        }
                    } else if ex[ptr] == b'}' && self.brace_level > 0 {
                        self.brace_level -= 1;
                    }
                }
            }
            ptr += 1;
        }
        ex.truncate(xptr);
        self.push_new(ex);
        Ok(())
    }

    /// §437–438
    fn x_substring(&mut self) -> R {
        let (l1, l2, l3) = (self.pop(), self.pop(), self.pop());
        let (Lit::Int(n), Lit::Int(start), Lit::Str(s, _)) = (&l1, &l2, &l3) else {
            match (&l1, &l2) {
                (Lit::Int(_), Lit::Int(_)) => self.print_wrong_stk_lit(&l3, STK_STR)?,
                (Lit::Int(_), _) => self.print_wrong_stk_lit(&l2, STK_INT)?,
                _ => self.print_wrong_stk_lit(&l1, STK_INT)?,
            }
            self.push_null();
            return Ok(());
        };
        let (mut n, mut start) = (*n, *start);
        let sp_length = s.len() as i64;
        if n >= sp_length && (start == 1 || start == -1) {
            self.push(l3.clone());
            return Ok(());
        }
        if n <= 0 || start == 0 || start > sp_length || start < -sp_length {
            self.push_null();
            return Ok(());
        }
        let (from, to) = if start > 0 {
            if n > sp_length - (start - 1) {
                n = sp_length - (start - 1);
            }
            (start - 1, start - 1 + n)
        } else {
            start = -start;
            if n > sp_length - (start - 1) {
                n = sp_length - (start - 1);
            }
            let end = sp_length - (start - 1);
            (end - n, end)
        };
        let v = s[from as usize..to as usize].to_vec();
        self.push_new(v);
        Ok(())
    }

    /// §443–445
    fn x_text_prefix(&mut self) -> R {
        let (l1, l2) = (self.pop(), self.pop());
        let (Lit::Int(n), Lit::Str(s, _)) = (&l1, &l2) else {
            if let Lit::Int(_) = l1 {
                self.print_wrong_stk_lit(&l2, STK_STR)?;
            } else {
                self.print_wrong_stk_lit(&l1, STK_INT)?;
            }
            self.push_null();
            return Ok(());
        };
        if *n <= 0 {
            self.push_null();
            return Ok(());
        }
        let (end, _, level) = text_chars(s, *n);
        let mut v = s[..end].to_vec();
        v.extend(core::iter::repeat_n(b'}', level as usize));
        self.push_new(v);
        Ok(())
    }

    /// §450–453
    fn x_width(&mut self) -> R {
        let l = self.pop();
        let Lit::Str(s, _) = &l else {
            self.print_wrong_stk_lit(&l, STK_STR)?;
            self.push_int(0);
            return Ok(());
        };
        let s = s.clone();
        self.ex_buf = s.to_vec();
        let len = s.len();
        let mut width = 0i64;
        self.brace_level = 0;
        let mut ptr = 0;
        while ptr < len {
            let c = self.ex_buf[ptr];
            if c == b'{' {
                self.brace_level += 1;
                if self.brace_level == 1 && ptr + 1 < len && self.ex_buf[ptr + 1] == b'\\' {
                    // §452: the width of a special character
                    ptr += 1;
                    while ptr < len && self.brace_level > 0 {
                        ptr += 1;
                        let xptr = ptr;
                        while ptr < len && lex(self.ex_buf[ptr]) == ALPHA {
                            ptr += 1;
                        }
                        if ptr < len && ptr == xptr {
                            ptr += 1;
                        } else if let Some(cs) = self.control_seq(xptr, ptr) {
                            // §453
                            width += match cs {
                                N_SS => 500,
                                N_AE => 722,
                                N_OE => 778,
                                N_AE_UPPER => 903,
                                N_OE_UPPER => 1014,
                                _ => char_width(self.ex_buf[xptr]),
                            };
                        }
                        while ptr < len && lex(self.ex_buf[ptr]) == WHITE_SPACE {
                            ptr += 1;
                        }
                        while ptr < len && self.brace_level > 0 && self.ex_buf[ptr] != b'\\' {
                            match self.ex_buf[ptr] {
                                b'}' => self.brace_level -= 1,
                                b'{' => self.brace_level += 1,
                                c => width += char_width(c),
                            }
                            ptr += 1;
                        }
                    }
                    ptr -= 1;
                } else {
                    width += char_width(b'{');
                }
            } else if c == b'}' {
                self.decr_brace_level(&s);
                width += char_width(b'}');
            } else {
                width += char_width(c);
            }
            ptr += 1;
        }
        self.check_brace_level(&s);
        self.push_int(width);
        Ok(())
    }
}

/// §442, §445: scan `s` for up to `limit` text characters (a special
/// character counts as one): where the scan stopped, the characters
/// counted, and the brace level there.
fn text_chars(s: &[u8], limit: i64) -> (usize, i64, i64) {
    let end = s.len();
    let (mut p, mut num, mut level) = (0, 0i64, 0i64);
    while p < end && num < limit {
        p += 1;
        match s[p - 1] {
            b'{' => {
                level += 1;
                if level == 1 && p < end && s[p] == b'\\' {
                    p += 1;
                    while p < end && level > 0 {
                        match s[p] {
                            b'}' => level -= 1,
                            b'{' => level += 1,
                            _ => {}
                        }
                        p += 1;
                    }
                    num += 1;
                }
            }
            b'}' => {
                if level > 0 {
                    level -= 1;
                }
            }
            _ => num += 1,
        }
    }
    (p, num, level)
}
