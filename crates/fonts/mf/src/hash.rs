//! Parts 12 and 13 of `mf.web`: the command codes' printing, the hash
//! table, the primitives; TeX Live's string pool (`mf.pool`, made by
//! `tangle mf.web mf.ch`), loaded as `mf-binpool.ch` does.

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::{Mf, Pointer, R, StrNumber, consts::*};

const POOL_FILE: &str = include_str!("mf.pool");

/// The strings of `mf.pool`, in order (string 256 first).
pub(crate) fn pool_strings() -> &'static [&'static str] {
    static P: OnceLock<Vec<&'static str>> = OnceLock::new();
    P.get_or_init(|| {
        let mut v = Vec::new();
        for line in POOL_FILE.split('\n') {
            if line.starts_with('*') || line.len() < 2 {
                continue;
            }
            let n: usize = line[..2].parse().expect("pool length");
            v.push(&line[2..2 + n]);
        }
        v
    })
}

fn pool_index() -> &'static HashMap<&'static str, i32> {
    static M: OnceLock<HashMap<&'static str, i32>> = OnceLock::new();
    M.get_or_init(|| {
        let mut m = HashMap::new();
        for (i, s) in pool_strings().iter().enumerate() {
            m.entry(*s).or_insert(256 + i as i32);
        }
        m
    })
}

impl Mf {
    /// The string number tangle gave the constant `s` (`"s"` in `mf.web`).
    pub(crate) fn pstr(&self, s: &str) -> StrNumber {
        let b = s.as_bytes();
        if b.len() == 1 {
            return i32::from(b[0]);
        }
        match pool_index().get(s) {
            Some(&n) => n,
            None => panic!("string {s:?} is not in mf.pool"),
        }
    }

    // §187
    pub(crate) fn print_type(&mut self, t: i32) {
        let s = match t {
            VACUOUS => "vacuous",
            BOOLEAN_TYPE => "boolean",
            UNKNOWN_BOOLEAN => "unknown boolean",
            STRING_TYPE => "string",
            UNKNOWN_STRING => "unknown string",
            PEN_TYPE => "pen",
            UNKNOWN_PEN => "unknown pen",
            FUTURE_PEN => "future pen",
            PATH_TYPE => "path",
            UNKNOWN_PATH => "unknown path",
            PICTURE_TYPE => "picture",
            UNKNOWN_PICTURE => "unknown picture",
            TRANSFORM_TYPE => "transform",
            PAIR_TYPE => "pair",
            KNOWN => "known numeric",
            DEPENDENT => "dependent",
            PROTO_DEPENDENT => "proto-dependent",
            NUMERIC_TYPE => "numeric",
            INDEPENDENT => "independent",
            TOKEN_LIST => "token list",
            STRUCTURED => "structured",
            UNSUFFIXED_MACRO => "unsuffixed macro",
            SUFFIXED_MACRO => "suffixed macro",
            _ => "undefined",
        };
        self.prints(s);
    }

    // §189
    pub(crate) fn print_op(&mut self, c: i32) {
        if c <= NUMERIC_TYPE {
            self.print_type(c);
            return;
        }
        let s = match c {
            TRUE_CODE => "true",
            FALSE_CODE => "false",
            NULL_PICTURE_CODE => "nullpicture",
            NULL_PEN_CODE => "nullpen",
            JOB_NAME_OP => "jobname",
            READ_STRING_OP => "readstring",
            PEN_CIRCLE => "pencircle",
            NORMAL_DEVIATE => "normaldeviate",
            ODD_OP => "odd",
            KNOWN_OP => "known",
            UNKNOWN_OP => "unknown",
            NOT_OP => "not",
            DECIMAL => "decimal",
            REVERSE => "reverse",
            MAKE_PATH_OP => "makepath",
            MAKE_PEN_OP => "makepen",
            TOTAL_WEIGHT_OP => "totalweight",
            OCT_OP => "oct",
            HEX_OP => "hex",
            ASCII_OP => "ASCII",
            CHAR_OP => "char",
            LENGTH_OP => "length",
            TURNING_OP => "turningnumber",
            X_PART => "xpart",
            Y_PART => "ypart",
            XX_PART => "xxpart",
            XY_PART => "xypart",
            YX_PART => "yxpart",
            YY_PART => "yypart",
            SQRT_OP => "sqrt",
            M_EXP_OP => "mexp",
            M_LOG_OP => "mlog",
            SIN_D_OP => "sind",
            COS_D_OP => "cosd",
            FLOOR_OP => "floor",
            UNIFORM_DEVIATE => "uniformdeviate",
            CHAR_EXISTS_OP => "charexists",
            ANGLE_OP => "angle",
            CYCLE_OP => "cycle",
            PLUS => "+",
            MINUS => "-",
            TIMES => "*",
            OVER => "/",
            PYTHAG_ADD => "++",
            PYTHAG_SUB => "+-+",
            OR_OP => "or",
            AND_OP => "and",
            LESS_THAN => "<",
            LESS_OR_EQUAL => "<=",
            GREATER_THAN => ">",
            GREATER_OR_EQUAL => ">=",
            EQUAL_TO => "=",
            UNEQUAL_TO => "<>",
            CONCATENATE => "&",
            ROTATED_BY => "rotated",
            SLANTED_BY => "slanted",
            SCALED_BY => "scaled",
            SHIFTED_BY => "shifted",
            TRANSFORMED_BY => "transformed",
            X_SCALED => "xscaled",
            Y_SCALED => "yscaled",
            Z_SCALED => "zscaled",
            INTERSECT => "intersectiontimes",
            SUBSTRING_OF => "substring",
            SUBPATH_OF => "subpath",
            DIRECTION_TIME_OF => "directiontime",
            POINT_OF => "point",
            PRECONTROL_OF => "precontrol",
            POSTCONTROL_OF => "postcontrol",
            PEN_OFFSET_OF => "penoffset",
            _ => "..",
        };
        self.prints(s);
    }

