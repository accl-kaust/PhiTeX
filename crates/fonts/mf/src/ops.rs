//! Part 42 of `mf.web`: doing the operations.

use crate::arith::{ab_vs_cd, floor_scaled, half, odd, round_fraction, round_unscaled};
use crate::{Mf, Pointer, R, Scaled, consts::*};

const THREE_SIXTY_UNITS: i32 = 23_592_960;
const COEF_BOUND: i32 = 0o4525252525;
// §230
const X_PART_LOC: i32 = 0;
const Y_PART_LOC: i32 = 2;
const XX_PART_LOC: i32 = 4;
const XY_PART_LOC: i32 = 6;
const YX_PART_LOC: i32 = 8;
const YY_PART_LOC: i32 = 10;

impl Mf {
    fn boolean_reset(&mut self, b: bool) {
        self.cur_exp = if b { TRUE_CODE } else { FALSE_CODE };
    }
    #[inline]
    fn bns(&self, t: i32) -> i32 {
        self.big_node_size[(t - TRANSFORM_TYPE) as usize]
    }

    // §895
    pub(crate) fn do_nullary(&mut self, c: i32) -> R {
        self.check_arith()?;
        if self.int(TRACING_COMMANDS) > TWO {
            self.show_cmd_mod(NULLARY, c)?;
        }
        match c {
            TRUE_CODE | FALSE_CODE => {
                self.cur_type = BOOLEAN_TYPE;
                self.cur_exp = c;
            }
            NULL_PICTURE_CODE => {
                self.cur_type = PICTURE_TYPE;
                self.cur_exp = self.get_node(EDGE_HEADER_SIZE)?;
                self.init_edges(self.cur_exp);
            }
            NULL_PEN_CODE => {
                self.cur_type = PEN_TYPE;
                self.cur_exp = NULL_PEN;
            }
            NORMAL_DEVIATE => {
                self.cur_type = KNOWN;
                self.cur_exp = self.norm_rand()?;
            }
            PEN_CIRCLE => {
                // §896
                self.cur_type = FUTURE_PEN;
                self.cur_exp = self.get_node(KNOT_NODE_SIZE)?;
                let e = self.cur_exp;
                self.set_left_type(e, OPEN);
                self.set_right_type(e, OPEN);
                self.set_link(e, e);
                self.set_x_coord(e, 0);
                self.set_y_coord(e, 0);
                self.set_left_x(e, UNITY);
                self.set_left_y(e, 0);
                self.set_right_x(e, 0);
                self.set_right_y(e, UNITY);
            }
            JOB_NAME_OP => {
                if self.job_name == 0 {
                    self.open_log_file()?;
                }
                self.cur_type = STRING_TYPE;
                self.cur_exp = self.job_name;
            }
            READ_STRING_OP => {
                // §897
                if self.interaction <= NONSTOP_MODE {
                    return self.fatal_error("*** (cannot readstring in nonstop modes)");
                }
                self.begin_file_reading()?;
                self.set_name(1);
                self.prompt_input("")?;
                self.str_room(self.last - self.start())?;
                for k in self.start()..self.last {
                    self.append_char(self.buffer[k as usize]);
                }
                self.end_file_reading()?;
                self.cur_type = STRING_TYPE;
                self.cur_exp = self.make_string()?;
            }
            _ => {}
        }
        self.check_arith()
    }

