//! A format's bindings as the prelude cache keeps them: its values each
//! once, a table of distinct nodes (a list's parts before it), and its
//! bindings by field. Loading one copies the table and finds where each
//! node starts: no value is decoded, and none is interned, until a job
//! asks for it ([`FormatEnv::value`]), each node then once per job
//! ([`Made`]), a value two bindings share made once.

use std::sync::OnceLock;

use phitex_defuse::{Field, FxHashMap, Value, ValueId, Values};

/// The node tags.
const UNIT: u8 = 0;
const BOOL: u8 = 1;
const INT: u8 = 2;
const STR: u8 = 3;
const NIL: u8 = 4;
const LIST: u8 = 5;
const SEQ: u8 = 6;

/// A format's bindings ([`crate::api`]'s prelude environment).
#[derive(Default)]
pub(crate) struct FormatEnv {
    /// The nodes, serialised in order: node `k` at `offs[k]`.
    nodes: Vec<u8>,
    offs: Vec<u32>,
    /// By field (a hash of its name, [`name_key`]: a field's id is its
    /// value store's), each field once: the key, its name's node, its
    /// value's node.
    binds: Vec<(u64, u32, u32)>,
}

/// A field's key in [`FormatEnv`]: its name's hash.
fn name_key(name: &str) -> u64 {
    phitex_defuse::hash_bytes(0, name.as_bytes())
}

/// The values a job made of a format's nodes, by node (each once).
pub(crate) struct Made(Vec<OnceLock<ValueId>>);

impl Made {
    /// The values made so far, for a collection ([`crate::collect`]):
    /// each is handed out again by its node.
    pub(crate) fn roots(&self, keep: &mut dyn FnMut(ValueId)) {
        self.0.iter().filter_map(OnceLock::get).for_each(|&x| keep(x));
    }
}

impl FormatEnv {
    /// The bindings `env` (name, value in `v`), the last of a name kept.
    pub(crate) fn of(v: &Values, env: &[(String, ValueId)]) -> FormatEnv {
        let mut w = Writer {
            out: FormatEnv::default(),
            memo: FxHashMap::default(),
        };
        let mut binds: Vec<(u64, u32, u32)> = env
            .iter()
            .map(|(name, x)| {
                let f = v.field(name);
                let n = w.node(v, f.0);
                let k = w.node(v, *x);
                (name_key(name), n, k)
            })
            .collect();
        // (the last given of a field first among its own, which the
        // dedup keeps)
        binds.reverse();
        binds.sort_by_key(|b| b.0);
        binds.dedup_by_key(|b| b.0);
        let mut out = w.out;
        out.binds = binds;
        out
    }

    /// How many bindings.
    pub(crate) fn len(&self) -> usize {
        self.binds.len()
    }

    /// A fresh record of the values made of these nodes, for one job.
    pub(crate) fn made(&self) -> Made {
        Made((0..self.offs.len()).map(|_| OnceLock::new()).collect())
    }

    /// The bindings as (field, value) in `v`, by field id.
    pub(crate) fn bindings(&self, v: &Values, made: &Made, range: std::ops::Range<usize>) -> Vec<(Field, ValueId)> {
        self.binds[range]
            .iter()
            .map(|&(_, n, k)| (Field(self.value(v, n, made)), self.value(v, k, made)))
            .collect()
    }

    /// The value bound to `name`, made in `v`.
    pub(crate) fn get(&self, v: &Values, name: &str, made: &Made) -> Option<ValueId> {
        let f = name_key(name);
        let i = self.binds.binary_search_by_key(&f, |b| b.0).ok()?;
        Some(self.value(v, self.binds[i].2, made))
    }

    /// The value bound to field `f`, made in `v` (none: the format binds
    /// no such field).
    pub(crate) fn get_field(&self, v: &Values, f: Field, made: &Made) -> Option<ValueId> {
        let k = v.with_str(f.0, name_key)?;
        let i = self.binds.binary_search_by_key(&k, |b| b.0).ok()?;
        Some(self.value(v, self.binds[i].2, made))
    }