    // §194
    pub(crate) fn fix_date_and_time(&mut self) {
        let (t, d, m, y) = self.cfg.date;
        self.sys_time = t;
        self.sys_day = d;
        self.sys_month = m;
        self.sys_year = y;
        self.internal[TIME as usize] = self.sys_time * UNITY;
        self.internal[DAY as usize] = self.sys_day * UNITY;
        self.internal[MONTH as usize] = self.sys_month * UNITY;
        self.internal[YEAR as usize] = self.sys_year * UNITY;
    }

    // §205
    pub(crate) fn id_lookup(&mut self, j: i32, l: i32) -> R<Pointer> {
        if l == 1 {
            // §206
            let p = i32::from(self.buffer[j as usize]) + 1;
            self.set_text(p, p - 1);
            return Ok(p);
        }
        // §208
        let mut h = i32::from(self.buffer[j as usize]);
        for k in j + 1..j + l {
            h = h + h + i32::from(self.buffer[k as usize]);
            while h >= HASH_PRIME {
                h -= HASH_PRIME;
            }
        }
        let mut p = h + HASH_BASE;
        loop {
            if self.text(p) > 0
                && self.length(self.text(p)) == l
                && self.str_eq_buf(self.text(p), j)
            {
                return Ok(p);
            }
            if self.next(p) == 0 {
                // §207
                if self.text(p) > 0 {
                    loop {
                        if self.hash_used == HASH_BASE {
                            return self.overflow("hash size", HASH_SIZE);
                        }
                        self.hash_used -= 1;
                        if self.text(self.hash_used) == 0 {
                            break;
                        }
                    }
                    self.set_next(p, self.hash_used);
                    p = self.hash_used;
                }
                self.str_room(l)?;
                for k in j..j + l {
                    self.append_char(self.buffer[k as usize]);
                }
                let s = self.make_string()?;
                self.set_text(p, s);
                self.str_ref[s as usize] = MAX_STR_REF as u8;
                self.st_count += 1;
                return Ok(p);
            }
            p = self.next(p);
        }
    }

    // §210
    pub(crate) fn primitive(&mut self, s: &str, c: i32, o: i32) -> R {
        let s = self.pstr(s);
        let k = self.str_start[s as usize];
        let l = self.str_start[(s + 1) as usize] - k;
        for j in 0..l {
            self.buffer[j as usize] = self.str_pool[(k + j) as usize];
        }
        self.cur_sym = self.id_lookup(0, l)?;
        if s >= 256 {
            self.flush_string(self.str_ptr - 1);
            self.set_text(self.cur_sym, s);
        }
        self.set_eq_type(self.cur_sym, c);
        self.set_equiv(self.cur_sym, o);
        Ok(())
    }

