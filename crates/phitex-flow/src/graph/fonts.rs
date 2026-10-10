//! Fonts as values (tex.web 1252–1261, 577–578, 1253). `\font\f=name` is
//! a node that reads the TFM file (a pure function of its bytes) and
//! defines `\f` as the font; a font's identity is its name and size
//! (`Font::key`), so equal loads are equal values. Its `\fontdimen`s,
//! `\hyphenchar`, `\skewchar` (defined when it is first loaded:
//! `Key::FontLoaded`) and the name it was last defined by are global
//! `Key`s.

use std::sync::Arc;

use super::Exec;
use crate::arith::xn_over_d;
use crate::micro;
use crate::tfm::{Font, null_font};
use crate::val::{IntPar, Key, Meaning, Prim, Tok, Val};

impl Exec<'_> {
    /// `\font\f=name [at d | scaled n]` (tex.web 1257–1260).
    pub(super) fn new_font(&mut self, global: bool) {
        let Some(key) = self.r_token() else { return };
        let ident: Arc<str> = match &key {
            Key::Cs(n) => {
                let s = self.env.cs().name(*n);
                if s.is_empty() { Arc::from("FONT") } else { s }
            }
            Key::Active(c) => Arc::from(format!("FONT{c}")),
            _ => Arc::from("FONT"),
        };
        // (the name means `\nullfont` while the size is scanned, tex.web
        // 1257)
        self.define(key.clone(), Val::Meaning(Meaning::Font(null_font())), global);
        self.optional_equals();
        let name = self.scan_file_name();
        // (§1258: the size)
        let s = if self.keyword("at") {
            let s = self.scan_dimen();
            if s <= 0 || s >= 0o1_000_000_000 {
                self.err("Improper `at' size, replaced by 10pt".into());
                10 * 65536
            } else {
                s
            }
        } else if self.keyword("scaled") {
            let n = self.scan_int();
            if n <= 0 || n > 32768 {
                self.err("Illegal magnification has been changed to 1000".into());
                -1000
            } else {
                -n
            }
        } else {
            -1000
        };
        let Some(data) = self.env.font_file(&name) else {
            self.err(format!("Font {}={name} not loadable: Metric (TFM) file not found", key_text(&key, self.env.cs())));
            return;
        };
        // (tex.web 568: the size; a bad file is not loaded)
        let size = match s {
            -1000 => Ok(None),
            s if s > 0 => Ok(Some(s)),
            s => Font::design_size_of(&data).map(|z| Some(xn_over_d(z, -s, 1000).0)),
        };
        let Some(f) = size.and_then(|at| Font::load(&name, data, at)).ok() else {
            self.err(format!("Font {}={name} not loadable: Bad metric (TFM) file", key_text(&key, self.env.cs())));
            return;
        };
        let f = Arc::new(f);
        // (§1260: a font already loaded — the same name at the same size — is
        // that font, its parameters as they are now)
        let loaded = Key::FontLoaded(f.key.clone());
        if self.get(&loaded) == Val::Int(0) {
            self.define(loaded, Val::Int(1), true);
            // (`font_ptr`: the most recently loaded font, whose parameters
            // can be extended, tex.web 579)
            self.define(Key::LastFont, Val::Name(f.key.clone()), true);
            for (k, &v) in f.params.iter().enumerate().skip(1) {
                self.define(Key::FontDimen(f.key.clone(), k as u32), Val::Int(v), true);
            }
            let h = self.int_par(IntPar::Defaulthyphenchar);
            let sk = self.int_par(IntPar::Defaultskewchar);
            self.define(Key::HyphenChar(f.key.clone()), Val::Int(h), true);
            self.define(Key::SkewChar(f.key.clone()), Val::Int(sk), true);
        }
        self.define(Key::FontIdent(f.key.clone()), Val::Name(ident), true);
        self.define(key, Val::Meaning(Meaning::Font(f)), global);
    }

    /// tex.web 526: a file name: characters up to a space (which goes) or
    /// anything else (which stays), after blanks; `"` quotes spaces.
    pub(super) fn scan_file_name(&mut self) -> String {
        let mut name = String::new();
        let mut quoted = false;
        let mut first = true;
        let was = std::mem::replace(&mut self.name_in_progress, true);
        while let Some((t, m)) = self.next_x_m(false) {
            let c = match m {
                Meaning::Char(cat, c) if (1..=12).contains(&cat) && u32::from(c) <= 255 => c,
                _ => {
                    self.back(t);
                    break;
                }
            };
            if c == ' ' && first {
                continue;
            }
            first = false;
            if c == '"' {
                quoted = !quoted;
                continue;
            }
            if c == ' ' && !quoted {
                break;
            }
            name.push(c);
        }
        self.name_in_progress = was;
        match name.strip_suffix(".tfm") {
            Some(n) => n.to_string(),
            None => name,
        }
    }

    /// tex.web 577: a font identifier (`\font` is the current font).
    pub(super) fn scan_font_ident(&mut self) -> Arc<Font> {
        loop {
            match self.next_x_m(false) {
                Some((_, Meaning::Char(10, _))) => {}
                Some((_, Meaning::Prim(Prim::Font))) => return self.font(),
                Some((_, Meaning::Font(f))) => return f,
                Some((_, Meaning::Prim(Prim::DefFamily(size)))) => {
                    let fam = self.scan_four_bit_int() as u8;
                    return self.fam_font(size, fam);
                }
                other => {
                    self.err("Missing font identifier".into());
                    if let Some((t, _)) = other {
                        self.back(t);
                    }
                    return null_font();
                }
            }
        }
    }

    /// `\fontdimen n f`: its key (tex.web 578).
    pub(super) fn find_font_dimen(&mut self) -> Option<Key> {
        let n = self.scan_int();
        let f = self.scan_font_ident();
        let np = self.font_params(&f);
        if n > 0 && n as usize > np {
            // (tex.web 580: the most recently loaded font's parameters grow,
            // the new ones zero, whether this reads or writes)
            let last = matches!(self.get(&Key::LastFont), Val::Name(k) if *k == *f.key);
            if last {
                self.define(Key::FontParams(f.key.clone()), Val::Int(n), true);
                return Some(Key::FontDimen(f.key.clone(), n.cast_unsigned()));
            }
        }
        if n <= 0 || n as usize > np {
            let id = self.font_ident(&f);
            self.err(format!("Font \\{id} has only {np} fontdimen parameters"));
            return None;
        }
        Some(Key::FontDimen(f.key.clone(), n.cast_unsigned()))
    }

    /// The parameters font `f` has (`font_params`): its own, or more given
    /// since by `\fontdimen` while it was the font loaded last
    /// (`Key::FontParams`): what math's parameter checks count too
    /// (tex.web 1195).
    pub(super) fn font_params(&mut self, f: &Font) -> usize {
        match self.get(&Key::FontParams(f.key.clone())) {
            Val::Int(k) if k > 0 => k as usize,
            _ => f.nparams(),
        }
    }

    /// The name font `f` was last defined by (`font_id_text`).
    pub(super) fn font_ident(&mut self, f: &Font) -> Arc<str> {
        match self.get(&Key::FontIdent(f.key.clone())) {
            Val::Name(n) => n,
            _ => Arc::from("FONT"),
        }
    }

    /// `\hyphenchar f` (0) or `\skewchar f` (1): its key.
    pub(super) fn font_int_key(&mut self, which: u8) -> Key {
        let f = self.scan_font_ident();
        if which == 0 { Key::HyphenChar(f.key.clone()) } else { Key::SkewChar(f.key.clone()) }
    }

    /// `\hyphenchar f=n`, `\skewchar f=n`: always global (tex.web 1253);
    /// pdfTeX's `\lpcode f c=n` and the other codes (global too:
    /// `set_*_code`), `\pdfnoligatures f`.
    pub(super) fn assign_font_int(&mut self, which: u8) {
        if which < 2 {
            let k = self.font_int_key(which);
            self.optional_equals();
            let v = self.scan_int();
            self.define(k, Val::Int(v), true);
            return;
        }
        let f = self.scan_font_ident();
        let key = Key::FontCode(which, f.key.clone());
        // (`\pdfnoligatures` and `\tagcode` clear characters' tags: the
        // font's `micro::TAG` table of the tags cleared, `set_no_ligatures`
        // the lig/kern program's of every character, `set_tag_code` those
        // of `abs(fix_int(n, -7, 0))`'s bits of an existing character)
        if which == micro::NOLIG || which == micro::TAG {
            let tkey = Key::FontCode(micro::TAG, f.key.clone());
            let mut t: Vec<i32> = self.font_code_table(&tkey).to_vec();
            t.resize(256, 0);
            if which == micro::NOLIG {
                for x in &mut t {
                    *x |= micro::TAG_LIG;
                }
            } else {
                let c = self.scan_char_num();
                self.optional_equals();
                let v = self.scan_int().clamp(-7, 0).abs();
                if f.glyph(c).is_none() {
                    return;
                }
                t[c as usize] |= v;
            }
            self.define(tkey, Val::Penalties(t.into()), true);
            return;
        }
        let c = self.scan_char_num();
        self.optional_equals();
        let v = self.scan_int();
        let table = self.font_code_table(&key);
        let t = micro::set_code(&table, which, u8::try_from(c).unwrap_or(0), v);
        self.define(key, Val::Penalties(t), true);
    }

    /// A font's table of a code (`Key::FontCode`; empty: the defaults).
    pub(super) fn font_code_table(&mut self, key: &Key) -> Arc<[i32]> {
        match self.get(key) {
            Val::Penalties(t) => t,
            _ => Arc::from([]),
        }
    }

    /// `\the` of `\hyphenchar f`, `\skewchar f`, a pdfTeX code `f c` or
    /// `\pdfnoligatures f` (pdftex.web 425's `Fetch a font integer`).
    pub(super) fn font_int_value(&mut self, which: u8) -> i32 {
        if which < 2 {
            let k = self.font_int_key(which);
            return self.int(&k);
        }
        let f = self.scan_font_ident();
        if which == micro::NOLIG || which == micro::TAG {
            let t = self.font_code_table(&Key::FontCode(micro::TAG, f.key.clone()));
            if which == micro::NOLIG {
                return f.test_no_ligatures(&t);
            }
            let c = self.scan_char_num();
            return micro::tag_code(&f, &t, c);
        }
        let c = self.scan_char_num();
        let t = self.font_code_table(&Key::FontCode(which, f.key.clone()));
        micro::code(&t, which, u8::try_from(c).unwrap_or(0))
    }

    /// pdftex.web's `pdf_error`: an error that ends the job.
    pub(super) fn pdf_error(&mut self, t: &str, p: &str) {
        self.err(format!("pdfTeX error ({t}): {p}"));
        self.overflow(String::new());
    }

    /// `\pdffontexpand f stretch shrink step [autoexpand]`
    /// (`read_expand_font`): the font's parameters, global, checked
    /// against those it was given before. Its fonts expanded by the limits
    /// are loaded now (they are numbered now: `get_expand_font`); the
    /// others where a line is set in them. Every font is taken as
    /// `autoexpand` (CORE.md).
    pub(super) fn pdf_font_expand(&mut self) {
        let f = self.scan_font_ident();
        if *f.key == *"nullfont" {
            self.pdf_error("font expansion", "invalid font identifier");
            return;
        }
        if f.expanded_from().is_some() {
            self.pdf_error(
                "font expansion",
                "\\pdffontexpand cannot be used this way (the base font has been expanded)",
            );
            return;
        }
        self.optional_equals();
        let mut stretch = self.scan_int().clamp(0, 1000);
        let mut shrink = self.scan_int().clamp(0, 500);
        let step = self.scan_int().clamp(0, 100);
        if step == 0 {
            self.pdf_error("font expansion", "invalid step");
            return;
        }
        stretch -= stretch % step;
        shrink -= shrink % step;
        if stretch == 0 && shrink == 0 {
            self.pdf_error("font expansion", "invalid limit(s)");
            return;
        }
        let auto = self.keyword("autoexpand");
        if auto {
            self.optional_space();
        }
        let key = Key::FontExpand(f.key.clone());
        let now = micro::Expand { stretch, shrink, step, auto };
        if let Val::Penalties(v) = self.get(&key)
            && let Some(was) = micro::Expand::of(&v)
        {
            let bad = if was.step != step {
                Some("font has been expanded with different expansion step")
            } else if was.stretch != stretch {
                Some("font has been expanded with different stretch limit")
            } else if was.shrink != shrink {
                Some("font has been expanded with different shrink limit")
            } else if was.auto != auto {
                Some("font has been expanded with different auto expansion value")
            } else {
                None
            };
            if let Some(b) = bad {
                self.pdf_error("font expansion", b);
            }
            return;
        }
        self.define(key, Val::Penalties(now.val()), true);
        for e in [stretch, -shrink] {
            if e != 0 {
                let x = Font::expanded(&f, e);
                let loaded = Key::FontLoaded(x.key.clone());
                if self.get(&loaded) == Val::Int(0) {
                    self.define(loaded, Val::Int(1), true);
                }
            }
        }
    }

    /// `\pdffontattr f {text}`: the text, global; read by PDF output only
    /// (an error in DVI mode, `check_pdfoutput`).
    pub(super) fn pdf_font_attr(&mut self) {
        if self.int_par(IntPar::Pdfoutput) <= 0 {
            self.pdf_error("\\pdffontattr", "not allowed in DVI mode (\\pdfoutput <= 0)");
            return;
        }
        let f = self.scan_font_ident();
        if *f.key == *"nullfont" {
            self.pdf_error("font", "invalid font identifier");
            return;
        }
        let toks = self.scan_toks_x();
        let mut text = String::new();
        self.show_toks(&mut text, &toks);
        self.define(Key::FontAttr(f.key.clone()), Val::Name(Arc::from(text)), true);
    }

    /// The name `key` (`\font`'s, `\pdfcopyfont`'s) gives the font it
    /// defines (`font_id_text`).
    fn ident_of(&self, key: &Key) -> Arc<str> {
        match key {
            Key::Cs(n) => {
                let s = self.env.cs().name(*n);
                if s.is_empty() { Arc::from("FONT") } else { s }
            }
            Key::Active(c) => Arc::from(format!("FONT{c}")),
            _ => Arc::from("FONT"),
        }
    }

    /// The next number of a font made from `f` (`Key::FontCopies`).
    fn next_copy(&mut self, f: &Font) -> u32 {
        let k = Key::FontCopies(f.key.clone());
        let n = self.int(&k) + 1;
        self.define(k, Val::Int(n), true);
        n.cast_unsigned()
    }

    /// A font pdfTeX made is loaded (`font_ptr` its): its parameters,
    /// `\hyphenchar` and `\skewchar` defined, and `key` defined as it.
    fn define_made_font(&mut self, key: Key, f: Font, hyphen: i32, skew: i32, global: bool) {
        let f = Arc::new(f);
        let ident = self.ident_of(&key);
        self.define(Key::FontLoaded(f.key.clone()), Val::Int(1), true);
        self.define(Key::LastFont, Val::Name(f.key.clone()), true);
        for (k, &v) in f.params.iter().enumerate().skip(1) {
            self.define(Key::FontDimen(f.key.clone(), k as u32), Val::Int(v), true);
        }
        self.define(Key::HyphenChar(f.key.clone()), Val::Int(hyphen), true);
        self.define(Key::SkewChar(f.key.clone()), Val::Int(skew), true);
        self.define(Key::FontIdent(f.key.clone()), Val::Name(ident), true);
        self.define(key, Val::Meaning(Meaning::Font(f)), global);
    }

    /// `\pdfcopyfont\cs=f` (`make_font_copy`): a copy of `f`, a font of
    /// its own, with `f`'s parameters, `\hyphenchar` and `\skewchar` as
    /// they are now.
    pub(super) fn pdf_copy_font(&mut self, global: bool) {
        let Some(key) = self.r_token() else { return };
        self.define(key.clone(), Val::Meaning(Meaning::Font(null_font())), global);
        self.optional_equals();
        let f = self.scan_font_ident();
        if f.expanded_from().is_some() || self.font_expand_of(&f).is_some() {
            self.pdf_error("\\pdfcopyfont", "cannot copy an expanded font");
            return;
        }
        if matches!(f.derived.as_deref(), Some(crate::tfm::Derived { kind: crate::tfm::DerivedKind::LetterSpaced { .. }, .. })) {
            self.pdf_error("\\pdfcopyfont", "cannot copy a letterspaced font");
            return;
        }
        let np = self.font_params(&f);
        let mut params = vec![0; np.max(7) + 1];
        for (k, p) in params.iter_mut().enumerate().skip(1) {
            *p = self.int(&Key::FontDimen(f.key.clone(), k as u32));
        }
        let hyphen = self.int(&Key::HyphenChar(f.key.clone()));
        let skew = self.int(&Key::SkewChar(f.key.clone()));
        let n = self.next_copy(&f);
        let c = Font::copied(&f, n, params.into());
        self.define_made_font(key, c, hyphen, skew, global);
    }

    /// `\letterspacefont\cs=f n [nolig]` (`new_letterspaced_font`): `f`'s
    /// file loaded again, its characters `n` thousandths of its quad wider.
    pub(super) fn letterspace_font(&mut self, global: bool) {
        let Some(key) = self.r_token() else { return };
        self.define(key.clone(), Val::Meaning(Meaning::Font(null_font())), global);
        self.optional_equals();
        let f = self.scan_font_ident();
        let e = self.scan_int().clamp(-1000, 1000);
        let nolig = self.keyword("nolig");
        let quad = self.int(&Key::FontDimen(f.key.clone(), 6));
        let n = self.next_copy(&f);
        let ls = Font::letterspaced(&f, e, n, nolig, quad);
        let h = self.int_par(IntPar::Defaulthyphenchar);
        let sk = self.int_par(IntPar::Defaultskewchar);
        self.define_made_font(key, ls, h, sk, global);
    }

    /// A font's `\pdffontexpand` parameters, if it was given them.
    pub(super) fn font_expand_of(&mut self, f: &Font) -> Option<micro::Expand> {
        match self.get(&Key::FontExpand(f.key.clone())) {
            Val::Penalties(v) => micro::Expand::of(&v),
            _ => None,
        }
    }

    /// `\fontname f` (tex.web 472).
    pub(super) fn font_name_text(&mut self, out: &mut String) {
        let f = self.scan_font_ident();
        out.push_str(&f.name);
        if f.size != f.design_size {
            out.push_str(" at ");
            crate::print::scaled(out, f.size);
            out.push_str("pt");
        }
    }

    /// `\the` of a font identifier: the name it was last defined by, as a
    /// token (TeX's frozen `font_id_base+f`; here the name itself, whose
    /// meaning is that font unless it was redefined since: CORE.md).
    pub(super) fn font_ident_tok(&mut self, f: &Font) -> Tok {
        let n = self.font_ident(f);
        Tok::Cs(self.cs_of(&n))
    }

    /// `\textfont n` and friends.
    pub(super) fn fam_font(&mut self, size: u8, fam: u8) -> Arc<Font> {
        match self.get(&Key::FamFont(size, fam)) {
            Val::Font(f) => f,
            _ => null_font(),
        }
    }

    /// The current font after selecting `f` (`\f` as a command, tex.web
    /// 1217).
    pub(super) fn select_font(&mut self, f: Arc<Font>, global: bool) {
        self.define(Key::Font, Val::Font(f), global);
    }
}

/// How an error names the key a `\font` defines.
fn key_text(k: &Key, cs: &crate::build::CsTable) -> String {
    match k {
        Key::Cs(n) => format!("\\{}", cs.name(*n)),
        Key::Active(c) => c.to_string(),
        _ => String::new(),
    }
}
