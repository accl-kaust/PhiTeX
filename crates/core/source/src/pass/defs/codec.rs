//! [`Definitions`] as bytes, to keep a format's harvest with the format
//! (a client's cache): every map by its keys in order, so the same
//! definitions give the same bytes however they were made. The stamps
//! are the process's own: a name stamped in the bytes is stamped afresh
//! as they are read, as a harvest would stamp it.

use std::sync::Arc;

use super::super::hints::{Def, Effects};
use super::{Arg, At, Body, Catcoder, Definitions, HashMap, HashSet, Lit, Shape};

/// The bytes' first word: the layout of what follows.
const MAGIC: &[u8; 8] = b"PHXDEFS1";

impl Definitions {
    /// These definitions as bytes ([`Definitions::from_bytes`]).
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut w = W(MAGIC.to_vec());
        w.map(&self.macros, |w, v| w.ats(v, W::shape));
        w.map(&self.envs, |w, v| w.ats(v, |w, a| w.args(a)));
        w.map(&self.catcoders, |w, v| w.ats(v, W::catcoder));
        w.set(&self.env_catcoders);
        w.u128(self.active);
        w.map(&self.bodies, |w, v| {
            w.ats(v, |w, b| {
                w.str(&b.text);
                w.effects(&b.effects);
            });
        });
        match &self.order {
            None => w.u8(0),
            Some(o) => {
                w.u8(1);
                let mut o: Vec<(u32, u32)> = o.iter().map(|(&a, &b)| (a, b)).collect();
                o.sort_unstable();
                w.len(o.len());
                for (a, b) in o {
                    w.u32(a);
                    w.u32(b);
                }
            }
        }
        w.set(&self.fonts);
        w.map(&self.registers, |w, r| w.u32(*r));
        let stamped: HashSet<String> = self.stamps.keys().cloned().collect();
        w.set(&stamped);
        w.0
    }

    /// The definitions [`Definitions::to_bytes`] wrote; `None` for any
    /// other bytes.
    #[must_use]
    pub fn from_bytes(b: &[u8]) -> Option<Definitions> {
        let mut r = R(b);
        if r.take(8)? != MAGIC {
            return None;
        }
        let mut d = Definitions {
            macros: r.map(|r| r.ats(R::shape))?,
            envs: r.map(|r| r.ats(R::args))?,
            catcoders: r.map(|r| r.ats(R::catcoder))?,
            env_catcoders: r.set()?,
            active: r.u128()?,
            bodies: r.map(|r| {
                r.ats(|r| {
                    let text = r.str()?;
                    let effects = r.effects()?;
                    Some(Arc::new(Body { text, effects }))
                })
            })?,
            ..Definitions::default()
        };
        d.order = match r.u8()? {
            0 => None,
            1 => {
                let n = r.len()?;
                let mut o = HashMap::default();
                for _ in 0..n {
                    let a = r.u32()?;
                    o.insert(a, r.u32()?);
                }
                Some(o)
            }
            _ => return None,
        };
        d.fonts = r.set()?;
        d.registers = r.map(R::u32)?;
        let mut stamped: Vec<String> = r.set()?.into_iter().collect();
        if !r.0.is_empty() {
            return None;
        }
        // (stamped afresh, in the names' order: marks unique in this
        // process, as the harvest's would be)
        stamped.sort_unstable();
        for n in stamped {
            d.touch(&n);
        }
        Some(d)
    }
}

/// A writer.
struct W(Vec<u8>);