    // §898
    pub(crate) fn do_unary(&mut self, c: i32) -> R {
        self.check_arith()?;
        if self.int(TRACING_COMMANDS) > TWO {
            // §902
            self.begin_diagnostic();
            self.print_nl("{");
            self.print_op(c);
            self.print_char(b'(');
            self.print_exp(NULL, 0)?;
            self.prints(")}");
            self.end_diagnostic(false);
        }
        match c {
            PLUS => {
                if self.cur_type < PAIR_TYPE && self.cur_type != PICTURE_TYPE {
                    self.bad_unary(PLUS)?;
                }
            }
            MINUS => {
                // §903
                match self.cur_type {
                    PAIR_TYPE | INDEPENDENT => {
                        let q = self.cur_exp;
                        self.make_exp_copy(q)?;
                        if self.cur_type == DEPENDENT {
                            self.negate_dep_list(self.dep_list(self.cur_exp));
                        } else if self.cur_type == PAIR_TYPE {
                            let p = self.value(self.cur_exp);
                            for l in [p + X_PART_LOC, p + Y_PART_LOC] {
                                if self.type_(l) == KNOWN {
                                    self.set_value(l, -self.value(l));
                                } else {
                                    self.negate_dep_list(self.dep_list(l));
                                }
                            }
                        }
                        self.recycle_value(q)?;
                        self.free_node(q, VALUE_NODE_SIZE);
                    }
                    DEPENDENT | PROTO_DEPENDENT => {
                        self.negate_dep_list(self.dep_list(self.cur_exp))
                    }
                    KNOWN => self.cur_exp = -self.cur_exp,
                    PICTURE_TYPE => self.negate_edges(self.cur_exp),
                    _ => self.bad_unary(MINUS)?,
                }
            }
            NOT_OP => {
                // §905
                if self.cur_type != BOOLEAN_TYPE {
                    self.bad_unary(NOT_OP)?;
                } else {
                    self.cur_exp = TRUE_CODE + FALSE_CODE - self.cur_exp;
                }
            }
            SQRT_OP | M_EXP_OP | M_LOG_OP | SIN_D_OP | COS_D_OP | FLOOR_OP | UNIFORM_DEVIATE
            | ODD_OP | CHAR_EXISTS_OP => {
                // §906
                if self.cur_type != KNOWN {
                    self.bad_unary(c)?;
                } else {
                    match c {
                        SQRT_OP => self.cur_exp = self.square_rt(self.cur_exp)?,
                        M_EXP_OP => self.cur_exp = self.m_exp(self.cur_exp),
                        M_LOG_OP => self.cur_exp = self.m_log(self.cur_exp)?,
                        SIN_D_OP | COS_D_OP => {
                            self.n_sin_cos((self.cur_exp % THREE_SIXTY_UNITS) * 16);
                            if c == SIN_D_OP {
                                self.cur_exp = round_fraction(self.n_sin);
                            } else {
                                self.cur_exp = round_fraction(self.n_cos);
                            }
                        }
                        FLOOR_OP => self.cur_exp = floor_scaled(self.cur_exp),
                        UNIFORM_DEVIATE => self.cur_exp = self.unif_rand(self.cur_exp),
                        ODD_OP => {
                            self.boolean_reset(odd(round_unscaled(self.cur_exp)));
                            self.cur_type = BOOLEAN_TYPE;
                        }
                        _ => {
                            // §1181
                            self.cur_exp = round_unscaled(self.cur_exp) % 256;
                            if self.cur_exp < 0 {
                                self.cur_exp += 256;
                            }
                            self.boolean_reset(self.char_exists[self.cur_exp as usize]);
                            self.cur_type = BOOLEAN_TYPE;
                        }
                    }
                }
            }
            ANGLE_OP => {
                // §907
                if self.nice_pair(self.cur_exp, self.cur_type) {
                    let p = self.value(self.cur_exp);
                    let x = self.n_arg(self.value(p + X_PART_LOC), self.value(p + Y_PART_LOC))?;
                    if x >= 0 {
                        self.flush_cur_exp((x + 8) / 16)?;
                    } else {
                        self.flush_cur_exp(-((-x + 8) / 16))?;
                    }
                } else {
                    self.bad_unary(ANGLE_OP)?;
                }
            }
            X_PART | Y_PART => {
                // §909
                if self.cur_type <= PAIR_TYPE && self.cur_type >= TRANSFORM_TYPE {
                    self.take_part(c)?;
                } else {
                    self.bad_unary(c)?;
                }
            }
            XX_PART | XY_PART | YX_PART | YY_PART => {
                if self.cur_type == TRANSFORM_TYPE {
                    self.take_part(c)?;
                } else {
                    self.bad_unary(c)?;
                }
            }
            CHAR_OP => {
                // §912
                if self.cur_type != KNOWN {
                    self.bad_unary(CHAR_OP)?;
                } else {
                    self.cur_exp = round_unscaled(self.cur_exp) % 256;
                    self.cur_type = STRING_TYPE;
                    if self.cur_exp < 0 {
                        self.cur_exp += 256;
                    }
                    if self.length(self.cur_exp) != 1 {
                        self.str_room(1)?;
                        self.append_char(self.cur_exp as u8);
                        self.cur_exp = self.make_string()?;
                    }
                }
            }
            DECIMAL => {
                if self.cur_type != KNOWN {
                    self.bad_unary(DECIMAL)?;
                } else {
                    self.old_setting = self.selector;
                    self.selector = NEW_STRING;
                    self.print_scaled(self.cur_exp);
                    self.cur_exp = self.make_string()?;
                    self.selector = self.old_setting;
                    self.cur_type = STRING_TYPE;
                }
            }
            OCT_OP | HEX_OP | ASCII_OP => {
                if self.cur_type != STRING_TYPE {
                    self.bad_unary(c)?;
                } else {
                    self.str_to_num(c)?;
                }
            }
            LENGTH_OP => {
                // §915
                if self.cur_type == STRING_TYPE {
                    self.flush_cur_exp(self.length(self.cur_exp) * UNITY)?;
                } else if self.cur_type == PATH_TYPE {
                    let l = self.path_length();
                    self.flush_cur_exp(l)?;
                } else if self.cur_type == KNOWN {
                    self.cur_exp = self.cur_exp.abs();
                } else if self.nice_pair(self.cur_exp, self.cur_type) {
                    let v = self.value(self.cur_exp);
                    let l = self.pyth_add(self.value(v + X_PART_LOC), self.value(v + Y_PART_LOC));
                    self.flush_cur_exp(l)?;
                } else {
                    self.bad_unary(c)?;
                }
            }
            TURNING_OP => {
                // §917
                if self.cur_type == PAIR_TYPE {
                    self.flush_cur_exp(0)?;
                } else if self.cur_type != PATH_TYPE {
                    self.bad_unary(TURNING_OP)?;
                } else if self.left_type(self.cur_exp) == ENDPOINT {
                    self.flush_cur_exp(0)?;
                } else {
                    self.cur_pen = NULL_PEN;
                    self.cur_path_type = CONTOUR_CODE;
                    self.cur_exp =
                        self.make_spec(self.cur_exp, FRACTION_ONE - HALF_UNIT - 1 - EL_GORDO, 0)?;
                    self.flush_cur_exp(self.turning_number * UNITY)?;
                }
            }
            BOOLEAN_TYPE => self.type_range(BOOLEAN_TYPE, UNKNOWN_BOOLEAN)?,
            STRING_TYPE => self.type_range(STRING_TYPE, UNKNOWN_STRING)?,
            PEN_TYPE => self.type_range(PEN_TYPE, FUTURE_PEN)?,
            PATH_TYPE => self.type_range(PATH_TYPE, UNKNOWN_PATH)?,
            PICTURE_TYPE => self.type_range(PICTURE_TYPE, UNKNOWN_PICTURE)?,
            TRANSFORM_TYPE | PAIR_TYPE => self.type_range(c, c)?,
            NUMERIC_TYPE => self.type_range(KNOWN, INDEPENDENT)?,
            KNOWN_OP | UNKNOWN_OP => self.test_known(c)?,
            CYCLE_OP => {
                // §920
                if self.cur_type != PATH_TYPE {
                    self.flush_cur_exp(FALSE_CODE)?;
                } else if self.left_type(self.cur_exp) != ENDPOINT {
                    self.flush_cur_exp(TRUE_CODE)?;
                } else {
                    self.flush_cur_exp(FALSE_CODE)?;
                }
                self.cur_type = BOOLEAN_TYPE;
            }
            MAKE_PEN_OP => {
                // §921
                if self.cur_type == PAIR_TYPE {
                    self.pair_to_path()?;
                }
                if self.cur_type == PATH_TYPE {
                    self.cur_type = FUTURE_PEN;
                } else {
                    self.bad_unary(MAKE_PEN_OP)?;
                }
            }
            MAKE_PATH_OP => {
                if self.cur_type == FUTURE_PEN {
                    self.materialize_pen()?;
                }
                if self.cur_type != PEN_TYPE {
                    self.bad_unary(MAKE_PATH_OP)?;
                } else {
                    let p = self.make_path(self.cur_exp)?;
                    self.flush_cur_exp(p)?;
                    self.cur_type = PATH_TYPE;
                }
            }
            TOTAL_WEIGHT_OP => {
                if self.cur_type != PICTURE_TYPE {
                    self.bad_unary(TOTAL_WEIGHT_OP)?;
                } else {
                    let w = self.total_weight(self.cur_exp);
                    self.flush_cur_exp(w)?;
                }
            }
            REVERSE => {
                if self.cur_type == PATH_TYPE {
                    let mut p = self.htap_ypoc(self.cur_exp)?;
                    if self.right_type(p) == ENDPOINT {
                        p = self.link(p);
                    }
                    self.toss_knot_list(self.cur_exp);
                    self.cur_exp = p;
                } else if self.cur_type == PAIR_TYPE {
                    self.pair_to_path()?;
                } else {
                    self.bad_unary(REVERSE)?;
                }
            }
            _ => {}
        }
        self.check_arith()
    }
    // §918
    fn type_range(&mut self, lo: i32, hi: i32) -> R {
        if self.cur_type >= lo && self.cur_type <= hi {
            self.flush_cur_exp(TRUE_CODE)?;
        } else {
            self.flush_cur_exp(FALSE_CODE)?;
        }
        self.cur_type = BOOLEAN_TYPE;
        Ok(())
    }

    // §899
    pub(crate) fn nice_pair(&self, p: i32, t: i32) -> bool {
        if t == PAIR_TYPE {
            let p = self.value(p);
            if self.type_(p + X_PART_LOC) == KNOWN && self.type_(p + Y_PART_LOC) == KNOWN {
                return true;
            }
        }
        false
    }

    // §900
    pub(crate) fn print_known_or_unknown_type(&mut self, t: i32, v: i32) {
        self.print_char(b'(');
        if t < DEPENDENT {
            if t != PAIR_TYPE {
                self.print_type(t);
            } else if self.nice_pair(v, PAIR_TYPE) {
                self.prints("pair");
            } else {
                self.prints("unknown pair");
            }
        } else {
            self.prints("unknown numeric");
        }
        self.print_char(b')');
    }

    // §901
    pub(crate) fn bad_unary(&mut self, c: i32) -> R {
        self.exp_err("Not implemented: ")?;
        self.print_op(c);
        self.print_known_or_unknown_type(self.cur_type, self.cur_exp);
        self.help(&[
            "I'm afraid I don't know how to apply that operation to that",
            "particular type. Continue, and I'll simply return the",
            "argument (shown above) as the result of the operation.",
        ]);
        self.put_get_error()
    }

    // §904
    pub(crate) fn negate_dep_list(&mut self, mut p: Pointer) {
        loop {
            self.set_value(p, -self.value(p));
            if self.info(p) == NULL {
                return;
            }
            p = self.link(p);
        }
    }

    // §908
    pub(crate) fn pair_to_path(&mut self) -> R {
        self.cur_exp = self.new_knot()?;
        self.cur_type = PATH_TYPE;
        Ok(())
    }

    // §910
    pub(crate) fn take_part(&mut self, c: i32) -> R {
        let p = self.value(self.cur_exp);
        self.set_value(TEMP_VAL, p);
        self.set_type(TEMP_VAL, self.cur_type);
        self.set_link(p, TEMP_VAL);
        self.free_node(self.cur_exp, VALUE_NODE_SIZE);
        self.make_exp_copy(p + 2 * (c - X_PART))?;
        self.recycle_value(TEMP_VAL)
    }

