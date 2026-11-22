//! The markup: the new body with the old one's differences marked as
//! latexdiff marks them, and the change list.

use crate::myers::patience as lcs;
use crate::sig::{Class, EnvKind, Signatures};
use crate::tok::{Kind, SECTIONS, Tok};
use std::hash::Hash;

/// Common runs shorter than this between two changes are made part of
/// them (latexdiff's `MINWORDSBLOCK`).
const MIN_BLOCK: usize = 3;

/// Which side a run is marked as.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Side {
    Add,
    Del,
}

/// The context of a run: in a float or an alignment (`fl`: the `FL`
/// markup); in a list (`list`: a deleted item is shown as an item, its
/// text struck out, as latexdiff shows it; in a numbered list or a
/// bibliography as `\item[]`, which takes no number).
#[derive(Clone, Copy, Default)]
struct Ctx {
    fl: bool,
    list: List,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum List {
    #[default]
    None,
    Bullets,
    Numbered,
}

impl Ctx {
    /// The context inside node `t`.
    fn enter(self, t: &Tok) -> Ctx {
        match &t.kind {
            Kind::Env { name, kind } => Ctx {
                fl: self.fl || matches!(kind, EnvKind::Float | EnvKind::Tabular),
                list: match (kind, name.as_str()) {
                    (EnvKind::List, "itemize" | "description") => List::Bullets,
                    (EnvKind::List, _) => List::Numbered,
                    _ => List::None,
                },
            },
            _ => self,
        }
    }
}

/// A change, in flat offsets.
#[derive(Clone, Debug)]
pub struct RawChange {
    pub old: (usize, usize),
    pub new: (usize, usize),
    pub out: (usize, usize),
    pub section: Option<String>,
}

/// The markup's writer.
pub struct Em<'a> {
    pub old: &'a str,
    pub new: &'a str,
    pub sigs: &'a Signatures,
    /// The new preamble loads amsmath (`align*` is there for deleted
    /// alignments).
    pub amsmath: bool,
    pub out: String,
    pub changes: Vec<RawChange>,
    /// The section the text written is in (the title of the last
    /// sectioning command written).
    pub section: Option<String>,
    /// A line end is owed before the next text (a comment was written
    /// last): unless that text begins with one.
    pub nl: bool,
    /// The runs of `out` copied from the new text as they are: (where in
    /// `out`, length, where in `new`), in order, for the map back to the
    /// user's files ([`crate::DiffOut::map`]).
    pub spans: Vec<(usize, usize, usize)>,
    /// Where each added token's text was written inside the markup of a
    /// run of added text (`\DIFadd{…}`): (where in `new`, where in `out`,
    /// the markup command it is inside, none: at a place outside any),
    /// in order. A cut there ([`crate::split`]) closes the markup and
    /// opens it again after the cut.
    pub adds: Vec<(usize, usize, Option<&'static str>)>,
    /// The added tokens in the buffer not written yet: where in `new`,
    /// where in the buffer.
    pending: Vec<(usize, usize)>,
    /// Whether the look shows deleted text ([`crate::Markup::shows_deleted`]):
    /// if not, a deleted float, display or picture is commented out, not
    /// shown as a unit.
    pub show_del: bool,
    /// An image replaced by another: beside it, under it, or not shown.
    pub fig: crate::FigStyle,
}

impl<'a> Em<'a> {
    #[must_use]
    pub fn new(old: &'a str, new: &'a str, sigs: &'a Signatures, amsmath: bool) -> Em<'a> {
        Em {
            old,
            new,
            sigs,
            amsmath,
            out: String::new(),
            changes: Vec::new(),
            section: None,
            nl: false,
            spans: Vec::new(),
            adds: Vec::new(),
            pending: Vec::new(),
            show_del: true,
            fig: crate::FigStyle::Side,
        }
    }

    /// The added text at `at` in `new` is at `off` in the buffer.
    fn added(&mut self, side: Side, at: usize, off: usize) {
        if side == Side::Add {
            self.pending.push((at, off));
        }
    }

    /// Write `s`, after the line end a comment is owed.
    pub fn push(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }
        if self.nl && !s.starts_with('\n') {
            self.out.push('\n');
        }
        self.nl = false;
        // (a slice of the new text, written as it is: where it came from;
        // markup and words gathered into a buffer are not)
        let (here, new) = (s.as_ptr() as usize, self.new.as_ptr() as usize);
        if here >= new && here + s.len() <= new + self.new.len() {
            let (at, from) = (self.out.len(), here - new);
            match self.spans.last_mut() {
                Some((o, len, f)) if *o + *len == at && *f + *len == from => *len += s.len(),
                _ => self.spans.push((at, s.len(), from)),
            }
        }
        self.out.push_str(s);
    }

    /// The output, the line end owed written.
    pub fn finish(mut self) -> (String, Vec<RawChange>) {
        if self.nl {
            self.out.push('\n');
        }
        (self.out, self.changes)
    }

    /// The body: paragraphs aligned first (a blank line ends one), then
    /// each run of unmatched ones diffed by tokens. `old_end`, `new_end`:
    /// where the bodies end.
    pub fn body(&mut self, old: &[Tok], new: &[Tok], old_end: usize, new_end: usize) {
        let oc = chunks(old);
        let nc = chunks(new);
        let ok: Vec<u64> = oc.iter().map(|r| chunk_key(&old[r.clone()])).collect();
        let nk: Vec<u64> = nc.iter().map(|r| chunk_key(&new[r.clone()])).collect();
        let pairs = lcs(&ok, &nk);
        let (mut i, mut j) = (0, 0);
        for (pi, pj) in pairs
            .into_iter()
            .chain(std::iter::once((oc.len(), nc.len())))
        {
            if i < pi || j < pj {
                let oa = oc.get(i).map_or(old.len(), |r| r.start);
                let ob = oc.get(pi).map_or(old.len(), |r| r.start);
                let na = nc.get(j).map_or(new.len(), |r| r.start);
                let nb = nc.get(pj).map_or(new.len(), |r| r.start);
                let oe = old.get(ob).map_or(old_end, |t| t.start);
                let ne = new.get(nb).map_or(new_end, |t| t.start);
                self.run(&old[oa..ob], &new[na..nb], oe, ne);
            }
            if pi < oc.len() {
                for t in &new[nc[pj].clone()] {
                    self.same(t);
                }
            }
            i = pi + 1;
            j = pj + 1;
        }
    }

    /// A run of paragraphs that differ, diffed by tokens (`old_end`,
    /// `new_end`: where the tokens after them begin).
    pub fn run(&mut self, old: &[Tok], new: &[Tok], old_end: usize, new_end: usize) {
        self.seq(old, new, old_end, new_end, Ctx::default());
    }

    /// Diff two token sequences (ending at `old_end`, `new_end`).
    fn seq(&mut self, old: &[Tok], new: &[Tok], old_end: usize, new_end: usize, ctx: Ctx) {
        let ok: Vec<u64> = old.iter().map(|t| t.key).collect();
        let nk: Vec<u64> = new.iter().map(|t| t.key).collect();
        let pairs = merge_short(&lcs(&ok, &nk), old, new);
        let (mut i, mut j) = (0, 0);
        for (pi, pj) in pairs
            .into_iter()
            .chain(std::iter::once((old.len(), new.len())))
        {
            if i < pi || j < pj {
                let oe = old.get(pi).map_or(old_end, |t| t.start);
                let ne = new.get(pj).map_or(new_end, |t| t.start);
                self.hunk(&old[i..pi], &new[j..pj], oe, ne, ctx);
            }
            if pi < old.len() {
                self.same(&new[pj]);
            }
            i = pi + 1;
            j = pj + 1;
        }
    }

