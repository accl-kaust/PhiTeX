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

/// The prefix of a symbol's internal name: `\u{10FFFE}sym\u{2}<register>\u{2}<node raw>`.
pub(crate) const SYM_PREFIX: &str = "\u{10FFFE}sym\u{2}";

/// A symbol: integer register `reg`'s value as of piece `node`. By the
/// register's number: a symbol goes from run to run in what a macro or
/// a list holds, and a run's location of a register past the static
/// ones is that run's own (`State::spill_loc`, in the order it met
/// them).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Sym {
    pub reg: i32,
    pub node: NodeId,
    /// The steps the piece made of the register before the read
    /// (docs/note-relative-counters.md §2.1): the value is the one reaching
    /// the piece plus these.
    pub j: i32,
}

impl Sym {
    /// The symbol of register `loc` as of piece `node`, if `loc` is an
    /// integer register.
    pub(crate) fn at(r: &Run<'_, '_>, loc: Loc, node: NodeId) -> Option<Sym> {
        Some(Sym {
            reg: r.count_reg(loc)?,
            node,
            j: 0,
        })
    }

    /// The register's location in this run.
    pub(crate) fn loc(self, r: &mut Run<'_, '_>) -> Loc {
        r.reg_loc(0, self.reg)
    }
}

/// The token of symbol `s` (an internal control sequence, interned once
/// per (register, piece): `Tok::cs` of its name).
pub(crate) fn sym_tok(s: Sym) -> Tok {
    Tok::cs(&format!(
        "{SYM_PREFIX}{}\u{2}{}\u{2}{}",
        s.reg,
        s.node.raw(),
        s.j
    ))
}

/// The symbol a token is, if it is one (its name starts with
/// [`SYM_PREFIX`]; parsed back, `CsId::name`).
pub(crate) fn sym_of(t: Tok) -> Option<Sym> {
    let rest = t.as_cs()?.name().strip_prefix(SYM_PREFIX)?;
    let (reg, rest) = rest.split_once('\u{2}')?;
    let (node, j) = rest.split_once('\u{2}')?;
    Some(Sym {
        reg: reg.parse().ok()?,
        node: NodeId::from_raw(node.parse().ok()?),
        j: j.parse().ok()?,
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
    routine_regs(r);
    r.defer_regs.as_ref().is_some_and(|ls| ls.contains(&loc))
}

/// The page-layer registers ([`deferrable`]): `count:0` and the integer
/// registers the output routine may define.
pub(crate) fn page_regs(r: &mut Run<'_, '_>) -> Vec<Loc> {
    routine_regs(r);
    let mut ls = vec![r.reg_loc(0, 0)];
    ls.extend(r.defer_regs.iter().flatten().copied());
    ls
}

/// [`Run::defer_regs`], made once per run.
fn routine_regs(r: &mut Run<'_, '_>) {
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
            if v.with_str(f.0, |n| {
                n.strip_prefix("count:")
                    .is_some_and(|k| k.parse::<i32>().is_ok())
            }) == Some(true)
            {
                locs.push(r.loc_of(f));
            }
        }
        r.defer_regs = Some(locs);
    }
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
    let j = r.st.steps_of(loc);
    r.st.note_symbol(loc);
    r.st.note_read(loc);
    let reg = r.count_reg(loc).expect("deferrable: an integer register");
    sym_tok(Sym { reg, node, j })
}

