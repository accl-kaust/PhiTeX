//! Part 20: Token lists (§289–§296) and part 21: Introduction to the
//! syntactic routines (§297–§299), with every "Cases of `print_cmd_chr`"
//! chunk of the merged source.

use crate::fonts::fx;
use crate::host::Host;
use crate::mem::NULL;
use crate::nodes::{GLUE_NODE, HEIGHT_OFFSET, KERN_NODE, NORMAL, WIDTH_OFFSET};
use crate::tex::Tex;
use crate::track::Tracker;
use crate::web::*;
use partex_engine::node::Tokens;

impl<H: Host, T: Tracker> Tex<H, T> {
    /// §316: `set_trick_count`, the "magic computation" of §320.
    pub(crate) fn set_trick_count(&mut self) {
        self.first_count = self.tally;
        self.trick_count = self.tally + 1 + self.params.error_line - self.params.half_error_line;
        if self.trick_count < self.params.error_line {
            self.trick_count = self.params.error_line;
        }
    }

    /// §292: display token list `p` up to `l` characters; `q` is the index
    /// where the second line of `show_context` starts (`NULL` for none).
    /// (tex.web's `null` list, and its "CLOBBERED" test, have no place:
    /// a list is a value, and a missing one is shown by the caller.)
    pub(crate) fn show_token_list(&mut self, p: &[i32], q: i32, l: i32) {
        self.tally = 0;
        let len = i32::try_from(p.len()).unwrap_or(i32::MAX);
        self.show_tokens(len, |_, i| p[usize::try_from(i).unwrap_or(0)], q, l);
    }

    /// §292 for a shared list (a mark or `\write` text).
    pub(crate) fn show_token_slice(&mut self, toks: &Tokens, l: i32) {
        self.show_token_list(toks, NULL, l);
    }

    /// §292: the display loop over `len` tokens, token `i` being `at(i)`.
    fn show_tokens(&mut self, len: i32, at: impl Fn(&Self, i32) -> i32, q: i32, l: i32) {
        let mut match_chr = i32::from(b'#');
        let mut n = b'0';
        let mut i = 0;
        // (a character token shown into a string being made goes in as it
        // is, §59's `print` and §58's `print_char` for `new_string`: what
        // they would read, the selector and `\newlinechar`, decides
        // nothing then; a `\pdfliteral`'s text is made so at each ship)
        let to_string = self.selector() == crate::print::NEW_STRING
            && !self.special_printing
            && !self.message_printing
            && self.diag.as_ref().is_none_or(|d| !d.capturing);
        while i < len && self.tally < l {
            if i == q {
                self.set_trick_count(); // §320: do magic computation
            }
            // §293: display token `p`, and return if there are problems.
            let t = at(self, i);
            if t >= CS_TOKEN_FLAG {
                self.print_cs(t - CS_TOKEN_FLAG);
            } else if t < 0 {
                self.print_esc(b"BAD.");
            } else {
                let m = tok_cmd(t);
                let c = tok_chr(t);
                // §294: display the token (m, c).
                match m {
                    LEFT_BRACE | RIGHT_BRACE | MATH_SHIFT | TAB_MARK | SUP_MARK | SUB_MARK
                    | SPACER | LETTER | OTHER_CHAR
                        if to_string =>
                    {
                        if self.unicode {
                            // (`XeTeX`: UTF-16 units, as `print_char` makes them)
                            if self.pool_ptr < self.pool_size() {
                                self.append_char(crate::input::cu(c));
                            }
                            self.tally += if c >= 0x1_0000 { 2 } else { 1 };
                        } else {
                            if self.pool_ptr < self.pool_size() {
                                self.append_char(u8::try_from(c).unwrap_or(0));
                            }
                            self.tally += 1;
                        }
                    }
                    LEFT_BRACE | RIGHT_BRACE | MATH_SHIFT | TAB_MARK | SUP_MARK | SUB_MARK
                    | SPACER | LETTER | OTHER_CHAR => self.print_chr(c),
                    MAC_PARAM => {
                        self.print_chr(c);
                        self.print_chr(c);
                    }
                    OUT_PARAM => {
                        self.print_chr(match_chr);
                        if c <= 9 {
                            self.print_char(b'0' + u8::try_from(c).unwrap_or(0));
                        } else {
                            self.print_char(b'!');
                            return;
                        }
                    }
                    MATCH => {
                        match_chr = c;
                        self.print_chr(c);
                        n += 1;
                        self.print_char(n);
                        if n > b'9' {
                            return;
                        }
                    }
                    END_MATCH => self.print_str(b"->"),
                    _ => self.print_esc(b"BAD."),
                }
            }
            i += 1;
        }
        if i < len {
            self.print_esc(b"ETC.");
        }
    }

