//! Reads taken back and relations not shared: what a client says of its
//! own reads and of its operation's nodes.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use phitex_defuse::{Ctx, Engine, Field, OpId, Operation, Report, Stop, ValueId};

/// `[s, x]`: contributes x to s.
struct Contrib;

impl Operation for Contrib {
    fn name(&self) -> &'static str {
        "contrib"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        cx.contribute(Field(cx.args()[0]), cx.args()[1]);
        Ok(cx.args()[1])
    }
}

/// `[s]`: folds s and the next contribution to it, then takes both reads
/// back: its output does not depend on them.
struct Unread(AtomicU64);

impl Operation for Unread {
    fn name(&self) -> &'static str {
        "unread"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let s = Field(cx.args()[0]);
        cx.fold(s, None, None)?;
        cx.next_contribution(s, None)?;
        cx.unread(s);
        self.0.fetch_add(1, Ordering::Relaxed);
        Ok(ValueId::UNIT)
    }
}

/// `[f, k]`: f := f + 1 as a relation where k is 0, else defines nothing:
/// each node its own text.
struct Text;

impl Operation for Text {
    fn name(&self) -> &'static str {
        "text"
    }

    fn alike(&self) -> bool {
        false
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        if cx.values().as_int(cx.args()[1]) == Some(0) {
            cx.define_rel(Field(cx.args()[0]), 1);
        }
        Ok(ValueId::UNIT)
    }
}

/// `[f]`: the definition of f reaching it.
struct Get;

impl Operation for Get {
    fn name(&self) -> &'static str {
        "get"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        Ok(cx.read(Field(cx.args()[0]))?.unwrap_or(ValueId::UNIT))
    }
}

#[test]
fn a_fold_and_a_next_taken_back_are_not_asked_again() {
    let e = Engine::new();
    let v = e.values();
    let contrib = e.register(Arc::new(Contrib));
    let unread = Arc::new(Unread(AtomicU64::new(0)));
    let u_op: OpId = e.register(unread.clone());
    let s = v.field("s");
    let u = e.new_unit();
    let c = e.push(u, contrib, vec![s.0, v.int(1)]);
    e.push(u, u_op, vec![s.0]);
    e.evaluate(2).unwrap();
    assert_eq!(unread.0.load(Ordering::Relaxed), 1);
    e.replace(c, contrib, vec![s.0, v.int(2)]);
    let r = e.evaluate(2).unwrap();
    assert_eq!(Report::by_op(&r.runs_by_op, "unread"), 0, "its reads of s were taken back: {r:?}");
    assert_eq!(unread.0.load(Ordering::Relaxed), 1);
    assert!(e.check_invariants().is_empty());
}

#[test]
fn an_operation_whose_nodes_differ_shapes_none() {
    let e = Engine::new();
    let v = e.values();
    let text = e.register(Arc::new(Text));
    let get = e.register(Arc::new(Get));
    let f = v.field("c");
    let u = e.new_unit();
    let mut gets = Vec::new();
    for i in 0..50 {
        e.push(u, text, vec![f.0, v.int(i64::from(i != 0))]);
        gets.push(e.push(u, get, vec![f.0]));
    }
    let r = e.evaluate_sequential().unwrap();
    assert_eq!(r.shaped, 0, "one node's relation is no other's: {r:?}");
    for g in gets {
        assert_eq!(e.output(g).and_then(|x| v.as_int(x)), Some(1));
    }
    assert!(e.check_invariants().is_empty());
}

/// The interleaving of `SlowClaim`'s two runs: the later node claimed,
/// then the earlier one took it.
static CLAIMED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
static TAKEN: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

fn wait_for(flag: &std::sync::atomic::AtomicU32, ms: u64) -> bool {
    let t = std::time::Instant::now();
    while flag.load(Ordering::SeqCst) == 0 {
        if t.elapsed() > std::time::Duration::from_millis(ms) {
            return false;
        }
        std::thread::yield_now();
    }
    true
}

