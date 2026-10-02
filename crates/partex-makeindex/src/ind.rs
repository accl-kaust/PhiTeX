//! Writing the `.ind` file (genind.c).

use alloc::vec::Vec;

use crate::{ALPHA, DUPLICATE, FIELD_MAX, Mk, P, SYMBOL, at, fmt, strtoint, tolower, toupper};

/// genind.c's statics.
#[derive(Default)]
struct Gen {
    curr: Option<usize>,
    prev: Option<usize>,
    begin: Option<usize>,
    the_end: Option<usize>,
    range_ptr: Option<usize>,
    level: usize,
    prev_level: usize,
    encap: Vec<u8>,
    prev_encap: Option<Vec<u8>>,
    in_range: bool,
    encap_range: bool,
    buff: Vec<u8>,
    line: Vec<u8>,
    ind_lc: i32,
    ind_ec: i32,
    ind_indent: i32,
}

fn cat(parts: &[&[u8]]) -> Vec<u8> {
    parts.concat()
}

impl Mk<'_> {
    fn put(&mut self, s: &[u8]) {
        self.ind.extend_from_slice(s);
    }

    fn putln(&mut self, g: &mut Gen, s: &[u8]) {
        self.ind.extend_from_slice(s);
        self.ind.push(b'\n');
        g.ind_lc += 1;
    }

    /// `IND_ERROR`: a warning about the current entry.
    fn ind_error(&mut self, g: &mut Gen, f: &str, args: &[P]) {
        self.error_start();
        let e = &self.entries[g.curr.unwrap_or(0)];
        let name = self.idx_names[e.fn_].clone();
        let lc = e.lc;
        let ind_fn = self.ind_fn.clone();
        let head = fmt(
            "## Warning (input = %s, line = %d; output = %s, line = %d):\n   -- ",
            &[P::S(&name), lc.into(), P::S(&ind_fn), (g.ind_lc + 1).into()],
        );
        self.ilg_put(&head);
        let s = fmt(f, args);
        self.ilg_put(&s);
        g.ind_ec += 1;
    }

    /// `SAVE`.
    fn save(g: &mut Gen) {
        g.begin = g.curr;
        g.the_end = g.curr;
        g.prev_encap = Some(g.encap.clone());
    }

    /// `gen_ind`.
    pub(crate) fn gen_ind(&mut self) {
        let mut g = Gen::default();
        let ind_fn = self.ind_fn.clone();
        self.message("Generating output file %s...", &[P::S(&ind_fn)]);
        let pre = self.st.preamble.clone();
        self.put(&pre);
        g.ind_lc += self.st.prelen;
        if self.init_page {
            self.insert_page(&mut g);
        }
        self.idx_dc = 0;
        for n in 0..self.idx_key.len() {
            if self.entries[self.idx_key[n]].typ != DUPLICATE {
                self.make_entry(&mut g, n);
                self.idx_dot_tick(crate::DOT_MAX);
            }
        }
        if g.in_range {
            g.curr = g.range_ptr;
            let r = self.st.idx_ropen;
            self.ind_error(
                &mut g,
                "Unmatched range opening operator %c.\n",
                &[P::S(&[r])],
            );
        }
        g.prev = g.curr;
        self.flush_line(&mut g, true);
        let t = self.st.delim_t.clone();
        self.put(&t);
        let post = self.st.postamble.clone();
        self.put(&post);
        let tmp_lc = g.ind_lc + self.st.postlen;
        let w = if g.ind_ec == 1 { "warning" } else { "warnings" };
        self.done(tmp_lc, "lines written", g.ind_ec, w);
    }

    /// `make_entry`.
    fn make_entry(&mut self, g: &mut Gen, n: usize) {
        g.prev = g.curr;
        let c = self.idx_key[n];
        g.curr = Some(c);
        let (ropen, rclose) = (self.st.idx_ropen, self.st.idx_rclose);
        let e0 = at(&self.entries[c].encap, 0);
        g.encap = if e0 == ropen || e0 == rclose {
            self.entries[c].encap[1..].to_vec()
        } else {
            self.entries[c].encap.clone()
        };
        match g.prev.filter(|_| n != 0) {
            None => {
                g.prev_level = 0;
                g.level = 0;
                let let_ = i32::from(at(&self.entries[c].sf[0], 0));
                self.put_header(g, let_);
                self.make_item(g, b"");
            }
            Some(p) => {
                g.prev_level = g.level;
                let (ce, pe) = (&self.entries[c], &self.entries[p]);
                g.level = (0..FIELD_MAX)
                    .find(|&l| ce.sf[l] != pe.sf[l] || ce.af[l] != pe.af[l])
                    .unwrap_or(FIELD_MAX);
                if g.level < FIELD_MAX {
                    self.new_entry(g);
                } else if !(e0 == ropen && g.in_range) {
                    self.old_entry(g);
                }
            }
        }
        let encap = self.entries[c].encap.clone();
        if e0 == ropen && e0 != 0 {
            if g.in_range {
                self.ind_error(g, "Extra range opening operator %c.\n", &[P::S(&[ropen])]);
            } else {
                g.in_range = true;
                g.range_ptr = g.curr;
            }
        } else if e0 == rclose && e0 != 0 {
            if g.in_range {
                g.in_range = false;
                let rest = &encap[1..];
                if !rest.is_empty() && g.prev_encap.as_deref() != Some(rest) {
                    self.ind_error(
                        g,
                        "Range closing operator has an inconsistent encapsulator %s.\n",
                        &[P::S(rest)],
                    );
                }
            } else {
                self.ind_error(
                    g,
                    "Unmatched range closing operator %c.\n",
                    &[P::S(&[rclose])],
                );
            }
        } else if e0 != 0 && g.prev_encap.as_deref() != Some(&encap[..]) && g.in_range {
            self.ind_error(
                g,
                "Inconsistent page encapsulator %s within range.\n",
                &[P::S(&encap)],
            );
        }
    }

    /// `make_item`.
    fn make_item(&mut self, g: &mut Gen, term: &[u8]) {
        let c = Self::idx_key_curr(g);
        let level = g.level;
        let text = |mk: &Self, l: usize| {
            let e = &mk.entries[c];
            if e.af[l].is_empty() {
                e.sf[l].clone()
            } else {
                e.af[l].clone()
            }
        };
        if level > g.prev_level {
            g.line = cat(&[term, &self.st.item_u[level], &text(self, level)]);
            g.ind_lc += self.st.ilen_u[level];
        } else {
            g.line = cat(&[term, &self.st.item_r[level], &text(self, level)]);
            g.ind_lc += self.st.ilen_r[level];
        }
        let mut i = level + 1;
        while i < FIELD_MAX && !self.entries[c].sf[i].is_empty() {
            let line = core::mem::take(&mut g.line);
            self.put(&line);
            g.line = cat(&[&self.st.item_x[i], &text(self, i)]);
            g.ind_lc += self.st.ilen_x[i];
            g.level = i;
            i += 1;
        }
        g.ind_indent = 0;
        g.line.extend_from_slice(&self.st.delim_p[g.level]);
        Self::save(g);
    }

    fn idx_key_curr(g: &Gen) -> usize {
        g.curr.unwrap_or(0)
    }

    /// `first_letter`.
    fn first_letter(&self, term: &[u8]) -> u8 {
        if self.thai_sort {
            // (`strchr` finds the terminator too)
            return if at(term, 0) == 0 || b"\xe0\xe1\xe2\xe3\xe4".contains(&at(term, 0)) {
                at(term, 1)
            } else {
                at(term, 0)
            };
        }
        tolower(at(term, 0))
    }

    /// `new_entry`.
    fn new_entry(&mut self, g: &mut Gen) {
        let mut let_ = -1i32;
        if g.in_range {
            let ptr = g.curr;
            g.curr = g.range_ptr;
            let r = self.st.idx_ropen;
            self.ind_error(g, "Unmatched range opening operator %c.\n", &[P::S(&[r])]);
            g.in_range = false;
            g.curr = ptr;
        }
        self.flush_line(g, true);
        let c = g.curr.unwrap_or(0);
        let p = g.prev.unwrap_or(0);
        let (cg, pg) = (self.entries[c].group, self.entries[p].group);
        let new_group = (cg != ALPHA && cg != pg && pg == SYMBOL)
            || (cg == ALPHA && {
                let l = self.first_letter(&self.entries[c].sf[0]);
                let_ = i32::from(l);
                l != self.first_letter(&self.entries[p].sf[0])
            })
            || (self.german_sort && cg != ALPHA && pg == ALPHA);
        if new_group {
            let t = self.st.delim_t.clone();
            self.put(&t);
            let s = self.st.group_skip.clone();
            self.put(&s);
            g.ind_lc += self.st.skiplen;
            self.put_header(g, let_);
            self.make_item(g, b"");
        } else {
            let t = self.st.delim_t.clone();
            self.make_item(g, &t);
        }
    }

    /// `old_entry`: the same key again; append its page.
    fn old_entry(&mut self, g: &mut Gen) {
        let c = g.curr.unwrap_or(0);
        let p = g.prev.unwrap_or(0);
        let diff = self.page_diff(g.the_end.unwrap_or(0), c);
        let same_type = self.entries[p].typ == self.entries[c].typ;
        let same_encap = g.prev_encap.as_deref() == Some(&g.encap[..]);
        if same_type
            && diff != -1
            && ((diff == 0 && same_encap)
                || (self.merge_page && diff == 1 && same_encap)
                || g.in_range)
        {
            g.the_end = g.curr;
            let encap = self.entries[c].encap.clone();
            if g.in_range
                && !encap.is_empty()
                && encap[0] != self.st.idx_rclose
                && g.prev_encap.as_deref() != Some(&encap[..])
            {
                g.buff = cat(&[
                    &self.st.encap_p,
                    &encap,
                    &self.st.encap_i,
                    &self.entries[c].lpg,
                    &self.st.encap_s,
                ]);
                self.wrap_line(g, false);
            }
            if g.in_range {
                g.encap_range = true;
            }
        } else {
            self.flush_line(g, false);
            if diff == 0 && same_type {
                self.ind_error(
                    g,
                    "Conflicting entries: multiple encaps for the same page under same key.\n",
                    &[],
                );
            } else if g.in_range && !same_type {
                self.ind_error(
                    g,
                    "Illegal range formation: starting & ending pages are of different types.\n",
                    &[],
                );
            } else if g.in_range && diff == -1 {
                self.ind_error(
                    g,
                    "Illegal range formation: starting & ending pages cross chap/sec breaks.\n",
                    &[],
                );
            }
            Self::save(g);
        }
    }

    /// `page_diff`.
    fn page_diff(&self, a: usize, b: usize) -> i32 {
        let (a, b) = (&self.entries[a], &self.entries[b]);
        if a.count != b.count {
            return -1;
        }
        for i in 0..a.count.saturating_sub(1) {
            if a.npg[i] != b.npg[i] {
                return -1;
            }
        }
        let last = |e: &crate::Field| e.npg[e.count.wrapping_sub(1).min(crate::PAGEFIELD_MAX - 1)];
        last(b).wrapping_sub(last(a))
    }

    /// `put_header`.
    fn put_header(&mut self, g: &mut Gen, let_: i32) {
        let flag = self.st.headings_flag;
        if flag == 0 {
            return;
        }
        let pre = self.st.heading_pre.clone();
        self.put(&pre);
        g.ind_lc += self.st.headprelen;
        match self.entries[g.curr.unwrap_or(0)].group {
            SYMBOL => {
                let s = if flag > 0 {
                    self.st.symhead_pos.clone()
                } else {
                    self.st.symhead_neg.clone()
                };
                self.put(&s);
            }
            ALPHA => {
                let c = let_ as u8;
                self.ind
                    .push(if flag > 0 { toupper(c) } else { tolower(c) });
            }
            _ => {
                let s = if flag > 0 {
                    self.st.numhead_pos.clone()
                } else {
                    self.st.numhead_neg.clone()
                };
                self.put(&s);
            }
        }
        let suf = self.st.heading_suf.clone();
        self.put(&suf);
        g.ind_lc += self.st.headsuflen;
    }

    /// `flush_line`: the pages collected for the entry.
    fn flush_line(&mut self, g: &mut Gen, print: bool) {
        let begin = g.begin.unwrap_or(0);
        let the_end = g.the_end.unwrap_or(0);
        let prev = g.prev.unwrap_or(0);
        let lpg = |mk: &Self, e: usize| mk.entries[e].lpg.clone();
        if self.page_diff(begin, the_end) != 0 {
            let two = !self.st.suffix_2p.is_empty();
            if g.encap_range || self.page_diff(begin, prev) > i32::from(!two) {
                let diff = self.page_diff(begin, the_end);
                let st = &self.st;
                g.buff = if diff == 1 && two {
                    cat(&[&lpg(self, begin), &st.suffix_2p])
                } else if diff == 2 && !st.suffix_3p.is_empty() {
                    cat(&[&lpg(self, begin), &st.suffix_3p])
                } else if diff >= 2 && !st.suffix_mp.is_empty() {
                    cat(&[&lpg(self, begin), &st.suffix_mp])
                } else {
                    cat(&[&lpg(self, begin), &st.delim_r, &lpg(self, the_end)])
                };
                g.encap_range = false;
            } else {
                g.buff = cat(&[&lpg(self, begin), &self.st.delim_n, &lpg(self, the_end)]);
            }
        } else {
            g.encap_range = false;
            g.buff = lpg(self, begin);
        }
        if let Some(pe) = g.prev_encap.as_ref().filter(|pe| !pe.is_empty()) {
            g.buff = cat(&[
                &self.st.encap_p,
                pe,
                &self.st.encap_i,
                &g.buff,
                &self.st.encap_s,
            ]);
        }
        self.wrap_line(g, print);
    }

    /// `wrap_line`.
    fn wrap_line(&mut self, g: &mut Gen, print: bool) {
        let len = (g.line.len() + g.buff.len()) as i32 + g.ind_indent;
        let long = len > self.st.linemax;
        if print {
            let line = g.line.clone();
            if long {
                self.putln(g, &line);
                let s = self.st.indent_space.clone();
                self.put(&s);
                g.ind_indent = self.st.indent_length;
            } else {
                self.put(&line);
            }
            let b = g.buff.clone();
            self.put(&b);
        } else if long {
            let line = g.line.clone();
            self.putln(g, &line);
            g.line = cat(&[&self.st.indent_space, &g.buff, &self.st.delim_n]);
            g.ind_indent = self.st.indent_length;
        } else {
            g.buff.extend_from_slice(&self.st.delim_n);
            g.line.extend_from_slice(&g.buff);
        }
    }

    /// `insert_page`: `\setcounter{page}` from `-p`.
    fn insert_page(&mut self, g: &mut Gen) {
        if self.even_odd >= 0 {
            let p = &self.pageno;
            let mut i = p.len();
            while i > 0 && p[i - 1].is_ascii_digit() {
                i -= 1;
            }
            let mut page = strtoint(&p[i..]).wrapping_add(1);
            if (self.even_odd == 1 && page % 2 == 0) || (self.even_odd == 2 && page % 2 == 1) {
                page += 1;
            }
            let mut n = p[..i].to_vec();
            n.extend_from_slice(alloc::format!("{page}").as_bytes());
            self.pageno = n;
        }
        let o = self.st.setpage_open.clone();
        self.put(&o);
        let p = self.pageno.clone();
        self.put(&p);
        let c = self.st.setpage_close.clone();
        self.put(&c);
        g.ind_lc += self.st.setpagelen;
    }
}
