//! Deferred reads of page-layer registers (docs/spec-deferred-reads.md):
//! `\the\c@page` in a piece yields a symbol standing for the register's
//! value as of that piece, forced into a real read only where TeX needs
//! the digits, resolved by the page's finisher at shipout, and a
//! restore of the register to its own symbol is the identity map.
//!
//! Each function's doc names where it is called from.

use phitex_defuse::{At, FieldAt, NodeId, ValueId};

use crate::run::Run;
use crate::state::Loc;
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
    Tok::cs(&format!("{SYM_PREFIX}{}\u{2}{}", s.loc, s.node.raw()))
}

/// The symbol a token is, if it is one (its name starts with
/// [`SYM_PREFIX`]; parsed back, `CsId::name`).
pub(crate) fn sym_of(t: Tok) -> Option<Sym> {
    let rest = t.as_cs()?.name().strip_prefix(SYM_PREFIX)?;
    let (loc, node) = rest.split_once('\u{2}')?;
    Some(Sym {
        loc: loc.parse().ok()?,
        node: NodeId::from_raw(node.parse().ok()?),
    })
}

/// Whether register `loc` is a page-layer register in this run: `count:0`,
/// or an integer register the output routine may define (the finisher's
/// `may_define` from `hints::routine_hint`). Asked by [`Run::int_loc`]
/// under [`Run::defer_ok`] before it loads the register.
pub(crate) fn deferrable(r: &mut Run<'_, '_>, loc: Loc) -> bool {
    if r.count_reg(loc) == Some(0) {
        return true;
    }
    if r.defer_regs.is_none() {
        // (the integer registers the finishers defined as the evaluation
        // began, `Ops::fin_defs_start`: what a routine defines, by name;
        // one table for the evaluation, so no run's answer follows the
        // schedule)
        let table = r.ops.fin_defs_start.lock().map(|t| t.clone()).ok();
        let mut fs: Vec<phitex_defuse::Field> = table
            .iter()
            .flat_map(|t| t.values())
            .flat_map(|d| d.seen.iter().copied())
            .collect();
        fs.sort_unstable_by_key(|f| f.0);
        fs.dedup();
        let v = r.v;
        let mut locs = Vec::new();
        for f in fs {
            if v.with_str(f.0, |n| n.strip_prefix("count:").is_some_and(|k| k.parse::<i32>().is_ok())) == Some(true) {
                locs.push(r.loc_of(f));
            }
        }
        r.defer_regs = Some(locs);
    }
    r.defer_regs.as_ref().is_some_and(|ls| ls.contains(&loc))
}

/// `\the`/`\number` on register `loc` alone, in a piece, where the
/// register is not loaded: the symbol's token instead of the digits.
/// Called from `expand::the_toks_r` and `expand::conv_toks` (c == 0)
/// after their scan, when [`Run::defer_hit`] was set by [`Run::int_loc`]
/// (set only while [`Run::defer_ok`] is true, and only when the scanned
/// operand was the register alone: `scan_int` clears `defer_ok` as soon
/// as it consumes anything but that one internal quantity).
pub(crate) fn symbol_for(r: &mut Run<'_, '_>, loc: Loc) -> Tok {
    let node = r.cx.node();
    sym_tok(r.st.symbol(loc, node).unwrap_or(Sym { loc, node }))
}

/// The expansion of a symbol token met by `expand` (its command code
/// `cmd::SYM`): in a list built with expansion (`scan_toks` with
/// `xpand`, as `etex::protect` sees it) it is kept; elsewhere in a piece
/// it is forced (`force`), in a finisher resolved (`resolve`); any other
/// operation: [`leaked`]. The digits go back onto the input as
/// `conv_toks` puts `\number`'s.
pub(crate) fn expand_sym(r: &mut Run<'_, '_>, s: Sym) -> crate::run::R<()> {
    let x = value(r, s)?;
    r.push_number(x);
    Ok(())
}

/// A symbol's digits where they are needed: forced in a piece, resolved
/// in a finisher, [`leaked`] anywhere else.
pub(crate) fn value(r: &mut Run<'_, '_>, s: Sym) -> crate::run::R<i32> {
    match r.op_name() {
        "exec" | "resume" => force(r, s),
        "finish" => resolve(r, s),
        _ => leaked(r, s),
    }
}

/// `toks` with every symbol replaced by its digits ([`value`]), or
/// `None` where it holds none.
pub(crate) fn digits_of(r: &mut Run<'_, '_>, toks: &[Tok]) -> crate::run::R<Option<Vec<Tok>>> {
    if !toks.iter().any(|&t| is_sym(t)) {
        return Ok(None);
    }
    let mut out = Vec::with_capacity(toks.len() + 4);
    let mut d = String::new();
    for &t in toks {
        match sym_of(t).filter(|_| is_sym(t)) {
            Some(s) => {
                d.clear();
                print_sym(r, s, &mut d)?;
                out.extend(d.chars().map(|c| Tok::ch(12, u32::from(c))));
            }
            None => out.push(t),
        }
    }
    Ok(Some(out))
}

/// Whether `t` is a symbol: an internal name past [`SYM_PREFIX`].
#[inline]
pub(crate) fn is_sym(t: Tok) -> bool {
    t.is_internal_cs() && t.as_cs().is_some_and(|c| c.name().starts_with(SYM_PREFIX))
}