    /// §295: show the tokens of a (reference-counted) list.
    pub(crate) fn token_show(&mut self, p: &[i32]) {
        self.show_token_list(p, NULL, 10_000_000);
    }

    /// §296: display the meaning of `cur_cmd`, `cur_chr`.
    pub(crate) fn print_meaning(&mut self) {
        self.print_cmd_chr(self.cur_cmd, self.cur_chr);
        if self.cur_cmd >= CALL {
            self.print_char(b':');
            self.print_ln();
            if let Some(t) = self.equiv_toks(self.cur_cs).cloned() {
                self.token_show(&t);
            }
        } else if self.cur_cmd == TOP_BOT_MARK && self.cur_chr < MARKS_CODE {
            self.print_char(b':');
            self.print_ln();
            if let Some(m) = self.mark(0, self.cur_chr) {
                self.show_token_slice(&m, 10_000_000);
            }
        }
    }

    /// §298: `chr_cmd`.
    fn chr_cmd(&mut self, s: &[u8], chr_code: i32) {
        self.print_str(s);
        self.print_chr(chr_code);
    }

    /// `XeTeX`'s forms of `print_cmd_chr` where two primitives mean the
    /// same (its `\U…` names) or the meaning is no primitive's. Whether
    /// `cmd`, `chr_code` was one.
    fn print_cmd_chr_xetex(&mut self, cmd: i32, chr_code: i32) -> bool {
        let name: &[u8] = match (cmd, chr_code) {
            (XETEX_DEF_CODE, SF_CODE_BASE) => b"XeTeXcharclass",
            (XETEX_DEF_CODE, MATH_CODE_BASE) => b"Umathcodenum",
            (XETEX_DEF_CODE, c) if c == MATH_CODE_BASE + 1 => b"Umathcode",
            (XETEX_DEF_CODE, DEL_CODE_BASE) => b"Udelcodenum",
            (XETEX_DEF_CODE, _) => b"Udelcode",
            (DELIM_NUM, 1) => b"Udelimiter",
            (MATH_ACCENT, 1) => b"Umathaccent",
            (MATH_CHAR_NUM, 2) => b"Umathchar",
            (MATH_CHAR_NUM, 1) => b"Umathcharnum",
            (RADICAL, 1) => b"Uradical",
            (SHORTHAND_DEF, XETEX_MATH_CHAR_DEF_CODE) => b"Umathchardef",
            (SHORTHAND_DEF, XETEX_MATH_CHAR_NUM_DEF_CODE) => b"Umathcharnumdef",
            (ASSIGN_TOKS, XETEX_INTER_CHAR_LOC) => b"XeTeXinterchartoks",
            _ => return false,
        };
        self.print_esc(name);
        true
    }

