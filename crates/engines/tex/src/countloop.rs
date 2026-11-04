//! A macro that calls itself over simple register statements, run whole.
//!
//! `\def\m{<statements>\ifnum a<b\relax\expandafter\m\fi}` (what `\loop`
//! makes): when each statement is a `\advance` of an integer register or an
//! assignment to a character code table, and all it reads are integer
//! registers and constants, a pass is arithmetic, and the passes up to the
//! one whose test fails can be done with no token read. The last pass is
//! left to the interpreter, and so is any pass that would report an error
//! or overflow, so nothing but the plain case is done here.
//!
//! Everything the passes read is read as they read it: the meaning of every
//! token of the body in order (stopping at the first token that does not
//! fit, the first the interpreter would have made anything else of), and
//! the two values every assignment asks about. Nothing is read that the
//! first pass would not read, and a register not yet loaded (a deferred
//! read) leaves the loop to the interpreter.

use crate::run::{R, Run};
use crate::state::Entry;
use crate::val::{Tok, cmd};
use phitex_defuse::Stop;

/// Most passes at one time: the engine's bound on a run sees work between.
const MAX_PASSES: i32 = 1 << 17;
/// Most integer registers a loop body names.
const MAX_REGS: usize = 4;
/// Most statements of a loop body.
const MAX_STMTS: usize = 8;
/// Fewest entries left to wait: fewer are written at once.
const BULK_MIN: i32 = 1024;
const USV: i32 = crate::xetex::BIGGEST_USV.cast_signed();

/// An integer the body reads.
#[derive(Clone, Copy)]
enum Arg {
    /// Register, by its index in [`Plan::regs`].
    Reg(u8),
    Const(i32),
}

/// A statement of the body.
#[derive(Clone, Copy)]
enum Stmt {
    /// `\advance` of a register by a value.
    Advance(u8, Arg),
    /// `\<table>code <c>=<value>`: the table's name, the largest value it
    /// holds.
    Code(&'static str, i32, Arg, Arg),
}

#[derive(Default)]
struct Plan {
    regs: Vec<crate::state::Loc>,
    advanced: [bool; MAX_REGS],
    stmts: Vec<Stmt>,
}

impl Plan {
    fn reg(&mut self, l: crate::state::Loc) -> Option<u8> {
        if let Some(i) = self.regs.iter().position(|&x| x == l) {
            return u8::try_from(i).ok();
        }
        if self.regs.len() == MAX_REGS {
            return None;
        }
        self.regs.push(l);
        u8::try_from(self.regs.len() - 1).ok()
    }
}

#[inline]
fn value(a: Arg, r: &[i32; MAX_REGS]) -> i32 {
    match a {
        Arg::Reg(i) => r[usize::from(i)],
        Arg::Const(c) => c,
    }
}

/// Whether a macro `name` with this body ends in the tail of a loop: its last
/// three tokens are `\expandafter`-like, itself and `\fi`-like (their
/// meanings are looked at when the loop is read). One check per macro the
/// command loop calls.
#[inline]
pub(crate) fn has_loop_tail(body: &[Tok], name: Tok) -> bool {
    matches!(body, [.., _, n, _] if *n == name)
}

fn skip_spaces(body: &[Tok], i: &mut usize) {
    while body.get(*i).is_some_and(|t| t.cat() == Some(10)) {
        *i += 1;
    }
}

impl Run<'_, '_> {
    /// The meaning of `t` as a command and its code, read as the
    /// interpreter would read it.
    fn loop_meaning(&mut self, t: Tok) -> R<(u8, i32)> {
        self.set_cur_flag(t);
        self.ck()?;
        Ok((self.cur.cmd, self.cur.chr))
    }

