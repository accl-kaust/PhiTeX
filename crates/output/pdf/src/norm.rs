//! A PDF file's content up to renumbering (RELAXED.md, gate 2): the
//! object graph from the trailer's `/Root` and `/Info`, objects named by
//! the order a depth-first walk with sorted keys meets them, resource
//! names by what they refer to, streams by their decoded bytes. Two files
//! that differ only in object numbers, resource names, object streams
//! or compression have equal normal forms.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::sync::Arc;

use phitex_codecs::pdfread::{Dict, Doc, Obj, Ref};

/// The normal form of a PDF file: one line per object, in walk order.
///
/// # Errors
/// A file that does not open.
pub fn normal_form(pdf: &[u8]) -> Result<Vec<String>, String> {
    let data: Arc<[u8]> = Arc::from(pdf);
    let doc = Doc::open(&data).map_err(|e| format!("{e:?}"))?;
    let mut w = Walk {
        doc: &doc,
        ids: HashMap::new(),
        out: Vec::new(),
        queue: Vec::new(),
    };
    let mut top = String::new();
    for k in [&b"Root"[..], b"Info"] {
        if let Some(o) = doc.trailer.get(k) {
            let s = w.obj(o, &BTreeMap::new());
            let _ = write!(top, "/{} {s} ", String::from_utf8_lossy(k));
        }
    }
    w.out.push((0, format!("trailer {top}")));
    while let Some((id, r, names)) = w.queue.pop() {
        let o = doc.fetch(r);
        let s = w.body(&o, &names);
        w.out.push((id, s));
    }
    w.out.sort();
    Ok(w.out
        .into_iter()
        .map(|(i, s)| format!("{i}: {s}"))
        .collect())
}

struct Walk<'d> {
    doc: &'d Doc,
    ids: HashMap<Ref, usize>,
    out: Vec<(usize, String)>,
    /// Objects met and not yet written, with the resource names their
    /// content streams use.
    #[allow(clippy::type_complexity)]
    queue: Vec<(usize, Ref, BTreeMap<Vec<u8>, String>)>,
}

