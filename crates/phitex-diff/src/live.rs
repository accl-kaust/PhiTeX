//! The live diff (phase 2): a baseline kept, and each new version of the
//! document diffed against it again only where it changed since the last.
//!
//! - *The new body* is kept as a CST ([`Tree`]). An edit (where the new
//!   flattened text differs from the last one) is applied to it, and the
//!   splice it answers says which paragraphs changed; paragraphs it read
//!   again whose text is the same are not changes.
//! - *Units.* Only the changed paragraphs are read into tokens again, with
//!   the paragraphs they read together: a unit is a run of paragraphs no
//!   token crosses (an environment, `\[…\]`, is one token, blank lines and
//!   all). A `\begin` with no `\end` after it reads to the end of the body
//!   (latexdiff's reader does), so a unit with such an opener is read again
//!   with the text after it when a closer appears there, and the text
//!   after an edit is read with it while the edit leaves one open.
//! - *Segments.* The new chunks (paragraphs of tokens, as [`emit`] aligns
//!   them) are aligned with the old ones once, and the alignment is kept:
//!   chunks matched, and runs that differ, each with its markup and changes.
//!   An edit aligns again only the segments it touched and the runs next
//!   to them, between the matched chunks on either side, and diffs only
//!   the runs there by tokens.
//! - *The output* (the marked-up text, the change list) is put together
//!   from the segments; a change's place in the new files is found from
//!   the flattened text then, so an edit before it moves it at no cost.
//!
//! [`Stats`] counts what an update read and diffed again: a one-word edit
//! reads one paragraph and diffs one chunk.

use crate::emit::{self, Em};
use crate::myers::patience as lcs;
use crate::tok::{self, Kind, SECTIONS, Tok};
use crate::{Baseline, Body, Change, ChangeKind, DiffOut, Flat, Loc, Signatures, find_body, loads};
use phitex_syntax::{SyntaxKind, Tree};
use std::fmt::Write as _;
use std::hash::{DefaultHasher, Hash, Hasher};

/// What an update did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// The new version was diffed whole: the first one, or an edit outside
    /// the body (the preamble, `\end{document}`, a group left open at the
    /// body's end).
    pub whole: bool,
    /// The new body's paragraphs read into tokens.
    pub paras_read: usize,
    /// The new chunks (paragraphs of tokens) diffed by tokens.
    pub chunks_diffed: usize,
    /// The segments aligned again.
    pub segments: usize,
}

/// A live diff: a baseline, and the last new version diffed against it.
pub struct Live {
    base: Baseline,
    side: Option<Side>,
    out: DiffOut,
    stats: Stats,
}

/// The new side, as the last update left it.
struct Side {
    flat: Flat,
    body: Body,
    sigs: Signatures,
    amsmath: bool,
    /// The output before the body.
    head: String,
    old: Old,
    /// The new body's CST, and each paragraph's length and hash.
    tree: Tree,
    phash: Vec<(usize, u64)>,
    units: Vec<Unit>,
    chunks: Vec<Chunk>,
    segs: Vec<Seg>,
}

/// The old side's tokens, read by the new version's signatures, and its
/// chunks.
struct Old {
    toks: Vec<Tok>,
    chunks: Vec<std::ops::Range<usize>>,
    keys: Vec<u64>,
}

/// A run of the new body's paragraphs no token crosses: how many
/// paragraphs and chunks, and the openers in it that found no closer.
struct Unit {
    paras: usize,
    chunks: usize,
    opens: Vec<Opener>,
}

/// An opener that reads on to its closer, wherever it is.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Opener {
    Env(String),
    Display,
    Inline,
}

/// A chunk of the new body: its text, its tokens (from its start), its
/// key, and the last sectioning command in it.
#[derive(Clone)]
struct Chunk {
    text: String,
    toks: Vec<Tok>,
    key: u64,
    section: Option<String>,
}

/// A segment of the alignment: `old` old chunks and `new` new ones,
/// matched (`run` none: the new ones as they are) or diffed.
struct Seg {
    old: usize,
    new: usize,
    run: Option<Run>,
}

/// A run's markup: its text, whether it owes a line end after it, the
/// last section written in it, and its changes (new ranges from the run's
/// start, `out` from its text's).
struct Run {
    out: String,
    nl: bool,
    section: Option<String>,
    changes: Vec<RunChange>,
}

struct RunChange {
    raw: emit::RawChange,
    new_text: String,
}

/// Where a change is shown: its page (from 0) and how far down it, in
/// PDF points from the page's top.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Place {
    pub page: usize,
    pub y: f32,
}

impl Live {
    /// A live diff against `base`; nothing diffed yet.
    #[must_use]
    pub fn new(base: Baseline) -> Live {
        Live {
            base,
            side: None,
            out: DiffOut {
                tex: String::new(),
                changes: Vec::new(),
            },
            stats: Stats::default(),
        }
    }

    #[must_use]
    pub fn baseline(&self) -> &Baseline {
        &self.base
    }

    /// The last diff.
    #[must_use]
    pub fn out(&self) -> &DiffOut {
        &self.out
    }

    /// What the last update did.
    #[must_use]
    pub fn stats(&self) -> Stats {
        self.stats
    }