    /// A run of changes: nodes with the same prefix (a command whose text
    /// changed, an environment whose body did) are paired and diffed
    /// inside; the rest is deleted and added.
    fn hunk(&mut self, old: &[Tok], new: &[Tok], old_end: usize, new_end: usize, ctx: Ctx) {
        // (only nodes pair: the leaves were not common, or the first diff
        // would have matched them)
        // (a citation or a reference list pairs with one of the same
        // command and options: its keys are diffed)
        let key = |t: &Tok, text: &str, side: usize, i: usize| {
            if t.is_node() {
                (0, pair_key(t, text))
            } else if let Some(h) = cite_head(t, text) {
                (3, h)
            } else if let Some(h) = ref_head(t, text) {
                (5, h)
            } else if let Kind::Display { env, .. } = &t.kind
                && ROW_ENVS.contains(&env.as_str())
            {
                use std::hash::{DefaultHasher, Hasher};
                let mut h = DefaultHasher::new();
                env.hash(&mut h);
                (4, h.finish())
            } else {
                (side, i as u64)
            }
        };
        let ok: Vec<_> = old
            .iter()
            .enumerate()
            .map(|(i, t)| key(t, self.old, 1, i))
            .collect();
        let nk: Vec<_> = new
            .iter()
            .enumerate()
            .map(|(i, t)| key(t, self.new, 2, i))
            .collect();
        let pairs = lcs(&ok, &nk);
        let (mut i, mut j) = (0, 0);
        for (pi, pj) in pairs
            .into_iter()
            .chain(std::iter::once((old.len(), new.len())))
        {
            if i < pi || j < pj {
                let oe = old.get(pi).map_or(old_end, |t| t.start);
                let ne = new.get(pj).map_or(new_end, |t| t.start);
                self.block(&old[i..pi], &new[j..pj], oe, ne, ctx);
            }
            if pi < old.len() {
                let (o, n) = (&old[pi], &new[pj]);
                let ne = new.get(pj + 1).map_or(new_end, |t| t.start);
                let oe = old.get(pi + 1).map_or(old_end, |t| t.start);
                if o.key == n.key {
                    self.same(n);
                } else if matches!(n.kind, Kind::Display { .. }) {
                    self.display_pair(o, n);
                } else if ref_head(n, self.new).is_some() {
                    // (a reference whose label was renamed: the same
                    // thing referred to, nothing to mark)
                    if self.renamed(o, n) {
                        self.same(n);
                    } else {
                        self.block(
                            std::slice::from_ref(o),
                            std::slice::from_ref(n),
                            oe,
                            ne,
                            ctx,
                        );
                    }
                } else if !n.is_node() {
                    self.cite(o, n, oe, ne, ctx);
                } else if alike(o, n) {
                    self.pair(o, n, ctx);
                } else {
                    // (two different floats: the old one deleted, the new
                    // one added, each whole)
                    self.block(
                        std::slice::from_ref(o),
                        std::slice::from_ref(n),
                        oe,
                        ne,
                        ctx,
                    );
                }
            }
            i = pi + 1;
            j = pj + 1;
        }
    }

    /// Two citations (or reference lists) of the same command and options
    /// whose keys differ: one citation of the keys both have, as the new
    /// one orders them, then the keys only the old one has, deleted, and
    /// those only the new one has, added (each a short citation of a few
    /// keys, so that the markup breaks between them). With no key in
    /// common, the old one deleted and the new one added.
    fn cite(&mut self, o: &Tok, n: &Tok, old_end: usize, new_end: usize, ctx: Ctx) {
        let os = &self.old[o.start..o.cend];
        let ns = &self.new[n.start..n.cend];
        let (Some(ol), Some(nl)) = (key_list(os), key_list(ns)) else {
            self.block(
                std::slice::from_ref(o),
                std::slice::from_ref(n),
                old_end,
                new_end,
                ctx,
            );
            return;
        };
        let okeys = keys(&os[ol.1..ol.2]);
        let nkeys = keys(&ns[nl.1..nl.2]);
        let common: Vec<&str> = nkeys
            .iter()
            .copied()
            .filter(|k| okeys.contains(k))
            .collect();
        let gone: Vec<&str> = okeys
            .iter()
            .copied()
            .filter(|k| !nkeys.contains(k))
            .collect();
        let came: Vec<&str> = nkeys
            .iter()
            .copied()
            .filter(|k| !okeys.contains(k))
            .collect();
        if common.is_empty() {
            self.block(
                std::slice::from_ref(o),
                std::slice::from_ref(n),
                old_end,
                new_end,
                ctx,
            );
            return;
        }
        if gone.is_empty() && came.is_empty() {
            // (the same keys, reordered or spaced otherwise)
            self.same(n);
            return;
        }
        let start = self.out.len();
        let head = &ns[..nl.0];
        // (a bracketed citation, `\cite`/`\citep` with no option: one
        // citation, its keys removed and added marked inside it where the
        // citations are natbib's numbers, `\DIFcite{beside}{inside}`;
        // else beside it)
        let merged = matches!(head.trim_end(), "\\cite" | "\\citep");
        if merged {
            // (`\citep` is bracketed in every natbib mode, `\cite` in its
            // numbers only)
            self.push(if head.trim_end() == "\\citep" {
                "\\DIFcitep{"
            } else {
                "\\DIFcite{"
            });
        }
        self.push(head);
        self.push(&format!("{{{}}}", common.join(",")));
        let parts: Vec<(Side, &Vec<&str>)> = [(Side::Del, &gone), (Side::Add, &came)]
            .into_iter()
            .filter(|(side, ks)| !ks.is_empty() && (*side == Side::Add || self.show_del))
            .collect();
        for &(side, ks) in &parts {
            self.push(" ");
            let mut buf = cite_chunks(head, ks);
            self.flush(&mut buf, side, ctx);
        }
        if merged {
            self.push(&format!("}}{{\\citealp{{{}}}", common.join(",")));
            for &(side, ks) in &parts {
                self.push("\\DIFcitesep ");
                let mut buf =
                    cite_chunks("\\citealp", ks).replace(" \\mbox", "\\DIFcitesep \\mbox");
                self.flush(&mut buf, side, ctx);
            }
            self.push("}");
        }
        self.push(&self.new[n.cend..n.end]);
        let section = self.section.clone();
        self.changes.push(RawChange {
            old: (o.start, o.cend),
            new: (n.start, n.cend),
            out: (start, self.out.len()),
            section,
        });
    }

    /// Two nodes with the same prefix: the new one's open and close parts,
    /// and their children diffed.
    ///
    /// A changed alignment is boxed (`\DIFtabularbegin` … `\DIFtabularend`)
    /// and, if its cells' old and new text make it wider than the line,
    /// scaled down to the line's width: it stays in its column.
    fn pair(&mut self, o: &Tok, n: &Tok, ctx: Ctx) {
        let (first, at) = (self.changes.len(), self.out.len());
        self.pair_inside(o, n, ctx);
        // (a float changed inside is one change, one stop for the viewer's
        // next change: the whole float)
        if is_float(n) && self.changes.len() > first {
            let section = self.changes[first].section.clone();
            self.changes.truncate(first);
            self.changes.push(RawChange {
                old: (o.start, o.cend),
                new: (n.start, n.cend),
                out: (at, self.out.len()),
                section,
            });
        }
    }