impl Walk<'_> {
    /// An object's id, given in the order objects are first met.
    fn id(&mut self, r: Ref, names: &BTreeMap<Vec<u8>, String>) -> usize {
        if let Some(&i) = self.ids.get(&r) {
            return i;
        }
        let i = self.ids.len() + 1;
        self.ids.insert(r, i);
        self.queue.insert(0, (i, r, names.clone()));
        i
    }

    /// A direct object (references by id).
    fn obj(&mut self, o: &Obj, names: &BTreeMap<Vec<u8>, String>) -> String {
        match o {
            Obj::Ref(r) => {
                // a content stream's names come from the object that
                // holds it (a page or a form)
                format!("@{}", self.id(*r, names))
            }
            Obj::Array(a) => {
                let xs: Vec<String> = a.iter().map(|x| self.obj(x, names)).collect();
                format!("[{}]", xs.join(" "))
            }
            Obj::Dict(d) => self.dict(d, names),
            Obj::Real(x) => format!("{x}"),
            Obj::Int(n) => format!("{n}"),
            Obj::Str(s) => format!("({})", hex(s)),
            Obj::Name(n) => format!("/{}", String::from_utf8_lossy(n)),
            Obj::Bool(b) => format!("{b}"),
            Obj::Null => "null".to_owned(),
            other => format!("{other:?}"),
        }
    }

    /// A dictionary: keys sorted; a `/Resources` entry names its
    /// fonts and `XObjects` by what they refer to, and the dictionary's
    /// content streams use those names.
    fn dict(&mut self, d: &Dict, names: &BTreeMap<Vec<u8>, String>) -> String {
        let mut names = names.clone();
        let mut items: Vec<(Vec<u8>, String)> = Vec::new();
        if let Some(res) = d.get(b"Resources") {
            let (s, n) = self.resources(res);
            names = n;
            items.push((b"Resources".to_vec(), s));
        }
        let mut keys: Vec<&(Vec<u8>, Obj)> =
            d.0.iter()
                .filter(|(k, _)| {
                    !matches!(
                        k.as_slice(),
                        b"Resources" | b"Length" | b"Filter" | b"DecodeParms"
                    )
                })
                .collect();
        keys.sort_by(|a, b| a.0.cmp(&b.0));
        for (k, v) in keys {
            let s = self.obj(v, &names);
            items.push((k.clone(), s));
        }
        items.sort();
        let mut s = String::from("<<");
        for (k, v) in items {
            let _ = write!(s, " /{} {v}", String::from_utf8_lossy(&k));
        }
        s.push_str(" >>");
        s
    }

    /// A resources dictionary: the names of its fonts, `XObjects` and
    /// other named resources replaced by what they refer to.
    fn resources(&mut self, res: &Obj) -> (String, BTreeMap<Vec<u8>, String>) {
        let res = self.doc.follow(res);
        let Some(d) = res.as_dict() else {
            return ("null".to_owned(), BTreeMap::new());
        };
        let mut names = BTreeMap::new();
        let mut items: Vec<String> = Vec::new();
        let empty = BTreeMap::new();
        for (k, v) in &d.0 {
            let v = self.doc.follow(v);
            if let (
                b"Font" | b"XObject" | b"ExtGState" | b"ColorSpace" | b"Pattern" | b"Shading",
                Some(sub),
            ) = (k.as_slice(), v.as_dict())
            {
                let mut entries: Vec<String> = Vec::new();
                for (n, x) in &sub.0 {
                    let target = self.obj(x, &empty);
                    names.insert(n.clone(), target.clone());
                    entries.push(target);
                }
                entries.sort();
                items.push(format!(
                    "/{} {{{}}}",
                    String::from_utf8_lossy(k),
                    entries.join(" ")
                ));
            } else {
                let s = self.obj(&v, &empty);
                items.push(format!("/{} {s}", String::from_utf8_lossy(k)));
            }
        }
        items.sort();
        (format!("res<<{}>>", items.join(" ")), names)
    }

    /// An indirect object's body: a stream's dictionary and decoded
    /// bytes (with resource names replaced), or the object.
    fn body(&mut self, o: &Obj, names: &BTreeMap<Vec<u8>, String>) -> String {
        match o {
            Obj::Stream(st) => {
                let mut local = names.clone();
                if let Some(res) = st.dict.get(b"Resources") {
                    local = self.resources(res).1;
                }
                let d = self.dict(&st.dict, &local);
                let data = self.doc.decode(st);
                let data = if local.is_empty() {
                    data
                } else {
                    rename(&data, &local)
                };
                format!("stream {d} {}", hex(&data))
            }
            o => self.obj(o, names),
        }
    }
}

/// `/Name` tokens of a content stream replaced by what they refer to.
fn rename(data: &[u8], names: &BTreeMap<Vec<u8>, String>) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut i = 0;
    while i < data.len() {
        if data[i] == b'/' {
            let s = i + 1;
            let mut e = s;
            while e < data.len() && !b" \t\r\n\x0c()<>[]{}/%".contains(&data[e]) {
                e += 1;
            }
            if let Some(t) = names.get(&data[s..e]) {
                out.push(b'/');
                out.extend_from_slice(t.as_bytes());
                i = e;
                continue;
            }
            out.extend_from_slice(&data[i..e]);
            i = e;
            continue;
        }
        out.push(data[i]);
        i += 1;
    }
    out
}

fn hex(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() * 2);
    for c in b {
        let _ = write!(s, "{c:02x}");
    }
    s
}

/// The first difference between two files' normal forms.
#[must_use]
pub fn first_difference(a: &[u8], b: &[u8]) -> Option<String> {
    let (x, y) = match (normal_form(a), normal_form(b)) {
        (Ok(x), Ok(y)) => (x, y),
        (Err(e), _) => return Some(format!("first file: {e}")),
        (_, Err(e)) => return Some(format!("second file: {e}")),
    };
    for (i, (p, q)) in x.iter().zip(&y).enumerate() {
        if p != q {
            let at = p
                .bytes()
                .zip(q.bytes())
                .position(|(a, b)| a != b)
                .unwrap_or(0);
            let from = at.saturating_sub(120);
            let cut = |s: &str| {
                s.get(from..(at + 180).min(s.len()))
                    .unwrap_or("")
                    .to_owned()
            };
            return Some(format!(
                "object line {i}, at {at}:\n  {}\n  {}",
                cut(p),
                cut(q)
            ));
        }
    }
    (x.len() != y.len()).then(|| format!("{} objects, {} objects", x.len(), y.len()))
}