/// `[n, role]`: claims its next n siblings; role 1 (the earlier node)
/// waits for role 2's claim first, role 2 then waits to be taken.
struct SlowClaim;

impl Operation for SlowClaim {
    fn name(&self) -> &'static str {
        "slow_claim"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let n = usize::try_from(v.as_int(cx.args()[0]).unwrap_or(0)).unwrap_or(0);
        let role = v.as_int(cx.args()[1]).unwrap_or(0);
        if role == 1 {
            wait_for(&CLAIMED, 500);
            cx.claim(n);
            TAKEN.store(1, Ordering::SeqCst);
        } else {
            cx.claim(n);
            CLAIMED.store(1, Ordering::SeqCst);
            wait_for(&TAKEN, 500);
        }
        Ok(cx.args()[0])
    }
}

/// `[us]`: spins.
struct Spin;

impl Operation for Spin {
    fn name(&self) -> &'static str {
        "spin"
    }

    fn sibling(&self) -> bool {
        false
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let t = std::time::Instant::now();
        let us = cx.values().as_int(cx.args()[0]).unwrap_or(0).unsigned_abs();
        while t.elapsed() < std::time::Duration::from_micros(us) {
            std::hint::spin_loop();
        }
        Ok(ValueId::UNIT)
    }
}

#[test]
fn a_node_taken_while_its_run_claims_gives_back_what_it_took() {
    let mut interleaved = 0;
    for _ in 0..20 {
        CLAIMED.store(0, Ordering::SeqCst);
        TAKEN.store(0, Ordering::SeqCst);
        let e = Engine::new();
        let v = e.values();
        let claim = e.register(Arc::new(SlowClaim));
        let get = e.register(Arc::new(Get));
        let spin = e.register(Arc::new(Spin));
        let f = v.field("x");
        let u = e.new_unit();
        // (work around them, so the workers start, and the two in two
        // workers' first shares: they run side by side)
        for _ in 0..8 {
            e.push(u, spin, vec![v.int(500)]);
        }
        e.push(u, claim, vec![v.int(1), v.int(1)]);
        let a = e.push(u, claim, vec![v.int(3), v.int(2)]);
        let sibs: Vec<_> = (0..3).map(|_| e.push(u, get, vec![f.0])).collect();
        for _ in 0..59 {
            e.push(u, spin, vec![v.int(500)]);
        }
        e.evaluate(8).unwrap();
        if CLAIMED.load(Ordering::SeqCst) == 1 && TAKEN.load(Ordering::SeqCst) == 1 {
            interleaved += 1;
        }
        assert_eq!(e.node_info(a).map(|i| i.state), Some(phitex_defuse::NodeState::Inert));
        for s in sibs {
            assert_eq!(e.node_info(s).map(|i| i.state), Some(phitex_defuse::NodeState::Done), "{s:?} is held by a node taken");
        }
        assert!(e.check_invariants().is_empty());
    }
    assert!(interleaved > 0, "the two claims never ran side by side");
}

/// `[s, to]`: a view on the fold of s up to node `to`, read one step in;
/// counts its runs.
struct ViewTo(std::sync::atomic::AtomicU64);

impl Operation for ViewTo {
    fn name(&self) -> &'static str {
        "view_to"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        self.0.fetch_add(1, Ordering::Relaxed);
        let v = cx.values();
        let to = phitex_defuse::NodeId::from_raw(v.as_int(cx.args()[1]).unwrap_or(0).unsigned_abs());
        let w = cx.view_through(Field(cx.args()[0]), None, to)?;
        Ok(cx.at(w, phitex_defuse::Step::Index(0)).map_or(ValueId::UNIT, |x| cx.whole(x)))
    }
}

