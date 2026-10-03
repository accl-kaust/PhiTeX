//! Part 12: Displaying boxes (§173–§198), with the mlist cases of part 34
//! (§690–§697), `print_size` (§699), the whatsit display (§1355–§1356)
//! and `print_file_name` (§518).

use alloc::vec::Vec;

use partex_engine::math::{DEFAULT_CODE, Delim, Field, Item, Kind, Limits, Noad};
use partex_engine::node::{
    BoxNode, Dims, Disc, GlueSign, GlueSpec, Leaders, Node, Order, PdfId, PdfWhatsit, Tokens,
    Unset, Whatsit,
};

use crate::pdf::objtab::Aux;

use crate::arith::Scaled;
use crate::fonts::fx;
use crate::host::Host;
use crate::nodes::*;
use crate::tex::Tex;
use crate::track::Tracker;
use crate::web::{
    AFTER, DLIST, L_CODE, PDF_TRACING_FONTS_CODE, R_CODE, SCRIPT_SIZE, SHOW_BOX_BREADTH_CODE,
    SHOW_BOX_DEPTH_CODE, TEXT_SIZE,
};

/// §138: `is_running(d)`.
pub(crate) fn is_running(d: Scaled) -> bool {
    d == partex_engine::node::RUNNING
}

/// §101: `unity`.
const UNITY: Scaled = 0o200000;

/// A `page` action's page number.
fn id_num(id: &PdfId) -> i32 {
    match id {
        PdfId::Num(n) => *n,
        PdfId::Name(_) => 0,
    }
}

impl<H: Host, T: Tracker> Tex<H, T> {
    /// pdfTeX §192: the font identifier, with pdfTeX's font details.
    pub(crate) fn print_font_identifier(&mut self, f: i32) {
        // (an expanded font by its base font's name)
        self.font_read(f, crate::track::font::EXPAND);
        self.font_read(f, crate::track::font::METRICS);
        self.print_font_id(self.fonts.base(f));
        let ratio = self.fonts.expand[fx(f)].ratio;
        if self.int_par(PDF_TRACING_FONTS_CODE) <= 0 && ratio != 0 {
            self.print_str(b" (");
            if ratio > 0 {
                self.print_char(b'+');
            }
            self.print_int(ratio);
            self.print_char(b')');
        }
        if self.int_par(PDF_TRACING_FONTS_CODE) > 0 {
            self.print_str(b" (");
            self.print(self.fonts.name[fx(f)]);
            if self.fonts.metrics[fx(f)].size != self.fonts.metrics[fx(f)].design_size {
                self.print_char(b'@');
                self.print_scaled(self.fonts.metrics[fx(f)].size);
                self.print_str(b"pt");
            }
            self.print_char(b')');
        }
    }

    /// §174: print highlights of list `list`.
    pub(crate) fn short_display(&mut self, list: &[Node]) {
        for n in list {
            match n {
                Node::Glyphs(g) => {
                    for &c in g.chars() {
                        self.short_display_char(i32::from(g.font.0), c);
                    }
                }
                // §175: print a short indication of the contents of node `p`.
                Node::Box(_)
                | Node::Ins(_)
                | Node::Whatsit(_)
                | Node::Mark(_)
                | Node::Adjust(_)
                | Node::Unset(_) => self.print_str(b"[]"),
                Node::Rule { .. } => self.print_char(b'|'),
                Node::Glue { spec, .. } => {
                    if !spec.shared_zero {
                        self.print_char(b' ');
                    }
                }
                Node::Leaders(l) => {
                    if !l.spec.shared_zero {
                        self.print_char(b' ');
                    }
                }
                Node::Math { subtype, .. } => {
                    if i32::from(*subtype) >= L_CODE {
                        self.print_str(b"[]");
                    } else {
                        self.print_char(b'$');
                    }
                }
                Node::Ligature(l) => {
                    for &c in &l.original {
                        self.short_display_char(i32::from(l.font.0), c);
                    }
                }
                Node::Disc(d) => {
                    // The replaced nodes are skipped.
                    self.short_display(&d.pre);
                    self.short_display(&d.post);
                }
                Node::Kern { .. } | Node::Penalty(_) | Node::MarginKern { .. } => {}
            }
        }
    }

    /// §174: one character of `short_display`.
    fn short_display_char(&mut self, f: i32, c: u8) {
        if f != self.font_in_short_display {
            self.print_font_identifier(f);
            self.print_char(b' ');
            self.font_in_short_display = f;
        }
        self.print(i32::from(c));
    }

    /// §176: print `char_node` data.
    pub(crate) fn print_font_and_char(&mut self, f: i32, c: u8) {
        self.print_font_identifier(f);
        self.print_char(b' ');
        self.print(i32::from(c));
    }

