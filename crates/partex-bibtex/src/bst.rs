//! Reading the style file (§146–162) and its commands (§163–217).

use alloc::vec::Vec;

use crate::table::{BST_COMMAND_ILK, BST_FN_ILK, INTEGER_ILK, Loc, MACRO_ILK, TEXT_ILK};
use crate::{
    BUILT_IN, Bib, Ch, END_OF_DEF, FIELD, ID_NULL, INT_ENTRY_VAR, INT_GLOBAL_VAR, INT_LITERAL,
    OTHER_CHAR_ADJACENT, Piece, QUOTE_NEXT_FN, R, SPECIFIED_CHAR_ADJACENT, STR_ENTRY_VAR,
    STR_GLOBAL_VAR, STR_LITERAL, Src, Stop, WHITE_ADJACENT, WHITE_SPACE, WIZ_DEFINED, lex,
};

// §78
pub const N_BST_ENTRY: i64 = 0;
pub const N_BST_EXECUTE: i64 = 1;
pub const N_BST_FUNCTION: i64 = 2;
pub const N_BST_INTEGERS: i64 = 3;
pub const N_BST_ITERATE: i64 = 4;
pub const N_BST_MACRO: i64 = 5;
pub const N_BST_READ: i64 = 6;
pub const N_BST_REVERSE: i64 = 7;
pub const N_BST_SORT: i64 = 8;
pub const N_BST_STRINGS: i64 = 9;

/// A style-file error has been reported; the command is abandoned
/// (bibtex.web's `return` after `bst_err`).
pub struct BstErr;

/// The result of a step of a style command: `Err(Ok(BstErr))` abandons
/// the command, `Err(Err(stop))` stops the program.
type B<T = ()> = Result<T, Result<BstErr, Stop>>;

fn stop(s: Stop) -> Result<BstErr, Stop> {
    Err(s)
}

