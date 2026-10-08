//! Check mode for steps (`Config::check`): every live step runs again,
//! dry, from the state before it, and what it makes is compared with what
//! the graph holds: its output state, its members (kind, op and value, in
//! order) and its definitions. A step's interior is evaluated again in a
//! buffer, as its sweep does; with `Config::keep_interior` every emission
//! is a member, so every leaf is compared.

use std::marker::PhantomData;

use super::{
    Args, DEAD, END, Emit, Graph, Kind, NONE, Opd, Pos, SArg, SEALED, StepCx, resolve, with_extra,
};
use crate::lang::{Lang, Step};
use crate::seq::ElemId;
use crate::value::Value;

impl<L: Lang, const P: bool> Graph<L, P> {
    /// Every live step run again and compared (sealed regions are not:
    /// their interiors are gone by design).
    pub(super) fn check_steps(&self) {
        for i in 1..self.n.h.len() {
            let h = &self.n.h[i];
            if h.kind == Kind::Step && h.flags & (DEAD | SEALED) == 0 {
                #[allow(clippy::cast_possible_truncation, reason = "ids fit u32")]
                self.check_step(i as u32);
            }
        }
    }

    #[allow(clippy::too_many_lines, reason = "one dry run, then the comparison")]
    fn check_step(&self, s: u32) {
        let si = self.n.h[s as usize].aux as usize;
        let u = self.steps[si].unfold;
        let ui = self.n.h[u as usize].aux as usize;
        let input = self.unfolds[ui].input.clone();
        let at = self.steps[si].at;
        let start = match &input {
            None => 0,
            Some(inp) if at == END => inp.len(),
            Some(inp) => inp.index_of(at).unwrap_or(inp.len()),
        };
        let prev_o = self.n.opds_of(s)[0];
        let mut em: Emit<L> = Emit::default();
        let res = {
            let uo = self.n.opds_of(u);
            let mut cx = StepCx {
                g: &self.n,
                names: &self.names,
                groups: &self.groups,
                unfold_opds: uo,
                input: input.as_ref(),
                idx: start,
                start,
                leaf: &[] as &[(ElemId, L::Val)],
                lbase: 0,
                step: s,
                pos: self.n.pos(s),
                grp: self.steps[si].grp_in,
                opened: 0,
                em: &mut em,
                ext: None,
                keep: self.cfg.keep_interior,
                cancel: &self.cancel,
                hook: None,
                tick: crate::profile::Tick::default(),
                _brand: PhantomData,
            };
            let args = Args::of(&self.n, &uo[2..]);
            let st = self.n.read(&prev_o);
            L::step(self.n.h[s as usize].op, &st, &args, &mut cx)
        };
        let out = match &res {
            Step::Next { st, .. } | Step::Done(st) => st.ver(),
        };
        assert!(
            out == self.n.val[s as usize].ver(),
            "check: step %{s} makes {:?} but holds {:?}",
            match res {
                Step::Next { st, .. } | Step::Done(st) => st,
            },
            self.n.val[s as usize]
        );
        // its members, in order: the big emissions
        let kids = self.children(s);
        let mut bigs = em.bigs.clone();
        bigs.sort_unstable();
        assert_eq!(
            bigs.len(),
            kids.len(),
            "check: step %{s} makes {} members but holds {}",
            bigs.len(),
            kids.len()
        );
        let mut ids = vec![NONE; em.specs.len()];
        for (k, &i) in bigs.iter().enumerate() {
            let (sp, c) = (&em.specs[i as usize], kids[k]);
            assert!(
                sp.kind == self.n.h[c as usize].kind && sp.op == self.n.h[c as usize].op,
                "check: step %{s}'s member {k} is %{c} ({:?} {:?}), made {:?} {:?}",
                self.n.h[c as usize].kind,
                self.n.h[c as usize].op,
                sp.kind,
                sp.op
            );
            ids[i as usize] = c;
        }
        // the interior evaluated again in order, as the sweep does
        let mut vals = std::mem::take(&mut em.vals);
        for i in 0..em.specs.len() {
            let sp = &em.specs[i];
            let (a0, an) = (sp.args.0 as usize, sp.args.1 as usize);
            match sp.kind {
                Kind::Leaf if !sp.done => {
                    for a in a0..a0 + an {
                        if let SArg::Name(m, x) = em.args[a].a {
                            let at = Pos {
                                parent: s,
                                ord: u64::from(sp.sub),
                            };
                            em.args[a].o =
                                with_extra(resolve(&self.n, &self.names, &self.groups, m, at), x);
                        } else if let SArg::Node(src, sel) = em.args[a].a {
                            em.args[a].o = Opd {
                                src,
                                sel,
                                name: NONE,
                            };
                        }
                    }
                    let v = {
                        let args = Args {
                            g: &self.n,
                            opds: &[],
                            sweep: Some((&em.args[a0..a0 + an], &ids, &vals)),
                        };
                        L::eval(sp.op, &args)
                    };
                    vals[i] = v;
                }
                Kind::Leaf | Kind::Const => {}
                // (a creator's or a cross read's value is its node's)
                _ => vals[i] = self.n.val[ids[i] as usize].clone(),
            }
            let c = ids[i];
            if c != NONE && matches!(sp.kind, Kind::Leaf | Kind::Const) {
                assert!(
                    vals[i].ver() == self.n.val[c as usize].ver(),
                    "check: step %{s}'s member %{c} ({:?}) holds {:?} but evaluates to {:?}",
                    sp.op,
                    self.n.val[c as usize],
                    vals[i]
                );
            }
        }
        // its definitions
        let held = self.step_defs(s);
        assert_eq!(
            held.len(),
            em.defs.len(),
            "check: step %{s} makes {} definitions but holds {}",
            em.defs.len(),
            held.len()
        );
        for (d, h) in em.defs.iter().zip(&held) {
            let src = match d.1 {
                SArg::Local(ix, _) => ids[ix as usize],
                SArg::Node(src, _) => src,
                SArg::Name(m, x) => {
                    let at = Pos {
                        parent: s,
                        ord: d.3,
                    };
                    with_extra(resolve(&self.n, &self.names, &self.groups, m, at), x).src
                }
            };
            assert!(
                d.0 == h.2 && src == h.0 && d.2 == h.3,
                "check: step %{s} defines name {} from %{src} but holds name {} from %{}",
                d.0,
                h.2,
                h.0
            );
        }
    }
}