    /// §176: print token list data in braces.
    pub(crate) fn print_mark(&mut self, toks: &Tokens) {
        self.print_char(b'{');
        let l = self.params.max_print_line - 10;
        self.show_token_slice(toks, l);
        self.print_char(b'}');
    }

    /// §176: print a dimension of a rule node.
    pub(crate) fn print_rule_dimen(&mut self, d: Scaled) {
        if is_running(d) {
            self.print_char(b'*');
        } else {
            self.print_scaled(d);
        }
    }

    /// §177: print a glue component; `s` is the unit (`b""` for none).
    pub(crate) fn print_glue(&mut self, d: Scaled, order: Order, s: &[u8]) {
        self.print_scaled(d);
        if order > Order::Normal {
            self.print_str(b"fil");
            for _ in 1..order.index() {
                self.print_char(b'l');
            }
        } else if !s.is_empty() {
            self.print_str(s);
        }
    }

    /// §178: print a glue specification; `s` is the unit (`b""` for none).
    pub(crate) fn print_spec(&mut self, g: &GlueSpec, s: &[u8]) {
        self.print_scaled(g.width);
        if !s.is_empty() {
            self.print_str(s);
        }
        if g.stretch != 0 {
            self.print_str(b" plus ");
            self.print_glue(g.stretch, g.stretch_order, s);
        }
        if g.shrink != 0 {
            self.print_str(b" minus ");
            self.print_glue(g.shrink, g.shrink_order, s);
        }
    }

    /// §180: `node_list_display`: show a sublist one level deeper.
    fn node_list_display(&mut self, list: &[Node], c: u8) {
        self.append_char(c);
        self.show_node_list(list);
        self.flush_char();
    }

    /// Is the display too deep to show a sublist (§182)? Prints " []" for
    /// a nonempty one.
    fn too_deep(&mut self, nonempty: bool) -> bool {
        if i32::try_from(self.cur_length()).unwrap_or(i32::MAX) > self.depth_threshold {
            if nonempty {
                self.print_str(b" []"); // indicate that there's been some truncation
            }
            return true;
        }
        false
    }

    /// §182: start the display of one node; false once `\showboxbreadth`
    /// nodes have been shown.
    fn next_display_line(&mut self, n: &mut i32) -> bool {
        self.print_ln();
        self.print_current_string(); // display the nesting history
        *n += 1;
        if *n > self.breadth_max {
            self.print_str(b"etc.");
            return false;
        }
        true
    }

    /// §182: print a node list symbolically.
    pub(crate) fn show_node_list(&mut self, list: &[Node]) {
        if self.too_deep(!list.is_empty()) {
            return;
        }
        let mut n = 0;
        let _ = self.show_nodes(list, &mut n);
    }

    /// The loop of §182 over `list`; `Err` once "etc." was printed.
    fn show_nodes(&mut self, list: &[Node], n: &mut i32) -> Result<(), ()> {
        for p in list {
            match p {
                // One line per character, as tex.web has one node each.
                Node::Glyphs(g) => {
                    for &c in g.chars() {
                        if !self.next_display_line(n) {
                            return Err(());
                        }
                        self.print_font_and_char(i32::from(g.font.0), c);
                    }
                }
                Node::Disc(d) => {
                    if !self.next_display_line(n) {
                        return Err(());
                    }
                    self.display_disc(d);
                    // tex.web keeps the replaced nodes after the
                    // discretionary.
                    self.show_nodes(&d.replace, n)?;
                }
                _ => {
                    if !self.next_display_line(n) {
                        return Err(());
                    }
                    self.display_node(p);
                }
            }
        }
        Ok(())
    }

    /// §195: display a discretionary (without its replaced nodes).
    fn display_disc(&mut self, d: &Disc) {
        self.print_esc(b"discretionary");
        let r = tex_len(&d.replace);
        if r > 0 {
            self.print_str(b" replacing ");
            self.print_int(r);
        }
        self.node_list_display(&d.pre, b'.');
        self.node_list_display(&d.post, b'|');
    }