    /// The names bound, in the bindings' order.
    pub(crate) fn names(&self) -> impl Iterator<Item = &str> + '_ {
        self.binds.iter().filter_map(|&(_, n, _)| self.str_at(n))
    }

    /// Node `k` as a string, when it is one.
    fn str_at(&self, k: u32) -> Option<&str> {
        let o = *self.offs.get(k as usize)? as usize;
        if self.nodes[o] != STR {
            return None;
        }
        let n = u32_at(&self.nodes, o + 1)? as usize;
        std::str::from_utf8(self.nodes.get(o + 5..o + 5 + n)?).ok()
    }

    /// Node `k` made in `v` (its parts first), once per `made`.
    pub(crate) fn value(&self, v: &Values, k: u32, made: &Made) -> ValueId {
        if let Some(&x) = made.0[k as usize].get() {
            return x;
        }
        let b = &self.nodes;
        let o = self.offs[k as usize] as usize;
        let parts = |o: usize| -> Vec<ValueId> {
            let n = u32_at(b, o + 1).unwrap_or(0) as usize;
            (0..n)
                .map(|i| self.value(v, u32_at(b, o + 5 + 4 * i).unwrap_or(0), made))
                .collect()
        };
        let x = match b[o] {
            UNIT => ValueId::UNIT,
            BOOL => v.bool(b[o + 1] != 0),
            INT => v.int(i64::from_le_bytes(
                b[o + 1..o + 9].try_into().unwrap_or([0; 8]),
            )),
            STR => v.str(self.str_at(k).unwrap_or("")),
            LIST => v.list(parts(o)),
            SEQ => {
                let mut s = ValueId::NIL;
                for e in parts(o) {
                    s = v.seq_concat(s, v.seq_unit(e));
                }
                s
            }
            _ => ValueId::NIL,
        };
        let _ = made.0[k as usize].set(x);
        x
    }

    /// As bytes: the nodes, then the bindings.
    pub(crate) fn write(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&(self.offs.len() as u64).to_le_bytes());
        out.extend_from_slice(&(self.nodes.len() as u64).to_le_bytes());
        out.extend_from_slice(&self.nodes);
        out.extend_from_slice(&(self.binds.len() as u64).to_le_bytes());
        for &(f, n, k) in &self.binds {
            out.extend_from_slice(&f.to_le_bytes());
            out.extend_from_slice(&n.to_le_bytes());
            out.extend_from_slice(&k.to_le_bytes());
        }
    }

    /// What [`FormatEnv::write`] wrote at the start of `b`, and the rest
    /// of `b`; `None` for anything else (a list's part not before it, a
    /// binding's node out of the table, bindings not by field).
    pub(crate) fn read(b: &[u8]) -> Option<(FormatEnv, &[u8])> {
        let count = usize::try_from(u64_at(b, 0)?).ok()?;
        let len = usize::try_from(u64_at(b, 8)?).ok()?;
        let nodes = b.get(16..16 + len)?.to_vec();
        let mut rest = b.get(16 + len..)?;
        let mut offs = Vec::with_capacity(count);
        let mut o = 0usize;
        while o < nodes.len() {
            let k = u32::try_from(offs.len()).ok()?;
            offs.push(u32::try_from(o).ok()?);
            o += match *nodes.get(o)? {
                UNIT | NIL => 1,
                BOOL => 2,
                INT => 9,
                STR => 5 + u32_at(&nodes, o + 1)? as usize,
                LIST | SEQ => {
                    let n = u32_at(&nodes, o + 1)? as usize;
                    for i in 0..n {
                        if u32_at(&nodes, o + 5 + 4 * i)? >= k {
                            return None;
                        }
                    }
                    5 + 4 * n
                }
                _ => return None,
            };
        }
        if o != nodes.len() || offs.len() != count {
            return None;
        }
        let n = usize::try_from(u64_at(rest, 0)?).ok()?;
        rest = rest.get(8..)?;
        let mut binds = Vec::with_capacity(n.min(rest.len() / 16));
        for _ in 0..n {
            let f = u64_at(rest, 0)?;
            let (nm, k) = (u32_at(rest, 8)?, u32_at(rest, 12)?);
            if nm as usize >= count || k as usize >= count || binds.last().is_some_and(|l: &(u64, u32, u32)| l.0 >= f) {
                return None;
            }
            binds.push((f, nm, k));
            rest = rest.get(16..)?;
        }
        // (a name's node a string: its bytes are read as UTF-8 when a
        // job makes it, [`FormatEnv::value`], not all of them here)
        let env = FormatEnv { nodes, offs, binds };
        if env.binds.iter().any(|&(_, n, _)| env.nodes[env.offs[n as usize] as usize] != STR) {
            return None;
        }
        Some((env, rest))
    }
}

fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}

fn u64_at(b: &[u8], o: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(o..o + 8)?.try_into().ok()?))
}

/// [`FormatEnv::of`]'s table being written: each value's node once.
struct Writer {
    out: FormatEnv,
    memo: FxHashMap<ValueId, u32>,
}

impl Writer {
    /// The node of `x` (its parts' first).
    fn node(&mut self, v: &Values, x: ValueId) -> u32 {
        if let Some(&k) = self.memo.get(&x) {
            return k;
        }
        let parts = |w: &mut Writer, xs: Vec<ValueId>| -> Vec<u32> {
            xs.into_iter().map(|y| w.node(v, y)).collect()
        };
        let (tag, body): (u8, Vec<u8>) = match &*v.get(x) {
            Value::Unit => (UNIT, Vec::new()),
            Value::Bool(b) => (BOOL, vec![u8::from(*b)]),
            Value::Int(i) => (INT, i.to_le_bytes().to_vec()),
            Value::Str(s) => {
                let mut b = u32::try_from(s.len()).unwrap_or(u32::MAX).to_le_bytes().to_vec();
                b.extend_from_slice(s.as_bytes());
                (STR, b)
            }
            Value::Nil => (NIL, Vec::new()),
            Value::List(xs) => {
                let ks = parts(self, xs.to_vec());
                (LIST, list_body(&ks))
            }
            Value::Seq(_) => {
                let ks = parts(self, v.seq_to_vec(x));
                (SEQ, list_body(&ks))
            }
        };
        let k = u32::try_from(self.out.offs.len()).unwrap_or(u32::MAX);
        self.out
            .offs
            .push(u32::try_from(self.out.nodes.len()).unwrap_or(u32::MAX));
        self.out.nodes.push(tag);
        self.out.nodes.extend_from_slice(&body);
        self.memo.insert(x, k);
        k
    }
}

fn list_body(ks: &[u32]) -> Vec<u8> {
    let mut b = u32::try_from(ks.len()).unwrap_or(u32::MAX).to_le_bytes().to_vec();
    for k in ks {
        b.extend_from_slice(&k.to_le_bytes());
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_format_env_reads_back_what_it_wrote() {
        let v = Values::new();
        let t = v.list(vec![v.int(-3), v.str("x"), v.list(vec![v.str("x")]), ValueId::UNIT, ValueId::NIL]);
        let s = v.seq_concat(v.seq_unit(v.int(1)), v.seq_unit(t));
        let env = vec![
            ("cs:a".to_owned(), t),
            ("cs:b".to_owned(), s),
            ("cs:a".to_owned(), v.bool(true)),
        ];
        let f = FormatEnv::of(&v, &env);
        assert_eq!(f.len(), 2);
        let mut b = Vec::new();
        f.write(&mut b);
        b.push(7);
        let (g, rest) = FormatEnv::read(&b).expect("read back");
        assert_eq!(rest, &[7]);
        let mut c = Vec::new();
        g.write(&mut c);
        assert_eq!(c, b[..b.len() - 1]);
        // (in another table: the same values by content; each store gives
        // its plain values ids of its own)
        let w = Values::new();
        let m = g.made();
        assert_eq!(g.get(&w, "cs:a", &m), Some(v.bool(true)));
        assert_eq!(g.get(&w, "cs:b", &m).map(|x| w.render(x)), Some(v.render(s)));
        assert_eq!(g.get(&w, "cs:c", &m), None);
        let mut names: Vec<&str> = g.names().collect();
        names.sort_unstable();
        assert_eq!(names, ["cs:a", "cs:b"]);
        // (a table cut short is refused)
        let mut bad = b.clone();
        bad.truncate(b.len() - 1);
        assert!(FormatEnv::read(&bad[..20]).is_none());
    }
}
