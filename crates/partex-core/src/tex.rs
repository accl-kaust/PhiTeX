//! The engine: tex.web's global variables as one struct.
//!
//! Procedures are methods in `impl` blocks spread over one module per WEB
//! part (`charset`, `strings`, `print`, …), so a WEB section maps to one
//! place in Rust. Non-local `goto`s (`jump_out`, `final_end`) become
//! [`Jump`] errors propagated with `?`.

use alloc::collections::BTreeMap;
use alloc::vec;
use alloc::vec::Vec;

use crate::diag::DiagState;
use crate::eqtb::{EQTB_SIZE, HASH_BASE, UNDEFINED_CONTROL_SEQUENCE};
use crate::fonts::FontArrays;
use crate::host::{Host, WriteId};
use crate::input::{AlphaFile, InStateRecord};
use crate::mem::{MemoryWord, NULL, Pointer};
use crate::nest::ListStateRecord;
use crate::params::Params;
use crate::track::{Tracker, Untracked};
use crate::web::{BOTTOM_LEVEL, LEVEL_ONE, NORMAL};
use partex_engine::node::{Node, Tokens};

/// Control leaving the normal flow: tex.web's `goto end_of_TEX` / `final_end`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Jump {
    /// `jump_out` (§81): close files and terminate. The closing happens
    /// once the jump reaches `run`, so work in flight (a page being shipped
    /// out) is handed over first, as tex.web's buffers would have it.
    JumpOut,
    /// Not an exit: main control stopped between two commands so that the
    /// caller can take a checkpoint (see [`Tex::start`]).
    Checkpoint,
    /// Terminate after `jump_out`'s cleanup.
    EndOfTex,
    /// `goto final_end` (§1332): terminate without the usual cleanup.
    FinalEnd,
}

pub type Result<T> = core::result::Result<T, Jump>;

/// A buffered text output file (log or `\write` stream).
#[derive(Clone, Default)]
pub(crate) struct OutFile {
    pub(crate) id: Option<WriteId>,
    pub(crate) buf: Vec<u8>,
}

partex_engine::persist_struct!(OutFile { id, buf });

/// Flush threshold for buffered output.
pub(crate) const OUT_CHUNK: usize = 1 << 16;

#[derive(Clone)]
pub struct Tex<H: Host, T: Tracker = Untracked> {
    pub(crate) host: H,
    pub(crate) tracker: T,
    /// Capacity parameters after `const_chk` clamping.
    pub(crate) params: Params,

    // §20: character set translation.
    pub(crate) xord: [u8; 256],
    pub(crate) xchr: [u8; 256],
    /// Non-zero iff the character prints as itself (web2c, §24).
    pub(crate) xprn: [bool; 256],
    /// `XeTeX`'s Unicode (DESIGN 4.7): pool units UTF-16 kept as CESU-8,
    /// characters printed as UTF-8. Set from the flavor, once.
    pub(crate) unicode: bool,
    /// `XeTeX` §61: printing a `\special`'s text (characters go out raw).
    pub(crate) doing_special: bool,
    /// Scratch for a name being looked up, in the pool's encoding.
    pub(crate) name_scratch: alloc::vec::Vec<u8>,
    /// `XeTeX`: what finding native fonts keeps (not engine state).
    pub(crate) xfont: crate::native::NativeEnv,
    /// `XeTeX` §548: the quote a file name being scanned is in (0: none).
    pub(crate) file_name_quote_char: u32,

    // §39: the string pool.
    pub(crate) str_pool: crate::flat::Flat<u8>,
    pub(crate) str_start: crate::flat::Flat<usize>,
    pub(crate) pool_ptr: usize,
    pub(crate) str_ptr: usize,
    pub(crate) init_pool_ptr: usize,
    pub(crate) init_str_ptr: usize,
    /// The pool's strings by contents (`search_string`).
    pub(crate) str_index: crate::strings::StrIndex,
    /// Skipped conditional text, remembered (`skipcache.rs`).
    pub(crate) skip: crate::skipcache::SkipCache,
    /// With a machine's tracker, skips are remembered too (`skip_tracked`):
    /// what they depend on, every control sequence's class to a skip, is
    /// one cell, hashed in `class_hash` (derived from eqtb, not state);
    /// whether a remembered skip was used and a class changed since the
    /// machine last looked (scratch).
    pub(crate) skip_tracked: bool,
    pub(crate) class_hash: u128,
    pub(crate) classes_read: bool,
    pub(crate) classes_written: bool,

    // §54: printing.
    pub(crate) log_file: OutFile,
    pub(crate) term_buf: Vec<u8>,
    pub(crate) write_file: [OutFile; 16],
    pub(crate) selector: i32,
    pub(crate) dig: [u8; 23],
    pub(crate) tally: i32,
    pub(crate) term_offset: i32,
    pub(crate) file_offset: i32,
    /// With the columns the link's, what printing makes (`effects/flow.rs`).
    pub(crate) flow: crate::effects::flow::Flow,
    pub(crate) trick_buf: Vec<u32>,
    pub(crate) trick_count: i32,
    pub(crate) first_count: i32,

    // §104: arithmetic.
    pub(crate) arith_error: bool,
    pub(crate) save_arith_error: bool,
    pub(crate) remainder: i32,

    /// `XeTeX` §767: `cur_f`, the font of the last math character
    /// fetched, which `XeTeX`'s math reads after the fact (whether it is
    /// an OpenType math font); `null_font` in TeX and pdfTeX.
    pub(crate) cur_f: i32,

    /// §304: the current line number in the current source file.
    pub(crate) line: i32,

    // §30: the input buffer.
    pub(crate) buffer: crate::flat::Flat<u32>,
    pub(crate) first: usize,
    pub(crate) last: usize,
    pub(crate) max_buf_stack: usize,