    // §913
    pub(crate) fn str_to_num(&mut self, c: i32) -> R {
        let mut n: i32;
        if c == ASCII_OP {
            if self.length(self.cur_exp) == 0 {
                n = -1;
            } else {
                n = i32::from(self.str_pool[self.str_start[self.cur_exp as usize] as usize]);
            }
        } else {
            let b = if c == OCT_OP { 8 } else { 16 };
            n = 0;
            let mut bad_char = false;
            for k in
                self.str_start[self.cur_exp as usize]..self.str_start[(self.cur_exp + 1) as usize]
            {
                let mut m = i32::from(self.str_pool[k as usize]);
                if (i32::from(b'0')..=i32::from(b'9')).contains(&m) {
                    m -= i32::from(b'0');
                } else if (i32::from(b'A')..=i32::from(b'F')).contains(&m) {
                    m = m - i32::from(b'A') + 10;
                } else if (i32::from(b'a')..=i32::from(b'f')).contains(&m) {
                    m = m - i32::from(b'a') + 10;
                } else {
                    bad_char = true;
                    m = 0;
                }
                if m >= b {
                    bad_char = true;
                    m = 0;
                }
                if n < 32768 / b {
                    n = n * b + m;
                } else {
                    n = 32767;
                }
            }
            // §914
            if bad_char {
                self.exp_err("String contains illegal digits")?;
                if c == OCT_OP {
                    self.help(&["I zeroed out characters that weren't in the range 0..7."]);
                } else {
                    self.help(&["I zeroed out characters that weren't hex digits."]);
                }
                self.put_get_error()?;
            }
            if n > 4095 {
                self.print_err("Number too large (");
                self.print_int(n);
                self.print_char(b')');
                self.help(&["I have trouble with numbers greater than 4095; watch out."]);
                self.put_get_error()?;
            }
        }
        self.flush_cur_exp(n * UNITY)
    }

    // §916
    pub(crate) fn path_length(&self) -> Scaled {
        let mut p = self.cur_exp;
        let mut n = if self.left_type(p) == ENDPOINT {
            -UNITY
        } else {
            0
        };
        loop {
            p = self.link(p);
            n += UNITY;
            if p == self.cur_exp {
                break;
            }
        }
        n
    }