    /// §183: display node `p`.
    fn display_node(&mut self, p: &Node) {
        match p {
            Node::Glyphs(_) | Node::Disc(_) => {} // shown by `show_nodes`
            Node::Box(b) => self.display_box(b),
            Node::Unset(u) => self.display_unset(u),
            Node::Rule {
                width,
                height,
                depth,
                ..
            } => {
                // §187
                self.print_esc(b"rule(");
                self.print_rule_dimen(*height);
                self.print_char(b'+');
                self.print_rule_dimen(*depth);
                self.print_str(b")x");
                self.print_rule_dimen(*width);
            }
            Node::Ins(i) => {
                // §188
                self.print_esc(b"insert");
                self.print_int(i32::from(i.number));
                self.print_str(b", natural size ");
                self.print_scaled(i.height);
                self.print_str(b"; split(");
                self.print_spec(&i.split_top, b"");
                self.print_char(b',');
                self.print_scaled(i.split_max_depth);
                self.print_str(b"); float cost ");
                self.print_int(i.float_cost);
                self.node_list_display(&i.list, b'.');
            }
            Node::Whatsit(w) => self.display_whatsit(w),
            Node::Glue { spec, subtype, .. } => self.display_glue(spec, *subtype),
            Node::Leaders(l) => {
                // §190: display leaders `p`.
                self.print_esc(b"");
                match l.kind {
                    Leaders::Centered => self.print_char(b'c'),
                    Leaders::Expanded => self.print_char(b'x'),
                    Leaders::Aligned => {}
                }
                self.print_str(b"leaders ");
                self.print_spec(&l.spec, b"");
                self.node_list_display(core::slice::from_ref(&l.leader), b'.');
            }
            Node::MarginKern { width, left, .. } => {
                // pdfTeX's margin kern
                self.print_esc(b"kern");
                self.print_scaled(*width);
                self.print_str(if *left {
                    b" (left margin)"
                } else {
                    b" (right margin)"
                });
            }
            Node::Kern { width, subtype, .. } => {
                // §191
                let subtype = i32::from(*subtype);
                if subtype == MU_GLUE {
                    self.print_esc(b"mkern");
                    self.print_scaled(*width);
                    self.print_str(b"mu");
                } else {
                    self.print_esc(b"kern");
                    if subtype != NORMAL {
                        self.print_char(b' ');
                    }
                    self.print_scaled(*width);
                    if subtype == ACC_KERN {
                        self.print_str(b" (for accent)");
                    }
                }
            }
            Node::Math { width, subtype, .. } if i32::from(*subtype) > AFTER => {
                // pdfTeX §192: an LR node.
                self.print_esc(if subtype % 2 == 1 { b"end" } else { b"begin" });
                self.print_char(if i32::from(*subtype) > R_CODE {
                    b'R'
                } else if i32::from(*subtype) > L_CODE {
                    b'L'
                } else {
                    b'M'
                });
                let _ = width;
            }
            Node::Math { width, subtype, .. } => {
                // §192
                self.print_esc(b"math");
                if i32::from(*subtype) == BEFORE {
                    self.print_str(b"on");
                } else {
                    self.print_str(b"off");
                }
                if *width != 0 {
                    self.print_str(b", surrounded ");
                    self.print_scaled(*width);
                }
            }
            Node::Ligature(l) => {
                // §193
                let f = i32::from(l.font.0);
                self.print_font_and_char(f, l.ch);
                self.print_str(b" (ligature ");
                if l.subtype > 1 {
                    self.print_char(b'|');
                }
                self.font_in_short_display = f;
                for &c in &l.original {
                    self.short_display_char(f, c);
                }
                if l.subtype % 2 == 1 {
                    self.print_char(b'|');
                }
                self.print_char(b')');
            }
            Node::Penalty(n) => {
                // §194
                self.print_esc(b"penalty ");
                self.print_int(*n);
            }
            Node::Mark(m) => {
                // §196
                self.print_esc(b"mark");
                if m.class != 0 {
                    self.print_char(b's');
                    self.print_int(m.class);
                }
                self.print_mark(&m.tokens);
            }
            Node::Adjust(a) => {
                // §197
                self.print_esc(b"vadjust");
                if a.pre {
                    self.print_str(b" pre ");
                }
                self.node_list_display(&a.list, b'.');
            }
        }
    }

    /// §184: display box `b`.
    fn display_box(&mut self, b: &BoxNode) {
        // (a sealed line is shown with its contents, which it reads)
        let open = self.unsealed_box(b);
        let b = open.as_ref().unwrap_or(b);
        self.print_esc(if b.vertical { b"v" } else { b"h" });
        self.print_str(b"box(");
        self.print_scaled(b.height);
        self.print_char(b'+');
        self.print_scaled(b.depth);
        self.print_str(b")x");
        self.print_scaled(b.width);
        // §186: display the value of `glue_set(p)`.
        let g = b.glue_set;
        if g != 0.0 && b.glue_sign != GlueSign::Normal {
            self.print_str(b", glue set ");
            if b.glue_sign == GlueSign::Shrinking {
                self.print_str(b"- ");
            }
            if g.abs() > 20000.0 {
                if g > 0.0 {
                    self.print_char(b'>');
                } else {
                    self.print_str(b"< -");
                }
                self.print_glue(20000 * UNITY, b.glue_order, b"");
            } else {
                let r = crate::arith::zround(f64::from(UNITY) * g);
                self.print_glue(r, b.glue_order, b"");
            }
        }
        if b.shift != 0 {
            self.print_str(b", shifted ");
            self.print_scaled(b.shift);
        }
        if self.etex_ex() && !b.vertical && i32::from(b.subtype) == DLIST {
            // pdfTeX §1704: display if this box is never to be reversed.
            self.print_str(b", display");
        }
        self.node_list_display(&b.list, b'.');
    }

