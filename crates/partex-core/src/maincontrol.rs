//! Part 46: The chief executive (§1029–§1054), and the `main_control`
//! half of part 53, Extensions (§1347–§1354, §1375–§1377).

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;

use partex_engine::font::{Font, Glyph, LigKern, Tag};
use partex_engine::math::{Item, Kind, Noad};
use partex_engine::node::{GlueSpec, Ligature, Node, Whatsit};
use partex_engine::origin::{Org, Side};

use crate::build::{LEADER_FLAG, norm_min};
use crate::cmds::*;
use crate::fonts::{NON_CHAR, font_id, fx};
use crate::host::Host;
use crate::input::ux;
use crate::mathmode::{NUCLEUS, noad_kind};
use crate::nest::IGNORE_DEPTH;
use crate::nodes::{new_glue, new_kern, subtype};
use crate::tex::{Jump, Tex};
use crate::track::Tracker;

/// The labels of `main_control` (§1030) that are jumped to.
#[derive(Clone, Copy)]
enum L {
    BigSwitch,
    Reswitch,
    MainLoop,
    Wrapup,
    Move,
    Move1,
    Move2,
    MoveLig,
    Lookahead,
    Lookahead1,
    LigLoop,
    LigLoop1,
    LigLoop2,
    AppendNormalSpace,
}

/// §1034: an entry of `lig_stack`: the lookahead character (a character
/// node in tex.web), or a ligature item made by a `|=:` instruction, with
/// the lookahead character it replaced (`lig_ptr`), if any; each
/// character with its origin (`srcmap.rs`; none when origins are off).
#[derive(Clone, Copy)]
enum Lig {
    Char(u8, Org),
    Item { ch: i32, ptr: Option<(u8, Org)> },
}

impl Lig {
    /// `character(lig_stack)`
    fn ch(self) -> i32 {
        match self {
            Lig::Char(c, _) => i32::from(c),
            Lig::Item { ch, .. } => ch,
        }
    }
}

/// §1032: the inner loop's registers, with the ligature cursor of §907.
struct Main {
    f: i32,
    font: Arc<Font>,
    /// `main_i`: the metrics of `cur_l`.
    i: Glyph,
    /// `main_j`, at index `main_k` of the lig/kern program.
    j: LigKern,
    k: usize,
    bchar: i32,
    false_bchar: i32,
    ins_disc: bool,
    cur_l: i32,
    cur_r: i32,
    /// How many characters follow `cur_q` (they end the current list).
    after_q: usize,
    lig_stack: Vec<Lig>,
    ligature_present: bool,
    lft_hit: bool,
    rt_hit: bool,
    /// The origin of the character read last (a ligature with no
    /// characters of its own takes it).
    last_org: Org,
}

impl<H: Host, T: Tracker> Tex<H, T> {
    /// Post the commands run, and the file and line being read, to the
    /// progress board (`progress.rs`): what a terminal shows, read from
    /// the fields themselves (not a read of the command's).
    #[cold]
    #[inline(never)]
    fn post_progress(&self) {
        let board = &crate::progress::BOARD;
        board.commands(crate::progress::EVERY);
        if let Some(f) = self.input_file.get(self.in_open).and_then(Option::as_ref) {
            board.reading(&f.name, self.line);
        }
    }