    // §73–§96: error handling state.
    pub(crate) interaction: i32,
    pub(crate) deletions_allowed: bool,
    pub(crate) set_box_allowed: bool,
    pub(crate) history: i32,
    pub(crate) error_count: i32,
    pub(crate) help_line: [&'static [u8]; 6],
    pub(crate) help_ptr: usize,
    pub(crate) use_err_help: bool,

    // encTeX printing flags (merged §20). encTeX itself (`-enc`) is not
    // supported yet; these keep their initial values.
    pub(crate) special_printing: bool,
    pub(crate) message_printing: bool,
    pub(crate) no_convert: bool,
    pub(crate) active_noconvert: bool,
    pub(crate) cs_converting: bool,

    // §173, §181: box display.
    pub(crate) font_in_short_display: i32,
    pub(crate) depth_threshold: i32,
    pub(crate) breadth_max: i32,

    // §213: the semantic nest.
    /// The enclosing levels (`nest[0..nest_ptr]`).
    pub(crate) nest: Vec<ListStateRecord>,
    pub(crate) max_nest_stack: usize,
    pub(crate) cur_list: ListStateRecord,
    pub(crate) shown_mode: i32,

    // §246, §253: eqtb, and the values its glue, box and shape entries
    // name.
    pub(crate) eqtb: crate::journal::JVec<MemoryWord>,
    /// The objects eqtb's entries hold beside their words (`objs.rs`),
    /// by location below the registers above 255: journaled as eqtb is,
    /// so a snapshot copies the chunks written since the last one, not
    /// the whole table (it spans the hash's extra places too: 16.8 MB on
    /// a LaTeX format, copied by each of a course's 589 snapshots).
    pub(crate) eqtb_obj: crate::journal::JVec<Option<crate::objs::Obj>, { crate::objs::OBJ_CHUNK }>,
    /// The next glue lineage (`objs.rs`: glue's identity, as data).
    pub(crate) glue_lineage: u64,
    /// `xeq_level[int_base..=eqtb_size]`, stored from index 0.
    pub(crate) xeq_level: Vec<i32>,
    pub(crate) eqtb_top: i32,
    pub(crate) old_setting: i32,
    pub(crate) sys_time: i32,
    pub(crate) sys_day: i32,
    pub(crate) sys_month: i32,
    pub(crate) sys_year: i32,

    // §256: the hash table (`next` = lh, `text` = rh), indexed from `hash_base`.
    pub(crate) hash: crate::journal::JVec<MemoryWord>,
    pub(crate) hash_used: i32,
    pub(crate) hash_top: i32,
    pub(crate) hash_high: i32,
    pub(crate) no_new_control_sequence: bool,
    pub(crate) cs_count: i32,

    /// §410: the value returned by scanning routines (and `primitive`).
    pub(crate) cur_val: i32,
    /// The value of glue scanned when `cur_val_level` is `glue_val` or
    /// `mu_val` (tex.web keeps a spec pointer in `cur_val`).
    pub(crate) cur_glue: partex_engine::node::GlueSpec,
    /// The token list scanned when `cur_val_level` is `tok_val` (tex.web
    /// keeps its pointer in `cur_val`; scratch, set by the scan).
    pub(crate) cur_toks: Option<partex_engine::node::Tokens>,
    /// The lineage of `cur_glue` if it is a stored glue unchanged (e-TeX
    /// then assigns that very glue; see objs.rs).
    pub(crate) glue_origin: Option<u64>,
    /// e-TeX: what the last `\vsplit` discarded, if saved (`split_disc`).
    /// e-TeX's `split_disc` (§977): a persistent list, a value of the
    /// page family (DESIGN 7.17.12, `track::page::SPLIT_DISCARDS`).
    pub(crate) split_discards: partex_engine::nodelist::NodeList,
    /// §333: the location and token of `\par`.
    pub(crate) par_loc: i32,
    pub(crate) par_token: i32,
    /// §1345: the eqtb location of `\write`.
    pub(crate) write_loc: i32,
    /// web2c: `MLTeX` and `encTeX` are active (set by `init_prim` or the format).
    pub(crate) mltex_enabled_p: bool,
    pub(crate) enctex_enabled_p: bool,

    // §96: interrupts; web2c's `halting_on_error_p`; the `E` option (§84).
    pub(crate) interrupt: i32,
    pub(crate) ok_to_interrupt: bool,
    pub(crate) halting_on_error: bool,
    /// Set by the `E` response to an error: (file name string, line).
    pub(crate) edit_request: Option<(i32, i32)>,

    // §271, §286: the save stack.
    pub(crate) save_stack: crate::journal::JVec<MemoryWord>,
    pub(crate) save_ptr: i32,
    /// Which save stack slots hold copies of eqtb words (see `mark_save`).
    pub(crate) save_eqtb: Vec<bool>,
    /// The objects the save stack's copies of eqtb words hold (`objs.rs`).
    pub(crate) save_obj: Vec<Option<crate::objs::Obj>>,
    pub(crate) max_save_stack: i32,
    pub(crate) cur_level: i32,
    pub(crate) cur_group: i32,
    pub(crate) cur_boundary: i32,
    pub(crate) mag_set: i32,

    // §297: the current token.
    pub(crate) cur_cmd: i32,
    pub(crate) cur_chr: i32,
    pub(crate) cur_cs: Pointer,
    pub(crate) cur_tok: i32,

    // §301–§310: input stacks and states.
    pub(crate) input_stack: Vec<InStateRecord>,
    pub(crate) input_ptr: usize,
    pub(crate) max_in_stack: usize,
    pub(crate) cur_input: InStateRecord,
    pub(crate) in_open: usize,
    pub(crate) open_parens: i32,
    /// `SyncTeX`'s `synctex_tag_counter`: the files opened while it is on
    /// (each one's tag is its `AlphaFile::synctex_tag`).
    pub(crate) synctex_tags: i32,
    /// In SSA mode, `SyncTeX`'s controller's flags as its events left them
    /// so far (`synctex.rs`, `FLAG_*`): its warnings are printed where
    /// pdfTeX prints them, by the step, not by the render.
    pub(crate) synctex_flags: i32,
    /// The first file's name as the host found it (`SyncTeX`'s root, for a
    /// `\synctex` the document sets), and whether a page or form was
    /// shipped while `SyncTeX` was not on (`synctex.rs`).
    pub(crate) synctex_root: Option<alloc::sync::Arc<[u8]>>,
    pub(crate) synctex_shipped: bool,
    pub(crate) input_file: Vec<Option<AlphaFile>>,
    pub(crate) line_stack: Vec<i32>,
    /// e-TeX: per input file, `cur_boundary` and the depth of the
    /// condition stack when it began (`grp_stack`, `if_stack`), and
    /// whether `\everyeof` has been inserted at its end.
    pub(crate) grp_stack: Vec<i32>,
    pub(crate) if_stack: Vec<usize>,
    pub(crate) eof_seen: Vec<bool>,
    /// e-TeX: the lines not yet read of each `\scantokens` pseudo file.
    pub(crate) pseudo_files: Vec<crate::input::PseudoFile>,
    /// The input stack's value (DESIGN 7.17.13 item 1), kept when the
    /// tracker keeps values.
    pub(crate) input_values: crate::input::InputValues,
    pub(crate) source_filename_stack: Vec<i32>,
    pub(crate) full_source_filename_stack: Vec<i32>,
    pub(crate) scanner_status: i32,
    pub(crate) warning_index: Pointer,
    /// §473: the list a definition is building (tex.web's `def_ref`), and
    /// e-TeX's `\protected` flag for it.
    pub(crate) def_ref: Vec<i32>,
    pub(crate) def_protected: bool,
    /// §308: the parameters of the macros being expanded: shared lists.
    pub(crate) param_stack: Vec<Option<partex_engine::node::Tokens>>,
    pub(crate) param_ptr: i32,
    pub(crate) max_param_stack: i32,
    pub(crate) align_state: i32,
    pub(crate) base_ptr: usize,

    /// web2c: the current depth of `expand` recursion.
    pub(crate) expand_depth_count: i32,
    /// §410, §438, §447: set by the scanning routines.
    pub(crate) cur_val_level: i32,
    pub(crate) radix: i32,
    pub(crate) cur_order: i32,

    // §592, §646, §980–§982, §989: page builder state read by scanners.
    pub(crate) dead_cycles: i32,
    /// e-TeX: shipping out right-to-left text (`cur_dir`).
    pub(crate) out_rtl: bool,
    /// e-TeX: `\endL`/`\endR` problems of the page being shipped out:
    /// 10000 per missing closing node plus one per extra one.
    pub(crate) lr_problems: u32,
    pub(crate) last_badness: i32,
    pub(crate) output_active: bool,

    /// §480: `\read` files and their states.
    pub(crate) read_file: [Option<AlphaFile>; 16],
    pub(crate) read_open: [i32; 17],

    // §512, §513, §26, §527: file names.
    pub(crate) cur_name: i32,
    pub(crate) cur_area: i32,
    pub(crate) cur_ext: i32,
    pub(crate) area_delimiter: usize,
    pub(crate) ext_delimiter: usize,
    pub(crate) quoted_filename: bool,
    pub(crate) stop_at_space: bool,
    pub(crate) name_of_file: Vec<u8>,
    /// §1299: the format identification (`" (INITEX)"` in INITEX).
    pub(crate) format_ident: i32,
    /// The format file loaded (§1303), a value: its definitions are what
    /// a slot holds before any step of the build defines it (DESIGN
    /// 7.17.3, `ssa::rebuild`).
    pub(crate) format_data: Option<alloc::sync::Arc<[u8]>>,

    /// §361: should the next `\input` be aborted early?
    pub(crate) force_eof: bool,
    /// §387: governs the acceptance of `\par` in macro arguments.
    pub(crate) long_state: i32,
    /// §489, §493: the condition stack and where skipping began.
    pub(crate) cond_stack: crate::conds::CondStack,
    pub(crate) if_limit: i32,
    pub(crate) cur_if: i32,
    pub(crate) if_line: i32,
    pub(crate) skip_line: i32,

    /// §382: `cur_mark[top_mark_code..split_bot_mark_code]`, by mark
    /// class (e-TeX's `\marks`; class 0 is `\mark`).
    pub(crate) cur_mark: BTreeMap<i32, [Option<Tokens>; 5]>,

    // §527, §532: file names.
    pub(crate) job_name: i32,
    pub(crate) log_opened: bool,
    pub(crate) name_in_progress: bool,
    pub(crate) output_file_name: i32,
    pub(crate) log_name: i32,

    // §549–§550: fonts.
    pub(crate) fmem_ptr: i32,
    pub(crate) font_ptr: i32,
    // §592, §1342: DVI output and `\write` streams.
    pub(crate) dvi: crate::dvi::DviState,
    pub(crate) write_open: [bool; 18],
    // §646, §647, §661: packaging.
    pub(crate) pack_begin_line: i32,
    /// §1085: material migrating out of an `adjusted_hbox_group` box
    /// (tex.web's `adjust_tail` list, when it is not null).
    pub(crate) adjust: Option<Vec<Node>>,
    // §892–§950: hyphenation; §907: the ligature cursor.
    pub(crate) hyph: crate::hyph::HyphState,
    // §980: the page builder.
    pub(crate) page: partex_engine::builder::Builder,
    // §770: alignment; §1074: the box being built; §1266, §1281.
    pub(crate) align: crate::align::AlignState,
    /// pdfTeX §1652: `eTeX_mode` is 1: e-TeX's extended mode.
    pub(crate) etex_mode: bool,
    /// pdfTeX: the start of `\pdfelapsedtime` (seconds, microseconds).
    pub(crate) epoch: (i32, i32),
    /// pdfTeX: inside `\csname` or `\ifcsname` (`\ifincsname`).
    pub(crate) is_in_csname: bool,
    /// e-TeX's registers above 255.
    pub(crate) xregs: crate::xregs::ExtRegs,
    /// pdfTeX §1814: the largest allowed register number, and the help
    /// line that says so.
    pub(crate) max_reg_num: i32,
    pub(crate) max_reg_help_line: &'static [u8],
    /// Lists no one else holds, kept to be filled again (`tok.rs`: macro
    /// arguments and backed-up tokens are made by the tens of millions).
    pub(crate) tok_pool: Vec<partex_engine::node::Tokens>,
    /// `macro_call`'s arguments, kept between calls (scratch).
    pub(crate) pstack_buf: Vec<Option<partex_engine::node::Tokens>>,
    /// §162: `null_list` and `omit_template`, made once (shared values).
    pub(crate) empty_list: partex_engine::node::Tokens,
    pub(crate) omit_list: partex_engine::node::Tokens,
    /// The argument `macro_call` is scanning (tex.web builds it at
    /// `temp_head`), for "Runaway argument", while `arg_active`.
    pub(crate) arg_list: Vec<i32>,
    pub(crate) arg_active: bool,
    /// The lookups `get_next` makes want only the tokens: an argument's,
    /// a body's without expansion, an assignment's target
    /// ([`Tex::tokens_only`], [`Tracker::CLASSES`]). (Not saved: set
    /// only inside a scan, by a tracker that takes no snapshots.)
    pub(crate) token_only: bool,
    /// The template `get_preamble_token` is scanning (tex.web builds it at
    /// `hold_head`), for "Runaway preamble", while `preamble_active`.
    pub(crate) preamble_list: Vec<i32>,
    pub(crate) preamble_active: bool,
    /// pdfTeX §275: the primitive table.
    pub(crate) prims: crate::prim::PrimTable,
    /// pdfTeX §110: the random number generator.
    pub(crate) random: crate::random::Randoms,
    /// §1074: the box (or leader rule) being built.
    pub(crate) cur_box: Option<Node>,
    pub(crate) after_token: i32,
    /// `XeTeX`'s `prev_class` and `space_class` (`xmain.rs`).
    pub(crate) prev_class: i32,
    pub(crate) space_class: i32,
    /// `XeTeX`'s `XeTeX_default_input_mode` and `_encoding`, as an input
    /// file's `mode` and `conv` (`input.rs`): the mode in the low byte,
    /// the converter above it.
    pub(crate) default_input: i32,
    /// Memoized macro calls (`memo.rs`).
    pub(crate) memo: crate::memo::Memo,
    /// Control sequence names by a hash of their text, to their location
    /// (`id_lookup`'s shortcut past the hash chains; shared by clones).
    pub(crate) cs_cache: crate::hash::CsCache,
    /// Sub-hashes of shared chunks for the machine's state hash
    /// (`hashmemo.rs`; scratch, not state).
    pub(crate) hash_memo: crate::hashmemo::HashMemo,
    /// Lines read from files, by name and number (`u32::MAX`: the file
    /// was opened to be read by lines), for a machine's line cells;
    /// drained by `machine.rs` (scratch, not state).
    pub(crate) line_log: Vec<(alloc::sync::Arc<[u8]>, u32)>,
    /// Keep `line_log` (a machine drains it; anyone else would carry it
    /// to the end of the job, cloned into every checkpoint).
    pub(crate) log_lines: bool,
    /// The glyphs marked used since a machine last looked, by font, and
    /// whether the glyphs used were read (the fonts written); scratch.
    pub(crate) glyphs_used: Vec<[u64; 4]>,
    /// Record each object stream written (`Tex::record_objstms`), and the
    /// records not taken yet: output, not state.
    pub(crate) record_objstms: bool,
    pub(crate) objstms_written: Vec<crate::pdf::out::ObjStmWritten>,
    pub(crate) glyphs_read: bool,
    /// Sealed lines (`seal.rs`): the table (cells of their own, not
    /// `Rest`), whether lines are sealed, where the last paragraph was
    /// broken and how many were broken there, and the log of sealed
    /// lines read (`Some` version) and written for a machine (scratch).
    pub(crate) seals: crate::seal::SealTable,
    pub(crate) seal_lines: bool,
    /// A machine's `Rest` hashes a reference to a string made by the run
    /// by the string's characters, not its number (`statehash.rs`): two
    /// runs that made the same strings in another order are in the same
    /// state (scratch: how the state is hashed, not state).
    pub(crate) canon_strings: bool,
    /// A control sequence made in the `hash_extra` region goes to a place
    /// its name picks (probed), not the next free one (`hash.rs`): two
    /// runs that made the same names in another order give them the same
    /// locations (a machine's; scratch: where names go is not state TeX
    /// observes).
    pub(crate) cs_by_name: bool,
    /// The hash table's names are a machine's cells (`machine.rs`): a
    /// slot's name is part of its eqtb word's value, its link a cell of
    /// its own, and a lookup that finds a name missing (or enters it)
    /// reads the name's cell, logged in `name_log` (with whether it was
    /// entered). The table and the string pool then leave `Rest`.
    pub(crate) name_cells: bool,
    pub(crate) name_log: Vec<(alloc::sync::Arc<[u8]>, bool)>,
    /// The fonts are a machine's cells (`machine.rs`): each slot one
    /// (`MCell::Font`), the fonts by name, the expanded fonts, and the
    /// order of loading, which accumulates; the searches by name and for
    /// an expanded font are logged in `font_log`. They then leave `Rest`.
    pub(crate) font_cells: bool,
    pub(crate) font_log: Vec<crate::fonts::FontTouch>,
    /// With names as cells and placed by name: the run's names are placed
    /// by probing and found that way, not chained (`hash.rs`,
    /// `id_lookup_probe`), so a lookup reads no links.
    pub(crate) probe_names: bool,
    /// The session's shared interner (`interner.rs`): the names the run
    /// makes are placed by it, one place for every view.
    #[cfg(feature = "std")]
    pub(crate) shared_names: Option<alloc::sync::Arc<crate::interner::SharedNames>>,
    pub(crate) seal_at: (u128, u32),
    pub(crate) seal_log: Vec<(u128, Option<u128>)>,
    /// Stop before `\shipout` (a machine's region boundary: the region
    /// that ships a page reads its lines, the one before does not), and
    /// whether the `\shipout` about to be read again is the one stopped
    /// before (1); with `stop_after_ship` (a machine's page layer), a
    /// `\shipout` just done (2) and main control stopped after it (3).
    pub(crate) stop_before_ship: bool,
    pub(crate) ship_stop: u8,
    pub(crate) stop_after_ship: bool,
    /// Stop at the command after one that read a file whole by name (SSA
    /// mode: `\pdffilesize`, which LaTeX's `\IfFileExists` asks), and
    /// whether one did (1) or main control stopped there (2).
    pub(crate) stop_after_load: bool,
    pub(crate) load_stop: u8,
    /// Defer the page builder after a paragraph's end (§1094), and after
    /// its start (§1091) in a step that ran commands before it, to the next
    /// command, a step boundary (SSA mode, `CleanPoint::Page`), and whether
    /// it is pending.
    pub(crate) defer_page: bool,
    pub(crate) page_pending: bool,
    /// Stopped at a paragraph's start (`CleanPoint::Graf`), or before the
    /// page builder `new_graf` deferred: the step that begins there takes
    /// the paragraph's `mode_line` as its own.
    pub(crate) graf_stop: bool,
    /// The commands begun when the open step began (SSA mode).
    pub(crate) step_began: u64,
    /// A paragraph was begun (§1091, at `nest_ptr = 1`) since the last
    /// candidate boundary: the next one is its start (`Tex::clean_point`).
    pub(crate) par_start: bool,
    /// The page builder decided to fire and the fire waits for the next
    /// `big_switch`, a step boundary (DESIGN §7.16.1, "The deferred
    /// fire's form"); only while [`Tex::defer_fire`] is on.
    pub(crate) fire_pending: bool,
    /// Defer the page builder's fires to the next `big_switch` (an SSA
    /// build's steps, `ssa.rs`).
    pub(crate) defer_fire: bool,
    /// Relocatable numbers (DESIGN 4.1, `reloc.rs`): whether `\the` and
    /// `\number` tag the digits they make of a count register (the
    /// machine's switch).
    pub(crate) tags_on: bool,
    /// The tagged token `get_next` read last, as its list holds it (0:
    /// none since): what a copy of `cur_tok` stores.
    pub(crate) cur_raw: i32,
    /// The origin of a tagged token read and not yet stored or backed up,
    /// plus one (0: none): an observation when the next token is read or
    /// the region ends.
    pub(crate) tag_pending: i32,
    /// `scan_something_internal`'s caller takes the origin of `cur_val`
    /// (`cur_val_origin`); `scan_int`'s caller does (`int_origin_req`).
    pub(crate) want_origin: bool,
    pub(crate) int_origin_req: bool,
    /// The origin of `cur_val` (`reloc::NO_ORIGIN`: none), and of the
    /// expression `scan_expr` just evaluated.
    pub(crate) cur_val_origin: i32,
    pub(crate) expr_origin: i32,
    /// The count register `\advance` is about to assign its sum (0: none):
    /// an affine write, no observation.
    pub(crate) affine_count: i32,
    /// Windows (DESIGN 4.3 item 1): the commands after which the next
    /// boundary ends the open window (0: no windows, the clean points of
    /// [`Tex::clean_point`]); the command counter where the open window
    /// began; and whether an event since then ends it at the next
    /// boundary (a paragraph's end, a fire, a file level opened or
    /// closed). Scheduling, not state.
    pub(crate) window: u64,
    pub(crate) window_start: u64,
    pub(crate) window_cut: Option<crate::run::WindowEvent>,
    /// `\def` is defining a body just made (and maybe interned).
    pub(crate) fresh_def: bool,
    pub(crate) long_help_seen: bool,
    /// §1032: `\noboundary` was just seen.
    pub(crate) cancel_boundary: bool,
    /// Stop for a checkpoint every this many commands (0: never).
    pub(crate) checkpoint_every: u64,
    /// Commands main control has begun (see [`Tex::commands`]).
    pub(crate) commands: u64,
    /// Stop for a checkpoint once `commands` reaches this (0: never; the
    /// sanitizer's region boundaries, `sanitize.rs`).
    pub(crate) stop_at: u64,
    /// Stop before the next command read straight from a file (a
    /// machine step, `machine.rs`).
    pub(crate) stop_at_candidate: bool,
    /// Stopped for a checkpoint before the current command.
    pub(crate) at_checkpoint: bool,
    /// Where the last checkpoint was: input file level, block of lines.
    pub(crate) checkpoint_at: (usize, i64),
    /// The line block the input is in, and the pages shipped when it
    /// entered it.
    pub(crate) block_entered: (i64, u64),
    /// Also stop at each new line of a file read in vertical mode (between
    /// paragraphs), apart from the checkpoints above (`Tex::set_dense`);
    /// where the last such stop was; whether the last stop was only that.
    /// Not state.
    pub(crate) dense: bool,
    pub(crate) dense_at: (usize, i32),
    pub(crate) dense_last: bool,
    /// Pages shipped out so far.
    pub(crate) shipped: u64,
    /// pdfTeX's `\pdfglyphtounicode` table (utils.c's
    /// `glyph_unicode_tree`): a persistent map carrying its version (the
    /// `TOUNICODE` field, `pdf::val`).
    pub(crate) tounicode: crate::pdfconv::ToUnicodeTable,
    /// pdfTeX's PDF output state.
    pub(crate) pdf: crate::pdf::PdfState,
    /// pdfTeX's font map (mapfile.c).
    pub(crate) fontmap: crate::fontmap::FontMap,
    /// What the map file read gives, kept by its contents' identity (not
    /// state: `fontmap::MapCache`).
    pub(crate) map_cache: crate::fontmap::MapCache,
    /// TFM names whose map entries have been used (`in_use`; the
    /// `FONTS_MAPPED` field).
    pub(crate) fonts_mapped: crate::pdf::val::VSet<Vec<u8>>,
    /// What the streams' values need beside the fields above: the file
    /// each `\write` stream stores to, each `\openin` stream's contents'
    /// version (`streams`).
    pub(crate) streams: crate::streams::Streams,
    pub(crate) fonts: FontArrays,

    /// Structured diagnostics (`diag.rs`); `None` when disabled.
    pub(crate) diag: Option<DiagState>,
    /// Outputs as values, not yet taken (`effects.rs`); `None`: outputs
    /// go to the host as they are made.
    pub(crate) effects: Option<Vec<crate::effects::Effect>>,
    /// Glyph origins (`srcmap.rs`, DESIGN 4.4), when they are recorded:
    /// the table nodes' handles point into, what the walk collects, and
    /// how the sources' edits move them. `None`: off, and free.
    pub(crate) org: Option<alloc::boxed::Box<crate::srcmap::OrgState>>,
    /// `SyncTeX` (`synctex.rs`, DESIGN 4.5), when asked for: the places
    /// nodes' handles point into, the controller and its file. `None`:
    /// off, and free.
    pub(crate) sync: crate::synctex::State,
    /// Display lists (`displist.rs`, DESIGN 4.6), when they are kept: the
    /// open stream's literals, the streams shipped (without a recorder),
    /// and what the queries made. `None`: off, and free.
    pub(crate) dl: Option<alloc::boxed::Box<crate::displist::DlState>>,
    /// A page's stream for [`Host::stream_shipped`], from its stream's end
    /// until its page object is written (never held at a snapshot).
    pub(crate) tap: Option<alloc::boxed::Box<crate::displist::Shipped>>,
}

/// web2c's `const_chk` bounds (merged §11): (inf, sup) per parameter.
fn clamp(v: i32, inf: i32, sup: i32) -> i32 {
    v.clamp(inf, sup)
}

impl<H: Host, T: Tracker> Tex<H, T> {
    /// Allocate the engine the way `main_body` (§1332) does: clamp the
    /// parameters, then size the arrays.
    pub fn new(host: H, tracker: T, params: Params) -> Self {
        let mut p = params;
        // §1332: `const_chk` on every bound (merged §11 has the limits).
        p.mem_bot = clamp(p.mem_bot, 0, 1);
        p.main_memory = clamp(p.main_memory, 3000, 256_000_000);
        p.trie_size = clamp(p.trie_size, 8000, 0x3F_FFFF);
        p.hyph_size = clamp(p.hyph_size, 610, 65535);
        p.buf_size = clamp(p.buf_size, 500, 30_000_000);
        p.nest_size = clamp(p.nest_size, 40, 4000);
        p.max_in_open = clamp(p.max_in_open, 6, 127);
        p.param_size = clamp(p.param_size, 60, 32767);
        p.save_size = clamp(p.save_size, 600, 30_000_000);
        p.stack_size = clamp(p.stack_size, 200, 30000);
        p.dvi_buf_size = clamp(p.dvi_buf_size, 800, 65536);
        p.pool_size = clamp(p.pool_size, 32000, 40_000_000);
        p.string_vacancies = clamp(p.string_vacancies, 8000, 40_000_000 - 23000);
        p.pool_free = clamp(p.pool_free, 1000, 40_000_000);
        p.max_strings = clamp(p.max_strings, 3000, 2_097_151);
        p.strings_free = clamp(p.strings_free, 100, 2_097_151);
        p.font_mem_size = clamp(p.font_mem_size, 20000, 147_483_647);
        p.font_max = clamp(p.font_max, 50, 9000);
        p.hash_extra = clamp(p.hash_extra, 0, 2_097_151);
        // `expand_depth` has bounds in §11 but main_body does not clamp it.
        p.error_line = p.error_line.min(255);

        let pool_size = usize::try_from(p.pool_size).unwrap_or(0);
        // §1332 (web2c): `eqtb_top`, `hash_top`. web2c allocates `eqtb_top`
        // words but initializes `eqtb[eqtb_top]` (§222); we allocate one more.
        let eqtb_top = EQTB_SIZE + p.hash_extra;
        let hash_top = if p.hash_extra == 0 {
            UNDEFINED_CONTROL_SEQUENCE
        } else {
            eqtb_top
        };
        let eqtb_words = usize::try_from(eqtb_top + 1).unwrap_or(0);
        let hash_words = usize::try_from(hash_top - HASH_BASE + 1).unwrap_or(0);
        let buf_size = usize::try_from(p.buf_size).unwrap_or(0);
        let nest_size = usize::try_from(p.nest_size).unwrap_or(0);
        let max_strings = usize::try_from(p.max_strings).unwrap_or(0);
        let save_size = usize::try_from(p.save_size).unwrap_or(0);
        let stack_size = usize::try_from(p.stack_size).unwrap_or(0);
        let max_in_open = usize::try_from(p.max_in_open).unwrap_or(0);
        let param_size = usize::try_from(p.param_size).unwrap_or(0);
        Self {
            host,
            tracker,
            xord: [0; 256],
            xchr: [0; 256],
            xprn: [false; 256],
            unicode: p.flavor == crate::params::Flavor::XeTeX,
            doing_special: false,
            name_scratch: alloc::vec::Vec::new(),
            xfont: crate::native::NativeEnv::default(),
            file_name_quote_char: 0,
            str_pool: crate::flat::Flat::new(vec![0; pool_size.min(1 << 16) + 1]),
            str_start: crate::flat::Flat::new(vec![0; max_strings.min(1 << 12) + 1]),
            pool_ptr: 0,
            str_ptr: 0,
            init_pool_ptr: 0,
            init_str_ptr: 0,
            str_index: crate::strings::StrIndex::default(),
            skip: crate::skipcache::SkipCache::default(),
            log_file: OutFile::default(),
            term_buf: Vec::new(),
            write_file: Default::default(),
            selector: 0,
            dig: [0; 23],
            tally: 0,
            term_offset: 0,
            file_offset: 0,
            flow: crate::effects::flow::Flow::default(),
            trick_buf: vec![0; 256],
            trick_count: 0,
            first_count: 0,
            line: 0,
            arith_error: false,
            save_arith_error: false,
            remainder: 0,
            cur_f: 0,
            buffer: crate::flat::Flat::new(vec![0; buf_size + 1]),
            first: 0,
            last: 0,
            max_buf_stack: 0,
            interaction: crate::error::ERROR_STOP_MODE,
            deletions_allowed: true,
            set_box_allowed: true,
            history: crate::error::FATAL_ERROR_STOP,
            error_count: 0,
            help_line: [b""; 6],
            help_ptr: 0,
            use_err_help: false,
            special_printing: false,
            message_printing: false,
            no_convert: false,
            active_noconvert: false,
            cs_converting: false,
            font_in_short_display: 0,
            depth_threshold: 0,
            breadth_max: 0,
            nest: Vec::with_capacity(nest_size),
            max_nest_stack: 0,
            cur_list: ListStateRecord::default(),
            shown_mode: 0,
            eqtb: crate::journal::JVec::from_elem(MemoryWord::default(), eqtb_words),
            eqtb_obj: crate::journal::JVec::from_elem(None, eqtb_words),
            glue_lineage: 0,
            xeq_level: vec![0; usize::try_from(EQTB_SIZE - crate::eqtb::INT_BASE + 1).unwrap_or(0)],
            eqtb_top,
            old_setting: 0,
            sys_time: 0,
            sys_day: 0,
            sys_month: 0,
            sys_year: 0,
            hash: crate::journal::JVec::from_elem(MemoryWord::default(), hash_words),
            hash_used: 0,
            hash_top,
            hash_high: 0,
            no_new_control_sequence: true,
            cs_count: 0,
            cur_val: 0,
            cur_glue: partex_engine::node::GlueSpec::ZERO_GLUE,
            glue_origin: None,
            cur_toks: None,
            split_discards: partex_engine::nodelist::NodeList::new(),
            par_loc: 0,
            par_token: 0,
            write_loc: 0,
            mltex_enabled_p: false,
            enctex_enabled_p: false,
            interrupt: 0,
            ok_to_interrupt: true,
            halting_on_error: false,
            edit_request: None,
            save_stack: crate::journal::JVec::from_elem(MemoryWord::default(), save_size + 1),
            save_ptr: 0,
            save_eqtb: Vec::new(),
            save_obj: Vec::new(),
            max_save_stack: 0,
            cur_level: LEVEL_ONE,
            cur_group: BOTTOM_LEVEL,
            cur_boundary: 0,
            mag_set: 0,
            cur_cmd: 0,
            cur_chr: 0,
            cur_cs: 0,
            cur_tok: 0,
            input_stack: vec![InStateRecord::default(); stack_size + 1],
            input_ptr: 0,
            max_in_stack: 0,
            cur_input: InStateRecord::default(),
            in_open: 0,
            open_parens: 0,
            synctex_tags: 0,
            synctex_flags: 0,
            synctex_root: None,
            synctex_shipped: false,
            input_file: (0..=max_in_open).map(|_| None).collect(),
            line_stack: vec![0; max_in_open + 1],
            grp_stack: vec![0; max_in_open + 1],
            if_stack: vec![0; max_in_open + 1],
            eof_seen: vec![false; max_in_open + 1],
            pseudo_files: Vec::new(),
            input_values: crate::input::InputValues::default(),
            source_filename_stack: vec![0; max_in_open + 1],
            full_source_filename_stack: vec![0; max_in_open + 1],
            scanner_status: NORMAL,
            warning_index: NULL,
            def_ref: Vec::new(),
            def_protected: false,
            param_stack: vec![None; param_size + 1],
            param_ptr: 0,
            max_param_stack: 0,
            align_state: 1_000_000,
            base_ptr: 0,
            expand_depth_count: 0,
            cur_val_level: 0,
            radix: 0,
            cur_order: 0,
            dead_cycles: 0,
            out_rtl: false,
            lr_problems: 0,
            last_badness: 0,
            output_active: false,
            read_file: Default::default(),
            read_open: [crate::toklists::CLOSED; 17],
            cur_name: 0,
            cur_area: 0,
            cur_ext: 0,
            area_delimiter: 0,
            ext_delimiter: 0,
            quoted_filename: false,
            stop_at_space: true,
            name_of_file: Vec::new(),
            format_ident: 0,
            format_data: None,
            force_eof: false,
            long_state: 0,
            cond_stack: crate::conds::CondStack::default(),
            if_limit: NORMAL,
            cur_if: 0,
            if_line: 0,
            skip_line: 0,
            cur_mark: BTreeMap::default(),
            job_name: 0,
            log_opened: false,
            name_in_progress: false,
            output_file_name: 0,
            log_name: 0,
            fmem_ptr: 0,
            font_ptr: 0,
            dvi: crate::dvi::DviState::default(),
            write_open: [false; 18],
            pack_begin_line: 0,
            adjust: None,
            hyph: crate::hyph::HyphState::new(p.trie_size, p.hyph_size, p.ini),
            page: partex_engine::builder::Builder::default(),
            align: crate::align::AlignState::default(),
            random: crate::random::Randoms::default(),
            etex_mode: false,
            epoch: (0, 0),
            is_in_csname: false,
            xregs: crate::xregs::ExtRegs::new(p.fast & crate::params::FAST_XREGS != 0),
            prims: crate::prim::PrimTable::default(),
            tok_pool: Vec::new(),
            pstack_buf: Vec::new(),
            empty_list: partex_engine::node::TokenList::shared(&[]),
            omit_list: partex_engine::node::TokenList::shared(&[crate::web::END_TEMPLATE_TOKEN]),
            arg_list: Vec::new(),
            arg_active: false,
            token_only: false,
            preamble_list: Vec::new(),
            preamble_active: false,
            max_reg_num: 255,
            max_reg_help_line: b"A register number must be between 0 and 255.",
            cur_box: None,
            after_token: 0,
            prev_class: 0,
            space_class: 0,
            default_input: 0,
            memo: crate::memo::Memo::new(p.memo),
            cs_cache: crate::hash::CsCache::default(),
            hash_memo: crate::hashmemo::HashMemo::default(),
            line_log: Vec::new(),
            log_lines: false,
            glyphs_used: Vec::new(),
            record_objstms: false,
            objstms_written: Vec::new(),
            glyphs_read: false,
            skip_tracked: false,
            class_hash: 0,
            classes_read: false,
            classes_written: false,
            seals: crate::seal::SealTable::default(),
            seal_lines: false,
            canon_strings: false,
            cs_by_name: false,
            name_cells: false,
            name_log: Vec::new(),
            font_cells: false,
            font_log: Vec::new(),
            probe_names: false,
            #[cfg(feature = "std")]
            shared_names: None,
            seal_at: (0, 0),
            seal_log: Vec::new(),
            stop_before_ship: false,
            ship_stop: 0,
            stop_after_ship: false,
            stop_after_load: false,
            load_stop: 0,
            defer_page: false,
            page_pending: false,
            graf_stop: false,
            step_began: 0,
            par_start: false,
            fire_pending: false,
            defer_fire: false,
            tags_on: false,
            cur_raw: 0,
            tag_pending: 0,
            want_origin: false,
            int_origin_req: false,
            cur_val_origin: crate::reloc::NO_ORIGIN,
            expr_origin: crate::reloc::NO_ORIGIN,
            affine_count: 0,
            window: 0,
            window_start: 0,
            window_cut: None,
            fresh_def: false,
            long_help_seen: false,
            cancel_boundary: false,
            checkpoint_every: 0,
            commands: 0,
            stop_at: 0,
            stop_at_candidate: false,
            at_checkpoint: false,
            checkpoint_at: (0, -1),
            block_entered: (-1, 0),
            dense: false,
            dense_at: (0, 0),
            dense_last: false,
            shipped: 0,
            tounicode: crate::pdfconv::ToUnicodeTable::default(),
            pdf: crate::pdf::PdfState::default(),
            fontmap: crate::fontmap::FontMap::default(),
            map_cache: crate::fontmap::MapCache::default(),
            fonts_mapped: crate::pdf::val::VSet::default(),
            streams: crate::streams::Streams::default(),
            fonts: FontArrays::new(p.font_max),
            diag: p.diagnostics.then(DiagState::default),
            effects: None,
            org: None,
            sync: None,
            dl: None,
            tap: None,
            params: p,
        }
    }