    /// §184–§185: display unset node `u`.
    fn display_unset(&mut self, u: &Unset) {
        self.print_esc(b"unset");
        self.print_str(b"box(");
        self.print_scaled(u.height);
        self.print_char(b'+');
        self.print_scaled(u.depth);
        self.print_str(b")x");
        self.print_scaled(u.width);
        if u.span_count != 0 {
            self.print_str(b" (");
            self.print_int(i32::from(u.span_count) + 1);
            self.print_str(b" columns)");
        }
        if u.stretch != 0 {
            self.print_str(b", stretch ");
            self.print_glue(u.stretch, u.stretch_order, b"");
        }
        if u.shrink != 0 {
            self.print_str(b", shrink ");
            self.print_glue(u.shrink, u.shrink_order, b"");
        }
        self.node_list_display(&u.list, b'.');
    }

    /// §189: display glue.
    fn display_glue(&mut self, spec: &GlueSpec, subtype: u8) {
        let subtype = i32::from(subtype);
        self.print_esc(b"glue");
        if subtype != NORMAL {
            self.print_char(b'(');
            if subtype < COND_MATH_GLUE {
                self.print_skip_param(subtype - 1);
            } else if subtype == COND_MATH_GLUE {
                self.print_esc(b"nonscript");
            } else {
                self.print_esc(b"mskip");
            }
            self.print_char(b')');
        }
        if subtype != COND_MATH_GLUE {
            self.print_char(b' ');
            if subtype < COND_MATH_GLUE {
                self.print_spec(spec, b"");
            } else {
                self.print_spec(spec, b"mu");
            }
        }
    }

    /// §198: display a box (or list) with `\showboxdepth` and
    /// `\showboxbreadth`.
    pub(crate) fn show_box(&mut self, list: &[Node]) {
        self.set_show_box_limits();
        self.show_node_list(list); // the show starts at `p`
        self.print_ln();
    }

    /// §198: `show_box` for one box.
    pub(crate) fn show_box_node(&mut self, b: &BoxNode) {
        self.set_show_box_limits();
        if !self.too_deep(true) {
            let mut n = 0;
            if self.next_display_line(&mut n) {
                self.display_box(b);
            }
        }
        self.print_ln();
    }

    /// §198: `show_box` for an mlist.
    pub(crate) fn show_box_mlist(&mut self, list: &[Item]) {
        self.set_show_box_limits();
        self.show_mlist(list);
        self.print_ln();
    }

    /// §236: assign `depth_threshold` and `breadth_max`.
    fn set_show_box_limits(&mut self) {
        self.depth_threshold = self.int_par(SHOW_BOX_DEPTH_CODE);
        self.breadth_max = self.int_par(SHOW_BOX_BREADTH_CODE);
        if self.breadth_max <= 0 {
            self.breadth_max = 5;
        }
        let pool_ptr = i32::try_from(self.pool_ptr).unwrap_or(i32::MAX);
        let pool_size = i32::try_from(self.pool_size()).unwrap_or(i32::MAX);
        if pool_ptr.saturating_add(self.depth_threshold) >= pool_size {
            self.depth_threshold = pool_size - pool_ptr - 1;
        } // now there's enough room for prefix string
    }

    /// §182 for an mlist: noads and nodes.
    pub(crate) fn show_mlist(&mut self, list: &[Item]) {
        if self.too_deep(!list.is_empty()) {
            return;
        }
        let mut n = 0;
        for p in list {
            if let Item::Node(node) = p {
                if self
                    .show_nodes(core::slice::from_ref(node), &mut n)
                    .is_err()
                {
                    return;
                }
                continue;
            }
            if !self.next_display_line(&mut n) {
                return;
            }
            match p {
                Item::Node(_) => {}
                // §690: cases that arise in mlists only.
                Item::Style(s) => self.print_style(i32::from(*s)),
                Item::Choice(c) => {
                    // §695
                    self.print_esc(b"mathchoice");
                    for (l, ch) in c.iter().zip(*b"DTSs") {
                        self.append_char(ch);
                        self.show_mlist(l);
                        self.flush_char();
                    }
                }
                Item::Noad(q) => {
                    if let Kind::Fraction { .. } = q.kind {
                        self.display_fraction(q);
                    } else {
                        self.display_noad(q);
                    }
                }
            }
        }
    }