    /// §1030: governs TeX's activities.
    pub(crate) fn main_control(&mut self) -> Result<(), Jump> {
        let mut m = Main {
            f: 0,
            font: self.fonts.metrics[0].clone(),
            i: Glyph::default(),
            j: LigKern {
                skip: 0,
                next: 0,
                op: 0,
                remainder: 0,
            },
            k: 0,
            bchar: NON_CHAR,
            false_bchar: NON_CHAR,
            ins_disc: false,
            cur_l: 0,
            cur_r: 0,
            after_q: 0,
            lig_stack: Vec::new(),
            ligature_present: false,
            lft_hit: false,
            rt_hit: false,
            last_org: Org::NONE,
        };
        let mut l = L::BigSwitch;
        loop {
            l = match l {
                L::BigSwitch => {
                    // Between commands the whole state is in `self`: a
                    // clean place to stop for a checkpoint and resume.
                    if self.at_checkpoint {
                        // (resumed: this command was counted)
                        self.at_checkpoint = false;
                        self.load_stop = 0;
                        if core::mem::take(&mut self.graf_stop) {
                            // (a paragraph's lines go on here, on the line
                            // its level began on (`CleanPoint::Graf`, or the
                            // page builder `new_graf` deferred), its
                            // `mode_line`. The step takes it as its own: the
                            // one before kept a number that is not where the
                            // paragraph is after an edit above it moved its
                            // lines, and only the over/underfull boxes'
                            // messages read it)
                            self.set_ml(self.line);
                        }
                        if self.page_pending {
                            // (a step that begins at a deferred page
                            // builder begins with it and ends after it: at
                            // a fire it decides, at a clean point)
                            self.page_pending = false;
                            self.build_page()?;
                            if self.fire_pending || self.candidate_due()? {
                                self.at_checkpoint = true;
                                return Err(Jump::Checkpoint);
                            }
                        }
                        if self.fire_pending {
                            // (a step that begins at a fire begins with
                            // it; then the test below is made again,
                            // DESIGN §7.16.1)
                            self.fire_deferred()?;
                            if self.candidate_due()? {
                                self.at_checkpoint = true;
                                return Err(Jump::Checkpoint);
                            }
                        }
                    } else {
                        self.commands += 1;
                        if self.commands.is_multiple_of(crate::progress::EVERY) {
                            self.post_progress();
                        }
                        if T::VALUES {
                            // (the fields themselves: an observer's
                            // reads are not the command's)
                            let outer = self.nest.is_empty()
                                && self.cur_list.mode == VMODE
                                && self.cur_list.list.is_empty()
                                && !self.output_active
                                && self.cur_input.state != crate::web::TOKEN_LIST;
                            self.tracker.command(
                                self.commands,
                                self.in_open,
                                self.line,
                                self.cur_level,
                                outer,
                            );
                            if self.tracker.stop_due(self.commands) {
                                self.at_checkpoint = true;
                                return Err(Jump::Checkpoint);
                            }
                        }
                        if self.load_stop == 1 {
                            // (the command before read a file whole: a step
                            // boundary, `CleanPoint::Load`)
                            self.load_stop = 2;
                            self.at_checkpoint = true;
                            return Err(Jump::Checkpoint);
                        }
                        if self.page_pending {
                            // (the page builder a paragraph's start or end
                            // deferred: a step boundary, `CleanPoint::Page`)
                            self.at_checkpoint = true;
                            return Err(Jump::Checkpoint);
                        }
                        if self.candidate_due()? {
                            self.at_checkpoint = true;
                            return Err(Jump::Checkpoint);
                        }
                    }
                    // (macros expanded right here are memo candidates:
                    // nothing else is collecting their tokens)
                    self.memo.top = true;
                    self.origin_fetch();
                    let r = self.get_command();
                    self.memo.top = false;
                    r?;
                    if self.stop_before_ship
                        && self.cur_cmd == LEADER_SHIP
                        && self.cur_chr == A_LEADERS - 1
                    {
                        if self.ship_stop == 1 {
                            // (the `\shipout` stopped before, read again)
                            self.ship_stop = 0;
                        } else {
                            self.back_input()?;
                            self.ship_stop = 1;
                            self.at_checkpoint = true;
                            return Err(Jump::Checkpoint);
                        }
                    }
                    L::Reswitch
                }
                L::Reswitch => {
                    // §1031: give diagnostic information, if requested.
                    if self.interrupt != 0 && self.ok_to_interrupt {
                        self.back_input()?;
                        self.check_interrupt()?;
                        L::BigSwitch
                    } else {
                        if self.int_par(TRACING_COMMANDS_CODE) > 0 {
                            self.show_cur_cmd_chr();
                        }
                        match self.big_case()? {
                            Some(l) => l,
                            None => return Ok(()),
                        }
                    }
                }
                L::MainLoop => self.main_loop_start(&mut m)?,
                L::Wrapup => {
                    // §1035
                    let z = m.rt_hit;
                    self.wrapup(&mut m, z);
                    L::Move
                }
                L::Move => {
                    // §1036
                    match m.lig_stack.last() {
                        None => L::Reswitch,
                        Some(top) => {
                            m.after_q = 0; // `cur_q:=tail`
                            m.cur_l = top.ch();
                            L::Move1
                        }
                    }
                }
                L::Move1 => {
                    if let Some(Lig::Char(..)) = m.lig_stack.last() {
                        L::Move2
                    } else {
                        L::MoveLig
                    }
                }
                L::Move2 => {
                    let e = self.effective_char(false, m.f, self.cur_chr);
                    let exists = e <= m.font.ec && e >= m.font.bc && {
                        match self.effective_glyph(m.f, m.cur_l) {
                            Some(g) => {
                                m.i = g;
                                true
                            }
                            None => false,
                        }
                    };
                    if exists {
                        // `main_loop_lookahead` is next
                        if let Some(Lig::Char(c, o)) = m.lig_stack.pop() {
                            self.push_glyph(font_id(m.f), c, o);
                            m.after_q += 1;
                        }
                        L::Lookahead
                    } else {
                        self.char_warning(m.f, self.cur_chr)?;
                        m.lig_stack.clear();
                        L::BigSwitch
                    }
                }
                L::MoveLig => {
                    // §1037: move the cursor past a pseudo-ligature.
                    let main_p = match m.lig_stack.pop() {
                        Some(Lig::Item { ptr, .. }) => ptr,
                        _ => None,
                    };
                    if let Some((c, o)) = main_p {
                        // append a single character
                        self.push_glyph(font_id(m.f), c, o);
                        m.after_q += 1;
                    }
                    m.i = self.char_metrics(m.f, m.cur_l);
                    m.ligature_present = true;
                    match m.lig_stack.last() {
                        None if main_p.is_none() => {
                            m.cur_r = m.bchar;
                            L::LigLoop
                        }
                        None => L::Lookahead,
                        Some(top) => {
                            m.cur_r = top.ch();
                            L::LigLoop
                        }
                    }
                }
                L::Lookahead => {
                    // §1038: look ahead for another character, or leave
                    // `lig_stack` empty if there's none there.
                    self.origin_fetch();
                    self.get_next()?; // set only `cur_cmd` and `cur_chr`, for speed
                    if matches!(self.cur_cmd, LETTER | OTHER_CHAR | CHAR_GIVEN) {
                        L::Lookahead1
                    } else {
                        self.x_token()?; // now expand and set `cur_cmd`, `cur_chr`, `cur_tok`
                        if matches!(self.cur_cmd, LETTER | OTHER_CHAR | CHAR_GIVEN) {
                            L::Lookahead1
                        } else if self.cur_cmd == CHAR_NUM {
                            self.char_num_pending();
                            self.scan_char_num()?;
                            self.cur_chr = self.cur_val;
                            L::Lookahead1
                        } else {
                            if self.cur_cmd == NO_BOUNDARY {
                                m.bchar = NON_CHAR;
                            }
                            m.cur_r = m.bchar;
                            m.lig_stack.clear();
                            L::LigLoop
                        }
                    }
                }
                L::Lookahead1 => {
                    self.adjust_space_factor();
                    if !matches!(self.check_for_inter_char_toks()?, crate::xmain::Inter::None) {
                        m.lig_stack.clear();
                        l = L::BigSwitch;
                        continue;
                    }
                    m.lig_stack.clear();
                    m.last_org = self.char_org();
                    m.lig_stack.push(Lig::Char(
                        u8::try_from(self.cur_chr).unwrap_or(0),
                        m.last_org,
                    ));
                    m.cur_r = self.cur_chr;
                    if m.cur_r == m.false_bchar {
                        m.cur_r = NON_CHAR; // this prevents spurious ligatures
                    }
                    L::LigLoop
                }
                L::LigLoop => {
                    // §1039: if there's a ligature/kern command relevant to
                    // `cur_l` and `cur_r`, adjust the text appropriately;
                    // exit to `main_loop_wrapup`.
                    match m.i.tag {
                        Tag::Lig(start) if m.cur_r != NON_CHAR => {
                            m.k = usize::from(start);
                            m.j = m.font.lig_kerns[m.k];
                            if m.j.skip <= LigKern::STOP {
                                L::LigLoop2
                            } else {
                                m.k = 256 * usize::from(m.j.op) + usize::from(m.j.remainder);
                                L::LigLoop1
                            }
                        }
                        _ => L::Wrapup,
                    }
                }
                L::LigLoop1 => {
                    m.j = m.font.lig_kerns[m.k];
                    L::LigLoop2
                }
                L::LigLoop2 => {
                    if i32::from(m.j.next) == m.cur_r && m.j.skip <= LigKern::STOP {
                        self.lig_kern_command(&mut m)?
                    } else if m.j.skip == 0 {
                        m.k += 1;
                        L::LigLoop1
                    } else if m.j.skip >= LigKern::STOP {
                        L::Wrapup
                    } else {
                        m.k += usize::from(m.j.skip) + 1;
                        L::LigLoop1
                    }
                }
                L::AppendNormalSpace => {
                    if self.check_for_post_char_toks()? {
                        l = L::BigSwitch;
                        continue;
                    }
                    // §1041: append a normal inter-word space to the
                    // current list, then `goto big_switch`.
                    let g = if self.glue_par(SPACE_SKIP_CODE).shared_zero {
                        new_glue(self.font_space_glue())
                    } else {
                        self.new_param_glue(SPACE_SKIP_CODE)
                    };
                    self.tail_append(g);
                    L::BigSwitch
                }
            };
        }
    }

