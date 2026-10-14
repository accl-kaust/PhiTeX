//! A readable snapshot of the scheduler's state: the queue, the nodes still
//! in work with what each waits on, and the counters. It only try-locks a
//! node's data, so it is safe to take from another thread while an
//! evaluation is stuck (a node whose data is held shows as `busy`).

use std::fmt::Write as _;
use std::sync::atomic::Ordering;

use super::query::Subj;
use super::{S_CLEAN, S_DIRTY, S_FREE, S_INERT, S_MARKER, S_NEW, S_PARK, S_RUN, S_STEP, Shared, Slot};

/// The most nodes a dump lists in each section.
const LIST: usize = 200;

fn st_name(st: u8) -> &'static str {
    match st {
        S_FREE => "free",
        S_NEW => "new",
        S_DIRTY => "dirty",
        S_RUN => "run",
        S_PARK => "park",
        S_STEP => "step",
        S_CLEAN => "clean",
        S_INERT => "inert",
        S_MARKER => "marker",
        _ => "?",
    }
}

impl Shared {
    fn value_text(&self, v: crate::values::ValueId) -> String {
        let mut s = format!("{:?}", self.values.get(v));
        if s.len() > 80 {
            let mut end = 80;
            while !s.is_char_boundary(end) {
                end -= 1;
            }
            s.truncate(end);
            s.push('…');
        }
        s
    }

    /// One line about node `s`: id, operation, arguments, unit, depth,
    /// state, and what it waits on.
    pub(crate) fn dump_node(&self, out: &mut String, s: Slot) {
        let r = self.rec(s);
        let st = r.st();
        let _ = write!(
            out,
            "  n{} unit={} depth={} state={}",
            r.id.load(Ordering::Acquire),
            r.unit.load(Ordering::Acquire),
            r.depth.load(Ordering::Acquire),
            st_name(st)
        );
        let Ok(d) = r.d.try_lock() else {
            out.push_str(" (busy)\n");
            return;
        };
        let _ = write!(out, " op={}", self.op_name(d.op));
        if !d.args.is_empty() {
            out.push_str(" args=[");
            for (i, a) in d.args.iter().take(4).enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(&self.value_text(*a));
            }
            if d.args.len() > 4 {
                out.push_str(", …");
            }
            out.push(']');
        }
        if let Some(w) = d.wait_on() {
            let _ = write!(out, " waits={}", w.kind);
            match &w.on {
                Subj::Node(x) => {
                    let xr = self.rec(*x);
                    let _ = write!(out, " on=n{}({})", xr.id.load(Ordering::Acquire), st_name(xr.st()));
                }
                Subj::Env(f) => {
                    let _ = write!(out, " on=env:{}", self.value_text(f.0));
                }
                Subj::Child(h) => {
                    let _ = write!(out, " on=child#{h:x}");
                }
            }
            if let Some(f) = w.field {
                let _ = write!(out, " field={}", self.value_text(f.0));
            }
            if let Some(p) = &w.prefix {
                let _ = write!(out, " prefix={p:?}");
            }
        }
        #[cfg(feature = "observe")]
        if let Some(n) = super::lock(&self.selects).get(&s) {
            let _ = write!(out, " selected={n}");
        }
        out.push('\n');
    }

    /// The scheduler's state as text.
    pub(crate) fn dump(&self) -> String {
        let mut out = String::new();
        let c = &self.c;
        let _ = writeln!(
            out,
            "counters: runs={} commits={} stops={} raises={} pending_waits={} resumed={} restarted={} speculations={} mispredictions={} spec_stops={} created={} freed={} gated={}",
            c.runs.load(Ordering::Relaxed),
            c.commits.load(Ordering::Relaxed),
            c.stops.load(Ordering::Relaxed),
            c.raises.load(Ordering::Relaxed),
            c.pending_waits.load(Ordering::Relaxed),
            c.resumed.load(Ordering::Relaxed),
            c.restarted.load(Ordering::Relaxed),
            c.speculations.load(Ordering::Relaxed),
            c.mispredictions.load(Ordering::Relaxed),
            c.spec_stops.load(Ordering::Relaxed),
            c.created.load(Ordering::Relaxed),
            c.freed.load(Ordering::Relaxed),
            c.gated.load(Ordering::Relaxed),
        );
        let _ = writeln!(
            out,
            "workers: busy={} sleeping={} work={} qlen={} stepped={} abort={}",
            self.busy.load(Ordering::Relaxed),
            self.sleepers.load(Ordering::Relaxed),
            self.work.load(Ordering::Relaxed),
            self.qlen.load(Ordering::Relaxed),
            self.stepped(),
            self.abort.load(Ordering::Relaxed),
        );
        // The ready queue, in pick order where it has one.
        let mut queued: Vec<Slot> = Vec::new();
        if let Ok(q) = self.q.try_lock() {
            let mut cur: Vec<_> = q.cur.iter().map(|r| r.0).collect();
            cur.sort_unstable();
            queued.extend(cur.into_iter().map(|(_, s)| s));
            queued.extend(q.next.iter().copied());
        } else {
            out.push_str("queue: (locked)\n");
        }
        for m in &*self.shards {
            if let Ok(h) = m.try_lock() {
                let mut v: Vec<_> = h.iter().map(|r| r.0).collect();
                v.sort_unstable();
                queued.extend(v.into_iter().map(|(_, s)| s));
            }
        }
        let _ = writeln!(out, "queued: {}", queued.len());
        for &s in queued.iter().take(LIST) {
            self.dump_node(&mut out, s);
        }
        // Every node by state; the ones in work listed.
        let mut by_state = [0usize; 9];
        let mut in_work: Vec<Slot> = Vec::new();
        for i in 0..self.recs.len() {
            #[allow(clippy::cast_possible_truncation)]
            let s = i as Slot;
            let st = self.rec(s).st();
            if let Some(n) = by_state.get_mut(st as usize) {
                *n += 1;
            }
            if super::in_work(st) {
                in_work.push(s);
            }
        }
        out.push_str("states:");
        for (st, n) in by_state.iter().enumerate() {
            if *n > 0 {
                #[allow(clippy::cast_possible_truncation)]
                let _ = write!(out, " {}={n}", st_name(st as u8));
            }
        }
        out.push('\n');
        let _ = writeln!(out, "in work: {}", in_work.len());
        // Parked first: they are the ones that can starve.
        in_work.sort_by_key(|&s| (self.rec(s).st() != S_PARK, self.rec(s).label.load(Ordering::Relaxed)));
        for &s in in_work.iter().take(LIST) {
            self.dump_node(&mut out, s);
        }
        #[cfg(feature = "observe")]
        {
            let sel = super::lock(&self.selects);
            let mut top: Vec<(Slot, u32)> = sel.iter().map(|(&s, &n)| (s, n)).collect();
            top.sort_unstable_by_key(|&(s, n)| (std::cmp::Reverse(n), s));
            let _ = writeln!(out, "most selected (of {} nodes):", top.len());
            drop(sel);
            for &(s, _) in top.iter().take(20) {
                self.dump_node(&mut out, s);
            }
        }
        out
    }
}