    /// §691: print family and character.
    pub(crate) fn print_fam_and_char(&mut self, fam: u8, ch: u8) {
        self.print_esc(b"fam");
        self.print_int(i32::from(fam));
        self.print_char(b' ');
        self.print(i32::from(ch));
    }

    /// §691: print a delimiter as a 24-bit hex value.
    pub(crate) fn print_delimiter(&mut self, d: Delim) {
        let a = i32::from(d.small_fam) * 256 + i32::from(d.small_char);
        let a = a * 0x1000 + i32::from(d.large_fam) * 256 + i32::from(d.large_char);
        self.print_hex(a);
    }

    /// §692: display a noad field.
    pub(crate) fn print_subsidiary_data(&mut self, f: &Field, c: u8) {
        if i32::try_from(self.cur_length()).unwrap_or(i32::MAX) >= self.depth_threshold {
            if *f != Field::Empty {
                self.print_str(b" []");
            }
        } else {
            self.append_char(c); // include `c` in the recursion history
            match f {
                Field::Char { fam, ch } => {
                    self.print_ln();
                    self.print_current_string();
                    self.print_fam_and_char(*fam, *ch);
                }
                Field::Box(b) => self.show_node_list(&[Node::Box(b.clone())]),
                Field::Mlist(l) if l.is_empty() => {
                    self.print_ln();
                    self.print_current_string();
                    self.print_str(b"{}");
                }
                Field::Mlist(l) => self.show_mlist(l),
                Field::Empty | Field::TextChar { .. } => {}
            }
            self.flush_char(); // remove `c` from the recursion history
        }
    }

    /// §694
    pub(crate) fn print_style(&mut self, c: i32) {
        match c / 2 {
            0 => self.print_esc(b"displaystyle"),
            1 => self.print_esc(b"textstyle"),
            2 => self.print_esc(b"scriptstyle"),
            3 => self.print_esc(b"scriptscriptstyle"),
            _ => self.print_str(b"Unknown style!"),
        }
    }

    /// §696: display normal noad `p`.
    fn display_noad(&mut self, p: &Noad) {
        match &p.kind {
            Kind::Ord => self.print_esc(b"mathord"),
            Kind::Op(_) => self.print_esc(b"mathop"),
            Kind::Bin => self.print_esc(b"mathbin"),
            Kind::Rel => self.print_esc(b"mathrel"),
            Kind::Open => self.print_esc(b"mathopen"),
            Kind::Close => self.print_esc(b"mathclose"),
            Kind::Punct => self.print_esc(b"mathpunct"),
            Kind::Inner => self.print_esc(b"mathinner"),
            Kind::Over => self.print_esc(b"overline"),
            Kind::Under => self.print_esc(b"underline"),
            Kind::Vcenter => self.print_esc(b"vcenter"),
            Kind::Radical(d) => {
                self.print_esc(b"radical");
                self.print_delimiter(*d);
            }
            Kind::Accent { fam, ch } => {
                self.print_esc(b"accent");
                self.print_fam_and_char(*fam, *ch);
            }
            Kind::Left(d) => {
                self.print_esc(b"left");
                self.print_delimiter(*d);
            }
            Kind::Right(d) => {
                self.print_esc(b"right");
                self.print_delimiter(*d);
            }
            Kind::Middle(d) => {
                self.print_esc(b"middle");
                self.print_delimiter(*d);
            }
            Kind::Fraction { .. } => {}
        }
        match p.kind {
            Kind::Op(Limits::Limits) => self.print_esc(b"limits"),
            Kind::Op(Limits::NoLimits) => self.print_esc(b"nolimits"),
            _ => {}
        }
        if !matches!(p.kind, Kind::Left(_) | Kind::Right(_) | Kind::Middle(_)) {
            self.print_subsidiary_data(&p.nucleus, b'.');
        }
        self.print_subsidiary_data(&p.sup, b'^');
        self.print_subsidiary_data(&p.sub, b'_');
    }

    /// §697: display fraction noad `p`.
    fn display_fraction(&mut self, p: &Noad) {
        let Kind::Fraction {
            thickness,
            left,
            right,
        } = &p.kind
        else {
            return;
        };
        self.print_esc(b"fraction, thickness ");
        if *thickness == DEFAULT_CODE {
            self.print_str(b"= default");
        } else {
            self.print_scaled(*thickness);
        }
        if *left != Delim::default() {
            self.print_str(b", left-delimiter ");
            self.print_delimiter(*left);
        }
        if *right != Delim::default() {
            self.print_str(b", right-delimiter ");
            self.print_delimiter(*right);
        }
        self.print_subsidiary_data(&p.sup, b'\\');
        self.print_subsidiary_data(&p.sub, b'/');
    }

