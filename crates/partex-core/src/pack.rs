//! Part 33: Packaging (§644–§679).

//!
//! `hpack` and `vpack` are recorded calls (DESIGN 7.17.2): each is named
//! by its inputs' versions (the list, the spec, and what else decides
//! the box and its report), reads what the packing and its report read
//! (the parameters, the fonts, the line a report prints), and writes
//! `last_badness` and its result (7.17.12's boundary rule: a pack's
//! result, `cur_box`, is live where it ends; `track::scalar::HPACK_RESULT`,
//! `VPACK_RESULT`).

use alloc::vec::Vec;

use partex_engine::node::{BoxNode, Node};
use partex_engine::pack::{self, Report, Spec};
use partex_ssa::Version;

use crate::arith::Scaled;
use crate::host::Host;
use crate::nest::IGNORE_DEPTH;
use crate::nodes::param_glue;
use crate::scan::MAX_DIMEN;
use crate::ssa::Func;
use crate::tex::{Jump, Tex};
use crate::track::{Row, Tracker, scalar};
use crate::web::*;

/// What `hpack` gives its caller: the box (its report printed) and its
/// list's glue totals by order (§646; §796 records the stretch in an
/// alignment entry, §1201 looks at the shrink of a display).
pub(crate) struct Hpacked {
    pub(crate) node: BoxNode,
    pub(crate) total_stretch: [Scaled; 4],
    pub(crate) total_shrink: [Scaled; 4],
}

/// How `hpack` packs: TeX's §649; pdfTeX's with the line's fonts
/// expanded to fit (`\pdfadjustspacing`, a line of §889); or §649 with
/// `\overfullrule` zero, as §804 packs an alignment's preamble.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Packer {
    Plain,
    Expanded,
    NoRule,
}

/// The version of a list as a call's input (DESIGN 7.17.2): made from
/// its nodes, each box by the version it carries.
pub(crate) fn list_version(list: &[Node]) -> Version {
    Version::of(list)
}

/// A spec's version.
fn spec_version(spec: Spec) -> Version {
    match spec {
        Spec::Exactly(v) => Version::of(&(0u8, v)),
        Spec::Additional(v) => Version::of(&(1u8, v)),
    }
}

/// A packed box's version as a result: its parts, its list by its nodes
/// (the version it will carry once shared, `BoxNode::share`).
fn box_version(b: &BoxNode) -> Version {
    Version(b.parts_version())
}

/// §644: box dimension is pre-specified.
pub(crate) const EXACTLY: i32 = 0;
/// §644: box dimension is increased from the natural one.
pub(crate) const ADDITIONAL: i32 = 1;

impl<H: Host, T: Tracker> Tex<H, T> {
    /// §645: scan a box specification and left brace.
    pub(crate) fn scan_spec(&mut self, c: i32, three_codes: bool) -> Result<(), Jump> {
        let s = if three_codes { self.saved(0) } else { 0 };
        let spec_code = if self.scan_keyword(b"to")? {
            self.scan_normal_dimen()?;
            EXACTLY
        } else if self.scan_keyword(b"spread")? {
            self.scan_normal_dimen()?;
            ADDITIONAL
        } else {
            self.cur_val = 0;
            ADDITIONAL
        };
        // found:
        if three_codes {
            self.set_saved(0, s);
            self.set_save_ptr(self.save_ptr() + 1);
        }
        self.set_saved(0, spec_code);
        self.set_saved(1, self.cur_val);
        self.set_save_ptr(self.save_ptr() + 2);
        self.new_save_level(c)?;
        self.scan_left_brace()
    }

    /// §649: package hlist `list`; with `adjust`, insertions, marks and
    /// `\vadjust` material migrate there (§655). Reports are printed.
    pub(crate) fn hpack(
        &mut self,
        list: Vec<Node>,
        spec: Spec,
        adjust: Option<&mut Vec<Node>>,
    ) -> BoxNode {
        self.hpack_full(list, spec, adjust).node
    }

    /// [`Tex::hpack`] with the glue totals.
    pub(crate) fn hpack_full(
        &mut self,
        list: Vec<Node>,
        spec: Spec,
        adjust: Option<&mut Vec<Node>>,
    ) -> Hpacked {
        let Ok(mut p) = self.hpack_call(list, spec, adjust, Packer::Plain, |t, list, adjust| {
            Ok::<_, core::convert::Infallible>(t.hpack_plain(list, spec, adjust))
        });
        // (`SyncTeX`: the box, made now, placed here)
        self.sync_box(&mut p.node);
        p
    }

