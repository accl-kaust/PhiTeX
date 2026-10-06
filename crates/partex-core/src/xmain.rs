//! `XeTeX`'s part of `main_control` (§1034, its "added code for native
//! font support"): the characters of a native font gathered into words,
//! broken after hyphens, and joined to the word before when nothing
//! separates them; and the inter-character token lists
//! (`\XeTeXinterchartoks`) inserted between characters of given classes.

use alloc::boxed::Box;
use alloc::vec::Vec;

use partex_engine::native::NativeWord;
use partex_engine::node::{Node, Whatsit};

use crate::cmds::*;
use crate::fonts::fx;
use crate::host::Host;
use crate::native::font_id;
use crate::tex::{Jump, Tex};
use crate::track::Tracker;

/// What `check_for_inter_char_toks` did.
pub(crate) enum Inter {
    /// No list was inserted.
    None,
    /// The boundary's list was inserted before the run's first character
    /// (`goto big_switch`).
    BigSwitch,
    /// A list was inserted between two characters (`goto` the label given).
    Found,
}

/// The word of a native-word node.
fn native_word(n: &Node) -> Option<&NativeWord> {
    match n {
        Node::Whatsit(w) => match &**w {
            Whatsit::NativeWord(w) => Some(w),
            _ => None,
        },
        _ => None,
    }
}

/// `node_is_invisible_to_interword_space`: penalties, insertions, marks,
/// adjustments, and the whatsits `\openout`, `\write`, `\closeout`,
/// `\special` and language.
fn invisible_to_interword_space(n: &Node) -> bool {
    match n {
        Node::Penalty(_) | Node::Ins(_) | Node::Mark(_) | Node::Adjust(_) => true,
        Node::Whatsit(w) => matches!(
            **w,
            Whatsit::Open { .. }
                | Whatsit::Write { .. }
                | Whatsit::Close { .. }
                | Whatsit::Special { .. }
                | Whatsit::Language { .. }
        ),
        _ => false,
    }
}

/// The length of `text` up to and including its first hyphen (or dash,
/// with `dash`), if it has one.
fn break_after(text: &[u16], hyph: i32, dash: bool) -> Option<usize> {
    let is_break = |u: u16| {
        let u = i32::from(u);
        u == hyph || (dash && (u == 0x2014 || u == 0x2013))
    };
    text.iter().position(|&u| is_break(u)).map(|i| i + 1)
}

impl<H: Host, T: Tracker> Tex<H, T> {
    /// `XeTeX_inter_char_tokens_en`.
    fn inter_char_tokens_en(&self) -> bool {
        self.params.flavor == crate::params::Flavor::XeTeX
            && self.xetex_state(XETEX_INTER_CHAR_TOKENS_CODE) > 0
    }

    /// The list from class `c1` to `c2`, if one is set.
    fn inter_char_toks(&self, c1: i32, c2: i32) -> Option<crate::tok::Tokens> {
        self.equiv_toks(crate::wide::inter_char_loc(c1, c2))
            .cloned()
    }

    /// `prev_class` and `space_class`, written where they change.
    fn put_classes(&mut self, prev: i32, space: i32) {
        if self.prev_class != prev {
            self.set_prev_class(prev);
        }
        if self.space_class != space {
            self.set_space_class(space);
        }
    }

    /// The start of "Append character `cur_chr`…" (`XeTeX`): a run of
    /// characters begins at a boundary.
    pub(crate) fn begin_char_run(&mut self) {
        if self.params.flavor == crate::params::Flavor::XeTeX {
            let space = self.space_class;
            self.put_classes(CHAR_CLASS_BOUNDARY, space);
        }
    }

    /// `check_for_inter_char_toks`: insert the list between the class of
    /// the character before and `cur_chr`'s, the character read again
    /// after it.
    pub(crate) fn check_for_inter_char_toks(&mut self) -> Result<Inter, Jump> {
        if self.params.flavor != crate::params::Flavor::XeTeX {
            return Ok(Inter::None);
        }
        let space = self.sf_code(self.cur_chr) / 0x1_0000;
        let mut prev = self.prev_class;
        if self.inter_char_tokens_en() && space != CHAR_CLASS_IGNORED {
            prev = self.prev_class();
            if prev == CHAR_CLASS_BOUNDARY {
                if (self.state() != TOKEN_LIST || self.token_type() != BACKED_UP_CHAR)
                    && let Some(t) = self.inter_char_toks(CHAR_CLASS_BOUNDARY, space)
                {
                    self.put_classes(prev, space);
                    self.back_char_for_inter_char()?;
                    self.begin_token_list(t, INTER_CHAR_TEXT)?;
                    return Ok(Inter::BigSwitch);
                }
            } else if let Some(t) = self.inter_char_toks(prev, space) {
                self.put_classes(CHAR_CLASS_BOUNDARY, space);
                self.back_char_for_inter_char()?;
                self.begin_token_list(t, INTER_CHAR_TEXT)?;
                return Ok(Inter::Found);
            }
            prev = space;
        }
        self.put_classes(prev, space);
        Ok(Inter::None)
    }

