//! Alignment (§768–§812): `\halign` as groups. The whole alignment is a
//! group whose list holds the rows; each entry is a group of its own
//! whose kind carries the row so far. The preamble is the field
//! `align:<id>`; `align_state` is the field `align:state`, kept only
//! while an alignment is open (`align:open`).
//!
//! The kinds' extras: the alignment `["align", spec code, size, saved
//! align_state]`; an entry `["cell", column, first column of its span,
//! the entry's nodes before a \span, the row so far, its adjustments,
//! omitted, alignment id]`; `\noalign` `[alignment id]`.

use phitex_defuse::ValueId;
use phitex_typeset::align::{
    AlignColumn, TAB_SKIP, column_widths, pack_entry, pack_row, prototype, set_alignment,
};
use phitex_typeset::glue::Glue;
use phitex_typeset::node::{GlueKind, GlueNode, List, Node};
use phitex_typeset::pack::Spec;

use crate::build::Mode;
use crate::fields::Par;
use crate::nodes::{self, Side};
use crate::run::{R, Run};
use crate::val::{
    Tok, cmd, glue_of, glue_value, group, int_of, list_of, str_of, toks_of, toks_value,
};

const SPAN_CODE: i32 = 256;
const CR_CODE: i32 = 257;
const CR_CR_CODE: i32 = 258;
const TABSKIP_LOC: i32 = 2893;

/// The preamble: `[tabskip before column 0, [[u, v, tabskip after]…],
/// the periodic part's next column or -1]`.
struct Preamble {
    tab0: Glue,
    cols: Vec<(Vec<Tok>, Vec<Tok>, Glue)>,
    looping: i64,
}

impl Preamble {
    fn encode(&self, v: &phitex_defuse::Values) -> ValueId {
        let cols = self
            .cols
            .iter()
            .map(|(u, w, g)| v.list(vec![toks_value(v, u), toks_value(v, w), glue_value(v, g)]))
            .collect();
        v.list(vec![
            glue_value(v, &self.tab0),
            v.list(cols),
            v.int(self.looping),
        ])
    }

    fn decode(v: &phitex_defuse::Values, x: Option<ValueId>) -> Preamble {
        let xs = x.map(|x| list_of(v, x)).unwrap_or_default();
        let cols = xs
            .get(1)
            .map(|&c| list_of(v, c))
            .unwrap_or_default()
            .into_iter()
            .map(|c| {
                let ys = list_of(v, c);
                (
                    toks_of(v, ys.first().copied()),
                    toks_of(v, ys.get(1).copied()),
                    glue_of(v, ys.get(2).copied()),
                )
            })
            .collect();
        Preamble {
            tab0: glue_of(v, xs.first().copied()),
            cols,
            looping: xs.get(2).map_or(-1, |&y| int_of(v, y)),
        }
    }
}

fn tab_glue(g: Glue) -> Node {
    Node::Glue(GlueNode {
        spec: g,
        kind: GlueKind::Param(TAB_SKIP),
        leader: None,
    })
}

impl Run<'_, '_> {
    // ---------------------------------------------------- align_state

    fn align_open(&mut self) -> R<bool> {
        Ok(self.int("align:open")? > 0)
    }

    fn astate(&mut self) -> R<i64> {
        if let Some(s) = self.astate {
            return Ok(s);
        }
        let s = self
            .get("align:state")?
            .map_or(1_000_000, |x| int_of(self.v, x));
        self.astate = Some(s);
        Ok(s)
    }

    pub(crate) fn set_astate(&mut self, s: i64) {
        self.astate = Some(s);
        let x = self.v.int(s);
        self.set("align:state", x, true);
    }

    /// The end of a u template (§324): `align_state` becomes 0.
    pub(crate) fn align_u_end(&mut self) -> R<()> {
        if self.align_open()? && self.astate()? > 500_000 {
            self.set_astate(0);
        }
        Ok(())
    }