    /// Diff `new`, the flattened new version, again: only where it differs
    /// from the last one.
    pub fn update(&mut self, new: &Flat) -> &DiffOut {
        self.stats = Stats::default();
        let patched = match self.side.as_mut() {
            Some(side) => side.patch(&self.base, new, &mut self.stats),
            None => false,
        };
        if !patched {
            self.side = Some(Side::whole(&self.base, new, &mut self.stats));
        }
        if let Some(side) = &self.side {
            self.out = side.assemble(&self.base);
        }
        &self.out
    }
}

/// The (length, hash) of a paragraph's text.
fn para_hash(s: &str) -> (usize, u64) {
    let mut h = DefaultHasher::new();
    s.hash(&mut h);
    (s.len(), h.finish())
}

impl Side {
    /// Diff `new` whole.
    fn whole(base: &Baseline, new: &Flat, stats: &mut Stats) -> Side {
        stats.whole = true;
        let body = find_body(&new.text);
        let preamble = &new.text[..body.begin];
        let sigs = base.signatures(preamble);
        let amsmath = loads(preamble, &["amsmath", "mathtools"]);
        let head = base.head(new, body);
        let toks = base.old_tokens(&sigs);
        let chunks = emit::chunks(&toks);
        let keys = chunks
            .iter()
            .map(|r| emit::chunk_key(&toks[r.clone()]))
            .collect();
        let tree = Tree::parse(&new.text[body.start..body.end]);
        let phash = tree.paras().iter().map(|p| para_hash(p.text())).collect();
        let mut side = Side {
            flat: new.clone(),
            body,
            sigs,
            amsmath,
            head,
            old: Old { toks, chunks, keys },
            tree,
            phash,
            units: Vec::new(),
            chunks: Vec::new(),
            segs: Vec::new(),
        };
        let n = side.tree.paras().len();
        let (units, chunks) = side.read(0, n, stats);
        side.units = units;
        side.chunks = chunks;
        let (oc, nc) = (side.old.chunks.len(), side.chunks.len());
        side.segs = side.align(base, (0, oc), (0, nc), stats);
        side
    }