    /// An integer the body names at `body[*i]`: an integer register, a
    /// `\chardef` or `\mathchardef`, or decimal digits ended by a space (the
    /// space is theirs) or a `\relax`: nothing expanded to find their end.
    fn loop_arg(&mut self, p: &mut Plan, body: &[Tok], i: &mut usize) -> R<Option<Arg>> {
        let Some(&t) = body.get(*i) else {
            return Ok(None);
        };
        if t.is_cs_or_active() {
            let (c, chr) = self.loop_meaning(t)?;
            *i += 1;
            return Ok(match c {
                cmd::CHAR_GIVEN | cmd::MATH_GIVEN => Some(Arg::Const(chr)),
                cmd::ASSIGN_INT => {
                    let l = self.quantity_loc(c, chr);
                    if self.count_reg(l).is_some() {
                        p.reg(l).map(Arg::Reg)
                    } else {
                        None
                    }
                }
                _ => None,
            });
        }
        let mut n: i32 = 0;
        let mut digits = 0;
        while let Some(&d) = body.get(*i)
            && d.cat() == Some(12)
            && (0x30..=0x39).contains(&d.chr())
        {
            // (no value near the integers' limit: scan_int's error is
            // the interpreter's)
            if digits >= 9 {
                return Ok(None);
            }
            n = n * 10 + i32::try_from(d.chr() - 0x30).unwrap_or(0);
            digits += 1;
            *i += 1;
        }
        if digits == 0 {
            return Ok(None);
        }
        let Some(&end) = body.get(*i) else {
            return Ok(None);
        };
        if end.is_cs_or_active() {
            let (c, chr) = self.loop_meaning(end)?;
            if c != cmd::RELAX || chr == crate::val::NO_EXPAND_FLAG {
                return Ok(None);
            }
        } else if end.cat() == Some(10) {
            *i += 1;
        } else {
            return Ok(None);
        }
        Ok(Some(Arg::Const(n)))
    }

    /// `\advance`'s operands at `body[*i]`, added to the plan: false for
    /// another shape.
    fn loop_advance(&mut self, p: &mut Plan, body: &[Tok], i: &mut usize) -> R<bool> {
        if !body.get(*i).is_some_and(|r| r.is_cs_or_active()) {
            return Ok(false);
        }
        let Some(Arg::Reg(reg)) = self.loop_arg(p, body, i)? else {
            return Ok(false);
        };
        skip_spaces(body, i);
        if let (Some(a), Some(b)) = (body.get(*i), body.get(*i + 1))
            && a.cat().is_some_and(|c| c == 11 || c == 12)
            && b.cat().is_some_and(|c| c == 11 || c == 12)
            && a.chr() == u32::from(b'b')
            && b.chr() == u32::from(b'y')
        {
            *i += 2;
        }
        skip_spaces(body, i);
        let Some(by) = self.loop_arg(p, body, i)? else {
            return Ok(false);
        };
        p.advanced[usize::from(reg)] = true;
        p.stmts.push(Stmt::Advance(reg, by));
        Ok(true)
    }

    /// A code assignment's operands at `body[*i]`, of the table at `chr`,
    /// added to the plan: false for another table or shape.
    fn loop_code(&mut self, p: &mut Plan, chr: i32, body: &[Tok], i: &mut usize) -> R<bool> {
        let (table, max) = match chr {
            crate::val::LC_CODE_BASE if self.xe => ("lc", USV),
            crate::val::UC_CODE_BASE if self.xe => ("uc", USV),
            crate::val::LC_CODE_BASE => ("lc", 255),
            crate::val::UC_CODE_BASE => ("uc", 255),
            crate::val::CAT_CODE_BASE => ("cat", 15),
            _ => return Ok(false),
        };
        let Some(ch) = self.loop_arg(p, body, i)? else {
            return Ok(false);
        };
        if body
            .get(*i)
            .is_some_and(|t| t.cat() == Some(12) && t.chr() == u32::from(b'='))
        {
            *i += 1;
        }
        let Some(val) = self.loop_arg(p, body, i)? else {
            return Ok(false);
        };
        p.stmts.push(Stmt::Code(table, max, ch, val));
        Ok(true)
    }

    /// `\ifnum`'s operands at `body[*i]`.
    fn loop_test(
        &mut self,
        p: &mut Plan,
        body: &[Tok],
        i: &mut usize,
    ) -> R<Option<(Arg, u32, Arg)>> {
        let Some(lhs) = self.loop_arg(p, body, i)? else {
            return Ok(None);
        };
        skip_spaces(body, i);
        let Some(&r) = body.get(*i) else {
            return Ok(None);
        };
        if r.cat() != Some(12) || !matches!(r.chr(), 0x3C..=0x3E) {
            return Ok(None);
        }
        *i += 1;
        skip_spaces(body, i);
        let Some(rhs) = self.loop_arg(p, body, i)? else {
            return Ok(None);
        };
        Ok(Some((lhs, r.chr(), rhs)))
    }