    /// §342, §347, §357 for `cur`: braces count in `align_state`; a tab
    /// or `\cr` at `align_state` 0 inserts the v template (§789). True if
    /// `cur` was taken.
    pub(crate) fn align_token(&mut self) -> R<bool> {
        let c = self.cur.cmd;
        let brace =
            !self.cur.tok.is_cs_or_active() && (c == cmd::LEFT_BRACE || c == cmd::RIGHT_BRACE);
        let tab = c == cmd::TAB_MARK || c == cmd::CAR_RET;
        if !(brace || tab) || !self.align_open()? {
            return Ok(false);
        }
        let s = self.astate()?;
        if brace {
            self.set_astate(if c == cmd::LEFT_BRACE { s + 1 } else { s - 1 });
            return Ok(false);
        }
        if std::env::var_os("PHITEX_TEX_TRACE").is_some() {
            eprintln!("    align: {:?} state {s}", self.cur.tok);
        }
        if s != 0 {
            return Ok(false);
        }
        let Some(cell) = self.cur_cell() else {
            return Ok(false);
        };
        let v = self.v;
        let x = |i: usize| cell.get(i).map_or(0, |&y| int_of(v, y));
        let pre = self.preamble(x(7))?;
        let col = usize::try_from(x(1)).unwrap_or(0);
        let mut toks = if x(6) != 0 {
            Vec::new()
        } else {
            pre.cols.get(col).map(|c| c.1.clone()).unwrap_or_default()
        };
        toks.push(Tok::cs(&format!("\u{10FFFE}endv:{}", self.cur.chr)));
        self.push_toks(toks);
        self.set_astate(1_000_000);
        Ok(true)
    }

    /// The innermost entry's kind extra (`cur_align`).
    fn cur_cell(&self) -> Option<Vec<ValueId>> {
        let v = self.v;
        // the run's own levels, innermost out (an alignment is never
        // split across nodes)
        let is_cell = |xs: &[ValueId]| xs.first().map(|&t| str_of(v, t)).as_deref() == Some("cell");
        if let Some(l) = self
            .nest
            .iter()
            .rev()
            .find(|l| l.group == group::ALIGN && is_cell(&l.extra))
        {
            return Some(l.extra.clone());
        }
        None
    }

    fn preamble(&mut self, asid: i64) -> R<Preamble> {
        let x = self.get(&format!("align:{asid}"))?;
        Ok(Preamble::decode(self.v, x))
    }

    // ------------------------------------------------------ init_align

    /// `init_align` (§774–§784) in vertical mode.
    pub(crate) fn init_align(&mut self, md: &Mode) -> R<()> {
        let v = self.v;
        let saved = if self.align_open()? {
            self.astate()?
        } else {
            1_000_000
        };
        let open = self.int("align:open")?;
        self.set("align:open", v.int(open + 1), true);
        self.set_astate(-1_000_000);
        let spec = if self.scan_keyword("to")? {
            (0, self.scan_normal_dimen()?)
        } else if self.scan_keyword("spread")? {
            (1, self.scan_normal_dimen()?)
        } else {
            (1, 0)
        };
        self.scan_left_brace()?;
        // §777: "at this point, cur_cmd = left_brace"
        self.set_astate(-1_000_000);
        let extra = vec![
            v.str("align"),
            v.int(spec.0),
            v.int(i64::from(spec.1)),
            v.int(saved),
        ];
        // the alignment: a level of the nest owning the rows' list
        let asid = self.push_list(-1, group::ALIGN, extra);
        let pd = self.prev_depth(md)?;
        let m = v.list(vec![v.int(crate::nodes::T_PD), v.int(i64::from(pd))]);
        self.append(asid, m);
        // §777
        let mut tab = self.gpar(Par::tabskip)?;
        let mut pre = Preamble {
            tab0: tab,
            cols: Vec::new(),
            looping: -1,
        };
        loop {
            // §779: the u part
            let mut u = Vec::new();
            loop {
                self.get_preamble_token(&mut tab)?;
                let c = self.cur.cmd;
                if c == cmd::MAC_PARAM {
                    break;
                }
                if (c == cmd::TAB_MARK || c == cmd::CAR_RET) && self.astate()? == -1_000_000 {
                    if u.is_empty() && pre.looping < 0 && c == cmd::TAB_MARK {
                        pre.looping = i64::try_from(pre.cols.len()).unwrap_or(0);
                    } else {
                        self.log("error", "Missing # inserted in alignment preamble")?;
                        self.back_cur();
                        break;
                    }
                } else if c != cmd::SPACER || !u.is_empty() {
                    u.push(self.cur.tok);
                }
            }
            // §783: the v part
            let mut w = Vec::new();
            loop {
                self.get_preamble_token(&mut tab)?;
                let c = self.cur.cmd;
                if (c == cmd::TAB_MARK || c == cmd::CAR_RET) && self.astate()? == -1_000_000 {
                    break;
                }
                if c == cmd::MAC_PARAM {
                    self.log("error", "Only one # is allowed per tab")?;
                    continue;
                }
                w.push(self.cur.tok);
            }
            pre.cols.push((u, w, tab));
            if self.cur.cmd == cmd::CAR_RET {
                break;
            }
        }
        let p = pre.encode(v);
        self.set(&format!("align:{asid}"), p, true);
        // REDUCE-GAP: tex.web opens a second align_group here, for the
        // entries and what comes between rows; the entries' groups are
        // opened per entry, and between rows the alignment's group holds.
        self.every_cr_then_peek()
    }

