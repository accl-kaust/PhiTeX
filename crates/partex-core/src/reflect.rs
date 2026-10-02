//! e-TeX's `TeXXeT` at shipout: hlists in visual order.
//!
//! Right-to-left segments of an hlist, and every hlist met while
//! shipping out right-to-left text, are emitted reversed; an
//! [`Item::Edge`] marks each change of direction so the DVI writer can
//! place boxes and leaders. The box itself is never touched: the walk
//! keeps a stack of pending entries, and reversing a segment replaces its
//! entries by their reflection.

use alloc::vec;
use alloc::vec::Vec;

use crate::arith::Scaled;
use crate::dvi::SetGlue;
use crate::host::Host;
use crate::pageir::{Item, Page};
use crate::tex::{Jump, Tex};
use crate::track::Tracker;
use partex_engine::lr::{BEFORE, L_CODE, begin_of, end_of, is_end, is_rtl};
use partex_engine::node::{BoxNode, LeaderNode, Node};

/// An entry of an hlist being shipped out.
#[derive(Clone, Copy)]
enum Ent<'a> {
    Node(&'a Node),
    Char(i32, u8),
    /// A math node (its subtype flipped inside a reflected segment).
    Math {
        width: Scaled,
        subtype: u8,
    },
    /// Glue or a kern of settled width.
    Move(Scaled),
    /// Leaders of settled width.
    Leaders(&'a LeaderNode, Scaled),
    Edge {
        width: Scaled,
        dist: Scaled,
        rtl: bool,
    },
}

/// The state of one hlist: its LR stack (of closing subtypes, above a
/// `before` that matches nothing) and positions relative to its start.
struct Walk {
    stack: Vec<u8>,
    h: Scaled,
    left_edge: Scaled,
}

impl Walk {
    fn top(&self) -> u8 {
        *self.stack.last().expect("LR stack")
    }
}

/// Does `list` need the LR walk in left-to-right text?
fn has_lr(list: &[Node]) -> bool {
    list.iter()
        .any(|n| matches!(n, Node::Math { subtype, .. } if *subtype >= L_CODE))
}

/// `list` as entries, discretionaries replaced by their replacement text.
fn flatten<'a>(list: &'a [Node], out: &mut Vec<Ent<'a>>) {
    for n in list {
        match n {
            Node::Math { width, subtype } => out.push(Ent::Math {
                width: *width,
                subtype: *subtype,
            }),
            Node::Disc(d) => flatten(&d.replace, out),
            _ => out.push(Ent::Node(n)),
        }
    }
}

impl<H: Host, T: Tracker> Tex<H, T> {
    /// pdfTeX §1705–§1713: the items of hlist `this_box` in extended
    /// mode; `display` for a display line, never reflected.
    pub(crate) fn build_hlist_lr(
        &mut self,
        this_box: &BoxNode,
        display: bool,
        glue: &mut SetGlue,
        page: &mut Page,
        leaders: bool,
    ) -> Result<(), Jump> {
        let display_in_rtl = display && self.out_rtl;
        if display_in_rtl {
            self.out_rtl = false;
        }
        let walked = if !self.out_rtl && !has_lr(&this_box.list) {
            self.build_items(this_box, &this_box.list, glue, page, leaders)
        } else {
            self.lr_items(this_box, glue, page, leaders)
        };
        if display_in_rtl {
            self.out_rtl = true;
        }
        walked
    }

    fn lr_items(
        &mut self,
        this_box: &BoxNode,
        glue: &mut SetGlue,
        page: &mut Page,
        leaders: bool,
    ) -> Result<(), Jump> {
        // The pending entries, the next one last.
        let mut rest = Vec::with_capacity(this_box.list.len());
        flatten(&this_box.list, &mut rest);
        rest.reverse();
        let mut w = Walk {
            stack: vec![BEFORE],
            h: 0,
            left_edge: 0,
        };
        if self.out_rtl {
            // Reverse the complete hlist, starting from its right end.
            let (l, _) = self.reverse(this_box, &mut rest, false, &mut w, glue)?;
            rest = l;
            rest.push(Ent::Move(-w.h));
            w.h = 0;
        }
        while let Some(e) = rest.pop() {
            w.h += match e {
                Ent::Node(n) => self.node_item(this_box, n, glue, page, leaders)?,
                Ent::Char(f, c) => self.char_item(f, i32::from(c), page),
                Ent::Move(d) => {
                    page.items.push(Item::Move(d));
                    d
                }
                Ent::Leaders(l, wd) => {
                    self.leaders_items(this_box, l, wd, page)?;
                    wd
                }
                Ent::Edge { width, dist, rtl } => {
                    page.items.push(Item::Edge { width, dist, rtl });
                    w.left_edge = w.h + width + dist;
                    self.out_rtl = rtl;
                    width
                }
                Ent::Math { width, subtype } => {
                    if is_end(subtype) {
                        if w.top() == end_of(subtype) {
                            w.stack.pop();
                        } else if subtype > L_CODE {
                            self.lr_problems += 1;
                        }
                    } else {
                        w.stack.push(end_of(subtype));
                        if is_rtl(subtype) != self.out_rtl {
                            // Reverse the segment this opens: an edge,
                            // its reflection, an edge back.
                            self.out_rtl = !self.out_rtl;
                            let save_h = w.h;
                            w.h = w.h - w.left_edge + width;
                            let (l, tail) =
                                self.reverse(this_box, &mut rest, true, &mut w, glue)?;
                            let (tw, td) = tail.expect("reflected segment end");
                            rest.push(Ent::Edge {
                                width: tw,
                                dist: td,
                                rtl: !self.out_rtl,
                            });
                            rest.extend(l);
                            rest.push(Ent::Edge {
                                width,
                                dist: w.h,
                                rtl: self.out_rtl,
                            });
                            w.h = save_h;
                            continue;
                        }
                    }
                    page.items.push(Item::Move(width));
                    width
                }
            };
        }
        while w.top() != BEFORE {
            if w.top() > L_CODE {
                self.lr_problems += 10000;
            }
            w.stack.pop();
        }
        Ok(())
    }

