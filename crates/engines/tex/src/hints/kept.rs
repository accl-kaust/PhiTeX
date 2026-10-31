//! A file's analysis kept between builds ([`crate::AnalysisStore`]): a
//! file read by `\input` (a class or package in the document's
//! directory, which no format holds) is analysed once per text, not once
//! per process. The analysis reads nothing but the text and its kind
//! (no format's definitions, [`super::file_hints`]), so its key is the
//! text's hash, the kind and the engine's identity.
//!
//! The bytes: a magic, then each part of [`TextHints`] in order, every
//! number little-endian, a string as its length and bytes, the hints as
//! a table of the distinct ones (shared as the analysis shares them) and
//! an index per atom.

use std::sync::Arc;

use phitex_source::pass::FileKind;

use super::{Bound, Cats, Hint, TextHints, head};

const MAGIC: &[u8; 8] = b"PHTXHNT1";

/// The key of `text`'s analysis as a file of kind `kind`.
pub(crate) fn key(text: &str, kind: FileKind) -> String {
    let k = match kind {
        FileKind::Document => 'd',
        FileKind::Package => 'p',
        FileKind::Initex => 'i',
    };
    let h = phitex_codecs::sha256::sha256(text.as_bytes());
    let mut s = format!("{:016x}-{k}-", crate::api::engine_id());
    for b in h {
        use std::fmt::Write as _;
        let _ = write!(s, "{b:02x}");
    }
    s
}

struct W(Vec<u8>);

impl W {
    fn u8(&mut self, x: u8) {
        self.0.push(x);
    }
    fn u32(&mut self, x: usize) {
        self.0
            .extend_from_slice(&u32::try_from(x).unwrap_or(u32::MAX).to_le_bytes());
    }
    fn u64(&mut self, x: u64) {
        self.0.extend_from_slice(&x.to_le_bytes());
    }
    fn str(&mut self, s: &str) {
        self.u32(s.len());
        self.0.extend_from_slice(s.as_bytes());
    }
    fn strs<S: AsRef<str>>(&mut self, xs: &[S]) {
        self.u32(xs.len());
        for x in xs {
            self.str(x.as_ref());
        }
    }
    fn bound(&mut self, b: &Bound) {
        self.strs(&b.names);
        self.strs(&b.prefixes);
        self.u32(b.cats.len());
        for &(c, k) in b.cats.iter() {
            self.u8(c);
            self.u8(k);
        }
        self.u32(b.means.len());
        for (n, k) in &b.means {
            self.str(n);
            self.u8(*k);
        }
        match &b.state {
            None => self.u8(0),
            Some(st) => {
                self.u8(1);
                self.strs(st);
            }
        }
        self.u8(u8::from(b.no_scope));
        match b.claim {
            None => self.u8(0),
            Some(n) => {
                self.u8(1);
                self.u64(n as u64);
            }
        }
        self.strs(&b.reads);
    }
    fn opt_bound(&mut self, b: Option<&Bound>) {
        match b {
            None => self.u8(0),
            Some(b) => {
                self.u8(1);
                self.bound(b);
            }
        }
    }
    fn hint(&mut self, h: &Hint) {
        self.str(&h.cause);
        self.str(&h.excerpt);
        self.opt_bound(h.bound.as_ref());
        self.opt_bound(h.guess.as_ref());
    }
}

struct Rd<'a>(&'a [u8]);

