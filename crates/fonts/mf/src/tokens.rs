//! Parts 14 to 16 of `mf.web`: token lists, data structures for
//! variables, saving and restoring equivalents.

use crate::arith::halfp;
use crate::{Mf, Pointer, R, Scaled, Word, consts::*};

impl Mf {
    // §215
    pub(crate) fn new_num_tok(&mut self, v: Scaled) -> R<Pointer> {
        let p = self.get_node(TOKEN_NODE_SIZE)?;
        self.set_value(p, v);
        self.set_type(p, KNOWN);
        self.set_name_type(p, TOKEN);
        Ok(p)
    }

    // §216
    pub(crate) fn flush_token_list(&mut self, mut p: Pointer) -> R {
        while p != NULL {
            let q = p;
            p = self.link(p);
            if q >= self.hi_mem_min {
                self.free_avail(q);
            } else {
                match self.type_(q) {
                    VACUOUS | BOOLEAN_TYPE | KNOWN => {}
                    STRING_TYPE => self.delete_str_ref(self.value(q)),
                    UNKNOWN_BOOLEAN | UNKNOWN_STRING | UNKNOWN_PEN | UNKNOWN_PICTURE
                    | UNKNOWN_PATH | PEN_TYPE | PATH_TYPE | FUTURE_PEN | PICTURE_TYPE
                    | PAIR_TYPE | TRANSFORM_TYPE | DEPENDENT | PROTO_DEPENDENT | INDEPENDENT => {
                        self.g_pointer = q;
                        self.token_recycle()?;
                    }
                    _ => return self.confusion("token"),
                }
                self.free_node(q, TOKEN_NODE_SIZE);
            }
        }
        Ok(())
    }

    // §217
    pub(crate) fn show_token_list(&mut self, mut p: i32, q: i32, l: i32, null_tally: i32) -> R {
        let mut class = PERCENT_CLASS;
        self.tally = null_tally;
        while p != NULL && self.tally < l {
            if p == q {
                self.set_trick_count();
            }
            // §218
            let mut c = LETTER_CLASS;
            if p < MEM_MIN || p > self.mem_end {
                self.prints(" CLOBBERED");
                return Ok(());
            }
            if p < self.hi_mem_min {
                // §219
                if self.name_type(p) == TOKEN {
                    if self.type_(p) == KNOWN {
                        // §220
                        if class == DIGIT_CLASS {
                            self.print_char(b' ');
                        }
                        let v = self.value(p);
                        if v < 0 {
                            if class == LEFT_BRACKET_CLASS {
                                self.print_char(b' ');
                            }
                            self.print_char(b'[');
                            self.print_scaled(v);
                            self.print_char(b']');
                            c = RIGHT_BRACKET_CLASS;
                        } else {
                            self.print_scaled(v);
                            c = DIGIT_CLASS;
                        }
                    } else if self.type_(p) != STRING_TYPE {
                        self.prints(" BAD");
                    } else {
                        self.print_char(b'"');
                        self.slow_print(self.value(p));
                        self.print_char(b'"');
                        c = STRING_CLASS;
                    }
                } else if self.name_type(p) != CAPSULE
                    || self.type_(p) < VACUOUS
                    || self.type_(p) > INDEPENDENT
                {
                    self.prints(" BAD");
                } else {
                    self.g_pointer = p;
                    self.print_capsule()?;
                    c = RIGHT_PAREN_CLASS;
                }
            } else {
                let mut r = self.info(p);
                if r >= EXPR_BASE {
                    // §222
                    if r < SUFFIX_BASE {
                        self.prints("(EXPR");
                        r -= EXPR_BASE;
                    } else if r < TEXT_BASE {
                        self.prints("(SUFFIX");
                        r -= SUFFIX_BASE;
                    } else {
                        self.prints("(TEXT");
                        r -= TEXT_BASE;
                    }
                    self.print_int(r);
                    self.print_char(b')');
                    c = RIGHT_PAREN_CLASS;
                } else if r < 1 {
                    if r == 0 {
                        // §221
                        if class == LEFT_BRACKET_CLASS {
                            self.print_char(b' ');
                        }
                        self.prints("[]");
                        c = RIGHT_BRACKET_CLASS;
                    } else {
                        self.prints(" IMPOSSIBLE");
                    }
                } else {
                    r = self.text(r);
                    if r < 0 || r >= self.str_ptr {
                        self.prints(" NONEXISTENT");
                    } else {
                        // §223
                        c = self.char_class
                            [self.str_pool[self.str_start[r as usize] as usize] as usize];
                        if c == class {
                            match c {
                                LETTER_CLASS => self.print_char(b'.'),
                                5..=8 => {}
                                _ => self.print_char(b' '),
                            }
                        }
                        self.slow_print(r);
                    }
                }
            }
            class = c;
            p = self.link(p);
        }
        if p != NULL {
            self.prints(" ETC.");
        }
        Ok(())
    }