    /// pdfTeX §1714–§1719: take entries off `rest` and return them
    /// reversed (in taking order, so the last one comes first). A
    /// `segment` ends at the node closing it: then also the width of that
    /// node and the distance of the left edge after it, relative to the
    /// running position `w.h`, which advances over what was taken.
    #[allow(clippy::type_complexity)]
    fn reverse<'a>(
        &mut self,
        this_box: &BoxNode,
        rest: &mut Vec<Ent<'a>>,
        segment: bool,
        w: &mut Walk,
        glue: &mut SetGlue,
    ) -> Result<(Vec<Ent<'a>>, Option<(Scaled, Scaled)>), Jump> {
        let mut l = Vec::new();
        // unmatched math nodes: of the segment's direction, and of the
        // other one (kept, flipped)
        let (mut m, mut n) = (0u32, 0u32);
        loop {
            let Some(p) = rest.pop() else {
                if !segment && m == 0 && n == 0 {
                    return Ok((l, None));
                }
                // manufacture one missing math node
                rest.push(Ent::Math {
                    width: 0,
                    subtype: w.top(),
                });
                self.lr_problems += 10000;
                continue;
            };
            let (e, width) = match p {
                Ent::Node(Node::Glyphs(g)) => {
                    let f = i32::from(g.font.0);
                    for &c in g.chars() {
                        w.h += self.char_width(f, c);
                        l.push(Ent::Char(f, c));
                    }
                    continue;
                }
                Ent::Node(Node::Ligature(x)) => {
                    let f = i32::from(x.font.0);
                    (Ent::Char(f, x.ch), self.char_width(f, x.ch))
                }
                Ent::Char(f, c) => (p, self.char_width(f, c)),
                Ent::Node(Node::Box(b)) => (p, b.width),
                Ent::Node(Node::Rule { width, .. }) => (p, *width),
                Ent::Node(Node::Kern { width, .. }) => (Ent::Move(*width), *width),
                Ent::Node(Node::Glue { spec, .. }) => {
                    let wd = glue.set(this_box, spec);
                    (Ent::Move(wd), wd)
                }
                Ent::Node(Node::Leaders(x)) => {
                    let wd = glue.set(this_box, &x.spec);
                    (Ent::Leaders(x, wd), wd)
                }
                Ent::Move(wd) | Ent::Leaders(_, wd) => (p, wd),
                Ent::Math { width, subtype } => {
                    let e = if is_end(subtype) {
                        if w.top() == end_of(subtype) {
                            w.stack.pop();
                            if n > 0 {
                                n -= 1;
                                Ent::Math {
                                    width,
                                    subtype: begin_of(subtype),
                                }
                            } else if m > 0 {
                                m -= 1;
                                Ent::Move(width)
                            } else {
                                // the end of the reflected segment
                                return Ok((l, Some((width, -w.h - width))));
                            }
                        } else {
                            self.lr_problems += 1;
                            Ent::Move(width)
                        }
                    } else {
                        w.stack.push(end_of(subtype));
                        if n > 0 || is_rtl(subtype) != self.out_rtl {
                            n += 1;
                            Ent::Math {
                                width,
                                subtype: subtype + 1,
                            }
                        } else {
                            m += 1;
                            Ent::Move(width)
                        }
                    };
                    (e, width)
                }
                Ent::Edge { .. } => return self.confusion(b"LR2"),
                // no width
                Ent::Node(_) => (p, 0),
            };
            w.h += width;
            l.push(e);
        }
    }

    fn char_width(&self, f: i32, c: u8) -> Scaled {
        self.font_read(f, crate::track::font::METRICS);
        self.fonts.get(f).glyph(i32::from(c)).map_or(0, |g| g.width)
    }

    /// pdfTeX's check for LR anomalies at the end of `ship_out`.
    pub(crate) fn report_lr_problems(&mut self) {
        if self.lr_problems > 0 {
            self.print_ln();
            self.print_nl(b"\\endL or \\endR problem (");
            self.print_int(i32::try_from(self.lr_problems / 10000).expect("count"));
            self.print_str(b" missing, ");
            self.print_int(i32::try_from(self.lr_problems % 10000).expect("count"));
            self.print_str(b" extra");
            self.lr_problems = 0;
            self.print_char(b')');
            self.print_ln();
        }
        self.out_rtl = false;
    }
}