    /// §298: symbolic printing of a command code and modifier.
    pub(crate) fn print_cmd_chr(&mut self, cmd: i32, chr_code: i32) {
        let e = |t: &mut Self, s: &[u8]| t.print_esc(s);
        if chr_code >= crate::xregs::EXT_BASE
            && matches!(
                cmd,
                ASSIGN_INT | ASSIGN_DIMEN | ASSIGN_GLUE | ASSIGN_MU_GLUE | ASSIGN_TOKS
            )
        {
            // e-TeX: a register above 255
            let (kind, n) = crate::xregs::ext_reg(chr_code);
            self.print_register_name(kind, n);
            return;
        }
        if self.params.flavor == crate::params::Flavor::XeTeX
            && self.print_cmd_chr_xetex(cmd, chr_code)
        {
            return;
        }
        if self.params.flavor != crate::params::Flavor::Tex {
            // e-TeX's, pdfTeX's and `XeTeX`'s commands print their
            // primitive's name, except for these forms.
            if cmd == IF_TEST && chr_code >= UNLESS_CODE {
                e(self, b"unless");
                return self.print_cmd_chr(cmd, chr_code % UNLESS_CODE);
            }
            if !matches!(
                cmd,
                SET_FONT
                    | ASSIGN_INT
                    | ASSIGN_DIMEN
                    | ASSIGN_GLUE
                    | ASSIGN_MU_GLUE
                    | ASSIGN_TOKS
                    | REGISTER
                    | TOKS_REGISTER
            ) && let Some(s) = self.prim_name(cmd, chr_code)
            {
                return self.print_esc_num(s);
            }
        }
        match cmd {
            LEFT_BRACE => self.chr_cmd(b"begin-group character ", chr_code),
            RIGHT_BRACE => self.chr_cmd(b"end-group character ", chr_code),
            MATH_SHIFT => self.chr_cmd(b"math shift character ", chr_code),
            MAC_PARAM => self.chr_cmd(b"macro parameter character ", chr_code),
            SUP_MARK => self.chr_cmd(b"superscript character ", chr_code),
            SUB_MARK => self.chr_cmd(b"subscript character ", chr_code),
            ENDV => self.print_str(b"end of alignment template"),
            SPACER => self.chr_cmd(b"blank space ", chr_code),
            LETTER => self.chr_cmd(b"the letter ", chr_code),
            OTHER_CHAR => self.chr_cmd(b"the character ", chr_code),
            // §227
            ASSIGN_GLUE | ASSIGN_MU_GLUE => {
                if chr_code < SKIP_BASE {
                    self.print_skip_param(chr_code - GLUE_BASE);
                } else if chr_code < MU_SKIP_BASE {
                    e(self, b"skip");
                    self.print_int(chr_code - SKIP_BASE);
                } else {
                    e(self, b"muskip");
                    self.print_int(chr_code - MU_SKIP_BASE);
                }
            }
            // §231
            ASSIGN_TOKS => {
                if chr_code >= TOKS_BASE {
                    e(self, b"toks");
                    self.print_int(chr_code - TOKS_BASE);
                } else {
                    e(
                        self,
                        match chr_code {
                            OUTPUT_ROUTINE_LOC => b"output",
                            EVERY_PAR_LOC => b"everypar",
                            EVERY_MATH_LOC => b"everymath",
                            EVERY_DISPLAY_LOC => b"everydisplay",
                            EVERY_HBOX_LOC => b"everyhbox",
                            EVERY_VBOX_LOC => b"everyvbox",
                            EVERY_JOB_LOC => b"everyjob",
                            EVERY_CR_LOC => b"everycr",
                            // pdfTeX §249
                            EVERY_EOF_LOC => b"everyeof",
                            PDF_PAGES_ATTR_LOC => b"pdfpagesattr",
                            PDF_PAGE_ATTR_LOC => b"pdfpageattr",
                            PDF_PAGE_RESOURCES_LOC => b"pdfpageresources",
                            PDF_PK_MODE_LOC => b"pdfpkmode",
                            _ => b"errhelp",
                        },
                    );
                }
            }
            // §239
            ASSIGN_INT => {
                if chr_code < COUNT_BASE {
                    self.print_param(chr_code - INT_BASE);
                } else {
                    e(self, b"count");
                    self.print_int(chr_code - COUNT_BASE);
                }
            }
            // §249
            ASSIGN_DIMEN => {
                if chr_code < SCALED_BASE {
                    self.print_length_param(chr_code - DIMEN_BASE);
                } else {
                    e(self, b"dimen");
                    self.print_int(chr_code - SCALED_BASE);
                }
            }
            // §266
            ACCENT => e(self, b"accent"),
            ADVANCE => e(self, b"advance"),
            AFTER_ASSIGNMENT => e(self, b"afterassignment"),
            AFTER_GROUP => e(self, b"aftergroup"),
            ASSIGN_FONT_DIMEN => e(self, b"fontdimen"),
            BEGIN_GROUP => e(self, b"begingroup"),
            BREAK_PENALTY => e(self, b"penalty"),
            CHAR_NUM => e(self, b"char"),
            CS_NAME => e(self, b"csname"),
            DEF_FONT => e(self, b"font"),
            DELIM_NUM => e(self, b"delimiter"),
            DIVIDE => e(self, b"divide"),
            END_CS_NAME => {
                if chr_code == 10 {
                    e(self, b"endmubyte");
                } else {
                    e(self, b"endcsname");
                }
            }
            END_GROUP => e(self, b"endgroup"),
            EX_SPACE => e(self, b" "),
            EXPAND_AFTER => e(self, b"expandafter"),
            HALIGN => e(self, b"halign"),
            HRULE => e(self, b"hrule"),
            IGNORE_SPACES => e(self, b"ignorespaces"),
            INSERT => e(self, b"insert"),
            ITAL_CORR => e(self, b"/"),
            MARK => e(self, b"mark"),
            MATH_ACCENT => e(self, b"mathaccent"),
            MATH_CHAR_NUM => e(self, b"mathchar"),
            MATH_CHOICE => e(self, b"mathchoice"),
            MULTIPLY => e(self, b"multiply"),
            NO_ALIGN => e(self, b"noalign"),
            NO_BOUNDARY => e(self, b"noboundary"),
            NO_EXPAND => e(self, b"noexpand"),
            NON_SCRIPT => e(self, b"nonscript"),
            OMIT => e(self, b"omit"),
            RADICAL => e(self, b"radical"),
            READ_TO_CS => e(self, b"read"),
            RELAX => e(self, b"relax"),
            SET_BOX => e(self, b"setbox"),
            SET_PREV_GRAF => e(self, b"prevgraf"),
            SET_SHAPE => e(
                self,
                match chr_code {
                    // pdfTeX §1860
                    INTER_LINE_PENALTIES_LOC => b"interlinepenalties",
                    CLUB_PENALTIES_LOC => b"clubpenalties",
                    WIDOW_PENALTIES_LOC => b"widowpenalties",
                    DISPLAY_WIDOW_PENALTIES_LOC => b"displaywidowpenalties",
                    _ => b"parshape",
                },
            ),
            THE => e(self, b"the"),
            TOKS_REGISTER => e(self, b"toks"),
            VADJUST => e(self, b"vadjust"),
            VALIGN => e(self, b"valign"),
            VCENTER => e(self, b"vcenter"),
            VRULE => e(self, b"vrule"),
            // §335
            PAR_END => e(self, b"par"),
            // §377
            INPUT => e(self, if chr_code == 0 { b"input" } else { b"endinput" }),
            // §385
            TOP_BOT_MARK => e(
                self,
                match chr_code {
                    FIRST_MARK_CODE => b"firstmark",
                    BOT_MARK_CODE => b"botmark",
                    SPLIT_FIRST_MARK_CODE => b"splitfirstmark",
                    SPLIT_BOT_MARK_CODE => b"splitbotmark",
                    _ => b"topmark",
                },
            ),
            // §412
            REGISTER => e(
                self,
                match chr_code {
                    INT_VAL => b"count",
                    DIMEN_VAL => b"dimen",
                    GLUE_VAL => b"skip",
                    _ => b"muskip",
                },
            ),
            // §417
            SET_AUX => e(
                self,
                if chr_code == VMODE {
                    b"prevdepth"
                } else {
                    b"spacefactor"
                },
            ),
            SET_PAGE_INT => e(
                self,
                match chr_code {
                    0 => b"deadcycles",
                    1 => b"insertpenalties",
                    _ => b"interactionmode",
                },
            ),
            SET_BOX_DIMEN => e(
                self,
                match chr_code {
                    WIDTH_OFFSET => b"wd",
                    HEIGHT_OFFSET => b"ht",
                    _ => b"dp",
                },
            ),
            LAST_ITEM => e(
                self,
                match chr_code {
                    INT_VAL => b"lastpenalty",
                    DIMEN_VAL => b"lastkern",
                    GLUE_VAL => b"lastskip",
                    INPUT_LINE_NO_CODE => b"inputlineno",
                    _ => b"badness",
                },
            ),
            // §469
            CONVERT => e(
                self,
                match chr_code {
                    NUMBER_CODE => b"number",
                    ROMAN_NUMERAL_CODE => b"romannumeral",
                    STRING_CODE => b"string",
                    MEANING_CODE => b"meaning",
                    FONT_NAME_CODE => b"fontname",
                    _ => b"jobname",
                },
            ),
            // §488
            IF_TEST => e(
                self,
                match chr_code {
                    IF_CAT_CODE => b"ifcat",
                    IF_INT_CODE => b"ifnum",
                    IF_DIM_CODE => b"ifdim",
                    IF_ODD_CODE => b"ifodd",
                    IF_VMODE_CODE => b"ifvmode",
                    IF_HMODE_CODE => b"ifhmode",
                    IF_MMODE_CODE => b"ifmmode",
                    IF_INNER_CODE => b"ifinner",
                    IF_VOID_CODE => b"ifvoid",
                    IF_HBOX_CODE => b"ifhbox",
                    IF_VBOX_CODE => b"ifvbox",
                    IFX_CODE => b"ifx",
                    IF_EOF_CODE => b"ifeof",
                    IF_TRUE_CODE => b"iftrue",
                    IF_FALSE_CODE => b"iffalse",
                    IF_CASE_CODE => b"ifcase",
                    _ => b"if",
                },
            ),
            // §492
            FI_OR_ELSE => e(
                self,
                match chr_code {
                    FI_CODE => b"fi",
                    OR_CODE => b"or",
                    _ => b"else",
                },
            ),
            // §781
            TAB_MARK => {
                if chr_code == SPAN_CODE {
                    e(self, b"span");
                } else {
                    self.chr_cmd(b"alignment tab character ", chr_code);
                }
            }
            CAR_RET => e(self, if chr_code == CR_CODE { b"cr" } else { b"crcr" }),
            // §984
            SET_PAGE_DIMEN => e(
                self,
                match chr_code {
                    0 => b"pagegoal",
                    1 => b"pagetotal",
                    2 => b"pagestretch",
                    3 => b"pagefilstretch",
                    4 => b"pagefillstretch",
                    5 => b"pagefilllstretch",
                    6 => b"pageshrink",
                    _ => b"pagedepth",
                },
            ),
            // §1053
            STOP => e(self, if chr_code == 1 { b"dump" } else { b"end" }),
            // §1059
            HSKIP => e(
                self,
                match chr_code {
                    SKIP_CODE => b"hskip",
                    FIL_CODE => b"hfil",
                    FILL_CODE => b"hfill",
                    SS_CODE => b"hss",
                    _ => b"hfilneg",
                },
            ),
            VSKIP => e(
                self,
                match chr_code {
                    SKIP_CODE => b"vskip",
                    FIL_CODE => b"vfil",
                    FILL_CODE => b"vfill",
                    SS_CODE => b"vss",
                    _ => b"vfilneg",
                },
            ),
            MSKIP => e(self, b"mskip"),
            KERN => e(self, b"kern"),
            MKERN => e(self, b"mkern"),
            // §1072
            HMOVE => e(
                self,
                if chr_code == 1 {
                    b"moveleft"
                } else {
                    b"moveright"
                },
            ),
            VMOVE => e(self, if chr_code == 1 { b"raise" } else { b"lower" }),
            MAKE_BOX => {
                let s: &[u8] = if chr_code == BOX_CODE {
                    b"box"
                } else if chr_code == COPY_CODE {
                    b"copy"
                } else if chr_code == LAST_BOX_CODE {
                    b"lastbox"
                } else if chr_code == VSPLIT_CODE {
                    b"vsplit"
                } else if chr_code == VTOP_CODE {
                    b"vtop"
                } else if chr_code == VTOP_CODE + VMODE {
                    b"vbox"
                } else {
                    b"hbox"
                };
                e(self, s);
            }
            LEADER_SHIP => e(
                self,
                match chr_code {
                    crate::nodes::A_LEADERS => b"leaders",
                    crate::nodes::C_LEADERS => b"cleaders",
                    crate::nodes::X_LEADERS => b"xleaders",
                    _ => b"shipout",
                },
            ),
            // §1089
            START_PAR => e(
                self,
                if chr_code == 0 {
                    b"noindent"
                } else {
                    b"indent"
                },
            ),
            // §1108
            REMOVE_ITEM => e(
                self,
                match chr_code {
                    GLUE_NODE => b"unskip",
                    KERN_NODE => b"unkern",
                    _ => b"unpenalty",
                },
            ),
            UN_HBOX => e(
                self,
                if chr_code == COPY_CODE {
                    b"unhcopy"
                } else {
                    b"unhbox"
                },
            ),
            UN_VBOX => e(
                self,
                if chr_code == COPY_CODE {
                    b"unvcopy"
                } else {
                    b"unvbox"
                },
            ),
            // §1115
            DISCRETIONARY => e(
                self,
                if chr_code == 1 {
                    b"-"
                } else {
                    b"discretionary"
                },
            ),
            // §1143
            EQ_NO => e(self, if chr_code == 1 { b"leqno" } else { b"eqno" }),
            // §1157
            MATH_COMP => e(
                self,
                match chr_code {
                    ORD_NOAD => b"mathord",
                    OP_NOAD => b"mathop",
                    BIN_NOAD => b"mathbin",
                    REL_NOAD => b"mathrel",
                    OPEN_NOAD => b"mathopen",
                    CLOSE_NOAD => b"mathclose",
                    PUNCT_NOAD => b"mathpunct",
                    INNER_NOAD => b"mathinner",
                    UNDER_NOAD => b"underline",
                    _ => b"overline",
                },
            ),
            LIMIT_SWITCH => e(
                self,
                match chr_code {
                    LIMITS => b"limits",
                    NO_LIMITS => b"nolimits",
                    _ => b"displaylimits",
                },
            ),
            // §1170
            MATH_STYLE => self.print_style(chr_code),
            // §1179
            ABOVE => {
                let s: &[u8] = if chr_code == OVER_CODE {
                    b"over"
                } else if chr_code == ATOP_CODE {
                    b"atop"
                } else if chr_code == DELIMITED_CODE + ABOVE_CODE {
                    b"abovewithdelims"
                } else if chr_code == DELIMITED_CODE + OVER_CODE {
                    b"overwithdelims"
                } else if chr_code == DELIMITED_CODE + ATOP_CODE {
                    b"atopwithdelims"
                } else {
                    b"above"
                };
                e(self, s);
            }
            // §1189
            LEFT_RIGHT => e(
                self,
                if chr_code == LEFT_NOAD {
                    b"left"
                } else {
                    b"right"
                },
            ),
            // §1209
            PREFIX => e(
                self,
                match chr_code {
                    1 => b"long",
                    2 => b"outer",
                    _ => b"global",
                },
            ),
            DEF => e(
                self,
                match chr_code {
                    0 => b"def",
                    1 => b"gdef",
                    2 => b"edef",
                    _ => b"xdef",
                },
            ),
            // §1220 (with encTeX's \mubyte and \noconvert)
            LET => {
                let s: &[u8] = if chr_code == NORMAL {
                    b"let"
                } else if chr_code == NORMAL + 10 {
                    b"mubyte"
                } else if chr_code == NORMAL + 11 {
                    b"noconvert"
                } else {
                    b"futurelet"
                };
                e(self, s);
            }
            // §1223
            SHORTHAND_DEF => e(
                self,
                match chr_code {
                    CHAR_DEF_CODE => b"chardef",
                    MATH_CHAR_DEF_CODE => b"mathchardef",
                    COUNT_DEF_CODE => b"countdef",
                    DIMEN_DEF_CODE => b"dimendef",
                    SKIP_DEF_CODE => b"skipdef",
                    MU_SKIP_DEF_CODE => b"muskipdef",
                    CHAR_SUB_DEF_CODE => b"charsubdef",
                    _ => b"toksdef",
                },
            ),
            CHAR_GIVEN => {
                e(self, b"char");
                self.print_hex(chr_code);
            }
            MATH_GIVEN => {
                e(self, b"mathchar");
                self.print_hex(chr_code);
            }
            // `XeTeX` §1277
            XETEX_MATH_GIVEN => {
                use crate::mathcodes::{math_char_field, math_class_field, math_fam_field};
                e(self, b"Umathchar");
                self.print_hex(math_class_field(chr_code));
                self.print_hex(math_fam_field(chr_code));
                self.print_hex(math_char_field(chr_code));
            }
            // §1231
            DEF_CODE => {
                let s: &[u8] = if chr_code == XORD_CODE_BASE {
                    b"xordcode"
                } else if chr_code == XCHR_CODE_BASE {
                    b"xchrcode"
                } else if chr_code == XPRN_CODE_BASE {
                    b"xprncode"
                } else if chr_code == CAT_CODE_BASE {
                    b"catcode"
                } else if chr_code == MATH_CODE_BASE {
                    b"mathcode"
                } else if chr_code == LC_CODE_BASE {
                    b"lccode"
                } else if chr_code == UC_CODE_BASE {
                    b"uccode"
                } else if chr_code == SF_CODE_BASE {
                    b"sfcode"
                } else {
                    b"delcode"
                };
                e(self, s);
            }
            DEF_FAMILY => self.print_size(chr_code - MATH_FONT_BASE),
            // §1251
            HYPH_DATA => e(
                self,
                if chr_code == 1 {
                    b"patterns"
                } else {
                    b"hyphenation"
                },
            ),
            // §1255
            ASSIGN_FONT_INT => e(
                self,
                if chr_code == 0 {
                    b"hyphenchar"
                } else {
                    b"skewchar"
                },
            ),
            // §1261
            SET_FONT => {
                self.print_str(b"select font ");
                self.font_read(chr_code, crate::track::font::METRICS);
                let f = fx(chr_code);
                self.slow_print(self.fonts.name[f]);
                if self.fonts.metrics[f].size != self.fonts.metrics[f].design_size {
                    self.print_str(b" at ");
                    self.print_scaled(self.fonts.metrics[f].size);
                    self.print_str(b"pt");
                }
            }
            // §1263
            SET_INTERACTION => e(
                self,
                match chr_code {
                    crate::error::BATCH_MODE => b"batchmode",
                    crate::error::NONSTOP_MODE => b"nonstopmode",
                    crate::error::SCROLL_MODE => b"scrollmode",
                    _ => b"errorstopmode",
                },
            ),
            // §1273
            IN_STREAM => e(self, if chr_code == 0 { b"closein" } else { b"openin" }),
            // §1278
            MESSAGE => e(
                self,
                if chr_code == 0 {
                    b"message"
                } else {
                    b"errmessage"
                },
            ),
            // §1287
            CASE_SHIFT => e(
                self,
                if chr_code == LC_CODE_BASE {
                    b"lowercase"
                } else {
                    b"uppercase"
                },
            ),
            // §1292
            XRAY => e(
                self,
                match chr_code {
                    SHOW_BOX_CODE => b"showbox",
                    SHOW_THE_CODE => b"showthe",
                    SHOW_LISTS_CODE => b"showlists",
                    _ => b"show",
                },
            ),
            // §1295
            UNDEFINED_CS => self.print_str(b"undefined"),
            CALL | LONG_CALL | OUTER_CALL | LONG_OUTER_CALL => {
                let mut n = cmd - CALL;
                // (a macro's word carries its list's `\protected` flag)
                if chr_code == 1 {
                    n += 4;
                }
                if (n / 4) % 2 == 1 {
                    e(self, b"protected");
                }
                if n % 2 == 1 {
                    e(self, b"long");
                }
                if (n / 2) % 2 == 1 {
                    e(self, b"outer");
                }
                if n > 0 {
                    self.print_char(b' ');
                }
                self.print_str(b"macro");
            }
            END_TEMPLATE => e(self, b"outer endtemplate"),
            // §1346
            EXTENSION => match chr_code {
                OPEN_NODE => e(self, b"openout"),
                WRITE_NODE => e(self, b"write"),
                CLOSE_NODE => e(self, b"closeout"),
                SPECIAL_NODE => e(self, b"special"),
                IMMEDIATE_CODE => e(self, b"immediate"),
                SET_LANGUAGE_CODE => e(self, b"setlanguage"),
                _ => self.print_str(b"[unknown extension!]"),
            },
            _ => self.print_str(b"[unknown command code!]"),
        }
    }