    /// The statements of `body` up to the `\ifnum`, then the test and the
    /// tail: the plan and the test, or `None` where the body is another
    /// shape or a meaning is another.
    fn loop_plan(&mut self, name: Tok, body: &[Tok]) -> R<Option<(Plan, Arg, u32, Arg)>> {
        let mut p = Plan::default();
        let mut i = 0;
        let (lhs, rel, rhs) = loop {
            if p.stmts.len() > MAX_STMTS {
                return Ok(None);
            }
            let Some(&t) = body.get(i) else {
                return Ok(None);
            };
            if !t.is_cs_or_active() {
                return Ok(None);
            }
            let (c, chr) = self.loop_meaning(t)?;
            i += 1;
            match c {
                cmd::ADVANCE if chr == 0 => {
                    if !self.loop_advance(&mut p, body, &mut i)? {
                        return Ok(None);
                    }
                }
                cmd::DEF_CODE => {
                    if !self.loop_code(&mut p, chr, body, &mut i)? {
                        return Ok(None);
                    }
                }
                cmd::IF_TEST if chr == 2 => {
                    let Some(test) = self.loop_test(&mut p, body, &mut i)? else {
                        return Ok(None);
                    };
                    break test;
                }
                _ => return Ok(None),
            }
        };
        // (the true branch: `\relax`, if any, then the call by way of `\fi`)
        let mut tail = &body[i.min(body.len())..];
        if let [t, rest @ ..] = tail
            && rest.len() == 3
        {
            let (c, chr) = self.loop_meaning(*t)?;
            if c != cmd::RELAX || chr == crate::val::NO_EXPAND_FLAG {
                return Ok(None);
            }
            tail = rest;
        }
        let [x, n, f] = tail else {
            return Ok(None);
        };
        if *n != name
            || self.loop_meaning(*x)? != (cmd::EXPAND_AFTER, 0)
            || self.loop_meaning(*f)? != (cmd::FI_OR_ELSE, crate::expand::FI_CODE)
        {
            return Ok(None);
        }
        Ok(Some((p, lhs, rel, rhs)))
    }

    /// A body that sets each entry of a code table to its own character and
    /// steps to the next, up to a limit, in a group, over enough entries
    /// past the fixed slots: the passes done at once, the entries past those
    /// slots left to wait ([`crate::state::BulkCodes`]). The register's
    /// index in the plan and the passes done.
    fn bulk_pass(
        &mut self,
        p: &Plan,
        lhs: Arg,
        rel: u32,
        rhs: Arg,
        regs: &[i32; MAX_REGS],
    ) -> Option<(u8, i32)> {
        let [
            Stmt::Code(name, max, Arg::Reg(c), Arg::Reg(val)),
            Stmt::Advance(step, Arg::Const(1)),
        ] = p.stmts[..]
        else {
            return None;
        };
        let Arg::Reg(l) = lhs else {
            return None;
        };
        if c != val || c != step || c != l || rel != 0x3C || matches!(rhs, Arg::Reg(r) if r == c) {
            return None;
        }
        let table = crate::val::CODE_NAMES.iter().position(|&n| n == name)?;
        let s = regs[usize::from(c)];
        let top = if self.xe { USV.min(max) } else { max.min(255) };
        if s < 0 {
            return None;
        }
        // (the last pass, whose test fails, and one past the table: the
        // interpreter's)
        let n = i64::from(value(rhs, regs)) - i64::from(s) - 1;
        let n = i32::try_from(n.min(i64::from(top) - i64::from(s) + 1)).ok()?;
        let e = s + n;
        let lo = s.max(256);
        if e - lo < BULK_MIN || self.read_back_done || !self.st.bulk_ok() {
            return None;
        }
        let gid = self.innermost_group()?;
        if crate::fields::code_slot(table, 256).is_some() {
            return None;
        }
        for x in s..lo {
            let l = self.code_loc(name, x);
            self.set_loc(l, Entry::int(x), false);
        }
        self.st.bulk_codes(table, lo, e, gid);
        self.muts += 1;
        Some((c, n))
    }