    /// merged source: `MLTeX`'s `effective_char_info`: the metrics of `c`
    /// or of its substitution's base character, if either exists.
    fn effective_glyph(&self, f: i32, c: i32) -> Option<Glyph> {
        self.font_read(f, crate::track::font::METRICS);
        let font = self.fonts.get(f);
        if let Some(g) = font.glyph(c) {
            return Some(g);
        }
        if !self.mltex_enabled_p {
            return None;
        }
        if c >= self.int_par(CHAR_SUB_DEF_MIN_CODE)
            && c <= self.int_par(CHAR_SUB_DEF_MAX_CODE)
            && self.equiv(CHAR_SUB_CODE_BASE + c) > 0
        {
            return font.glyph(self.equiv(CHAR_SUB_CODE_BASE + c) % 256);
        }
        None
    }

    /// §1034: `adjust_space_factor` (`XeTeX`'s classes apart).
    pub(crate) fn adjust_space_factor(&mut self) {
        let main_s = self.sf_code(self.cur_chr) % 0x1_0000;
        if main_s == 1000 {
            self.set_space_factor(1000);
        } else if main_s < 1000 {
            if main_s > 0 {
                self.set_space_factor(main_s);
            }
        } else if self.space_factor() < 1000 {
            self.set_space_factor(1000);
        } else {
            self.set_space_factor(main_s);
        }
    }

    /// §1034: append character `cur_chr` and the following characters (if
    /// any) to the current hlist in the current font.
    fn main_loop_start(&mut self, m: &mut Main) -> Result<L, Jump> {
        // (`insert_src_special_auto` is off in `tex`.)
        self.begin_char_run();
        if self.params.flavor == crate::params::Flavor::XeTeX
            && self.is_native_font(self.cur_font())
        {
            return Ok(if self.main_loop_native()? {
                L::BigSwitch
            } else {
                L::Reswitch
            });
        }
        self.adjust_space_factor();
        if !matches!(self.check_for_inter_char_toks()?, crate::xmain::Inter::None) {
            return Ok(L::BigSwitch);
        }
        m.f = self.cur_font();
        // (the font's metrics, a shared value, read once for the run of
        // characters)
        self.font_read(m.f, crate::track::font::METRICS);
        m.font = self.fonts.metrics[fx(m.f)].clone();
        m.bchar = m.font.bchar;
        m.false_bchar = m.font.false_bchar;
        if self.mode() > 0 && self.int_par(LANGUAGE_CODE) != self.clang() {
            self.fix_language();
        }
        m.lig_stack.clear();
        m.last_org = self.char_org();
        m.lig_stack.push(Lig::Char(
            u8::try_from(self.cur_chr).unwrap_or(0),
            m.last_org,
        ));
        m.cur_l = self.cur_chr;
        m.after_q = 0; // `cur_q:=tail`
        let label = if self.cancel_boundary {
            self.cancel_boundary = false;
            None
        } else {
            m.font.bchar_label
        };
        let Some(k) = label else {
            return Ok(L::Move2); // no left boundary processing
        };
        m.k = usize::from(k);
        m.cur_r = m.cur_l;
        m.cur_l = NON_CHAR;
        Ok(L::LigLoop1) // begin with cursor after left boundary
    }

