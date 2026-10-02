//! Reading the database files (§218–289).

use alloc::vec::Vec;

use crate::table::{
    BIB_COMMAND_ILK, BST_FN_ILK, CITE_ILK, LC_CITE_ILK, Loc, MACRO_ILK, Str, TEXT_ILK,
};
use crate::{
    Bib, Ch, EMPTY, FIELD, ID_NULL, OTHER_CHAR_ADJACENT, Piece, R, SPECIFIED_CHAR_ADJACENT,
    STR_LITERAL, Src, Stop, UNDEFINED, WHITE_ADJACENT, WHITE_SPACE, WIZ_DEFINED, get, lex, set,
};

// §78
pub const N_BIB_COMMENT: i64 = 0;
pub const N_BIB_PREAMBLE: i64 = 1;
pub const N_BIB_STRING: i64 = 2;

/// How a step of reading an entry ended early.
enum Ex {
    /// An error was reported and the rest of the entry is skipped
    /// (bibtex.web's `return`), or the database file ended.
    Ret,
    Stop(Stop),
}

type D<T = ()> = Result<T, Ex>;

impl From<Stop> for Ex {
    fn from(s: Stop) -> Self {
        Ex::Stop(s)
    }
}

impl Bib<'_> {
    /// §223: read the `.bib` file(s).
    pub(crate) fn read_bib_files(&mut self) -> R {
        // §224–227: final initialization for .bib processing
        self.field_info = alloc::vec![None; self.num_fields * self.num_cites];
        let n = self.num_cites;
        self.type_list = alloc::vec![EMPTY; n];
        self.cite_info = alloc::vec![0; n];
        self.cite_info_str = alloc::vec![self.s_null.clone(); n];
        self.entry_exists = alloc::vec![false; n];
        self.old_num_cites = self.num_cites;
        if self.all_entries {
            for i in self.all_marker..self.old_num_cites {
                self.cite_info_str[i] = self.cite_list[i].clone();
                self.entry_exists[i] = false;
            }
            self.cite_ptr = self.all_marker;
        } else {
            self.cite_ptr = self.num_cites;
            self.all_marker = 0;
        }
        self.read_performed = true;
        self.bib_ptr = 0;
        while self.bib_ptr < self.num_bib_files {
            let k = self.bib_ptr + 1;
            let name = self.bib_name();
            self.progress(&[&"Database file #", &k, &": ", &&name[..]]);
            self.bib_line_num = 0;
            self.buf_ptr2 = self.last;
            while !self.eof(Src::Bib) {
                match self.get_bib_command_or_entry_and_process() {
                    Ok(()) | Err(Ex::Ret) => {}
                    Err(Ex::Stop(s)) => return Err(s),
                }
            }
            self.bib_ptr += 1;
        }
        self.reading_completed = true;
        self.final_initialization_for_entries()?;
        self.read_completed = true;
        Ok(())
    }

    /// §220
    fn bib_ln_num_print(&mut self) {
        let n = self.bib_line_num;
        self.print(&[&"--line ", &n, &" of file "]);
        self.print_bib_name();
    }

    /// §221: `bib_err`.
    fn bib_err(&mut self, pieces: &[&dyn Piece]) -> Ex {
        self.print(pieces);
        self.print(&[&"-"]);
        self.bib_ln_num_print();
        self.print_bad_input_line();
        self.print_skipping_whatever_remains();
        if self.at_bib_command {
            self.print_ln(&[&"command"]);
        } else {
            self.print_ln(&[&"entry"]);
        }
        Ex::Ret
    }

    /// §222: `bib_warn_print`.
    fn bib_warn_print(&mut self) {
        self.bib_ln_num_print();
        self.mark_warning();
    }

    /// §222: `bib_warn_newline`.
    fn bib_warn_newline(&mut self, pieces: &[&dyn Piece]) {
        self.print_ln(pieces);
        self.bib_warn_print();
    }

    /// §228
    fn eat_bib_white_space(&mut self) -> bool {
        while !self.scan_white_space() {
            if !self.input_ln(Src::Bib) {
                return false;
            }
            self.bib_line_num += 1;
            self.buf_ptr2 = 0;
        }
        true
    }

    /// §229: `eat_bib_white_and_eof_check`.
    fn eat_bib(&mut self) -> D {
        if self.eat_bib_white_space() {
            Ok(())
        } else {
            Err(self.bib_err(&[&"Illegal end of database file"]))
        }
    }

    /// §230
    fn bib_one_of_two(&mut self, c1: u8, c2: u8) -> Ex {
        self.bib_err(&[&"I was expecting a `", &Ch(c1), &"' or a `", &Ch(c2), &"'"])
    }

    /// §231
    fn bib_equals_sign_expected(&mut self) -> Ex {
        self.bib_err(&[&"I was expecting an \"", &Ch(b'='), &"\""])
    }

    /// §234: `macro_name_warning`.
    fn macro_name_warning(&mut self, what: &str) {
        self.print(&[&"Warning--string name \""]);
        self.print_token();
        self.print(&[&"\" is "]);
        self.bib_warn_newline(&[&what]);
    }

    /// §235: `bib_identifier_scan_check`.
    fn bib_identifier_scan_check(&mut self, what: &str) -> D {
        if self.scan_result == WHITE_ADJACENT || self.scan_result == SPECIFIED_CHAR_ADJACENT {
            return Ok(());
        }
        if self.scan_result == ID_NULL {
            self.print(&[&"You're missing "]);
        } else if self.scan_result == OTHER_CHAR_ADJACENT {
            let c = self.scan_char();
            self.print(&[&"\"", &Ch(c), &"\" immediately follows "]);
        } else {
            return Err(self.confusion("Identifier scanning error").into());
        }
        Err(self.bib_err(&[&what]))
    }

    /// A left delimiter (`{` or `(`): sets `right_outer_delim`.
    fn outer_delim(&mut self) -> D {
        self.right_outer_delim = match self.scan_char() {
            b'{' => b'}',
            b'(' => b')',
            _ => return Err(self.bib_one_of_two(b'{', b'(')),
        };
        self.buf_ptr2 += 1;
        Ok(())
    }

    /// §236
    fn get_bib_command_or_entry_and_process(&mut self) -> D {
        self.at_bib_command = false;
        // §237: skip to the next database entry or .bib command
        while !self.scan1(b'@') {
            if !self.input_ln(Src::Bib) {
                return Ok(());
            }
            self.bib_line_num += 1;
            self.buf_ptr2 = 0;
        }
        // §238: scan the entry type or scan and process the .bib command
        if self.scan_char() != b'@' {
            return Err(self.confusion("An \"@\" disappeared").into());
        }
        self.buf_ptr2 += 1;
        self.eat_bib()?;
        self.scan_identifier(b'{', b'(', b'(');
        self.bib_identifier_scan_check("an entry type")?;
        self.lower_token();
        if let Some(loc) = self.find_token(BIB_COMMAND_ILK) {
            self.command_num = self.t.info(loc);
            return self.process_bib_command();
        }
        match self.find_token(BST_FN_ILK) {
            Some(loc) if self.t.fn_type(loc) == WIZ_DEFINED => {
                self.entry_type_loc = loc;
                self.type_exists = true;
            }
            _ => self.type_exists = false,
        }
        self.eat_bib()?;
        self.scan_database_key()?;
        self.eat_bib()?;
        // §274: scan the entry's list of fields
        while self.scan_char() != self.right_outer_delim {
            if self.scan_char() != b',' {
                let d = self.right_outer_delim;
                return Err(self.bib_one_of_two(b',', d));
            }
            self.buf_ptr2 += 1;
            self.eat_bib()?;
            if self.scan_char() == self.right_outer_delim {
                break;
            }
            // §275: get the next field name
            self.scan_identifier(b'=', b'=', b'=');
            self.bib_identifier_scan_check("a field name")?;
            self.store_field = false;
            if self.store_entry {
                self.lower_token();
                if let Some(loc) = self.find_token(BST_FN_ILK)
                    && self.t.fn_type(loc) == FIELD
                {
                    self.field_name_loc = loc;
                    self.store_field = true;
                }
            }
            self.eat_bib()?;
            if self.scan_char() != b'=' {
                return Err(self.bib_equals_sign_expected());
            }
            self.buf_ptr2 += 1;
            self.eat_bib()?;
            self.scan_and_store_the_field_value_and_eat_white()?;
        }
        self.buf_ptr2 += 1;
        Ok(())
    }

    /// §239
    fn process_bib_command(&mut self) -> D {
        self.at_bib_command = true;
        match self.command_num {
            N_BIB_COMMENT => Ok(()),
            N_BIB_PREAMBLE => {
                // §242
                self.eat_bib()?;
                self.outer_delim()?;
                self.eat_bib()?;
                self.store_field = true;
                self.scan_and_store_the_field_value_and_eat_white()?;
                if self.scan_char() != self.right_outer_delim {
                    let d = self.right_outer_delim;
                    return Err(self.bib_err(&[&"Missing \"", &Ch(d), &"\" in preamble command"]));
                }
                self.buf_ptr2 += 1;
                Ok(())
            }
            N_BIB_STRING => {
                // §243–246
                self.eat_bib()?;
                self.outer_delim()?;
                self.eat_bib()?;
                self.scan_identifier(b'=', b'=', b'=');
                self.bib_identifier_scan_check("a string name")?;
                self.lower_token();
                let (loc, _) = self.insert_token(MACRO_ILK);
                self.cur_macro_loc = loc;
                let own = self.t.text_id(loc);
                self.t.set_info(loc, own);
                self.eat_bib()?;
                if self.scan_char() != b'=' {
                    return Err(self.bib_equals_sign_expected());
                }
                self.buf_ptr2 += 1;
                self.eat_bib()?;
                self.store_field = true;
                self.scan_and_store_the_field_value_and_eat_white()?;
                if self.scan_char() != self.right_outer_delim {
                    let d = self.right_outer_delim;
                    return Err(self.bib_err(&[&"Missing \"", &Ch(d), &"\" in string command"]));
                }
                self.buf_ptr2 += 1;
                Ok(())
            }
            _ => Err(self.confusion("Unknown database-file command").into()),
        }
    }

    /// §249
    fn scan_and_store_the_field_value_and_eat_white(&mut self) -> D {
        self.field_vl.clear();
        self.scan_a_field_token_and_eat_white()?;
        while self.scan_char() == b'#' {
            self.buf_ptr2 += 1;
            self.eat_bib()?;
            self.scan_a_field_token_and_eat_white()?;
        }
        if self.store_field {
            self.store_the_field_value()?;
        }
        Ok(())
    }

    /// §250
    fn scan_a_field_token_and_eat_white(&mut self) -> D {
        match self.scan_char() {
            b'{' => {
                self.right_str_delim = b'}';
                self.scan_balanced_braces()?;
            }
            b'"' => {
                self.right_str_delim = b'"';
                self.scan_balanced_braces()?;
            }
            b'0'..=b'9' => {
                // §258: scan a number
                if !self.scan_nonneg_integer() {
                    return Err(self.confusion("A digit disappeared").into());
                }
                if self.store_field {
                    let t = self.token().to_vec();
                    self.field_vl.extend_from_slice(&t);
                }
            }
            _ => self.scan_a_macro_name()?,
        }
        self.eat_bib()
    }

    /// §252: `check_for_and_compress_bib_white_space`.
    fn compress_white(&mut self) -> D {
        if lex(self.scan_char()) == WHITE_SPACE || self.buf_ptr2 == self.last {
            // `compress_bib_white`
            self.field_vl.push(b' ');
            while !self.scan_white_space() {
                if !self.input_ln(Src::Bib) {
                    return Err(self.bib_err(&[&"Illegal end of database file"]));
                }
                self.bib_line_num += 1;
                self.buf_ptr2 = 0;
            }
        }
        Ok(())
    }

    /// §253
    fn scan_balanced_braces(&mut self) -> D {
        self.buf_ptr2 += 1;
        self.compress_white()?;
        let n = self.field_vl.len();
        if n > 1 && self.field_vl[n - 1] == b' ' && self.field_vl[n - 2] == b' ' {
            self.field_vl.pop();
        }
        let mut level = 0i64;
        let delim = self.right_str_delim;
        if self.store_field {
            // §256: a full brace-balanced scan
            while self.scan_char() != delim {
                match self.scan_char() {
                    b'{' => {
                        level += 1;
                        self.field_vl.push(b'{');
                        self.buf_ptr2 += 1;
                        self.compress_white()?;
                        // §257
                        loop {
                            let c = self.scan_char();
                            self.field_vl.push(c);
                            self.buf_ptr2 += 1;
                            self.compress_white()?;
                            if c == b'}' {
                                level -= 1;
                                if level == 0 {
                                    break;
                                }
                            } else if c == b'{' {
                                level += 1;
                            }
                        }
                    }
                    b'}' => return Err(self.bib_err(&[&"Unbalanced braces"])),
                    c => {
                        self.field_vl.push(c);
                        self.buf_ptr2 += 1;
                        self.compress_white()?;
                    }
                }
            }
        } else {
            // §254: a quick one
            while self.scan_char() != delim {
                match self.scan_char() {
                    b'{' => {
                        level += 1;
                        self.buf_ptr2 += 1;
                        self.eat_bib()?;
                        // §255
                        while level > 0 {
                            match self.scan_char() {
                                b'}' => {
                                    level -= 1;
                                    self.buf_ptr2 += 1;
                                    self.eat_bib()?;
                                }
                                b'{' => {
                                    level += 1;
                                    self.buf_ptr2 += 1;
                                    self.eat_bib()?;
                                }
                                _ => {
                                    self.buf_ptr2 += 1;
                                    if !self.scan2(b'}', b'{') {
                                        self.eat_bib()?;
                                    }
                                }
                            }
                        }
                    }
                    b'}' => return Err(self.bib_err(&[&"Unbalanced braces"])),
                    _ => {
                        self.buf_ptr2 += 1;
                        if !self.scan3(delim, b'{', b'}') {
                            self.eat_bib()?;
                        }
                    }
                }
            }
        }
        self.buf_ptr2 += 1;
        Ok(())
    }

    /// §259
    fn scan_a_macro_name(&mut self) -> D {
        let d = self.right_outer_delim;
        self.scan_identifier(b',', d, b'#');
        self.bib_identifier_scan_check("a field part")?;
        if !self.store_field {
            return Ok(());
        }
        self.lower_token();
        let found = self.find_token(MACRO_ILK);
        let mut store_token = true;
        if self.at_bib_command
            && self.command_num == N_BIB_STRING
            && found == Some(self.cur_macro_loc)
        {
            store_token = false;
            self.macro_name_warning("used in its own definition");
        }
        let Some(loc) = found else {
            self.macro_name_warning("undefined");
            return Ok(());
        };
        if store_token {
            // §260: copy the macro string, compressing white space
            let s = self.t.by_id(self.t.info(loc)).clone();
            let mut i = 0;
            if self.field_vl.is_empty() && !s.is_empty() && lex(s[0]) == WHITE_SPACE {
                self.field_vl.push(b' ');
                i = 1;
                while i < s.len() && lex(s[i]) == WHITE_SPACE {
                    i += 1;
                }
            }
            while i < s.len() {
                if lex(s[i]) != WHITE_SPACE {
                    self.field_vl.push(s[i]);
                } else if self.field_vl.last() != Some(&b' ') {
                    self.field_vl.push(b' ');
                }
                i += 1;
            }
        }
        Ok(())
    }

    /// §261: store the field value string.
    fn store_the_field_value(&mut self) -> D {
        if !self.at_bib_command && self.field_vl.last() == Some(&b' ') {
            self.field_vl.pop();
        }
        let field_start = usize::from(!self.at_bib_command && self.field_vl.first() == Some(&b' '));
        let value = core::mem::take(&mut self.field_vl);
        let (val_loc, _) = self.t.insert(TEXT_ILK, &value[field_start..]);
        self.t.set_fn_type(val_loc, STR_LITERAL);
        let text = self.t.text(val_loc).clone();
        if self.at_bib_command {
            // §262
            match self.command_num {
                N_BIB_PREAMBLE => self.s_preamble.push(text),
                N_BIB_STRING => {
                    let id = self.t.text_id(val_loc);
                    self.t.set_info(self.cur_macro_loc, id);
                }
                _ => return Err(self.confusion("Unknown database-file command").into()),
            }
        } else {
            // §263
            let field_ptr =
                self.entry_cite_ptr * self.num_fields + self.t.info(self.field_name_loc) as usize;
            if get_field(&self.field_info, field_ptr).is_some() {
                let key = self.cite_list[self.entry_cite_ptr].clone();
                let name = self.t.text(self.field_name_loc).clone();
                self.print(&[&"Warning--I'm ignoring ", &key, &"'s extra \"", &name]);
                self.bib_warn_newline(&[&"\" field"]);
            } else {
                set(&mut self.field_info, field_ptr, Some(text));
                if self.t.info(self.field_name_loc) == self.crossref_num as i64 && !self.all_entries
                {
                    // §264: add or update a cross reference on cite_list
                    let key = &value[field_start..];
                    let lc = key.to_ascii_lowercase();
                    let (lc_cite_loc, found) = self.t.insert(LC_CITE_ILK, &lc);
                    if found {
                        let cite_loc = self.t.info(lc_cite_loc) as Loc;
                        let c = self.t.info(cite_loc) as usize;
                        if c >= self.old_num_cites {
                            self.cite_info[c] += 1;
                        }
                    } else {
                        let (cite_loc, found) = self.t.insert(CITE_ILK, key);
                        if found {
                            return Err(self.confusion("Cite hash error").into());
                        }
                        self.add_database_cite(cite_loc, lc_cite_loc);
                        let c = self.t.info(cite_loc) as usize;
                        self.cite_info[c] = 1;
                    }
                }
            }
        }
        self.field_vl = value;
        Ok(())
    }

    /// §265: put the cite key at `cite_loc` at `cite_ptr`.
    fn add_database_cite(&mut self, cite_loc: Loc, lc_cite_loc: Loc) {
        let new_cite = self.cite_ptr;
        if new_cite >= self.type_list.len() {
            let n = new_cite + 1;
            self.type_list.resize(n, EMPTY);
            self.cite_info.resize(n, 0);
            self.cite_info_str.resize(n, self.s_null.clone());
            self.entry_exists.resize(n, false);
        }
        let need = self.num_fields * (new_cite + 1);
        if self.field_info.len() < need {
            self.field_info.resize(need, None);
        }
        let s = self.t.text(cite_loc).clone();
        set(&mut self.cite_list, new_cite, s);
        self.t.set_info(cite_loc, new_cite as i64);
        self.t.set_info(lc_cite_loc, i64::from(cite_loc));
        self.cite_ptr += 1;
    }

    /// §266–273: scan the entry's database key and decide whether to
    /// store the entry.
    fn scan_database_key(&mut self) -> D {
        self.outer_delim()?;
        self.eat_bib()?;
        if self.right_outer_delim == b')' {
            self.scan1_white(b',');
        } else {
            self.scan2_white(b',', b'}');
        }
        // §267: check for a database key of interest
        let lc = self.token().to_ascii_lowercase();
        let (lc_cite_loc, mut found) = if self.all_entries {
            self.t.insert(LC_CITE_ILK, &lc)
        } else {
            self.t
                .find(LC_CITE_ILK, &lc)
                .map_or((0, false), |l| (l, true))
        };
        if found {
            let cite_loc = self.t.info(lc_cite_loc) as Loc;
            self.entry_cite_ptr = self.t.info(cite_loc) as usize;
            self.check_repeated_entry(lc_cite_loc)?;
            found = true;
        }
        self.store_entry = true;
        if self.all_entries {
            // §272: put this cite key in its place
            let place = if found {
                if self.entry_cite_ptr < self.all_marker {
                    None
                } else {
                    self.entry_exists[self.entry_cite_ptr] = true;
                    Some(self.t.info(lc_cite_loc) as Loc)
                }
            } else {
                let (cite_loc, found) = self.insert_token(CITE_ILK);
                if found {
                    return Err(self.confusion("Cite hash error").into());
                }
                Some(cite_loc)
            };
            if let Some(cite_loc) = place {
                self.entry_cite_ptr = self.cite_ptr;
                self.add_database_cite(cite_loc, lc_cite_loc);
            }
        } else if !found {
            self.store_entry = false;
        }
        if self.store_entry {
            // §273
            if self.type_exists {
                self.type_list[self.entry_cite_ptr] = self.entry_type_loc;
            } else {
                self.type_list[self.entry_cite_ptr] = UNDEFINED;
                self.print(&[&"Warning--entry type for \""]);
                self.print_token();
                self.bib_warn_newline(&[&"\" isn't style-file defined"]);
            }
        }
        Ok(())
    }

    /// §268: check for a duplicate or crossref-matching database key.
    fn check_repeated_entry(&mut self, lc_cite_loc: Loc) -> D {
        let e = self.entry_cite_ptr;
        if !self.all_entries || e < self.all_marker || e >= self.old_num_cites {
            if self.type_list[e] == EMPTY {
                // §269: make sure this entry's database key is on cite_list
                if !self.all_entries && e >= self.old_num_cites {
                    let (cite_loc, found) = self.insert_token(CITE_ILK);
                    if !found {
                        self.t.set_info(lc_cite_loc, i64::from(cite_loc));
                        self.t.set_info(cite_loc, e as i64);
                        self.cite_list[e] = self.t.text(cite_loc).clone();
                    }
                }
                return Ok(());
            }
        } else if !self.entry_exists[e] {
            // §270
            let lc = self.cite_info_str[e].to_ascii_lowercase();
            let Some(lc_xcite_loc) = self.t.find(LC_CITE_ILK, &lc) else {
                return Err(self.confusion("A cite key disappeared").into());
            };
            if lc_xcite_loc == lc_cite_loc {
                return Ok(());
            }
        }
        if self.type_list[e] == EMPTY {
            return Err(self.confusion("The cite list is messed up").into());
        }
        Err(self.bib_err(&[&"Repeated entry"]))
    }

    /// §278: the exact and lower-case locations of cite key `s`.
    fn find_cite_locs(&self, s: &[u8]) -> (Option<Loc>, Option<Loc>) {
        let lc = s.to_ascii_lowercase();
        (self.t.find(CITE_ILK, s), self.t.find(LC_CITE_ILK, &lc))
    }

    /// §276
    fn final_initialization_for_entries(&mut self) -> R {
        self.num_cites = self.cite_ptr;
        self.num_preamble_strings = self.s_preamble.len();
        let nf = self.num_fields;
        let npd = self.num_pre_defined_fields;
        let cr = self.crossref_num;
        let need = nf * self.num_cites;
        if self.field_info.len() < need {
            self.field_info.resize(need, None);
        }
        // §277: add cross-reference information
        for c in 0..self.num_cites {
            self.cite_ptr = c;
            let fp = c * nf + cr;
            if let Some(s) = self.field_info[fp].clone()
                && let (_, Some(lc)) = self.find_cite_locs(&s)
            {
                let cite_loc = self.t.info(lc) as Loc;
                self.field_info[fp] = Some(self.t.text(cite_loc).clone());
                let parent = self.t.info(cite_loc) as usize;
                for k in npd..nf {
                    if self.field_info[c * nf + k].is_none() {
                        self.field_info[c * nf + k] = get_field(&self.field_info, parent * nf + k);
                    }
                }
            }
        }
        // §279: subtract cross-reference information
        for c in 0..self.num_cites {
            self.cite_ptr = c;
            let fp = c * nf + cr;
            let Some(s) = self.field_info[fp].clone() else {
                continue;
            };
            match self.find_cite_locs(&s) {
                (exact, None) => {
                    if exact.is_some() {
                        return Err(self.confusion("Cite hash error"));
                    }
                    self.nonexistent_cross_reference_error(&s);
                    self.field_info[fp] = None;
                }
                (exact, Some(lc)) => {
                    let lc_target = self.t.info(lc) as Loc;
                    if exact != Some(lc_target) {
                        return Err(self.confusion("Cite hash error"));
                    }
                    let parent = self.t.info(lc_target) as usize;
                    if get(&self.type_list, parent) == EMPTY {
                        self.nonexistent_cross_reference_error(&s);
                        self.field_info[fp] = None;
                    } else {
                        if get_field(&self.field_info, parent * nf + cr).is_some() {
                            // §282
                            self.print(&[&"Warning--you've nested cross references"]);
                            let p = self.cite_list[parent].clone();
                            self.bad_cross_reference_print(&p);
                            self.print_ln(&[&"\", which also refers to something"]);
                            self.mark_warning();
                        }
                        if !self.all_entries
                            && parent >= self.old_num_cites
                            && self.cite_info[parent] < self.opts.min_crossrefs
                        {
                            self.field_info[fp] = None;
                        }
                    }
                }
            }
        }
        // §283: remove missing entries or those cross referenced too few times
        let mut cite_xptr = 0;
        for c in 0..self.num_cites {
            if self.type_list[c] == EMPTY {
                let s = self.cite_list[c].clone();
                self.print_missing_entry(&s);
            } else if self.all_entries
                || c < self.old_num_cites
                || self.cite_info[c] >= self.opts.min_crossrefs
            {
                if c > cite_xptr {
                    // §285: slide this cite key down to its permanent spot
                    self.cite_list[cite_xptr] = self.cite_list[c].clone();
                    self.type_list[cite_xptr] = self.type_list[c];
                    let s = self.cite_list[c].clone();
                    let (exact, lc) = self.find_cite_locs(&s);
                    let Some(lc) = lc else {
                        return Err(self.confusion("A cite key disappeared"));
                    };
                    let Some(exact) = exact.filter(|&e| i64::from(e) == self.t.info(lc)) else {
                        return Err(self.confusion("Cite hash error"));
                    };
                    self.t.set_info(exact, cite_xptr as i64);
                    for k in 0..nf {
                        let v = self.field_info[c * nf + k].clone();
                        self.field_info[cite_xptr * nf + k] = v;
                    }
                }
                cite_xptr += 1;
            }
        }
        self.num_cites = cite_xptr;
        if self.all_entries {
            // §286
            for c in self.all_marker..self.old_num_cites {
                if !self.entry_exists[c] {
                    let s = self.cite_info_str[c].clone();
                    self.print_missing_entry(&s);
                }
            }
        }
        // §287–289
        self.entry_ints = alloc::vec![0; self.num_ent_ints * self.num_cites];
        self.entry_strs = alloc::vec![Vec::new(); self.num_ent_strs * self.num_cites];
        self.sorted_cites = (0..self.num_cites).collect();
        Ok(())
    }

    /// §280
    fn bad_cross_reference_print(&mut self, s: &Str) {
        let cur = self.cite_list[self.cite_ptr].clone();
        self.print(&[&"--entry \"", &cur]);
        self.print_ln(&[&"\""]);
        self.print(&[&"refers to entry \"", s]);
    }

    /// §281
    fn nonexistent_cross_reference_error(&mut self, s: &Str) {
        self.print(&[&"A bad cross reference-"]);
        self.bad_cross_reference_print(s);
        self.print_ln(&[&"\", which doesn't exist"]);
        self.mark_error();
    }

    /// §284
    fn print_missing_entry(&mut self, s: &Str) {
        self.print(&[&"Warning--I didn't find a database entry for \"", s]);
        self.print_ln(&[&"\""]);
        self.mark_warning();
    }
}

fn get_field(v: &[Option<Str>], i: usize) -> Option<Str> {
    v.get(i).cloned().flatten()
}
