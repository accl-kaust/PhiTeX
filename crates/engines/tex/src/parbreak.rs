//! Layer 3: a paragraph's line breaking as a node of its own, [`Break`]
//! (docs/spec-layers.md §2).
//!
//! Where a piece ends a paragraph on the main vertical list (§1096
//! `end_graf`) and the piece ends there (every paragraph end is a cut,
//! [`crate::pages`]), it does not break the lines itself: it captures
//! everything line breaking reads where the paragraph ends ([`BreakIn`]:
//! the list, the parameters, the language, the `\lccode`s of the
//! characters in it, the interline glue parameters) and places a child
//! [`Break`] with it. The child is right after the piece in the text, so
//! what it contributes (the lines, as a unit of [`crate::vprof::VPROF`]
//! and [`crate::pages::PITEMS`], and the `items` of the main list) and
//! what it defines (what the lines leave the main list: the previous
//! depth, the last item, `\prevgraf`) are where the lines are in TeX.
//!
//! The node is pure: it reads the fonts of its list and the hyphenation
//! patterns, nothing of the text. The piece's expansion never waits for
//! it; the next piece reads what it leaves only where that piece's own
//! items do not decide it (`crate::pages::Run::define_main_tail`), and
//! `\prevgraf`, `\lastbox` and their kin read it on a prediction
//! ([`crate::pages::Feedback::LastLine`]).
//!
//! Where the lines are read in the piece itself they are broken there,
//! as today ([`Run::line_break`]):
//!
//! - a paragraph inside a list of the run's own (a `\vbox`, an
//!   insertion, an alignment's cell): the lines go to that list;
//! - a paragraph interrupted by a display (§1145): the display reads
//!   the last line's width ([`crate::pages::Feedback::PreDisplay`]);
//! - a paragraph ended where the piece cannot end (inside a group or a
//!   conditional of its own).

use phitex_defuse::{Ctx, Effect, Field, MayDefine, Operation, Stop, ValueId, Values};

use crate::build::{MAIN_LAST, MAIN_PD, MAIN_SINCE, Mode};
use crate::ops::{Shared, ops};
use crate::run::{R, Run};

/// What line breaking reads where a paragraph ends, captured by the
/// piece ending it: the [`Break`] node's argument, so the node reads
/// nothing of the text.
#[expect(
    dead_code,
    reason = "the layers skeleton: used once its stubs are filled (docs/spec-layers.md §7)"
)]
#[derive(Clone, Debug)]
pub(crate) struct BreakIn {
    /// The paragraph's horizontal list, encoded.
    pub list: ValueId,
    /// `phitex_typeset::linebreak::LineBreakParams` encoded: the
    /// parameters of §816–§848 and §889–§890, `\parshape`, the e-TeX
    /// penalty arrays, `\prevgraf` where the paragraph began, the
    /// language and hyphen minima, the box packing parameters.
    pub params: ValueId,
    /// `\lccode` of each character in the list: `[[c, lc]…]`, and
    /// `\uchyph`.
    pub lc: ValueId,
    pub uc_hyph: i32,
    /// The hyphenation patterns' value (the `PATTERNS` cell), the key of
    /// the process's pattern cache.
    pub patterns: ValueId,
    /// `\baselineskip`, `\lineskip`, `\lineskiplimit` (§679): the
    /// interline glue parameters every line carries.
    pub interline: [ValueId; 3],
}

#[expect(
    dead_code,
    reason = "the layers skeleton: used once its stubs are filled (docs/spec-layers.md §7)"
)]
impl BreakIn {
    /// The node's arguments.
    pub(crate) fn args(&self, v: &Values) -> Vec<ValueId> {
        vec![
            self.list,
            self.params,
            self.lc,
            v.int(i64::from(self.uc_hyph)),
            self.patterns,
            v.list(self.interline.to_vec()),
        ]
    }

    /// From the node's arguments.
    pub(crate) fn of(v: &Values, args: &[ValueId]) -> Option<BreakIn> {
        let [list, params, lc, uc, patterns, il] = args[..] else {
            return None;
        };
        let il = crate::val::list_of(v, il);
        Some(BreakIn {
            list,
            params,
            lc,
            uc_hyph: i32::try_from(crate::val::int_of(v, uc)).unwrap_or(0),
            patterns,
            interline: [
                il.first().copied().unwrap_or(ValueId::UNIT),
                il.get(1).copied().unwrap_or(ValueId::UNIT),
                il.get(2).copied().unwrap_or(ValueId::UNIT),
            ],
        })
    }
}

/// The fields a paragraph's lines define for the main list after them:
/// what they leave it (`crate::build::MainTail`) and `\prevgraf`.
fn break_defs(v: &Values) -> Vec<Field> {
    vec![
        v.field(MAIN_PD),
        v.field(MAIN_LAST),
        v.field(MAIN_SINCE),
        v.field("pg:0"),
    ]
}

/// A paragraph's line breaking (the module's doc). Its argument is its
/// [`BreakIn`].
pub(crate) struct Break(pub Shared);

impl Operation for Break {
    fn name(&self) -> &'static str {
        "break"
    }
    fn may_define(&self, _: &[ValueId], v: &Values) -> MayDefine {
        MayDefine::Names(break_defs(v))
    }
    /// What the lines leave the main list, as the last run on the same
    /// argument left it ([`crate::pages::Feedback::LastLine`]); none
    /// before any.
    fn predicted_defs(&self, args: &[ValueId], v: &Values) -> Option<Vec<(Field, ValueId)>> {
        let _ = (ops(&self.0), args, v);
        None
    }
    fn effect(&self, _: &[ValueId], _: &Values) -> Effect {
        Effect::None
    }
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let _nt = crate::exec::NodeTimer::new(self.name(), cx.node().raw());
        let _ = ops(&self.0);
        todo!("layer 3: a paragraph's lines as a node of its own (docs/spec-layers.md §7 step 2)")
    }
}

impl Run<'_, '_> {
    /// Whether the paragraph ending here on `md`'s list is broken by a
    /// [`Break`] node (the module's doc): on the main vertical list, in
    /// a piece that ends at this paragraph's end, not before a display.
    /// None yet: every paragraph is broken in its piece.
    #[expect(
        clippy::unused_self,
        reason = "the layers skeleton: used once its stubs are filled (docs/spec-layers.md §7)"
    )]
    pub(crate) fn break_as_node(&mut self, md: &Mode) -> bool {
        let _ = md;
        false
    }

    /// What line breaking reads where the paragraph ends
    /// ([`BreakIn`], as `Run::line_break_` reads it), and the [`Break`]
    /// child placed with it.
    pub(crate) fn place_break(&mut self, md: &Mode, items: Vec<crate::nest::Item>) -> R<()> {
        let _ = (md, items);
        todo!("layer 3: what line breaking reads, captured where the paragraph ends")
    }
}