    // §224
    pub(crate) fn print_capsule(&mut self) -> R {
        self.print_char(b'(');
        self.print_exp(self.g_pointer, 0)?;
        self.print_char(b')');
        Ok(())
    }
    pub(crate) fn token_recycle(&mut self) -> R {
        self.recycle_value(self.g_pointer)
    }

    // §226
    pub(crate) fn delete_mac_ref(&mut self, p: Pointer) -> R {
        if self.ref_count(p) == NULL {
            self.flush_token_list(p)
        } else {
            let v = self.ref_count(p) - 1;
            self.set_ref_count(p, v);
            Ok(())
        }
    }

    // §227
    pub(crate) fn show_macro(&mut self, mut p: Pointer, q: i32, mut l: i32) -> R {
        p = self.link(p);
        while self.info(p) > TEXT_MACRO {
            let r = self.link(p);
            self.set_link(p, NULL);
            self.show_token_list(p, NULL, l, 0)?;
            self.set_link(p, r);
            p = r;
            if l > 0 {
                l -= self.tally;
            } else {
                return Ok(());
            }
        }
        self.tally = 0;
        match self.info(p) {
            GENERAL_MACRO => self.prints("->"),
            PRIMARY_MACRO | SECONDARY_MACRO | TERTIARY_MACRO => {
                self.print_char(b'<');
                self.print_cmd_mod(PARAM_TYPE, self.info(p))?;
                self.prints(">->");
            }
            EXPR_MACRO => self.prints("<expr>->"),
            OF_MACRO => self.prints("<expr>of<primary>->"),
            SUFFIX_MACRO => self.prints("<suffix>->"),
            TEXT_MACRO => self.prints("<text>->"),
            _ => {}
        }
        self.show_token_list(self.link(p), q, l - self.tally, 0)
    }

    // §232
    pub(crate) fn init_big_node(&mut self, p: Pointer) -> R {
        let mut s = self.big_node_size[(self.type_(p) - TRANSFORM_TYPE) as usize];
        let q = self.get_node(s)?;
        loop {
            s -= 2;
            self.new_indep(q + s)?;
            self.set_name_type(q + s, halfp(s) + X_PART_SECTOR);
            self.set_link(q + s, NULL);
            if s == 0 {
                break;
            }
        }
        self.set_link(q, p);
        self.set_value(p, q);
        Ok(())
    }

    // §233
    pub(crate) fn id_transform(&mut self) -> R<Pointer> {
        let p = self.get_node(VALUE_NODE_SIZE)?;
        self.set_type(p, TRANSFORM_TYPE);
        self.set_name_type(p, CAPSULE);
        self.set_value(p, NULL);
        self.init_big_node(p)?;
        let q = self.value(p);
        let mut r = q + TRANSFORM_NODE_SIZE;
        loop {
            r -= 2;
            self.set_type(r, KNOWN);
            self.set_value(r, 0);
            if r == q {
                break;
            }
        }
        self.set_value(q + 4, UNITY);
        self.set_value(q + 10, UNITY);
        Ok(p)
    }