impl W {
    fn u8(&mut self, x: u8) {
        self.0.push(x);
    }
    fn u32(&mut self, x: u32) {
        self.0.extend_from_slice(&x.to_le_bytes());
    }
    fn u64(&mut self, x: u64) {
        self.0.extend_from_slice(&x.to_le_bytes());
    }
    fn u128(&mut self, x: u128) {
        self.0.extend_from_slice(&x.to_le_bytes());
    }
    fn len(&mut self, n: usize) {
        self.u64(n as u64);
    }
    fn char(&mut self, c: char) {
        self.u32(u32::from(c));
    }
    fn bool(&mut self, b: bool) {
        self.u8(u8::from(b));
    }
    fn str(&mut self, s: &str) {
        self.len(s.len());
        self.0.extend_from_slice(s.as_bytes());
    }
    fn strs(&mut self, xs: &[String]) {
        self.len(xs.len());
        for x in xs {
            self.str(x);
        }
    }
    fn set(&mut self, s: &HashSet<String>) {
        let mut ks: Vec<&String> = s.iter().collect();
        ks.sort_unstable();
        self.len(ks.len());
        for k in ks {
            self.str(k);
        }
    }
    fn map<T>(&mut self, m: &HashMap<String, T>, f: impl Fn(&mut W, &T)) {
        let mut ks: Vec<&String> = m.keys().collect();
        ks.sort_unstable();
        self.len(ks.len());
        for k in ks {
            self.str(k);
            f(self, &m[k]);
        }
    }
    fn ats<T>(&mut self, v: &[At<T>], f: impl Fn(&mut W, &T)) {
        self.len(v.len());
        for a in v {
            self.u32(a.file);
            self.u64(a.pos as u64);
            f(self, &a.what);
        }
    }
    fn lits(&mut self, ls: &[Lit]) {
        self.len(ls.len());
        for l in ls {
            match l {
                Lit::Cs(s) => {
                    self.u8(0);
                    self.str(s);
                }
                Lit::Char(c) => {
                    self.u8(1);
                    self.char(*c);
                }
            }
        }
    }
    fn args(&mut self, a: &[Arg]) {
        self.len(a.len());
        for x in a {
            self.arg(x);
        }
    }
    fn arg(&mut self, a: &Arg) {
        let unit = |w: &mut W, k: u8| w.u8(k);
        match a {
            Arg::Star => unit(self, 0),
            Arg::OptChar(c) => {
                self.u8(1);
                self.char(*c);
            }
            Arg::Opt => unit(self, 2),
            Arg::M => unit(self, 3),
            Arg::Tok => unit(self, 4),
            Arg::Relation => unit(self, 5),
            Arg::ParamText => unit(self, 6),
            Arg::Group => unit(self, 7),
            Arg::BoxSpec => unit(self, 8),
            Arg::Eq => unit(self, 9),
            Arg::Int => unit(self, 10),
            Arg::Dimen => unit(self, 11),
            Arg::Glue => unit(self, 12),
            Arg::Value => unit(self, 13),
            Arg::Reg => unit(self, 14),
            Arg::CsName => unit(self, 15),
            Arg::Expr => unit(self, 16),
            Arg::File => unit(self, 17),
            Arg::Toks => unit(self, 18),
            Arg::Rule => unit(self, 19),
            Arg::By => unit(self, 20),
            Arg::To => unit(self, 21),
            Arg::LetTarget => unit(self, 22),
            Arg::FontSize => unit(self, 23),
            Arg::Keywords => unit(self, 24),
            Arg::Tikz => unit(self, 25),
            Arg::Semicolon => unit(self, 26),
            Arg::Verb => unit(self, 27),
            Arg::Env => unit(self, 28),
            Arg::StarSkip => unit(self, 29),
            Arg::UntilCs(s) => {
                self.u8(30);
                self.str(s);
            }
            Arg::Literal(ls) => {
                self.u8(31);
                self.lits(ls);
            }
            Arg::Delimited(ls) => {
                self.u8(32);
                self.lits(ls);
            }
            Arg::UntilOpen => unit(self, 33),
            Arg::Pending(s) => {
                self.u8(34);
                self.str(s);
            }
            Arg::Pdf(s) => {
                self.u8(35);
                self.str(s);
            }
            Arg::Array(c) => {
                self.u8(36);
                self.char(*c);
            }
            Arg::Delim => unit(self, 37),
        }
    }
    fn shape(&mut self, s: &Shape) {
        match s {
            Shape::Macro(a) => {
                self.u8(0);
                self.args(a);
            }
            Shape::Alias(t) => {
                self.u8(1);
                self.str(t);
            }
            Shape::Quantity(c) => {
                self.u8(2);
                self.char(*c);
            }
            Shape::Cond => self.u8(3),
            Shape::Opaque(e) => {
                self.u8(4);
                self.str(e);
            }
        }
    }
    fn pairs(&mut self, xs: &[(u8, u32)]) {
        self.len(xs.len());
        for &(a, b) in xs {
            self.u8(a);
            self.u32(b);
        }
    }
    fn catcoder(&mut self, c: &Catcoder) {
        self.u8(c.kind.0);
        self.u128(c.kind.1);
        self.pairs(&c.set);
        self.pairs(&c.of_args);
        self.bool(c.ends_group);
    }
    fn defs(&mut self, ds: &[Def]) {
        self.len(ds.len());
        for d in ds {
            self.str(&d.field);
            self.bool(d.global);
        }
    }
    fn effects(&mut self, e: &Effects) {
        let Effects {
            names,
            prefixes,
            unbounded,
            calls,
            assigned,
            outer_calls,
        } = e;
        self.defs(names);
        self.defs(prefixes);
        self.strs(unbounded);
        self.strs(calls);
        self.strs(assigned);
        self.strs(outer_calls);
    }
}