    fn pair_inside(&mut self, o: &Tok, n: &Tok, ctx: Ctx) {
        self.note_section(n);
        let boxed =
            matches!(&n.kind, Kind::Env { name, kind: EnvKind::Tabular } if name != "longtable");
        let ctx = ctx.enter(n);
        if boxed {
            self.push("\\DIFtabularbegin ");
        }
        self.push(&self.new[n.start..n.open_end]);
        self.seq(&o.children, &n.children, o.close_start, n.close_start, ctx);
        if boxed {
            self.push(&self.new[n.close_start..n.cend]);
            self.unit_end("\\DIFtabularend", &self.new[n.cend..n.end]);
        } else {
            self.push(&self.new[n.close_start..n.end]);
        }
    }

    /// A token the same on both sides.
    fn same(&mut self, n: &Tok) {
        self.note_section(n);
        self.push(&self.new[n.start..n.end]);
    }

    fn note_section(&mut self, n: &Tok) {
        if let Kind::Text { name, .. } = &n.kind
            && SECTIONS.contains(&name.as_str())
        {
            self.section = Some(self.new[n.open_end..n.close_start].trim().to_owned());
        }
    }

    /// Deleted tokens `old` and added `new` (where they are when empty:
    /// `old_at`, `new_at`): one change.
    fn block(&mut self, old: &[Tok], new: &[Tok], old_at: usize, new_at: usize, ctx: Ctx) {
        // (comments and labels alone: nothing the pages show changed, so
        // no markup and no change; the new text as it is)
        let quiet = |t: &Tok, src: &str| {
            matches!(t.kind, Kind::Comment | Kind::Ws)
                || (t.kind == Kind::Unsafe && cs_at(&src[t.start..t.cend], 0).0 == "label")
        };
        if !(old.is_empty() && new.is_empty())
            && old.iter().all(|t| quiet(t, self.old))
            && new.iter().all(|t| quiet(t, self.new))
        {
            for t in new {
                self.same(t);
            }
            return;
        }
        let span = |t: &[Tok], at: usize| match (t.first(), t.last()) {
            (Some(a), Some(b)) => (a.start, b.cend),
            _ => (at, at),
        };
        let o = span(old, old_at);
        let n = span(new, new_at);
        let start = self.out.len();
        let fl = if ctx.fl { "FL" } else { "" };
        // (a deletion the old text set apart from the word before it,
        // that word now written with no space after it: set apart still)
        let spaced = |t: &str, at: usize| t[..at].ends_with(char::is_whitespace);
        if old.first().is_some_and(|t| spaced(self.old, t.start))
            && self.out.ends_with(|c: char| {
                c.is_alphanumeric() || matches!(c, '.' | ',' | ';' | ':' | ')' | ']')
            })
        {
            self.push("\\DIFchangesep ");
        }
        // (an image replaced by another: the two side by side, scaled
        // together to the line if they are wider)
        let image = |t: &[Tok], src: &str| {
            let mut it = t.iter().filter(|t| t.kind != Kind::Ws);
            it.next().is_some_and(|t| is_image(&src[t.start..t.cend])) && it.next().is_none()
        };
        let pair = image(old, self.old) && image(new, self.new) && self.show_del;
        // (the look's: the new image alone, or the old one under it)
        let fig = if pair {
            self.fig
        } else {
            crate::FigStyle::Side
        };
        if pair {
            self.push("\\DIFtabularbegin ");
        }
        if fig == crate::FigStyle::Over {
            self.push("\\DIFoverbegin ");
        }
        if !old.is_empty() && fig != crate::FigStyle::New {
            self.push(&format!("\\DIFdelbegin{fl} "));
            self.mark(old, Side::Del, ctx);
            self.push(&format!("\\DIFdelend{fl} "));
            if fig == crate::FigStyle::Over {
                self.push("\\DIFoverend ");
            }
            // (deleted text right before added text with no space between
            // is set apart from it: `fooarbar` reads as one word)
            else if !new.is_empty() && old.last().is_some_and(|t| t.end == t.cend) {
                self.push("\\DIFchangesep ");
            }
        } else if fig == crate::FigStyle::Over {
            self.push("\\DIFoverend ");
        }
        if !new.is_empty() {
            self.push(&format!("\\DIFaddbegin{fl} "));
            self.mark(new, Side::Add, ctx);
            self.push(&format!("\\DIFaddend{fl} "));
        }
        if pair {
            self.push("\\DIFtabularend ");
        }
        // (the section a change is in: the last one before it, or its own)
        let section = self.section.clone();
        self.changes.push(RawChange {
            old: o,
            new: n,
            out: (start, self.out.len()),
            section,
        });
    }

    fn text(&self, side: Side) -> &'a str {
        match side {
            Side::Add => self.new,
            Side::Del => self.old,
        }
    }