impl Rd<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        let (a, b) = self.0.split_at_checked(n)?;
        self.0 = b;
        Some(a)
    }
    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|b| b[0])
    }
    fn u32(&mut self) -> Option<usize> {
        let b = self.take(4)?;
        usize::try_from(u32::from_le_bytes(b.try_into().ok()?)).ok()
    }
    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
    /// A count of things each at least `min` bytes long: none beyond
    /// what the bytes left can hold.
    fn count(&mut self, min: usize) -> Option<usize> {
        let n = self.u32()?;
        (n.saturating_mul(min) <= self.0.len()).then_some(n)
    }
    fn str(&mut self) -> Option<String> {
        let n = self.u32()?;
        String::from_utf8(self.take(n)?.to_vec()).ok()
    }
    fn strs(&mut self) -> Option<Vec<String>> {
        let n = self.count(4)?;
        (0..n).map(|_| self.str()).collect()
    }
    fn bound(&mut self) -> Option<Bound> {
        let names = self.strs()?;
        let prefixes = self.strs()?;
        let n = self.count(2)?;
        let cats: Cats = (0..n)
            .map(|_| Some((self.u8()?, self.u8()?)))
            .collect::<Option<Vec<_>>>()?
            .into_iter()
            .collect();
        let n = self.count(5)?;
        let means = (0..n)
            .map(|_| Some((self.str()?, self.u8()?)))
            .collect::<Option<Vec<_>>>()?;
        let state = match self.u8()? {
            0 => None,
            _ => Some(self.strs()?.iter().map(|h| head(h)).collect()),
        };
        let no_scope = self.u8()? != 0;
        let claim = match self.u8()? {
            0 => None,
            _ => Some(usize::try_from(self.u64()?).ok()?),
        };
        let reads = self.strs()?;
        Some(Bound {
            names,
            prefixes,
            cats,
            means,
            state,
            no_scope,
            claim,
            reads,
        })
    }
    /// A bound or none, into `out`; `None` where the bytes end short.
    fn opt_bound(&mut self, out: &mut Option<Bound>) -> Option<()> {
        *out = match self.u8()? {
            0 => None,
            _ => Some(self.bound()?),
        };
        Some(())
    }
    fn hint(&mut self) -> Option<Hint> {
        let mut h = Hint {
            cause: self.str()?,
            excerpt: self.str()?,
            ..Hint::default()
        };
        self.opt_bound(&mut h.bound)?;
        self.opt_bound(&mut h.guess)?;
        Some(h)
    }
    fn flags(&mut self) -> Option<Vec<bool>> {
        let n = self.count(1)?;
        Some(self.take(n)?.iter().map(|&b| b != 0).collect())
    }
}

impl TextHints {
    /// The analysis as bytes ([`TextHints::decode`]).
    pub(crate) fn encode(&self) -> Vec<u8> {
        let mut w = W(MAGIC.to_vec());
        w.u32(self.slices.len());
        for &(s, e) in &self.slices {
            w.u64(s as u64);
            w.u64(e as u64);
        }
        w.u32(self.starts.len());
        for &s in &self.starts {
            w.0.extend_from_slice(&s.to_le_bytes());
        }
        // (the distinct hints, by what they hold, once each: the
        // analysis makes one per atom, most of them alike)
        let mut table: phitex_defuse::FxHashMap<Vec<u8>, usize> =
            phitex_defuse::FxHashMap::default();
        let mut bytes: Vec<u8> = Vec::new();
        let mut by_ptr: phitex_defuse::FxHashMap<usize, usize> =
            phitex_defuse::FxHashMap::default();
        let idx: Vec<usize> = self
            .hints
            .iter()
            .map(|h| match h {
                None => 0,
                Some(h) => *by_ptr.entry(Arc::as_ptr(h) as usize).or_insert_with(|| {
                    let mut one = W(Vec::new());
                    one.hint(h);
                    let n = table.len();
                    *table.entry(one.0).or_insert_with_key(|b| {
                        bytes.extend_from_slice(b);
                        n + 1
                    })
                }),
            })
            .collect();
        w.u32(table.len());
        w.0.extend_from_slice(&bytes);
        w.u32(idx.len());
        for i in idx {
            w.u32(i);
        }
        for flags in [&self.skip, &self.pars] {
            w.u32(flags.len());
            w.0.extend(flags.iter().map(|&b| u8::from(b)));
        }
        let mut table: phitex_defuse::FxHashMap<&str, usize> = phitex_defuse::FxHashMap::default();
        let mut order: Vec<&str> = Vec::new();
        let idx: Vec<usize> = self
            .slice_hints
            .iter()
            .map(|s| {
                *table.entry(&**s).or_insert_with(|| {
                    order.push(s);
                    order.len() - 1
                })
            })
            .collect();
        w.strs(&order);
        w.u32(idx.len());
        for i in idx {
            w.u32(i);
        }
        w.u32(self.sigs.len());
        for &s in &self.sigs {
            w.u64(s);
        }
        w.0
    }

