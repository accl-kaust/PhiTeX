//! Fonts as values (tex.web 1252–1261, 577–578, 1253). `\font\f=name` is
//! a node that reads the TFM file (a pure function of its bytes) and
//! defines `\f` as the font; a font's identity is its internal number, the
//! order in which the document first loaded it (`Key::Fonts`, read and
//! extended by each `\font` that loads a new one), so equal loads are
//! equal values. Its `\fontdimen`s, `\hyphenchar`, `\skewchar` and the name
//! it was last defined by are global `Key`s defined at the load.

use std::sync::Arc;

use super::Exec;
use crate::arith::xn_over_d;
use crate::tfm::{Font, null_font};
use crate::val::{IntPar, Key, Meaning, Prim, Tok, Val};

impl Exec<'_> {
    /// `\font\f=name [at d | scaled n]` (tex.web 1257–1260).
    pub(super) fn new_font(&mut self, global: bool) {
        let Some(key) = self.r_token() else { return };
        let ident: Arc<str> = match &key {
            Key::Cs(n) if n.is_empty() => Arc::from("FONT"),
            Key::Cs(n) => n.clone(),
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
        let fonts = match self.get(&Key::Fonts) {
            Val::Fonts(v) => v,
            _ => Arc::new(Vec::new()),
        };
        // (§1260: a font already loaded at this size is that font)
        let same = fonts.iter().find(|f| {
            f.name == name && if s > 0 { s == f.size } else { f.size == xn_over_d(f.design_size, -s, 1000).0 }
        });
        let f = if let Some(f) = same {
            f.clone()
        } else {
            let id = fonts.len() as u16 + 1;
            let loaded = self.env.font_file(&name).and_then(|data| {
                let at = if s == -1000 {
                    None
                } else if s > 0 {
                    Some(s)
                } else {
                    Some(xn_over_d(Font::design_size_of(&data).ok()?, -s, 1000).0)
                };
                Font::load(&name, id, data, at).ok()
            });
            let Some(f) = loaded else {
                self.err(format!("Font {}={name} not loadable: Metric (TFM) file not found", key_text(&key)));
                return;
            };
            let f = Arc::new(f);
            let mut all = (*fonts).clone();
            all.push(f.clone());
            self.define(Key::Fonts, Val::Fonts(Arc::new(all)), true);
            for (k, &v) in f.params.iter().enumerate().skip(1) {
                self.define(Key::FontDimen(id, k as u16), Val::Int(v), true);
            }
            let h = self.int_par(IntPar::Defaulthyphenchar);
            let sk = self.int_par(IntPar::Defaultskewchar);
            self.define(Key::HyphenChar(id), Val::Int(h), true);
            self.define(Key::SkewChar(id), Val::Int(sk), true);
            f
        };
        self.define(Key::FontIdent(f.id), Val::Name(ident), true);
        self.define(key, Val::Meaning(Meaning::Font(f)), global);
    }

    /// tex.web 526: a file name: characters up to a space (which goes) or
    /// anything else (which stays), after blanks; `"` quotes spaces.
    pub(super) fn scan_file_name(&mut self) -> String {
        let mut name = String::new();
        let mut quoted = false;
        let mut first = true;
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
        if n <= 0 || n as usize > f.nparams() {
            let id = self.font_ident(&f);
            self.err(format!("Font \\{id} has only {} fontdimen parameters", f.nparams()));
            return None;
        }
        Some(Key::FontDimen(f.id, n as u16))
    }

    /// The name font `f` was last defined by (`font_id_text`).
    pub(super) fn font_ident(&mut self, f: &Font) -> Arc<str> {
        match self.get(&Key::FontIdent(f.id)) {
            Val::Name(n) => n,
            _ => Arc::from("FONT"),
        }
    }

    /// `\hyphenchar f` (0) or `\skewchar f` (1): its key.
    pub(super) fn font_int_key(&mut self, which: u8) -> Key {
        let f = self.scan_font_ident();
        if which == 0 { Key::HyphenChar(f.id) } else { Key::SkewChar(f.id) }
    }

    /// `\hyphenchar f=n`, `\skewchar f=n`: always global (tex.web 1253).
    pub(super) fn assign_font_int(&mut self, which: u8) {
        let k = self.font_int_key(which);
        self.optional_equals();
        let v = self.scan_int();
        self.define(k, Val::Int(v), true);
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
        Tok::Cs(n)
    }

    /// The current font after selecting `f` (`\f` as a command, tex.web
    /// 1217).
    pub(super) fn select_font(&mut self, f: Arc<Font>, global: bool) {
        self.define(Key::Font, Val::Font(f), global);
    }
}

/// How an error names the key a `\font` defines.
fn key_text(k: &Key) -> String {
    match k {
        Key::Cs(n) => format!("\\{n}"),
        Key::Active(c) => c.to_string(),
        _ => String::new(),
    }
}