    /// Mark tokens as added or deleted: runs of those that can sit in the
    /// markup in `\DIFadd{…}`/`\DIFdel{…}`; the others outside (added) or
    /// commented out (deleted); nodes opened and marked inside.
    #[allow(clippy::too_many_lines)]
    fn mark(&mut self, toks: &[Tok], side: Side, ctx: Ctx) {
        let src = self.text(side);
        let mut buf = String::new();
        for t in toks {
            let text = &src[t.start..t.end];
            match &t.kind {
                Kind::Word | Kind::Safe | Kind::Math
                    if side == Side::Del && self.old_only(text) =>
                {
                    self.flush(&mut buf, side, ctx);
                    self.comment_out(text);
                }
                Kind::Word | Kind::Safe | Kind::Math | Kind::Ws => {
                    self.added(side, t.start, buf.len());
                    // (and the white space after it: a file may end there)
                    if t.cend > t.start && t.cend < t.end {
                        self.added(side, t.cend, buf.len() + (t.cend - t.start));
                    }
                    buf.push_str(text);
                }
                Kind::Mbox => {
                    self.added(side, t.start, buf.len());
                    let body = &src[t.start..t.cend];
                    match key_list(body) {
                        Some((h, a, b)) if !body[..h].contains('[') => {
                            buf.push_str(&cite_chunks(&body[..h], &keys(&body[a..b])));
                        }
                        _ => {
                            buf.push_str("\\mbox{");
                            buf.push_str(body);
                            buf.push('}');
                        }
                    }
                    if t.cend < t.end {
                        self.added(side, t.cend, buf.len());
                    }
                    buf.push_str(&src[t.cend..t.end]);
                }
                Kind::Comment => {
                    self.flush(&mut buf, side, ctx);
                    self.push(text);
                    self.nl = !text.ends_with('\n');
                }
                Kind::Picture => {
                    self.flush(&mut buf, side, ctx);
                    self.picture(t, side);
                }
                Kind::Unsafe
                    if side == Side::Del
                        && self.show_del
                        && (shown_whole(text) || self.user_macro(text))
                        && !self.old_only(text) =>
                {
                    self.flush(&mut buf, side, ctx);
                    self.whole(t, side, false);
                }
                Kind::Unsafe if side == Side::Add && is_image(text) => {
                    self.flush(&mut buf, side, ctx);
                    self.whole(t, side, false);
                }
                Kind::Unsafe | Kind::Rule | Kind::Par | Kind::Amp | Kind::RowEnd => {
                    self.flush(&mut buf, side, ctx);
                    match side {
                        // (an added item's label marked: `\item[\DIFadd{x}]`)
                        Side::Add => match item_label(text) {
                            Some((a, b)) => {
                                self.push(&text[..a]);
                                let mut label = text[a..b].to_owned();
                                self.flush(&mut label, side, ctx);
                                self.push(&text[b..]);
                            }
                            None => self.push(text),
                        },
                        // (a deleted paragraph's end: still one, so that
                        // a deleted paragraph is a paragraph, not run into
                        // the next)
                        Side::Del if t.kind == Kind::Par && self.show_del && !ctx.fl => {
                            self.push(text);
                        }
                        Side::Del => {
                            self.comment_out(text);
                            // (a deleted item: an item still, its label
                            // struck out)
                            if ctx.list == List::Numbered && is_item(text) {
                                self.push("\\item[] ");
                            } else if ctx.list == List::Bullets && is_item(text) {
                                self.push("\\item");
                                if let Some((a, b)) = item_label(text) {
                                    self.push("[");
                                    let mut label = text[a..b].to_owned();
                                    self.flush(&mut label, side, ctx);
                                    self.push("]");
                                }
                                self.push(" ");
                            }
                        }
                    }
                }
                Kind::Display { env, arr, body } => {
                    self.flush(&mut buf, side, ctx);
                    self.display(t, env, *arr, *body, side);
                }
                Kind::Text { name, class, star } => {
                    self.flush(&mut buf, side, ctx);
                    if side == Side::Add {
                        self.note_section(t);
                    }
                    match (side, class) {
                        (Side::Del, _) if self.sigs.old_only(name) => self.comment_out(text),
                        (Side::Del, Class::Context2) => self.comment_out(text),
                        (Side::Del, Class::Context1) => {
                            self.mark(&t.children, side, ctx);
                            self.push(&src[t.cend..t.end]);
                        }
                        (Side::Del, Class::Text { counter: true }) if !star => {
                            self.push(&src[t.start..t.open_end]);
                            self.mark(&t.children, side, ctx);
                            self.push(&src[t.close_start..t.cend]);
                            self.push(&format!("\\addtocounter{{{name}}}{{-1}}"));
                            self.push(&src[t.cend..t.end]);
                        }
                        _ => self.node(t, side, ctx),
                    }
                }
                Kind::Group => {
                    self.flush(&mut buf, side, ctx);
                    self.node(t, side, ctx);
                }
                Kind::Row => {
                    self.flush(&mut buf, side, ctx);
                    self.row(t, side, ctx);
                }
                Kind::Env { name, kind } => {
                    self.flush(&mut buf, side, ctx);
                    let inner = ctx.enter(t);
                    match (side, kind) {
                        // (a float added or deleted: whole, in a frame)
                        (_, EnvKind::Float) => self.float(t, side),
                        // (an alignment added or deleted whole: as it is,
                        // coloured, its cells never cut apart)
                        (_, EnvKind::Tabular) if side == Side::Add || self.show_del => {
                            self.alignment(t, side);
                        }
                        (Side::Add, _) => self.node(t, side, inner),
                        (Side::Del, EnvKind::Tabular) => self.comment_out(text),
                        // (a list or a layout environment deleted: still
                        // one, its text struck out)
                        (Side::Del, EnvKind::List) if !self.sigs.old_only(name) => {
                            self.node(t, side, inner);
                        }
                        (Side::Del, EnvKind::Plain) if LAYOUT.contains(&name.as_str()) => {
                            self.node(t, side, inner);
                        }
                        (Side::Del, _) => {
                            // (its items are not items any more)
                            let inner = Ctx {
                                list: List::None,
                                ..inner
                            };
                            self.comment_out(&src[t.start..t.open_end]);
                            self.mark(&t.children, side, inner);
                            self.comment_out(&src[t.close_start..t.end]);
                        }
                    }
                }
            }
        }
        self.flush(&mut buf, side, ctx);
    }

    /// A node marked inside: its open and close parts as they are.
    fn node(&mut self, t: &Tok, side: Side, ctx: Ctx) {
        let src = self.text(side);
        self.push(&src[t.start..t.open_end]);
        self.mark(&t.children, side, ctx);
        self.push(&src[t.close_start..t.end]);
    }

    /// An alignment's row: a deleted one is shown, its cells struck out,
    /// its `&` and `\\` kept (its rules commented out).
    fn row(&mut self, t: &Tok, side: Side, ctx: Ctx) {
        if side == Side::Add {
            self.mark(&t.children, side, ctx);
            return;
        }
        let src = self.old;
        let mut run: Vec<Tok> = Vec::new();
        for k in &t.children {
            if matches!(k.kind, Kind::Amp | Kind::RowEnd) {
                self.mark(&run, side, ctx);
                run.clear();
                self.push(&src[k.start..k.end]);
            } else {
                run.push(k.clone());
            }
        }
        self.mark(&run, side, ctx);
    }

    /// A float added or deleted: whole, as it is typeset, in a frame
    /// (`\DIFblockaddbegin` … `\DIFblockaddend`, `\DIFblockdel…`) inside
    /// its own environment, so that it floats as it did. A deleted one's
    /// labels are dropped (the new version's are the labels) and its
    /// captions step their counter back after it: the new version's floats
    /// keep their numbers. One the look does not show, or that needs a
    /// command only the old preamble defines, is commented out.
    fn float(&mut self, t: &Tok, side: Side) {
        let src = self.text(side);
        let text = &src[t.start..t.end];
        let (open, body, close) = (
            &src[t.start..t.open_end],
            &src[t.open_end..t.close_start],
            &src[t.close_start..t.end],
        );
        match side {
            Side::Add => {
                self.push(open);
                self.push("\\DIFblockaddbegin ");
                self.push(body);
                self.push("\\DIFblockaddend ");
                self.push(close);
            }
            Side::Del if !self.show_del || self.old_only(text) => self.comment_out(text),
            Side::Del => {
                // (numbered as it was in the old version, the counter
                // given back after it)
                let numbered = captions(body) > 0;
                self.push(&near_placement(open));
                if numbered {
                    self.push(&format!("\\DIFfloatnumber{{{}}}", self.old_number(t)));
                }
                self.push("\\DIFblockdelbegin ");
                self.push(&self.unlabel(body));
                self.push("\\DIFblockdelend ");
                if numbered {
                    self.push("\\DIFfloatrestore ");
                }
                self.push(close);
            }
        }
    }

    /// An alignment added or deleted whole: as it is, coloured
    /// (`\DIFcoloraddbegin` … `\DIFcoloraddend`): its cells and rules are
    /// never cut apart by markup.
    fn alignment(&mut self, t: &Tok, side: Side) {
        let src = self.text(side);
        let text = &src[t.start..t.cend];
        if side == Side::Del && self.old_only(text) {
            self.comment_out(&src[t.start..t.end]);
            return;
        }
        let s = side_name(side);
        // (an alignment that is a paragraph's material begins one: the
        // colour then is in it; longtable is vertical material)
        let leave = !text.starts_with("\\begin{longtable");
        self.push(&format!(
            "{}\\DIFcolor{s}begin ",
            if leave { "\\leavevmode" } else { "" }
        ));
        self.push(&match side {
            Side::Add => text.to_owned(),
            Side::Del => self.unlabel(text),
        });
        self.unit_end(&format!("\\DIFcolor{s}end"), &src[t.cend..t.end]);
    }

    /// A picture or a verbatim environment added or deleted: whole, as it
    /// is. A verbatim one (its own lines) in a block's frame, as a float's
    /// (a long one, which a frame could not break across pages, coloured
    /// only); a picture in a frame of its own in the line.
    fn picture(&mut self, t: &Tok, side: Side) {
        let src = self.text(side);
        let text = &src[t.start..t.cend];
        if side == Side::Del && (!self.show_del || self.old_only(text)) {
            self.comment_out(&src[t.start..t.end]);
            return;
        }
        let s = side_name(side);
        let name = env_name(text).unwrap_or("");
        if PICTURES.contains(&name) {
            self.whole(t, side, true);
        } else if text.lines().count() > 40 {
            self.push(&format!("\\DIFcolor{s}begin "));
            self.push(text);
            self.unit_end(&format!("\\DIFcolor{s}end"), &src[t.cend..t.end]);
        } else {
            self.push(&format!("\\DIFblock{s}begin "));
            self.push(text);
            self.unit_end(&format!("\\DIFblock{s}end"), &src[t.cend..t.end]);
        }
    }

