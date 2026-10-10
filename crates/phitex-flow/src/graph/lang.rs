//! Hyphenation on the graph (CORE.md, "Hyphenation"): `\patterns` and
//! `\hyphenation` as assignment nodes defining `Key::Patterns(lang)` and
//! `Key::Exceptions(lang)` (globally: TeX's tables have no save stack), the
//! language whatsits (`\setlanguage`, a change of `\language` in a
//! paragraph, tex.web 1376–1377), and what a paragraph's line breaking
//! reads to hyphenate (`hyph::HyphParams`, tex.web 891–899).

use std::sync::Arc;

use super::{Exec, Mode};
use crate::hyph::{self, HyphParams, LangState, Tok as HTok};
use crate::item::Item;
use crate::tfm::Font;
use crate::val::{IntPar, Key, Meaning, Prim, Val};

impl Exec<'_> {
    /// `\patterns{…}` (tex.web 960–965) and `\hyphenation{…}` (934–940).
    pub(super) fn hyph_data(&mut self, patterns: bool) {
        let lang = hyph::cur_lang(self.int_par(IntPar::Language));
        self.scan_left_brace();
        let mut toks = Vec::new();
        loop {
            let Some((_, m)) = self.next_x_m(false) else {
                self.err("File ended while scanning text".into());
                break;
            };
            match m {
                Meaning::Char(11 | 12, c) => toks.push(HTok::Char(c)),
                Meaning::CharGiven(c) if !patterns => toks.push(HTok::Char(char::from_u32(c).unwrap_or('\0'))),
                Meaning::Prim(Prim::Char) if !patterns => {
                    let c = self.scan_char_num();
                    toks.push(HTok::Char(char::from_u32(c as u32).unwrap_or('\0')));
                }
                Meaning::Char(10, _) => toks.push(HTok::Space),
                Meaning::Char(2, _) => break,
                _ => toks.push(HTok::Bad),
            }
        }
        // (the `\lccode` of each character read)
        let mut lcs: Vec<(char, char)> = Vec::new();
        for t in &toks {
            if let HTok::Char(c) = t
                && *c != '-'
                && !lcs.iter().any(|(x, _)| x == c)
            {
                let v = self.int(&Key::Lccode(*c));
                lcs.push((*c, char::from_u32(v as u32).unwrap_or('\0')));
            }
        }
        let lc = |c: char| lcs.iter().find(|(x, _)| *x == c).map_or('\0', |(_, l)| *l);
        let (key, val, errors) = if patterns {
            let old = match self.get(&Key::Patterns(lang)) {
                Val::Patterns(p) => p,
                _ => hyph::Patterns::new(),
            };
            let (new, e) = old.add(&toks, lc);
            (Key::Patterns(lang), Val::Patterns(new), e)
        } else {
            let old = match self.get(&Key::Exceptions(lang)) {
                Val::Exceptions(x) => x,
                _ => hyph::Exceptions::new(),
            };
            let (new, e) = old.add(&toks, lc);
            (Key::Exceptions(lang), Val::Exceptions(new), e)
        };
        for e in errors {
            self.err(e.message().into());
        }
        self.define(key, val, true);
    }

    /// The language whatsit for `l` with the minima now (tex.web 1376).
    fn language_item(&mut self, l: u8) -> Item {
        let lhm = hyph::norm_min(self.int_par(IntPar::Lefthyphenmin)) as u8;
        let rhm = hyph::norm_min(self.int_par(IntPar::Righthyphenmin)) as u8;
        Item::Lang { lang: l, lhm, rhm }
    }

    /// `\setlanguage` (tex.web 1377).
    pub(super) fn set_language(&mut self) {
        if self.st.mode != Mode::H {
            let m = if self.st.mode == Mode::V {
                if self.st.inner { "internal vertical" } else { "vertical" }
            } else if self.st.inner {
                "math"
            } else {
                "display math"
            };
            self.err(format!("You can't use `\\setlanguage' in {m} mode"));
            return;
        }
        let v = self.scan_int();
        let l = if v <= 0 || v > 255 { 0 } else { v as u8 };
        self.st.hyph_lang[0] = l;
        let it = self.language_item(l);
        self.emit.push(it);
    }

    /// tex.web 1376 `fix_language`, where a run of characters begins in a
    /// paragraph.
    pub(super) fn fix_language(&mut self) {
        if self.st.mode != Mode::H || self.st.inner {
            return;
        }
        let l = hyph::cur_lang(self.int_par(IntPar::Language));
        if l != self.st.hyph_lang[0] {
            let it = self.language_item(l);
            self.emit.push(it);
            self.st.hyph_lang[0] = l;
        }
    }

    /// A paragraph's language state where it begins (tex.web 1091, and
    /// 1200 after a display): `clang`, and `prev_graf`'s language and
    /// minima.
    pub(super) fn begin_paragraph_language(&mut self) {
        let l = hyph::cur_lang(self.int_par(IntPar::Language));
        let lhm = hyph::norm_min(self.int_par(IntPar::Lefthyphenmin)) as u8;
        let rhm = hyph::norm_min(self.int_par(IntPar::Righthyphenmin)) as u8;
        self.st.hyph_lang = [l, l, lhm, rhm];
    }

    /// What breaking the paragraph `hlist` reads to hyphenate it (tex.web
    /// 891–899): read where it is broken.
    pub(super) fn hyph_params(&mut self, hlist: &[Item]) -> Arc<HyphParams> {
        let [_, lang, lhm, rhm] = self.st.hyph_lang;
        let mut seen = [false; 256];
        let mut fonts: Vec<Arc<Font>> = Vec::new();
        let mut langs = vec![lang];
        let mut wide: Vec<char> = Vec::new();
        let font = |f: &Arc<Font>, fonts: &mut Vec<Arc<Font>>| {
            if !fonts.iter().any(|x| x.key == f.key) {
                fonts.push(f.clone());
            }
        };
        for it in hlist {
            match it {
                Item::Char(f, c) => {
                    seen[usize::from(*c)] = true;
                    font(f, &mut fonts);
                }
                Item::Lig(l) => {
                    for &c in &l.orig {
                        seen[usize::from(c)] = true;
                    }
                    font(&l.font, &mut fonts);
                }
                Item::Lang { lang, .. } if !langs.contains(lang) => langs.push(*lang),
                Item::Native(w) => {
                    for c in crate::native::chars_of(&w.text) {
                        if let Some(ch) = char::from_u32(c) {
                            wide.push(ch);
                        }
                    }
                    font(&w.font, &mut fonts);
                }
                _ => {}
            }
        }
        wide.sort_unstable();
        wide.dedup();
        let lc_wide: Vec<(char, u32)> =
            wide.into_iter().map(|c| (c, self.int(&Key::Lccode(c)).max(0).cast_unsigned())).collect();
        let max_len = if self.xetex {
            self.int_par(IntPar::XeTeXhyphenatablelength).clamp(0, 4095) as usize
        } else {
            63
        };
        let mut lc = vec![0u8; 256];
        for (c, s) in seen.iter().enumerate() {
            if *s {
                let v = self.int(&Key::Lccode(char::from(c as u8)));
                lc[c] = u8::try_from(v).unwrap_or(0);
            }
        }
        let hyphen_chars = fonts.iter().map(|f| (f.key.clone(), self.int(&Key::HyphenChar(f.key.clone())))).collect();
        let tables = langs
            .into_iter()
            .map(|l| {
                let p = match self.get(&Key::Patterns(l)) {
                    Val::Patterns(p) => p,
                    _ => hyph::Patterns::new(),
                };
                let x = match self.get(&Key::Exceptions(l)) {
                    Val::Exceptions(x) => x,
                    _ => hyph::Exceptions::new(),
                };
                (l, p, x)
            })
            .collect();
        Arc::new(HyphParams {
            init: LangState { lang, lhm, rhm },
            uc_hyph: self.int_par(IntPar::Uchyph) > 0,
            lc,
            hyphen_chars,
            tables,
            lost_chars: self.int_par(IntPar::Tracinglostchars) > 0,
            lc_wide,
            max_len,
        })
    }
}