    /// `\everycr` (§799), then `align_peek` reads what follows it.
    fn every_cr_then_peek(&mut self) -> R<()> {
        let e = self.par(Par::everycr)?;
        let toks = toks_of(self.v, e);
        self.push_toks(toks);
        self.push_toks(vec![Tok::cs("\u{10FFFE}alignpeek")]);
        Ok(())
    }

    /// `get_preamble_token` (§782).
    fn get_preamble_token(&mut self, tab: &mut Glue) -> R<()> {
        loop {
            self.get_token()?;
            while self.cur.cmd == cmd::TAB_MARK && self.cur.chr == SPAN_CODE {
                self.get_token()?;
                if self.cur.cmd > cmd::MAX_COMMAND {
                    self.expand()?;
                    self.get_token()?;
                }
            }
            if self.cur.cmd == cmd::ASSIGN_GLUE && self.cur.chr == TABSKIP_LOC {
                self.scan_optional_equals()?;
                let g = self.scan_glue(2)?;
                let global = self.ipar(Par::globaldefs)? > 0;
                let gv = glue_value(self.v, &g);
                self.set_par(Par::tabskip, gv, global);
                *tab = g;
                continue;
            }
            return Ok(());
        }
    }

    // ------------------------------------------------------ align_peek

    /// `align_peek` (§785), in the alignment's group.
    pub(crate) fn align_peek(&mut self, md: &Mode) -> R<()> {
        let v = self.v;
        loop {
            self.set_astate(1_000_000);
            loop {
                self.get_x_token()?;
                if self.cur.cmd != cmd::SPACER {
                    break;
                }
            }
            match self.cur.cmd {
                cmd::NO_ALIGN => {
                    self.scan_left_brace()?;
                    // §785: a group sharing the alignment's list
                    self.push_group(-1, md.lid, group::NO_ALIGN, vec![v.int(md.lid)]);
                    return self.normal_paragraph();
                }
                cmd::RIGHT_BRACE => return self.fin_align(md),
                cmd::CAR_RET if self.cur.chr == CR_CR_CODE => {}
                _ => {
                    // init_row (§786), init_span, init_col
                    let pre = self.preamble(md.lid)?;
                    let omit = self.init_col(md.lid, 0)?;
                    let row = v.list(vec![nodes::encode(
                        v,
                        &tab_glue(pre.tab0),
                        &Side::default(),
                    )]);
                    let extra = vec![
                        v.str("cell"),
                        v.int(0),
                        v.int(0),
                        v.list(Vec::new()),
                        row,
                        v.list(Vec::new()),
                        v.int(i64::from(omit)),
                        v.int(md.lid),
                    ];
                    self.push_list(-102, group::ALIGN, extra);
                    return Ok(());
                }
            }
        }
    }