    /// §649 as a recorded call (DESIGN 7.17.2) around `body`, named by
    /// the list, the spec, whether material migrates, the packer, and the
    /// kind of context its report names (`pack_begin_line`'s sign: a
    /// paragraph, an alignment, none; the line itself is a read where the
    /// report prints it, 7.17.1). The result is the box, what migrated
    /// into `adjust`, and the glue totals.
    pub(crate) fn hpack_call<E>(
        &mut self,
        list: Vec<Node>,
        spec: Spec,
        adjust: Option<&mut Vec<Node>>,
        packer: Packer,
        body: impl FnOnce(&mut Self, Vec<Node>, Option<&mut Vec<Node>>) -> Result<Hpacked, E>,
    ) -> Result<Hpacked, E> {
        if !T::VALUES {
            return body(self, list, adjust);
        }
        let name = Version::node(
            0x6870_6163,
            &[
                list_version(&list),
                spec_version(spec),
                Version::of(&(
                    adjust.is_some(),
                    packer as u8,
                    self.pack_begin_line.signum(),
                )),
            ],
        );
        self.tracker.call_begin(Func::Hpack, &[name.0], self);
        let mut adjust = adjust;
        let start = adjust.as_ref().map_or(0, |a| a.len());
        let r = body(self, list, adjust.as_deref_mut());
        if let Ok(p) = &r {
            let migrated = adjust.as_ref().map_or(&[][..], |a| &a[start..]);
            let v = Version::node(
                0x6870_6b72,
                &[
                    box_version(&p.node),
                    list_version(migrated),
                    Version::of(&(p.total_stretch, p.total_shrink)),
                ],
            );
            self.tracker
                .row_wrote(Row::Scalar(scalar::HPACK_RESULT), v.0);
        }
        self.tracker.call_end(self);
        r
    }

    /// §804: `hpack` of an alignment's preamble, with `\overfullrule`
    /// zero ("prevent rule from being packaged": TeX stores zero in the
    /// parameter around the call, the call here does not read it).
    pub(crate) fn hpack_preamble(&mut self, list: Vec<Node>, spec: Spec) -> Hpacked {
        let Ok(p) = self.hpack_call(list, spec, None, Packer::NoRule, |t, list, adjust| {
            let params = pack::Params {
                badness: t.int_par(HBADNESS_CODE),
                fuzz: t.dimen_par(HFUZZ_CODE),
                overfull_rule: 0,
                texxet: t.texxet_en(),
            };
            let packed = pack::hpack(list, spec, &params, &t.tracked_fonts(), adjust);
            Ok::<_, core::convert::Infallible>(t.hpacked(packed))
        });
        let mut p = p;
        self.sync_box(&mut p.node);
        p
    }

    /// §649's body: the packing and its report.
    fn hpack_plain(
        &mut self,
        list: Vec<Node>,
        spec: Spec,
        adjust: Option<&mut Vec<Node>>,
    ) -> Hpacked {
        let params = self.hpack_params();
        let packed = pack::hpack(list, spec, &params, &self.tracked_fonts(), adjust);
        self.hpacked(packed)
    }

    /// A packed hlist with its report printed.
    pub(crate) fn hpacked(&mut self, packed: pack::Packed) -> Hpacked {
        let (total_stretch, total_shrink) = (packed.total_stretch, packed.total_shrink);
        Hpacked {
            node: self.report_hpack(packed),
            total_stretch,
            total_shrink,
        }
    }

    /// The parameters `hpack` reads.
    pub(crate) fn hpack_params(&self) -> pack::Params {
        pack::Params {
            badness: self.int_par(HBADNESS_CODE),
            fuzz: self.dimen_par(HFUZZ_CODE),
            overfull_rule: self.dimen_par(OVERFULL_RULE_CODE),
            texxet: self.texxet_en(),
        }
    }