    /// `cur_chr` read again after an inter-character list, as a letter
    /// or other character (`backed_up_char`).
    fn back_char_for_inter_char(&mut self) -> Result<(), Jump> {
        if self.cur_cmd != LETTER {
            self.cur_cmd = OTHER_CHAR;
        }
        self.cur_tok = self.cur_cmd * MAX_CHAR_VAL + self.cur_chr;
        self.back_input()?;
        self.cur_input.index = BACKED_UP_CHAR;
        Ok(())
    }

    /// `check_for_post_char_toks`: after a run of characters, insert the
    /// list from the last one's class to the boundary, the token just
    /// read read again after it. Whether one was.
    pub(crate) fn check_for_post_char_toks(&mut self) -> Result<bool, Jump> {
        if !self.inter_char_tokens_en() {
            return Ok(false);
        }
        let space = self.space_class();
        if space == CHAR_CLASS_IGNORED || self.prev_class() == CHAR_CLASS_BOUNDARY {
            return Ok(false);
        }
        self.put_classes(CHAR_CLASS_BOUNDARY, space);
        let Some(t) = self.inter_char_toks(space, CHAR_CLASS_BOUNDARY) else {
            return Ok(false);
        };
        if self.cur_cs == 0 {
            if self.cur_cmd == CHAR_NUM {
                self.cur_cmd = OTHER_CHAR;
            }
            self.cur_tok = self.cur_cmd * MAX_CHAR_VAL + self.cur_chr;
        } else {
            self.cur_tok = CS_TOKEN_FLAG + self.cur_cs;
        }
        self.back_input()?;
        self.begin_token_list(t, INTER_CHAR_TEXT)?;
        Ok(true)
    }

    /// `hyphen_char[f]`, read.
    fn hyphen_char_of(&self, f: i32) -> i32 {
        self.tracker.read(crate::track::Cell::Font(f));
        self.font_read(f, crate::track::font::HYPHEN_CHAR);
        self.fonts.hyphen_char[fx(f)]
    }