    /// A token shown whole in the line, as it is: framed (a picture, an
    /// image: `\DIFinlineaddbegin` … `\DIFinlineaddend`) or coloured
    /// (`\verb`, which no markup command can take as an argument).
    fn whole(&mut self, t: &Tok, side: Side, framed: bool) {
        let src = self.text(side);
        let text = &src[t.start..t.cend];
        let s = side_name(side);
        let kind = if framed || is_image(text) {
            "inline"
        } else {
            "color"
        };
        self.push(&format!("\\leavevmode\\DIF{kind}{s}begin "));
        match image_path(text) {
            // (a deleted image whose file the new version may not have:
            // its name shown where its file is gone)
            Some(path) if side == Side::Del => {
                self.push(&format!("\\DIFoldimage{{{path}}}{{{text}}}"));
            }
            _ => self.push(text),
        }
        self.unit_end(&format!("\\DIF{kind}{s}end"), &src[t.cend..t.end]);
    }

    /// A unit's closing command `cmd`, then the white space after the unit
    /// (`{}` ending the command where there is some: a space after a
    /// control word would be taken by it, not typeset).
    fn unit_end(&mut self, cmd: &str, ws: &str) {
        if ws.is_empty() {
            self.push(&format!("{cmd} "));
        } else {
            self.push(&format!("{cmd}{{}}"));
            self.push(ws);
        }
    }

    /// Two displays of an environment of rows (`align`, `gather`, …):
    /// their rows diffed, a row the same on both sides as it is, a deleted
    /// row coloured and marked `(deleted)` where its number would be
    /// (taking none: the new version's equations keep their numbers), an
    /// added row coloured, numbered as it is.
    fn display_pair(&mut self, o: &Tok, n: &Tok) {
        let (Kind::Display { body: ob, .. }, Kind::Display { env, body: nb, .. }) =
            (&o.kind, &n.kind)
        else {
            return;
        };
        if !self.show_del || (!self.amsmath && !env.starts_with("eqnarray")) {
            self.block(
                std::slice::from_ref(o),
                std::slice::from_ref(n),
                o.start,
                n.start,
                Ctx::default(),
            );
            return;
        }
        let start = self.out.len();
        let orows = math_rows(&self.old[ob.0..ob.1]);
        let nrows = math_rows(&self.new[nb.0..nb.1]);
        let key = |r: &str| {
            use std::hash::{DefaultHasher, Hasher};
            let mut h = DefaultHasher::new();
            crate::tok::norm_hash(r, &mut h);
            h.finish()
        };
        let ok: Vec<u64> = orows.iter().map(|r| key(r.0)).collect();
        let nk: Vec<u64> = nrows.iter().map(|r| key(r.0)).collect();
        let mut rows: Vec<(String, String)> = Vec::new();
        let (mut i, mut j) = (0, 0);
        for (pi, pj) in lcs(&ok, &nk)
            .into_iter()
            .chain(std::iter::once((orows.len(), nrows.len())))
        {
            for r in &orows[i..pi] {
                if !r.0.trim().is_empty() {
                    rows.push((self.del_row(r.0), String::new()));
                }
            }
            for r in &nrows[j..pj] {
                rows.push((
                    format!("\\DIFmathaddbegin {}\\DIFmathaddend ", r.0),
                    r.1.to_owned(),
                ));
            }
            if pi < orows.len() {
                rows.push((nrows[pj].0.to_owned(), nrows[pj].1.to_owned()));
            }
            i = pi + 1;
            j = pj + 1;
        }
        self.push(&self.new[n.start..nb.0]);
        let last = rows.len().saturating_sub(1);
        for (k, (row, end)) in rows.iter().enumerate() {
            self.push(row);
            if k < last {
                self.push(if end.is_empty() { "\\\\\n" } else { end });
            }
        }
        self.push(&self.new[nb.1..n.end]);
        let section = self.section.clone();
        self.changes.push(RawChange {
            old: (o.start, o.cend),
            new: (n.start, n.cend),
            out: (start, self.out.len()),
            section,
        });
    }

    /// A deleted row of a display: coloured, its labels, tags and number
    /// gone, marked `(deleted)` where its number would be (eqnarray's:
    /// unnumbered).
    fn del_row(&self, row: &str) -> String {
        let body = untag(&unlabel(row, |_| true));
        let mark = if self.amsmath {
            "\\tag*{\\DIFmathdeltag}"
        } else {
            "\\nonumber"
        };
        format!(
            "\\DIFmathdelbegin {} {mark}\\DIFmathdelend ",
            body.trim_end()
        )
    }

    /// Display math, whole: an added one as it is, its body coloured
    /// (`\DIFmathaddbegin` … `\DIFmathaddend`); a deleted one as it was,
    /// unnumbered (its environment's starred form: the new version's
    /// equations keep their numbers), its labels dropped, its body
    /// coloured (`\DIFmathdel…`). Math is never cut apart by markup.
    fn display(&mut self, t: &Tok, env: &str, arr: bool, body: (usize, usize), side: Side) {
        let src = self.text(side);
        let (open, inner, close) = (
            &src[t.start..body.0],
            &src[body.0..body.1],
            &src[body.1..t.cend],
        );
        let s = side_name(side);
        // (the colour's end before a last `\\`, which would make a row of
        // its own)
        let trimmed = inner.trim_end();
        let cut = trimmed.strip_suffix("\\\\").map_or(inner.len(), str::len);
        match side {
            Side::Add => {
                self.push(open);
                self.push(&format!("\\DIFmath{s}begin "));
                self.push(&inner[..cut]);
                self.push(&format!("\\DIFmath{s}end "));
                self.push(&inner[cut..]);
                self.push(&src[body.1..t.end]);
            }
            Side::Del if !self.show_del || self.old_only(&src[t.start..t.cend]) => {
                self.comment_out(&src[t.start..t.end]);
            }
            Side::Del => {
                let star = unnumbered(env, arr, self.amsmath);
                let swap = |part: &str| match &star {
                    Some(to) => part.replacen(&format!("{{{env}}}"), &format!("{{{to}}}"), 1),
                    None => part.to_owned(),
                };
                self.push(&swap(open));
                if ROW_ENVS.contains(&env) && self.amsmath && !env.starts_with("eqnarray") {
                    // (each row deleted, marked as one: `(deleted)` where
                    // its number would be)
                    let rows: Vec<String> = math_rows(inner)
                        .into_iter()
                        .filter(|r| !r.0.trim().is_empty())
                        .map(|r| self.del_row(r.0))
                        .collect();
                    self.push(&rows.join("\\\\\n"));
                } else {
                    self.push(&format!("\\DIFmath{s}begin "));
                    let tagged =
                        self.amsmath && !open.starts_with("$$") && ams_tagged(env, star.as_deref());
                    if tagged {
                        self.push(&untag(&unlabel(&inner[..cut], |_| true)));
                        self.push(&format!("{} ", untag(&unlabel(&inner[cut..], |_| true))));
                        self.push("\\tag*{\\DIFmathdeltag}");
                    } else {
                        self.push(&unlabel(&inner[..cut], |_| true));
                        self.push(&unlabel(&inner[cut..], |_| true));
                    }
                    self.push(&format!("\\DIFmath{s}end "));
                }
                self.push(&swap(close));
                self.push(&src[t.cend..t.end]);
            }
        }
    }