    // §234
    pub(crate) fn new_root(&mut self, x: Pointer) -> R {
        let p = self.get_node(VALUE_NODE_SIZE)?;
        self.set_type(p, UNDEFINED);
        self.set_name_type(p, ROOT);
        self.set_link(p, x);
        self.set_equiv(x, p);
        Ok(())
    }

    // §235
    pub(crate) fn print_variable_name(&mut self, mut p: Pointer) -> R {
        while self.name_type(p) >= X_PART_SECTOR {
            // §237
            match self.name_type(p) {
                X_PART_SECTOR => self.print_char(b'x'),
                Y_PART_SECTOR => self.print_char(b'y'),
                XX_PART_SECTOR => self.prints("xx"),
                XY_PART_SECTOR => self.prints("xy"),
                YX_PART_SECTOR => self.prints("yx"),
                YY_PART_SECTOR => self.prints("yy"),
                CAPSULE => {
                    self.prints("%CAPSULE");
                    self.print_int(p - NULL);
                    return Ok(());
                }
                _ => {}
            }
            self.prints("part ");
            p = self.link(p - 2 * (self.name_type(p) - X_PART_SECTOR));
        }
        let mut q = NULL;
        while self.name_type(p) > SAVED_ROOT {
            // §236
            let r;
            if self.name_type(p) == SUBSCR {
                r = self.new_num_tok(self.subscript(p))?;
                loop {
                    p = self.link(p);
                    if self.name_type(p) == ATTR {
                        break;
                    }
                }
            } else if self.name_type(p) == STRUCTURED_ROOT {
                p = self.link(p);
                p = self.parent(p);
                continue;
            } else {
                if self.name_type(p) != ATTR {
                    return self.confusion("var");
                }
                r = self.get_avail()?;
                self.set_info(r, self.attr_loc(p));
            }
            self.set_link(r, q);
            q = r;
            p = self.parent(p);
        }
        let r = self.get_avail()?;
        self.set_info(r, self.link(p));
        self.set_link(r, q);
        if self.name_type(p) == SAVED_ROOT {
            self.prints("(SAVED)");
        }
        self.show_token_list(r, NULL, EL_GORDO, self.tally)?;
        self.flush_token_list(r)
    }

    // §238
    pub(crate) fn interesting(&self, p: Pointer) -> bool {
        if self.int(TRACING_CAPSULES) > 0 {
            true
        } else {
            let mut t = self.name_type(p);
            if t >= X_PART_SECTOR && t != CAPSULE {
                t = self.name_type(self.link(p - 2 * (t - X_PART_SECTOR)));
            }
            t != CAPSULE
        }
    }

    // §239
    pub(crate) fn new_structure(&mut self, p: Pointer) -> R<Pointer> {
        let r;
        match self.name_type(p) {
            ROOT => {
                let q = self.link(p);
                r = self.get_node(VALUE_NODE_SIZE)?;
                self.set_equiv(q, r);
            }
            SUBSCR => {
                // §240
                let mut q = p;
                loop {
                    q = self.link(q);
                    if self.name_type(q) == ATTR {
                        break;
                    }
                }
                q = self.parent(q);
                let mut rr = q + 1;
                loop {
                    q = rr;
                    rr = self.link(rr);
                    if rr == p {
                        break;
                    }
                }
                rr = self.get_node(SUBSCR_NODE_SIZE)?;
                self.set_link(q, rr);
                self.set_subscript(rr, self.subscript(p));
                r = rr;
            }
            ATTR => {
                // §241
                let mut q = self.parent(p);
                let mut rr = self.attr_head(q);
                loop {
                    q = rr;
                    rr = self.link(rr);
                    if rr == p {
                        break;
                    }
                }
                rr = self.get_node(ATTR_NODE_SIZE)?;
                self.set_link(q, rr);
                self.mem[(rr + 2) as usize] = self.mem[(p + 2) as usize];
                if self.attr_loc(p) == COLLECTIVE_SUBSCRIPT {
                    q = self.parent(p) + 1;
                    while self.link(q) != p {
                        q = self.link(q);
                    }
                    self.set_link(q, rr);
                }
                r = rr;
            }
            _ => return self.confusion("struct"),
        }
        self.set_link(r, self.link(p));
        self.set_type(r, STRUCTURED);
        self.set_name_type(r, self.name_type(p));
        self.set_attr_head(r, p);
        self.set_name_type(p, STRUCTURED_ROOT);
        let q = self.get_node(ATTR_NODE_SIZE)?;
        self.set_link(p, q);
        self.set_subscr_head(r, q);
        self.set_parent(q, r);
        self.set_type(q, UNDEFINED);
        self.set_name_type(q, ATTR);
        self.set_link(q, END_ATTR);
        self.set_attr_loc(q, COLLECTIVE_SUBSCRIPT);
        Ok(r)
    }