    /// §1035: `pack_lig(z)`: the characters after `cur_q` become a
    /// ligature.
    fn pack_lig(&mut self, m: &mut Main, z: bool) {
        let on = self.origins_on();
        // (the entries of the characters' origins, with origins on)
        let mut entries: Vec<u32> = Vec::new();
        let list = self.nodes_mut();
        let mut original = Vec::with_capacity(m.after_q);
        for _ in 0..m.after_q {
            match list.last() {
                Some(Node::Glyphs(g)) if g.chars().len() > 1 => {
                    if on && g.org() != 0 {
                        entries.push(g.org() + u32::try_from(g.chars().len() - 1).unwrap_or(0));
                    }
                    // (the run made again, one character shorter)
                    let c = list.edit_last(|n| match n {
                        Node::Glyphs(g) => g.pop(),
                        _ => None,
                    });
                    original.extend(c.flatten());
                }
                Some(Node::Glyphs(g)) => {
                    if on && g.org() != 0 {
                        entries.push(g.org());
                    }
                    original.push(g.chars()[0]);
                    list.pop();
                }
                _ => break,
            }
        }
        original.reverse();
        let org = if on {
            let mut u = Org::NONE;
            for e in entries {
                let o = self.org_at(e);
                u = self.org_union(u, o);
            }
            if u.is_none() {
                u = m.last_org;
            }
            Side(self.org_handle(u))
        } else {
            Side(0)
        };
        let list = self.nodes_mut();
        let mut subtype = 0;
        if m.lft_hit {
            subtype = 2;
            m.lft_hit = false;
        }
        if z && m.lig_stack.is_empty() {
            subtype += 1;
            m.rt_hit = false;
        }
        list.push(Node::Ligature(Box::new(Ligature {
            font: font_id(m.f),
            ch: u8::try_from(m.cur_l).unwrap_or(0),
            subtype,
            original,
            org,
        })));
        m.after_q = 0; // the ligature is not a character
        m.ligature_present = false;
    }

    /// §1035: `wrapup(z)`.
    fn wrapup(&mut self, m: &mut Main, z: bool) {
        if m.cur_l < NON_CHAR {
            if m.after_q > 0
                && let Some(Node::Glyphs(g)) = self.nodes().last()
                && {
                    self.font_read(m.f, crate::track::font::HYPHEN_CHAR);
                    i32::from(g.chars()[g.chars().len() - 1]) == self.fonts.hyphen_char[fx(m.f)]
                }
            {
                m.ins_disc = true;
            }
            if m.ligature_present {
                self.pack_lig(m, z);
            }
            if m.ins_disc {
                m.ins_disc = false;
                if self.mode() > 0 {
                    self.tail_append(Node::Disc(Box::default()));
                }
            }
        }
    }

    /// §1040: do ligature or kern command, returning to `main_lig_loop` or
    /// `main_loop_wrapup` or `main_loop_move`.
    fn lig_kern_command(&mut self, m: &mut Main) -> Result<L, Jump> {
        let op = m.j.op;
        if op >= LigKern::KERN {
            let z = m.rt_hit;
            self.wrapup(m, z);
            let k = 256 * usize::from(op - LigKern::KERN) + usize::from(m.j.remainder);
            self.tail_append(new_kern(m.font.kerns[k]));
            return Ok(L::Move);
        }
        if m.cur_l == NON_CHAR {
            m.lft_hit = true;
        } else if m.lig_stack.is_empty() {
            m.rt_hit = true;
        }
        self.check_interrupt()?; // allow a way out in case there's an infinite ligature loop
        let rem = i32::from(m.j.remainder);
        match op {
            1 | 5 => {
                // `=:|`, `=:|>`
                m.cur_l = rem;
                m.i = self.char_metrics(m.f, m.cur_l);
                m.ligature_present = true;
            }
            2 | 6 => {
                // `|=:`, `|=:>`
                m.cur_r = rem;
                match m.lig_stack.last_mut() {
                    None => {
                        // right boundary character is being consumed
                        m.lig_stack.push(Lig::Item { ch: rem, ptr: None });
                        m.bchar = NON_CHAR;
                    }
                    Some(top @ Lig::Char(..)) => {
                        // `link(lig_stack)=null`
                        let Lig::Char(c, o) = *top else {
                            unreachable!()
                        };
                        *top = Lig::Item {
                            ch: rem,
                            ptr: Some((c, o)),
                        };
                    }
                    Some(Lig::Item { ch, .. }) => *ch = rem,
                }
            }
            3 => {
                // `|=:|`
                m.cur_r = rem;
                m.lig_stack.push(Lig::Item { ch: rem, ptr: None });
            }
            7 | 11 => {
                // `|=:|>`, `|=:|>>`
                self.wrapup(m, false);
                m.after_q = 0; // `cur_q:=tail`
                m.cur_l = rem;
                m.i = self.char_metrics(m.f, m.cur_l);
                m.ligature_present = true;
            }
            _ => {
                // `=:`
                m.cur_l = rem;
                m.ligature_present = true;
                return Ok(if m.lig_stack.is_empty() {
                    L::Wrapup
                } else {
                    L::Move1
                });
            }
        }
        if op > 4 && op != 7 {
            return Ok(L::Wrapup);
        }
        if m.cur_l < NON_CHAR {
            return Ok(L::LigLoop);
        }
        // (tex.web's `bchar_label` is `non_address`, 0, if there is none)
        m.k = m.font.bchar_label.map_or(0, usize::from);
        Ok(L::LigLoop1)
    }