#[test]
fn a_view_whose_bound_is_removed_runs_once_more_and_settles() {
    let e = Engine::new();
    let v = e.values();
    let contrib = e.register(Arc::new(Contrib));
    let view = Arc::new(ViewTo(AtomicU64::new(0)));
    let vo = e.register(view.clone());
    let s = v.field("s");
    let u = e.new_unit();
    let c = e.push(u, contrib, vec![s.0, v.seq_unit(v.int(1))]);
    e.push(u, vo, vec![s.0, v.int(i64::try_from(c.raw()).unwrap())]);
    e.evaluate(2).unwrap();
    assert_eq!(view.0.load(Ordering::Relaxed), 1);
    e.remove(c);
    e.set_limits(phitex_defuse::Limits { runs_per_node: 50, ..Default::default() });
    let r = e.evaluate(2);
    assert!(r.is_ok(), "the reader ran on for ever: {r:?}");
    assert_eq!(view.0.load(Ordering::Relaxed), 2, "once more, on the bound gone");
    assert!(e.check_invariants().is_empty());
}

/// The store's footprint counts what it interned, once per distinct value.
#[test]
fn the_value_store_footprint_counts_its_values() {
    let v = phitex_defuse::Values::new();
    let (n0, b0, _) = v.footprint();
    let a = v.str("abcdef");
    let l = v.list(vec![a, a, a]);
    assert_eq!(v.list(vec![a, a, a]), l);
    let (n1, b1, _) = v.footprint();
    assert_eq!(n1, n0 + 2, "a string and a list");
    assert_eq!(b1, b0 + (32 + 6) + (32 + 24));
}

/// `[x, n]`: reads x and n fields y0.. (none defined).
struct ReadMany;

impl Operation for ReadMany {
    fn name(&self) -> &'static str {
        "read_many"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let n = v.as_int(cx.args()[1]).unwrap_or(0);
        for j in 0..n {
            cx.read(v.field(&format!("y{j}")))?;
        }
        Ok(cx.read(Field(cx.args()[0]))?.unwrap_or(ValueId::UNIT))
    }
}

/// `[f, x]`: defines f := x.
struct Def;

impl Operation for Def {
    fn name(&self) -> &'static str {
        "def"
    }

    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        cx.define(Field(cx.args()[0]), cx.args()[1]);
        Ok(ValueId::UNIT)
    }
}

/// The records of a node's earlier runs are dropped where the line is
/// quiet once they outnumber those standing: what is filed stays within
/// a bound of what stands, and an evaluation that swept ends with only
/// standing records.
#[test]
fn reader_records_of_earlier_runs_are_swept_at_quiet_points() {
    let e = Engine::new();
    let v = e.values();
    let def = e.register(Arc::new(Def));
    let rd = e.register(Arc::new(ReadMany));
    let x = v.field("x");
    let u = e.new_unit();
    let d = e.push(u, def, vec![x.0, v.int(0)]);
    let r = e.push(u, rd, vec![x.0, v.int(3000)]);
    e.evaluate(2).unwrap();
    let mut sweeps = 0;
    for i in 1..80 {
        e.replace(d, def, vec![x.0, v.int(i)]);
        let rep = e.evaluate(2).unwrap();
        assert_eq!(e.output(r), Some(v.int(i)));
        let (filed, standing, _) = e.reader_records();
        if rep.readers_swept > 0 {
            sweeps += 1;
            assert_eq!(filed, standing, "after the sweep of evaluation {i}");
        }
        assert!(filed <= standing + (1 << 16) + 2 * 3001, "{filed} filed for {standing} standing");
    }
    assert!(sweeps >= 2, "{sweeps} sweeps");
    assert!(e.check_invariants().is_empty());
}

/// A node run again and again files its reads anew each time; the records
/// of its earlier runs go as a list runs out of room, so what a segment
/// holds stays a few times what stands (the course's segments held ten
/// times their readers, growing with every run: 9 M records, 1 GB).
#[test]
fn reader_records_of_earlier_runs_do_not_pile_up() {
    let e = Engine::new();
    let v = e.values();
    let def = e.register(Arc::new(Def));
    let rd = e.register(Arc::new(ReadMany));
    let x = v.field("x");
    let u = e.new_unit();
    let d = e.push(u, def, vec![x.0, v.int(0)]);
    let r = e.push(u, rd, vec![x.0, v.int(100)]);
    e.evaluate(2).unwrap();
    for i in 1..200 {
        e.replace(d, def, vec![x.0, v.int(i)]);
        e.evaluate(2).unwrap();
        assert_eq!(e.output(r), Some(v.int(i)));
    }
    let (filed, standing, _) = e.reader_records();
    assert!(standing >= 101, "{standing}");
    assert!(filed <= 8 * standing, "{filed} records filed for {standing} standing");
    assert!(e.check_invariants().is_empty());
}