    /// Apply the edit that makes the last version `new` and diff again
    /// where it touched. False if it must be diffed whole.
    #[allow(clippy::too_many_lines, clippy::many_single_char_names)]
    fn patch(&mut self, base: &Baseline, new: &Flat, stats: &mut Stats) -> bool {
        let (a, b) = (self.flat.text.as_bytes(), new.text.as_bytes());
        if a == b {
            self.flat = new.clone();
            return true;
        }
        // (the edit: the bytes between the common prefix and suffix, on
        // character boundaries)
        let mut p = a.iter().zip(b).take_while(|(x, y)| x == y).count();
        while !new.text.is_char_boundary(p) || !self.flat.text.is_char_boundary(p) {
            p -= 1;
        }
        let max = a.len().min(b.len()) - p;
        let mut s = a
            .iter()
            .rev()
            .zip(b.iter().rev())
            .take(max)
            .take_while(|(x, y)| x == y)
            .count();
        while !new.text.is_char_boundary(b.len() - s)
            || !self.flat.text.is_char_boundary(a.len() - s)
        {
            s -= 1;
        }
        let (old_end, new_end) = (a.len() - s, b.len() - s);
        // (an edit outside the body; or a body that ran to the text's end,
        // with no `\end{document}` after it: diffed whole, the body found
        // again)
        if p < self.body.start || old_end > self.body.end || self.body.end == a.len() {
            return false;
        }
        let start = p - self.body.start;
        let old_len = old_end - p;
        let text = &new.text[p..new_end];
        let splice = self.tree.edit(start, old_len, text);
        self.body.end = self.body.end - old_len + text.len();
        self.flat = new.clone();
        let body = &self.flat.text[self.body.start..self.body.end];
        // (`\end{document}` read otherwise: after a group or a verbatim
        // run left open at the body's end, which the CST reads on into, or
        // on a line the edit touched, after a `%` or a backslash)
        if let Some(last) = self.tree.paras().last()
            && open_at_end(&last.green)
        {
            return false;
        }
        if !body.ends_with('\n') && start + text.len() >= body.rfind('\n').map_or(0, |k| k + 1) {
            return false;
        }
        // (the paragraphs read again whose text is the same are kept)
        let fresh: Vec<(usize, u64)> = self.tree.paras()[splice.at..splice.at + splice.inserted]
            .iter()
            .map(|p| para_hash(p.text()))
            .collect();
        let old_hashes = &self.phash[splice.at..splice.at + splice.removed];
        let head = old_hashes
            .iter()
            .zip(&fresh)
            .take_while(|(x, y)| x == y)
            .count();
        let tail = old_hashes[head..]
            .iter()
            .rev()
            .zip(fresh[head..].iter().rev())
            .take_while(|(x, y)| x == y)
            .count();
        let at = splice.at + head;
        let removed = splice.removed - head - tail;
        let inserted = splice.inserted - head - tail;
        self.phash
            .splice(splice.at..splice.at + splice.removed, fresh.iter().copied());
        if removed == 0 && inserted == 0 {
            return true;
        }
        // (the units the changed paragraphs are in: `ua..ub`, from
        // paragraph `pa` and chunk `ca`; an insertion between two units
        // touches none)
        let mut ua = 0;
        let mut pa = 0;
        let mut ca = 0;
        while ua < self.units.len() && pa + self.units[ua].paras <= at {
            pa += self.units[ua].paras;
            ca += self.units[ua].chunks;
            ua += 1;
        }
        let mut ub = ua;
        let mut pb = pa;
        let mut cb = ca;
        let last = at + removed;
        while ub < self.units.len() && (pb < last || (removed == 0 && pb == pa && at > pa)) {
            pb += self.units[ub].paras;
            cb += self.units[ub].chunks;
            ub += 1;
        }
        // (`pb` in the new numbering)
        let delta = inserted.cast_signed() - removed.cast_signed();
        let mut pb_new = pb.checked_add_signed(delta).unwrap_or(pb);
        loop {
            // (an opener before them that a closer they now hold ends)
            let text = self.paras_text(pa, pb_new);
            if let Some(u) = (0..ua)
                .rev()
                .find(|&u| self.units[u].opens.iter().any(|o| has_closer(&text, o)))
            {
                while ua > u {
                    ua -= 1;
                    pa -= self.units[ua].paras;
                    ca -= self.units[ua].chunks;
                }
                continue;
            }
            break;
        }
        let (units, chunks) = loop {
            let (units, chunks) = self.read(pa, pb_new, stats);
            // (an opener left open that a closer after them may end: read
            // on to the unit it is in)
            let opens: Vec<Opener> = units.iter().flat_map(|u| u.opens.clone()).collect();
            if opens.is_empty() || ub == self.units.len() {
                break (units, chunks);
            }
            let mut q = pb_new;
            let mut found = None;
            for (k, par) in self.tree.paras()[pb_new..].iter().enumerate() {
                if opens.iter().any(|o| has_closer(par.text(), o)) {
                    found = Some(pb_new + k);
                    break;
                }
            }
            let Some(f) = found else {
                break (units, chunks);
            };
            while q <= f && ub < self.units.len() {
                q += self.units[ub].paras;
                cb += self.units[ub].chunks;
                ub += 1;
            }
            pb_new = q;
        };
        let nchunks = chunks.len();
        self.units.splice(ua..ub, units);
        self.chunks.splice(ca..cb, chunks);
        // (the segments these chunks were in, and the runs beside them,
        // aligned again between the matched chunks around them; a match
        // cut where the edit begins and ends)
        let removed_chunks = cb - ca;
        self.split(ca);
        self.split(cb);
        let (mut sa, mut na, mut oa) = (0, 0, 0);
        while sa < self.segs.len() {
            let s = &self.segs[sa];
            let end = na + s.new;
            if end > ca || (end == ca && s.run.is_some()) {
                break;
            }
            na = end;
            oa += s.old;
            sa += 1;
        }
        let (mut sb, mut nb, mut ob) = (sa, na, oa);
        while sb < self.segs.len() && (nb < cb || (nb == cb && self.segs[sb].run.is_some())) {
            nb += self.segs[sb].new;
            ob += self.segs[sb].old;
            sb += 1;
        }
        let nb_new = nb - removed_chunks + nchunks;
        let segs = self.align(base, (oa, ob), (na, nb_new), stats);
        self.segs.splice(sa..sb, segs);
        true
    }

    /// Cut the matched segment new chunk `x` is inside (not at its start)
    /// in two there.
    fn split(&mut self, x: usize) {
        let mut n = 0;
        for k in 0..self.segs.len() {
            let s = &self.segs[k];
            if s.run.is_none() && n < x && x < n + s.new {
                let rest = n + s.new - x;
                self.segs[k].old = x - n;
                self.segs[k].new = x - n;
                self.segs.insert(
                    k + 1,
                    Seg {
                        old: rest,
                        new: rest,
                        run: None,
                    },
                );
                return;
            }
            n += s.new;
            if n > x {
                return;
            }
        }
    }

    /// The text of the new body's paragraphs `a..b`.
    fn paras_text(&self, a: usize, b: usize) -> String {
        self.tree.paras()[a..b]
            .iter()
            .map(phitex_syntax::Para::text)
            .collect()
    }

