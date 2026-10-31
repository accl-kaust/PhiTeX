//! METAFONT, ported from Knuth's `mf.web` (version 2.71828182) with web2c's
//! `mf.ch` as TeX Live builds it (`texk/web2c`; the floating-point fraction
//! routines of `lib/texmfmp.c`, the macros of `cpascal.h`, the `input_line`
//! and `topenin` of `lib/texmfmp.c`, the trap test's screen of
//! `window/trap.c`).
//!
//! The program is one structure, [`Mf`], holding `mf.web`'s global
//! variables; its procedures are methods named as in `mf.web`, in modules
//! by the program's parts. Files are bytes: input files come from an
//! [`Io`] the caller gives, output files (the log, the TFM, the GF, the
//! base) are kept in [`Mf::outputs`]. The terminal is bytes too: its input
//! is given (`Config::terminal_input`, read as web2c reads stdin), its
//! output kept in [`Mf::term_out`]. A procedure that can end the run
//! (`jump_out`: a fatal error, an overflow, a hundred errors) returns
//! `Err(Stop)`, carried out by `?` to [`Mf::run`].

#![allow(
    clippy::pedantic,
    clippy::too_many_arguments,
    clippy::collapsible_if,
    clippy::collapsible_else_if,
    clippy::needless_return,
    clippy::manual_range_contains,
    clippy::nonminimal_bool,
    clippy::if_same_then_else,
    clippy::neg_multiply,
    clippy::assign_op_pattern,
    clippy::comparison_chain,
    clippy::never_loop,
    clippy::while_let_loop,
    clippy::identity_op,
    clippy::precedence,
    clippy::needless_range_loop,
    clippy::too_many_lines,
    clippy::cognitive_complexity,
    clippy::single_match,
    clippy::match_like_matches_macro,
    clippy::bool_comparison,
    clippy::int_plus_one,
    clippy::redundant_field_names,
    clippy::manual_div_ceil,
    clippy::new_without_default,
    clippy::collapsible_match,
    clippy::manual_swap,
    clippy::field_reassign_with_default,
    clippy::unnecessary_cast,
    clippy::needless_late_init,
    clippy::erasing_op,
    clippy::absurd_extreme_comparisons,
    clippy::empty_loop,
    clippy::double_parens,
    clippy::let_and_return,
    clippy::manual_memcpy,
    clippy::same_item_push,
    clippy::explicit_counter_loop,
    clippy::question_mark,
    clippy::len_zero,
    clippy::unusual_byte_groupings,
    clippy::zero_prefixed_literal,
    clippy::mixed_case_hex_literals,
    clippy::type_complexity,
    clippy::upper_case_acronyms
)]

mod arith;
mod cond;
mod consts;
mod edges;
mod exprs;
mod files;
mod gf;
mod hash;
mod input;
mod linear;
mod macros;
mod memory;
mod moves;
mod octants;
mod ops;
mod paths;
mod pens;
mod print;
mod run;
mod scan;
mod scan2;
mod screen;
mod statements;
mod tfm;
mod tokens;

pub use consts::*;

/// `pointer`, `halfword`, `scaled`, `fraction`, `angle`, `str_number`,
/// `pool_pointer`: all integers.
pub type Pointer = i32;
pub type Halfword = i32;
pub type Scaled = i32;
pub type Fraction = i32;
pub type Angle = i32;
pub type StrNumber = i32;

/// The run ended (`jump_out`): the files were closed, the history says how.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stop;

pub(crate) type R<T = ()> = Result<T, Stop>;

/// A `memory_word` as web2c lays it out on a little-endian machine with
/// 32-bit halfwords (`texmfmem.h`): `lh` in the first four bytes, `rh` in
/// the last four; `int` (and `sc`) is `rh`; the `hh.b0` and `hh.b1` fields
/// are the high and low shorts of `lh`.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Word {
    pub lh: i32,
    pub rh: i32,
}