    /// §1042: the glue specification for text spaces in the current font.
    fn font_space_glue(&mut self) -> GlueSpec {
        let f = self.cur_font();
        self.tracker.read(crate::track::Cell::Font(f));
        self.font_read(f, crate::track::font::GLUE);
        if let Some(g) = self.fonts.glue[fx(f)] {
            return g;
        }
        self.font_read(f, crate::track::font::PARAMS);
        let font = self.fonts.get(f);
        let g = GlueSpec {
            width: font.param(ux(SPACE_CODE)),
            stretch: font.param(ux(SPACE_STRETCH_CODE)),
            shrink: font.param(ux(SPACE_SHRINK_CODE)),
            ..GlueSpec::default()
        };
        self.fonts.glue[fx(f)] = Some(g);
        self.font_wrote(f, crate::track::font::GLUE);
        g
    }

    /// §1043: handle spaces when `space_factor<>1000`.
    fn app_space(&mut self) {
        let q = if self.space_factor() >= 2000 && !self.glue_par(XSPACE_SKIP_CODE).shared_zero {
            self.new_param_glue(XSPACE_SKIP_CODE)
        } else {
            let mut main_p = if self.glue_par(SPACE_SKIP_CODE).shared_zero {
                self.font_space_glue()
            } else {
                self.glue_par(SPACE_SKIP_CODE)
            }
            .copy();
            // §1044: modify the glue specification in `main_p` according
            // to the space factor.
            let sf = self.space_factor();
            if sf >= 2000 {
                main_p.width += self.font_param(EXTRA_SPACE_CODE, self.cur_font());
            }
            main_p.stretch = self.xn_over_d(main_p.stretch, sf, 1000);
            main_p.shrink = self.xn_over_d(main_p.shrink, 1000, sf);
            new_glue(main_p)
        };
        self.tail_append(q);
    }