/// A piece needs the digits now: the read the piece makes today
/// (`Run::int_loc` with deferral off), memoised: the register is loaded,
/// later symbols of it in this run are concrete.
pub(crate) fn force(r: &mut Run<'_, '_>, s: Sym) -> crate::run::R<i32> {
    // (another piece's symbol, met in this one through a macro or a
    // list it left: the register as of that piece, as it would have
    // read it)
    if s.node != r.cx.node() {
        return resolve(r, s);
    }
    r.defer_ok = false;
    // (the run has not written it since: the piece's own read, memoised;
    // else the value reaching the run, as first loaded or read now)
    let pristine = !r.st.has_map(s.loc) && !r.st.defined_after(s.loc, 0);
    if pristine {
        return r.int_loc(s.loc);
    }
    if let Some(&b) = r.reg_loaded_base.get(&s.loc) {
        return Ok(b);
    }
    let f = r.loc_field(s.loc);
    let x = crate::val::i32_of(r.v, r.cx.read(f)?);
    r.reg_loaded_base.insert(s.loc, x);
    Ok(x)
}

/// A finisher resolves a symbol: the definition of the register reaching
/// the piece's position, `Ctx::read_at(field, At::Before(s.node))`
/// (`FieldAt`'s value; none: the environment's value, as the piece
/// would have read). Never a read at the finisher's own position.
pub(crate) fn resolve(r: &mut Run<'_, '_>, s: Sym) -> crate::run::R<i32> {
    let f = r.loc_field(s.loc);
    Ok(match r.cx.read_at(f, At::Before(s.node))? {
        FieldAt::Def(x, _) => int_of(r.v, x).unwrap_or(0),
        // (none reaches it: an absent integer is 0, as a read gives it;
        // the piece gone, the finisher is asked again when it comes back)
        FieldAt::Unset | FieldAt::Gone => 0,
    })
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
    use crate::val::cmd;
    if r.count_reg(loc).is_none() || !r.defer_allowed() {
        return Ok(false);
    }
    // (the operand's first token as `scan_int` meets it, §440-§441:
    // blanks skipped, expandable tokens expanded, a symbol not)
    loop {
        r.get_token();
        r.ck()?;
        let c = r.cur.cmd;
        if c == cmd::SYM || c == cmd::SPACER {
            if c == cmd::SYM {
                break;
            }
            continue;
        }
        if c <= cmd::MAX_COMMAND || (c >= cmd::END_TEMPLATE && c != cmd::DONT_EXPAND) {
            r.back_cur();
            return Ok(false);
        }
        r.expand();
        r.ck()?;
    }
    let t = r.cur.tok;
    if sym_of(t) != Some(Sym { loc, node: r.cx.node() }) {
        r.back_input(t);
        return Ok(false);
    }
    // (§443-§444: the number ends at a token not a digit, one space
    // taken; a digit goes on with it, so the digits are needed)
    r.get_x_token();
    r.ck()?;
    let u = r.cur.tok;
    let space = r.cur.cmd == cmd::SPACER;
    if !u.is_cs_or_active() && u.cat() == Some(12) && (48..=57).contains(&u.chr()) {
        r.back_input(u);
        r.back_input(t);
        return Ok(false);
    }
    let done = restore_reaching(r, loc, global)?;
    if !done {
        r.back_input(u);
        r.back_input(t);
        return Ok(false);
    }
    if !space {
        r.back_input(u);
    }
    Ok(true)
}

/// Integer register `loc` put back to the value reaching the run
/// (docs/spec-deferred-reads.md §2), where that is neither a read nor a
/// definition: the run's writes of it maps, back to the identity; none,
/// it holds that value already; its own definitions, made at its base
/// level after its last candidate, taken back. False where none is so
/// (the caller assigns as TeX does).
pub(crate) fn restore_reaching(r: &mut Run<'_, '_>, loc: Loc, global: bool) -> crate::run::R<bool> {
    if r.map_ok(loc, global) && !r.st.defined_after(loc, 0) {
        r.reg_restore(loc, global);
        return Ok(true);
    }
    let own = !r.st.has_map(loc);
    if !own || !(global || r.innermost_group().is_none()) {
        return Ok(false);
    }
    if !r.st.defined_after(loc, 0) {
        return Ok(true);
    }
    if r.reg_first_def.get(&loc) == Some(&r.cand) && r.st.undefine_fresh(loc) {
        r.reg_first_def.remove(&loc);
        return Ok(true);
    }
    Ok(false)
}

/// An assignment of `x` to integer register `loc` where `x` is the value
/// reaching the run, which the run read (forced): a restore
/// ([`restore_reaching`]), so a register saved and set back through
/// other registers (calc's `\setcounter`: `\calc@Acount`, then
/// `\c@page`) defines nothing; the read stands, so the run is asked
/// again where that value moves.
pub(crate) fn restores(r: &mut Run<'_, '_>, loc: Loc, x: i32, global: bool) -> crate::run::R<bool> {
    if r.reg_loaded_base.get(&loc) != Some(&x) || !r.defer_allowed() || r.count_reg(loc).is_none() {
        return Ok(false);
    }
    restore_reaching(r, loc, global)
}

/// A symbol printed (`show_toks_into`: `\message`, `\special`, the log):
/// its digits, forced in a piece, resolved in a finisher.
pub(crate) fn print_sym(r: &mut Run<'_, '_>, s: Sym, out: &mut String) -> crate::run::R<()> {
    let x = value(r, s)?;
    crate::print::print_int(out, i64::from(x));
    Ok(())
}

/// The value a symbol's resolution decodes from the engine (an int
/// value), for tests and traces.
pub(crate) fn int_of(v: &phitex_defuse::Values, x: ValueId) -> Option<i32> {
    v.as_int(x).and_then(|n| i32::try_from(n).ok())
}