impl Bib<'_> {
    /// §151: read and execute the `.bst` file.
    pub(crate) fn read_bst(&mut self) -> R {
        if self.bst_str.is_none() {
            return Ok(());
        }
        self.bst_line_num = 0;
        self.bbl_line_num = 1;
        self.buf_ptr2 = self.last;
        loop {
            if !self.eat_bst_white_space() {
                break;
            }
            match self.get_bst_command_and_process() {
                Ok(()) | Err(Ok(BstErr)) => {}
                Err(Err(Stop::BstDone)) => break,
                Err(Err(Stop::Fatal)) => return Err(Stop::Fatal),
            }
        }
        Ok(())
    }

    /// §148
    pub(crate) fn bst_ln_num_print(&mut self) {
        let n = self.bst_line_num;
        self.print(&[&"--line ", &n, &" of file "]);
        self.print_bst_name();
    }

    /// §149: `bst_err_print_and_look_for_blank_line`.
    fn bst_err_print(&mut self) -> Result<BstErr, Stop> {
        self.print(&[&"-"]);
        self.bst_ln_num_print();
        self.print_bad_input_line();
        while self.last != 0 {
            if !self.input_ln(Src::Bst) {
                return Err(Stop::BstDone);
            }
            self.bst_line_num += 1;
        }
        self.buf_ptr2 = self.last;
        Ok(BstErr)
    }

    /// §149: `bst_err`.
    fn bst_err(&mut self, pieces: &[&dyn Piece]) -> Result<BstErr, Stop> {
        self.print(pieces);
        self.bst_err_print()
    }

    /// §150: `bst_warn`.
    pub(crate) fn bst_warn(&mut self, pieces: &[&dyn Piece]) {
        self.print(pieces);
        self.bst_ln_num_print();
        self.mark_warning();
    }

    /// §152
    fn eat_bst_white_space(&mut self) -> bool {
        loop {
            if self.scan_white_space() && self.scan_char() != b'%' {
                return true;
            }
            if !self.input_ln(Src::Bst) {
                return false;
            }
            self.bst_line_num += 1;
            self.buf_ptr2 = 0;
        }
    }

    /// §153: `eat_bst_white_and_eof_check`.
    fn eat_bst(&mut self, cmd: &str) -> B {
        if !self.eat_bst_white_space() {
            self.print(&[&"Illegal end of style file in command: "]);
            return Err(self.bst_err(&[&cmd]));
        }
        Ok(())
    }

    /// §154
    fn get_bst_command_and_process(&mut self) -> B {
        if !self.scan_alpha() {
            let c = self.scan_char();
            return Err(self.bst_err(&[&"\"", &Ch(c), &"\" can't start a style-file command"]));
        }
        self.lower_token();
        let Some(loc) = self.find_token(BST_COMMAND_ILK) else {
            self.print_token();
            return Err(self.bst_err(&[&" is an illegal style-file command"]));
        };
        // §155
        match self.t.info(loc) {
            N_BST_ENTRY => self.bst_entry_command(),
            N_BST_EXECUTE => self.bst_execute_command(),
            N_BST_FUNCTION => self.bst_function_command(),
            N_BST_INTEGERS => self.bst_integers_command(),
            N_BST_ITERATE => self.bst_iterate_command(),
            N_BST_MACRO => self.bst_macro_command(),
            N_BST_READ => self.bst_read_command(),
            N_BST_REVERSE => self.bst_reverse_command(),
            N_BST_SORT => self.bst_sort_command(),
            N_BST_STRINGS => self.bst_strings_command(),
            _ => Err(stop(self.confusion("Unknown style-file command"))),
        }
    }

    /// §158
    pub(crate) fn print_fn_class(&mut self, loc: Loc) {
        let s = match self.t.fn_type(loc) {
            BUILT_IN => "built-in",
            WIZ_DEFINED => "wizard-defined",
            INT_LITERAL => "integer-literal",
            STR_LITERAL => "string-literal",
            FIELD => "field",
            INT_ENTRY_VAR => "integer-entry-variable",
            STR_ENTRY_VAR => "string-entry-variable",
            INT_GLOBAL_VAR => "integer-global-variable",
            _ => "string-global-variable",
        };
        self.print(&[&s]);
    }

    /// §166: `bst_identifier_scan`.
    fn bst_identifier_scan(&mut self, cmd: &str) -> B {
        self.scan_identifier(b'}', b'%', b'%');
        if self.scan_result == WHITE_ADJACENT || self.scan_result == SPECIFIED_CHAR_ADJACENT {
            return Ok(());
        }
        let c = self.scan_char();
        if self.scan_result == ID_NULL {
            self.print(&[&"\"", &Ch(c), &"\" begins identifier, command: "]);
        } else if self.scan_result == OTHER_CHAR_ADJACENT {
            self.print(&[
                &"\"",
                &Ch(c),
                &"\" immediately follows identifier, command: ",
            ]);
        } else {
            return Err(stop(self.confusion("Identifier scanning error")));
        }
        Err(self.bst_err(&[&cmd]))
    }

    /// §167–168: `bst_get_and_check_left_brace` and `…right_brace`.
    fn bst_brace(&mut self, brace: u8, cmd: &str) -> B {
        if self.scan_char() != brace {
            self.print(&[&"\"", &Ch(brace), &"\" is missing in command: "]);
            return Err(self.bst_err(&[&cmd]));
        }
        self.buf_ptr2 += 1;
        Ok(())
    }

    /// §169: `check_for_already_seen_function`.
    fn check_already_seen(&mut self, found: bool, loc: Loc) -> B {
        if found {
            let s = self.t.text(loc).clone();
            self.print(&[&s, &" is already a type \""]);
            self.print_fn_class(loc);
            self.print_ln(&[&"\" function name"]);
            return Err(self.bst_err_print());
        }
        Ok(())
    }

    /// Enter the current token (lower-cased) as a function of class `class`
    /// with information `info` (§172, §174, §176, §202, §216).
    fn insert_fn(&mut self, class: u8, info: i64) -> B<Loc> {
        self.lower_token();
        let (loc, found) = self.insert_token(BST_FN_ILK);
        self.check_already_seen(found, loc)?;
        self.t.set_fn_type(loc, class);
        self.t.set_info(loc, info);
        Ok(loc)
    }

    /// §171, §173, §175, §201, §215: a brace-delimited list of names, each
    /// entered by `enter`.
    fn bst_name_list(&mut self, cmd: &str, mut enter: impl FnMut(&mut Self) -> B) -> B {
        self.bst_brace(b'{', cmd)?;
        self.eat_bst(cmd)?;
        while self.scan_char() != b'}' {
            self.bst_identifier_scan(cmd)?;
            enter(self)?;
            self.eat_bst(cmd)?;
        }
        self.buf_ptr2 += 1;
        Ok(())
    }

    /// §170
    fn bst_entry_command(&mut self) -> B {
        if self.entry_seen {
            return Err(self.bst_err(&[&"Illegal, another entry command"]));
        }
        self.entry_seen = true;
        self.eat_bst("entry")?;
        self.bst_name_list("entry", |b| {
            let n = b.num_fields as i64;
            b.insert_fn(FIELD, n)?;
            b.num_fields += 1;
            Ok(())
        })?;
        self.eat_bst("entry")?;
        if self.num_fields == self.num_pre_defined_fields {
            self.bst_warn(&[&"Warning--I didn't find any fields"]);
        }
        self.bst_name_list("entry", |b| {
            let n = b.num_ent_ints as i64;
            b.insert_fn(INT_ENTRY_VAR, n)?;
            b.num_ent_ints += 1;
            Ok(())
        })?;
        self.eat_bst("entry")?;
        self.bst_name_list("entry", |b| {
            let n = b.num_ent_strs as i64;
            b.insert_fn(STR_ENTRY_VAR, n)?;
            b.num_ent_strs += 1;
            Ok(())
        })
    }

    /// §177: `bad_argument_token`: the function named by the token, if it
    /// may be executed.
    fn argument_token(&mut self) -> B<Loc> {
        self.lower_token();
        let Some(loc) = self.find_token(BST_FN_ILK) else {
            self.print_token();
            return Err(self.bst_err(&[&" is an unknown function"]));
        };
        let t = self.t.fn_type(loc);
        if t != BUILT_IN && t != WIZ_DEFINED {
            self.print_token();
            self.print(&[&" has bad function type "]);
            self.print_fn_class(loc);
            return Err(self.bst_err_print());
        }
        Ok(loc)
    }

    /// The `{function}` argument of `execute`, `iterate` and `reverse`
    /// (§178, §203, §212), after the read-command check.
    fn bst_fn_argument(&mut self, cmd: &str) -> B<Loc> {
        if !self.read_seen {
            return Err(self.bst_err(&[&"Illegal, ", &cmd, &" command before read command"]));
        }
        self.eat_bst(cmd)?;
        self.bst_brace(b'{', cmd)?;
        self.eat_bst(cmd)?;
        self.bst_identifier_scan(cmd)?;
        let loc = self.argument_token()?;
        self.eat_bst(cmd)?;
        self.bst_brace(b'}', cmd)?;
        Ok(loc)
    }

    /// §178
    fn bst_execute_command(&mut self) -> B {
        let loc = self.bst_fn_argument("execute")?;
        // §296
        self.init_command_execution();
        self.mess_with_entries = false;
        self.execute_fn(loc).map_err(stop)?;
        self.check_command_execution().map_err(stop)
    }

    /// §180
    fn bst_function_command(&mut self) -> B {
        self.eat_bst("function")?;
        // §181
        self.bst_brace(b'{', "function")?;
        self.eat_bst("function")?;
        self.bst_identifier_scan("function")?;
        // §182
        self.lower_token();
        let (loc, found) = self.insert_token(BST_FN_ILK);
        self.wiz_loc = loc;
        self.check_already_seen(found, loc)?;
        self.t.set_fn_type(loc, WIZ_DEFINED);
        if &**self.t.text(loc) == b"default.type" {
            self.b_default = loc;
        }
        self.eat_bst("function")?;
        self.bst_brace(b'}', "function")?;
        self.eat_bst("function")?;
        self.bst_brace(b'{', "function")?;
        self.scan_fn_def(loc)
    }

    /// §183: `skip_token_print`.
    fn skip_token_print(&mut self) {
        self.print(&[&"-"]);
        self.bst_ln_num_print();
        self.mark_error();
        self.scan2_white(b'}', b'%');
    }

    /// §187: scan a function definition (after its left brace).
    fn scan_fn_def(&mut self, fn_hash_loc: Loc) -> B {
        self.eat_bst("function")?;
        let mut single: Vec<Loc> = Vec::new();
        while self.scan_char() != b'}' {
            // §189: get the next function of the definition
            match self.scan_char() {
                b'#' => {
                    // §190
                    self.buf_ptr2 += 1;
                    if self.scan_integer() {
                        let (loc, found) = self.insert_token(INTEGER_ILK);
                        if !found {
                            self.t.set_fn_type(loc, INT_LITERAL);
                            self.t.set_info(loc, self.token_value);
                        }
                        if self.literal_followed_badly() {
                            self.skip_illegal_stuff_after_literal();
                        } else {
                            single.push(loc);
                        }
                    } else {
                        self.print(&[&"Illegal integer in integer literal"]);
                        self.skip_token_print();
                    }
                }
                b'"' => {
                    // §191
                    self.buf_ptr2 += 1;
                    if self.scan1(b'"') {
                        let (loc, _) = self.insert_token(TEXT_ILK);
                        self.t.set_fn_type(loc, STR_LITERAL);
                        self.buf_ptr2 += 1;
                        if self.literal_followed_badly() {
                            self.skip_illegal_stuff_after_literal();
                        } else {
                            single.push(loc);
                        }
                    } else {
                        self.print(&[&"No `", &Ch(b'"'), &"' to end string literal"]);
                        self.skip_token_print();
                    }
                }
                b'\'' => {
                    // §192
                    self.buf_ptr2 += 1;
                    self.scan2_white(b'}', b'%');
                    self.lower_token();
                    match self.find_token(BST_FN_ILK) {
                        None => self.skip_token_unknown_function(),
                        Some(loc) if loc == self.wiz_loc => self.skip_recursive_token(),
                        Some(loc) => {
                            // §193
                            single.push(QUOTE_NEXT_FN);
                            single.push(loc);
                        }
                    }
                }
                b'{' => {
                    // §194: start a new (implicit) function definition
                    let mut name = alloc::vec![b'\''];
                    crate::Piece::put(&self.impl_fn_num, &mut name);
                    let (loc, found) = self.t.insert(BST_FN_ILK, &name);
                    if found {
                        return Err(stop(
                            self.confusion("Already encountered implicit function"),
                        ));
                    }
                    self.impl_fn_num += 1;
                    self.t.set_fn_type(loc, WIZ_DEFINED);
                    single.push(QUOTE_NEXT_FN);
                    single.push(loc);
                    self.buf_ptr2 += 1;
                    match self.scan_fn_def(loc) {
                        Ok(()) | Err(Ok(BstErr)) => {}
                        Err(Err(s)) => return Err(Err(s)),
                    }
                }
                _ => {
                    // §199
                    self.scan2_white(b'}', b'%');
                    self.lower_token();
                    match self.find_token(BST_FN_ILK) {
                        None => self.skip_token_unknown_function(),
                        Some(loc) if loc == self.wiz_loc => self.skip_recursive_token(),
                        Some(loc) => single.push(loc),
                    }
                }
            }
            // next_token:
            self.eat_bst("function")?;
        }
        // §200: complete this function's definition
        single.push(END_OF_DEF);
        let start = self.wiz_functions.len() as i64;
        self.t.set_info(fn_hash_loc, start);
        self.wiz_functions.extend_from_slice(&single);
        self.buf_ptr2 += 1;
        Ok(())
    }

    /// §190–191: whether a literal is followed by something other than
    /// white space, the end of the line, a right brace or a comment.
    fn literal_followed_badly(&self) -> bool {
        let c = self.scan_char();
        lex(c) != WHITE_SPACE && self.buf_ptr2 < self.last && c != b'}' && c != b'%'
    }

    /// §186
    fn skip_illegal_stuff_after_literal(&mut self) {
        let c = self.scan_char();
        self.print(&[&"\"", &Ch(c), &"\" can't follow a literal"]);
        self.skip_token_print();
    }

    /// §185
    fn skip_token_unknown_function(&mut self) {
        self.print_token();
        self.print(&[&" is an unknown function"]);
        self.skip_token_print();
    }

    /// §184
    fn skip_recursive_token(&mut self) {
        self.print_ln(&[&"Curse you, wizard, before you recurse me:"]);
        self.print(&[&"function "]);
        self.print_token();
        self.print_ln(&[&" is illegal in its own definition"]);
        self.skip_token_print();
    }

    /// §201
    fn bst_integers_command(&mut self) -> B {
        self.eat_bst("integers")?;
        self.bst_name_list("integers", |b| b.insert_fn(INT_GLOBAL_VAR, 0).map(|_| ()))
    }

    /// §203
    fn bst_iterate_command(&mut self) -> B {
        let loc = self.bst_fn_argument("iterate")?;
        // §297
        self.init_command_execution();
        self.mess_with_entries = true;
        for i in 0..self.num_cites {
            self.cite_ptr = self.sorted_cites[i];
            self.execute_fn(loc).map_err(stop)?;
            self.check_command_execution().map_err(stop)?;
        }
        Ok(())
    }

    /// §205
    fn bst_macro_command(&mut self) -> B {
        if self.read_seen {
            return Err(self.bst_err(&[&"Illegal, macro command after read command"]));
        }
        self.eat_bst("macro")?;
        // §206: the macro name
        self.bst_brace(b'{', "macro")?;
        self.eat_bst("macro")?;
        self.bst_identifier_scan("macro")?;
        // §207
        self.lower_token();
        let (name_loc, found) = self.insert_token(MACRO_ILK);
        if found {
            self.print_token();
            return Err(self.bst_err(&[&" is already defined as a macro"]));
        }
        let own = self.t.text_id(name_loc);
        self.t.set_info(name_loc, own);
        self.eat_bst("macro")?;
        self.bst_brace(b'}', "macro")?;
        self.eat_bst("macro")?;
        // §208: its definition
        self.bst_brace(b'{', "macro")?;
        self.eat_bst("macro")?;
        if self.scan_char() != b'"' {
            return Err(self.bst_err(&[&"A macro definition must be ", &Ch(b'"'), &"-delimited"]));
        }
        // §209
        self.buf_ptr2 += 1;
        if !self.scan1(b'"') {
            return Err(self.bst_err(&[&"There's no `", &Ch(b'"'), &"' to end macro definition"]));
        }
        let (def_loc, _) = self.insert_token(TEXT_ILK);
        self.t.set_fn_type(def_loc, STR_LITERAL);
        let id = self.t.text_id(def_loc);
        self.t.set_info(name_loc, id);
        self.buf_ptr2 += 1;
        self.eat_bst("macro")?;
        self.bst_brace(b'}', "macro")
    }

    /// §211
    fn bst_read_command(&mut self) -> B {
        if self.read_seen {
            return Err(self.bst_err(&[&"Illegal, another read command"]));
        }
        self.read_seen = true;
        if !self.entry_seen {
            return Err(self.bst_err(&[&"Illegal, read command before entry command"]));
        }
        // save the rest of the .bst line
        let (sv_ptr1, sv_ptr2) = (self.buf_ptr2.min(self.last), self.last);
        let saved = self.buffer[sv_ptr1..sv_ptr2].to_vec();
        self.read_bib_files().map_err(stop)?;
        self.buf_ptr2 = sv_ptr1;
        self.last = sv_ptr2;
        self.buffer[sv_ptr1..sv_ptr2].copy_from_slice(&saved);
        Ok(())
    }

    /// §212
    fn bst_reverse_command(&mut self) -> B {
        let loc = self.bst_fn_argument("reverse")?;
        // §298
        self.init_command_execution();
        self.mess_with_entries = true;
        for i in (0..self.num_cites).rev() {
            self.cite_ptr = self.sorted_cites[i];
            self.execute_fn(loc).map_err(stop)?;
            self.check_command_execution().map_err(stop)?;
        }
        Ok(())
    }

    /// §214
    fn bst_sort_command(&mut self) -> B {
        if !self.read_seen {
            return Err(self.bst_err(&[&"Illegal, sort command before read command"]));
        }
        // §299–306: `less_than` orders entries by `sort.key$`, ties by
        // position, so the order is total and any sort gives bibtex's
        let key = |b: &Self, c: usize| -> Vec<u8> {
            let s = &b.entry_strs[c * b.num_ent_strs + b.sort_key_num];
            let end = s
                .iter()
                .position(|&x| x == crate::END_OF_STRING)
                .unwrap_or(s.len());
            s[..end].to_vec()
        };
        let n = self.num_cites;
        let keys: Vec<Vec<u8>> = (0..n).map(|c| key(self, c)).collect();
        let mut sorted = core::mem::take(&mut self.sorted_cites);
        sorted[..n].sort_unstable_by(|&a, &b| keys[a].cmp(&keys[b]).then(a.cmp(&b)));
        self.sorted_cites = sorted;
        Ok(())
    }

    /// §215
    fn bst_strings_command(&mut self) -> B {
        self.eat_bst("strings")?;
        self.bst_name_list("strings", |b| {
            let n = b.num_glb_strs as i64;
            b.insert_fn(STR_GLOBAL_VAR, n)?;
            b.num_glb_strs += 1;
            crate::set(&mut b.glb_strs, b.num_glb_strs - 1, (None, Vec::new()));
            Ok(())
        })
    }
}