    /// The big `case abs(mode)+cur_cmd` of §1030 and §1045. Returns the
    /// next label, or `None` for `return` (the end of `main_control`).
    fn big_case(&mut self) -> Result<Option<L>, Jump> {
        if self.memo.recording() && !self.memo_command_ok(self.cur_cmd) {
            self.memo.impure_cmd(self.cur_cmd);
        }
        let mode = self.mode().abs();
        match (mode, self.cur_cmd) {
            (HMODE, LETTER | OTHER_CHAR | CHAR_GIVEN) => return Ok(Some(L::MainLoop)),
            (HMODE, CHAR_NUM) => {
                self.char_num_pending();
                if self.params.flavor == crate::params::Flavor::XeTeX {
                    self.scan_usv_num()?;
                } else {
                    self.scan_char_num()?;
                }
                self.cur_chr = self.cur_val;
                return Ok(Some(L::MainLoop));
            }
            (HMODE, NO_BOUNDARY) => {
                self.get_x_token()?;
                if matches!(self.cur_cmd, LETTER | OTHER_CHAR | CHAR_GIVEN | CHAR_NUM) {
                    self.cancel_boundary = true;
                }
                return Ok(Some(L::Reswitch));
            }
            (HMODE, _) if self.check_for_post_char_toks()? => return Ok(Some(L::BigSwitch)),
            (HMODE, SPACER) => {
                if self.space_factor() == 1000 {
                    return Ok(Some(L::AppendNormalSpace));
                }
                self.app_space();
            }
            (HMODE | MMODE, EX_SPACE) => return Ok(Some(L::AppendNormalSpace)),
            // §1045: cases of `main_control` that are not part of the
            // inner loop.
            // (`any_mode(relax)`, `vmode+spacer`, `mmode+spacer` and
            // `mmode+no_boundary` do nothing: the final `_` arm.)
            (_, IGNORE_SPACES) if self.cur_chr == 1 => {
                // pdfTeX §1223: `\pdfprimitive` of an unexpandable
                // primitive: its primitive meaning.
                let t = self.scanner_status;
                self.scanner_status = NORMAL;
                self.get_next()?;
                self.scanner_status = t;
                let p = self.prim_of_cur_cs()?;
                if p != UNDEFINED_PRIMITIVE {
                    (self.cur_cmd, self.cur_chr) = self.prim_meaning(p);
                    self.cur_tok = CS_TOKEN_FLAG + PRIM_EQTB_BASE + p;
                    return Ok(Some(L::Reswitch));
                }
            }
            (_, IGNORE_SPACES) => {
                // §406: get the next non-blank non-call token.
                loop {
                    self.get_x_token()?;
                    if self.cur_cmd != SPACER {
                        break;
                    }
                }
                return Ok(Some(L::Reswitch));
            }
            (VMODE, STOP) => {
                if self.its_all_over()? {
                    return Ok(None); // this is the only way out
                }
            }
            // §1048: forbidden cases detected in `main_control`.
            // (with the additions of §1098, §1111 and §1144)
            (VMODE, VMOVE | VADJUST | ITAL_CORR)
            | (HMODE | MMODE, HMOVE)
            | (VMODE | HMODE, EQ_NO)
            | (_, LAST_ITEM | MAC_PARAM) => {
                self.report_illegal_case()?;
            }
            // §1046: math-only cases in non-math modes, or vice versa.
            (
                VMODE | HMODE,
                SUP_MARK | SUB_MARK | MATH_CHAR_NUM | MATH_GIVEN | MATH_COMP | DELIM_NUM
                | LEFT_RIGHT | ABOVE | RADICAL | MATH_STYLE | MATH_CHOICE | VCENTER | NON_SCRIPT
                | MKERN | LIMIT_SWITCH | MSKIP | MATH_ACCENT,
            )
            | (MMODE, ENDV | PAR_END | STOP | VSKIP | UN_VBOX | VALIGN | HRULE) => {
                self.insert_dollar_sign()?;
            }
            // Cases of `main_control` that build boxes and lists (part 47).
            // §1056
            (VMODE, HRULE) | (HMODE | MMODE, VRULE) => {
                let r = self.scan_rule_spec()?;
                self.tail_append(r);
                if mode == VMODE {
                    self.set_prev_depth(IGNORE_DEPTH);
                } else if mode == HMODE {
                    self.set_space_factor(1000);
                }
            }
            // §1057
            (VMODE, VSKIP) | (HMODE | MMODE, HSKIP) | (MMODE, MSKIP) => self.append_glue()?,
            (_, KERN) | (MMODE, MKERN) => self.append_kern()?,
            // §1063
            (VMODE | HMODE, LEFT_BRACE) => self.new_save_level(SIMPLE_GROUP)?,
            (_, BEGIN_GROUP) => self.new_save_level(SEMI_SIMPLE_GROUP)?,
            (_, END_GROUP) => {
                if self.cur_group() == SEMI_SIMPLE_GROUP {
                    self.unsave()?;
                } else {
                    self.off_save()?;
                }
            }
            // §1067
            (_, RIGHT_BRACE) => self.handle_right_brace()?,
            // §1073
            (VMODE, HMOVE) | (HMODE | MMODE, VMOVE) => {
                let t = self.cur_chr;
                self.scan_normal_dimen()?;
                if t == 0 {
                    self.scan_box(self.cur_val)?;
                } else {
                    self.scan_box(-self.cur_val)?;
                }
            }
            (_, LEADER_SHIP) => self.scan_box(LEADER_FLAG - A_LEADERS + self.cur_chr)?,
            (_, MAKE_BOX) => self.begin_box(0)?,
            // §1090
            (VMODE, START_PAR) => self.new_graf(self.cur_chr > 0)?,
            (
                VMODE,
                LETTER | OTHER_CHAR | CHAR_NUM | CHAR_GIVEN | MATH_SHIFT | UN_HBOX | VRULE | ACCENT
                | DISCRETIONARY | HSKIP | VALIGN | EX_SPACE | NO_BOUNDARY,
            ) => {
                self.back_input()?;
                self.new_graf(true)?;
            }
            // §1092
            (HMODE | MMODE, START_PAR) => self.indent_in_hmode(),
            // §1094
            (VMODE, PAR_END) => {
                self.normal_paragraph()?;
                if self.mode() > 0 {
                    self.build_page_after_par()?;
                }
            }
            (HMODE, PAR_END) => {
                if self.align_state() < 0 {
                    self.off_save()?; // this tries to recover from an alignment that didn't end properly
                }
                self.end_graf()?; // this takes us to the enclosing mode, if `mode>0`
                if self.mode() == VMODE {
                    self.build_page_after_par()?;
                }
            }
            (HMODE, STOP | VSKIP | HRULE | UN_VBOX | HALIGN) => self.head_for_vmode()?,
            // §1097
            (_, INSERT) | (HMODE | MMODE, VADJUST) => self.begin_insert_or_adjust()?,
            (_, MARK) => self.make_mark()?,
            // §1102
            (_, BREAK_PENALTY) => self.append_penalty()?,
            // §1104
            (_, REMOVE_ITEM) => self.delete_last()?,
            // §1109
            (VMODE, UN_VBOX) | (HMODE | MMODE, UN_HBOX) => self.unpackage()?,
            // §1112
            (HMODE, ITAL_CORR) => self.append_italic_correction(),
            (MMODE, ITAL_CORR) => self.tail_append(new_kern(0)),
            // §1116
            (HMODE | MMODE, DISCRETIONARY) => self.append_discretionary()?,
            // §1122
            (HMODE, ACCENT) => self.make_accent()?,
            // §1126
            (_, CAR_RET | TAB_MARK) => self.align_error()?,
            (_, NO_ALIGN) => self.no_align_error()?,
            (_, OMIT) => self.omit_error()?,
            // §1130
            // pdfTeX §1703: `\beginL`, `\endL`, `\beginR`, `\endR`.
            (HMODE, VALIGN) if self.cur_chr > 0 => {
                if self.etex_enabled(self.texxet_en(), self.cur_cmd, self.cur_chr)? {
                    self.tail_append(crate::nodes::new_math(0, self.cur_chr));
                }
            }
            (VMODE, HALIGN) | (HMODE, VALIGN) => self.init_align()?,
            (MMODE, HALIGN) => {
                if self.privileged()? {
                    if self.cur_group() == MATH_SHIFT_GROUP {
                        self.init_align()?;
                    } else {
                        self.off_save()?;
                    }
                }
            }
            (VMODE | HMODE, ENDV) => self.do_endv()?,
            // §1134
            (_, END_CS_NAME) => self.cs_error()?,
            // Part 48. §1137
            (HMODE, MATH_SHIFT) => self.init_math()?,
            // §1140
            (MMODE, EQ_NO) => {
                if self.privileged()? {
                    if self.cur_group() == MATH_SHIFT_GROUP {
                        self.start_eq_no()?;
                    } else {
                        self.off_save()?;
                    }
                }
            }
            // §1150
            (MMODE, LEFT_BRACE) => {
                self.push_noad(Noad::new(Kind::Ord));
                self.back_input()?;
                self.scan_math(NUCLEUS)?;
            }
            // §1154
            (MMODE, LETTER | OTHER_CHAR | CHAR_GIVEN) => {
                self.set_math_char(self.math_code(self.cur_chr))?;
            }
            (MMODE, CHAR_NUM) => {
                self.scan_char_num()?;
                self.cur_chr = self.cur_val;
                self.set_math_char(self.math_code(self.cur_chr))?;
            }
            (MMODE, MATH_CHAR_NUM) => {
                self.scan_fifteen_bit_int()?;
                self.set_math_char(self.cur_val)?;
            }
            (MMODE, MATH_GIVEN) => self.set_math_char(self.cur_chr)?,
            (MMODE, DELIM_NUM) => {
                self.scan_twenty_seven_bit_int()?;
                self.set_math_char(self.cur_val / 0o10000)?;
            }
            // §1158
            (MMODE, MATH_COMP) => {
                self.push_noad(Noad::new(noad_kind(self.cur_chr)));
                self.scan_math(NUCLEUS)?;
            }
            (MMODE, LIMIT_SWITCH) => self.math_limit_switch()?,
            // §1162
            (MMODE, RADICAL) => self.math_radical()?,
            // §1164
            (MMODE, ACCENT | MATH_ACCENT) => self.math_ac()?,
            // §1167
            (MMODE, VCENTER) => self.begin_vcenter()?,
            // §1171
            (MMODE, MATH_STYLE) => {
                let s = u8::try_from(self.cur_chr).unwrap_or(0);
                self.mlist_mut().push(Item::Style(s));
            }
            (MMODE, NON_SCRIPT) => self.tail_append(Node::Glue {
                spec: GlueSpec::ZERO_GLUE,
                subtype: subtype(COND_MATH_GLUE),
                sync: partex_engine::origin::Side(0),
            }),
            (MMODE, MATH_CHOICE) => self.append_choices()?,
            // §1175
            (MMODE, SUB_MARK | SUP_MARK) => self.sub_sup()?,
            // §1180
            (MMODE, ABOVE) => self.math_fraction()?,
            // §1190
            (MMODE, LEFT_RIGHT) => self.math_left_right()?,
            // §1193
            (MMODE, MATH_SHIFT) => {
                if self.cur_group() == MATH_SHIFT_GROUP {
                    self.after_math()?;
                } else {
                    self.off_save()?;
                }
            }
            // Cases of `main_control` that don't depend on `mode` (part 49).
            // §1210
            (
                _,
                TOKS_REGISTER | ASSIGN_TOKS | ASSIGN_INT | ASSIGN_DIMEN | ASSIGN_GLUE
                | ASSIGN_MU_GLUE | ASSIGN_FONT_DIMEN | ASSIGN_FONT_INT | SET_AUX | SET_PREV_GRAF
                | SET_PAGE_DIMEN | SET_PAGE_INT | SET_BOX_DIMEN | SET_SHAPE | DEF_CODE
                | XETEX_DEF_CODE | DEF_FAMILY | SET_FONT | DEF_FONT | REGISTER | ADVANCE | MULTIPLY
                | DIVIDE | PREFIX | LET | SHORTHAND_DEF | READ_TO_CS | DEF | SET_BOX | HYPH_DATA
                | SET_INTERACTION | LETTERSPACE_FONT | PDF_COPY_FONT,
            ) => self.prefixed_command()?,
            // §1268
            (_, AFTER_ASSIGNMENT) => {
                self.get_token()?;
                self.set_after_token(self.cur_tok);
            }
            // §1271
            (_, AFTER_GROUP) => {
                self.get_token()?;
                self.save_for_after(self.cur_tok)?;
            }
            // §1274
            (_, IN_STREAM) => self.open_or_close_in()?,
            // §1276
            (_, MESSAGE) => self.issue_message()?,
            // §1285
            (_, CASE_SHIFT) => self.shift_case()?,
            // §1290
            (_, XRAY) => self.show_whatever()?,
            // §1347: cases of `main_control` that are for extensions to TeX.
            (_, EXTENSION) => self.do_extension()?,
            // The remaining combinations cannot occur.
            _ => {}
        }
        Ok(Some(L::BigSwitch))
    }

