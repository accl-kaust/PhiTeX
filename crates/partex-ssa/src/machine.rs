//! The interface a language implements (`DESIGN.md` §7.17.2): its
//! addresses, values, effects and functions, and the bodies of its
//! calls, written against a [`Cx`] that records or does not.

use core::fmt;
use core::hash::Hash;

use crate::hash::{Stable, Version};
use crate::pvec::PVec;
use crate::value::Value;

/// A stream: a sequence of lines (7.17.5).
pub type Stream<V> = PVec<V>;

/// A language.
pub trait Machine {
    /// A slot of the state, or a stream. Its text form is one token
    /// without spaces, commas, parentheses, `;` or `=`.
    type Addr: Clone + Eq + Ord + Hash + fmt::Display + fmt::Debug;
    type Val: Value + fmt::Debug;
    /// An output effect. Its text form is one line.
    type Effect: Clone + Eq + Hash + fmt::Display + fmt::Debug;
    /// A function id. Its text form is one token.
    type Func: Copy + Eq + Hash + fmt::Display + fmt::Debug;

    /// The body of `f` over `args`.
    fn run<C: Cx<Self>>(&self, f: Self::Func, args: &[Self::Val], cx: &mut C) -> Self::Val;

    /// A slot's place in a dense array (a family and an index in it), if
    /// it has one: the open recorder keeps such slots' serials in arrays,
    /// with no hashing on the read path (§7.17.10 step 2b).
    fn dense(_a: &Self::Addr) -> Option<(usize, usize)> {
        None
    }
}

/// What a body sees: every read, write, effect and nested call goes
/// through it.
pub trait Cx<M: Machine + ?Sized> {
    fn read(&mut self, a: &M::Addr) -> Option<M::Val>;
    /// A field of the value at `a`: the read records the field's version.
    fn read_field(&mut self, a: &M::Addr, field: u32) -> Option<M::Val>;
    /// Write (or, with `None`, remove) the slot at `a`.
    fn write(&mut self, a: &M::Addr, v: Option<M::Val>);
    fn effect(&mut self, e: M::Effect);
    /// A nested call, evaluated by the same rule.
    fn call(&mut self, f: M::Func, args: &[M::Val]) -> M::Val;
    /// Make `stream` exist in this trip.
    fn open(&mut self, stream: &M::Addr);
    /// Append `line` to `stream` in this trip.
    fn store(&mut self, stream: &M::Addr, line: M::Val);
    /// The φ: `stream` as of the end of the previous trip.
    fn load(&mut self, stream: &M::Addr) -> Option<Stream<M::Val>>;
    /// Account `units` of work to the running body.
    fn cost(&mut self, units: u64);
}

/// What a read read: a slot, a field of a slot, or a stream's φ.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Loc<A> {
    State(A),
    Field(A, u32),
    Phi(A),
}

impl<A> Loc<A> {
    pub fn addr(&self) -> &A {
        match self {
            Loc::State(a) | Loc::Field(a, _) | Loc::Phi(a) => a,
        }
    }
}

impl<A: fmt::Display> fmt::Display for Loc<A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Loc::State(a) => write!(f, "{a}"),
            Loc::Field(a, i) => write!(f, "{a}.{i}"),
            Loc::Phi(a) => write!(f, "phi:{a}"),
        }
    }
}

/// A call's name: the hash of its function and its inputs' versions,
/// and nothing else (7.17.2).
pub fn name_of<F: Hash>(f: F, args: &[Version]) -> Version {
    let mut h = Stable::new();
    f.hash(&mut h);
    h.word(0x6e61_6d65 ^ args.len() as u64);
    for a in args {
        h.word128(a.0);
    }
    Version(h.finish128())
}