    /// Write the run in `buf` in its markup (white space alone as it is).
    fn flush(&mut self, buf: &mut String, side: Side, ctx: Ctx) {
        if buf.is_empty() {
            return;
        }
        let pending = std::mem::take(&mut self.pending);
        if buf.trim().is_empty() {
            self.push(buf);
            let at = self.out.len() - buf.len();
            self.adds
                .extend(pending.iter().map(|&(n, o)| (n, at + o, None)));
        } else {
            let cmd = match (side, ctx.fl) {
                (Side::Add, false) => "DIFadd",
                (Side::Add, true) => "DIFaddFL",
                (Side::Del, false) => "DIFdel",
                (Side::Del, true) => "DIFdelFL",
            };
            // (the white space after the run outside it: not struck out
            // or underlined, a space the text has; but after a control
            // word, which takes it)
            let mut k = buf.trim_end().len();
            let last = buf[..k].rfind('\\').map_or("", |b| &buf[b + 1..k]);
            if !last.is_empty() && last.bytes().all(|c| c.is_ascii_alphabetic()) {
                k = buf.len();
            }
            let s = format!("\\{cmd}{{{}}}{}", &buf[..k], &buf[k..]);
            self.push(&s);
            let at = self.out.len() - s.len();
            let open = cmd.len() + 2;
            self.adds.extend(pending.iter().map(|&(n, o)| {
                if o == 0 {
                    (n, at, None)
                } else if o >= k {
                    (n, at + open + o + 1, None)
                } else {
                    (n, at + open + o, Some(cmd))
                }
            }));
        }
        buf.clear();
    }

    /// Deleted text that cannot be shown, commented out (as latexdiff's
    /// `%DIFDELCMD <` lines).
    fn comment_out(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let body = text.trim_end();
        for line in body.split('\n') {
            self.push("%DIFDELCMD < ");
            self.push(line);
            self.push("\n");
        }
        self.push("%DIFDELCMD < %%%");
        self.nl = true;
    }

    /// Deleted text without the labels the new version defines too (it
    /// has them where they are now; one only the old version has is kept:
    /// a reference to what was deleted still finds it).
    fn unlabel(&self, text: &str) -> String {
        unlabel(text, |key| self.new.contains(&format!("\\label{{{key}}}")))
    }

    /// How many numbered captions the old version's floats of float `t`'s
    /// kind had before it: its number, less one, in the old version.
    fn old_number(&self, t: &Tok) -> usize {
        let src = &self.old[..t.start];
        let name = env_name(&self.old[t.start..t.cend]).unwrap_or("");
        let base = name.trim_end_matches('*');
        let mut n = 0;
        let mut at = 0;
        while let Some(k) = src[at..].find("\\begin{") {
            let b = at + k;
            at = b + 7;
            let line = src[..b].rfind('\n').map_or(0, |l| l + 1);
            if src[line..b].contains('%') {
                continue;
            }
            let Some(this) = env_name(&src[b..]) else {
                continue;
            };
            if this.trim_end_matches('*') != base {
                continue;
            }
            let end = src[b..]
                .find(&format!("\\end{{{this}}}"))
                .map_or(src.len(), |e| b + e);
            n += captions(&src[b..end]);
            at = end.max(at);
        }
        n
    }

    /// Whether `text` is a use of one of the document's own macros with
    /// arguments (what it typesets is shown when deleted, coloured, as
    /// `\verb` is).
    fn user_macro(&self, text: &str) -> bool {
        // (one that takes an argument: one without may open what another
        // closes, `\beq` an equation)
        let (name, _) = cs_at(text, 0);
        text.starts_with('\\')
            && self.sigs.user(name)
            && self.sigs.cmd(name).is_some_and(|s| s.spec.contains('m'))
    }

    /// Whether references `o` and `n` (of one command) name a label the old
    /// version had and the new one does not, and one the new version has
    /// and the old one did not: the label renamed.
    fn renamed(&self, o: &Tok, n: &Tok) -> bool {
        let key = |t: &Tok, src: &'a str| {
            let s = &src[t.start..t.cend];
            let a = s.rfind('{')?;
            s[a + 1..].strip_suffix('}').map(str::trim)
        };
        let (Some(a), Some(b)) = (key(o, self.old), key(n, self.new)) else {
            return false;
        };
        let has = |src: &str, k: &str| src.contains(&format!("\\label{{{k}}}"));
        has(self.old, a) && !has(self.new, a) && has(self.new, b) && !has(self.old, b)
    }

    /// Whether deleted text uses a command only the old preamble defines.
    fn old_only(&self, text: &str) -> bool {
        let b = text.as_bytes();
        let mut i = 0;
        while let Some(k) = text[i..].find('\\') {
            let (name, e) = cs_at(text, i + k);
            if self.sigs.old_only(name) {
                return true;
            }
            i = e.max(i + k + 1);
            if i >= b.len() {
                break;
            }
        }
        false
    }
}

/// The paragraphs of a token sequence: runs up to and including a blank
/// line.
pub fn chunks(toks: &[Tok]) -> Vec<std::ops::Range<usize>> {
    let mut out = Vec::new();
    let mut from = 0;
    for (i, t) in toks.iter().enumerate() {
        if t.kind == Kind::Par {
            out.push(from..i + 1);
            from = i + 1;
        }
    }
    if from < toks.len() {
        out.push(from..toks.len());
    }
    out
}

pub fn chunk_key(toks: &[Tok]) -> u64 {
    use std::hash::{DefaultHasher, Hash, Hasher};
    let mut h = DefaultHasher::new();
    for t in toks {
        t.key.hash(&mut h);
    }
    h.finish()
}

/// Drop common runs shorter than [`MIN_BLOCK`] words between two changes
/// that are much longer than they are: they become part of the change (a
/// rewrite is shown as one, not word by word).
fn merge_short(pairs: &[(usize, usize)], old: &[Tok], new: &[Tok]) -> Vec<(usize, usize)> {
    let mut out: Vec<(usize, usize)> = Vec::with_capacity(pairs.len());
    let mut k = 0;
    while k < pairs.len() {
        // (a run of consecutive pairs)
        let mut e = k + 1;
        while e < pairs.len()
            && pairs[e].0 == pairs[e - 1].0 + 1
            && pairs[e].1 == pairs[e - 1].1 + 1
        {
            e += 1;
        }
        let run = &pairs[k..e];
        let (fi, fj) = run[0];
        let (li, lj) = run[run.len() - 1];
        // (the change before: since the last run kept, merged ones in it)
        let (pi, pj) = out.last().map_or((0, 0), |&(i, j)| (i + 1, j + 1));
        let before = (fi - pi) + (fj - pj);
        let (ni, nj) = pairs.get(e).copied().unwrap_or((old.len(), new.len()));
        let after = (ni - li - 1) + (nj - lj - 1);
        let words = run
            .iter()
            .all(|&(i, _)| matches!(old[i].kind, Kind::Word | Kind::Ws));
        if !(words
            && run.len() < MIN_BLOCK
            && before > 0
            && after > 0
            && before + after > 4 * run.len())
        {
            out.extend_from_slice(run);
        }
        k = e;
    }
    out
}

fn line_end(b: &[u8], i: usize) -> usize {
    b[i..]
        .iter()
        .position(|&c| c == b'\n')
        .map_or(b.len(), |k| i + k + 1)
}

/// Past the `}` closing the `{` at `i` (`%` comments and `\{` skipped).
fn brace_end(b: &[u8], i: usize) -> usize {
    let mut depth = 0usize;
    let mut k = i;
    while k < b.len() {
        match b[k] {
            b'\\' => k += 1,
            b'%' => {
                k = line_end(b, k);
                continue;
            }
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return k + 1;
                }
            }
            _ => {}
        }
        k += 1;
    }
    b.len()
}