    /// §1348
    fn do_extension(&mut self) -> Result<(), Jump> {
        match self.cur_chr {
            OPEN_NODE | WRITE_NODE | CLOSE_NODE | SPECIAL_NODE => {
                // The node is the tail while its arguments are scanned
                // (`\lastpenalty` in them sees it), as in tex.web.
                self.tail_append(Node::Whatsit(Box::new(Whatsit::Close { stream: 0 })));
                let w = self.scan_whatsit()?;
                self.pop_tail();
                self.tail_append(Node::Whatsit(Box::new(w)));
            }
            IMMEDIATE_CODE => {
                // §1375: implement \immediate.
                self.get_x_token()?;
                if self.cur_cmd == EXTENSION {
                    // pdfTeX §1623: objects and forms written at once
                    match self.cur_chr {
                        PDF_OBJ_CODE => {
                            self.do_extension()?;
                            let k = self.pdf_last(crate::pdf::PdfLast::Obj);
                            if *self.pdf_obj_aux(k) == crate::pdf::objtab::Aux::None {
                                return self.pdf_error(
                                    b"ext1",
                                    b"`\\pdfobj reserveobjnum' cannot be used with \\immediate",
                                );
                            }
                            return self.pdf_write_obj(k);
                        }
                        PDF_XFORM_CODE => {
                            self.do_extension()?;
                            return self.pdf_immediate_xform();
                        }
                        PDF_XIMAGE_CODE => {
                            self.do_extension()?;
                            let k = self.pdf_last(crate::pdf::PdfLast::XImage);
                            return self.pdf_write_image(k);
                        }
                        _ => {}
                    }
                }
                if self.cur_cmd == EXTENSION && self.cur_chr <= CLOSE_NODE {
                    self.tail_append(Node::Whatsit(Box::new(Whatsit::Close { stream: 0 })));
                    let w = self.scan_whatsit()?;
                    self.pop_tail();
                    self.out_what(&w)?; // do the action immediately
                } else {
                    self.back_input()?;
                }
            }
            SET_LANGUAGE_CODE => {
                // §1377: implement \setlanguage.
                if self.mode().abs() == HMODE {
                    self.tail_append(Node::Whatsit(Box::new(Whatsit::Close { stream: 0 })));
                    self.scan_int()?;
                    let l = if self.cur_val <= 0 || self.cur_val > 255 {
                        0
                    } else {
                        self.cur_val
                    };
                    self.set_clang(l);
                    self.pop_tail();
                    let w = self.language_whatsit(l);
                    self.tail_append(w);
                } else {
                    self.report_illegal_case()?;
                }
            }
            SET_RANDOM_SEED_CODE => {
                // pdfTeX §1581: implement \pdfsetrandomseed (negative
                // seeds silently become positive).
                self.scan_int()?;
                if !T::VALUES {
                    self.tracker.write(crate::track::Cell::Random);
                }
                self.random.random_seed = self.cur_val.wrapping_abs();
                self.random.init_randoms(self.random.random_seed);
                self.random_wrote();
            }
            RESET_TIMER_CODE => {
                // pdfTeX §1582: implement \pdfresettimer (a read of the
                // host's clock).
                let e = self.host.seconds_and_micros();
                self.clock_read(crate::track::Query::Timer, &e);
                self.set_epoch(e);
            }
            PDF_GLYPH_TO_UNICODE_CODE => self.glyph_to_unicode()?, // pdfTeX §1587
            PDF_MAP_FILE_CODE | PDF_MAP_LINE_CODE => {
                // pdfTeX §1590–§1591: implement \pdfmapfile, \pdfmapline.
                let file = self.cur_chr == PDF_MAP_FILE_CODE;
                let name: &[u8] = if file {
                    b"\\pdfmapfile"
                } else {
                    b"\\pdfmapline"
                };
                self.check_pdfoutput(name, true)?;
                self.scan_toks(false, true)?;
                let s = self.take_def_ref_string();
                self.process_map_item(&s, file);
            }
            XETEX_LINEBREAK_LOCALE_EXTENSION_CODE
                if self.params.flavor == crate::params::Flavor::XeTeX =>
            {
                self.xetex_linebreak_locale()?;
            }
            PIC_FILE_CODE | PDF_FILE_CODE if self.params.flavor == crate::params::Flavor::XeTeX => {
                self.implement_picture(self.cur_chr == PDF_FILE_CODE)?;
            }
            c if c >= PDFTEX_FIRST_EXTENSION_CODE => {
                if !self.do_pdf_extension()? {
                    return self.pdf_error(b"ext1", b"not implemented in partex yet");
                }
            }
            _ => self.confusion(b"ext1")?,
        }
        Ok(())
    }