    /// The analysis [`TextHints::encode`] made `b` of; `None` where the
    /// bytes are not one whole.
    pub(crate) fn decode(b: &[u8]) -> Option<TextHints> {
        let mut r = Rd(b.strip_prefix(MAGIC)?);
        let n = r.count(16)?;
        let slices = (0..n)
            .map(|_| {
                Some((
                    usize::try_from(r.u64()?).ok()?,
                    usize::try_from(r.u64()?).ok()?,
                ))
            })
            .collect::<Option<Vec<_>>>()?;
        let n = r.count(4)?;
        let starts = (0..n)
            .map(|_| Some(u32::from_le_bytes(r.take(4)?.try_into().ok()?)))
            .collect::<Option<Vec<_>>>()?;
        let n = r.count(1)?;
        let table = (0..n)
            .map(|_| r.hint().map(Arc::new))
            .collect::<Option<Vec<_>>>()?;
        let n = r.count(4)?;
        let hints = (0..n)
            .map(|_| match r.u32()? {
                0 => Some(None),
                i => Some(Some(table.get(i - 1)?.clone())),
            })
            .collect::<Option<Vec<_>>>()?;
        let skip = r.flags()?;
        let pars = r.flags()?;
        let table: Vec<Arc<str>> = r.strs()?.into_iter().map(Arc::from).collect();
        let n = r.count(4)?;
        let slice_hints = (0..n)
            .map(|_| table.get(r.u32()?).cloned())
            .collect::<Option<Vec<_>>>()?;
        let n = r.count(8)?;
        let sigs = (0..n).map(|_| r.u64()).collect::<Option<Vec<_>>>()?;
        let whole = r.0.is_empty()
            && hints.len() == starts.len()
            && skip.len() == starts.len()
            && sigs.len() == starts.len()
            && pars.len() == slices.len()
            && slice_hints.len() == slices.len();
        whole.then_some(TextHints {
            slices,
            starts,
            hints,
            skip,
            pars,
            slice_hints,
            sigs,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A package's and a document's analyses kept and read back are the
    /// same analysis: the same bytes again, the same hint for every span
    /// and slice.
    #[test]
    fn an_analysis_kept_reads_back_the_same() {
        let package = "\\NeedsTeXFormat{LaTeX2e}\\ProvidesClass{x}\n\\newif\\if@x \\def\\x@a#1{\\gdef\\x@b{#1}}\n\\catcode`\\~=13 \\def~{\\nobreakspace}\n\\csname x@\\romannumeral 1\\endcsname\n\\DeclareOption{a}{\\@xtrue}\\ProcessOptions\\relax\n\\newcommand\\foo[2][x]{#1#2}\\let\\bar\\foo\n\\@ifundefined{baz}{\\def\\baz{}}{}\n\\endinput\n";
        let document = "\\documentclass{article}\n\\begin{document}\nA paragraph \\textbf{with} words.\n\n\\section{S}\\label{s}\nAnother, $x^2$ and \\ref{s}.\n\\count0=3 \\def\\q{\\par}\\q\n\\end{document}\n";
        for (text, kind) in [(package, FileKind::Package), (document, FileKind::Document)] {
            let t = super::super::text_hints(text, kind, super::super::no_defs());
            let b = t.encode();
            let u = TextHints::decode(&b).expect("read back");
            assert_eq!(u.encode(), b, "{kind:?}: the same bytes again");
            assert_eq!(u.slices, t.slices);
            for (k, &(s, e)) in t.slices.iter().enumerate() {
                assert_eq!(u.slice_hint(s), t.slice_hint(s), "{kind:?}: slice {k}");
                assert_eq!(u.at_until(s, e).encode(), t.at_until(s, e).encode());
            }
            for off in 0..text.len() {
                assert_eq!(
                    u.at_until(off, text.len()).encode(),
                    t.at_until(off, text.len()).encode(),
                    "{kind:?}: at {off}"
                );
            }
            assert!(TextHints::decode(&b[..b.len() - 1]).is_none(), "cut short");
            assert_ne!(
                key(text, kind),
                key(text, FileKind::Initex),
                "the kind is in the key"
            );
        }
    }
}