    /// §649: finish `hpack`: set `last_badness` and print the report.
    pub(crate) fn report_hpack(&mut self, mut packed: pack::Packed) -> BoxNode {
        self.set_last_badness(packed.badness);
        if let Some(r) = packed.report {
            self.print_pack_report(r, true);
            self.finish_hbox_report(&packed.node);
            self.diag_box(r, true, &packed.node);
        }
        if packed.lr.any() {
            // TeXXeT: report LR problems.
            let (missing, extra) = (packed.lr.missing.len(), packed.lr.extra);
            packed.close_lr();
            self.print_ln();
            self.print_nl(b"\\endL or \\endR problem (");
            self.print_int(i32::try_from(missing).unwrap_or(i32::MAX));
            self.print_str(b" missing, ");
            self.print_int(i32::try_from(extra).unwrap_or(i32::MAX));
            self.print_str(b" extra");
            self.finish_hbox_report(&packed.node);
        }
        packed.node
    }

    /// §663: finish issuing a diagnostic message for an overfull or
    /// underfull hbox.
    fn finish_hbox_report(&mut self, node: &BoxNode) {
        {
            if self.output_active() {
                self.print_str(b") has occurred while \\output is active");
            } else {
                if self.pack_begin_line != 0 {
                    if self.pack_begin_line > 0 {
                        self.print_str(b") in paragraph at lines ");
                    } else {
                        self.print_str(b") in alignment at lines ");
                    }
                    self.print_line_no(usize::MAX, self.pack_begin_line.abs());
                    self.print_str(b"--");
                } else {
                    self.print_str(b") detected at line ");
                }
                self.print_line_no(self.in_open, self.line);
            }
            self.print_ln();
            self.font_in_short_display = NULL_FONT;
            self.short_display(&node.list);
            self.print_ln();
            self.begin_diagnostic();
            self.show_box_node(node);
            self.end_diagnostic(true);
        }
    }

    /// §660–§667, §674–§678: the start of a packaging report.
    fn print_pack_report(&mut self, r: Report, horizontal: bool) {
        let what: &[u8] = if horizontal { b"hbox" } else { b"vbox" };
        self.print_ln();
        match r {
            Report::Underfull { badness } | Report::Loose { badness } => {
                if badness > 100 {
                    self.print_nl(b"Underfull");
                } else {
                    self.print_nl(b"Loose");
                }
                self.print_str(b" \\");
                self.print_str(what);
                self.print_str(b" (badness ");
                self.print_int(badness);
            }
            Report::Overfull { excess } => {
                self.print_nl(b"Overfull \\");
                self.print_str(what);
                self.print_str(b" (");
                self.print_scaled(excess);
                self.print_str(if horizontal {
                    b"pt too wide"
                } else {
                    b"pt too high"
                });
            }
            Report::Tight { badness } => {
                self.print_nl(b"Tight \\");
                self.print_str(what);
                self.print_str(b" (badness ");
                self.print_int(badness);
            }
        }
    }

    /// §668: `vpack`.
    pub(crate) fn vpack(&mut self, list: Vec<Node>, spec: Spec) -> Result<BoxNode, Jump> {
        self.vpackage(list, spec, MAX_DIMEN)
    }

    /// §668: package vlist `list`, with box depth at most `l`: a recorded
    /// call (DESIGN 7.17.2), named by the list, the spec, `l` and the kind
    /// of context its report names (as `hpack_call`'s).
    pub(crate) fn vpackage(
        &mut self,
        list: Vec<Node>,
        spec: Spec,
        l: Scaled,
    ) -> Result<BoxNode, Jump> {
        Ok(self.vpack_call(list, spec, l)?.node)
    }

    /// `vpackage` with the list's glue totals (§796).
    pub(crate) fn vpack_call(
        &mut self,
        list: Vec<Node>,
        spec: Spec,
        l: Scaled,
    ) -> Result<Hpacked, Jump> {
        self.vpack_with(list, spec, l, false)
    }

    /// §1017: `vpackage` of the page into box 255 with its report
    /// inhibited: TeX stores `inf_bad` in `\vbadness` and `max_dimen` in
    /// `\vfuzz` around the call, and the call here does not read them.
    pub(crate) fn vpack_quiet(
        &mut self,
        list: Vec<Node>,
        spec: Spec,
        l: Scaled,
    ) -> Result<Hpacked, Jump> {
        self.vpack_with(list, spec, l, true)
    }