    /// §299: display `cur_cmd` and `cur_chr` in symbolic form, with the
    /// mode if it has changed.
    /// `\count n`, `\dimen n`, … (`kind` is `int_val` … `tok_val`,
    /// `box_val`).
    pub(crate) fn print_register_name(&mut self, kind: i32, n: i32) {
        self.print_esc(match kind {
            INT_VAL => b"count",
            DIMEN_VAL => b"dimen",
            GLUE_VAL => b"skip",
            MU_VAL => b"muskip",
            BOX_VAL => b"box",
            _ => b"toks",
        });
        self.print_int(n);
    }

    pub(crate) fn show_cur_cmd_chr(&mut self) {
        self.begin_diagnostic();
        self.print_nl(b"{");
        if self.mode() != self.shown_mode() {
            self.print_mode(self.mode());
            self.print_str(b": ");
            self.set_shown_mode(self.mode());
        }
        self.print_cmd_chr(self.cur_cmd, self.cur_chr);
        if self.int_par(TRACING_IFS_CODE) > 0 && (IF_TEST..=FI_OR_ELSE).contains(&self.cur_cmd) {
            // e-TeX: the level of conditionals and where this one began
            self.print_str(b": ");
            self.cond_read();
            let depth = i32::try_from(self.cond_stack.len()).unwrap_or(i32::MAX);
            let (n, l) = if self.cur_cmd == FI_OR_ELSE {
                self.print_cmd_chr(IF_TEST, self.cur_if);
                self.print_char(b' ');
                (depth, self.if_line)
            } else {
                (depth + 1, self.line)
            };
            self.print_str(b"(level ");
            self.print_int(n);
            self.print_char(b')');
            self.print_if_line(l);
        }
        self.print_char(b'}');
        self.end_diagnostic(false);
    }
}