/// [`symbol_for`] with steps `j` given: a register's value reaching the
/// piece plus `j` (an expression's on it, [`Run::defer_sym`]).
pub(crate) fn symbol_with(r: &mut Run<'_, '_>, loc: Loc, j: i32) -> Tok {
    let node = r.cx.node();
    r.st.note_symbol(loc);
    r.st.note_read(loc);
    let reg = r.count_reg(loc).expect("deferrable: an integer register");
    sym_tok(Sym { reg, node, j })
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
    for &t in toks {
        match digits_str(r, t)? {
            Some(d) => out.extend(d.chars().map(|c| Tok::ch(12, u32::from(c)))),
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
    if crate::exec::obs_flag!("PHITEX_SYM_TRACE") {
        trace_force(r, s);
    }
    if s.node != r.cx.node() {
        return resolve(r, s);
    }
    r.defer_ok = false;
    let loc = s.loc(r);
    // (the run has not written it since: the piece's own read, memoised;
    // else the value reaching the run, as first loaded or read now)
    let pristine = !r.st.has_map(loc) && !r.st.defined_after(loc, 0);
    if pristine {
        return Ok(r.int_loc(loc)?.wrapping_add(s.j));
    }
    if let Some(&b) = r.reg_loaded_base.get(&loc) {
        return Ok(b.wrapping_add(s.j));
    }
    let f = r.loc_field(loc);
    let x = crate::val::i32_of(r.v, r.cx.read(f)?);
    r.reg_loaded_base.insert(loc, x);
    Ok(x.wrapping_add(s.j))
}

/// `PHITEX_SYM_TRACE` (observe): a symbol forced, for finding what keeps a
/// relative register's readers concrete.
#[cold]
fn trace_force(r: &Run<'_, '_>, s: Sym) {
    eprintln!(
        "sym-force: node {} (by {}) count {} j {} cur {:?} | {}",
        r.cx.node().raw(),
        s.node.raw(),
        s.reg,
        s.j,
        r.cur.tok.as_cs().map(|c| c.name().to_owned()),
        r.trace_context()
    );
}

/// A finisher resolves a symbol: the definition of the register reaching
/// the piece's position, `Ctx::read_at(field, At::Before(s.node))`
/// (`FieldAt`'s value; none: the environment's value, as the piece
/// would have read). Never a read at the finisher's own position.
pub(crate) fn resolve(r: &mut Run<'_, '_>, s: Sym) -> crate::run::R<i32> {
    let loc = s.loc(r);
    let f = r.loc_field(loc);
    let x = match r.cx.read_at(f, At::Before(s.node))? {
        FieldAt::Def(x, _) => int_of(r.v, x).unwrap_or(0),
        // (none reaches it: an absent integer is 0, as a read gives it;
        // the piece gone, the finisher is asked again when it comes back)
        FieldAt::Unset | FieldAt::Gone => 0,
    };
    Ok(x.wrapping_add(s.j))
}

/// A symbol met where none may be: the link, the files, a tool, or a
/// finisher's run outside shipout where the engine's answer is absent.
/// A bug of the implementation, never of a document.
pub(crate) fn leaked(r: &Run<'_, '_>, s: Sym) -> ! {
    panic!(
        "defer: symbol of register {} by node {} reached op {}",
        s.reg,
        s.node.raw(),
        r.op_name()
    )
}

/// `\count<n>=` (or `\c@page=` through `\setcounter`) whose operand's
/// first expanded token is a symbol, followed by nothing but an optional
/// space, or an integer register holding one unresolved (`\global\c@page
/// \calc@Acount` after `\global\calc@Acount\calc@Bcount`): neither read
/// nor value (docs/spec-state-model.md §2.2). This piece's own symbol of
/// the same register is a restore ([`restore_reaching`]): the run's map
/// of it the identity and, where the register was loaded, its run-local
/// value back to what was loaded before any map of this run
/// (`Run::reg_loaded_base`). Any other symbol is an alias ([`alias`]).
/// Called from `control::assign_int` and `control::do_register_command`
/// after the optional `=`, before `scan_int`; true when it consumed the
/// operand.
pub(crate) fn try_symbolic(r: &mut Run<'_, '_>, loc: Loc, global: bool) -> crate::run::R<bool> {
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
        if c == cmd::SYM {
            break;
        }
        if c == cmd::SPACER {
            continue;
        }
        // (a `\countdef` register holding an alias unresolved: copied
        // as it is, §413's internal integer, nothing scanned after it)
        if c == cmd::ASSIGN_INT {
            let l = r.quantity_loc(cmd::ASSIGN_INT, r.cur.chr);
            if let Some((g, node)) = r.st.alias_at(l, r.v) {
                let gl = r.loc_of(g);
                if let Some(s) = Sym::at(r, gl, node)
                    && symbolic(r, loc, s, global)
                {
                    return Ok(true);
                }
            }
            r.back_cur();
            return Ok(false);
        }
        if c <= cmd::MAX_COMMAND || (c >= cmd::END_TEMPLATE && c != cmd::DONT_EXPAND) {
            r.back_cur();
            return Ok(false);
        }
        r.expand();
        r.ck()?;
    }
    let t = r.cur.tok;
    let Some(s) = sym_of(t) else {
        r.back_input(t);
        return Ok(false);
    };
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
    if !symbolic(r, loc, s, global) {
        r.back_input(u);
        r.back_input(t);
        return Ok(false);
    }
    if !space {
        r.back_input(u);
    }
    Ok(true)
}

/// Register `loc` set to symbol `s`: a restore where `s` is this
/// piece's own symbol of `loc`, else an alias.
fn symbolic(r: &mut Run<'_, '_>, loc: Loc, s: Sym, global: bool) -> bool {
    let node = r.cx.node();
    if Sym::at(r, loc, node) == Some(s) {
        return restore_reaching(r, loc, global);
    }
    // (this piece's own symbol of the register with its steps: the value
    // reaching the piece plus them, a step of it again)
    if s.j != 0 && Sym::at(r, loc, node).is_some_and(|o| o.reg == s.reg && o.node == s.node) {
        return restore_plus(r, loc, s.j, global);
    }
    // (another register's or piece's value plus steps is neither: the
    // digits are read)
    s.j == 0 && alias(r, loc, s, global)
}

/// Integer register `loc` set to this piece's own symbol of it with steps
/// `j` (docs/spec-relative-counters.md §3.3): the value reaching the
/// piece plus `j`, which is a step of that value, so the run's map of the
/// register becomes `+j` whatever it held, neither a read nor a
/// definition (beamer's `\beamer@restorecounter`: a counter saved as
/// `\the\c@figure` after the frame stepped it, set back at each overlay).
/// Only where a step could be a relation ([`Run::map_ok`]) and the run
/// made no definition of it; false elsewhere (the caller forces the
/// digits and assigns as TeX does).
fn restore_plus(r: &mut Run<'_, '_>, loc: Loc, j: i32, global: bool) -> bool {
    if !r.map_ok(loc, global) || r.st.defined_after(loc, 0) {
        return false;
    }
    let v = r.v;
    if !r.st.restore_plus(v, loc, j, global) {
        return false;
    }
    r.muts += 1;
    true
}

/// Integer register `loc` set to symbol `s` of another register (or of
/// another piece), globally or at the run's base level, where it could
/// hold steps ([`Run::map_ok`]: a document piece, before its first
/// candidate): the run defines it as `s`'s register's value as of `s`'s
/// piece, `Ctx::define_alias` ([`crate::state::Entry::alias`]), so a
/// reader of `loc` reads that register there itself, and a change of it
/// asks that reader again, never this run. A use in the run resolves it
/// (`Run::load`). False elsewhere (the caller assigns as TeX does).
fn alias(r: &mut Run<'_, '_>, loc: Loc, s: Sym, global: bool) -> bool {
    if !r.map_ok(loc, global) {
        return false;
    }
    let gl = s.loc(r);
    let g = r.loc_field(gl);
    r.note_reg_def(loc);
    let e = crate::state::Entry::alias(r.v, g, s.node);
    r.set_loc(loc, e, global);
    true
}

/// Integer register `loc` put back to the value reaching the run
/// (docs/spec-deferred-reads.md §2), where that is neither a read nor a
/// definition: the run's writes of it maps, back to the identity; none,
/// it holds that value already; its own definitions, made at its base
/// level after its last candidate, taken back. False where none is so
/// (the caller assigns as TeX does).
pub(crate) fn restore_reaching(r: &mut Run<'_, '_>, loc: Loc, global: bool) -> bool {
    if r.map_ok(loc, global) && !r.st.defined_after(loc, 0) {
        r.reg_restore(loc, global);
        return true;
    }
    let own = !r.st.has_map(loc);
    if !own || !(global || r.innermost_group().is_none()) {
        return false;
    }
    if !r.st.defined_after(loc, 0) {
        return true;
    }
    if r.reg_first_def.get(&loc) == Some(&r.cand) && r.st.undefine_fresh(loc) {
        r.reg_first_def.remove(&loc);
        return true;
    }
    false
}

/// An assignment of `x` to integer register `loc` where `x` is the value
/// reaching the run, which the run read (forced): a restore
/// ([`restore_reaching`]), so a register saved and set back through
/// other registers (calc's `\setcounter`: `\calc@Acount`, then
/// `\c@page`) defines nothing; the read stands, so the run is asked
/// again where that value moves.
pub(crate) fn restores(r: &mut Run<'_, '_>, loc: Loc, x: i32, global: bool) -> bool {
    if r.reg_loaded_base.get(&loc) != Some(&x) || !r.defer_allowed() || r.count_reg(loc).is_none() {
        return false;
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

// ------------------------------------------------------ file size symbols

/// The tag of a size symbol's name, past [`SYM_PREFIX`]:
/// `<SYM_PREFIX>fsz\u{2}<part>\u{2}<key>`.
const SIZE_TAG: &str = "fsz\u{2}";

/// Which digits of a file's size a size symbol stands for
/// (docs/note-relative-counters.md §2.4): all of them (`\pdffilesize`'s
/// expansion, a non-empty digit string), the first (an undelimited
/// argument TeX took of them, one digit), or the rest after the first
/// (what TeX left on the input, possibly none).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Part {
    All,
    Head,
    Tail,
}

impl Part {
    fn tag(self) -> char {
        match self {
            Part::All => 'a',
            Part::Head => 'h',
            Part::Tail => 't',
        }
    }

    fn of_tag(c: char) -> Option<Part> {
        Some(match c {
            'a' => Part::All,
            'h' => Part::Head,
            't' => Part::Tail,
            _ => return None,
        })
    }
}

/// The token of the size symbol of `part` of file `key`'s size: one
/// internal control sequence, the same in every run and build (a
/// definition holding it does not change with the size).
pub(crate) fn size_tok(part: Part, key: &str) -> Tok {
    Tok::cs(&format!("{SYM_PREFIX}{SIZE_TAG}{}\u{2}{key}", part.tag()))
}

/// The part and file of a size symbol, if `t` is one.
pub(crate) fn size_of(t: Tok) -> Option<(Part, String)> {
    let c = t.as_cs()?;
    let name = c.name();
    let rest = name.strip_prefix(SYM_PREFIX)?.strip_prefix(SIZE_TAG)?;
    let mut cs = rest.chars();
    let part = Part::of_tag(cs.next()?)?;
    let key = cs.as_str().strip_prefix('\u{2}')?;
    Some((part, key.to_owned()))
}

/// Whether `t` is a size symbol.
#[inline]
pub(crate) fn is_size(t: Tok) -> bool {
    is_sym(t) && size_of(t).is_some()
}

/// `\pdffilesize` in a piece of TeX's file `key` ([`Run::tex_key`]), a file the job writes and reads
/// back, written the round before (its size known, `wsize:<key>`): the
/// size symbol of its digits instead of them. The run reads only that
/// the file is there (the presence of its loop-backs, the size not
/// `UNIT`), not the size: a reader that only tests the digits for
/// blankness (expl3's `\file_if_exist`, LaTeX's `\IfFileExists`) does
/// not depend on the size. Where TeX needs the digits they are forced
/// ([`size_digits`]). None where the size is read as before: outside a
/// document's piece, a file this run opened for writing (its text so
/// far), a cut document file, one a tool makes, one not written.
pub(crate) fn size_symbol(r: &mut Run<'_, '_>, key: &str) -> crate::run::R<Option<Tok>> {
    let op = r.cx.op();
    if !(op == r.ops.exec || op == r.ops.resume)
        || !r.doc
        || r.interp.is_some()
        || crate::exec::obs_flag!("PHITEX_NO_DEFER")
    {
        return Ok(None);
    }
    // (a file the job writes first: any other, a package's, fails here
    // and is looked up as before)
    let v = r.v;
    let ws = crate::tools::size_field(v, key);
    if !r.lb_known_present(key, ws)? {
        return Ok(None);
    }
    // (open for writing in this run: its text so far; read back, it is
    // no document file the client cut, `Run::read_cut`)
    if r.get(&format!("wopen:{key}"))?.is_some() {
        return Ok(None);
    }
    let k = r.keyed(crate::fields::Fam::Wkeys, key);
    let w = r.keyed(crate::fields::Fam::Wfile, key);
    if !(r.lb_known_present(key, k)? || r.lb_known_present(key, w)?) || r.tool_makes(key)? {
        return Ok(None);
    }
    // (written the round before: its size not `UNIT`, read as that alone)
    let p = r.cx.path(&[phitex_defuse::Step::Eq(ValueId::UNIT)]);
    let unit = r.cx.read_final_path(ws, p)?;
    if unit.and_then(|x| v.as_bool(x)) != Some(false) {
        return Ok(None);
    }
    Ok(Some(size_tok(Part::All, key)))
}

/// The digits a size symbol stands for, where TeX needs them: the
/// file's size as the round before wrote it (`wsize:<key>`, read
/// whole), as `\pdffilesize` read it where the symbol was made ([`size_symbol`]
/// found it written and not open for writing; a later `\openout` in the
/// run does not change what the expansion was). Forced in a piece,
/// resolved in a finisher; anywhere else a symbol is [`leaked`].
pub(crate) fn size_digits(r: &mut Run<'_, '_>, part: Part, key: &str) -> crate::run::R<String> {
    assert!(
        matches!(r.op_name(), "exec" | "resume" | "finish"),
        "defer: size symbol of {key} reached op {}",
        r.op_name()
    );
    let v = r.v;
    let n = r
        .lb(crate::tools::size_field(v, key))?
        .and_then(|x| v.as_int(x))
        .unwrap_or(0);
    let d = n.to_string();
    Ok(match part {
        Part::All => d,
        Part::Head => d[..1].to_owned(),
        Part::Tail => d[1..].to_owned(),
    })
}

/// The digits symbol `t` stands for (a register's or a size's), as
/// [`value`] and [`size_digits`] give them; None for another token.
pub(crate) fn digits_str(r: &mut Run<'_, '_>, t: Tok) -> crate::run::R<Option<String>> {
    if !is_sym(t) {
        return Ok(None);
    }
    if let Some((part, key)) = size_of(t) {
        return size_digits(r, part, &key).map(Some);
    }
    let Some(s) = sym_of(t) else {
        return Ok(None);
    };
    let mut d = String::new();
    print_sym(r, s, &mut d)?;
    Ok(Some(d))
}

/// A symbol where its digits are needed (`expand` on `cmd::SYM`, a
/// delimited argument, `\string`): they go onto the input, as
/// `conv_toks` puts `\number`'s.
pub(crate) fn expand_any(r: &mut Run<'_, '_>, t: Tok) -> crate::run::R<()> {
    if is_dname(t) {
        return expand_dname(r, t);
    }
    if let Some(d) = digits_str(r, t)? {
        r.push_toks(crate::expand::str_toks(&d));
    }
    Ok(())
}

// ------------------------------------------------------- derived names

/// The tag of a derived name's symbol, past [`SYM_PREFIX`]:
/// `<SYM_PREFIX>dn\u{2}<register>\u{2}<node raw>\u{2}<j>\u{2}<prefix>\u{2}<suffix>`.
const DNAME_TAG: &str = "dn\u{2}";

/// A relative name (docs/note-relative-counters.md §2.2): the control
/// sequence named `prefix`, the digits of symbol `s`, `suffix`, a
/// `\csname` made with the symbol inside. As a token ([`dname_tok`]) it
/// is what `\csname` gives where the name is defined: expanded or
/// compared by `\ifx` through the derived read of its meaning
/// (`Ctx::read_derived`), made concrete anywhere else ([`Run::d_ok`]).
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct DName {
    pub s: Sym,
    pub prefix: String,
    pub suffix: String,
}

/// The token of derived name `d`, if its parts can be told apart in one.
pub(crate) fn dname_tok(d: &DName) -> Option<Tok> {
    if d.prefix.contains('\u{2}') || d.suffix.contains('\u{2}') {
        return None;
    }
    Some(Tok::cs(&format!(
        "{SYM_PREFIX}{DNAME_TAG}{}\u{2}{}\u{2}{}\u{2}{}\u{2}{}",
        d.s.reg,
        d.s.node.raw(),
        d.s.j,
        d.prefix,
        d.suffix
    )))
}

/// Whether `t` is a derived name's symbol.
#[inline]
pub(crate) fn is_dname(t: Tok) -> bool {
    t.is_internal_cs()
        && t.as_cs().is_some_and(|c| {
            c.name()
                .strip_prefix(SYM_PREFIX)
                .is_some_and(|r| r.starts_with(DNAME_TAG))
        })
}

/// The derived name a token is, if it is one.
pub(crate) fn dname_of(t: Tok) -> Option<DName> {
    let c = t.as_cs()?;
    let name = c.name();
    let rest = name.strip_prefix(SYM_PREFIX)?.strip_prefix(DNAME_TAG)?;
    let mut it = rest.splitn(5, '\u{2}');
    let reg = it.next()?.parse().ok()?;
    let node = NodeId::from_raw(it.next()?.parse().ok()?);
    let j = it.next()?.parse().ok()?;
    let prefix = it.next()?.to_owned();
    let suffix = it.next()?.to_owned();
    Some(DName {
        s: Sym { reg, node, j },
        prefix,
        suffix,
    })
}

/// The characters of a `\csname` or `\ifcsname` name into `name`, as
/// §372 takes them, to the first non-character token (the current token
/// then): a register's symbol of this piece met there (the first one)
/// is kept as the place in the name where its digits go, not forced; any
/// other symbol's digits go in as TeX's. The kept symbol and its place.
pub(crate) fn name_chars(
    r: &mut Run<'_, '_>,
    name: &mut String,
) -> crate::run::R<Option<(usize, Sym)>> {
    use crate::val::cmd;
    let keep = r.defer_allowed() && r.rb.is_none();
    let was = r.reg_stop.replace(r.expand_depth);
    let mut kept = None;
    let out = loop {
        r.get_x_token();
        if let Err(e) = r.ck() {
            break Err(e);
        }
        if r.cur.cmd == cmd::SYM
            && let Some(s) = sym_of(r.cur.tok)
        {
            if keep && kept.is_none() && s.node == r.cx.node() {
                kept = Some((name.len(), s));
                continue;
            }
            match print_sym(r, s, name) {
                Ok(()) => continue,
                Err(e) => break Err(e),
            }
        }
        if r.cur.is_cs() {
            break Ok(kept);
        }
        // (XeTeX's names take every Unicode character)
        let c = r.cur.tok.chr();
        crate::print::push_code(name, if r.xe { c } else { c & 0xFF });
    };
    r.reg_stop = was;
    out
}

/// The name `prefix`, symbol `s`'s digits (forced), `suffix`.
pub(crate) fn forced_name(
    r: &mut Run<'_, '_>,
    s: Sym,
    prefix: &str,
    suffix: &str,
) -> crate::run::R<String> {
    let mut n = String::with_capacity(prefix.len() + suffix.len() + 4);
    n.push_str(prefix);
    print_sym(r, s, &mut n)?;
    n.push_str(suffix);
    Ok(n)
}

/// The derived read (`Ctx::read_derived`) of the meaning of the control
/// sequence named `prefix`, symbol `s`'s digits, `suffix`, through `p`:
/// its answer, or None where it cannot stand for the look-up (the caller
/// forces the symbol): another piece's symbol, a run that may not defer,
/// digits that could run into the name's own, a name of that shape the
/// run binds itself (the engine's answer is not the run's).
pub(crate) fn derived_read(
    r: &mut Run<'_, '_>,
    s: Sym,
    prefix: &str,
    suffix: &str,
    p: phitex_defuse::PathId,
) -> crate::run::R<Option<Answer>> {
    if s.node != r.cx.node()
        || !r.defer_allowed()
        || r.rb.is_some()
        || prefix.ends_with(|c: char| c.is_ascii_digit() || c == '-')
        || suffix.starts_with(|c: char| c.is_ascii_digit())
        || crate::exec::obs_flag!("PHITEX_NO_DERIVED")
        || binds_like(r, prefix, suffix)
    {
        return Ok(None);
    }
    let loc = s.loc(r);
    let base = r.loc_field(loc);
    let pre = format!("cs:{prefix}");
    let (x, g) = r.cx.read_derived(base, &pre, i64::from(s.j), suffix, p)?;
    if crate::exec::obs_flag!("PHITEX_SYM_TRACE") {
        eprintln!(
            "sym-derived: node {} count {} j {} name {} answer {:?}",
            r.cx.node().raw(),
            s.reg,
            s.j,
            crate::val::field_name(r.v, g),
            x.map(|x| r.v.render(x).chars().take(60).collect::<String>())
        );
    }
    Ok(Some(Answer(x)))
}

/// A derived read's answer ([`derived_read`]): the named field's value
/// through the path, `None` where the name is not defined.
pub(crate) struct Answer(pub Option<ValueId>);

/// Whether the run binds a control sequence named `prefix`, a number,
/// `suffix` itself (a definition of its own, or one a continuation
/// brought): the run's definitions looked over once, from where the
/// last ask of that shape stopped ([`Run::derived_own`]).
pub(crate) fn binds_like(r: &mut Run<'_, '_>, prefix: &str, suffix: &str) -> bool {
    let key = (prefix.to_owned(), suffix.to_owned());
    let (from, found) = r.derived_own.get(&key).copied().unwrap_or((0, false));
    if found {
        return true;
    }
    let (ls, n) = r.st.defined_locs_from(from);
    let pre = format!("cs:{prefix}");
    let v = r.v;
    let hit = ls.into_iter().any(|l| {
        let f = r.loc_field_peek(l);
        v.with_str(f.0, |name| numbered_like(name, &pre, suffix)) == Some(true)
    });
    r.derived_own.insert(key, (n, hit));
    hit
}

/// Whether `name` is `pre`, an optional minus, decimal digits, `suf`.
fn numbered_like(name: &str, pre: &str, suf: &str) -> bool {
    let Some(mid) = name.strip_prefix(pre).and_then(|m| m.strip_suffix(suf)) else {
        return false;
    };
    let mid = mid.strip_prefix('-').unwrap_or(mid);
    !mid.is_empty() && mid.bytes().all(|b| b.is_ascii_digit())
}

/// A derived name made concrete: its symbol's digits forced (resolved
/// in a finisher), the control sequence they name.
pub(crate) fn dname_concrete(r: &mut Run<'_, '_>, t: Tok) -> crate::run::R<Tok> {
    let d = dname_of(t).expect("a derived name's token");
    let n = forced_name(r, d.s, &d.prefix, &d.suffix)?;
    Ok(Tok::cs(&n))
}

/// A derived name's meaning, read through the derived read (whole); its
/// concrete name's where that read cannot stand.
pub(crate) fn dname_meaning(r: &mut Run<'_, '_>, t: Tok) -> crate::run::R<crate::val::Mean> {
    let d = dname_of(t).expect("a derived name's token");
    if let Some(Answer(x)) =
        derived_read(r, d.s, &d.prefix, &d.suffix, phitex_defuse::PathId::WHOLE)?
    {
        return Ok(crate::val::Mean::decode(r.v, x));
    }
    let g = dname_concrete(r, t)?;
    r.mean_of(g)
}

/// A derived name expanded (an expanding fetch, [`Run::d_ok`]): a macro
/// without parameters, not outer or protected, its body read through
/// the derived read; any other meaning by its concrete name, put back
/// for the fetch to take as TeX's.
pub(crate) fn expand_dname(r: &mut Run<'_, '_>, t: Tok) -> crate::run::R<()> {
    let d = dname_of(t).expect("a derived name's token");
    if let Some(Answer(x)) =
        derived_read(r, d.s, &d.prefix, &d.suffix, phitex_defuse::PathId::WHOLE)?
        && let crate::val::Mean::Macro(m) = crate::val::Mean::decode(r.v, x)
        && m.params.is_empty()
        && m.flags & (2 | crate::etex::PROTECTED) == 0
    {
        r.push_macro_body(&m);
        return Ok(());
    }
    let g = dname_concrete(r, t)?;
    r.back_input(g);
    Ok(())
}