    /// §668 as a recorded call, named also by whether its report is
    /// inhibited (`quiet`).
    fn vpack_with(
        &mut self,
        list: Vec<Node>,
        spec: Spec,
        l: Scaled,
        quiet: bool,
    ) -> Result<Hpacked, Jump> {
        if !T::VALUES {
            let mut r = self.vpack_body(list, spec, l, quiet);
            if let Ok(p) = &mut r {
                // (`SyncTeX`: the box, made now, placed here)
                self.sync_box(&mut p.node);
            }
            return r;
        }
        let name = Version::node(
            0x7670_6163,
            &[
                list_version(&list),
                spec_version(spec),
                Version::of(&(l, self.pack_begin_line.signum(), quiet)),
            ],
        );
        self.tracker.call_begin(Func::Vpack, &[name.0], self);
        let r = self.vpack_body(list, spec, l, quiet);
        if let Ok(p) = &r {
            let v = Version::node(
                0x7670_6b72,
                &[
                    box_version(&p.node),
                    Version::of(&(p.total_stretch, p.total_shrink)),
                ],
            );
            self.tracker
                .row_wrote(Row::Scalar(scalar::VPACK_RESULT), v.0);
        }
        self.tracker.call_end(self);
        let mut r = r;
        if let Ok(p) = &mut r {
            self.sync_box(&mut p.node);
        }
        r
    }

    fn vpack_body(
        &mut self,
        list: Vec<Node>,
        spec: Spec,
        l: Scaled,
        quiet: bool,
    ) -> Result<Hpacked, Jump> {
        let params = if quiet {
            pack::Params {
                badness: partex_engine::scaled::INF_BAD,
                fuzz: MAX_DIMEN,
                overfull_rule: 0,
                texxet: false,
            }
        } else {
            self.vpack_params()
        };
        match pack::vpack(list, spec, l, &params) {
            Ok(p) => {
                let (total_stretch, total_shrink) = (p.total_stretch, p.total_shrink);
                Ok(Hpacked {
                    node: self.report_vpack(p),
                    total_stretch,
                    total_shrink,
                })
            }
            Err(c) => self.confusion(c.0.as_bytes()),
        }
    }

    /// The parameters `vpack` reads.
    pub(crate) fn vpack_params(&self) -> pack::Params {
        pack::Params {
            badness: self.int_par(VBADNESS_CODE),
            fuzz: self.dimen_par(VFUZZ_CODE),
            overfull_rule: 0,
            texxet: false,
        }
    }

    /// §668: finish `vpack`: set `last_badness` and print the report.
    pub(crate) fn report_vpack(&mut self, packed: pack::Packed) -> BoxNode {
        self.set_last_badness(packed.badness);
        if let Some(r) = packed.report {
            self.print_pack_report(r, false);
            // §675: finish issuing a diagnostic message for an overfull or
            // underfull vbox.
            if self.output_active() {
                self.print_str(b") has occurred while \\output is active");
            } else {
                if self.pack_begin_line != 0 {
                    // it's actually negative
                    self.print_str(b") in alignment at lines ");
                    self.print_line_no(usize::MAX, self.pack_begin_line.abs());
                    self.print_str(b"--");
                } else {
                    self.print_str(b") detected at line ");
                }
                self.print_line_no(self.in_open, self.line);
                self.print_ln();
            }
            self.begin_diagnostic();
            self.show_box_node(&packed.node);
            self.end_diagnostic(true);
            self.diag_box(r, false, &packed.node);
        }
        packed.node
    }

    /// §679: append box (or unset row) `b` to the current vlist, with
    /// interline glue.
    pub(crate) fn append_to_vlist(&mut self, mut b: Node) {
        self.sync_node(&mut b);
        let (height, depth) = match &b {
            Node::Box(x) => (x.height, x.depth),
            Node::Unset(u) => (u.height, u.depth),
            _ => (0, 0),
        };
        if self.prev_depth() > IGNORE_DEPTH {
            let d = self.glue_par(BASELINE_SKIP_CODE).width - self.prev_depth() - height;
            let p = if d < self.dimen_par(LINE_SKIP_LIMIT_CODE) {
                self.new_param_glue(LINE_SKIP_CODE)
            } else {
                let mut g = self.glue_par(BASELINE_SKIP_CODE).copy();
                g.width = d;
                param_glue(g, BASELINE_SKIP_CODE)
            };
            let mut p = p;
            self.sync_node(&mut p);
            self.nodes_mut().push(p);
        }
        self.nodes_mut().push(b);
        self.set_prev_depth(depth);
    }
}

/// §645: the box specification saved by `scan_spec`.
pub(crate) fn spec(code: i32, v: Scaled) -> Spec {
    if code == EXACTLY {
        Spec::Exactly(v)
    } else {
        Spec::Additional(v)
    }
}