#[cfg(test)]
mod tests {
    use crate::testing::{engine, term_output};
    use crate::web::{CS_TOKEN_FLAG, END_MATCH, LETTER, MATCH, MAX_CHAR_VAL, OUT_PARAM};

    /// Oracle: `\message{[\meaning\hskip][\meaning\over]...}` in INITEX and
    /// `\def\a#1#2{x#1y#2}\message{[\meaning\a]}`.
    #[test]
    fn meanings_match_tex() {
        let mut t = engine();
        t.init_prim().unwrap();
        let mut out = alloc::vec::Vec::new();
        for name in [
            &b"hskip"[..],
            b"over",
            b"catcode",
            b"undefined",
            b"relax",
            b"errhelp",
            b"dump",
        ] {
            for (d, &s) in t.buffer[..name.len()].iter_mut().zip(name) {
                *d = u32::from(s);
            }
            let p = t.id_lookup(0, name.len()).unwrap();
            t.cur_cmd = t.eq_type(p);
            t.cur_chr = t.equiv(p);
            out.extend(term_output(&mut t, |t| {
                t.print_char(b'[');
                t.print_meaning();
                t.print_char(b']');
            }));
        }
        assert_eq!(
            core::str::from_utf8(&out).unwrap(),
            "[\\hskip][\\over][\\catcode][undefined][\\relax][\\errhelp][\\dump]"
        );

        // A macro body: ref count, `#1#2->x#1y#2`.
        let rc = t.tok_from(&[
            MATCH * MAX_CHAR_VAL + 35,
            MATCH * MAX_CHAR_VAL + 35,
            END_MATCH * MAX_CHAR_VAL,
            LETTER * MAX_CHAR_VAL + 120,
            OUT_PARAM * MAX_CHAR_VAL + 1,
            LETTER * MAX_CHAR_VAL + 121,
            OUT_PARAM * MAX_CHAR_VAL + 2,
        ]);
        let p = crate::web::SINGLE_BASE + i32::from(b'm');
        let mut w = t.peek_eqtb(p);
        w.set_b0(crate::web::CALL);
        w.set_rh(0);
        t.set_eqtb_entry(p, w, Some(crate::objs::Obj::Toks(rc)));
        t.cur_cs = p;
        t.cur_cmd = crate::web::CALL;
        t.cur_chr = 0;
        let out = term_output(&mut t, |t| {
            t.print_char(b'[');
            t.print_meaning();
            t.print_char(b']');
        });
        assert_eq!(out, b"[macro:\n#1#2->x#1y#2]");
        let _ = CS_TOKEN_FLAG;
    }
}
