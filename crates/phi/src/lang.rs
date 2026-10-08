//! The client's side (DESIGN 7.3): its values, its ops, and the pure
//! functions the core calls. Nothing here can reach the graph.

use std::fmt::Debug;
use std::hash::Hash;

use crate::graph::{Args, StepCx};
use crate::seq::Seq;
use crate::value::Value;

/// An effect chain's number.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct Chain(pub u32);

/// A cross-run slot's identity (stable across runs: a hash of what it
/// names).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct Slot(pub u64);

/// A family of cross-run slots, readable in order as a sequence (a
/// table of contents: one slot per entry).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct Fam(pub u32);

/// A node's purity class.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum Class {
    #[default]
    Pure,
    /// A payload on an effect chain, ordered by position within it.
    Effect(Chain),
    /// Orders every chain around it.
    Barrier,
    /// The value slot `s` takes in the next run (the last in position
    /// order wins).
    Publish(Slot),
    /// Publish slot `s` and list it in family `f`, in position order.
    Entry(Fam, Slot),
}

impl Class {
    /// The slot this publishes, if any.
    #[must_use]
    pub fn slot(self) -> Option<Slot> {
        match self {
            Class::Publish(s) | Class::Entry(_, s) => Some(s),
            _ => None,
        }
    }
}

/// What a step returns.
pub enum Step<V> {
    /// Go on: the next step's state and key.
    Next { st: V, key: u64 },
    /// The unfold's result.
    Done(V),
}

/// A place a speculative segment may start (DESIGN 7.4).
pub struct Entry<V> {
    /// The input element the segment starts at.
    pub at: usize,
    /// The key the step there has.
    pub key: u64,
    /// The client's guess of the state there.
    pub guess: Option<V>,
}

/// A client language.
pub trait Lang: Sized + 'static {
    type Val: Value + Default + Debug;
    type Op: Copy + Eq + Hash + Debug + Default + Send + Sync + 'static;

    /// A leaf: its value from its operands.
    fn eval(op: Self::Op, args: &Args<'_, Self>) -> Self::Val;

    /// One step of an unfold: from the state, the unfold's operands and
    /// what it reads through `cx`, the next state or the result.
    fn step(op: Self::Op, st: &Self::Val, args: &Args<'_, Self>, cx: &mut StepCx<'_, Self>) -> Step<Self::Val> {
        let _ = (op, st, args, cx);
        unimplemented!("{op:?} is not an unfold")
    }

    /// One element of a scan: the next state and the element's output.
    fn scan(op: Self::Op, st: &Self::Val, x: &Self::Val, args: &Args<'_, Self>) -> (Self::Val, Self::Val) {
        let _ = (op, st, x, args);
        unimplemented!("{op:?} is not a scan")
    }

    /// A scan's value, from its last state and its outputs.
    fn scan_result(op: Self::Op, st: &Self::Val, outs: &Seq<Self::Val>, args: &Args<'_, Self>) -> Self::Val {
        let _ = (op, outs, args);
        st.clone()
    }

    /// The sequence a value holds, for an unfold's or a scan's input.
    fn as_seq(v: &Self::Val) -> Option<&Seq<Self::Val>>;

    /// A chain's payloads, in order, as a value (a chain read's).
    fn chain_val(items: Seq<Self::Val>) -> Self::Val;

    /// Where an unfold may be entered speculatively.
    fn entries(op: Self::Op, input: &Self::Val, args: &Args<'_, Self>) -> Vec<Entry<Self::Val>> {
        let _ = (op, input, args);
        Vec::new()
    }

    /// Whether a leaf's results go through the memo store.
    fn memo(op: Self::Op) -> bool {
        let _ = op;
        false
    }

    /// A stable number for the op, for memo keys.
    fn op_tag(op: Self::Op) -> u64;

    /// The text form of an op and a value (DESIGN 7.14).
    fn fmt_op(op: Self::Op) -> String {
        format!("{op:?}")
    }
    fn parse_op(s: &str) -> Option<Self::Op> {
        let _ = s;
        None
    }
    fn fmt_val(v: &Self::Val) -> String {
        format!("{v:?}")
    }
    fn parse_val(s: &str) -> Option<Self::Val> {
        let _ = s;
        None
    }
}