    /// `init_col` (§788) with `cur` the entry's first token: true if
    /// `\omit`.
    fn init_col(&mut self, asid: i64, col: usize) -> R<bool> {
        if self.cur.cmd == cmd::OMIT {
            self.set_astate(0);
            return Ok(true);
        }
        self.back_cur();
        let pre = self.preamble(asid)?;
        let mut u = pre.cols.get(col).map(|c| c.0.clone()).unwrap_or_default();
        u.push(Tok::cs("\u{10FFFE}uend"));
        self.push_toks(u);
        Ok(false)
    }

    // ------------------------------------------------- fin_col, fin_row

    /// `do_endv` (§1131): the end of an entry's v template.
    pub(crate) fn do_endv(&mut self, md: &Mode) -> R<()> {
        let v = self.v;
        let is_cell = md.group == group::ALIGN
            && md.extra.first().map(|&t| str_of(v, t)).as_deref() == Some("cell");
        if !is_cell {
            return self.log("error", "Extra alignment tab");
        }
        self.fin_col(md)
    }

    /// `fin_col` (§791–§796) and `fin_row` (§799).
    #[allow(clippy::too_many_lines)]
    fn fin_col(&mut self, md: &Mode) -> R<()> {
        let v = self.v;
        let x = |i: usize| md.extra.get(i).map_or(0, |&y| int_of(v, y));
        let l = |i: usize| md.extra.get(i).map(|&y| list_of(v, y)).unwrap_or_default();
        let (col, first, asid) = (usize::try_from(x(1)).unwrap_or(0), x(2), x(7));
        let mut prefix = l(3);
        let mut row = l(4);
        let mut adjust = l(5);
        let mut kind = self.cur.chr;
        let mut pre = self.preamble(asid)?;
        // §792
        if col + 1 >= pre.cols.len() && kind < CR_CODE {
            if pre.looping >= 0 {
                // §793: the preamble grows periodically
                let k = usize::try_from(pre.looping).unwrap_or(0);
                let c = pre.cols[k.min(pre.cols.len() - 1)].clone();
                pre.cols.push(c);
                pre.looping += 1;
                let p = pre.encode(v);
                self.set(&format!("align:{asid}"), p, true);
            } else {
                self.log("error", "Extra alignment tab has been changed to \\cr")?;
                kind = CR_CODE;
            }
        }
        let (items, _) = self.list_items(md.lid)?;
        prefix.extend(items);
        // the entry's level closes (its items read above) and the next
        // column's opens
        let next = |r: &mut Self, extra: Vec<ValueId>| -> R<()> {
            r.pop_level();
            r.push_list(-102, group::ALIGN, extra);
            Ok(())
        };
        if kind == SPAN_CODE {
            // the entry goes on into the next column
            // REDUCE-GAP: tex.web keeps the entry's save level across
            // \span; here the next column is a group of its own
            let omit = self.next_col(asid, col + 1)?;
            let extra = vec![
                v.str("cell"),
                v.int(i64::try_from(col + 1).unwrap_or(0)),
                v.int(first),
                v.list(prefix),
                v.list(row),
                v.list(adjust),
                v.int(i64::from(omit)),
                v.int(asid),
            ];
            return next(self, extra);
        }
        // §796
        let mut side = Side::default();
        let list: List = prefix
            .iter()
            .filter_map(|&y| nodes::decode(v, y, &mut side))
            .collect();
        let view = self.font_view(&list)?;
        let pp = self.pack_params(true)?;
        let span = u16::try_from(i64::try_from(col).unwrap_or(0) - first).unwrap_or(0);
        let mut rec = AlignColumn::default();
        let (u, adj) = pack_entry(list, true, span, &mut rec, &view, &pp);
        row.push(nodes::encode(v, &Node::Unset(u), &side));
        let g = pre.cols.get(col).map_or(Glue::ZERO, |c| c.2);
        row.push(nodes::encode(v, &tab_glue(g), &side));
        for a in &adj {
            adjust.push(nodes::encode(v, a, &side));
        }
        if kind >= CR_CODE {
            // §799 fin_row
            let mut s2 = Side::default();
            let rl: List = row
                .iter()
                .filter_map(|&y| nodes::decode(v, y, &mut s2))
                .collect();
            let view = self.font_view(&rl)?;
            let r = pack_row(rl, true, &view, &pp);
            let amd = self.close_group(md)?;
            let rv = nodes::encode(v, &Node::Unset(r), &s2);
            self.append_to_vlist(&amd, rv)?;
            for a in adjust {
                self.append(amd.lid, a);
            }
            return self.every_cr_then_peek();
        }
        let omit = self.next_col(asid, col + 1)?;
        let extra = vec![
            v.str("cell"),
            v.int(i64::try_from(col + 1).unwrap_or(0)),
            v.int(i64::try_from(col + 1).unwrap_or(0)),
            v.list(Vec::new()),
            v.list(row),
            v.list(adjust),
            v.int(i64::from(omit)),
            v.int(asid),
        ];
        next(self, extra)
    }