    /// Read the new body's paragraphs `a..b` into tokens: their units and
    /// chunks.
    #[allow(clippy::many_single_char_names)]
    fn read(&self, a: usize, b: usize, stats: &mut Stats) -> (Vec<Unit>, Vec<Chunk>) {
        stats.paras_read += b - a;
        let paras = &self.tree.paras()[a..b];
        let text = self.paras_text(a, b);
        let lx = tok::lexemes_of(paras.iter().map(|p| &*p.green), &text);
        let r = tok::Reader {
            text: &text,
            sigs: &self.sigs,
        };
        let toks = r.seq(&lx);
        // (where each paragraph ends)
        let mut ends = Vec::with_capacity(paras.len());
        let mut e = 0;
        for p in paras {
            e += p.text().len();
            ends.push(e);
        }
        let mut units = Vec::new();
        let mut chunks = Vec::new();
        let mut unit = Unit {
            paras: 0,
            chunks: 0,
            opens: Vec::new(),
        };
        let mut pi = 0;
        let mut from = 0;
        // (a unit's tokens, cut into chunks at its end)
        let mut cur: Vec<Tok> = Vec::new();
        let mut unit_start = 0;
        let close = |cur: &mut Vec<Tok>,
                     unit: &mut Unit,
                     start: usize,
                     end: usize,
                     chunks: &mut Vec<Chunk>| {
            let toks = std::mem::take(cur);
            let ranges = emit::chunks(&toks);
            let n = ranges.len().max(1);
            for (k, r) in ranges.iter().enumerate() {
                let cs = if k == 0 { start } else { toks[r.start].start };
                let ce = ranges.get(k + 1).map_or(end, |n| toks[n.start].start);
                chunks.push(chunk(&text[cs..ce], &toks[r.clone()], cs));
            }
            if ranges.is_empty() {
                chunks.push(chunk(&text[start..end], &[], start));
            }
            unit.chunks = n;
        };
        for t in toks {
            if t.kind == Kind::Unsafe {
                let s = &text[t.start..t.cend];
                if let Some(o) = opener(s) {
                    unit.opens.push(o);
                }
            }
            let (end, par) = (t.end, t.kind == Kind::Par);
            cur.push(t);
            while pi < ends.len() && ends[pi] < end {
                pi += 1;
            }
            // (a unit ends where a paragraph does with a blank line, a
            // chunk's end: not where a line end that is no blank line, as
            // after `\` at a line's end, runs into the next)
            if pi < ends.len() && ends[pi] == end && (par || pi + 1 == ends.len()) {
                pi += 1;
                unit.paras = pi - from;
                close(&mut cur, &mut unit, unit_start, end, &mut chunks);
                units.push(std::mem::replace(
                    &mut unit,
                    Unit {
                        paras: 0,
                        chunks: 0,
                        opens: Vec::new(),
                    },
                ));
                from = pi;
                unit_start = end;
            }
        }
        if from < ends.len() || !cur.is_empty() {
            unit.paras = ends.len() - from;
            close(&mut cur, &mut unit, unit_start, text.len(), &mut chunks);
            units.push(unit);
        }
        (units, chunks)
    }

    /// Align old chunks `o.0..o.1` with new chunks `n.0..n.1`, and diff the
    /// runs that differ: the segments.
    #[allow(clippy::many_single_char_names)]
    fn align(
        &self,
        base: &Baseline,
        o: (usize, usize),
        n: (usize, usize),
        stats: &mut Stats,
    ) -> Vec<Seg> {
        let ok = &self.old.keys[o.0..o.1];
        let nk: Vec<u64> = self.chunks[n.0..n.1].iter().map(|c| c.key).collect();
        let pairs = lcs(ok, &nk);
        let mut segs: Vec<Seg> = Vec::new();
        let (mut i, mut j) = (0, 0);
        for (pi, pj) in pairs
            .into_iter()
            .chain(std::iter::once((o.1 - o.0, n.1 - n.0)))
        {
            if i < pi || j < pj {
                let run = self.run(base, (o.0 + i, o.0 + pi), (n.0 + j, n.0 + pj));
                stats.chunks_diffed += pj - j;
                segs.push(Seg {
                    old: pi - i,
                    new: pj - j,
                    run: Some(run),
                });
            }
            if pi < o.1 - o.0 {
                match segs.last_mut() {
                    Some(s) if s.run.is_none() => {
                        s.old += 1;
                        s.new += 1;
                    }
                    _ => segs.push(Seg {
                        old: 1,
                        new: 1,
                        run: None,
                    }),
                }
            }
            i = pi + 1;
            j = pj + 1;
        }
        stats.segments += segs.len();
        segs
    }

    /// Diff old chunks `o.0..o.1` against new chunks `n.0..n.1` by tokens.
    fn run(&self, base: &Baseline, o: (usize, usize), n: (usize, usize)) -> Run {
        let old = &self.old;
        let tok_at = |c: usize| old.chunks.get(c).map_or(old.toks.len(), |r| r.start);
        let (ta, tb) = (tok_at(o.0), tok_at(o.1));
        let old_end = old.toks.get(tb).map_or(base.body.end, |t| t.start);
        // (the new chunks' text and tokens, from the run's start)
        let mut text = String::new();
        let mut toks = Vec::new();
        for c in &self.chunks[n.0..n.1] {
            let at = text.len();
            text.push_str(&c.text);
            toks.extend(c.toks.iter().map(|t| shifted(t, at)));
        }
        let mut em = Em::new(&base.flat.text, &text, &self.sigs, self.amsmath);
        em.run(&old.toks[ta..tb], &toks, old_end, text.len());
        let nl = em.nl;
        let section = em.section.clone();
        let changes = em
            .changes
            .iter()
            .map(|c| RunChange {
                raw: c.clone(),
                new_text: text[c.new.0..c.new.1].to_owned(),
            })
            .collect();
        Run {
            out: em.out,
            nl,
            section,
            changes,
        }
    }