    // §242
    pub(crate) fn find_variable(&mut self, mut t: Pointer) -> R<Pointer> {
        let mut p = self.info(t);
        t = self.link(t);
        if self.eq_type(p) % OUTER_TAG != TAG_TOKEN {
            return Ok(NULL);
        }
        if self.equiv(p) == NULL {
            self.new_root(p)?;
        }
        p = self.equiv(p);
        let mut pp = p;
        while t != NULL {
            // §243
            if self.type_(pp) != STRUCTURED {
                if self.type_(pp) > STRUCTURED {
                    return Ok(NULL);
                }
                let ss = self.new_structure(pp)?;
                if p == pp {
                    p = ss;
                }
                pp = ss;
            }
            if self.type_(p) != STRUCTURED {
                p = self.new_structure(p)?;
            }
            if t < self.hi_mem_min {
                // §244
                let n = self.value(t);
                pp = self.link(self.attr_head(pp));
                let q = self.link(self.attr_head(p));
                let save_word: Word = self.mem[(q + 2) as usize];
                self.set_subscript(q, EL_GORDO);
                let mut s = p + 1;
                let mut r;
                loop {
                    r = s;
                    s = self.link(s);
                    if n <= self.subscript(s) {
                        break;
                    }
                }
                if n == self.subscript(s) {
                    p = s;
                } else {
                    p = self.get_node(SUBSCR_NODE_SIZE)?;
                    self.set_link(r, p);
                    self.set_link(p, s);
                    self.set_subscript(p, n);
                    self.set_name_type(p, SUBSCR);
                    self.set_type(p, UNDEFINED);
                }
                self.mem[(q + 2) as usize] = save_word;
            } else {
                // §245
                let n = self.info(t);
                let mut ss = self.attr_head(pp);
                let mut rr;
                loop {
                    rr = ss;
                    ss = self.link(ss);
                    if n <= self.attr_loc(ss) {
                        break;
                    }
                }
                if n < self.attr_loc(ss) {
                    let qq = self.get_node(ATTR_NODE_SIZE)?;
                    self.set_link(rr, qq);
                    self.set_link(qq, ss);
                    self.set_attr_loc(qq, n);
                    self.set_name_type(qq, ATTR);
                    self.set_type(qq, UNDEFINED);
                    self.set_parent(qq, pp);
                    ss = qq;
                }
                if p == pp {
                    p = ss;
                    pp = ss;
                } else {
                    pp = ss;
                    let mut s = self.attr_head(p);
                    let mut r;
                    loop {
                        r = s;
                        s = self.link(s);
                        if n <= self.attr_loc(s) {
                            break;
                        }
                    }
                    if n == self.attr_loc(s) {
                        p = s;
                    } else {
                        let q = self.get_node(ATTR_NODE_SIZE)?;
                        self.set_link(r, q);
                        self.set_link(q, s);
                        self.set_attr_loc(q, n);
                        self.set_name_type(q, ATTR);
                        self.set_type(q, UNDEFINED);
                        self.set_parent(q, p);
                        p = q;
                    }
                }
            }
            t = self.link(t);
        }
        if self.type_(pp) >= STRUCTURED {
            if self.type_(pp) == STRUCTURED {
                pp = self.attr_head(pp);
            } else {
                return Ok(NULL);
            }
        }
        if self.type_(p) == STRUCTURED {
            p = self.attr_head(p);
        }
        if self.type_(p) == UNDEFINED {
            if self.type_(pp) == UNDEFINED {
                self.set_type(pp, NUMERIC_TYPE);
                self.set_value(pp, NULL);
            }
            self.set_type(p, self.type_(pp));
            self.set_value(p, NULL);
        }
        Ok(p)
    }