#[test]
fn strings_hashed_a_word_at_a_time_intern_by_their_content() {
    // (the hash takes 8-byte words, the last padded with zeros: a string
    // and the same with zero bytes after it hash alike and intern apart)
    let e = Engine::new();
    let v = e.values();
    let mut texts: Vec<String> = (0..=24).map(|n| "abcdefghijklmnopqrstuvwxyz"[..n].to_string()).collect();
    texts.extend(["abc\0", "abc\0\0\0\0\0", "abcdefgh\0", "abcdefghabcdefgh", "abcdefgiabcdefgh", "abcdefghabcdefgi"].map(String::from));
    let ids: Vec<ValueId> = texts.iter().map(|t| v.str(t)).collect();
    for (i, t) in texts.iter().enumerate() {
        assert_eq!(v.as_str(ids[i]).as_deref(), Some(t.as_str()));
        assert_eq!(v.str(t), ids[i], "{t:?} interned twice");
        for j in 0..i {
            assert_ne!(ids[i], ids[j], "{t:?} and {:?}", texts[j]);
        }
    }
    let h = |s: &str| phitex_defuse::hash_bytes(7, s.as_bytes());
    assert_ne!(h("abcdefghabcdefgh"), h("abcdefghabcdefgi"), "a byte of the second word");
    assert_ne!(h("abcdefgh"), h("abcdefghabcdefgh"), "a second word");
}

/// The fingerprints (`hash_bytes`, `hash_words`) keep apart the inputs a
/// rotate-xor-multiply hash (FxHash) collides on by its structure: two
/// texts that differ in the high byte of one word and the low byte of the
/// next (`cs:fp_set:Nn` and `cs:fp_sub:Nn` hashed alike, and a format
/// that keyed its bindings by that hash gave `\fp_set:Nn` the meaning of
/// `\fp_sub:Nn`), every such pair over all 65536 values of the two bytes,
/// at several seeds; a text and the same text with NULs after it; a list
/// of words and the same list with a zero after it.
#[test]
fn fingerprints_keep_structured_near_inputs_apart() {
    use phitex_defuse::{hash_bytes, hash_words};
    for seed in [0u64, 1, 7, 0x6e6f_6465] {
        assert_ne!(hash_bytes(seed, b"cs:fp_set:Nn"), hash_bytes(seed, b"cs:fp_sub:Nn"));
        let mut seen = std::collections::HashSet::new();
        let mut words = std::collections::HashSet::new();
        for hi in 0..=255u8 {
            for lo in 0..=255u8 {
                let mut t = *b"cs:fp_s_?:Nn";
                t[7] = hi;
                t[8] = lo;
                assert!(seen.insert(hash_bytes(seed, &t)), "seed {seed}: bytes {hi} {lo} collide");
                let w = [0x5f70_665f_3a73_63u64 | (u64::from(hi) << 56), 0x6e4e_3a00 | u64::from(lo)];
                assert!(words.insert(hash_words(seed, &w)), "seed {seed}: words {hi} {lo} collide");
            }
        }
        assert_ne!(hash_bytes(seed, b"ab"), hash_bytes(seed, b"ab\0"));
        assert_ne!(hash_bytes(seed, b""), hash_bytes(seed, b"\0"));
        assert_ne!(hash_words(seed, &[5]), hash_words(seed, &[5, 0]));
        assert_ne!(hash_words(seed, &[]), hash_words(seed, &[0]));
    }
}