    /// The diff: the head, each segment's text, the tail; the changes,
    /// placed.
    fn assemble(&self, base: &Baseline) -> DiffOut {
        let mut tex = String::with_capacity(self.head.len() + self.flat.text.len() * 11 / 10);
        tex.push_str(&self.head);
        let mut changes = Vec::new();
        let mut nl = false;
        let mut section: Option<String> = None;
        let mut pos = self.body.start;
        let mut c = 0;
        let put = |tex: &mut String, s: &str, nl: &mut bool| -> bool {
            if s.is_empty() {
                return false;
            }
            let owed = *nl && !s.starts_with('\n');
            if owed {
                tex.push('\n');
            }
            *nl = false;
            tex.push_str(s);
            owed
        };
        for seg in &self.segs {
            let chunks = &self.chunks[c..c + seg.new];
            c += seg.new;
            match &seg.run {
                None => {
                    for ch in chunks {
                        put(&mut tex, &ch.text, &mut nl);
                        if ch.section.is_some() {
                            section.clone_from(&ch.section);
                        }
                        pos += ch.text.len();
                    }
                }
                Some(run) => {
                    let before = tex.len();
                    let owed = put(&mut tex, &run.out, &mut nl);
                    let at = tex.len() - run.out.len();
                    if !run.out.is_empty() {
                        nl = run.nl;
                    }
                    for rc in &run.changes {
                        let r = &rc.raw;
                        let new = (pos + r.new.0, pos + r.new.1);
                        let out_start = if owed && r.out.0 == 0 {
                            before
                        } else {
                            at + r.out.0
                        };
                        let sec = r.section.clone().or_else(|| section.clone());
                        let loc = locate(&self.flat, new);
                        changes.push(base.change(
                            r,
                            loc,
                            rc.new_text.clone(),
                            sec,
                            out_start..at + r.out.1,
                        ));
                    }
                    if run.section.is_some() {
                        section.clone_from(&run.section);
                    }
                    pos += chunks.iter().map(|ch| ch.text.len()).sum::<usize>();
                }
            }
        }
        put(&mut tex, &self.flat.text[self.body.end..], &mut nl);
        if nl {
            tex.push('\n');
        }
        DiffOut { tex, changes }
    }
}

fn locate(f: &Flat, r: (usize, usize)) -> Loc {
    let (file, start, end) = f.locate_range(r.0, r.1);
    Loc {
        file: f.files.get(file).cloned().unwrap_or_default(),
        start,
        end,
    }
}

/// A chunk of `text` (its tokens `toks`, at `at` in the text they were
/// read from).
fn chunk(text: &str, toks: &[Tok], at: usize) -> Chunk {
    let toks: Vec<Tok> = toks.iter().map(|t| unshifted(t, at)).collect();
    let section = toks.iter().rev().find_map(|t| match &t.kind {
        Kind::Text { name, .. } if SECTIONS.contains(&name.as_str()) => {
            Some(text[t.open_end..t.close_start].trim().to_owned())
        }
        _ => None,
    });
    Chunk {
        text: text.to_owned(),
        key: emit::chunk_key(&toks),
        toks,
        section,
    }
}

/// Token `t` and its children moved `by` bytes on.
fn shifted(t: &Tok, by: usize) -> Tok {
    let mut t = t.clone();
    shift(&mut t, |x| x + by);
    t
}

/// Token `t` and its children moved `by` bytes back.
fn unshifted(t: &Tok, by: usize) -> Tok {
    let mut t = t.clone();
    shift(&mut t, |x| x - by);
    t
}

fn shift(t: &mut Tok, f: impl Fn(usize) -> usize + Copy) {
    t.start = f(t.start);
    t.cend = f(t.cend);
    t.end = f(t.end);
    t.open_end = f(t.open_end);
    t.close_start = f(t.close_start);
    if let Kind::Display { body, .. } = &mut t.kind {
        *body = (f(body.0), f(body.1));
    }
    for k in &mut t.children {
        shift(k, f);
    }
}

/// Whether paragraph `g` holds a group its text does not close, or ends
/// in a verbatim run (which may be one its text does not end).
fn open_at_end(g: &phitex_syntax::Green) -> bool {
    g.children()
        .last()
        .is_some_and(|c| c.kind() == SyntaxKind::Verbatim)
        || g.children().iter().any(|c| {
            c.kind() == SyntaxKind::Group
                && (c.children().len() < 2
                    || c.children()
                        .last()
                        .is_none_or(|l| l.kind() != SyntaxKind::Brace))
        })
}

/// The opener an unsafe token is, if it reads on to a closer it did not
/// find (`\begin{name}` alone, `\[`, `\(`).
fn opener(s: &str) -> Option<Opener> {
    match s {
        "\\[" => return Some(Opener::Display),
        "\\(" => return Some(Opener::Inline),
        _ => {}
    }
    let rest = s.strip_prefix("\\begin")?.trim_start();
    let name = rest.strip_prefix('{')?.strip_suffix('}')?;
    Some(Opener::Env(name.trim().to_owned()))
}

/// Whether `text` may hold a closer of `o` (a closer in a comment counts:
/// it only makes more be read).
fn has_closer(text: &str, o: &Opener) -> bool {
    match o {
        Opener::Display => text.contains("\\]"),
        Opener::Inline => text.contains("\\)"),
        Opener::Env(name) => text.match_indices("\\end").any(|(k, _)| {
            let rest = text[k + 4..].trim_start();
            rest.strip_prefix('{')
                .and_then(|r| r.split_once('}'))
                .is_some_and(|(n, _)| n.trim() == name)
        }),
    }
}