/// The control sequence at `i` (a backslash): its name and where it ends.
fn cs_at(s: &str, i: usize) -> (&str, usize) {
    // (not a backslash: no control sequence, the character passed)
    if !s[i..].starts_with('\\') {
        return ("", i + s[i..].chars().next().map_or(1, char::len_utf8));
    }
    let b = s.as_bytes();
    let mut e = i + 1;
    if e < b.len() && b[e].is_ascii_alphabetic() {
        while e < b.len() && b[e].is_ascii_alphabetic() {
            e += 1;
        }
    } else if e < b.len() {
        e += s[e..].chars().next().map_or(1, char::len_utf8);
    }
    (&s[(i + 1).min(e)..e], e)
}

/// The label of an `\item[…]` (its range in `text`), if it is plain text.
fn item_label(text: &str) -> Option<(usize, usize)> {
    let rest = text.strip_prefix("\\item")?;
    let open = 5 + rest.find('[')?;
    if !text[5..open].trim().is_empty() {
        return None;
    }
    let close = open + text[open..].find(']')?;
    let label = &text[open + 1..close];
    (!label.contains(['\\', '{', '}', '%', '&', '$'])).then_some((open + 1, close))
}

/// Whether `text` is an `\item` (with its label).
fn is_item(text: &str) -> bool {
    text.strip_prefix("\\item")
        .or_else(|| text.strip_prefix("\\bibitem"))
        .is_some_and(|r| !r.starts_with(|c: char| c.is_ascii_alphabetic()))
}

/// `add` or `del`, as the markup's command names have them.
fn side_name(side: Side) -> &'static str {
    match side {
        Side::Add => "add",
        Side::Del => "del",
    }
}

/// The environments shown in a frame in the line, not as a block.
const PICTURES: &[&str] = &[
    "tikzpicture",
    "picture",
    "pgfpicture",
    "tikzcd",
    "DIFnomarkup",
];

/// The layout environments a deletion keeps (their text struck out in
/// them): they number nothing.
const LAYOUT: &[&str] = &[
    "center",
    "flushleft",
    "flushright",
    "quote",
    "quotation",
    "verse",
    "minipage",
    "multicols",
    "small",
    "footnotesize",
    "scriptsize",
];

/// The name of the environment `text` begins (`\begin{name}`).
fn env_name(text: &str) -> Option<&str> {
    let rest = text.strip_prefix("\\begin")?.trim_start();
    let rest = rest.strip_prefix('{')?;
    Some(rest[..rest.find('}')?].trim())
}

/// Whether `text` is an image (`\includegraphics…`), shown framed.
fn is_image(text: &str) -> bool {
    let (name, _) = cs_at(text, 0);
    text.starts_with('\\') && matches!(name, "includegraphics" | "includesvg")
}

/// Whether a deleted unsafe token is shown whole (an image, `\verb`)
/// rather than commented out.
fn shown_whole(text: &str) -> bool {
    let (name, _) = cs_at(text, 0);
    text.starts_with('\\')
        && (is_image(text) || matches!(name, "verb" | "lstinline" | "url" | "path"))
}

/// `text` without the `\label{…}` commands `drop` takes.
fn unlabel(text: &str, drop: impl Fn(&str) -> bool) -> String {
    let b = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while let Some(k) = text[i..].find("\\label") {
        let at = i + k;
        let mut e = at + 6;
        if b.get(e).is_some_and(u8::is_ascii_alphabetic) {
            out.push_str(&text[i..e]);
            i = e;
            continue;
        }
        while b.get(e).is_some_and(|c| *c == b' ') {
            e += 1;
        }
        if b.get(e) == Some(&b'{') {
            let end = brace_end(b, e);
            if drop(text[e + 1..end.saturating_sub(1)].trim()) {
                out.push_str(&text[i..at]);
            } else {
                out.push_str(&text[i..end]);
            }
            i = end;
        } else {
            out.push_str(&text[i..at + 6]);
            i = at + 6;
        }
    }
    out.push_str(&text[i..]);
    out
}

/// How many numbered `\caption`s `text` has.
fn captions(text: &str) -> usize {
    let b = text.as_bytes();
    text.match_indices("\\caption")
        .filter(|(k, _)| {
            let next = b.get(k + 8).copied().unwrap_or(b' ');
            !next.is_ascii_alphabetic() && next != b'*' && next != b'@'
        })
        .count()
}

/// The unnumbered form of display environment `env` (none: it is one
/// already, or has none: as it is).
fn unnumbered(env: &str, arr: bool, amsmath: bool) -> Option<String> {
    if env.is_empty() || env.ends_with('*') || env == "displaymath" {
        return None;
    }
    let ams = [
        "equation", "align", "alignat", "gather", "multline", "flalign", "xalignat",
    ];
    if amsmath && ams.contains(&env) {
        return Some(format!("{env}*"));
    }
    match env {
        "equation" if !arr => Some("displaymath".into()),
        "eqnarray" => Some("eqnarray*".into()),
        _ => None,
    }
}

/// The commands whose last argument is a list of keys (citations,
/// cleveref's references): diffed by key.
const KEYED: &[&str] = &[
    "cite",
    "citep",
    "citet",
    "citealp",
    "citealt",
    "citeauthor",
    "citeyear",
    "citeyearpar",
    "Cite",
    "parencite",
    "Parencite",
    "textcite",
    "Textcite",
    "autocite",
    "Autocite",
    "footcite",
    "cref",
    "Cref",
    "labelcref",
];

/// A keyed command's parts (`text` its whole text): where its key group
/// begins (its command and options before), and its keys' range.
fn key_list(text: &str) -> Option<(usize, usize, usize)> {
    let b = text.as_bytes();
    if b.first() != Some(&b'\\') {
        return None;
    }
    let (name, mut e) = cs_at(text, 0);
    if !KEYED.contains(&name) {
        return None;
    }
    loop {
        while b.get(e).is_some_and(u8::is_ascii_whitespace) {
            e += 1;
        }
        match b.get(e)? {
            b'*' => e += 1,
            b'[' => {
                let mut depth = 0usize;
                let mut k = e;
                loop {
                    match b.get(k)? {
                        b'{' => depth += 1,
                        b'}' => depth = depth.saturating_sub(1),
                        b']' if depth == 0 => break,
                        _ => {}
                    }
                    k += 1;
                }
                e = k + 1;
            }
            b'{' => break,
            _ => return None,
        }
    }
    let end = brace_end(b, e);
    (end == text.trim_end().len() && end > e + 1).then_some((e, e + 1, end - 1))
}

/// The keys of a key list, trimmed, the empty ones dropped.
fn keys(list: &str) -> Vec<&str> {
    list.split(',')
        .map(str::trim)
        .filter(|k| !k.is_empty())
        .collect()
}

/// A keyed token's head (its command and options, white space
/// normalized), hashed: what two citations must share for their keys to
/// be diffed.
fn cite_head(t: &Tok, text: &str) -> Option<u64> {
    use std::hash::{DefaultHasher, Hasher};
    if t.kind != Kind::Mbox {
        return None;
    }
    let body = &text[t.start..t.cend];
    let (h, _, _) = key_list(body)?;
    let mut hs = DefaultHasher::new();
    "cite".hash(&mut hs);
    crate::tok::norm_hash(&body[..h], &mut hs);
    Some(hs.finish())
}

/// Keys `ks` cited by `head`, a few at a time, each citation in an
/// `\mbox` (the underlining's unit) and a space between them: the markup
/// breaks between them as text does, never one long box.
fn cite_chunks(head: &str, ks: &[&str]) -> String {
    ks.chunks(3)
        .map(|c| format!("\\mbox{{{head}{{{}}}}}", c.join(",")))
        .collect::<Vec<_>>()
        .join(" ")
}