    /// Everything waiting ([`crate::state::BulkCodes`]) written, as the
    /// passes would have written it, before the run does what can see it.
    #[cold]
    #[inline(never)]
    fn flush_bulk_rest(&mut self) {
        let Some((table, rest, gid)) = self.st.bulk_rest() else {
            return;
        };
        for c in rest {
            let l = self.numbered_loc(crate::val::CODE_NAMES[table], c.into());
            self.st.set(l, Entry::int(c), false, Some(gid));
        }
        self.muts += 1;
    }

    /// [`Run::flush_bulk_rest`] where anything waits.
    #[inline]
    pub(crate) fn flush_bulk_codes(&mut self) {
        if self.st.bulk_waits() {
            self.flush_bulk_rest();
        }
    }

    /// The command loop calls macro `name`, of body `body`, which ends as a
    /// loop does: the passes before the last made with no token read, if
    /// the body is one this does ([`crate::countloop`]). The call itself
    /// then goes on as ever and makes the next pass. `cur` as it was.
    pub(crate) fn counted_loop(&mut self, name: Tok, body: &[Tok]) -> R<()> {
        if self.aopen != 0
            || self.scanner.is_some()
            || self.in_skip
            || self.interp.is_some()
            || self.bare
            || self.rec_load
        {
            return Ok(());
        }
        let cur = self.cur;
        let plan = self.loop_plan(name, body);
        self.cur = cur;
        let Some((p, lhs, rel, rhs)) = plan? else {
            return Ok(());
        };
        // (no read here the interpreter would defer)
        if p.regs.iter().any(|&l| self.st.load_state(l) != 1) {
            return Ok(());
        }
        // (what each assignment asks: both read as it does)
        if self.ipar(crate::fields::Par::globaldefs)? != 0
            || self
                .get_loc(crate::fields::cell::AFTERTOKEN)?
                .is_some_and(|t| t != phitex_defuse::ValueId::UNIT)
        {
            return Ok(());
        }
        let mut regs = [0; MAX_REGS];
        for (x, &l) in regs.iter_mut().zip(&p.regs) {
            *x = self.st.int_of(l, self.v);
        }
        let start = regs;
        let mut passes = 0;
        let mut did_bulk = false;
        if let Some((i, n)) = self.bulk_pass(&p, lhs, rel, rhs, &regs) {
            regs[usize::from(i)] += n;
            passes = n;
            did_bulk = true;
        }
        let mut codes: Vec<(&'static str, i32, i32)> = Vec::with_capacity(p.stmts.len());
        'passes: while !did_bulk && passes < MAX_PASSES {
            let mut r = regs;
            codes.clear();
            for s in &p.stmts {
                match *s {
                    Stmt::Advance(i, by) => {
                        let i = usize::from(i);
                        // (an overflow, and a total no one step can make:
                        // the interpreter's)
                        let Some(x) = r[i].checked_add(value(by, &r)) else {
                            break 'passes;
                        };
                        if i32::try_from(i64::from(x) - i64::from(start[i])).is_err() {
                            break 'passes;
                        }
                        r[i] = x;
                    }
                    Stmt::Code(table, max, c, v) => {
                        let (c, v) = (value(c, &r), value(v, &r));
                        let top = if self.xe { USV } else { 255 };
                        // (its errors the interpreter's)
                        if !(0..=top).contains(&c) || !(0..=max).contains(&v) {
                            break 'passes;
                        }
                        codes.push((table, c, v));
                    }
                }
            }
            let (a, b) = (value(lhs, &r), value(rhs, &r));
            let go = match rel {
                0x3C => a < b,
                0x3D => a == b,
                _ => a > b,
            };
            if !go {
                break;
            }
            for &(table, c, v) in &codes {
                let l = self.code_loc(table, c);
                self.set_loc(l, Entry::int(v), false);
            }
            regs = r;
            passes += 1;
        }
        if passes == 0 {
            return Ok(());
        }
        for (i, &l) in p.regs.iter().enumerate() {
            if !p.advanced[i] || regs[i] == start[i] {
                continue;
            }
            // (as `\advance` does, the steps as one)
            if self.map_ok(l, false) && self.st.add(self.v, l, regs[i] - start[i], false) {
                self.muts += 1;
                continue;
            }
            self.note_reg_def(l);
            self.set_loc(l, Entry::int(regs[i]), false);
        }
        let stop: Result<(), Stop> = self.cx.tick(u64::try_from(passes).unwrap_or(0));
        stop.map_err(crate::run::Halt::Stop)
    }
}