    // §1210: every `primitive` call of `mf.web`, in its order.
    pub(crate) fn init_prim(&mut self) -> R {
        self.primitive("tracingtitles", INTERNAL_QUANTITY, TRACING_TITLES)?;
        self.primitive("tracingequations", INTERNAL_QUANTITY, TRACING_EQUATIONS)?;
        self.primitive("tracingcapsules", INTERNAL_QUANTITY, TRACING_CAPSULES)?;
        self.primitive("tracingchoices", INTERNAL_QUANTITY, TRACING_CHOICES)?;
        self.primitive("tracingspecs", INTERNAL_QUANTITY, TRACING_SPECS)?;
        self.primitive("tracingpens", INTERNAL_QUANTITY, TRACING_PENS)?;
        self.primitive("tracingcommands", INTERNAL_QUANTITY, TRACING_COMMANDS)?;
        self.primitive("tracingrestores", INTERNAL_QUANTITY, TRACING_RESTORES)?;
        self.primitive("tracingmacros", INTERNAL_QUANTITY, TRACING_MACROS)?;
        self.primitive("tracingedges", INTERNAL_QUANTITY, TRACING_EDGES)?;
        self.primitive("tracingoutput", INTERNAL_QUANTITY, TRACING_OUTPUT)?;
        self.primitive("tracingstats", INTERNAL_QUANTITY, TRACING_STATS)?;
        self.primitive("tracingonline", INTERNAL_QUANTITY, TRACING_ONLINE)?;
        self.primitive("year", INTERNAL_QUANTITY, YEAR)?;
        self.primitive("month", INTERNAL_QUANTITY, MONTH)?;
        self.primitive("day", INTERNAL_QUANTITY, DAY)?;
        self.primitive("time", INTERNAL_QUANTITY, TIME)?;
        self.primitive("charcode", INTERNAL_QUANTITY, CHAR_CODE)?;
        self.primitive("charext", INTERNAL_QUANTITY, CHAR_EXT)?;
        self.primitive("charwd", INTERNAL_QUANTITY, CHAR_WD)?;
        self.primitive("charht", INTERNAL_QUANTITY, CHAR_HT)?;
        self.primitive("chardp", INTERNAL_QUANTITY, CHAR_DP)?;
        self.primitive("charic", INTERNAL_QUANTITY, CHAR_IC)?;
        self.primitive("chardx", INTERNAL_QUANTITY, CHAR_DX)?;
        self.primitive("chardy", INTERNAL_QUANTITY, CHAR_DY)?;
        self.primitive("designsize", INTERNAL_QUANTITY, DESIGN_SIZE)?;
        self.primitive("hppp", INTERNAL_QUANTITY, HPPP)?;
        self.primitive("vppp", INTERNAL_QUANTITY, VPPP)?;
        self.primitive("xoffset", INTERNAL_QUANTITY, X_OFFSET)?;
        self.primitive("yoffset", INTERNAL_QUANTITY, Y_OFFSET)?;
        self.primitive("pausing", INTERNAL_QUANTITY, PAUSING)?;
        self.primitive("showstopping", INTERNAL_QUANTITY, SHOWSTOPPING)?;
        self.primitive("fontmaking", INTERNAL_QUANTITY, FONTMAKING)?;
        self.primitive("proofing", INTERNAL_QUANTITY, PROOFING)?;
        self.primitive("smoothing", INTERNAL_QUANTITY, SMOOTHING)?;
        self.primitive("autorounding", INTERNAL_QUANTITY, AUTOROUNDING)?;
        self.primitive("granularity", INTERNAL_QUANTITY, GRANULARITY)?;
        self.primitive("fillin", INTERNAL_QUANTITY, FILLIN)?;
        self.primitive("turningcheck", INTERNAL_QUANTITY, TURNING_CHECK)?;
        self.primitive("warningcheck", INTERNAL_QUANTITY, WARNING_CHECK)?;
        self.primitive("boundarychar", INTERNAL_QUANTITY, BOUNDARY_CHAR)?;
        self.primitive("..", PATH_JOIN, 0)?;
        self.primitive("[", LEFT_BRACKET, 0)?;
        self.eqtb[FROZEN_LEFT_BRACKET as usize] = self.eqtb[self.cur_sym as usize];
        self.primitive("]", RIGHT_BRACKET, 0)?;
        self.primitive("}", RIGHT_BRACE, 0)?;
        self.primitive("{", LEFT_BRACE, 0)?;
        self.primitive(":", COLON, 0)?;
        self.eqtb[FROZEN_COLON as usize] = self.eqtb[self.cur_sym as usize];
        self.primitive("::", DOUBLE_COLON, 0)?;
        self.primitive("||:", BCHAR_LABEL, 0)?;
        self.primitive(":=", ASSIGNMENT, 0)?;
        self.primitive(",", COMMA, 0)?;
        self.primitive(";", SEMICOLON, 0)?;
        self.eqtb[FROZEN_SEMICOLON as usize] = self.eqtb[self.cur_sym as usize];
        self.primitive("\\", RELAX, 0)?;
        self.primitive("addto", ADD_TO_COMMAND, 0)?;
        self.primitive("at", AT_TOKEN, 0)?;
        self.primitive("atleast", AT_LEAST, 0)?;
        self.primitive("begingroup", BEGIN_GROUP, 0)?;
        self.bg_loc = self.cur_sym;
        self.primitive("controls", CONTROLS, 0)?;
        self.primitive("cull", CULL_COMMAND, 0)?;
        self.primitive("curl", CURL_COMMAND, 0)?;
        self.primitive("delimiters", DELIMITERS, 0)?;
        self.primitive("display", DISPLAY_COMMAND, 0)?;
        self.primitive("endgroup", END_GROUP, 0)?;
        self.eqtb[FROZEN_END_GROUP as usize] = self.eqtb[self.cur_sym as usize];
        self.eg_loc = self.cur_sym;
        self.primitive("everyjob", EVERY_JOB_COMMAND, 0)?;
        self.primitive("exitif", EXIT_TEST, 0)?;
        self.primitive("expandafter", EXPAND_AFTER, 0)?;
        self.primitive("from", FROM_TOKEN, 0)?;
        self.primitive("inwindow", IN_WINDOW, 0)?;
        self.primitive("interim", INTERIM_COMMAND, 0)?;
        self.primitive("let", LET_COMMAND, 0)?;
        self.primitive("newinternal", NEW_INTERNAL, 0)?;
        self.primitive("of", OF_TOKEN, 0)?;
        self.primitive("openwindow", OPEN_WINDOW, 0)?;
        self.primitive("randomseed", RANDOM_SEED, 0)?;
        self.primitive("save", SAVE_COMMAND, 0)?;
        self.primitive("scantokens", SCAN_TOKENS, 0)?;
        self.primitive("shipout", SHIP_OUT_COMMAND, 0)?;
        self.primitive("skipto", SKIP_TO, 0)?;
        self.primitive("step", STEP_TOKEN, 0)?;
        self.primitive("str", STR_OP, 0)?;
        self.primitive("tension", TENSION, 0)?;
        self.primitive("to", TO_TOKEN, 0)?;
        self.primitive("until", UNTIL_TOKEN, 0)?;
        self.primitive("def", MACRO_DEF, START_DEF)?;
        self.primitive("vardef", MACRO_DEF, VAR_DEF)?;
        self.primitive("primarydef", MACRO_DEF, SECONDARY_PRIMARY_MACRO)?;
        self.primitive("secondarydef", MACRO_DEF, TERTIARY_SECONDARY_MACRO)?;
        self.primitive("tertiarydef", MACRO_DEF, EXPRESSION_TERTIARY_MACRO)?;
        self.primitive("enddef", MACRO_DEF, END_DEF)?;
        self.eqtb[FROZEN_END_DEF as usize] = self.eqtb[self.cur_sym as usize];
        self.primitive("for", ITERATION, EXPR_BASE)?;
        self.primitive("forsuffixes", ITERATION, SUFFIX_BASE)?;
        self.primitive("forever", ITERATION, START_FOREVER)?;
        self.primitive("endfor", ITERATION, END_FOR)?;
        self.eqtb[FROZEN_END_FOR as usize] = self.eqtb[self.cur_sym as usize];
        self.primitive("quote", MACRO_SPECIAL, QUOTE)?;
        self.primitive("#@", MACRO_SPECIAL, MACRO_PREFIX)?;
        self.primitive("@", MACRO_SPECIAL, MACRO_AT)?;
        self.primitive("@#", MACRO_SPECIAL, MACRO_SUFFIX)?;
        self.primitive("expr", PARAM_TYPE, EXPR_BASE)?;
        self.primitive("suffix", PARAM_TYPE, SUFFIX_BASE)?;
        self.primitive("text", PARAM_TYPE, TEXT_BASE)?;
        self.primitive("primary", PARAM_TYPE, PRIMARY_MACRO)?;
        self.primitive("secondary", PARAM_TYPE, SECONDARY_MACRO)?;
        self.primitive("tertiary", PARAM_TYPE, TERTIARY_MACRO)?;
        self.primitive("input", INPUT, 0)?;
        self.primitive("endinput", INPUT, 1)?;
        self.primitive("if", IF_TEST, IF_CODE)?;
        self.primitive("fi", FI_OR_ELSE, FI_CODE)?;
        self.eqtb[FROZEN_FI as usize] = self.eqtb[self.cur_sym as usize];
        self.primitive("else", FI_OR_ELSE, ELSE_CODE)?;
        self.primitive("elseif", FI_OR_ELSE, ELSE_IF_CODE)?;
        self.primitive("true", NULLARY, TRUE_CODE)?;
        self.primitive("false", NULLARY, FALSE_CODE)?;
        self.primitive("nullpicture", NULLARY, NULL_PICTURE_CODE)?;
        self.primitive("nullpen", NULLARY, NULL_PEN_CODE)?;
        self.primitive("jobname", NULLARY, JOB_NAME_OP)?;
        self.primitive("readstring", NULLARY, READ_STRING_OP)?;
        self.primitive("pencircle", NULLARY, PEN_CIRCLE)?;
        self.primitive("normaldeviate", NULLARY, NORMAL_DEVIATE)?;
        self.primitive("odd", UNARY, ODD_OP)?;
        self.primitive("known", UNARY, KNOWN_OP)?;
        self.primitive("unknown", UNARY, UNKNOWN_OP)?;
        self.primitive("not", UNARY, NOT_OP)?;
        self.primitive("decimal", UNARY, DECIMAL)?;
        self.primitive("reverse", UNARY, REVERSE)?;
        self.primitive("makepath", UNARY, MAKE_PATH_OP)?;
        self.primitive("makepen", UNARY, MAKE_PEN_OP)?;
        self.primitive("totalweight", UNARY, TOTAL_WEIGHT_OP)?;
        self.primitive("oct", UNARY, OCT_OP)?;
        self.primitive("hex", UNARY, HEX_OP)?;
        self.primitive("ASCII", UNARY, ASCII_OP)?;
        self.primitive("char", UNARY, CHAR_OP)?;
        self.primitive("length", UNARY, LENGTH_OP)?;
        self.primitive("turningnumber", UNARY, TURNING_OP)?;
        self.primitive("xpart", UNARY, X_PART)?;
        self.primitive("ypart", UNARY, Y_PART)?;
        self.primitive("xxpart", UNARY, XX_PART)?;
        self.primitive("xypart", UNARY, XY_PART)?;
        self.primitive("yxpart", UNARY, YX_PART)?;
        self.primitive("yypart", UNARY, YY_PART)?;
        self.primitive("sqrt", UNARY, SQRT_OP)?;
        self.primitive("mexp", UNARY, M_EXP_OP)?;
        self.primitive("mlog", UNARY, M_LOG_OP)?;
        self.primitive("sind", UNARY, SIN_D_OP)?;
        self.primitive("cosd", UNARY, COS_D_OP)?;
        self.primitive("floor", UNARY, FLOOR_OP)?;
        self.primitive("uniformdeviate", UNARY, UNIFORM_DEVIATE)?;
        self.primitive("charexists", UNARY, CHAR_EXISTS_OP)?;
        self.primitive("angle", UNARY, ANGLE_OP)?;
        self.primitive("cycle", CYCLE, CYCLE_OP)?;
        self.primitive("+", PLUS_OR_MINUS, PLUS)?;
        self.primitive("-", PLUS_OR_MINUS, MINUS)?;
        self.primitive("*", SECONDARY_BINARY, TIMES)?;
        self.primitive("/", SLASH, OVER)?;
        self.eqtb[FROZEN_SLASH as usize] = self.eqtb[self.cur_sym as usize];
        self.primitive("++", TERTIARY_BINARY, PYTHAG_ADD)?;
        self.primitive("+-+", TERTIARY_BINARY, PYTHAG_SUB)?;
        self.primitive("and", AND_COMMAND, AND_OP)?;
        self.primitive("or", TERTIARY_BINARY, OR_OP)?;
        self.primitive("<", EXPRESSION_BINARY, LESS_THAN)?;
        self.primitive("<=", EXPRESSION_BINARY, LESS_OR_EQUAL)?;
        self.primitive(">", EXPRESSION_BINARY, GREATER_THAN)?;
        self.primitive(">=", EXPRESSION_BINARY, GREATER_OR_EQUAL)?;
        self.primitive("=", EQUALS, EQUAL_TO)?;
        self.primitive("<>", EXPRESSION_BINARY, UNEQUAL_TO)?;
        self.primitive("substring", PRIMARY_BINARY, SUBSTRING_OF)?;
        self.primitive("subpath", PRIMARY_BINARY, SUBPATH_OF)?;
        self.primitive("directiontime", PRIMARY_BINARY, DIRECTION_TIME_OF)?;
        self.primitive("point", PRIMARY_BINARY, POINT_OF)?;
        self.primitive("precontrol", PRIMARY_BINARY, PRECONTROL_OF)?;
        self.primitive("postcontrol", PRIMARY_BINARY, POSTCONTROL_OF)?;
        self.primitive("penoffset", PRIMARY_BINARY, PEN_OFFSET_OF)?;
        self.primitive("&", AMPERSAND, CONCATENATE)?;
        self.primitive("rotated", SECONDARY_BINARY, ROTATED_BY)?;
        self.primitive("slanted", SECONDARY_BINARY, SLANTED_BY)?;
        self.primitive("scaled", SECONDARY_BINARY, SCALED_BY)?;
        self.primitive("shifted", SECONDARY_BINARY, SHIFTED_BY)?;
        self.primitive("transformed", SECONDARY_BINARY, TRANSFORMED_BY)?;
        self.primitive("xscaled", SECONDARY_BINARY, X_SCALED)?;
        self.primitive("yscaled", SECONDARY_BINARY, Y_SCALED)?;
        self.primitive("zscaled", SECONDARY_BINARY, Z_SCALED)?;
        self.primitive("intersectiontimes", TERTIARY_BINARY, INTERSECT)?;
        self.primitive("numeric", TYPE_NAME, NUMERIC_TYPE)?;
        self.primitive("string", TYPE_NAME, STRING_TYPE)?;
        self.primitive("boolean", TYPE_NAME, BOOLEAN_TYPE)?;
        self.primitive("path", TYPE_NAME, PATH_TYPE)?;
        self.primitive("pen", TYPE_NAME, PEN_TYPE)?;
        self.primitive("picture", TYPE_NAME, PICTURE_TYPE)?;
        self.primitive("transform", TYPE_NAME, TRANSFORM_TYPE)?;
        self.primitive("pair", TYPE_NAME, PAIR_TYPE)?;
        self.primitive("end", STOP, 0)?;
        self.primitive("dump", STOP, 1)?;
        self.primitive("batchmode", MODE_COMMAND, BATCH_MODE)?;
        self.primitive("nonstopmode", MODE_COMMAND, NONSTOP_MODE)?;
        self.primitive("scrollmode", MODE_COMMAND, SCROLL_MODE)?;
        self.primitive("errorstopmode", MODE_COMMAND, ERROR_STOP_MODE)?;
        self.primitive("inner", PROTECTION_COMMAND, 0)?;
        self.primitive("outer", PROTECTION_COMMAND, 1)?;
        self.primitive("showtoken", SHOW_COMMAND, SHOW_TOKEN_CODE)?;
        self.primitive("showstats", SHOW_COMMAND, SHOW_STATS_CODE)?;
        self.primitive("show", SHOW_COMMAND, SHOW_CODE)?;
        self.primitive("showvariable", SHOW_COMMAND, SHOW_VAR_CODE)?;
        self.primitive("showdependencies", SHOW_COMMAND, SHOW_DEPENDENCIES_CODE)?;
        self.primitive("contour", THING_TO_ADD, CONTOUR_CODE)?;
        self.primitive("doublepath", THING_TO_ADD, DOUBLE_PATH_CODE)?;
        self.primitive("also", THING_TO_ADD, ALSO_CODE)?;
        self.primitive("withpen", WITH_OPTION, PEN_TYPE)?;
        self.primitive("withweight", WITH_OPTION, KNOWN)?;
        self.primitive("dropping", CULL_OP, DROP_CODE)?;
        self.primitive("keeping", CULL_OP, KEEP_CODE)?;
        self.primitive("message", MESSAGE_COMMAND, MESSAGE_CODE)?;
        self.primitive("errmessage", MESSAGE_COMMAND, ERR_MESSAGE_CODE)?;
        self.primitive("errhelp", MESSAGE_COMMAND, ERR_HELP_CODE)?;
        self.primitive("charlist", TFM_COMMAND, CHAR_LIST_CODE)?;
        self.primitive("ligtable", TFM_COMMAND, LIG_TABLE_CODE)?;
        self.primitive("extensible", TFM_COMMAND, EXTENSIBLE_CODE)?;
        self.primitive("headerbyte", TFM_COMMAND, HEADER_BYTE_CODE)?;
        self.primitive("fontdimen", TFM_COMMAND, FONT_DIMEN_CODE)?;
        self.primitive("=:", LIG_KERN_TOKEN, 0)?;
        self.primitive("=:|", LIG_KERN_TOKEN, 1)?;
        self.primitive("=:|>", LIG_KERN_TOKEN, 5)?;
        self.primitive("|=:", LIG_KERN_TOKEN, 2)?;
        self.primitive("|=:>", LIG_KERN_TOKEN, 6)?;
        self.primitive("|=:|", LIG_KERN_TOKEN, 3)?;
        self.primitive("|=:|>", LIG_KERN_TOKEN, 7)?;
        self.primitive("|=:|>>", LIG_KERN_TOKEN, 11)?;
        self.primitive("kern", LIG_KERN_TOKEN, 128)?;
        self.primitive("special", SPECIAL_COMMAND, STRING_TYPE)?;
        self.primitive("numspecial", SPECIAL_COMMAND, KNOWN)?;
        Ok(())
    }