    // §246
    pub(crate) fn flush_variable(
        &mut self,
        mut p: Pointer,
        mut t: Pointer,
        discard_suffixes: bool,
    ) -> R {
        while t != NULL {
            if self.type_(p) != STRUCTURED {
                return Ok(());
            }
            let n = self.info(t);
            t = self.link(t);
            if n == COLLECTIVE_SUBSCRIPT {
                let mut r = p + 1;
                let mut q = self.link(r);
                while self.name_type(q) == SUBSCR {
                    self.flush_variable(q, t, discard_suffixes)?;
                    if t == NULL {
                        if self.type_(q) == STRUCTURED {
                            r = q;
                        } else {
                            self.set_link(r, self.link(q));
                            self.free_node(q, SUBSCR_NODE_SIZE);
                        }
                    } else {
                        r = q;
                    }
                    q = self.link(r);
                }
            }
            p = self.attr_head(p);
            loop {
                p = self.link(p);
                if self.attr_loc(p) >= n {
                    break;
                }
            }
            if self.attr_loc(p) != n {
                return Ok(());
            }
        }
        if discard_suffixes {
            self.flush_below_variable(p)
        } else {
            if self.type_(p) == STRUCTURED {
                p = self.attr_head(p);
            }
            self.recycle_value(p)
        }
    }

    // §247
    pub(crate) fn flush_below_variable(&mut self, p: Pointer) -> R {
        if self.type_(p) != STRUCTURED {
            self.recycle_value(p)?;
        } else {
            let mut q = self.subscr_head(p);
            while self.name_type(q) == SUBSCR {
                self.flush_below_variable(q)?;
                let r = q;
                q = self.link(q);
                self.free_node(r, SUBSCR_NODE_SIZE);
            }
            let r = self.attr_head(p);
            q = self.link(r);
            self.recycle_value(r)?;
            if self.name_type(p) <= SAVED_ROOT {
                self.free_node(r, VALUE_NODE_SIZE);
            } else {
                self.free_node(r, SUBSCR_NODE_SIZE);
            }
            loop {
                self.flush_below_variable(q)?;
                let r = q;
                q = self.link(q);
                self.free_node(r, ATTR_NODE_SIZE);
                if q == END_ATTR {
                    break;
                }
            }
            self.set_type(p, UNDEFINED);
        }
        Ok(())
    }

    // §248
    pub(crate) fn und_type(&self, p: Pointer) -> i32 {
        match self.type_(p) {
            UNDEFINED | VACUOUS => UNDEFINED,
            BOOLEAN_TYPE | UNKNOWN_BOOLEAN => UNKNOWN_BOOLEAN,
            STRING_TYPE | UNKNOWN_STRING => UNKNOWN_STRING,
            PEN_TYPE | UNKNOWN_PEN | FUTURE_PEN => UNKNOWN_PEN,
            PATH_TYPE | UNKNOWN_PATH => UNKNOWN_PATH,
            PICTURE_TYPE | UNKNOWN_PICTURE => UNKNOWN_PICTURE,
            TRANSFORM_TYPE | PAIR_TYPE | NUMERIC_TYPE => self.type_(p),
            KNOWN | DEPENDENT | PROTO_DEPENDENT | INDEPENDENT => NUMERIC_TYPE,
            // (there are no other cases: web2c's switch leaves the result
            // unset)
            _ => 0,
        }
    }