    // §919
    pub(crate) fn test_known(&mut self, c: i32) -> R {
        let mut b = FALSE_CODE;
        match self.cur_type {
            VACUOUS | BOOLEAN_TYPE | STRING_TYPE | PEN_TYPE | FUTURE_PEN | PATH_TYPE
            | PICTURE_TYPE | KNOWN => b = TRUE_CODE,
            TRANSFORM_TYPE | PAIR_TYPE => {
                let p = self.value(self.cur_exp);
                let mut q = p + self.bns(self.cur_type);
                'done: {
                    loop {
                        q -= 2;
                        if self.type_(q) != KNOWN {
                            break 'done;
                        }
                        if q == p {
                            break;
                        }
                    }
                    b = TRUE_CODE;
                }
            }
            _ => {}
        }
        if c == KNOWN_OP {
            self.flush_cur_exp(b)?;
        } else {
            self.flush_cur_exp(TRUE_CODE + FALSE_CODE - b)?;
        }
        self.cur_type = BOOLEAN_TYPE;
        Ok(())
    }

    // §922
    pub(crate) fn do_binary(&mut self, mut p: Pointer, c: i32) -> R {
        self.check_arith()?;
        if self.int(TRACING_COMMANDS) > TWO {
            // §924
            self.begin_diagnostic();
            self.print_nl("{(");
            self.print_exp(p, 0)?;
            self.print_char(b')');
            self.print_op(c);
            self.print_char(b'(');
            self.print_exp(NULL, 0)?;
            self.prints(")}");
            self.end_diagnostic(false);
        }
        // §926
        let mut old_p = match self.type_(p) {
            TRANSFORM_TYPE | PAIR_TYPE => self.tarnished(p),
            INDEPENDENT => VOID,
            _ => NULL,
        };
        if old_p != NULL {
            let q = self.stash_cur_exp()?;
            old_p = p;
            self.make_exp_copy(old_p)?;
            p = self.stash_cur_exp()?;
            self.unstash_cur_exp(q)?;
        }
        // §927
        let mut old_exp = match self.cur_type {
            TRANSFORM_TYPE | PAIR_TYPE => self.tarnished(self.cur_exp),
            INDEPENDENT => VOID,
            _ => NULL,
        };
        if old_exp != NULL {
            old_exp = self.cur_exp;
            self.make_exp_copy(old_exp)?;
        }
        if !self.binary_case(p, c)? {
            self.recycle_value(p)?;
            self.free_node(p, VALUE_NODE_SIZE);
        }
        // exit:
        self.check_arith()?;
        // §925
        if old_p != NULL {
            self.recycle_value(old_p)?;
            self.free_node(old_p, VALUE_NODE_SIZE);
        }
        if old_exp != NULL {
            self.recycle_value(old_exp)?;
            self.free_node(old_exp, VALUE_NODE_SIZE);
        }
        Ok(())
    }

    /// The cases of `do_binary`; true where `mf.web` says `return` (so
    /// that `p` is not recycled).
    fn binary_case(&mut self, p: Pointer, c: i32) -> R<bool> {
        match c {
            PLUS | MINUS => {
                // §929
                if self.cur_type < PAIR_TYPE || self.type_(p) < PAIR_TYPE {
                    if self.cur_type == PICTURE_TYPE && self.type_(p) == PICTURE_TYPE {
                        if c == MINUS {
                            self.negate_edges(self.cur_exp);
                        }
                        self.cur_edges = self.cur_exp;
                        self.merge_edges(self.value(p))?;
                    } else {
                        self.bad_binary(p, c)?;
                    }
                } else if self.cur_type == PAIR_TYPE {
                    if self.type_(p) != PAIR_TYPE {
                        self.bad_binary(p, c)?;
                    } else {
                        let q = self.value(p);
                        let r = self.value(self.cur_exp);
                        self.add_or_subtract(q + X_PART_LOC, r + X_PART_LOC, c)?;
                        self.add_or_subtract(q + Y_PART_LOC, r + Y_PART_LOC, c)?;
                    }
                } else if self.type_(p) == PAIR_TYPE {
                    self.bad_binary(p, c)?;
                } else {
                    self.add_or_subtract(p, NULL, c)?;
                }
            }
            LESS_THAN | LESS_OR_EQUAL | GREATER_THAN | GREATER_OR_EQUAL | EQUAL_TO | UNEQUAL_TO => {
                // §936
                'done: {
                    if self.cur_type > PAIR_TYPE && self.type_(p) > PAIR_TYPE {
                        self.add_or_subtract(p, NULL, MINUS)?;
                    } else if self.cur_type != self.type_(p) {
                        self.bad_binary(p, c)?;
                        break 'done;
                    } else if self.cur_type == STRING_TYPE {
                        let v = self.str_vs_str(self.value(p), self.cur_exp);
                        self.flush_cur_exp(v)?;
                    } else if self.cur_type == UNKNOWN_STRING || self.cur_type == UNKNOWN_BOOLEAN {
                        // §938
                        let mut q = self.value(self.cur_exp);
                        while q != self.cur_exp && q != p {
                            q = self.value(q);
                        }
                        if q == p {
                            self.flush_cur_exp(0)?;
                        }
                    } else if self.cur_type == PAIR_TYPE || self.cur_type == TRANSFORM_TYPE {
                        // §939
                        let mut q = self.value(p);
                        let mut r = self.value(self.cur_exp);
                        let rr = r + self.bns(self.cur_type) - 2;
                        loop {
                            self.add_or_subtract(q, r, MINUS)?;
                            if self.type_(r) != KNOWN || self.value(r) != 0 || r == rr {
                                break;
                            }
                            q += 2;
                            r += 2;
                        }
                        let part = X_PART + half(r - self.value(self.cur_exp));
                        self.take_part(part)?;
                    } else if self.cur_type == BOOLEAN_TYPE {
                        self.flush_cur_exp(self.cur_exp - self.value(p))?;
                    } else {
                        self.bad_binary(p, c)?;
                        break 'done;
                    }
                    // §937
                    if self.cur_type != KNOWN {
                        if self.cur_type < KNOWN {
                            self.disp_err(p, "")?;
                            self.help(&["The quantities shown above have not been equated."]);
                        } else {
                            self.help(&[
                                "Oh dear. I can't decide if the expression above is positive,",
                                "negative, or zero. So this comparison test won't be `true'.",
                            ]);
                        }
                        self.exp_err("Unknown relation will be considered false")?;
                        self.put_get_flush_error(FALSE_CODE)?;
                    } else {
                        let e = self.cur_exp;
                        match c {
                            LESS_THAN => self.boolean_reset(e < 0),
                            LESS_OR_EQUAL => self.boolean_reset(e <= 0),
                            GREATER_THAN => self.boolean_reset(e > 0),
                            GREATER_OR_EQUAL => self.boolean_reset(e >= 0),
                            EQUAL_TO => self.boolean_reset(e == 0),
                            _ => self.boolean_reset(e != 0),
                        }
                    }
                    self.cur_type = BOOLEAN_TYPE;
                }
            }
            AND_OP | OR_OP => {
                // §940
                if self.type_(p) != BOOLEAN_TYPE || self.cur_type != BOOLEAN_TYPE {
                    self.bad_binary(p, c)?;
                } else if self.value(p) == c + FALSE_CODE - AND_OP {
                    self.cur_exp = self.value(p);
                }
            }
            TIMES => {
                // §941
                if self.cur_type < PAIR_TYPE || self.type_(p) < PAIR_TYPE {
                    self.bad_binary(p, TIMES)?;
                } else if self.cur_type == KNOWN || self.type_(p) == KNOWN {
                    // §942
                    let v;
                    if self.type_(p) == KNOWN {
                        v = self.value(p);
                        self.free_node(p, VALUE_NODE_SIZE);
                    } else {
                        v = self.cur_exp;
                        self.unstash_cur_exp(p)?;
                    }
                    if self.cur_type == KNOWN {
                        self.cur_exp = self.take_scaled(self.cur_exp, v);
                    } else if self.cur_type == PAIR_TYPE {
                        let p = self.value(self.cur_exp);
                        self.dep_mult(p + X_PART_LOC, v, true)?;
                        self.dep_mult(p + Y_PART_LOC, v, true)?;
                    } else {
                        self.dep_mult(NULL, v, true)?;
                    }
                    return Ok(true);
                } else if (self.nice_pair(p, self.type_(p)) && self.cur_type > PAIR_TYPE)
                    || (self.nice_pair(self.cur_exp, self.cur_type) && self.type_(p) > PAIR_TYPE)
                {
                    self.hard_times(p)?;
                    return Ok(true);
                } else {
                    self.bad_binary(p, TIMES)?;
                }
            }
            OVER => {
                // §948
                if self.cur_type != KNOWN || self.type_(p) < PAIR_TYPE {
                    self.bad_binary(p, OVER)?;
                } else {
                    let v = self.cur_exp;
                    self.unstash_cur_exp(p)?;
                    if v == 0 {
                        // §950
                        self.exp_err("Division by zero")?;
                        self.help(&[
                            "You're trying to divide the quantity shown above the error",
                            "message by zero. I'm going to divide it by one instead.",
                        ]);
                        self.put_get_error()?;
                    } else if self.cur_type == KNOWN {
                        self.cur_exp = self.make_scaled(self.cur_exp, v);
                    } else if self.cur_type == PAIR_TYPE {
                        let p = self.value(self.cur_exp);
                        self.dep_div(p + X_PART_LOC, v)?;
                        self.dep_div(p + Y_PART_LOC, v)?;
                    } else {
                        self.dep_div(NULL, v)?;
                    }
                    return Ok(true);
                }
            }
            PYTHAG_ADD | PYTHAG_SUB => {
                // §951
                if self.cur_type == KNOWN && self.type_(p) == KNOWN {
                    if c == PYTHAG_ADD {
                        self.cur_exp = self.pyth_add(self.value(p), self.cur_exp);
                    } else {
                        self.cur_exp = self.pyth_sub(self.value(p), self.cur_exp)?;
                    }
                } else {
                    self.bad_binary(p, c)?;
                }
            }
            ROTATED_BY | SLANTED_BY | SCALED_BY | SHIFTED_BY | TRANSFORMED_BY | X_SCALED
            | Y_SCALED | Z_SCALED => {
                // §952
                let t = self.type_(p);
                if t == PATH_TYPE || t == FUTURE_PEN || t == PEN_TYPE {
                    self.path_trans(p, c)?;
                    return Ok(true);
                } else if t == PAIR_TYPE || t == TRANSFORM_TYPE {
                    self.big_trans(p, c)?;
                } else if t == PICTURE_TYPE {
                    self.edges_trans(p, c)?;
                    return Ok(true);
                } else {
                    self.bad_binary(p, c)?;
                }
            }
            CONCATENATE => {
                // §975
                if self.cur_type == STRING_TYPE && self.type_(p) == STRING_TYPE {
                    self.cat(p)?;
                } else {
                    self.bad_binary(p, CONCATENATE)?;
                }
            }
            SUBSTRING_OF => {
                if self.nice_pair(p, self.type_(p)) && self.cur_type == STRING_TYPE {
                    self.chop_string(self.value(p))?;
                } else {
                    self.bad_binary(p, SUBSTRING_OF)?;
                }
            }
            SUBPATH_OF => {
                if self.cur_type == PAIR_TYPE {
                    self.pair_to_path()?;
                }
                if self.nice_pair(p, self.type_(p)) && self.cur_type == PATH_TYPE {
                    self.chop_path(self.value(p))?;
                } else {
                    self.bad_binary(p, SUBPATH_OF)?;
                }
            }
            POINT_OF | PRECONTROL_OF | POSTCONTROL_OF => {
                // §983
                if self.cur_type == PAIR_TYPE {
                    self.pair_to_path()?;
                }
                if self.cur_type == PATH_TYPE && self.type_(p) == KNOWN {
                    self.find_point(self.value(p), c)?;
                } else {
                    self.bad_binary(p, c)?;
                }
            }
            PEN_OFFSET_OF => {
                if self.cur_type == FUTURE_PEN {
                    self.materialize_pen()?;
                }
                if self.cur_type == PEN_TYPE && self.nice_pair(p, self.type_(p)) {
                    // §984
                    let v = self.value(p);
                    self.find_offset(
                        self.value(v + X_PART_LOC),
                        self.value(v + Y_PART_LOC),
                        self.cur_exp,
                    );
                    self.pair_value(self.cur_x, self.cur_y)?;
                } else {
                    self.bad_binary(p, PEN_OFFSET_OF)?;
                }
            }
            DIRECTION_TIME_OF => {
                if self.cur_type == PAIR_TYPE {
                    self.pair_to_path()?;
                }
                if self.cur_type == PATH_TYPE && self.nice_pair(p, self.type_(p)) {
                    let v = self.value(p);
                    let t = self.find_direction_time(
                        self.value(v + X_PART_LOC),
                        self.value(v + Y_PART_LOC),
                        self.cur_exp,
                    )?;
                    self.flush_cur_exp(t)?;
                } else {
                    self.bad_binary(p, DIRECTION_TIME_OF)?;
                }
            }
            INTERSECT => {
                // §988
                let mut p = p;
                if self.type_(p) == PAIR_TYPE {
                    let q = self.stash_cur_exp()?;
                    self.unstash_cur_exp(p)?;
                    self.pair_to_path()?;
                    p = self.stash_cur_exp()?;
                    self.unstash_cur_exp(q)?;
                }
                if self.cur_type == PAIR_TYPE {
                    self.pair_to_path()?;
                }
                if self.cur_type == PATH_TYPE && self.type_(p) == PATH_TYPE {
                    self.path_intersection(self.value(p), self.cur_exp);
                    self.pair_value(self.cur_t, self.cur_tt)?;
                } else {
                    self.bad_binary(p, INTERSECT)?;
                }
                // (`p` may have changed: recycle it here)
                self.recycle_value(p)?;
                self.free_node(p, VALUE_NODE_SIZE);
                return Ok(true);
            }
            _ => {}
        }
        Ok(false)
    }

    // §923
    pub(crate) fn bad_binary(&mut self, p: Pointer, c: i32) -> R {
        self.disp_err(p, "")?;
        self.exp_err("Not implemented: ")?;
        if c >= MIN_OF {
            self.print_op(c);
        }
        self.print_known_or_unknown_type(self.type_(p), p);
        if c >= MIN_OF {
            self.prints("of");
        } else {
            self.print_op(c);
        }
        self.print_known_or_unknown_type(self.cur_type, self.cur_exp);
        self.help(&[
            "I'm afraid I don't know how to apply that operation to that",
            "combination of types. Continue, and I'll return the second",
            "argument (see above) as the result of the operation.",
        ]);
        self.put_get_error()
    }

    // §928
    pub(crate) fn tarnished(&self, p: Pointer) -> Pointer {
        let q = self.value(p);
        let mut r = q + self.bns(self.type_(p));
        loop {
            r -= 2;
            if self.type_(r) == INDEPENDENT {
                return VOID;
            }
            if r == q {
                break;
            }
        }
        NULL
    }

    // §930
    pub(crate) fn add_or_subtract(&mut self, p: Pointer, mut q: Pointer, c: i32) -> R {
        let mut t;
        let mut v;
        if q == NULL {
            t = self.cur_type;
            v = if t < DEPENDENT {
                self.cur_exp
            } else {
                self.dep_list(self.cur_exp)
            };
        } else {
            t = self.type_(q);
            v = if t < DEPENDENT {
                self.value(q)
            } else {
                self.dep_list(q)
            };
        }
        if t == KNOWN {
            if c == MINUS {
                v = -v;
            }
            if self.type_(p) == KNOWN {
                v = self.slow_add(self.value(p), v);
                if q == NULL {
                    self.cur_exp = v;
                } else {
                    self.set_value(q, v);
                }
                return Ok(());
            }
            // §931
            let mut r = self.dep_list(p);
            while self.info(r) != NULL {
                r = self.link(r);
            }
            let s = self.slow_add(self.value(r), v);
            self.set_value(r, s);
            if q == NULL {
                q = self.get_node(VALUE_NODE_SIZE)?;
                self.cur_exp = q;
                self.cur_type = self.type_(p);
                self.set_name_type(q, CAPSULE);
            }
            self.set_dep_list(q, self.dep_list(p));
            self.set_type(q, self.type_(p));
            self.set_prev_dep(q, self.prev_dep(p));
            let pd = self.prev_dep(p);
            self.set_link(pd, q);
            self.set_type(p, KNOWN);
        } else {
            if c == MINUS {
                self.negate_dep_list(v);
            }
            // §932
            if self.type_(p) == KNOWN {
                // §933
                while self.info(v) != NULL {
                    v = self.link(v);
                }
                let s = self.slow_add(self.value(p), self.value(v));
                self.set_value(v, s);
            } else {
                let s = self.type_(p);
                let r = self.dep_list(p);
                'done: {
                    if t == DEPENDENT {
                        if s == DEPENDENT && self.max_coef(r) + self.max_coef(v) < COEF_BOUND {
                            v = self.p_plus_q(v, r, DEPENDENT)?;
                            break 'done;
                        }
                        t = PROTO_DEPENDENT;
                        v = self.p_over_v(v, UNITY, DEPENDENT, PROTO_DEPENDENT);
                    }
                    if s == PROTO_DEPENDENT {
                        v = self.p_plus_q(v, r, PROTO_DEPENDENT)?;
                    } else {
                        v = self.p_plus_fq(v, UNITY, r, PROTO_DEPENDENT, DEPENDENT)?;
                    }
                }
                // §934
                if q != NULL {
                    self.dep_finish(v, q, t)?;
                } else {
                    self.cur_type = t;
                    self.dep_finish(v, NULL, t)?;
                }
            }
        }
        Ok(())
    }

    // §935
    pub(crate) fn dep_finish(&mut self, v: Pointer, q: Pointer, t: i32) -> R {
        let p = if q == NULL { self.cur_exp } else { q };
        self.set_dep_list(p, v);
        self.set_type(p, t);
        if self.info(v) == NULL {
            let vv = self.value(v);
            if q == NULL {
                self.flush_cur_exp(vv)?;
            } else {
                self.recycle_value(p)?;
                self.set_type(q, KNOWN);
                self.set_value(q, vv);
            }
        } else if q == NULL {
            self.cur_type = t;
        }
        if self.fix_needed {
            self.fix_dependencies()?;
        }
        Ok(())
    }

    // §943
    pub(crate) fn dep_mult(&mut self, p: Pointer, v: i32, v_is_scaled: bool) -> R {
        let mut q;
        if p == NULL {
            q = self.cur_exp;
        } else if self.type_(p) != KNOWN {
            q = p;
        } else {
            let w = if v_is_scaled {
                self.take_scaled(self.value(p), v)
            } else {
                self.take_fraction(self.value(p), v)
            };
            self.set_value(p, w);
            return Ok(());
        }
        let mut t = self.type_(q);
        q = self.dep_list(q);
        let s = t;
        if t == DEPENDENT
            && v_is_scaled
            && ab_vs_cd(self.max_coef(q), v.abs(), COEF_BOUND - 1, UNITY) >= 0
        {
            t = PROTO_DEPENDENT;
        }
        q = self.p_times_v(q, v, s, t, v_is_scaled);
        self.dep_finish(q, p, t)
    }

    // §944
    pub(crate) fn frac_mult(&mut self, n: Scaled, d: Scaled) -> R {
        if self.int(TRACING_COMMANDS) > TWO {
            // §945
            self.begin_diagnostic();
            self.print_nl("{(");
            self.print_scaled(n);
            self.print_char(b'/');
            self.print_scaled(d);
            self.prints(")*(");
            self.print_exp(NULL, 0)?;
            self.prints(")}");
            self.end_diagnostic(false);
        }
        let mut old_exp = match self.cur_type {
            TRANSFORM_TYPE | PAIR_TYPE => self.tarnished(self.cur_exp),
            INDEPENDENT => VOID,
            _ => NULL,
        };
        if old_exp != NULL {
            old_exp = self.cur_exp;
            self.make_exp_copy(old_exp)?;
        }
        let v = self.make_fraction(n, d);
        if self.cur_type == KNOWN {
            self.cur_exp = self.take_fraction(self.cur_exp, v);
        } else if self.cur_type == PAIR_TYPE {
            let p = self.value(self.cur_exp);
            self.dep_mult(p + X_PART_LOC, v, false)?;
            self.dep_mult(p + Y_PART_LOC, v, false)?;
        } else {
            self.dep_mult(NULL, v, false)?;
        }
        if old_exp != NULL {
            self.recycle_value(old_exp)?;
            self.free_node(old_exp, VALUE_NODE_SIZE);
        }
        Ok(())
    }

    // §946
    pub(crate) fn hard_times(&mut self, mut p: Pointer) -> R {
        if self.type_(p) == PAIR_TYPE {
            let q = self.stash_cur_exp()?;
            self.unstash_cur_exp(p)?;
            p = q;
        }
        let r = self.value(self.cur_exp);
        let u = self.value(r + X_PART_LOC);
        let v = self.value(r + Y_PART_LOC);
        // §947
        self.set_type(r + Y_PART_LOC, self.type_(p));
        let d = self.copy_dep_list(self.dep_list(p))?;
        self.new_dep(r + Y_PART_LOC, d);
        self.set_type(r + X_PART_LOC, self.type_(p));
        self.mem[(r + X_PART_LOC + 1) as usize] = self.mem[(p + 1) as usize];
        let pd = self.prev_dep(p);
        self.set_link(pd, r + X_PART_LOC);
        self.free_node(p, VALUE_NODE_SIZE);
        self.dep_mult(r + X_PART_LOC, u, true)?;
        self.dep_mult(r + Y_PART_LOC, v, true)
    }

    // §949
    pub(crate) fn dep_div(&mut self, p: Pointer, v: Scaled) -> R {
        let mut q;
        if p == NULL {
            q = self.cur_exp;
        } else if self.type_(p) != KNOWN {
            q = p;
        } else {
            let w = self.make_scaled(self.value(p), v);
            self.set_value(p, w);
            return Ok(());
        }
        let mut t = self.type_(q);
        q = self.dep_list(q);
        let s = t;
        if t == DEPENDENT && ab_vs_cd(self.max_coef(q), UNITY, COEF_BOUND - 1, v.abs()) >= 0 {
            t = PROTO_DEPENDENT;
        }
        q = self.p_over_v(q, v, s, t);
        self.dep_finish(q, p, t)
    }

    // §953
    pub(crate) fn set_up_trans(&mut self, c: i32) -> R {
        if c != TRANSFORMED_BY || self.cur_type != TRANSFORM_TYPE {
            // §955
            let p = self.stash_cur_exp()?;
            self.cur_exp = self.id_transform()?;
            self.cur_type = TRANSFORM_TYPE;
            let q = self.value(self.cur_exp);
            'done: {
                // §956
                match c {
                    ROTATED_BY => {
                        if self.type_(p) == KNOWN {
                            // §957
                            self.n_sin_cos((self.value(p) % THREE_SIXTY_UNITS) * 16);
                            self.set_value(q + XX_PART_LOC, round_fraction(self.n_cos));
                            self.set_value(q + YX_PART_LOC, round_fraction(self.n_sin));
                            self.set_value(q + XY_PART_LOC, -self.value(q + YX_PART_LOC));
                            self.set_value(q + YY_PART_LOC, self.value(q + XX_PART_LOC));
                            break 'done;
                        }
                    }
                    SLANTED_BY => {
                        if self.type_(p) > PAIR_TYPE {
                            self.install(q + XY_PART_LOC, p)?;
                            break 'done;
                        }
                    }
                    SCALED_BY => {
                        if self.type_(p) > PAIR_TYPE {
                            self.install(q + XX_PART_LOC, p)?;
                            self.install(q + YY_PART_LOC, p)?;
                            break 'done;
                        }
                    }
                    SHIFTED_BY => {
                        if self.type_(p) == PAIR_TYPE {
                            let r = self.value(p);
                            self.install(q + X_PART_LOC, r + X_PART_LOC)?;
                            self.install(q + Y_PART_LOC, r + Y_PART_LOC)?;
                            break 'done;
                        }
                    }
                    X_SCALED => {
                        if self.type_(p) > PAIR_TYPE {
                            self.install(q + XX_PART_LOC, p)?;
                            break 'done;
                        }
                    }
                    Y_SCALED => {
                        if self.type_(p) > PAIR_TYPE {
                            self.install(q + YY_PART_LOC, p)?;
                            break 'done;
                        }
                    }
                    Z_SCALED => {
                        if self.type_(p) == PAIR_TYPE {
                            // §958
                            let r = self.value(p);
                            self.install(q + XX_PART_LOC, r + X_PART_LOC)?;
                            self.install(q + YY_PART_LOC, r + X_PART_LOC)?;
                            self.install(q + YX_PART_LOC, r + Y_PART_LOC)?;
                            if self.type_(r + Y_PART_LOC) == KNOWN {
                                self.set_value(r + Y_PART_LOC, -self.value(r + Y_PART_LOC));
                            } else {
                                self.negate_dep_list(self.dep_list(r + Y_PART_LOC));
                            }
                            self.install(q + XY_PART_LOC, r + Y_PART_LOC)?;
                            break 'done;
                        }
                    }
                    _ => {}
                }
                self.disp_err(p, "Improper transformation argument")?;
                self.help(&[
                    "The expression shown above has the wrong type,",
                    "so I can't transform anything using it.",
                    "Proceed, and I'll omit the transformation.",
                ]);
                self.put_get_error()?;
            }
            self.recycle_value(p)?;
            self.free_node(p, VALUE_NODE_SIZE);
        }
        // §954
        let q = self.value(self.cur_exp);
        let mut r = q + TRANSFORM_NODE_SIZE;
        loop {
            r -= 2;
            if self.type_(r) != KNOWN {
                return Ok(());
            }
            if r == q {
                break;
            }
        }
        self.txx = self.value(q + XX_PART_LOC);
        self.txy = self.value(q + XY_PART_LOC);
        self.tyx = self.value(q + YX_PART_LOC);
        self.tyy = self.value(q + YY_PART_LOC);
        self.tx = self.value(q + X_PART_LOC);
        self.ty = self.value(q + Y_PART_LOC);
        self.flush_cur_exp(0)
    }

    // §959
    pub(crate) fn set_up_known_trans(&mut self, c: i32) -> R {
        self.set_up_trans(c)?;
        if self.cur_type != KNOWN {
            self.exp_err("Transform components aren't all known")?;
            self.help(&[
                "I'm unable to apply a partially specified transformation",
                "except to a fully known pair or transform.",
                "Proceed, and I'll omit the transformation.",
            ]);
            self.put_get_flush_error(0)?;
            self.txx = UNITY;
            self.txy = 0;
            self.tyx = 0;
            self.tyy = UNITY;
            self.tx = 0;
            self.ty = 0;
        }
        Ok(())
    }

    // §960
    pub(crate) fn trans(&mut self, p: Pointer, q: Pointer) {
        let mp = self.mem[p as usize].rh;
        let mq = self.mem[q as usize].rh;
        let v = self.take_scaled(mp, self.txx) + self.take_scaled(mq, self.txy) + self.tx;
        self.mem[q as usize].rh =
            self.take_scaled(mp, self.tyx) + self.take_scaled(mq, self.tyy) + self.ty;
        self.mem[p as usize].rh = v;
    }

    // §961
    pub(crate) fn path_trans(&mut self, p: Pointer, c: i32) -> R {
        self.set_up_known_trans(c)?;
        self.unstash_cur_exp(p)?;
        if self.cur_type == PEN_TYPE {
            if self.max_offset(self.cur_exp) == 0 && self.tx == 0 && self.ty == 0 {
                return Ok(());
            }
            let pp = self.make_path(self.cur_exp)?;
            self.flush_cur_exp(pp)?;
            self.cur_type = FUTURE_PEN;
        }
        let mut q = self.cur_exp;
        loop {
            if self.left_type(q) != ENDPOINT {
                self.trans(q + 3, q + 4);
            }
            self.trans(q + 1, q + 2);
            if self.right_type(q) != ENDPOINT {
                self.trans(q + 5, q + 6);
            }
            q = self.link(q);
            if q == self.cur_exp {
                break;
            }
        }
        Ok(())
    }

    // §963
    pub(crate) fn edges_trans(&mut self, p: Pointer, c: i32) -> R {
        self.set_up_known_trans(c)?;
        self.unstash_cur_exp(p)?;
        self.cur_edges = self.cur_exp;
        if self.empty_edges(self.cur_edges) {
            return Ok(());
        }
        if self.txx == 0 && self.tyy == 0 && self.txy % UNITY == 0 && self.tyx % UNITY == 0 {
            self.xy_swap_edges()?;
            self.txx = self.txy;
            self.tyy = self.tyx;
            self.txy = 0;
            self.tyx = 0;
            if self.empty_edges(self.cur_edges) {
                return Ok(());
            }
        }
        if self.txy == 0 && self.tyx == 0 && self.txx % UNITY == 0 && self.tyy % UNITY == 0 {
            // §964
            if self.txx == 0 || self.tyy == 0 {
                self.toss_edges(self.cur_edges);
                self.cur_exp = self.get_node(EDGE_HEADER_SIZE)?;
                self.init_edges(self.cur_exp);
            } else {
                if self.txx < 0 {
                    self.x_reflect_edges();
                    self.txx = -self.txx;
                }
                if self.tyy < 0 {
                    self.y_reflect_edges();
                    self.tyy = -self.tyy;
                }
                if self.txx != UNITY {
                    self.x_scale_edges(self.txx / UNITY)?;
                }
                if self.tyy != UNITY {
                    self.y_scale_edges(self.tyy / UNITY)?;
                }
                // §965
                self.tx = round_unscaled(self.tx);
                self.ty = round_unscaled(self.ty);
                let ce = self.cur_edges;
                if self.m_min(ce) + self.tx <= 0
                    || self.m_max(ce) + self.tx >= 8192
                    || self.n_min(ce) + self.ty <= 0
                    || self.n_max(ce) + self.ty >= 8191
                    || self.tx.abs() >= 4096
                    || self.ty.abs() >= 4096
                {
                    self.print_err("Too far to shift");
                    self.help(&[
                        "I can't shift the picture as requested---it would",
                        "make some coordinates too large or too small.",
                        "Proceed, and I'll omit the transformation.",
                    ]);
                    self.put_get_error()?;
                } else {
                    if self.tx != 0 {
                        if !crate::edges::valid_range(self.m_offset(ce) - self.tx) {
                            self.fix_offset();
                        }
                        self.set_m_min(ce, self.m_min(ce) + self.tx);
                        self.set_m_max(ce, self.m_max(ce) + self.tx);
                        self.set_m_offset(ce, self.m_offset(ce) - self.tx);
                        self.set_last_window_time(ce, 0);
                    }
                    if self.ty != 0 {
                        self.set_n_min(ce, self.n_min(ce) + self.ty);
                        self.set_n_max(ce, self.n_max(ce) + self.ty);
                        self.set_n_pos(ce, self.n_pos(ce) + self.ty);
                        self.set_last_window_time(ce, 0);
                    }
                }
            }
            return Ok(());
        }
        self.print_err("That transformation is too hard");
        self.help(&[
            "I can apply complicated transformations to paths,",
            "but I can only do integer operations on pictures.",
            "Proceed, and I'll omit the transformation.",
        ]);
        self.put_get_error()
    }

    // §966
    pub(crate) fn big_trans(&mut self, p: Pointer, c: i32) -> R {
        let s = self.bns(self.type_(p));
        let q = self.value(p);
        let mut r = q + s;
        loop {
            r -= 2;
            if self.type_(r) != KNOWN {
                // §967
                self.set_up_known_trans(c)?;
                self.make_exp_copy(p)?;
                let r = self.value(self.cur_exp);
                if self.cur_type == TRANSFORM_TYPE {
                    self.bilin1(r + YY_PART_LOC, self.tyy, q + XY_PART_LOC, self.tyx, 0)?;
                    self.bilin1(r + YX_PART_LOC, self.tyy, q + XX_PART_LOC, self.tyx, 0)?;
                    self.bilin1(r + XY_PART_LOC, self.txx, q + YY_PART_LOC, self.txy, 0)?;
                    self.bilin1(r + XX_PART_LOC, self.txx, q + YX_PART_LOC, self.txy, 0)?;
                }
                self.bilin1(r + Y_PART_LOC, self.tyy, q + X_PART_LOC, self.tyx, self.ty)?;
                self.bilin1(r + X_PART_LOC, self.txx, q + Y_PART_LOC, self.txy, self.tx)?;
                return Ok(());
            }
            if r == q {
                break;
            }
        }
        // §970
        self.set_up_trans(c)?;
        if self.cur_type == KNOWN {
            // §973
            self.make_exp_copy(p)?;
            let r = self.value(self.cur_exp);
            if self.cur_type == TRANSFORM_TYPE {
                self.bilin3(
                    r + YY_PART_LOC,
                    self.tyy,
                    self.value(q + XY_PART_LOC),
                    self.tyx,
                    0,
                );
                self.bilin3(
                    r + YX_PART_LOC,
                    self.tyy,
                    self.value(q + XX_PART_LOC),
                    self.tyx,
                    0,
                );
                self.bilin3(
                    r + XY_PART_LOC,
                    self.txx,
                    self.value(q + YY_PART_LOC),
                    self.txy,
                    0,
                );
                self.bilin3(
                    r + XX_PART_LOC,
                    self.txx,
                    self.value(q + YX_PART_LOC),
                    self.txy,
                    0,
                );
            }
            self.bilin3(
                r + Y_PART_LOC,
                self.tyy,
                self.value(q + X_PART_LOC),
                self.tyx,
                self.ty,
            );
            self.bilin3(
                r + X_PART_LOC,
                self.txx,
                self.value(q + Y_PART_LOC),
                self.txy,
                self.tx,
            );
        } else {
            let pp = self.stash_cur_exp()?;
            let qq = self.value(pp);
            self.make_exp_copy(p)?;
            let r = self.value(self.cur_exp);
            if self.cur_type == TRANSFORM_TYPE {
                self.bilin2(
                    r + YY_PART_LOC,
                    qq + YY_PART_LOC,
                    self.value(q + XY_PART_LOC),
                    qq + YX_PART_LOC,
                    NULL,
                )?;
                self.bilin2(
                    r + YX_PART_LOC,
                    qq + YY_PART_LOC,
                    self.value(q + XX_PART_LOC),
                    qq + YX_PART_LOC,
                    NULL,
                )?;
                self.bilin2(
                    r + XY_PART_LOC,
                    qq + XX_PART_LOC,
                    self.value(q + YY_PART_LOC),
                    qq + XY_PART_LOC,
                    NULL,
                )?;
                self.bilin2(
                    r + XX_PART_LOC,
                    qq + XX_PART_LOC,
                    self.value(q + YX_PART_LOC),
                    qq + XY_PART_LOC,
                    NULL,
                )?;
            }
            self.bilin2(
                r + Y_PART_LOC,
                qq + YY_PART_LOC,
                self.value(q + X_PART_LOC),
                qq + YX_PART_LOC,
                qq + Y_PART_LOC,
            )?;
            self.bilin2(
                r + X_PART_LOC,
                qq + XX_PART_LOC,
                self.value(q + Y_PART_LOC),
                qq + XY_PART_LOC,
                qq + X_PART_LOC,
            )?;
            self.recycle_value(pp)?;
            self.free_node(pp, VALUE_NODE_SIZE);
        }
        Ok(())
    }

    // §968
    pub(crate) fn bilin1(
        &mut self,
        p: Pointer,
        t: Scaled,
        q: Pointer,
        u: Scaled,
        mut delta: Scaled,
    ) -> R {
        if t != UNITY {
            self.dep_mult(p, t, true)?;
        }
        if u != 0 {
            if self.type_(q) == KNOWN {
                delta += self.take_scaled(self.value(q), u);
            } else {
                // §969
                if self.type_(p) != PROTO_DEPENDENT {
                    if self.type_(p) == KNOWN {
                        let c = self.const_dependency(self.value(p))?;
                        self.new_dep(p, c);
                    } else {
                        let d = self.p_times_v(
                            self.dep_list(p),
                            UNITY,
                            DEPENDENT,
                            PROTO_DEPENDENT,
                            true,
                        );
                        self.set_dep_list(p, d);
                    }
                    self.set_type(p, PROTO_DEPENDENT);
                }
                let d = self.p_plus_fq(
                    self.dep_list(p),
                    u,
                    self.dep_list(q),
                    PROTO_DEPENDENT,
                    self.type_(q),
                )?;
                self.set_dep_list(p, d);
            }
        }
        if self.type_(p) == KNOWN {
            self.set_value(p, self.value(p) + delta);
        } else {
            let mut r = self.dep_list(p);
            while self.info(r) != NULL {
                r = self.link(r);
            }
            delta += self.value(r);
            if r != self.dep_list(p) {
                self.set_value(r, delta);
            } else {
                self.recycle_value(p)?;
                self.set_type(p, KNOWN);
                self.set_value(p, delta);
            }
        }
        if self.fix_needed {
            self.fix_dependencies()?;
        }
        Ok(())
    }

    // §971
    pub(crate) fn add_mult_dep(&mut self, p: Pointer, v: Scaled, r: Pointer) -> R {
        if self.type_(r) == KNOWN {
            let w = self.value(self.dep_final) + self.take_scaled(self.value(r), v);
            self.set_value(self.dep_final, w);
        } else {
            let d = self.p_plus_fq(
                self.dep_list(p),
                v,
                self.dep_list(r),
                PROTO_DEPENDENT,
                self.type_(r),
            )?;
            self.set_dep_list(p, d);
            if self.fix_needed {
                self.fix_dependencies()?;
            }
        }
        Ok(())
    }

    // §972
    pub(crate) fn bilin2(
        &mut self,
        p: Pointer,
        t: Pointer,
        v: Scaled,
        u: Pointer,
        q: Pointer,
    ) -> R {
        let mut vv = self.value(p);
        self.set_type(p, PROTO_DEPENDENT);
        let c = self.const_dependency(0)?;
        self.new_dep(p, c);
        if vv != 0 {
            self.add_mult_dep(p, vv, t)?;
        }
        if v != 0 {
            self.add_mult_dep(p, v, u)?;
        }
        if q != NULL {
            self.add_mult_dep(p, UNITY, q)?;
        }
        if self.dep_list(p) == self.dep_final {
            vv = self.value(self.dep_final);
            self.recycle_value(p)?;
            self.set_type(p, KNOWN);
            self.set_value(p, vv);
        }
        Ok(())
    }

    // §974
    pub(crate) fn bilin3(
        &mut self,
        p: Pointer,
        t: Scaled,
        v: Scaled,
        u: Scaled,
        mut delta: Scaled,
    ) {
        if t != UNITY {
            delta += self.take_scaled(self.value(p), t);
        } else {
            delta += self.value(p);
        }
        if u != 0 {
            let w = delta + self.take_scaled(v, u);
            self.set_value(p, w);
        } else {
            self.set_value(p, delta);
        }
    }

    // §976
    pub(crate) fn cat(&mut self, p: Pointer) -> R {
        let a = self.value(p);
        let b = self.cur_exp;
        self.str_room(self.length(a) + self.length(b))?;
        for k in self.str_start[a as usize]..self.str_start[(a + 1) as usize] {
            self.append_char(self.str_pool[k as usize]);
        }
        for k in self.str_start[b as usize]..self.str_start[(b + 1) as usize] {
            self.append_char(self.str_pool[k as usize]);
        }
        self.cur_exp = self.make_string()?;
        self.delete_str_ref(b);
        Ok(())
    }

    // §977
    pub(crate) fn chop_string(&mut self, p: Pointer) -> R {
        let mut a = round_unscaled(self.value(p + X_PART_LOC));
        let mut b = round_unscaled(self.value(p + Y_PART_LOC));
        let reversed;
        if a <= b {
            reversed = false;
        } else {
            reversed = true;
            std::mem::swap(&mut a, &mut b);
        }
        let s = self.cur_exp;
        let l = self.length(s);
        if a < 0 {
            a = 0;
            if b < 0 {
                b = 0;
            }
        }
        if b > l {
            b = l;
            if a > l {
                a = l;
            }
        }
        self.str_room(b - a)?;
        let ss = self.str_start[s as usize];
        if reversed {
            let mut k = ss + b - 1;
            while k >= ss + a {
                self.append_char(self.str_pool[k as usize]);
                k -= 1;
            }
        } else {
            for k in ss + a..ss + b {
                self.append_char(self.str_pool[k as usize]);
            }
        }
        self.cur_exp = self.make_string()?;
        self.delete_str_ref(s);
        Ok(())
    }

    // §978
    pub(crate) fn chop_path(&mut self, p: Pointer) -> R {
        let l = self.path_length();
        let mut a = self.value(p + X_PART_LOC);
        let mut b = self.value(p + Y_PART_LOC);
        let reversed;
        if a <= b {
            reversed = false;
        } else {
            reversed = true;
            std::mem::swap(&mut a, &mut b);
        }
        // §979
        if a < 0 {
            if self.left_type(self.cur_exp) == ENDPOINT {
                a = 0;
                if b < 0 {
                    b = 0;
                }
            } else {
                loop {
                    a += l;
                    b += l;
                    if a >= 0 {
                        break;
                    }
                }
            }
        }
        if b > l {
            if self.left_type(self.cur_exp) == ENDPOINT {
                b = l;
                if a > l {
                    a = l;
                }
            } else {
                while a >= l {
                    a -= l;
                    b -= l;
                }
            }
        }
        let mut q = self.cur_exp;
        while a >= UNITY {
            q = self.link(q);
            a -= UNITY;
            b -= UNITY;
        }
        let mut pp;
        let mut qq;
        if b == a {
            // §981
            if a > 0 {
                qq = self.link(q);
                self.split_cubic(q, a * 0o10000, self.x_coord(qq), self.y_coord(qq))?;
                q = self.link(q);
            }
            pp = self.copy_knot(q)?;
            qq = pp;
        } else {
            // §980
            pp = self.copy_knot(q)?;
            qq = pp;
            let mut rr;
            loop {
                q = self.link(q);
                rr = qq;
                qq = self.copy_knot(q)?;
                self.set_link(rr, qq);
                b -= UNITY;
                if b <= 0 {
                    break;
                }
            }
            if a > 0 {
                let ss = pp;
                pp = self.link(pp);
                self.split_cubic(ss, a * 0o10000, self.x_coord(pp), self.y_coord(pp))?;
                pp = self.link(ss);
                self.free_node(ss, KNOT_NODE_SIZE);
                if rr == ss {
                    b = self.make_scaled(b, UNITY - a);
                    rr = pp;
                }
            }
            if b < 0 {
                self.split_cubic(
                    rr,
                    (b + UNITY) * 0o10000,
                    self.x_coord(qq),
                    self.y_coord(qq),
                )?;
                self.free_node(qq, KNOT_NODE_SIZE);
                qq = self.link(rr);
            }
        }
        self.set_left_type(pp, ENDPOINT);
        self.set_right_type(qq, ENDPOINT);
        self.set_link(qq, pp);
        self.toss_knot_list(self.cur_exp);
        if reversed {
            let h = self.htap_ypoc(pp)?;
            self.cur_exp = self.link(h);
            self.toss_knot_list(pp);
        } else {
            self.cur_exp = pp;
        }
        Ok(())
    }

    // §982
    pub(crate) fn pair_value(&mut self, x: Scaled, y: Scaled) -> R {
        let mut p = self.get_node(VALUE_NODE_SIZE)?;
        self.flush_cur_exp(p)?;
        self.cur_type = PAIR_TYPE;
        self.set_type(p, PAIR_TYPE);
        self.set_name_type(p, CAPSULE);
        self.init_big_node(p)?;
        p = self.value(p);
        self.set_type(p + X_PART_LOC, KNOWN);
        self.set_value(p + X_PART_LOC, x);
        self.set_type(p + Y_PART_LOC, KNOWN);
        self.set_value(p + Y_PART_LOC, y);
        Ok(())
    }

    // §985
    pub(crate) fn find_point(&mut self, mut v: Scaled, c: i32) -> R {
        let mut p = self.cur_exp;
        let mut n = if self.left_type(p) == ENDPOINT {
            -UNITY
        } else {
            0
        };
        loop {
            p = self.link(p);
            n += UNITY;
            if p == self.cur_exp {
                break;
            }
        }
        if n == 0 {
            v = 0;
        } else if v < 0 {
            if self.left_type(p) == ENDPOINT {
                v = 0;
            } else {
                v = n - 1 - ((-v - 1) % n);
            }
        } else if v > n {
            if self.left_type(p) == ENDPOINT {
                v = n;
            } else {
                v %= n;
            }
        }
        p = self.cur_exp;
        while v >= UNITY {
            p = self.link(p);
            v -= UNITY;
        }
        if v != 0 {
            // §986
            let q = self.link(p);
            self.split_cubic(p, v * 0o10000, self.x_coord(q), self.y_coord(q))?;
            p = self.link(p);
        }
        // §987
        match c {
            POINT_OF => self.pair_value(self.x_coord(p), self.y_coord(p)),
            PRECONTROL_OF => {
                if self.left_type(p) == ENDPOINT {
                    self.pair_value(self.x_coord(p), self.y_coord(p))
                } else {
                    self.pair_value(self.left_x(p), self.left_y(p))
                }
            }
            _ => {
                if self.right_type(p) == ENDPOINT {
                    self.pair_value(self.x_coord(p), self.y_coord(p))
                } else {
                    self.pair_value(self.right_x(p), self.right_y(p))
                }
            }
        }
    }
}