    // §625: `print_cmd_mod`, its cases from all over `mf.web`.
    pub(crate) fn print_cmd_mod(&mut self, c: i32, m: i32) -> R {
        match c {
            ADD_TO_COMMAND => self.prints("addto"),
            ASSIGNMENT => self.prints(":="),
            AT_LEAST => self.prints("atleast"),
            AT_TOKEN => self.prints("at"),
            BCHAR_LABEL => self.prints("||:"),
            BEGIN_GROUP => self.prints("begingroup"),
            COLON => self.prints(":"),
            COMMA => self.prints(","),
            CONTROLS => self.prints("controls"),
            CULL_COMMAND => self.prints("cull"),
            CURL_COMMAND => self.prints("curl"),
            DELIMITERS => self.prints("delimiters"),
            DISPLAY_COMMAND => self.prints("display"),
            DOUBLE_COLON => self.prints("::"),
            END_GROUP => self.prints("endgroup"),
            EVERY_JOB_COMMAND => self.prints("everyjob"),
            EXIT_TEST => self.prints("exitif"),
            EXPAND_AFTER => self.prints("expandafter"),
            FROM_TOKEN => self.prints("from"),
            IN_WINDOW => self.prints("inwindow"),
            INTERIM_COMMAND => self.prints("interim"),
            LEFT_BRACE => self.prints("{"),
            LEFT_BRACKET => self.prints("["),
            LET_COMMAND => self.prints("let"),
            NEW_INTERNAL => self.prints("newinternal"),
            OF_TOKEN => self.prints("of"),
            OPEN_WINDOW => self.prints("openwindow"),
            PATH_JOIN => self.prints(".."),
            RANDOM_SEED => self.prints("randomseed"),
            RELAX => self.print_char(b'\\'),
            RIGHT_BRACE => self.prints("}"),
            RIGHT_BRACKET => self.prints("]"),
            SAVE_COMMAND => self.prints("save"),
            SCAN_TOKENS => self.prints("scantokens"),
            SEMICOLON => self.prints(";"),
            SHIP_OUT_COMMAND => self.prints("shipout"),
            SKIP_TO => self.prints("skipto"),
            STEP_TOKEN => self.prints("step"),
            STR_OP => self.prints("str"),
            TENSION => self.prints("tension"),
            TO_TOKEN => self.prints("to"),
            UNTIL_TOKEN => self.prints("until"),
            MACRO_DEF => {
                if m <= VAR_DEF {
                    if m == START_DEF {
                        self.prints("def");
                    } else if m < START_DEF {
                        self.prints("enddef");
                    } else {
                        self.prints("vardef");
                    }
                } else if m == SECONDARY_PRIMARY_MACRO {
                    self.prints("primarydef");
                } else if m == TERTIARY_SECONDARY_MACRO {
                    self.prints("secondarydef");
                } else {
                    self.prints("tertiarydef");
                }
            }
            ITERATION => {
                if m <= START_FOREVER {
                    if m == START_FOREVER {
                        self.prints("forever");
                    } else {
                        self.prints("endfor");
                    }
                } else if m == EXPR_BASE {
                    self.prints("for");
                } else {
                    self.prints("forsuffixes");
                }
            }
            MACRO_SPECIAL => match m {
                MACRO_PREFIX => self.prints("#@"),
                MACRO_AT => self.print_char(b'@'),
                MACRO_SUFFIX => self.prints("@#"),
                _ => self.prints("quote"),
            },
            PARAM_TYPE => {
                if m >= EXPR_BASE {
                    if m == EXPR_BASE {
                        self.prints("expr");
                    } else if m == SUFFIX_BASE {
                        self.prints("suffix");
                    } else {
                        self.prints("text");
                    }
                } else if m < SECONDARY_MACRO {
                    self.prints("primary");
                } else if m == SECONDARY_MACRO {
                    self.prints("secondary");
                } else {
                    self.prints("tertiary");
                }
            }
            INPUT => {
                if m == 0 {
                    self.prints("input");
                } else {
                    self.prints("endinput");
                }
            }
            IF_TEST | FI_OR_ELSE => match m {
                IF_CODE => self.prints("if"),
                FI_CODE => self.prints("fi"),
                ELSE_CODE => self.prints("else"),
                _ => self.prints("elseif"),
            },
            NULLARY | UNARY | PRIMARY_BINARY | SECONDARY_BINARY | TERTIARY_BINARY
            | EXPRESSION_BINARY | CYCLE | PLUS_OR_MINUS | SLASH | AMPERSAND | EQUALS
            | AND_COMMAND => self.print_op(m),
            TYPE_NAME => self.print_type(m),
            STOP => {
                if m == 0 {
                    self.prints("end");
                } else {
                    self.prints("dump");
                }
            }
            MODE_COMMAND => match m {
                BATCH_MODE => self.prints("batchmode"),
                NONSTOP_MODE => self.prints("nonstopmode"),
                SCROLL_MODE => self.prints("scrollmode"),
                _ => self.prints("errorstopmode"),
            },
            PROTECTION_COMMAND => {
                if m == 0 {
                    self.prints("inner");
                } else {
                    self.prints("outer");
                }
            }
            SHOW_COMMAND => match m {
                SHOW_TOKEN_CODE => self.prints("showtoken"),
                SHOW_STATS_CODE => self.prints("showstats"),
                SHOW_CODE => self.prints("show"),
                SHOW_VAR_CODE => self.prints("showvariable"),
                _ => self.prints("showdependencies"),
            },
            LEFT_DELIMITER | RIGHT_DELIMITER => {
                if c == LEFT_DELIMITER {
                    self.prints("lef");
                } else {
                    self.prints("righ");
                }
                self.prints("t delimiter that matches ");
                self.slow_print(self.text(m));
            }
            TAG_TOKEN => {
                if m == NULL {
                    self.prints("tag");
                } else {
                    self.prints("variable");
                }
            }
            DEFINED_MACRO => self.prints("macro:"),
            SECONDARY_PRIMARY_MACRO | TERTIARY_SECONDARY_MACRO | EXPRESSION_TERTIARY_MACRO => {
                self.print_cmd_mod(MACRO_DEF, c)?;
                self.prints("'d macro:");
                self.print_ln();
                let l = self.link(self.link(m));
                self.show_token_list(l, NULL, 1000, 0)?;
            }
            REPEAT_LOOP => self.prints("[repeat the loop]"),
            INTERNAL_QUANTITY => self.slow_print(self.int_name[m as usize]),
            THING_TO_ADD => {
                if m == CONTOUR_CODE {
                    self.prints("contour");
                } else if m == DOUBLE_PATH_CODE {
                    self.prints("doublepath");
                } else {
                    self.prints("also");
                }
            }
            WITH_OPTION => {
                if m == PEN_TYPE {
                    self.prints("withpen");
                } else {
                    self.prints("withweight");
                }
            }
            CULL_OP => {
                if m == DROP_CODE {
                    self.prints("dropping");
                } else {
                    self.prints("keeping");
                }
            }
            MESSAGE_COMMAND => {
                if m < ERR_MESSAGE_CODE {
                    self.prints("message");
                } else if m == ERR_MESSAGE_CODE {
                    self.prints("errmessage");
                } else {
                    self.prints("errhelp");
                }
            }
            TFM_COMMAND => match m {
                0 => self.prints("charlist"),
                LIG_TABLE_CODE => self.prints("ligtable"),
                EXTENSIBLE_CODE => self.prints("extensible"),
                HEADER_BYTE_CODE => self.prints("headerbyte"),
                _ => self.prints("fontdimen"),
            },
            LIG_KERN_TOKEN => match m {
                0 => self.prints("=:"),
                1 => self.prints("=:|"),
                2 => self.prints("|=:"),
                3 => self.prints("|=:|"),
                5 => self.prints("=:|>"),
                6 => self.prints("|=:>"),
                7 => self.prints("|=:|>"),
                11 => self.prints("|=:|>>"),
                _ => self.prints("kern"),
            },
            SPECIAL_COMMAND => {
                if m == KNOWN {
                    self.prints("numspecial");
                } else {
                    self.prints("special");
                }
            }
            _ => self.prints("[unknown command code!]"),
        }
        Ok(())
    }
}