impl Word {
    #[inline]
    pub fn lh(&self) -> i32 {
        self.lh
    }
    #[inline]
    pub fn rh(&self) -> i32 {
        self.rh
    }
    #[inline]
    pub fn set_lh(&mut self, v: i32) {
        self.lh = v;
    }
    #[inline]
    pub fn set_rh(&mut self, v: i32) {
        self.rh = v;
    }
    #[inline]
    pub fn b0(&self) -> i32 {
        i32::from((self.lh >> 16) as i16)
    }
    #[inline]
    pub fn b1(&self) -> i32 {
        i32::from(self.lh as i16)
    }
    #[inline]
    pub fn set_b0(&mut self, v: i32) {
        self.lh = (self.lh & 0xFFFF) | ((v & 0xFFFF) << 16);
    }
    #[inline]
    pub fn set_b1(&mut self, v: i32) {
        self.lh = (self.lh & !0xFFFF) | (v & 0xFFFF);
    }
}

/// A `four_quarters` (the TFM's ligature/kern and extensible words).
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct FourQuarters {
    pub b0: u8,
    pub b1: u8,
    pub b2: u8,
    pub b3: u8,
}

/// The kinds of files the program asks the [`Io`] for (kpathsea's
/// `kpse_mf_format` and `kpse_base_format`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    Mf,
    Base,
}

/// Where input files come from.
pub trait Io {
    /// The file `name` (as `name_of_file` has it) of `kind`, as kpathsea
    /// would find it: its bytes and the name it was found under (the
    /// name `open_input` leaves in `name_of_file`).
    fn open_in(&mut self, name: &[u8], kind: FileKind) -> Option<(Vec<u8>, Vec<u8>)>;
}

/// The run's settings: web2c's command line and `texmf.cnf` values.
#[derive(Clone, Debug)]
pub struct Config {
    /// `ini_version`: INIMF.
    pub ini: bool,
    /// The base's name without `.base` (`dump_name`: the program's name,
    /// or the `&name` on the command line).
    pub dump_name: String,
    /// The command line after the options (`topenin`'s arguments).
    pub command_line: Vec<Vec<u8>>,
    /// What the terminal types: web2c's stdin.
    pub terminal_input: Vec<u8>,
    pub main_memory: i32,
    pub buf_size: i32,
    pub error_line: i32,
    pub half_error_line: i32,
    pub max_print_line: i32,
    pub screen_width: i32,
    pub screen_depth: i32,
    pub gf_buf_size: i32,
    /// `MFTERM=trap`: the trap test's screen (`window/trap.c`); else none.
    pub trap_screen: bool,
    /// `interaction_option` (unspecified: `None`).
    pub interaction_option: Option<u8>,
    /// A TCX translation: (xord, xchr, xprn) after `read_tcx_file`.
    pub translate: Option<(String, Box<[u8; 256]>, Box<[u8; 256]>, Box<[u8; 256]>)>,
    pub eight_bit: bool,
    pub file_line_error_style: bool,
    pub halt_on_error: bool,
    /// The date and time (`date_and_time`): minutes after midnight, day,
    /// month, year.
    pub date: (i32, i32, i32, i32),
    /// web2c's `version_string`, printed after the banner.
    pub version_string: String,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            ini: false,
            dump_name: "mf".into(),
            command_line: Vec::new(),
            terminal_input: Vec::new(),
            main_memory: 5_000_000,
            buf_size: 200_000,
            error_line: 79,
            half_error_line: 50,
            max_print_line: 79,
            screen_width: 768,
            screen_depth: 1024,
            gf_buf_size: 16384,
            trap_screen: false,
            interaction_option: None,
            translate: None,
            eight_bit: false,
            file_line_error_style: false,
            halt_on_error: false,
            date: (12 * 60, 4, 7, 1776),
            version_string: " (TeX Live 2026)".into(),
        }
    }
}