    // §249
    pub(crate) fn clear_symbol(&mut self, p: Pointer, saving: bool) -> R {
        let q = self.equiv(p);
        match self.eq_type(p) % OUTER_TAG {
            DEFINED_MACRO
            | SECONDARY_PRIMARY_MACRO
            | TERTIARY_SECONDARY_MACRO
            | EXPRESSION_TERTIARY_MACRO => {
                if !saving {
                    self.delete_mac_ref(q)?;
                }
            }
            TAG_TOKEN => {
                if q != NULL {
                    if saving {
                        self.set_name_type(q, SAVED_ROOT);
                    } else {
                        self.flush_below_variable(q)?;
                        self.free_node(q, VALUE_NODE_SIZE);
                    }
                }
            }
            _ => {}
        }
        self.eqtb[p as usize] = self.eqtb[FROZEN_UNDEFINED as usize];
        Ok(())
    }

    // §250
    pub(crate) fn save_boundary_item(&mut self) -> R<Pointer> {
        let p = self.get_avail()?;
        self.set_info(p, 0);
        self.set_link(p, self.save_ptr);
        self.save_ptr = p;
        Ok(p)
    }

    // §252
    pub(crate) fn save_variable(&mut self, q: Pointer) -> R {
        if self.save_ptr != NULL {
            let p = self.get_node(SAVE_NODE_SIZE)?;
            self.set_info(p, q);
            self.set_link(p, self.save_ptr);
            self.mem[(p + 1) as usize] = self.eqtb[q as usize];
            self.save_ptr = p;
        }
        self.clear_symbol(q, self.save_ptr != NULL)
    }

    // §253
    pub(crate) fn save_internal(&mut self, q: i32) -> R {
        if self.save_ptr != NULL {
            let p = self.get_node(SAVE_NODE_SIZE)?;
            self.set_info(p, HASH_END + q);
            self.set_link(p, self.save_ptr);
            self.set_value(p, self.internal[q as usize]);
            self.save_ptr = p;
        }
        Ok(())
    }

    // §254
    pub(crate) fn unsave(&mut self) -> R {
        while self.info(self.save_ptr) != 0 {
            let q = self.info(self.save_ptr);
            if q > HASH_END {
                if self.int(TRACING_RESTORES) > 0 {
                    self.begin_diagnostic();
                    self.print_nl("{restoring ");
                    self.slow_print(self.int_name[(q - HASH_END) as usize]);
                    self.print_char(b'=');
                    self.print_scaled(self.value(self.save_ptr));
                    self.print_char(b'}');
                    self.end_diagnostic(false);
                }
                self.internal[(q - HASH_END) as usize] = self.value(self.save_ptr);
            } else {
                if self.int(TRACING_RESTORES) > 0 {
                    self.begin_diagnostic();
                    self.print_nl("{restoring ");
                    self.slow_print(self.text(q));
                    self.print_char(b'}');
                    self.end_diagnostic(false);
                }
                self.clear_symbol(q, false)?;
                self.eqtb[q as usize] = self.mem[(self.save_ptr + 1) as usize];
                if self.eq_type(q) % OUTER_TAG == TAG_TOKEN {
                    let p = self.equiv(q);
                    if p != NULL {
                        self.set_name_type(p, ROOT);
                    }
                }
            }
            let p = self.link(self.save_ptr);
            self.free_node(self.save_ptr, SAVE_NODE_SIZE);
            self.save_ptr = p;
        }
        let p = self.link(self.save_ptr);
        self.free_avail(self.save_ptr);
        self.save_ptr = p;
        Ok(())
    }
}