/// A reader.
struct R<'a>(&'a [u8]);

impl R<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        if self.0.len() < n {
            return None;
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Some(a)
    }
    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
    fn u128(&mut self) -> Option<u128> {
        Some(u128::from_le_bytes(self.take(16)?.try_into().ok()?))
    }
    /// A count, never more than the bytes left could hold.
    fn len(&mut self) -> Option<usize> {
        let n = usize::try_from(self.u64()?).ok()?;
        (n <= self.0.len()).then_some(n)
    }
    fn char(&mut self) -> Option<char> {
        char::from_u32(self.u32()?)
    }
    fn bool(&mut self) -> Option<bool> {
        match self.u8()? {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        }
    }
    fn str(&mut self) -> Option<String> {
        let n = self.len()?;
        String::from_utf8(self.take(n)?.to_vec()).ok()
    }
    fn strs(&mut self) -> Option<Vec<String>> {
        let n = self.len()?;
        (0..n).map(|_| self.str()).collect()
    }
    fn set(&mut self) -> Option<HashSet<String>> {
        let n = self.len()?;
        let mut s = HashSet::default();
        for _ in 0..n {
            s.insert(self.str()?);
        }
        Some(s)
    }
    fn map<T>(&mut self, f: impl Fn(&mut Self) -> Option<T>) -> Option<HashMap<String, T>> {
        let n = self.len()?;
        let mut m = HashMap::default();
        for _ in 0..n {
            let k = self.str()?;
            m.insert(k, f(self)?);
        }
        Some(m)
    }
    fn ats<T>(&mut self, f: impl Fn(&mut Self) -> Option<T>) -> Option<Vec<At<T>>> {
        let n = self.len()?;
        let mut v = Vec::with_capacity(n);
        for _ in 0..n {
            let file = self.u32()?;
            let pos = usize::try_from(self.u64()?).ok()?;
            v.push(At {
                file,
                pos,
                what: f(self)?,
            });
        }
        Some(v)
    }
    fn lits(&mut self) -> Option<Vec<Lit>> {
        let n = self.len()?;
        (0..n)
            .map(|_| match self.u8()? {
                0 => Some(Lit::Cs(self.str()?)),
                1 => Some(Lit::Char(self.char()?)),
                _ => None,
            })
            .collect()
    }
    fn args(&mut self) -> Option<Vec<Arg>> {
        let n = self.len()?;
        (0..n).map(|_| self.arg()).collect()
    }
    fn arg(&mut self) -> Option<Arg> {
        Some(match self.u8()? {
            0 => Arg::Star,
            1 => Arg::OptChar(self.char()?),
            2 => Arg::Opt,
            3 => Arg::M,
            4 => Arg::Tok,
            5 => Arg::Relation,
            6 => Arg::ParamText,
            7 => Arg::Group,
            8 => Arg::BoxSpec,
            9 => Arg::Eq,
            10 => Arg::Int,
            11 => Arg::Dimen,
            12 => Arg::Glue,
            13 => Arg::Value,
            14 => Arg::Reg,
            15 => Arg::CsName,
            16 => Arg::Expr,
            17 => Arg::File,
            18 => Arg::Toks,
            19 => Arg::Rule,
            20 => Arg::By,
            21 => Arg::To,
            22 => Arg::LetTarget,
            23 => Arg::FontSize,
            24 => Arg::Keywords,
            25 => Arg::Tikz,
            26 => Arg::Semicolon,
            27 => Arg::Verb,
            28 => Arg::Env,
            29 => Arg::StarSkip,
            30 => Arg::UntilCs(self.str()?),
            31 => Arg::Literal(self.lits()?),
            32 => Arg::Delimited(self.lits()?),
            33 => Arg::UntilOpen,
            34 => Arg::Pending(self.str()?),
            35 => Arg::Pdf(self.str()?),
            36 => Arg::Array(self.char()?),
            37 => Arg::Delim,
            _ => return None,
        })
    }
    fn shape(&mut self) -> Option<Shape> {
        Some(match self.u8()? {
            0 => Shape::Macro(self.args()?),
            1 => Shape::Alias(self.str()?),
            2 => Shape::Quantity(self.char()?),
            3 => Shape::Cond,
            4 => Shape::Opaque(self.str()?),
            _ => return None,
        })
    }
    fn pairs(&mut self) -> Option<Vec<(u8, u32)>> {
        let n = self.len()?;
        (0..n)
            .map(|_| {
                let a = self.u8()?;
                Some((a, self.u32()?))
            })
            .collect()
    }
    fn catcoder(&mut self) -> Option<Catcoder> {
        let k = self.u8()?;
        Some(Catcoder {
            kind: (k, self.u128()?),
            set: self.pairs()?,
            of_args: self.pairs()?,
            ends_group: self.bool()?,
        })
    }
    fn defs(&mut self) -> Option<Vec<Def>> {
        let n = self.len()?;
        (0..n)
            .map(|_| {
                let field = self.str()?;
                Some(Def {
                    field,
                    global: self.bool()?,
                })
            })
            .collect()
    }
    fn effects(&mut self) -> Option<Effects> {
        Some(Effects {
            names: self.defs()?,
            prefixes: self.defs()?,
            unbounded: self.strs()?,
            calls: self.strs()?,
            assigned: self.strs()?,
            outer_calls: self.strs()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::FileKind;
    use super::*;

    #[test]
    fn definitions_read_back_as_they_were_written() {
        let mut d = Definitions::new();
        d.harvest(
            "\\catcode`\\@=11\n\\def\\a#1\\b{\\gdef\\c{#1}\\count@=1 }\n\\let\\x\\relax\n\\newcount\\n\n\\countdef\\p=0\n\\font\\f=cmr10\n\\def\\@m[#1]{\\catcode`\\~=13 \\x}\n\\output{\\shipout\\box255}\n\\newenvironment{e}[1]{\\def\\y{#1}}{}\n",
            FileKind::Package,
            3,
        );
        d.set_load_order(&[3]);
        let b = d.to_bytes();
        let e = Definitions::from_bytes(&b).expect("read back");
        assert_eq!(e.to_bytes(), b);
        assert_eq!(e.digest(), d.digest());
        assert_eq!(e.len(), d.len());
        for n in ["a", "c", "x", "n", "p", "f", "@m", "y"] {
            assert_eq!(e.shape(n), d.shape(n), "{n}");
            assert_eq!(e.register(n), d.register(n), "{n}");
            assert_eq!(e.is_defined(n), d.is_defined(n), "{n}");
        }
        assert_eq!(e.active_chars(), d.active_chars());
        // (stamped afresh: none of the harvest's marks)
        assert!(e.name_stamp("a") != 0 && e.name_stamp("a") != d.name_stamp("a"));
        assert!(Definitions::from_bytes(&b[..b.len() - 1]).is_none());
        assert!(Definitions::from_bytes(b"PHXDEFS0").is_none());
    }
}