/// An input file being read: its bytes and how far.
#[derive(Clone, Debug, Default)]
pub(crate) struct AlphaFile {
    pub data: Vec<u8>,
    pub pos: usize,
}

/// An entry of the input stack (`in_state_record`).
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct InStateRecord {
    pub index_field: i32,
    pub start_field: i32,
    pub loc_field: i32,
    pub limit_field: i32,
    pub name_field: i32,
}

/// METAFONT's global variables.
pub struct Mf {
    pub(crate) cfg: Config,
    pub(crate) io: Box<dyn Io>,
    /// The terminal's output (`term_out`).
    pub term_out: Vec<u8>,
    /// The output files, by the name each was opened under, in order.
    pub outputs: Vec<(Vec<u8>, Vec<u8>)>,
    pub(crate) term_in: AlphaFile,
    // [1] constants made variables by mf.ch
    pub(crate) mem_top: i32,
    pub(crate) mem_max: i32,
    pub(crate) buf_size: i32,
    pub(crate) error_line: i32,
    pub(crate) half_error_line: i32,
    pub(crate) max_print_line: i32,
    pub(crate) screen_width: i32,
    pub(crate) screen_depth: i32,
    pub(crate) gf_buf_size: i32,
    pub(crate) ini_version: bool,
    pub(crate) bad: i32,
    // [2]
    pub(crate) xord: [u8; 256],
    pub(crate) xchr: [u8; 256],
    pub(crate) xprn: [u8; 256],
    // [3]
    pub(crate) name_of_file: Vec<u8>,
    pub(crate) name_length: i32,
    pub(crate) buffer: Vec<u8>,
    pub(crate) first: i32,
    pub(crate) last: i32,
    pub(crate) max_buf_stack: i32,
    // [4]
    pub(crate) str_pool: Vec<u8>,
    pub(crate) str_start: Vec<i32>,
    pub(crate) pool_ptr: i32,
    pub(crate) str_ptr: i32,
    pub(crate) init_pool_ptr: i32,
    pub(crate) init_str_ptr: i32,
    pub(crate) max_pool_ptr: i32,
    pub(crate) max_str_ptr: i32,
    pub(crate) str_ref: Vec<u8>,
    // [5]
    pub(crate) log_file: Vec<u8>,
    pub(crate) log_file_name: Vec<u8>,
    pub(crate) selector: i32,
    pub(crate) dig: [u8; 23],
    pub(crate) tally: i32,
    pub(crate) term_offset: i32,
    pub(crate) file_offset: i32,
    pub(crate) trick_buf: Vec<u8>,
    pub(crate) trick_count: i32,
    pub(crate) first_count: i32,
    // [6]
    pub(crate) interaction: i32,
    pub(crate) deletions_allowed: bool,
    pub history: i32,
    pub(crate) error_count: i32,
    pub(crate) help_line: [&'static str; 6],
    pub(crate) help_ptr: usize,
    pub(crate) use_err_help: bool,
    pub(crate) err_help: StrNumber,
    pub(crate) interrupt: i32,
    pub(crate) ok_to_interrupt: bool,
    pub(crate) halting_on_error_p: bool,
    // [7]
    pub(crate) arith_error: bool,
    // [8]
    pub(crate) two_to_the: [i32; 31],
    pub(crate) spec_log: [i32; 29],
    pub(crate) spec_atan: [i32; 27],
    pub(crate) n_sin: Fraction,
    pub(crate) n_cos: Fraction,
    pub(crate) randoms: [Fraction; 55],
    pub(crate) j_random: i32,
    // [10]
    pub(crate) mem: Vec<Word>,
    pub(crate) lo_mem_max: Pointer,
    pub(crate) hi_mem_min: Pointer,
    pub(crate) var_used: i32,
    pub(crate) dyn_used: i32,
    pub(crate) avail: Pointer,
    pub(crate) mem_end: Pointer,
    pub(crate) rover: Pointer,
    // [12]
    pub(crate) internal: Vec<Scaled>,
    pub(crate) int_name: Vec<StrNumber>,
    pub(crate) int_ptr: i32,
    pub(crate) old_setting: i32,
    pub(crate) sys_time: i32,
    pub(crate) sys_day: i32,
    pub(crate) sys_month: i32,
    pub(crate) sys_year: i32,
    pub(crate) char_class: [u8; 256],
    // [13]
    pub(crate) hash_used: Pointer,
    pub(crate) st_count: i32,
    pub(crate) hash: Vec<Word>,
    pub(crate) eqtb: Vec<Word>,
    // [14]
    pub(crate) g_pointer: Pointer,
    // [15]
    pub(crate) big_node_size: [i32; 2],
    // [16]
    pub(crate) save_ptr: Pointer,
    // [17]
    pub(crate) path_tail: Pointer,
    // [18]
    pub(crate) delta_x: Vec<Scaled>,
    pub(crate) delta_y: Vec<Scaled>,
    pub(crate) delta: Vec<Scaled>,
    pub(crate) psi: Vec<Angle>,
    pub(crate) theta: Vec<Angle>,
    pub(crate) uu: Vec<Fraction>,
    pub(crate) st: Fraction,
    pub(crate) ct: Fraction,
    pub(crate) sf: Fraction,
    pub(crate) cf: Fraction,
    pub(crate) vv: Vec<Angle>,
    pub(crate) ww: Vec<Fraction>,
    // [19]
    pub(crate) move_: Vec<i32>,
    pub(crate) move_ptr: i32,
    pub(crate) bisect_stack: Vec<i32>,
    pub(crate) bisect_ptr: i32,
    // [20]
    pub(crate) cur_edges: Pointer,
    pub(crate) cur_wt: i32,
    pub(crate) trace_x: i32,
    pub(crate) trace_y: i32,
    pub(crate) trace_yy: i32,
    // [21]
    pub(crate) octant: i32,
    pub(crate) cur_x: Scaled,
    pub(crate) cur_y: Scaled,
    pub(crate) cur_spec: Pointer,
    pub(crate) turning_number: i32,
    pub(crate) cur_pen: Pointer,
    pub(crate) cur_path_type: i32,
    pub(crate) max_allowed: Scaled,
    pub(crate) before: Vec<Scaled>,
    pub(crate) after: Vec<Scaled>,
    pub(crate) node_to_round: Vec<Pointer>,
    pub(crate) cur_rounding_ptr: i32,
    pub(crate) max_rounding_ptr: i32,
    pub(crate) cur_gran: Scaled,
    pub(crate) octant_number: [i32; 9],
    pub(crate) octant_code: [i32; 9],
    pub(crate) rev_turns: bool,
    pub(crate) y_corr: [i32; 9],
    pub(crate) xy_corr: [i32; 9],
    pub(crate) z_corr: [i32; 9],
    pub(crate) x_corr: [i32; 9],
    // [22]
    pub(crate) m0: i32,
    pub(crate) n0: i32,
    pub(crate) m1: i32,
    pub(crate) n1: i32,
    pub(crate) d0: i32,
    pub(crate) d1: i32,
    // [23]
    pub(crate) env_move: Vec<i32>,
    // [25]
    pub(crate) tol_step: i32,
    // [26]
    pub(crate) cur_t: i32,
    pub(crate) cur_tt: i32,
    pub(crate) time_to_go: i32,
    pub(crate) max_t: i32,
    pub(crate) delx: i32,
    pub(crate) dely: i32,
    pub(crate) tol: i32,
    pub(crate) uv: i32,
    pub(crate) xy: i32,
    pub(crate) three_l: i32,
    pub(crate) appr_t: i32,
    pub(crate) appr_tt: i32,
    // [27]
    pub(crate) screen_started: bool,
    pub(crate) screen_ok: bool,
    pub(crate) window_open: [bool; 16],
    pub(crate) left_col: [i32; 16],
    pub(crate) right_col: [i32; 16],
    pub(crate) top_row: [i32; 16],
    pub(crate) bot_row: [i32; 16],
    pub(crate) m_window: [i32; 16],
    pub(crate) n_window: [i32; 16],
    pub(crate) window_time: [i32; 16],
    pub(crate) row_transition: Vec<i32>,
    // [28]
    pub(crate) serial_no: i32,
    pub(crate) fix_needed: bool,
    pub(crate) watch_coefs: bool,
    pub(crate) dep_final: Pointer,
    // [31]
    pub(crate) cur_cmd: i32,
    pub(crate) cur_mod: i32,
    pub(crate) cur_sym: Halfword,
    pub(crate) input_stack: Vec<InStateRecord>,
    pub(crate) input_ptr: i32,
    pub(crate) max_in_stack: i32,
    pub(crate) cur_input: InStateRecord,
    pub(crate) in_open: i32,
    pub(crate) open_parens: i32,
    pub(crate) input_file: Vec<AlphaFile>,
    pub(crate) line: i32,
    pub(crate) line_stack: Vec<i32>,
    pub(crate) full_source_filename_stack: Vec<StrNumber>,
    pub(crate) param_stack: Vec<Pointer>,
    pub(crate) param_ptr: i32,
    pub(crate) max_param_stack: i32,
    pub(crate) file_ptr: i32,
    // [33]
    pub(crate) scanner_status: i32,
    pub(crate) warning_info: i32,
    pub(crate) force_eof: bool,
    // [34]
    pub(crate) bg_loc: Pointer,
    pub(crate) eg_loc: Pointer,
    // [35]
    // [36]
    pub(crate) cond_ptr: Pointer,
    pub(crate) if_limit: i32,
    pub(crate) cur_if: i32,
    pub(crate) if_line: i32,
    // [37]
    pub(crate) loop_ptr: Pointer,
    // [38]
    pub(crate) cur_name: StrNumber,
    pub(crate) cur_area: StrNumber,
    pub(crate) cur_ext: StrNumber,
    pub(crate) area_delimiter: i32,
    pub(crate) ext_delimiter: i32,
    pub(crate) quoted_filename: bool,
    pub(crate) stop_at_space: bool,
    pub(crate) mf_base_default: Vec<u8>,
    pub(crate) job_name: StrNumber,
    pub(crate) log_opened: bool,
    pub(crate) log_name: StrNumber,
    pub(crate) gf_ext: StrNumber,
    // [39]
    pub(crate) cur_type: i32,
    pub(crate) cur_exp: i32,
    pub(crate) max_c: [i32; 2],
    pub(crate) max_ptr: [Pointer; 2],
    pub(crate) max_link: [Pointer; 2],
    pub(crate) var_flag: i32,
    // [42]
    pub(crate) txx: Scaled,
    pub(crate) txy: Scaled,
    pub(crate) tyx: Scaled,
    pub(crate) tyy: Scaled,
    pub(crate) tx: Scaled,
    pub(crate) ty: Scaled,
    // [43]
    pub(crate) start_sym: Halfword,
    // [44]
    pub(crate) long_help_seen: bool,
    // [45]
    pub(crate) tfm_file: Vec<u8>,
    pub(crate) metric_file_name: StrNumber,
    pub(crate) bc: i32,
    pub(crate) ec: i32,
    pub(crate) tfm_width: [Scaled; 256],
    pub(crate) tfm_height: [Scaled; 256],
    pub(crate) tfm_depth: [Scaled; 256],
    pub(crate) tfm_ital_corr: [Scaled; 256],
    pub(crate) char_exists: [bool; 256],
    pub(crate) char_tag: [i32; 256],
    pub(crate) char_remainder: [i32; 256],
    pub(crate) header_byte: Vec<i32>,
    pub(crate) lig_kern: Vec<FourQuarters>,
    pub(crate) nl: i32,
    pub(crate) kern: Vec<Scaled>,
    pub(crate) nk: i32,
    pub(crate) exten: [FourQuarters; 256],
    pub(crate) ne: i32,
    pub(crate) param: Vec<Scaled>,
    pub(crate) np: i32,
    pub(crate) nw: i32,
    pub(crate) nh: i32,
    pub(crate) nd: i32,
    pub(crate) ni: i32,
    pub(crate) perturbation: Scaled,
    pub(crate) excess: i32,
    pub(crate) dimen_head: [Pointer; 5],
    pub(crate) max_tfm_dimen: Scaled,
    pub(crate) tfm_changed: i32,
    pub(crate) skip_table: [i32; 256],
    pub(crate) lk_started: bool,
    pub(crate) bchar: i32,
    pub(crate) bch_label: i32,
    pub(crate) ll: i32,
    pub(crate) lll: i32,
    pub(crate) label_loc: [i32; 257],
    pub(crate) label_char: [i32; 257],
    pub(crate) label_ptr: i32,
    // [47]
    pub(crate) gf_file: Vec<u8>,
    pub(crate) gf_file_name: Vec<u8>,
    pub(crate) gf_min_m: i32,
    pub(crate) gf_max_m: i32,
    pub(crate) gf_min_n: i32,
    pub(crate) gf_max_n: i32,
    pub(crate) gf_prev_ptr: i32,
    pub(crate) total_chars: i32,
    pub(crate) char_ptr: [i32; 256],
    pub(crate) gf_dx: [i32; 256],
    pub(crate) gf_dy: [i32; 256],
    pub(crate) gf_buf: Vec<u8>,
    pub(crate) half_buf: i32,
    pub(crate) gf_limit: i32,
    pub(crate) gf_ptr: i32,
    pub(crate) gf_offset: i32,
    pub(crate) boc_c: i32,
    pub(crate) boc_p: i32,
    pub(crate) output_file_name: StrNumber,
    // [48]
    pub(crate) base_ident: StrNumber,
    pub(crate) base_file: Vec<u8>,
    pub(crate) base_pos: usize,
    // [49]
    pub(crate) ready_already: i32,
    // [51]
    pub(crate) edit_name_start: i32,
    pub(crate) edit_name_length: i32,
    pub(crate) edit_line: i32,
}

/// Field accessors: `$get(p)` reads, `$set(p, v)` writes the field
/// `$part` of `mem[p+$off]` (the macros of `mf.web`).
macro_rules! fields {
    ($($get:ident $set:ident = $off:literal $part:ident $setpart:ident;)*) => {
        impl Mf { $(
            #[inline]
            pub(crate) fn $get(&self, p: i32) -> i32 {
                self.mem[(p + $off) as usize].$part()
            }
            #[inline]
            pub(crate) fn $set(&mut self, p: i32, v: i32) {
                self.mem[(p + $off) as usize].$setpart(v)
            }
        )* }
    };
}

fields! {
    link set_link = 0 rh set_rh;
    info set_info = 0 lh set_lh;
    type_ set_type = 0 b0 set_b0;
    name_type set_name_type = 0 b1 set_b1;
    value set_value = 1 rh set_rh;
    // [166]
    llink set_llink = 1 lh set_lh;
    rlink set_rlink = 1 rh set_rh;
    // [228], [229]
    attr_head set_attr_head = 1 lh set_lh;
    subscr_head set_subscr_head = 1 rh set_rh;
    attr_loc set_attr_loc = 2 lh set_lh;
    parent set_parent = 2 rh set_rh;
    subscript set_subscript = 2 rh set_rh;
}

impl Mf {
    #[inline]
    pub(crate) fn node_size(&self, p: i32) -> i32 {
        self.info(p)
    }
    #[inline]
    pub(crate) fn set_node_size(&mut self, p: i32, v: i32) {
        self.set_info(p, v);
    }
    #[inline]
    pub(crate) fn ref_count(&self, p: i32) -> i32 {
        self.info(p)
    }
    #[inline]
    pub(crate) fn set_ref_count(&mut self, p: i32, v: i32) {
        self.set_info(p, v);
    }
    #[inline]
    pub(crate) fn add_mac_ref(&mut self, p: i32) {
        let v = self.info(p) + 1;
        self.set_info(p, v);
    }
    // [200]
    #[inline]
    pub(crate) fn next(&self, p: i32) -> i32 {
        self.hash[p as usize].lh
    }
    #[inline]
    pub(crate) fn set_next(&mut self, p: i32, v: i32) {
        self.hash[p as usize].lh = v;
    }
    #[inline]
    pub(crate) fn text(&self, p: i32) -> i32 {
        self.hash[p as usize].rh
    }
    #[inline]
    pub(crate) fn set_text(&mut self, p: i32, v: i32) {
        self.hash[p as usize].rh = v;
    }
    #[inline]
    pub(crate) fn eq_type(&self, p: i32) -> i32 {
        self.eqtb[p as usize].lh
    }
    #[inline]
    pub(crate) fn set_eq_type(&mut self, p: i32, v: i32) {
        self.eqtb[p as usize].lh = v;
    }
    #[inline]
    pub(crate) fn equiv(&self, p: i32) -> i32 {
        self.eqtb[p as usize].rh
    }
    #[inline]
    pub(crate) fn set_equiv(&mut self, p: i32, v: i32) {
        self.eqtb[p as usize].rh = v;
    }
    #[inline]
    pub(crate) fn loc(&self) -> i32 {
        self.cur_input.loc_field
    }
    #[inline]
    pub(crate) fn set_loc(&mut self, v: i32) {
        self.cur_input.loc_field = v;
    }
    #[inline]
    pub(crate) fn limit(&self) -> i32 {
        self.cur_input.limit_field
    }
    #[inline]
    pub(crate) fn set_limit(&mut self, v: i32) {
        self.cur_input.limit_field = v;
    }
    #[inline]
    pub(crate) fn start(&self) -> i32 {
        self.cur_input.start_field
    }
    #[inline]
    pub(crate) fn set_start(&mut self, v: i32) {
        self.cur_input.start_field = v;
    }
    #[inline]
    pub(crate) fn name(&self) -> i32 {
        self.cur_input.name_field
    }
    #[inline]
    pub(crate) fn set_name(&mut self, v: i32) {
        self.cur_input.name_field = v;
    }
    #[inline]
    pub(crate) fn index(&self) -> i32 {
        self.cur_input.index_field
    }
    #[inline]
    pub(crate) fn set_index(&mut self, v: i32) {
        self.cur_input.index_field = v;
    }
    #[inline]
    pub(crate) fn int(&self, v: i32) -> Scaled {
        self.internal[v as usize]
    }
}

fields! {
    // [255]: knots
    left_type set_left_type = 0 b0 set_b0;
    right_type set_right_type = 0 b1 set_b1;
    x_coord set_x_coord = 1 rh set_rh;
    y_coord set_y_coord = 2 rh set_rh;
    left_x set_left_x = 3 rh set_rh;
    left_y set_left_y = 4 rh set_rh;
    right_x set_right_x = 5 rh set_rh;
    right_y set_right_y = 6 rh set_rh;
}

fields! {
    // [325], [326]: edge structures
    knil set_knil = 0 lh set_lh;
    sorted set_sorted = 1 rh set_rh;
    unsorted set_unsorted = 1 lh set_lh;
    n_min set_n_min = 1 lh set_lh;
    n_max set_n_max = 1 rh set_rh;
    m_min set_m_min = 2 lh set_lh;
    m_max set_m_max = 2 rh set_rh;
    m_offset set_m_offset = 3 lh set_lh;
    last_window set_last_window = 3 rh set_rh;
    last_window_time set_last_window_time = 4 rh set_rh;
    n_pos set_n_pos = 5 lh set_lh;
    n_rover set_n_rover = 5 rh set_rh;
}