/// What two nodes must share to be paired: their prefix, and a float's
/// label (a float is paired with the one of its label, not the next one
/// of its kind).
fn pair_key(t: &Tok, text: &str) -> u64 {
    use std::hash::{DefaultHasher, Hasher};
    if !is_float(t) {
        return t.prefix;
    }
    let label = t
        .children
        .iter()
        .map(|c| &text[c.start..c.cend])
        .find(|c| c.starts_with("\\label"));
    let mut h = DefaultHasher::new();
    t.prefix.hash(&mut h);
    label.hash(&mut h);
    h.finish()
}

/// The keys of `t`'s leaves, at any depth, in order.
fn leaves(t: &Tok, out: &mut Vec<u64>) {
    for c in &t.children {
        if c.is_node() {
            leaves(c, out);
        } else {
            out.push(c.key);
        }
    }
}

fn is_float(t: &Tok) -> bool {
    matches!(
        t.kind,
        Kind::Env {
            kind: EnvKind::Float,
            ..
        }
    )
}

/// Whether two nodes paired are alike enough to be diffed inside: a float
/// sharing a third of its tokens with the other, a row its first cell or
/// two thirds of its tokens (else one is deleted and the other added,
/// each whole: a row of another thing is not this one's cells changed).
fn alike(o: &Tok, n: &Tok) -> bool {
    if is_float(o) {
        // (by their words, at any depth: a float is mostly one node, its
        // tabular or its algorithm)
        let mut ol = Vec::new();
        let mut nl = Vec::new();
        leaves(o, &mut ol);
        leaves(n, &mut nl);
        return lcs(&ol, &nl).len() * 3 >= ol.len().max(nl.len());
    }
    if o.kind == Kind::Row {
        // (its cells' keys; the first cell's words, its name: what a
        // citation or a command in it is aside)
        let cells = |t: &Tok| {
            let mut cells: Vec<Vec<u64>> = vec![Vec::new()];
            for c in &t.children {
                match c.kind {
                    Kind::Amp => cells.push(Vec::new()),
                    Kind::Rule | Kind::Comment | Kind::Ws | Kind::RowEnd => {}
                    _ => cells.last_mut().unwrap_or(&mut Vec::new()).push(c.key),
                }
            }
            cells
        };
        let name = |t: &Tok| {
            t.children
                .iter()
                .take_while(|c| c.kind != Kind::Amp)
                .filter(|c| c.kind == Kind::Word)
                .map(|c| c.key)
                .collect::<Vec<u64>>()
        };
        let (oc, nc) = (cells(o), cells(n));
        let same = oc.iter().zip(&nc).filter(|(a, b)| a == b).count();
        return name(o) == name(n) || same * 3 >= oc.len().max(nc.len()) * 2;
    }
    true
}

/// A deleted float's opening, placed as near as it may be to where it
/// was: here if it fits, else the top of a page (`[!htbp]`; a float across
/// both columns `[!tp]`), whatever its own placement asked.
fn near_placement(open: &str) -> String {
    let Some(name) = env_name(open) else {
        return open.to_owned();
    };
    let head = open.find('}').map_or(open.len(), |k| k + 1);
    let place = if name.ends_with('*') {
        "[!tp]"
    } else {
        "[!htbp]"
    };
    let rest = &open[head..];
    let rest = match rest.strip_prefix('[') {
        Some(r) if r.contains(']') => &r[r.find(']').map_or(0, |k| k + 1)..],
        _ => rest,
    };
    format!("{}{place}{rest}", &open[..head])
}

/// The environments of rows, numbered one by one: diffed by rows.
const ROW_ENVS: &[&str] = &[
    "align",
    "align*",
    "gather",
    "gather*",
    "flalign",
    "flalign*",
    "alignat",
    "alignat*",
    "eqnarray",
    "eqnarray*",
];

/// Whether a deleted display of `env` (unnumbered as `star`) takes
/// amsmath's `\tag*`: one equation.
fn ams_tagged(env: &str, star: Option<&str>) -> bool {
    let e = star.unwrap_or(env);
    e.is_empty() && env.is_empty() || matches!(e, "equation*" | "equation")
}

/// `text` without `\tag{…}`, `\tag*{…}`, `\notag` and `\nonumber`.
fn untag(text: &str) -> String {
    let b = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' {
            let (name, mut e) = cs_at(text, i);
            match name {
                "notag" | "nonumber" => {
                    i = e;
                    continue;
                }
                "tag" => {
                    if b.get(e) == Some(&b'*') {
                        e += 1;
                    }
                    while b.get(e) == Some(&b' ') {
                        e += 1;
                    }
                    if b.get(e) == Some(&b'{') {
                        e = brace_end(b, e);
                    }
                    i = e;
                    continue;
                }
                _ => {
                    out.push_str(&text[i..e]);
                    i = e;
                    continue;
                }
            }
        }
        let ch = text[i..].chars().next().map_or(1, char::len_utf8);
        out.push_str(&text[i..i + ch]);
        i += ch;
    }
    out
}

/// The rows of a display's body: each its text and the `\\` that ends it
/// (with its star and its `[…]`; none for the last), split at the body's
/// own level (not in a group or a nested environment; comments skipped).
fn math_rows(body: &str) -> Vec<(&str, &str)> {
    let b = body.as_bytes();
    let (mut rows, mut from, mut depth, mut envs, mut i) = (Vec::new(), 0, 0i32, 0i32, 0);
    while i < b.len() {
        match b[i] {
            b'%' => {
                i = line_end(b, i);
                continue;
            }
            b'{' => depth += 1,
            b'}' => depth -= 1,
            b'\\' => {
                let (name, mut e) = cs_at(body, i);
                match name {
                    "begin" => envs += 1,
                    "end" => envs -= 1,
                    "\\" if depth == 0 && envs == 0 => {
                        if b.get(e) == Some(&b'*') {
                            e += 1;
                        }
                        let mut k = e;
                        while b.get(k).is_some_and(|c| *c == b' ') {
                            k += 1;
                        }
                        if b.get(k) == Some(&b'[')
                            && let Some(c) = body[k..].find(']')
                        {
                            e = k + c + 1;
                        }
                        rows.push((&body[from..i], &body[i..e]));
                        from = e;
                    }
                    _ => {}
                }
                i = e;
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    rows.push((&body[from..], ""));
    rows
}

/// The file an `\includegraphics` names (its last group), when it is a
/// plain name.
fn image_path(text: &str) -> Option<&str> {
    if !is_image(text) {
        return None;
    }
    let t = text.trim_end();
    let open = t.rfind('{')?;
    let p = t[open + 1..].strip_suffix('}')?.trim();
    (!p.is_empty() && !p.contains(['\\', '{', '}', '#', '%'])).then_some(p)
}

/// A reference's command (`\ref`, `\eqref`, `\autoref`, `\pageref`,
/// `\nameref`), hashed: references of one command pair, so that one whose
/// label was renamed is not marked.
fn ref_head(t: &Tok, text: &str) -> Option<u64> {
    use std::hash::{DefaultHasher, Hasher};
    if t.kind != Kind::Safe {
        return None;
    }
    let s = &text[t.start..t.cend];
    let (name, e) = cs_at(s, 0);
    if !s.starts_with('\\') || !matches!(name, "ref" | "eqref" | "autoref" | "pageref" | "nameref")
    {
        return None;
    }
    let mut h = DefaultHasher::new();
    "ref".hash(&mut h);
    s[..e].hash(&mut h);
    Some(h.finish())
}
