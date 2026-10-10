//! More material (tex.web parts 47–48), as nodes emitting items: leaders
//! (1078), `\lastbox` (1080–1081), `\vsplit` (977, a pure function of a box
//! value and a height: `crate::split`), `\insert` and `\vadjust`
//! (1097–1100, groups whose packaging node emits the item), `\accent`
//! (1123–1125).

use std::sync::Arc;

use super::assign::prefixable;
use super::material::{BoxContext, BoxGroup, BoxKind, InsParams};
use super::scan::MAX_DIMEN;
use super::{Exec, Kind, Mode};
use crate::item::{BoxNode, Ins, Item, Leaders};
use crate::pack::{Params, Spec, hpack, vpack};
use crate::show::{Place, report, zround};
use crate::val::{DimenPar, GluePar, IntPar, Key, Mat, Meaning, Prim, Tok, Val};

impl Exec<'_> {
    // ------------------------------------------------------------ leaders

    /// tex.web 1078: a leader box or rule is followed by glue of the list's
    /// direction, which becomes glue with leaders.
    pub(super) fn leaders_after(&mut self, sub: u8, b: Item) {
        let v = self.st.mode == Mode::V;
        match self.nonblank_nonrelax() {
            Some((_, Meaning::Prim(Prim::Mat(Mat::Hskip(k))))) if !v => self.append_leaders(k, sub, b),
            Some((_, Meaning::Prim(Prim::Mat(Mat::Vskip(k))))) if v => self.append_leaders(k, sub, b),
            other => {
                self.err("Leaders not followed by proper glue".into());
                if let Some((t, _)) = other {
                    self.back(t);
                }
            }
        }
    }

    fn append_leaders(&mut self, k: u8, sub: u8, b: Item) {
        let (g, zero) = self.skip_glue(k);
        self.emit_here(Item::Leaders(Arc::new(Leaders { g: Arc::new(g), sub, b, zero })));
    }

    // ------------------------------------------------------------ \lastbox

    /// `\lastbox` (tex.web 1080–1081): the current list's last box, unless
    /// a discretionary replaces it, is removed (an `Item::Remove(3)` its
    /// gatherer applies) and is the result, unshifted. On the main vertical
    /// list TeX's list is the contributions since the page builder was last
    /// called; when there are none, an error. A read of material (CORE.md
    /// Conflict 13): the node runs again with its paragraph.
    pub(super) fn last_box(&mut self) -> Option<BoxNode> {
        if self.kind == Kind::Other {
            self.kind = Kind::Par;
        }
        let v = self.st.mode == Mode::V;
        let list = self.current_list(v);
        let mut from = 0;
        if v && self.st.level == 0 && !self.st.inner {
            from = list.iter().rposition(|i| matches!(i, Item::BuildPage(_))).map_or(0, |p| p + 1);
        }
        let at = list[from..]
            .iter()
            .rposition(|i| !matches!(i, Item::BuildPage(_) | Item::End(_) | Item::Shipout(_)))
            .map(|p| p + from);
        let Some(at) = at else {
            if v && self.st.level == 0 && !self.st.inner {
                self.err("You can't use `\\lastbox' in vertical mode".into());
            }
            return None;
        };
        let Item::Box(b) = &list[at] else { return None };
        let replaced =
            list[..at].iter().enumerate().any(|(j, it)| matches!(it, Item::Disc(d) if j + usize::from(d.replace) >= at));
        if replaced {
            return None;
        }
        let mut b = (**b).clone();
        b.shift = 0;
        self.emit_here(Item::Remove(3));
        Some(b)
    }

    // ------------------------------------------------------------ \vsplit

    /// `\vsplit n to h` (tex.web 977): the register's list up to its best
    /// break for `h` (`split::vsplit`), packed to `h` with `\splitmaxdepth`
    /// (reported with `\vbadness`, `\vfuzz`); the rest stays in the
    /// register, packed, at the scope of the definition it replaces
    /// (CORE.md Conflict 7); the split marks of every class are defined,
    /// globally, from the marks of the part split off.
    pub(super) fn vsplit_box(&mut self) -> Option<BoxNode> {
        let n = self.scan_eight_bit_int();
        if !self.keyword("to") {
            self.err("Missing `to' inserted".into());
        }
        let h = self.scan_dimen();
        let v = match self.get(&Key::Box(n)) {
            Val::Box(b) if b.vertical => b,
            other => {
                if matches!(other, Val::Box(_)) {
                    self.err("\\vsplit needs a \\vbox".into());
                }
                self.define(Key::SplitMarks, Val::SplitMarks(Arc::from(Vec::new())), true);
                return None;
            }
        };
        let d = self.dimen_par(DimenPar::Splitmaxdepth);
        let top = self.glue_par(GluePar::Splittopskip);
        let s = crate::split::vsplit(&v.list, h, d, &top);
        if s.bad_shrink {
            self.err("Infinite glue shrinkage found in box being split".into());
        }
        self.define(Key::SplitMarks, Val::SplitMarks(Arc::from(s.marks)), true);
        let rest = if s.rest.is_empty() {
            Val::Void
        } else {
            Val::Box(Arc::new(vpack(s.rest, Spec::NATURAL, MAX_DIMEN, &Params::QUIET).b))
        };
        self.change_box(n, rest);
        let params = Params {
            badness: self.int_par(IntPar::Vbadness),
            fuzz: self.dimen_par(DimenPar::Vfuzz),
            overfull_rule: 0,
        };
        let packed = vpack(s.first, Spec::Exactly(h), d, &params);
        if let Some(r) = packed.report {
            let cx = self.show_ctx(std::slice::from_ref(&Item::Box(Arc::new(packed.b.clone()))));
            let txt = report(r, &packed.b, Place::Here, &cx);
            self.log_raw(&txt);
        }
        Some(packed.b)
    }

    // ------------------------------------------------------------ \insert, \vadjust

    /// `\insert n{…}`, `\vadjust{…}` (tex.web 1097–1099): a group holding
    /// an internal vertical list, `normal_paragraph` inside it.
    pub(super) fn begin_insert(&mut self, vadjust: bool) {
        let n = if vadjust {
            255
        } else {
            let n = self.scan_eight_bit_int();
            if n == 255 {
                self.err("You can't \\insert255".into());
                0
            } else {
                n
            }
        };
        let g = BoxGroup {
            kind: BoxKind::Insert(n as u8, None),
            context: BoxContext::Shift(0),
            spec: Spec::NATURAL,
            adjusted: false,
            mode: self.st.mode,
            inner: self.st.inner,
            space_factor: self.st.space_factor,
            prev_depth: self.st.prev_depth,
            hyph_lang: self.st.hyph_lang,
            prev_graf: self.st.prev_graf,
        };
        self.scan_left_brace();
        let shaped = self.paragraph_shaped();
        self.open_box(g, Mode::V);
        if shaped {
            let t = self.cs("\u{0}normalparagraph");
            self.back(t);
        }
    }

    /// The end of an insertion's or adjustment's group (tex.web 1100): its
    /// list, packed at its natural size, is the item (`\insert`: with the
    /// parameters its `}` read); on the main vertical list the page builder
    /// is called.
    pub(super) fn insert_end(&mut self, n: u8, p: Option<&InsParams>, list: Vec<Item>, ships: Vec<Item>) {
        for s in ships {
            self.emit_here(s);
        }
        let b = vpack(list, Spec::NATURAL, MAX_DIMEN, &Params::QUIET).b;
        let it = if n == 255 {
            Item::Adjust(Arc::new(b.list))
        } else {
            let (split_top, depth, float_cost) = match p {
                Some(p) => (p.split_top.clone(), p.depth, p.float),
                None => (Arc::default(), 0, 0),
            };
            Item::Ins(Arc::new(Ins { n, height: b.h + b.d, depth, split_top, float_cost, list: b.list }))
        };
        self.emit_here(it);
        self.build_page();
    }

    // ------------------------------------------------------------ \accent

    /// `\accent` (tex.web 1123–1125): the accent character over the next
    /// character (after any assignments, which may change the font),
    /// shifted to its height and centred with kerns by the fonts' slants.
    pub(super) fn make_accent(&mut self) {
        let c = self.scan_char_num();
        let f = self.font();
        let Some(ga) = f.glyph(c) else {
            self.char_warning(&f, c);
            return;
        };
        let x = self.int(&Key::FontDimen(f.key.clone(), 5));
        let s = f64::from(self.int(&Key::FontDimen(f.key.clone(), 1))) / 65536.0;
        let a = ga.width;
        let next = self.do_assignments();
        // (§1124: the character accented, in the font now current)
        let f2 = self.font();
        let q = match next {
            Some((_, Meaning::Char(11 | 12, ch))) => Some(u32::from(ch) as i32),
            Some((_, Meaning::CharGiven(ch))) => Some(ch as i32),
            Some((_, Meaning::Prim(Prim::Char))) => Some(self.scan_char_num()),
            Some((t, _)) => {
                self.back(t);
                None
            }
            None => None,
        };
        let q = q.and_then(|q| {
            let g = f2.glyph(q);
            if g.is_none() {
                self.char_warning(&f2, q);
            }
            g.map(|g| (q, g))
        });
        let pa = Item::Char(f.clone(), c as u8);
        match q {
            Some((qc, gq)) => {
                // (§1125)
                let t = f64::from(self.int(&Key::FontDimen(f2.key.clone(), 1))) / 65536.0;
                let (w, h) = (gq.width, gq.height);
                let p = if h == x {
                    pa
                } else {
                    let mut b = hpack(vec![pa], Spec::NATURAL, &Params::QUIET, None).b;
                    b.shift = x - h;
                    Item::Box(Arc::new(b))
                };
                let delta = zround(f64::from(w - a) / 2.0 + f64::from(h) * t - f64::from(x) * s);
                self.emit.push(Item::Kern { w: delta, sub: 2 });
                self.emit.push(p);
                self.emit.push(Item::Kern { w: -a - delta, sub: 2 });
                self.emit.push(Item::Char(f2, qc as u8));
            }
            None => self.emit.push(pa),
        }
        self.st.space_factor = 1000;
    }

    /// tex.web 1270: `do_assignments`: assignments (with their prefixes)
    /// are done in this node up to the first token that is not one, which
    /// is returned, read; `\setbox` is not allowed.
    fn do_assignments(&mut self) -> Option<(Tok, Meaning)> {
        'next: loop {
            let (mut t, mut m) = self.nonblank_nonrelax()?;
            if !prefixable(&m) {
                return Some((t, m));
            }
            let (mut g, mut l, mut o, mut pr) = (false, false, false, false);
            while let Meaning::Prim(p @ (Prim::Global | Prim::Long | Prim::Outer | Prim::Protected)) = m {
                match p {
                    Prim::Global => g = true,
                    Prim::Long => l = true,
                    Prim::Outer => o = true,
                    _ => pr = true,
                }
                (t, m) = self.nonblank_nonrelax()?;
                if !prefixable(&m) {
                    self.err(format!("You can't use a prefix with `{}'", self.shown(&t)));
                    self.back(t);
                    continue 'next;
                }
            }
            if m == Meaning::Prim(Prim::Mat(Mat::SetBox)) {
                self.err("Improper \\setbox".into());
                continue;
            }
            self.prefixed(&t, &m, g, l, o, pr);
        }
    }
}