/// Where each change is shown, from where the glyphs of the marked-up
/// document came from: `glyphs` in the order they are shown (by page),
/// each its page, its y (points from the page's top) and the bytes of the
/// marked-up text it came from. A change is shown where its first glyph is.
pub fn places(
    changes: &[Change],
    glyphs: impl IntoIterator<Item = (usize, f32, usize, usize)>,
) -> Vec<Option<Place>> {
    let mut out = vec![None; changes.len()];
    let mut left = changes.len();
    for (page, y, a, b) in glyphs {
        if left == 0 {
            break;
        }
        // (the changes whose markup holds bytes of `a..b`)
        let mut k = changes.partition_point(|c| c.out.end <= a);
        while k < changes.len() && changes[k].out.start < b.max(a + 1) {
            if out[k].is_none() {
                out[k] = Some(Place { page, y });
                left -= 1;
            }
            k += 1;
        }
    }
    out
}

/// The changes as JSON: `[{"kind":"add"|"del"|"change","old":{"file",
/// "start","end"},"new":{…},"old_text","new_text","section"(null: none),
/// "page","y"}]`, `page` (from 0) and `y` only where it is known.
#[must_use]
pub fn changes_json(changes: &[Change], places: &[Option<Place>]) -> String {
    let mut s = String::from("[");
    for (i, c) in changes.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        let kind = match c.kind {
            ChangeKind::Add => "add",
            ChangeKind::Del => "del",
            ChangeKind::Change => "change",
        };
        let loc = |s: &mut String, l: &Loc| {
            s.push_str("{\"file\":");
            json_str(s, &l.file);
            let _ = write!(s, ",\"start\":{},\"end\":{}}}", l.start, l.end);
        };
        let _ = write!(s, "{{\"kind\":\"{kind}\",\"old\":");
        loc(&mut s, &c.old);
        s.push_str(",\"new\":");
        loc(&mut s, &c.new);
        s.push_str(",\"old_text\":");
        json_str(&mut s, &c.old_text);
        s.push_str(",\"new_text\":");
        json_str(&mut s, &c.new_text);
        s.push_str(",\"section\":");
        match &c.section {
            Some(t) => json_str(&mut s, t),
            None => s.push_str("null"),
        }
        if let Some(Some(p)) = places.get(i) {
            let _ = write!(s, ",\"page\":{},\"y\":{:.2}", p.page, p.y);
        }
        s.push('}');
    }
    s.push(']');
    s
}