    /// §1351–§1354: scan the arguments of `\openout`, `\write`,
    /// `\closeout` or `\special` (`cur_chr`).
    fn scan_whatsit(&mut self) -> Result<Whatsit, Jump> {
        Ok(match self.cur_chr {
            OPEN_NODE => {
                // §1351: implement \openout.
                let stream = self.scan_write_stream(false)?;
                self.scan_optional_equals()?;
                self.scan_file_name()?;
                let bytes = |t: &Self, s: i32| Arc::from(t.str_bytes(ux(s)));
                Whatsit::Open {
                    stream,
                    name: bytes(self, self.cur_name),
                    area: bytes(self, self.cur_area),
                    ext: bytes(self, self.cur_ext),
                }
            }
            WRITE_NODE => {
                // §1352: implement \write.
                let k = self.cur_cs;
                let stream = self.scan_write_stream(true)?;
                self.cur_cs = k;
                self.scan_toks(false, false)?;
                let tokens = self.take_def();
                Whatsit::Write { stream, tokens }
            }
            CLOSE_NODE => {
                // §1353: implement \closeout.
                let stream = self.scan_write_stream(true)?;
                Whatsit::Close { stream }
            }
            _ => {
                // §1354: implement \special. (encTeX's `\specialout` and
                // `\mubyteout` marks are not kept.) pdfTeX §1534:
                // `\special shipout` is expanded when shipped.
                if self.params.flavor == crate::params::Flavor::PdfTex
                    && self.scan_keyword(b"shipout")?
                {
                    self.scan_toks(false, false)?;
                    let tokens = self.take_def();
                    return Ok(Whatsit::LateSpecial { tokens });
                }
                self.scan_toks(false, true)?;
                let tokens = self.take_def();
                Whatsit::Special { tokens }
            }
        })
    }

    /// §1350: the stream number of `\write` and `\closeout` (`any`), or of
    /// `\openout`.
    fn scan_write_stream(&mut self, any: bool) -> Result<i32, Jump> {
        if any {
            self.scan_int()?;
            if self.cur_val < 0 {
                self.cur_val = 17;
            } else if self.cur_val > 15 && self.cur_val != 18 {
                self.cur_val = 16;
            }
        } else {
            self.scan_four_bit_int()?;
        }
        Ok(self.cur_val)
    }

    /// §1377: a language whatsit for language `l`.
    fn language_whatsit(&self, l: i32) -> Node {
        Node::Whatsit(Box::new(Whatsit::Language {
            language: l,
            left_hyphen_min: u8::try_from(norm_min(self.int_par(LEFT_HYPHEN_MIN_CODE)))
                .unwrap_or(1),
            right_hyphen_min: u8::try_from(norm_min(self.int_par(RIGHT_HYPHEN_MIN_CODE)))
                .unwrap_or(1),
        }))
    }

    /// §1376
    pub(crate) fn fix_language(&mut self) {
        let language = self.int_par(LANGUAGE_CODE);
        let l = if language <= 0 || language > 255 {
            0
        } else {
            language
        };
        if l != self.clang() {
            let w = self.language_whatsit(l);
            self.tail_append(w);
            self.set_clang(l);
        }
    }
}