    /// §699
    pub(crate) fn print_size(&mut self, s: i32) {
        if s == TEXT_SIZE {
            self.print_esc(b"textfont");
        } else if s == SCRIPT_SIZE {
            self.print_esc(b"scriptfont");
        } else {
            self.print_esc(b"scriptscriptfont");
        }
    }

    /// §1355
    fn print_write_whatsit(&mut self, s: &[u8], stream: i32) {
        self.print_esc(s);
        if stream < 16 {
            self.print_int(stream);
        } else if stream == 16 {
            self.print_char(b'*');
        } else {
            self.print_char(b'-');
        }
    }

    /// §1356: display a whatsit node. (encTeX's `\mubyteout` marks are not
    /// kept.)
    fn display_whatsit(&mut self, w: &Whatsit) {
        match w {
            Whatsit::Open {
                stream,
                name,
                area,
                ext,
            } => {
                self.print_write_whatsit(b"openout", *stream);
                self.print_char(b'=');
                self.print_file_name_bytes(name, area, ext);
            }
            Whatsit::Write { stream, tokens } => {
                self.print_write_whatsit(b"write", *stream);
                self.print_mark(tokens);
            }
            Whatsit::Close { stream } => self.print_write_whatsit(b"closeout", *stream),
            Whatsit::Special { tokens } => {
                self.print_esc(b"special");
                self.print_mark(tokens);
            }
            Whatsit::Language {
                language,
                left_hyphen_min,
                right_hyphen_min,
            } => {
                self.print_esc(b"setlanguage");
                self.print_int(*language);
                self.print_str(b" (hyphenmin ");
                self.print_int(i32::from(*left_hyphen_min));
                self.print_char(b',');
                self.print_int(i32::from(*right_hyphen_min));
                self.print_char(b')');
            }
            Whatsit::LateSpecial { tokens } => {
                self.print_esc(b"special");
                self.print_str(b" shipout");
                self.print_mark(tokens);
            }
            Whatsit::Pdf(p) => self.display_pdf_whatsit(p),
        }
    }

    /// pdfTeX §1603: a `<rule spec>` of a whatsit.
    fn print_dims(&mut self, d: &Dims) {
        self.print_str(b"(");
        self.print_rule_dimen(d.height);
        self.print_char(b'+');
        self.print_rule_dimen(d.depth);
        self.print_str(b")x");
        self.print_rule_dimen(d.width);
    }

    fn print_pdf_id(&mut self, id: &PdfId) {
        match id {
            PdfId::Name(t) => {
                self.print_str(b" name");
                self.print_mark(t);
            }
            PdfId::Num(n) => {
                self.print_str(b" num");
                self.print_int(*n);
            }
        }
    }