    /// `XeTeX` §1034, the characters of native font `cur_font` from
    /// `cur_chr` on: gathered, then appended as words. Whether to go on
    /// at `big_switch` (an inter-character list was inserted) rather than
    /// at `reswitch` with the token that ended them.
    pub(crate) fn main_loop_native(&mut self) -> Result<bool, Jump> {
        if self.mode() > 0 && self.int_par(LANGUAGE_CODE) != self.clang() {
            self.fix_language();
        }
        let f = self.cur_font();
        let hyph = self.hyphen_char_of(f);
        let mut text: Vec<u16> = Vec::new();
        let mut main_h = 0;
        let mut is_hyph = false;
        let found;
        let dash = self.xetex_state(XETEX_DASH_BREAK_CODE) > 0;
        // collect_native: as many characters as possible in the same font
        loop {
            self.adjust_space_factor();
            match self.check_for_inter_char_toks()? {
                Inter::BigSwitch => return Ok(true),
                Inter::Found => {
                    found = true;
                    break;
                }
                Inter::None => {}
            }
            text.extend(crate::native::utf16_of(self.cur_chr));
            is_hyph = self.cur_chr == hyph || (dash && matches!(self.cur_chr, 0x2013 | 0x2014));
            if main_h == 0 && is_hyph {
                main_h = text.len();
            }
            self.origin_fetch();
            self.get_next()?;
            if matches!(self.cur_cmd, LETTER | OTHER_CHAR | CHAR_GIVEN) {
                continue;
            }
            self.x_token()?;
            if matches!(self.cur_cmd, LETTER | OTHER_CHAR | CHAR_GIVEN) {
                continue;
            }
            if self.cur_cmd == CHAR_NUM {
                self.scan_usv_num()?;
                self.cur_chr = self.cur_val;
                continue;
            }
            found = self.check_for_post_char_toks()?;
            break;
        }
        // collected:
        let Some(nf) = self.native_font(f).cloned() else {
            return Ok(found);
        };
        if nf.font.mapping.is_some() {
            text = nf.font.apply_mapping(&text);
            main_h = break_after(&text, hyph, dash).unwrap_or(0);
        }
        if self.int_par(TRACING_LOST_CHARS_CODE) > 0 {
            for c in char::decode_utf16(text.iter().copied()) {
                let c = c.map_or(0xFFFD, |c| i32::try_from(u32::from(c)).unwrap_or(0));
                if nf.font.map_char_to_glyph(c) == 0 {
                    self.char_warning(f, c)?;
                }
            }
        }
        let mergeable = |t: &Self| {
            let list = t.nodes();
            let n = list.len();
            n >= 2
                && list
                    .last()
                    .and_then(native_word)
                    .is_some_and(|w| w.font == font_id(f))
                && !matches!(list.get(n - 2), Some(Node::Glyphs(_) | Node::Disc(_)))
        };
        if self.mode() == HMODE {
            let mut main_k = text.len();
            let mut temp = 0;
            let mut first = true;
            loop {
                if main_h == 0 {
                    main_h = main_k;
                }
                if first && mergeable(self) {
                    // the word before and this one's first fragment, as one
                    let old = self.nodes_mut().pop();
                    let mut s: Vec<u16> = old
                        .as_ref()
                        .and_then(native_word)
                        .map_or_else(Vec::new, |w| w.text.to_vec());
                    s.extend_from_slice(&text[temp..temp + main_h]);
                    self.do_locale_linebreaks(f, &s)?;
                    main_k = text.len() - main_h - temp;
                    temp = main_h;
                } else {
                    self.do_locale_linebreaks(f, &text[temp..temp + main_h])?;
                    temp += main_h;
                    main_k -= main_h;
                }
                first = false;
                let rest = &text[temp..temp + main_k];
                main_h = break_after(rest, hyph, dash).unwrap_or(rest.len());
                if main_k > 0 || is_hyph {
                    // a break after a hyphen, or after the text's last one
                    self.tail_append(Node::Disc(Box::default()));
                }
                if main_k == 0 {
                    break;
                }
            }
        } else {
            // restricted horizontal mode: no breaks
            let w = if mergeable(self) {
                let old = self.nodes_mut().pop();
                let mut s: Vec<u16> = old
                    .as_ref()
                    .and_then(native_word)
                    .map_or_else(Vec::new, |w| w.text.to_vec());
                s.extend_from_slice(&text);
                self.new_native_word(f, &s)
            } else {
                self.new_native_word(f, &text)
            };
            self.tail_append(Node::Whatsit(Box::new(Whatsit::NativeWord(w))));
        }
        if self.xetex_state(XETEX_INTERWORD_SPACE_SHAPING_CODE) > 0 {
            self.interword_space_shaping(f);
        }
        Ok(found)
    }

    /// `\XeTeXinterwordspaceshaping`: the space between the word just
    /// appended and a word before it in the same font, measured in
    /// context; where it differs from the font's, a kern after the glue
    /// makes up the difference.
    fn interword_space_shaping(&mut self, f: i32) {
        let list = self.nodes();
        let n = list.len();
        let Some(tail) = n.checked_sub(1) else {
            return;
        };
        let Some(pp) = (0..tail)
            .rev()
            .find(|&i| list.get(i).and_then(native_word).is_some())
        else {
            return;
        };
        let Some(word) = list.get(pp).and_then(native_word).cloned() else {
            return;
        };
        if word.font != font_id(f) {
            return;
        }
        let mut p = pp + 1;
        while list.get(p).is_some_and(invisible_to_interword_space) {
            p += 1;
        }
        if !matches!(list.get(p), Some(Node::Glue { .. })) {
            return;
        }
        let mut q = p + 1;
        while list.get(q).is_some_and(invisible_to_interword_space) {
            q += 1;
        }
        if q != tail {
            return;
        }
        let Some(last) = list.get(tail).and_then(native_word).cloned() else {
            return;
        };
        let mut s = word.text.to_vec();
        s.push(u16::from(b' '));
        s.extend_from_slice(&last.text);
        let both = self.new_native_word(f, &s);
        let t = both.width - word.width - last.width;
        // (`width(font_glue[f])`: the font's space if it was made)
        self.font_read(f, crate::track::font::GLUE);
        let space = self.fonts.glue[fx(f)].map_or(0, |g| g.width);
        if t != space {
            self.nodes_mut().insert(
                p + 1,
                Node::Kern {
                    width: t - space,
                    subtype: u8::try_from(SPACE_ADJUSTMENT).unwrap_or(0),
                    sync: partex_engine::origin::Side::default(),
                },
            );
        }
    }