    /// The host, for callers that drive the engine.
    pub fn host(&self) -> &H {
        &self.host
    }

    /// The host, for callers that drive the engine.
    pub fn host_mut(&mut self) -> &mut H {
        &mut self.host
    }

    /// The DVI writer, from the first page shipped out on (unless the
    /// host's page sink has it).
    pub fn dvi_writer(&self) -> Option<&crate::dviout::DviWriter> {
        self.dvi.writer.as_deref()
    }

    /// Replace the DVI writer by one that has written the same pages
    /// (another run's, spliced in by the host). Nothing happens if there
    /// is no writer yet.
    pub fn set_dvi_writer(&mut self, writer: crate::dviout::DviWriter) {
        if let Some(w) = self.dvi.writer.as_mut() {
            *w = crate::pdf::val::Val::new(writer);
        }
    }
}

/// A checkpoint can go to another thread, and be read from several at
/// once (DESIGN.md §5.3, §7.0: `Machine: Send + Sync`), when its host and
/// tracker can: nothing in the engine's own state is tied to one (no
/// `Cell`, `RefCell` or `Rc`; `&self` hooks update relaxed atomics).
const _: () = {
    const fn assert_send_sync<X: Send + Sync>() {}
    const fn engine<H: Host + Send + Sync, T: Tracker + Send + Sync>() {
        assert_send_sync::<Tex<H, T>>();
    }
};