/// `t` as a JSON string.
fn json_str(s: &mut String, t: &str) {
    s.push('"');
    for ch in t.chars() {
        match ch {
            '"' => s.push_str("\\\""),
            '\\' => s.push_str("\\\\"),
            '\n' => s.push_str("\\n"),
            '\r' => s.push_str("\\r"),
            '\t' => s.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(s, "\\u{:04x}", c as u32);
            }
            c => s.push(c),
        }
    }
    s.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Dir, Options, flatten};
    use std::collections::BTreeMap;
    use std::path::Path;

    fn corpus() -> Vec<std::path::PathBuf> {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
        let mut cases: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .collect();
        cases.sort();
        cases
    }

    /// A case's old version (none: an empty one).
    fn base_of(case: &Path) -> Baseline {
        Baseline::new(&Dir(case.join("old")), "main.tex", &Options::default())
            .unwrap_or_else(|_| Baseline::from_flat(Flat::default(), &Options::default()))
    }

    fn one(text: &str) -> Flat {
        let mut m = BTreeMap::new();
        m.insert("main.tex".to_owned(), text.to_owned());
        flatten(&m, "main.tex").unwrap()
    }

    fn same(a: &DiffOut, b: &DiffOut) -> bool {
        a.tex == b.tex && a.changes == b.changes
    }

    #[test]
    fn first_update_is_the_whole_diff() {
        for case in corpus() {
            let base = base_of(&case);
            let new = flatten(&Dir(case.join("new")), "main.tex").unwrap();
            let want = base.diff_flat(&new);
            let mut live = Live::new(base);
            let got = live.update(&new).clone();
            assert!(live.stats().whole);
            assert_eq!(got.tex, want.tex, "{}", case.display());
            assert_eq!(got.changes, want.changes, "{}", case.display());
        }
    }

    /// Each unit read alone gives the tokens the whole body gives.
    #[test]
    fn units_read_alone() {
        for case in corpus() {
            let base = base_of(&case);
            let new = flatten(&Dir(case.join("new")), "main.tex").unwrap();
            let mut st = Stats::default();
            let side = Side::whole(&base, &new, &mut st);
            let body = &new.text[side.body.start..side.body.end];
            let whole: String = side.chunks.iter().map(|c| c.text.as_str()).collect();
            assert_eq!(whole, body, "{}: chunks tile the body", case.display());
            let r = tok::Reader {
                text: &new.text,
                sigs: &side.sigs,
            };
            let toks = r.seq(&tok::lexemes(&new.text, side.body.start, side.body.end));
            let keys: Vec<u64> = emit::chunks(&toks)
                .iter()
                .map(|r| emit::chunk_key(&toks[r.clone()]))
                .collect();
            let mine: Vec<u64> = side.chunks.iter().map(|c| c.key).collect();
            assert_eq!(keys, mine, "{}", case.display());
        }
    }

    /// A small deterministic generator (xorshift).
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn below(&mut self, n: usize) -> usize {
            usize::try_from(self.next() % (n.max(1) as u64)).unwrap()
        }
    }

    const PIECES: &[&str] = &[
        "word ",
        " brave",
        "\n\n",
        "\n\nA new paragraph.\n\n",
        "\\section{Added}\n",
        "\\begin{itemize}\n\\item one\n",
        "\\end{itemize}\n",
        "\\begin{equation}\nx = 1\n\\end{equation}\n",
        "$x^2$",
        "\\[ y \\]",
        "\\[",
        "\\]",
        "{",
        "}",
        "% a comment\n",
        "\\emph{it}",
        "\\cite{k}",
        "  ",
    ];

    /// Random edits of the body (words, paragraphs, sections, openers and
    /// closers alone, braces, comments), each diffed live and whole: the
    /// same output, the same chunks read, and the body found where the
    /// whole diff finds it.
    #[test]
    fn random_edits_match_the_whole_diff() {
        let (mut live_read, mut whole_read) = (0, 0);
        for seed in [0x9e37_79b9_7f4a_7c15_u64, 7, 1_234_567] {
            let mut rng = Rng(seed);
            for case in corpus() {
                let base = base_of(&case);
                let mut text = flatten(&Dir(case.join("new")), "main.tex").unwrap().text;
                let mut live = Live::new(base_of(&case));
                live.update(&one(&text));
                for _ in 0..60 {
                    let prev = text.clone();
                    let b = find_body(&text);
                    let (lo, hi) = (b.start, b.end);
                    let mut at = lo + rng.below(hi - lo + 1);
                    while !text.is_char_boundary(at) {
                        at -= 1;
                    }
                    if rng.below(3) == 0 {
                        let mut end = (at + rng.below(30)).min(hi);
                        while !text.is_char_boundary(end) {
                            end -= 1;
                        }
                        text.replace_range(at..end, "");
                    } else {
                        text.insert_str(at, PIECES[rng.below(PIECES.len())]);
                    }
                    let flat = one(&text);
                    let got = live.update(&flat).clone();
                    let want = base.diff_flat(&flat);
                    let side = live.side.as_ref().unwrap();
                    let fb = find_body(&flat.text);
                    assert_eq!(
                        (side.body.begin, side.body.start, side.body.end),
                        (fb.begin, fb.start, fb.end),
                        "{}: the body: {:?} -> {:?}",
                        case.display(),
                        prev,
                        text
                    );
                    let r = tok::Reader {
                        text: &flat.text,
                        sigs: &side.sigs,
                    };
                    let toks = r.seq(&tok::lexemes(&flat.text, fb.start, fb.end));
                    let keys: Vec<u64> = emit::chunks(&toks)
                        .iter()
                        .map(|r| emit::chunk_key(&toks[r.clone()]))
                        .collect();
                    let mine: Vec<u64> = side.chunks.iter().map(|c| c.key).collect();
                    assert_eq!(
                        keys,
                        mine,
                        "{}: the chunks: {:?} -> {:?}\nmine {:?}\nwhole {:?}",
                        case.display(),
                        prev,
                        text,
                        side.chunks
                            .iter()
                            .map(|c| c.text.as_str())
                            .collect::<Vec<_>>(),
                        emit::chunks(&toks)
                            .iter()
                            .map(|r| &flat.text[toks[r.start].start..toks[r.end - 1].end])
                            .collect::<Vec<_>>()
                    );
                    assert_eq!(got.tex, want.tex, "{}: the text", case.display());
                    assert_eq!(got.changes, want.changes, "{}: the changes", case.display());
                    live_read += live.stats().paras_read;
                    whole_read += side.tree.paras().len();
                }
            }
        }
        eprintln!("paragraphs read: {live_read} live, {whole_read} whole");
    }

    /// Word edits all over a long document: the whole diff's output each
    /// time, from at most the paragraphs edited.
    #[test]
    fn word_edits_read_their_paragraphs() {
        let old = doc(300);
        let mut m = BTreeMap::new();
        m.insert("main.tex".to_owned(), old.clone());
        let base = Baseline::new(&m, "main.tex", &Options::default()).unwrap();
        let mut live = Live::new(Baseline::new(&m, "main.tex", &Options::default()).unwrap());
        let mut text = old.clone();
        live.update(&one(&text));
        let mut rng = Rng(42);
        for k in 0..200 {
            let i = rng.below(300);
            let words = ["says", "something", "about", "more"];
            let w = words[rng.below(words.len())];
            let from = format!("Paragraph {i} ");
            let Some(at) = text.find(&from) else { continue };
            let seg_end = at + text[at..].find("\n\n").unwrap();
            let Some(k2) = text[at..seg_end].find(w) else {
                continue;
            };
            let pos = at + k2;
            if k % 2 == 0 {
                text.insert_str(pos, "new ");
            } else {
                text.replace_range(pos..pos + w.len(), "other");
            }
            let flat = one(&text);
            let got = live.update(&flat).clone();
            let st = live.stats();
            // (and the chunks of the run it is in: a changed paragraph
            // beside it is diffed with it)
            assert!(!st.whole && st.paras_read == 1, "{st:?}");
            let runs: Vec<usize> = live
                .side
                .as_ref()
                .unwrap()
                .segs
                .iter()
                .filter(|s| s.run.is_some())
                .map(|s| s.new)
                .collect();
            assert!(
                st.chunks_diffed <= runs.iter().copied().max().unwrap_or(0),
                "{st:?}"
            );
            assert!(same(&got, &base.diff_flat(&flat)));
        }
    }

    fn doc(paras: usize) -> String {
        let mut s = String::from("\\documentclass{article}\n\\begin{document}\n");
        for i in 0..paras {
            if i % 10 == 0 {
                let _ = writeln!(s, "\\section{{Part {i}}}\n");
            }
            let _ = write!(
                s,
                "Paragraph {i} says something about item {i}, and more.\n\n"
            );
        }
        s.push_str("\\end{document}\n");
        s
    }

    #[test]
    fn a_one_word_edit_rediffs_one_paragraph() {
        let old = doc(200);
        let mut m = BTreeMap::new();
        m.insert("main.tex".to_owned(), old.clone());
        let base = Baseline::new(&m, "main.tex", &Options::default()).unwrap();
        let mut live = Live::new(base);
        live.update(&one(&old));
        assert!(live.stats().whole);
        assert_eq!(live.out().changes.len(), 0);
        // (one word: one paragraph read, one chunk diffed)
        let new = old.replace("item 120,", "thing 120,");
        let out = live.update(&one(&new)).clone();
        assert_eq!(
            live.stats(),
            Stats {
                whole: false,
                paras_read: 1,
                chunks_diffed: 1,
                segments: 1,
            }
        );
        assert_eq!(out.changes.len(), 1);
        assert_eq!(out.changes[0].old_text, "item");
        assert_eq!(out.changes[0].new_text, "thing");
        assert_eq!(out.changes[0].section.as_deref(), Some("Part 120"));
        let mut m2 = BTreeMap::new();
        m2.insert("main.tex".to_owned(), old.clone());
        let whole = Baseline::new(&m2, "main.tex", &Options::default())
            .unwrap()
            .diff_flat(&one(&new));
        assert!(same(&out, &whole));
        // (and back: the change gone, again one paragraph)
        live.update(&one(&old));
        assert_eq!(live.stats().paras_read, 1);
        assert_eq!(live.stats().chunks_diffed, 0);
        assert_eq!(live.out().changes, []);
        // (an environment spanning blank lines is read whole)
        let env = old.replace(
            "Paragraph 50 says",
            "\\begin{quote}\nA\n\nB\n\\end{quote}\nParagraph 50 says",
        );
        live.update(&one(&env));
        let e2 = env.replace("\nB\n", "\nC\n");
        live.update(&one(&e2));
        assert_eq!(live.stats().paras_read, 2);
        // (a preamble edit: whole)
        let pre = e2.replace("{article}", "{report}");
        live.update(&one(&pre));
        assert!(live.stats().whole);
    }

    #[test]
    fn openers_read_on() {
        let old = doc(30);
        let mut m = BTreeMap::new();
        m.insert("main.tex".to_owned(), old.clone());
        let base = || Baseline::new(&m, "main.tex", &Options::default()).unwrap();
        let mut live = Live::new(base());
        live.update(&one(&old));
        // (a \begin with no \end: then its \end, far after it)
        let a = old.replace("Paragraph 5 says", "\\begin{quote}Paragraph 5 says");
        let b = a.replace("Paragraph 20 says", "\\end{quote}Paragraph 20 says");
        let c = b.replace("\\begin{quote}", "");
        for t in [&a, &b, &c, &a, &old] {
            let got = live.update(&one(t)).clone();
            assert!(same(&got, &base().diff_flat(&one(t))));
        }
    }

    #[test]
    fn places_and_json() {
        let old = doc(3);
        let new = old.replace("item 1,", "thing 1,");
        let mut m = BTreeMap::new();
        m.insert("main.tex".to_owned(), old);
        let base = Baseline::new(&m, "main.tex", &Options::default()).unwrap();
        let mut live = Live::new(base);
        let out = live.update(&one(&new)).clone();
        let c = &out.changes[0];
        let p = places(
            &out.changes,
            [(0, 10.0, 0, 5), (1, 72.5, c.out.start + 2, c.out.start + 3)],
        );
        assert_eq!(p, [Some(Place { page: 1, y: 72.5 })]);
        let j = changes_json(&out.changes, &p);
        assert!(
            j.starts_with("[{\"kind\":\"change\",\"old\":{\"file\":\"main.tex\",\"start\":"),
            "{j}"
        );
        assert!(j.contains("\"old_text\":\"item\",\"new_text\":\"thing\",\"section\":\"Part 0\",\"page\":1,\"y\":72.50}]"), "{j}");
    }
}