    /// pdfTeX §1603: display a PDF whatsit.
    fn display_pdf_whatsit(&mut self, p: &PdfWhatsit) {
        match p {
            PdfWhatsit::Literal { late, mode, data } => {
                self.print_esc(b"pdfliteral");
                if *late {
                    self.print_str(b" shipout");
                }
                match mode {
                    1 => self.print_str(b" page"),
                    2 => self.print_str(b" direct"),
                    _ => {}
                }
                self.print_mark(data);
            }
            PdfWhatsit::ColorStack { stack, cmd, data } => {
                self.print_esc(b"pdfcolorstack ");
                self.print_int(*stack);
                self.print_str(match cmd {
                    0 => b" set ".as_slice(),
                    1 => b" push ",
                    2 => b" pop",
                    _ => b" current",
                });
                if let Some(d) = data {
                    self.print_mark(d);
                }
            }
            PdfWhatsit::SetMatrix { data } => {
                self.print_esc(b"pdfsetmatrix");
                self.print_mark(data);
            }
            PdfWhatsit::Save => self.print_esc(b"pdfsave"),
            PdfWhatsit::Restore => self.print_esc(b"pdfrestore"),
            PdfWhatsit::RefObj { objnum } => {
                self.print_esc(b"pdfrefobj");
                if let Aux::Obj(o) = self.pdf_obj_aux(*objnum) {
                    let o = o.clone();
                    if o.is_stream {
                        if let Some(a) = &o.stream_attr {
                            self.print_str(b" attr");
                            self.print_mark(a);
                        }
                        self.print_str(b" stream");
                    }
                    if o.is_file {
                        self.print_str(b" file");
                    }
                    self.print_mark(&o.data);
                }
            }
            PdfWhatsit::RefXForm { objnum, .. } | PdfWhatsit::RefXImage { objnum, .. } => {
                let (name, (w, h, d)) = match p {
                    PdfWhatsit::RefXForm { .. } => {
                        (b"pdfrefxform".as_slice(), self.xform_dims(*objnum))
                    }
                    _ => (b"pdfrefximage".as_slice(), self.ximage_dims(*objnum)),
                };
                self.print_esc(name);
                self.print_str(b"(");
                self.print_scaled(h);
                self.print_char(b'+');
                self.print_scaled(d);
                self.print_str(b")x");
                self.print_scaled(w);
            }
            PdfWhatsit::Annot { dims, data, .. } => {
                self.print_esc(b"pdfannot");
                self.print_dims(dims);
                self.print_mark(data);
            }
            PdfWhatsit::StartLink {
                dims, attr, action, ..
            } => {
                self.print_esc(b"pdfstartlink");
                self.print_dims(dims);
                if let Some(a) = attr {
                    self.print_str(b" attr");
                    self.print_mark(a);
                }
                self.print_str(b" action");
                if action.kind == 3 {
                    self.print_str(b" user");
                    if let Some(t) = &action.tokens {
                        self.print_mark(t);
                    }
                } else {
                    if let Some(f) = &action.file {
                        self.print_str(b" file");
                        self.print_mark(f);
                    }
                    match (action.kind, &action.id) {
                        (0, id) => {
                            self.print_str(b" page");
                            self.print_int(id_num(id));
                            if let Some(t) = &action.tokens {
                                self.print_mark(t);
                            }
                        }
                        (1, PdfId::Name(t)) => {
                            self.print_str(b" goto name");
                            self.print_mark(t);
                        }
                        (1, PdfId::Num(n)) => {
                            self.print_str(b" goto num");
                            self.print_int(*n);
                        }
                        (_, PdfId::Name(t)) => {
                            self.print_str(b" thread name");
                            self.print_mark(t);
                        }
                        (_, PdfId::Num(n)) => {
                            self.print_str(b" thread num");
                            self.print_int(*n);
                        }
                    }
                }
            }
            PdfWhatsit::EndLink => self.print_esc(b"pdfendlink"),
            PdfWhatsit::Dest {
                dims,
                struct_num,
                id,
                kind,
                zoom,
            } => {
                self.print_esc(b"pdfdest");
                if let Some(n) = struct_num {
                    self.print_str(b" struct");
                    self.print_int(*n);
                }
                self.print_pdf_id(id);
                self.print_str(b" ");
                match kind {
                    0 => {
                        self.print_str(b"xyz");
                        if let Some(z) = zoom {
                            self.print_str(b" zoom");
                            self.print_int(*z);
                        }
                    }
                    5 => self.print_str(b"fitbh"),
                    6 => self.print_str(b"fitbv"),
                    4 => self.print_str(b"fitb"),
                    2 => self.print_str(b"fith"),
                    3 => self.print_str(b"fitv"),
                    7 => {
                        self.print_str(b"fitr");
                        self.print_dims(dims);
                    }
                    1 => self.print_str(b"fit"),
                    _ => self.print_str(b"unknown!"),
                }
            }
            PdfWhatsit::Thread {
                start,
                dims,
                attr,
                id,
            } => {
                self.print_esc(if *start {
                    b"pdfstartthread".as_slice()
                } else {
                    b"pdfthread"
                });
                self.print_dims(dims);
                if let Some(a) = attr {
                    self.print_str(b" attr");
                    self.print_mark(a);
                }
                self.print_pdf_id(id);
            }
            PdfWhatsit::EndThread => self.print_esc(b"pdfendthread"),
            PdfWhatsit::SavePos => self.print_esc(b"pdfsavepos"),
            PdfWhatsit::SnapRefPoint => self.print_esc(b"pdfsnaprefpoint"),
            PdfWhatsit::SnapY { glue, final_skip } => {
                self.print_esc(b"pdfsnapy");
                self.print_char(b' ');
                self.print_spec(glue, b"");
                self.print_char(b' ');
                // (`print_spec(final_skip(p), 0)`: a scaled taken for a
                // pointer; it is 0, `zero_glue`, until the node is shipped)
                let _ = final_skip;
                self.print_spec(&GlueSpec::default(), b"");
            }
            PdfWhatsit::SnapYComp { ratio } => {
                self.print_esc(b"pdfsnapycomp");
                self.print_char(b' ');
                self.print_int(*ratio);
            }
            PdfWhatsit::InterwordSpaceOn => self.print_esc(b"pdfinterwordspaceon"),
            PdfWhatsit::InterwordSpaceOff => self.print_esc(b"pdfinterwordspaceoff"),
            PdfWhatsit::FakeSpace => self.print_esc(b"pdffakespace"),
            PdfWhatsit::RunningLinkOff => self.print_esc(b"pdfrunninglinkoff"),
            PdfWhatsit::RunningLinkOn => self.print_esc(b"pdfrunninglinkon"),
        }
    }

