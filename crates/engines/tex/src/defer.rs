#![allow(dead_code)] // (the skeleton: wired by the implementation, docs/spec-deferred-reads.md)
//! Deferred reads of page-layer registers (docs/spec-deferred-reads.md):
//! `\the\c@page` in a piece yields a symbol standing for the register's
//! value as of that piece, forced into a real read only where TeX needs
//! the digits, resolved by the page's finisher at shipout, and a
//! restore of the register to its own symbol is the identity map.
//!
//! Every `todo!()` here is the body the implementation fills; the doc of
//! each names where it is called from and what it must do.

use phitex_defuse::{At, NodeId, ValueId};

use crate::run::Run;
use crate::runstate::Loc;
use crate::val::Tok;

/// The prefix of a symbol's internal name: `\u{10FFFE}sym\u{2}<loc>\u{2}<node raw>`.
pub(crate) const SYM_PREFIX: &str = "\u{10FFFE}sym\u{2}";

/// A symbol: register `loc`'s value as of piece `node`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Sym {
    pub loc: Loc,
    pub node: NodeId,
}

/// The token of symbol `s` (an internal control sequence, interned once
/// per (register, piece): `Tok::cs` of its name).
pub(crate) fn sym_tok(s: Sym) -> Tok {
    let _ = s;
    // fill: Tok::cs(&format!("{SYM_PREFIX}{}\u{2}{}", s.loc, s.node.raw()))
    todo!()
}

/// The symbol a token is, if it is one (its name starts with
/// [`SYM_PREFIX`]; parsed back, `CsId::name`).
pub(crate) fn sym_of(t: Tok) -> Option<Sym> {
    let _ = t;
    // fill: t.as_cs().and_then(|c: CsId| parse c.name() past SYM_PREFIX)
    todo!()
}

/// Whether register `loc` is a page-layer register in this run: `count:0`,
/// or an integer register the output routine may define (the finisher's
/// `may_define` from `hints::routine_hint`). Asked by [`Run::int_loc`]
/// under [`Run::defer_ok`] before it loads the register.
pub(crate) fn deferrable(r: &mut Run<'_, '_>, loc: Loc) -> bool {
    let _ = (r, loc);
    // fill: r.count_reg(loc) == Some(0) || routine-defined count registers (cache the set once per run from the finisher's may_define / routine hint)
    todo!()
}

/// `\the`/`\number` on register `loc` alone, in a piece, where the
/// register is not loaded: the symbol's token instead of the digits.
/// Called from `expand::the_toks_r` and `expand::conv_toks` (c == 0)
/// after their scan, when [`Run::defer_hit`] was set by [`Run::int_loc`]
/// (set only while [`Run::defer_ok`] is true, and only when the scanned
/// operand was the register alone: `scan_int` clears `defer_ok` as soon
/// as it consumes anything but that one internal quantity).
pub(crate) fn symbol_for(r: &mut Run<'_, '_>, loc: Loc) -> Tok {
    let _ = (r, loc);
    // fill: sym_tok(Sym { loc, node: r.cx.node() })
    todo!()
}

/// The expansion of a symbol token met by `expand` (its command code
/// `cmd::SYM`): in a list built with expansion (`scan_toks` with
/// `xpand`, as `etex::protect` sees it) it is kept; elsewhere in a piece
/// it is forced (`force`), in a finisher resolved (`resolve`); any other
/// operation: [`leaked`]. The digits go back onto the input as
/// `conv_toks` puts `\number`'s.
pub(crate) fn expand_sym(r: &mut Run<'_, '_>, s: Sym) -> crate::run::R<()> {
    let _ = (r, s);
    // fill: match r.op_name() { "exec" | "resume" => force, "finish" => resolve, _ => leaked }; push digits
    todo!()
}

/// A piece needs the digits now: the read the piece makes today
/// (`Run::int_loc` with deferral off), memoised: the register is loaded,
/// later symbols of it in this run are concrete.
pub(crate) fn force(r: &mut Run<'_, '_>, s: Sym) -> crate::run::R<i32> {
    let _ = (r, s);
    // fill: debug_assert!(s.node == r.cx.node()); r.defer_ok = false; r.int_loc(s.loc)
    todo!()
}

/// A finisher resolves a symbol: the definition of the register reaching
/// the piece's position, `Ctx::read_at(field, At::Before(s.node))`
/// (`FieldAt`'s value; none: the environment's value, as the piece
/// would have read). Never a read at the finisher's own position.
pub(crate) fn resolve(r: &mut Run<'_, '_>, s: Sym) -> crate::run::R<i32> {
    let _ = (r, s, At::Before(NodeId::from_raw(0)));
    // fill: let f = r.loc_field(s.loc); r.cx.read_at(f, At::Before(s.node)) -> value -> as_int, env fallback via r.env_int(f)
    todo!()
}

/// A symbol met where none may be: the link, the files, a tool, or a
/// finisher's run outside shipout where the engine's answer is absent.
/// A bug of the implementation, never of a document.
pub(crate) fn leaked(r: &Run<'_, '_>, s: Sym) -> ! {
    panic!("defer: symbol of register {:?} by node {} reached op {}", s.loc, s.node.raw(), r.op_name())
}

/// `\count<n>=` (or `\c@page=` through `\setcounter`) whose operand's
/// first expanded token is this piece's own symbol of the same register,
/// followed by nothing but an optional space: a restore. Called from
/// `control::do_register_command` after the optional `=`, before
/// `scan_int`; true when it consumed the operand. The run's map of the
/// register becomes the identity ([`Run::reg_restore`]) and, where the
/// register was loaded, its run-local value goes back to what was loaded
/// before any map of this run (`Run::reg_loaded_base`).
pub(crate) fn try_restore(r: &mut Run<'_, '_>, loc: Loc, global: bool) -> crate::run::R<bool> {
    let _ = (r, loc, global);
    // fill: peek: r.get_x_token(); if sym_of(r.cur.tok) == Some(Sym{loc, node: r.cx.node()}) { optional space; r.reg_restore(loc, global); Ok(true) } else { r.back_cur(); Ok(false) }
    todo!()
}

/// A symbol printed (`show_toks_into`: `\message`, `\special`, the log):
/// its digits, forced in a piece, resolved in a finisher.
pub(crate) fn print_sym(r: &mut Run<'_, '_>, s: Sym, out: &mut String) -> crate::run::R<()> {
    let _ = (r, s, out);
    // fill: let x = if r.op_name() == "finish" { resolve(r, s)? } else { force(r, s)? }; crate::print::print_int(out, i64::from(x))
    todo!()
}

/// The value a symbol's resolution decodes from the engine (an int
/// value), for tests and traces.
pub(crate) fn int_of(v: &phitex_defuse::Values, x: ValueId) -> Option<i32> {
    v.as_int(x).and_then(|n| i32::try_from(n).ok())
}
