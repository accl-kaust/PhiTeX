//! Part 41 of `mf.web`: parsing secondary and higher expressions.

use crate::arith::{half, halfp};
use crate::{Mf, Pointer, R, consts::*};

impl Mf {
    // §862
    pub(crate) fn scan_secondary(&mut self) -> R {
        'restart: loop {
            if self.cur_cmd < MIN_PRIMARY_COMMAND || self.cur_cmd > MAX_PRIMARY_COMMAND {
                self.bad_exp("A secondary")?;
            }
            self.scan_primary()?;
            loop {
                if self.cur_cmd <= MAX_SECONDARY_COMMAND && self.cur_cmd >= MIN_SECONDARY_COMMAND {
                    let p = self.stash_cur_exp()?;
                    let c = self.cur_mod;
                    let d = self.cur_cmd;
                    let mut mac_name = NULL;
                    if d == SECONDARY_PRIMARY_MACRO {
                        mac_name = self.cur_sym;
                        self.add_mac_ref(c);
                    }
                    self.get_x_next()?;
                    self.scan_primary()?;
                    if d != SECONDARY_PRIMARY_MACRO {
                        self.do_binary(p, c)?;
                    } else {
                        self.back_input()?;
                        self.binary_mac(p, c, mac_name)?;
                        let rc = self.ref_count(c) - 1;
                        self.set_ref_count(c, rc);
                        self.get_x_next()?;
                        continue 'restart;
                    }
                    continue;
                }
                return Ok(());
            }
        }
    }

    // §863
    pub(crate) fn binary_mac(&mut self, p: Pointer, c: Pointer, n: Pointer) -> R {
        let q = self.get_avail()?;
        let r = self.get_avail()?;
        self.set_link(q, r);
        self.set_info(q, p);
        let s = self.stash_cur_exp()?;
        self.set_info(r, s);
        self.macro_call(c, q, n)
    }

    // §864
    pub(crate) fn scan_tertiary(&mut self) -> R {
        'restart: loop {
            if self.cur_cmd < MIN_PRIMARY_COMMAND || self.cur_cmd > MAX_PRIMARY_COMMAND {
                self.bad_exp("A tertiary")?;
            }
            self.scan_secondary()?;
            if self.cur_type == FUTURE_PEN {
                self.materialize_pen()?;
            }
            loop {
                if self.cur_cmd <= MAX_TERTIARY_COMMAND && self.cur_cmd >= MIN_TERTIARY_COMMAND {
                    let p = self.stash_cur_exp()?;
                    let c = self.cur_mod;
                    let d = self.cur_cmd;
                    let mut mac_name = NULL;
                    if d == TERTIARY_SECONDARY_MACRO {
                        mac_name = self.cur_sym;
                        self.add_mac_ref(c);
                    }
                    self.get_x_next()?;
                    self.scan_secondary()?;
                    if d != TERTIARY_SECONDARY_MACRO {
                        self.do_binary(p, c)?;
                    } else {
                        self.back_input()?;
                        self.binary_mac(p, c, mac_name)?;
                        let rc = self.ref_count(c) - 1;
                        self.set_ref_count(c, rc);
                        self.get_x_next()?;
                        continue 'restart;
                    }
                    continue;
                }
                return Ok(());
            }
        }
    }

    // §865
    pub(crate) fn materialize_pen(&mut self) -> R {
        let mut q = self.cur_exp;
        'common_ending: {
            if self.left_type(q) == ENDPOINT {
                self.print_err("Pen path must be a cycle");
                self.help(&[
                    "I can't make a pen from the given path.",
                    "So I've replaced it by the trivial path `(0,0)..cycle'.",
                ]);
                self.put_get_error()?;
                self.cur_exp = NULL_PEN;
                break 'common_ending;
            } else if self.left_type(q) == OPEN {
                // §866
                self.tx = self.x_coord(q);
                self.ty = self.y_coord(q);
                self.txx = self.left_x(q) - self.tx;
                self.tyx = self.left_y(q) - self.ty;
                self.txy = self.right_x(q) - self.tx;
                self.tyy = self.right_y(q) - self.ty;
                let a_minus_b = self.pyth_add(self.txx - self.tyy, self.tyx + self.txy);
                let a_plus_b = self.pyth_add(self.txx + self.tyy, self.tyx - self.txy);
                let major_axis = halfp(a_minus_b + a_plus_b);
                let minor_axis = halfp((a_plus_b - a_minus_b).abs());
                let theta = if major_axis == minor_axis {
                    0
                } else {
                    let a = self.n_arg(self.txx - self.tyy, self.tyx + self.txy)?;
                    let b = self.n_arg(self.txx + self.tyy, self.tyx - self.txy)?;
                    half(a + b)
                };
                self.free_node(q, KNOT_NODE_SIZE);
                q = self.make_ellipse(major_axis, minor_axis, theta)?;
                if self.tx != 0 || self.ty != 0 {
                    // §867
                    let mut p = q;
                    loop {
                        self.set_x_coord(p, self.x_coord(p) + self.tx);
                        self.set_y_coord(p, self.y_coord(p) + self.ty);
                        p = self.link(p);
                        if p == q {
                            break;
                        }
                    }
                }
            }
            self.cur_exp = self.make_pen(q)?;
        }
        self.toss_knot_list(q);
        self.cur_type = PEN_TYPE;
        Ok(())
    }

    // §868
    pub(crate) fn scan_expression(&mut self) -> R {
        let my_var_flag = self.var_flag;
        'restart: loop {
            if self.cur_cmd < MIN_PRIMARY_COMMAND || self.cur_cmd > MAX_PRIMARY_COMMAND {
                self.bad_exp("An")?;
            }
            self.scan_tertiary()?;
            loop {
                // continue:
                if !(self.cur_cmd <= MAX_EXPRESSION_COMMAND
                    && self.cur_cmd >= MIN_EXPRESSION_COMMAND
                    && (self.cur_cmd != EQUALS || my_var_flag != ASSIGNMENT))
                {
                    return Ok(());
                }
                let mut p = self.stash_cur_exp()?;
                let c = self.cur_mod;
                let mut d = self.cur_cmd;
                let mut mac_name = NULL;
                if d == EXPRESSION_TERTIARY_MACRO {
                    mac_name = self.cur_sym;
                    self.add_mac_ref(c);
                }
                if d < AMPERSAND
                    || (d == AMPERSAND
                        && (self.type_(p) == PAIR_TYPE || self.type_(p) == PATH_TYPE))
                {
                    // §869
                    let mut cycle_hit = false;
                    // §870
                    self.unstash_cur_exp(p)?;
                    if self.cur_type == PAIR_TYPE {
                        p = self.new_knot()?;
                    } else if self.cur_type == PATH_TYPE {
                        p = self.cur_exp;
                    } else {
                        return Ok(());
                    }
                    let mut q = p;
                    while self.link(q) != p {
                        q = self.link(q);
                    }
                    if self.left_type(p) != ENDPOINT {
                        let r = self.copy_knot(p)?;
                        self.set_link(q, r);
                        q = r;
                    }
                    self.set_left_type(p, OPEN);
                    self.set_right_type(q, OPEN);
                    let mut x = 0;
                    let mut y = 0;
                    let mut t = OPEN;
                    'finish_path: loop {
                        // continue_path: §874
                        if self.cur_cmd == LEFT_BRACE {
                            // §879
                            let tt = self.scan_direction()?;
                            if tt != OPEN {
                                self.set_right_type(q, tt);
                                self.set_right_x(q, self.cur_exp);
                                if self.left_type(q) == OPEN {
                                    self.set_left_type(q, tt);
                                    self.set_left_x(q, self.cur_exp);
                                }
                            }
                        }
                        d = self.cur_cmd;
                        if d == PATH_JOIN {
                            // §881
                            self.get_x_next()?;
                            'done: {
                                if self.cur_cmd == TENSION {
                                    // §882
                                    self.get_x_next()?;
                                    y = self.cur_cmd;
                                    if self.cur_cmd == AT_LEAST {
                                        self.get_x_next()?;
                                    }
                                    self.scan_primary()?;
                                    self.check_tension()?;
                                    if y == AT_LEAST {
                                        self.cur_exp = -self.cur_exp;
                                    }
                                    self.set_right_y(q, self.cur_exp);
                                    if self.cur_cmd == AND_COMMAND {
                                        self.get_x_next()?;
                                        y = self.cur_cmd;
                                        if self.cur_cmd == AT_LEAST {
                                            self.get_x_next()?;
                                        }
                                        self.scan_primary()?;
                                        self.check_tension()?;
                                        if y == AT_LEAST {
                                            self.cur_exp = -self.cur_exp;
                                        }
                                    }
                                    y = self.cur_exp;
                                } else if self.cur_cmd == CONTROLS {
                                    // §884
                                    self.set_right_type(q, EXPLICIT);
                                    t = EXPLICIT;
                                    self.get_x_next()?;
                                    self.scan_primary()?;
                                    self.known_pair()?;
                                    self.set_right_x(q, self.cur_x);
                                    self.set_right_y(q, self.cur_y);
                                    if self.cur_cmd != AND_COMMAND {
                                        x = self.right_x(q);
                                        y = self.right_y(q);
                                    } else {
                                        self.get_x_next()?;
                                        self.scan_primary()?;
                                        self.known_pair()?;
                                        x = self.cur_x;
                                        y = self.cur_y;
                                    }
                                } else {
                                    self.set_right_y(q, UNITY);
                                    y = UNITY;
                                    self.back_input()?;
                                    break 'done;
                                }
                                if self.cur_cmd != PATH_JOIN {
                                    self.missing_err("..");
                                    self.help(&["A path join command should end with two dots."]);
                                    self.back_error()?;
                                }
                            }
                        } else if d != AMPERSAND {
                            break 'finish_path;
                        }
                        self.get_x_next()?;
                        if self.cur_cmd == LEFT_BRACE {
                            // §880
                            t = self.scan_direction()?;
                            if self.right_type(q) != EXPLICIT {
                                x = self.cur_exp;
                            } else {
                                t = EXPLICIT;
                            }
                        } else if self.right_type(q) != EXPLICIT {
                            t = OPEN;
                            x = 0;
                        }
                        let pp;
                        let mut qq;
                        if self.cur_cmd == CYCLE {
                            // §886
                            cycle_hit = true;
                            self.get_x_next()?;
                            pp = p;
                            qq = p;
                            if d == AMPERSAND && p == q {
                                d = PATH_JOIN;
                                self.set_right_y(q, UNITY);
                                y = UNITY;
                            }
                        } else {
                            self.scan_tertiary()?;
                            // §885
                            if self.cur_type != PATH_TYPE {
                                pp = self.new_knot()?;
                            } else {
                                pp = self.cur_exp;
                            }
                            qq = pp;
                            while self.link(qq) != pp {
                                qq = self.link(qq);
                            }
                            if self.left_type(pp) != ENDPOINT {
                                let r = self.copy_knot(pp)?;
                                self.set_link(qq, r);
                                qq = r;
                            }
                            self.set_left_type(pp, OPEN);
                            self.set_right_type(qq, OPEN);
                        }
                        // §887
                        if d == AMPERSAND
                            && (self.x_coord(q) != self.x_coord(pp)
                                || self.y_coord(q) != self.y_coord(pp))
                        {
                            self.print_err("Paths don't touch; `&' will be changed to `..'");
                            self.help(&[
                                "When you join paths `p&q', the ending point of p",
                                "must be exactly equal to the starting point of q.",
                                "So I'm going to pretend that you said `p..q' instead.",
                            ]);
                            self.put_get_error()?;
                            d = PATH_JOIN;
                            self.set_right_y(q, UNITY);
                            y = UNITY;
                        }
                        // §889
                        if self.right_type(pp) == OPEN && (t == CURL || t == GIVEN) {
                            self.set_right_type(pp, t);
                            self.set_right_x(pp, x);
                        }
                        if d == AMPERSAND {
                            // §890
                            if self.left_type(q) == OPEN && self.right_type(q) == OPEN {
                                self.set_left_type(q, CURL);
                                self.set_left_x(q, UNITY);
                            }
                            if self.right_type(pp) == OPEN && t == OPEN {
                                self.set_right_type(pp, CURL);
                                self.set_right_x(pp, UNITY);
                            }
                            self.set_right_type(q, self.right_type(pp));
                            self.set_link(q, self.link(pp));
                            self.set_right_x(q, self.right_x(pp));
                            self.set_right_y(q, self.right_y(pp));
                            self.free_node(pp, KNOT_NODE_SIZE);
                            if qq == pp {
                                qq = q;
                            }
                        } else {
                            // §888
                            if self.right_type(q) == OPEN
                                && (self.left_type(q) == CURL || self.left_type(q) == GIVEN)
                            {
                                self.set_right_type(q, self.left_type(q));
                                self.set_right_x(q, self.left_x(q));
                            }
                            self.set_link(q, pp);
                            self.set_left_y(pp, y);
                            if t != OPEN {
                                self.set_left_x(pp, x);
                                self.set_left_type(pp, t);
                            }
                        }
                        q = qq;
                        if self.cur_cmd >= MIN_EXPRESSION_COMMAND
                            && self.cur_cmd <= AMPERSAND
                            && !cycle_hit
                        {
                            continue;
                        }
                        break;
                    }
                    // finish_path: §891
                    if cycle_hit {
                        if d == AMPERSAND {
                            p = q;
                        }
                    } else {
                        self.set_left_type(p, ENDPOINT);
                        if self.right_type(p) == OPEN {
                            self.set_right_type(p, CURL);
                            self.set_right_x(p, UNITY);
                        }
                        self.set_right_type(q, ENDPOINT);
                        if self.left_type(q) == OPEN {
                            self.set_left_type(q, CURL);
                            self.set_left_x(q, UNITY);
                        }
                        self.set_link(q, p);
                    }
                    self.make_choices(p)?;
                    self.cur_type = PATH_TYPE;
                    self.cur_exp = p;
                } else {
                    self.get_x_next()?;
                    self.scan_tertiary()?;
                    if d != EXPRESSION_TERTIARY_MACRO {
                        self.do_binary(p, c)?;
                    } else {
                        self.back_input()?;
                        self.binary_mac(p, c, mac_name)?;
                        let rc = self.ref_count(c) - 1;
                        self.set_ref_count(c, rc);
                        self.get_x_next()?;
                        continue 'restart;
                    }
                }
            }
        }
    }

    // §883
    fn check_tension(&mut self) -> R {
        if self.cur_type != KNOWN || self.cur_exp < THREE_QUARTER_UNIT {
            self.exp_err("Improper tension has been set to 1")?;
            self.help(&["The expression above should have been a number >=3/4."]);
            self.put_get_flush_error(UNITY)?;
        }
        Ok(())
    }

    // §871
    pub(crate) fn new_knot(&mut self) -> R<Pointer> {
        let q = self.get_node(KNOT_NODE_SIZE)?;
        self.set_left_type(q, ENDPOINT);
        self.set_right_type(q, ENDPOINT);
        self.set_link(q, q);
        self.known_pair()?;
        self.set_x_coord(q, self.cur_x);
        self.set_y_coord(q, self.cur_y);
        Ok(q)
    }

    fn undefined_coordinate(&mut self, p: Pointer, s: &str, xy: char) -> R {
        self.disp_err(p, s)?;
        if xy == 'x' {
            self.help(&[
                "I need a `known' x value for this part of the path.",
                "The value I found (see above) was no good;",
                "so I'll try to keep going by using zero instead.",
                "(Chapter 27 of The METAFONTbook explains that",
                "you might want to type `I ???' now.)",
            ]);
        } else {
            self.help(&[
                "I need a `known' y value for this part of the path.",
                "The value I found (see above) was no good;",
                "so I'll try to keep going by using zero instead.",
                "(Chapter 27 of The METAFONTbook explains that",
                "you might want to type `I ???' now.)",
            ]);
        }
        Ok(())
    }

    // §872
    pub(crate) fn known_pair(&mut self) -> R {
        if self.cur_type != PAIR_TYPE {
            self.exp_err("Undefined coordinates have been replaced by (0,0)")?;
            self.help(&[
                "I need x and y numbers for this part of the path.",
                "The value I found (see above) was no good;",
                "so I'll try to keep going by using zero instead.",
                "(Chapter 27 of The METAFONTbook explains that",
                "you might want to type `I ???' now.)",
            ]);
            self.put_get_flush_error(0)?;
            self.cur_x = 0;
            self.cur_y = 0;
        } else {
            let p = self.value(self.cur_exp);
            // §873
            if self.type_(p) == KNOWN {
                self.cur_x = self.value(p);
            } else {
                self.undefined_coordinate(p, "Undefined x coordinate has been replaced by 0", 'x')?;
                self.put_get_error()?;
                self.recycle_value(p)?;
                self.cur_x = 0;
            }
            if self.type_(p + 2) == KNOWN {
                self.cur_y = self.value(p + 2);
            } else {
                self.undefined_coordinate(
                    p + 2,
                    "Undefined y coordinate has been replaced by 0",
                    'y',
                )?;
                self.put_get_error()?;
                self.recycle_value(p + 2)?;
                self.cur_y = 0;
            }
            self.flush_cur_exp(0)?;
        }
        Ok(())
    }

    // §875
    pub(crate) fn scan_direction(&mut self) -> R<i32> {
        let t;
        self.get_x_next()?;
        if self.cur_cmd == CURL_COMMAND {
            // §876
            self.get_x_next()?;
            self.scan_expression()?;
            if self.cur_type != KNOWN || self.cur_exp < 0 {
                self.exp_err("Improper curl has been replaced by 1")?;
                self.help(&["A curl must be a known, nonnegative number."]);
                self.put_get_flush_error(UNITY)?;
            }
            t = CURL;
        } else {
            // §877
            self.scan_expression()?;
            if self.cur_type > PAIR_TYPE {
                // §878
                if self.cur_type != KNOWN {
                    self.undefined_coordinate(
                        NULL,
                        "Undefined x coordinate has been replaced by 0",
                        'x',
                    )?;
                    self.put_get_flush_error(0)?;
                }
                let x = self.cur_exp;
                if self.cur_cmd != COMMA {
                    self.missing_err(",");
                    self.help(&[
                        "I've got the x coordinate of a path direction;",
                        "will look for the y coordinate next.",
                    ]);
                    self.back_error()?;
                }
                self.get_x_next()?;
                self.scan_expression()?;
                if self.cur_type != KNOWN {
                    self.undefined_coordinate(
                        NULL,
                        "Undefined y coordinate has been replaced by 0",
                        'y',
                    )?;
                    self.put_get_flush_error(0)?;
                }
                self.cur_y = self.cur_exp;
                self.cur_x = x;
            } else {
                self.known_pair()?;
            }
            if self.cur_x == 0 && self.cur_y == 0 {
                t = OPEN;
            } else {
                t = GIVEN;
                self.cur_exp = self.n_arg(self.cur_x, self.cur_y)?;
            }
        }
        if self.cur_cmd != RIGHT_BRACE {
            self.missing_err("}");
            self.help(&[
                "I've scanned a direction spec for part of a path,",
                "so a right brace should have come next.",
                "I shall pretend that one was there.",
            ]);
            self.back_error()?;
        }
        self.get_x_next()?;
        Ok(t)
    }

    // §892
    pub(crate) fn get_boolean(&mut self) -> R {
        self.get_x_next()?;
        self.scan_expression()?;
        if self.cur_type != BOOLEAN_TYPE {
            self.exp_err("Undefined condition will be treated as `false'")?;
            self.help(&[
                "The expression shown above should have had a definite",
                "true-or-false value. I'm changing it to `false'.",
            ]);
            self.put_get_flush_error(FALSE_CODE)?;
            self.cur_type = BOOLEAN_TYPE;
        }
        Ok(())
    }
}