    /// `do_locale_linebreaks`: `text` appended as a native word (as words
    /// with breaks between, `\XeTeXlinebreaklocale` set).
    fn do_locale_linebreaks(&mut self, f: i32, text: &[u16]) -> Result<(), Jump> {
        if self.int_par(XETEX_LINEBREAK_LOCALE_CODE) != 0 && text.len() > 1 {
            return self.pdf_error(b"\\XeTeXlinebreaklocale", b"not implemented in partex yet");
        }
        let w = self.new_native_word(f, text);
        self.tail_append(Node::Whatsit(Box::new(Whatsit::NativeWord(w))));
        Ok(())
    }

    /// `\XeTeXinputencoding` (the current file's encoding, from its next
    /// line) and `\XeTeXdefaultencoding` (that of files opened after).
    pub(crate) fn xetex_encoding(&mut self, default: bool) -> Result<(), Jump> {
        use partex_engine::web::XETEX_INPUT_MODE_AUTO;
        // (`scan_and_pack_name`)
        self.scan_file_name()?;
        self.pack_file_name(self.cur_name, self.cur_area, self.cur_ext);
        let Some(m) = self.encoding_mode_and_info() else {
            let what: &[u8] = if default {
                b"\\XeTeXdefaultencoding"
            } else {
                b"\\XeTeXinputencoding"
            };
            return self.pdf_error(what, b"this ICU converter is not in partex yet");
        };
        if default {
            self.set_default_input(m);
        } else if m == XETEX_INPUT_MODE_AUTO {
            self.print_err(b"Encoding mode `auto' is not valid for \\XeTeXinputencoding");
            self.help(&[
                b"You can't use `auto' encoding here, only for \\XeTeXdefaultencoding.",
                b"I'll ignore this and leave the current encoding unchanged.",
            ]);
            self.error()?;
        } else if let Some(f) = self
            .input_file
            .get_mut(self.in_open)
            .and_then(Option::as_mut)
        {
            f.mode = u8::try_from(m & 0xFF).unwrap_or(0);
            f.conv = u16::try_from(m >> 8).unwrap_or(0);
        }
        Ok(())
    }

    /// `\XeTeXlinebreaklocale`: the locale's name, or 0 (set as `XeTeX`
    /// sets it: in place, not saved).
    pub(crate) fn xetex_linebreak_locale(&mut self) -> Result<(), Jump> {
        self.scan_file_name()?;
        let v = if self.length(crate::input::ux(self.cur_name)) == 0 {
            0
        } else {
            self.cur_name
        };
        self.set_int_par(XETEX_LINEBREAK_LOCALE_CODE, v);
        Ok(())
    }
}

impl<H: Host, T: Tracker> Tex<H, T> {
    /// `XeTeX`'s "Incorporate a whatsit node into an hbox": in a list
    /// being packed, each run of words in one native font, joined by
    /// discretionaries (their breaks not taken), becomes one word,
    /// measured again (its layout across the joins).
    pub(crate) fn merge_native_fragments(&mut self, mut list: Vec<Node>) -> Vec<Node> {
        if self.params.flavor != crate::params::Flavor::XeTeX
            || !list.iter().any(|n| native_word(n).is_some())
        {
            return list;
        }
        let mut i = 0;
        while i < list.len() {
            let Some(font) = native_word(&list[i]).map(|w| w.font) else {
                i += 1;
                continue;
            };
            let same = |n: Option<&Node>| n.and_then(native_word).is_some_and(|w| w.font == font);
            let mut j = i + 1;
            loop {
                if same(list.get(j)) {
                    j += 1;
                } else if let Some(Node::Disc(d)) = list.get(j) {
                    if d.replace.is_empty() {
                        if !same(list.get(j + 1)) {
                            break;
                        }
                        j += 2;
                    } else {
                        // (the chain goes on into the nodes it replaces:
                        // the discretionary goes, they stay)
                        if !same(d.replace.first()) {
                            break;
                        }
                        let Node::Disc(d) = list.remove(j) else {
                            break;
                        };
                        list.splice(j..j, d.replace);
                        j += 1;
                    }
                } else {
                    break;
                }
            }
            if j > i + 1 {
                let mut text: Vec<u16> = Vec::new();
                let mut actual_text = false;
                for (k, n) in list[i..j].iter().enumerate() {
                    if let Some(w) = native_word(n) {
                        if k == 0 {
                            actual_text = w.actual_text;
                        }
                        text.extend_from_slice(&w.text);
                    }
                }
                let mut w = NativeWord::new(font, actual_text, text.into());
                self.measure_native(&mut w);
                list.splice(i..j, [Node::Whatsit(Box::new(Whatsit::NativeWord(w)))]);
            }
            i += 1;
        }
        list
    }
}