    /// §791: the next column's first token, then `init_col`.
    fn next_col(&mut self, asid: i64, col: usize) -> R<bool> {
        self.set_astate(1_000_000);
        loop {
            self.get_x_token()?;
            if self.cur.cmd != cmd::SPACER {
                break;
            }
        }
        self.init_col(asid, col)
    }

    /// The `}` of `\noalign` (§1133).
    pub(crate) fn noalign_end(&mut self, md: &Mode) -> R<()> {
        self.close_group(md)?;
        self.push_toks(vec![Tok::cs("\u{10FFFE}alignpeek")]);
        Ok(())
    }

    /// A `}` in an entry (§1132): "Missing \cr inserted".
    pub(crate) fn align_right_brace(&mut self, md: &Mode) -> R<()> {
        let v = self.v;
        if md.extra.first().map(|&t| str_of(v, t)).as_deref() == Some("cell") {
            self.back_cur();
            self.back_input(Tok::cs("\u{10FFFE}cr"));
            return self.log("error", "Missing \\cr inserted");
        }
        self.fin_align(md)
    }

    // ------------------------------------------------------- fin_align

    /// `fin_align` (§800–§812).
    pub(crate) fn fin_align(&mut self, md: &Mode) -> R<()> {
        let v = self.v;
        let x = |i: usize| md.extra.get(i).map_or(0, |&y| int_of(v, y));
        let spec = if x(1) == 0 {
            Spec::Exactly(i32::try_from(x(2)).unwrap_or(0))
        } else {
            Spec::Additional(i32::try_from(x(2)).unwrap_or(0))
        };
        let saved = x(3);
        let pre = self.preamble(md.lid)?;
        let (items, _) = self.list_items(md.lid)?;
        let mut side = Side::default();
        let list: List = items
            .iter()
            .filter_map(|&y| nodes::decode(v, y, &mut side))
            .collect();
        // §801
        let mut cols = vec![AlignColumn::default(); pre.cols.len()];
        for n in &list {
            if let Node::Unset(r) = n {
                let mut c = 0usize;
                for e in &r.list {
                    if let Node::Unset(e) = e {
                        if let Some(col) = cols.get_mut(c) {
                            col.record(e.span_count, e.width);
                        }
                        c += usize::from(e.span_count) + 1;
                    }
                }
            }
        }
        let mut tabs: Vec<Glue> = std::iter::once(pre.tab0)
            .chain(pre.cols.iter().map(|c| c.2))
            .collect();
        let widths = column_widths(&mut cols, &mut tabs);
        let view = self.font_view(&list)?;
        let pp = self.pack_params(true)?;
        let proto = prototype(&widths, &tabs, spec, true, &view, &pp).node;
        // REDUCE-GAP: alignments in displays (§1206) are not in this
        // slice; the offset is 0
        let out = set_alignment(list, &proto, true, 0, &view);
        let pd = self.prev_depth(md)?;
        let outer = self.close_group(md)?;
        let open = self.int("align:open")?;
        self.set("align:open", v.int((open - 1).max(0)), true);
        self.set_astate(saved);
        for n in &out {
            let y = nodes::encode(v, n, &side);
            self.append(outer.lid, y);
        }
        self.set_prev_depth(&outer, pd);
        // §812: `if mode=vmode then build_page`
        if outer.m == 1 {
            self.build_page_outer()?;
        }
        Ok(())
    }
}