    /// §518 (web2c): print a file name given by string numbers.
    pub(crate) fn print_file_name(&mut self, n: i32, a: i32, e: i32) {
        let bytes = |t: &Self, s: i32| {
            if s == 0 {
                Vec::new()
            } else {
                t.str_bytes(sx(s)).to_vec()
            }
        };
        let (n, a, e) = (bytes(self, n), bytes(self, a), bytes(self, e));
        self.print_file_name_bytes(&n, &a, &e);
    }

    /// §518 (web2c): print a file name, quoted if any part has a space.
    pub(crate) fn print_file_name_bytes(&mut self, n: &[u8], a: &[u8], e: &[u8]) {
        let must_quote = [a, n, e].iter().any(|s| s.contains(&b' '));
        if must_quote {
            self.print_char(b'"');
        }
        for s in [a, n, e] {
            for &c in s {
                if c != b'"' {
                    self.print(i32::from(c));
                }
            }
        }
        if must_quote {
            self.print_char(b'"');
        }
    }
}

/// The number of tex.web nodes in `list` (one per character).
pub(crate) fn tex_len(list: &[Node]) -> i32 {
    list.iter()
        .map(|n| match n {
            Node::Glyphs(g) => i32::try_from(g.chars().len()).unwrap_or(0),
            _ => 1,
        })
        .sum()
}

fn sx(s: i32) -> usize {
    usize::try_from(s).expect("negative string number")
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use partex_engine::node::{BoxNode, GlueSign, GlueSpec, Mark, Node, Order, RUNNING};

    use crate::testing::{engine, term_output};
    use crate::web::{LETTER, MAC_PARAM, SHOW_BOX_BREADTH_CODE, SHOW_BOX_DEPTH_CODE};

    /// Oracle: `\setbox1\hbox{\kern3pt\penalty5\vrule width 2pt
    /// \hskip 1pt plus 2fil minus 3pt\kern-1pt}` inside
    /// `\vbox to 20pt{\box1\vskip 4pt plus 1fill\mark{a#b}}`, `\showbox`.
    #[test]
    fn show_box_matches_tex() {
        let mut t = engine();
        t.set_int_par(SHOW_BOX_DEPTH_CODE, 10);
        t.set_int_par(SHOW_BOX_BREADTH_CODE, 10);
        let u = 0o200000; // unity
        let h = BoxNode {
            width: 5 * u,
            list: vec![
                Node::Kern {
                    width: 3 * u,
                    subtype: 1,
                    sync: partex_engine::origin::Side(0),
                },
                Node::Penalty(5),
                Node::Rule {
                    width: 2 * u,
                    height: RUNNING,
                    depth: RUNNING,
                    sync: partex_engine::origin::Side(0),
                },
                Node::Glue {
                    spec: GlueSpec {
                        width: u,
                        stretch: 2 * u,
                        stretch_order: Order::Fil,
                        shrink: 3 * u,
                        ..GlueSpec::default()
                    },
                    subtype: 0,
                    sync: partex_engine::origin::Side(0),
                },
                Node::Kern {
                    width: -u,
                    subtype: 1,
                    sync: partex_engine::origin::Side(0),
                },
            ],
            ..BoxNode::default()
        };
        let v = BoxNode {
            vertical: true,
            height: 20 * u,
            width: 5 * u,
            glue_sign: GlueSign::Stretching,
            glue_order: Order::Fill,
            glue_set: 16.0,
            list: vec![
                Node::Box(h.share()),
                Node::Glue {
                    spec: GlueSpec {
                        width: 4 * u,
                        stretch: u,
                        stretch_order: Order::Fill,
                        ..GlueSpec::default()
                    },
                    subtype: 0,
                    sync: partex_engine::origin::Side(0),
                },
                Node::Mark(alloc::boxed::Box::new(Mark {
                    class: 0,
                    tokens: partex_engine::node::TokenList::shared(&[
                        LETTER * 256 + 97,
                        MAC_PARAM * 256 + 35,
                        LETTER * 256 + 98,
                    ]),
                })),
            ],
            ..BoxNode::default()
        };
        let out = term_output(&mut t, |t| t.show_box(&[Node::Box(v.share())]));
        assert_eq!(
            core::str::from_utf8(&out).unwrap(),
            "\n\\vbox(20.0+0.0)x5.0, glue set 16.0fill\n\
             .\\hbox(0.0+0.0)x5.0\n\
             ..\\kern 3.0\n\
             ..\\penalty 5\n\
             ..\\rule(*+*)x2.0\n\
             ..\\glue 1.0 plus 2.0fil minus 3.0\n\
             ..\\kern -1.0\n\
             .\\glue 4.0 plus 1.0fill\n\
             .\\mark{a##b}\n"
        );
    }
}
