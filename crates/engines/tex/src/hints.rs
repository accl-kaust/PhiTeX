//! What a node may define, read from its text by the syntactic pass
//! (`phitex-source`): the engine then makes readers of other fields wait
//! on nothing of it.
//!
//! A node's text is one command (its first atom: a control sequence, a
//! run of characters, a brace, a blank line). The pass bounds the
//! control sequences a command may define (`cs:` and `act:` fields: its
//! own statement and the bodies of the macros it calls, when those are
//! visible in the same text); every other field the run may set (the
//! lists, the page, fonts, conditionals: the engine's state) is covered
//! by prefixes, so the answer is [`MayDefine::Prefixes`] of the state
//! heads and the exact names, or [`MayDefine::Any`] where the pass
//! cannot bound it (a macro it has not seen, `\input`, a computed name).
//!
//! The pass's answer holds under its assumptions: the catcodes of the
//! bytes it read, and that the definer and the macros whose bodies it
//! read still have those meanings. [`verify`] checks them when the node
//! runs; a difference is a diagnostic ("hint regime violated"), and a
//! definition outside the bound is one too ("undeclared definition"),
//! both counted in [`crate::Stats`]. Neither changes the output: the
//! engine re-binds uses on every definition it sees, hints only decide
//! what waits.
//!
//! Where the text comes from: a document's or prelude's slices are
//! analysed whole in [`text_hints`] (the regime tracked across the text:
//! `\makeatletter`, `\ExplSyntaxOn`, literal `\catcode`s) and each slice's
//! hint goes into its node's arguments ([`hint_arg`]); a file read by
//! `\input` is analysed once by its text ([`file_hints`]) when its first
//! node asks, and the rest of its lines (nodes whose text frame names
//! the file and an offset) find it by name. Expansion tokens carry no
//! text position and answer `Any`.
//!
//! `PHITEX_HINTS=1` prints each distinct node text's answer and cause;
//! `PHITEX_HINTS_OFF=1` answers `Any` for every node (the comparison).

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use phitex_defuse::{MayDefine, Value, ValueId, Values};
use phitex_source::pass::{self, Atom, AtomKind, Def, DefSet, Definitions, FileKind, Pass, Scope};

use crate::run::{R, Run};
use crate::val::{Mean, Tok, int_of, list_of, str_of};

/// What a node's text says it may define.
#[derive(Clone, Debug, Default)]
pub(crate) struct Hint {
    /// The bound, or `None`: anything.
    pub bound: Option<Bound>,
    /// Why it is unbounded, or how it was bounded.
    pub cause: String,
    /// The text, shortened.
    pub excerpt: String,
}

/// A bounded definition set and what it rests on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Bound {
    /// The engine's fields (`cs:name`, `act:code`).
    pub names: Vec<String>,
    /// Prefixes of fields (`cs:name` for `\csname name…`).
    pub prefixes: Vec<String>,
    /// Bytes and the catcode each was read with.
    pub cats: Vec<(u8, u8)>,
    /// Meaning fields (`cs:name`, `act:code`) and the class assumed: 0
    /// a primitive, 1 anything defined.
    pub means: Vec<(String, u8)>,
    /// The state fields (prefixes) the command's class may set, or
    /// `None`: every state head ([`state_prefixes`]).
    pub state: Option<Vec<String>>,
}

/// The state fields a command of this name sets, when they are known
/// to be few (observed on the engine: `PHITEX_HINTS=2` prints each
/// node's definitions); `None`: every state head.
fn state_of(name: &str) -> Option<Vec<String>> {
    let few: &[&str] = match name {
        // (a body read verbatim, a token taken as it is: nothing expands)
        "def" | "gdef" | "let" | "futurelet" | "tex_def:D" | "tex_gdef:D" | "tex_let:D"
        | "tex_futurelet:D" => &["aftertoken"],
        // (expansion while reading: conditionals may end)
        "edef" | "xdef" | "csname" | "cs:w" | "tex_edef:D" | "tex_xdef:D" | "tex_csname:D" => {
            &["aftertoken", "ifstack"]
        }
        "relax" | "tex_relax:D" => &[],
        _ => return None,
    };
    Some(few.iter().map(|s| (*s).to_owned()).collect())
}

/// The heads of every field a run may set besides the control sequences:
/// each lowercase letter but `c` and `a`, and those two followed by any
/// letter but the `s` of `cs:` and the `c` of `act:`.
fn state_prefixes() -> &'static [String] {
    static P: OnceLock<Vec<String>> = OnceLock::new();
    P.get_or_init(|| {
        let mut v = Vec::new();
        for c in b'a'..=b'z' {
            if c != b'c' && c != b'a' {
                v.push((c as char).to_string());
            }
        }
        for c in (b'A'..=b'Z')
            .chain(b'0'..=b'9')
            .chain([b'_', b'@', b'.', b'-', b'/', b':'])
        {
            v.push((c as char).to_string());
        }
        for c in b'a'..=b'z' {
            if c != b's' {
                v.push(format!("c{}", c as char));
            }
            if c != b'c' {
                v.push(format!("a{}", c as char));
            }
        }
        v
    })
}

/// Whether `f` is a control sequence's or active character's meaning.
fn is_cs_field(f: &str) -> bool {
    f.starts_with("cs:") || f.starts_with("act:")
}

impl Hint {
    fn any(cause: impl Into<String>, excerpt: &str) -> Hint {
        Hint {
            bound: None,
            cause: cause.into(),
            excerpt: short(excerpt),
        }
    }

    /// The engine's answer.
    pub(crate) fn may_define(&self) -> MayDefine {
        match &self.bound {
            None => MayDefine::Any,
            Some(b) => {
                let mut ps: Vec<String> = match &b.state {
                    Some(st) => st.clone(),
                    None => state_prefixes().to_vec(),
                };
                ps.extend(b.names.iter().cloned());
                ps.extend(b.prefixes.iter().cloned());
                MayDefine::Prefixes(ps)
            }
        }
    }

    /// `A` or `B`, the cause, the excerpt, then the bound's parts;
    /// records by `\x1e`, items by `\x1f`, pairs by `\x1d`.
    fn encode(&self) -> String {
        let mut o = String::new();
        match &self.bound {
            None => {
                o.push('A');
                o.push('\x1e');
                o.push_str(&self.cause);
                o.push('\x1e');
                o.push_str(&self.excerpt);
            }
            Some(b) => {
                o.push('B');
                o.push('\x1e');
                o.push_str(&self.cause);
                o.push('\x1e');
                o.push_str(&self.excerpt);
                o.push('\x1e');
                o.push_str(&b.names.join("\x1f"));
                o.push('\x1e');
                o.push_str(&b.prefixes.join("\x1f"));
                o.push('\x1e');
                let cats: Vec<String> = b.cats.iter().map(|(c, k)| format!("{c}\x1d{k}")).collect();
                o.push_str(&cats.join("\x1f"));
                o.push('\x1e');
                let means: Vec<String> =
                    b.means.iter().map(|(n, k)| format!("{n}\x1d{k}")).collect();
                o.push_str(&means.join("\x1f"));
                o.push('\x1e');
                match &b.state {
                    None => o.push('*'),
                    Some(st) => o.push_str(&st.join("\x1f")),
                }
            }
        }
        o
    }

    fn decode(s: &str) -> Hint {
        let mut it = s.split('\x1e');
        let tag = it.next().unwrap_or("");
        let cause = it.next().unwrap_or("").to_owned();
        let excerpt = it.next().unwrap_or("").to_owned();
        if tag != "B" {
            return Hint {
                bound: None,
                cause: if cause.is_empty() {
                    "hint not decodable".to_owned()
                } else {
                    cause
                },
                excerpt,
            };
        }
        let items = |s: &str| -> Vec<String> {
            if s.is_empty() {
                Vec::new()
            } else {
                s.split('\x1f').map(str::to_owned).collect()
            }
        };
        let names = items(it.next().unwrap_or(""));
        let prefixes = items(it.next().unwrap_or(""));
        let cats = items(it.next().unwrap_or(""))
            .iter()
            .filter_map(|p| {
                let (a, b) = p.split_once('\x1d')?;
                Some((a.parse().ok()?, b.parse().ok()?))
            })
            .collect();
        let means = items(it.next().unwrap_or(""))
            .iter()
            .filter_map(|p| {
                let (a, b) = p.split_once('\x1d')?;
                Some((a.to_owned(), b.parse().ok()?))
            })
            .collect();
        let state = match it.next().unwrap_or("*") {
            "*" => None,
            st => Some(items(st)),
        };
        Hint {
            bound: Some(Bound {
                names,
                prefixes,
                cats,
                means,
                state,
            }),
            cause,
            excerpt,
        }
    }
}

fn short(s: &str) -> String {
    let t: String = s.chars().take(40).collect();
    t.replace('\n', "\\n")
}

// ---------------------------------------------------------------------
// From the pass.

/// The pass's field names as the engine's: `cs:\name` is `cs:name`, an
/// active character `cs:c` is `act:code`; a field of another family is
/// the engine's state (covered by the prefixes).
fn engine_field(f: &str) -> Option<String> {
    let rest = f.strip_prefix("cs:")?;
    if let Some(name) = rest.strip_prefix('\\') {
        return Tok::cs(name).field_name();
    }
    let mut cs = rest.chars();
    match (cs.next(), cs.next()) {
        (Some(c), None) => Tok::ch(13, u32::from(c)).field_name(),
        _ => Tok::cs(rest).field_name(),
    }
}

/// A prefix of the pass's as the engine's (`cs:\pre` → `cs:pre`).
fn engine_prefix(f: &str) -> Option<String> {
    let rest = f.strip_prefix("cs:")?;
    let name = rest.strip_prefix('\\').unwrap_or(rest);
    Some(format!("cs:{name}"))
}

/// Names that read on by expansion before their command is known: the
/// command run here is computed.
const LOOKTHROUGH: &[&str] = &[
    "expandafter",
    "exp_after:wN",
    "noexpand",
    "exp_not:N",
    "exp_not:n",
    "afterassignment",
    "aftergroup",
    "futurelet",
    "lowercase",
    "uppercase",
    "scantokens",
];

/// Prefixes of a command (§1211): the command after them runs here.
fn is_prefix(name: &str) -> bool {
    matches!(
        name,
        "global"
            | "long"
            | "outer"
            | "protected"
            | "tex_global:D"
            | "tex_long:D"
            | "tex_outer:D"
            | "tex_protected:D"
    )
}

/// One text analysed whole: its slices (for the document's nodes), and
/// the hint of the command at each atom.
pub(crate) struct TextHints {
    pub slices: Vec<(usize, usize)>,
    /// Byte starts of the atoms, and the encoded hint of the command
    /// beginning there.
    starts: Vec<usize>,
    hints: Vec<Arc<str>>,
    /// Per slice.
    slice_hints: Vec<Arc<str>>,
}

impl TextHints {
    /// The hint of the command at or after byte `off`.
    fn at(&self, off: usize) -> Hint {
        let k = self.starts.partition_point(|&s| s < off);
        match self.hints.get(k) {
            Some(h) => Hint::decode(h),
            None => Hint::any("end of text", ""),
        }
    }

    fn slice_hint(&self, start: usize) -> Option<&Arc<str>> {
        let k = self.slices.partition_point(|&(s, _)| s < start);
        (self.slices.get(k).map(|x| x.0) == Some(start)).then(|| &self.slice_hints[k])
    }
}

/// The slices of a text: nodes start at atoms (characters run together),
/// each with the blanks before the next.
fn slices_of(text: &str, p: &Pass) -> Vec<(usize, usize)> {
    let mut starts: Vec<usize> = Vec::new();
    let mut prev_char = false;
    for a in &p.atoms {
        let ch = matches!(a.kind, AtomKind::Char(_) | AtomKind::Verb);
        if !(ch && prev_char) {
            starts.push(a.start);
        }
        prev_char = ch;
    }
    if starts.first() != Some(&0) {
        starts.insert(0, 0);
    }
    starts.dedup();
    let mut out = Vec::with_capacity(starts.len());
    for (i, &s) in starts.iter().enumerate() {
        let e = starts.get(i + 1).copied().unwrap_or(text.len());
        if e > s {
            out.push((s, e));
        }
    }
    out
}

/// The analysis of one text under `kind`'s regime.
struct Analysis<'t> {
    text: &'t str,
    p: Pass,
    defs: Definitions,
    /// Item index by atom.
    item_of: HashMap<usize, usize>,
}

impl Analysis<'_> {
    fn new(text: &str, kind: FileKind) -> Analysis<'_> {
        let mut defs = Definitions::new();
        defs.harvest(text, kind, 0);
        let empty = Definitions::new();
        let scope = Scope {
            document: &defs,
            file: 0,
            prelude: &empty,
        };
        let p = pass::analyze(text, kind, scope);
        let item_of = p
            .items
            .iter()
            .enumerate()
            .map(|(i, it)| (it.atom, i))
            .collect();
        Analysis {
            text,
            p,
            defs,
            item_of,
        }
    }

    fn atoms(&self) -> &[Atom] {
        &self.p.atoms
    }

    /// The bytes an atom is read from, with the catcodes of its regime.
    fn cats_of(&self, a: usize, out: &mut Vec<(u8, u8)>) {
        let Some(at) = self.atoms().get(a) else {
            return;
        };
        let r = self.p.regime_at(at.tok);
        let bytes: Vec<u8> = match at.kind {
            AtomKind::Cs
            | AtomKind::Char(_)
            | AtomKind::Open
            | AtomKind::Close
            | AtomKind::Param(_) => self.text.as_bytes()[at.start..at.end].to_vec(),
            AtomKind::Space => vec![b' '],
            AtomKind::Par | AtomKind::Verb => Vec::new(),
        };
        for b in bytes {
            if b < 128 && (b == 9 || (32..127).contains(&b)) {
                let c = r.catcode(b);
                if !out.contains(&(b, c)) {
                    out.push((b, c));
                }
            }
        }
    }

    /// The hint of the command whose first atom is `a`.
    fn hint(&self, a: usize) -> Hint {
        let atoms = self.atoms();
        let Some(at) = atoms.get(a) else {
            return Hint::any("end of text", "");
        };
        let excerpt = &self.text[at.start..self.text.len().min(at.start + 40)];
        let mut b = Bound::default();
        let mut cause = String::new();
        let ok = match at.kind {
            AtomKind::Cs => self.cs_hint(a, &mut b, &mut cause),
            AtomKind::Char(_) | AtomKind::Verb => {
                // (a run of characters: the active ones expand here, and
                // the token after the run is looked at, §1038)
                let mut k = a;
                let mut ok = true;
                while let Some(x) = atoms.get(k)
                    && matches!(x.kind, AtomKind::Char(_) | AtomKind::Verb)
                {
                    self.cats_of(k, &mut b.cats);
                    if let AtomKind::Char(c) = x.kind
                        && self.is_active(c, k)
                    {
                        ok = ok && self.call_hint(&c.to_string(), &mut b, &mut cause);
                    }
                    k += 1;
                }
                if ok && atoms.get(k).is_some_and(|x| x.kind == AtomKind::Cs) {
                    ok = self.cs_hint(k, &mut b, &mut cause);
                }
                ok
            }
            AtomKind::Par => {
                self.cats_of(a, &mut b.cats);
                self.call_hint("par", &mut b, &mut cause)
            }
            AtomKind::Open | AtomKind::Close | AtomKind::Space | AtomKind::Param(_) => {
                self.cats_of(a, &mut b.cats);
                true
            }
        };
        if ok {
            if cause.is_empty() {
                cause = "bounded".to_owned();
            }
            Hint {
                bound: Some(b),
                cause,
                excerpt: short(excerpt),
            }
        } else {
            Hint::any(cause, excerpt)
        }
    }

    fn is_active(&self, c: char, _k: usize) -> bool {
        c == '~' || (c.is_ascii() && self.defs.active_chars() >> (c as u32) & 1 == 1)
    }

    /// A macro called by name (`\par` for a blank line, an active
    /// character).
    fn call_hint(&self, name: &str, b: &mut Bound, cause: &mut String) -> bool {
        let empty = Definitions::new();
        let set = self.defs.call_defines(&empty, name);
        let field = match name.chars().next() {
            Some(c) if name.chars().count() == 1 && !c.is_alphabetic() && name != "par" => {
                Tok::ch(13, u32::from(c)).field_name()
            }
            _ => Tok::cs(name).field_name(),
        };
        if let Some(f) = field {
            b.means.push((f, 1));
        }
        add_set(&set, b, cause)
    }

    /// The hint of the control sequence at `a`.
    fn cs_hint(&self, a: usize, b: &mut Bound, cause: &mut String) -> bool {
        let atoms = self.atoms();
        let name = atoms[a].name(self.text);
        self.cats_of(a, &mut b.cats);
        if is_prefix(name) {
            // (the prefixed command runs here)
            let mut k = a + 1;
            while let Some(x) = atoms.get(k) {
                match x.kind {
                    AtomKind::Space => {}
                    AtomKind::Cs if is_prefix(x.name(self.text)) => {}
                    AtomKind::Cs => return self.cs_hint(k, b, cause),
                    _ => break,
                }
                self.cats_of(k, &mut b.cats);
                k += 1;
            }
            // (`\global` then an active character or nothing)
            return true;
        }
        if LOOKTHROUGH.contains(&name) {
            *cause = format!("\\{name}: the command run here is computed");
            return false;
        }
        let Some(&i) = self.item_of.get(&a) else {
            // (an operand of an earlier command: claimed, never run)
            *cause = format!("\\{name}: operand of an earlier command");
            return false;
        };
        let it = &self.p.items[i];
        let end = it.end.max(a + 1).min(atoms.len());
        for k in a + 1..end {
            self.cats_of(k, &mut b.cats);
        }
        let class = u8::from(!crate::prims::PRIMS.iter().any(|p| p.0 == name));
        if let Some(f) = Tok::cs(name).field_name() {
            b.means.push((f, class));
        }
        for r in &it.hints.relied {
            if let Some(f) = Tok::cs(r).field_name()
                && !b.means.iter().any(|m| m.0 == f)
            {
                b.means.push((f, 1));
            }
        }
        b.state = state_of(name);
        match &it.hints.may_define {
            pass::MayDefine::Nothing => true,
            pass::MayDefine::Names(ds) => {
                add_defs(ds, false, b);
                true
            }
            pass::MayDefine::Prefixes(ds) => {
                add_defs(ds, true, b);
                true
            }
            pass::MayDefine::Any => {
                *cause = it.hints.cause.map_or_else(
                    || format!("\\{name}: unbounded"),
                    |c| format!("\\{name}: {c}"),
                );
                if *cause == format!("\\{name}: unbounded") {
                    // (the reason the pass gives: the closure's)
                    let empty = Definitions::new();
                    let set = self.defs.call_defines(&empty, name);
                    if let Some(u) = set.unbounded.first() {
                        *cause = u.clone();
                    }
                }
                false
            }
        }
    }
}

fn add_defs(ds: &[Def], prefixes: bool, b: &mut Bound) {
    for d in ds {
        if prefixes {
            if let Some(p) = engine_prefix(&d.field)
                && !b.prefixes.contains(&p)
            {
                b.prefixes.push(p);
            }
        } else if let Some(f) = engine_field(&d.field)
            && !b.names.contains(&f)
        {
            b.names.push(f);
        }
    }
}

fn add_set(set: &DefSet, b: &mut Bound, cause: &mut String) -> bool {
    if let Some(u) = set.unbounded.first() {
        *cause = u.clone();
        return false;
    }
    for r in &set.relied {
        if let Some(f) = Tok::cs(r).field_name()
            && !b.means.iter().any(|m| m.0 == f)
        {
            b.means.push((f, 1));
        }
    }
    for f in set.names.keys() {
        if let Some(f) = engine_field(f)
            && !b.names.contains(&f)
        {
            b.names.push(f);
        }
    }
    for f in set.prefixes.keys() {
        if let Some(p) = engine_prefix(f)
            && !b.prefixes.contains(&p)
        {
            b.prefixes.push(p);
        }
    }
    true
}

fn analyse(text: &str, kind: FileKind) -> TextHints {
    let an = Analysis::new(text, kind);
    let slices = slices_of(text, &an.p);
    let mut starts = Vec::with_capacity(an.p.atoms.len());
    let mut hints: Vec<Arc<str>> = Vec::with_capacity(an.p.atoms.len());
    let mut memo: HashMap<String, Arc<str>> = HashMap::new();
    let mut intern = |h: &Hint| -> Arc<str> {
        let s = h.encode();
        memo.entry(s.clone())
            .or_insert_with(|| Arc::from(s))
            .clone()
    };
    for a in 0..an.p.atoms.len() {
        starts.push(an.p.atoms[a].start);
        let h = an.hint(a);
        hints.push(intern(&h));
    }
    let mut slice_hints = Vec::with_capacity(slices.len());
    for &(s, _) in &slices {
        let k = starts.partition_point(|&x| x < s);
        let h = match hints.get(k) {
            Some(h) if starts[k] == s => h.clone(),
            // (a slice before the first atom: blanks, a comment)
            Some(h) => h.clone(),
            None => intern(&Hint::any("end of text", "")),
        };
        slice_hints.push(h);
    }
    TextHints {
        slices,
        starts,
        hints,
        slice_hints,
    }
}

// ---------------------------------------------------------------------
// Caches.

fn hash_str(s: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    s.len().hash(&mut h);
    h.finish()
}

type Cache<K> = Mutex<HashMap<K, Arc<TextHints>>>;

fn texts() -> &'static Cache<(u64, bool)> {
    static C: OnceLock<Cache<(u64, bool)>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

fn by_name() -> &'static Cache<u64> {
    static C: OnceLock<Cache<u64>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The analysis of a document's or prelude's text (cached by the text).
pub(crate) fn text_hints(text: &str, kind: FileKind) -> Arc<TextHints> {
    let key = (hash_str(text), kind == FileKind::Package);
    if let Some(t) = texts()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&key)
    {
        return t.clone();
    }
    let t = Arc::new(analyse(text, kind));
    let mut c = texts().lock().unwrap_or_else(PoisonError::into_inner);
    if c.len() > 256 {
        c.clear();
    }
    c.insert(key, t.clone());
    t
}

/// The hint of the document slice beginning at byte `s` of `text`, as
/// the node's extra argument.
pub(crate) fn hint_arg(v: &Values, text: &str, s: usize) -> ValueId {
    let t = text_hints(text, FileKind::Document);
    match t.slice_hint(s) {
        Some(h) => v.str(h),
        None => v.str(&Hint::any("slice not found", "").encode()),
    }
}

/// The analysis of a file read by `\input`: its text value `id`, named
/// `name`.
fn file_hints(v: &Values, id: ValueId, name: ValueId) -> Arc<TextHints> {
    let text = str_of(v, id);
    let nm = str_of(v, name);
    let t = text_hints(&text, FileKind::from_path(&nm));
    by_name()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(name.raw(), t.clone());
    t
}

/// The hint of a node from its arguments: the extra argument when the
/// slice carries one, else its first text frame.
pub(crate) fn hint_of_args(v: &Values, args: &[ValueId]) -> Arc<Hint> {
    static C: OnceLock<Mutex<HashMap<u64, Arc<Hint>>>> = OnceLock::new();
    let cache = C.get_or_init(|| Mutex::new(HashMap::new()));
    let key = {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for a in args {
            a.raw().hash(&mut h);
        }
        h.finish()
    };
    if let Some(h) = cache
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&key)
    {
        return h.clone();
    }
    let h = Arc::new(compute_hint(v, args));
    let mut c = cache.lock().unwrap_or_else(PoisonError::into_inner);
    if c.len() > 1 << 20 {
        c.clear();
    }
    c.insert(key, h.clone());
    h
}

fn compute_hint(v: &Values, args: &[ValueId]) -> Hint {
    if let Some(&h) = args.get(2) {
        return Hint::decode(&str_of(v, h));
    }
    let Some(&fr) = args.first() else {
        return Hint::any("no arguments", "");
    };
    let frames = list_of(v, fr);
    let Some(&f) = frames.first() else {
        return Hint::any("no text", "");
    };
    let xs = list_of(v, f);
    let tag = xs.first().map(|&t| str_of(v, t)).unwrap_or_default();
    match tag.as_str() {
        "f" => {
            if let (Some(&id), Some(&o), Some(&name)) = (xs.get(1), xs.get(2), xs.get(3)) {
                let off = usize::try_from(int_of(v, o)).unwrap_or(0);
                let t = file_hints(v, id, name);
                return t.at(off);
            }
            Hint::any("file frame without a position", "")
        }
        "s" => {
            let text = xs.get(1).map(|&t| str_of(v, t)).unwrap_or_default();
            if let (Some(&name), Some(&o)) = (xs.get(5), xs.get(6)) {
                let at = usize::try_from(int_of(v, o)).unwrap_or(0);
                let t = by_name()
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .get(&name.raw())
                    .cloned();
                return match t {
                    Some(t) => t.at(at),
                    None => Hint::any("file not analysed yet", &text),
                };
            }
            // (the rest of a document slice: read on its own, its
            // catcodes verified when it runs)
            let mut h = text_hints(&text, FileKind::Document).at(0);
            if h.bound.is_some() {
                h.cause = format!("{} (text on its own)", h.cause);
            }
            h
        }
        "t" => {
            // (tokens already read: faithful as text when every
            // character has the catcode the regime gives it)
            let toks = crate::val::toks_of(v, xs.get(1).copied());
            let mut text = String::new();
            let mut kind = None;
            for regime in [FileKind::Document, FileKind::Package] {
                let at = regime == FileKind::Package;
                text.clear();
                let mut ok = true;
                for t in &toks {
                    match t {
                        Tok::S(n) => {
                            // (a name the pass reads back as one token:
                            // letters of the regime, or one other character)
                            let word = !n.is_empty()
                                && n.chars()
                                    .all(|c| c.is_ascii_alphabetic() || (at && c == '@'));
                            let symbol = n.chars().count() == 1
                                && n.chars().all(|c| c.is_ascii() && !c.is_ascii_alphabetic());
                            if !(word || symbol) {
                                ok = false;
                                break;
                            }
                            text.push('\\');
                            text.push_str(n);
                            if word {
                                text.push(' ');
                            }
                        }
                        Tok::C(_) => {
                            let c = t.chr();
                            let cat = t.cat().unwrap_or(12);
                            let Some(ch) = char::from_u32(c).filter(|c| c.is_ascii()) else {
                                ok = false;
                                break;
                            };
                            if plain_catcode(ch as u8, at) != cat {
                                ok = false;
                                break;
                            }
                            text.push(ch);
                        }
                    }
                }
                if ok {
                    kind = Some(regime);
                    break;
                }
            }
            match kind {
                Some(k) => {
                    let mut h = text_hints(&text, k).at(0);
                    if let Some(b) = &mut h.bound {
                        // (the tokens are read already)
                        b.cats.clear();
                        h.cause = format!("{} (expansion tokens)", h.cause);
                    }
                    h
                }
                None => Hint::any("expansion tokens with unusual catcodes", &text),
            }
        }
        _ => Hint::any("unknown frame", ""),
    }
}

/// Plain TeX's catcode of an ASCII byte (`@` a letter when `at`).
fn plain_catcode(b: u8, at: bool) -> u8 {
    match b {
        b'\\' => 0,
        b'{' => 1,
        b'}' => 2,
        b'$' => 3,
        b'&' => 4,
        13 => 5,
        b'#' => 6,
        b'^' => 7,
        b'_' => 8,
        0 => 9,
        b' ' | b'\t' => 10,
        b'~' => 13,
        b'%' => 14,
        127 => 15,
        b'@' if at => 11,
        _ if b.is_ascii_alphabetic() => 11,
        _ => 12,
    }
}

// ---------------------------------------------------------------------
// At the node.

/// Counts since the process began (the fallback when the build's own
/// counters are not read): bounded, any, violations, undeclared.
pub(crate) static COUNTS: [AtomicU64; 4] = [
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
];

static OFF: AtomicBool = AtomicBool::new(false);

/// Turns the hints off (every node may define anything, as without
/// them) or on again, for comparisons; `PHITEX_HINTS_OFF=1` does the
/// same for a process.
pub fn set_hints_enabled(on: bool) {
    OFF.store(!on, Ordering::Relaxed);
}

/// The engine's question, answered from the text.
pub(crate) fn may_define(v: &Values, args: &[ValueId]) -> MayDefine {
    if OFF.load(Ordering::Relaxed) || std::env::var_os("PHITEX_HINTS_OFF").is_some() {
        return MayDefine::Any;
    }
    let h = hint_of_args(v, args);
    if std::env::var_os("PHITEX_HINTS").is_some() {
        let answer = match &h.bound {
            None => "Any".to_owned(),
            Some(b) => format!("Prefixes(state + {:?} + {:?})", b.names, b.prefixes),
        };
        eprintln!("hint {:?}: {answer} ({})", h.excerpt, h.cause);
    }
    h.may_define()
}

/// The accumulators the nodes count into.
pub(crate) const COUNTERS: [&str; 4] = [
    "hints:bounded",
    "hints:any",
    "hints:violations",
    "hints:undeclared",
];

/// Checks the hint's assumptions against the meanings and catcodes in
/// force where the node runs: the violation, if any.
pub(crate) fn verify(r: &mut Run<'_, '_>, h: &Hint) -> R<Option<String>> {
    let Some(b) = &h.bound else {
        return Ok(None);
    };
    for &(byte, cat) in &b.cats {
        let got = r.cat(u32::from(byte))?;
        if got != cat {
            return Ok(Some(format!(
                "catcode of {:?} is {got}, the hint read it as {cat} ({:?})",
                byte as char, h.excerpt
            )));
        }
    }
    for (name, class) in &b.means {
        let x = r.get(name)?;
        let m = r.decode_mean(x);
        let ok = match class {
            0 => matches!(m, Mean::Prim(..)),
            _ => !matches!(m, Mean::Undefined),
        };
        if !ok {
            return Ok(Some(format!(
                "{name} is {} where the hint assumed {} ({:?})",
                mean_kind(&m),
                if *class == 0 {
                    "a primitive"
                } else {
                    "a definition"
                },
                h.excerpt
            )));
        }
    }
    Ok(None)
}

fn mean_kind(m: &Mean) -> &'static str {
    match m {
        Mean::Prim(..) => "a primitive",
        Mean::Macro(_) => "a macro",
        Mean::Char(..) => "a character",
        Mean::Undefined => "undefined",
        Mean::Font(_) => "a font",
        Mean::CharDef(_) | Mean::MathCharDef(_) => "a chardef",
        Mean::Reg(..) => "a register",
        Mean::NotExpanded => "not expanded",
    }
}

/// The definitions a run made outside its bound: the violation, if any.
pub(crate) fn check_defs(r: &Run<'_, '_>, h: &Hint) -> Option<String> {
    let b = h.bound.as_ref()?;
    for (f, _, _) in &r.defs {
        let name = match &*r.v.get(f.0) {
            Value::Str(s) => s.to_string(),
            _ => continue,
        };
        if !is_cs_field(&name) {
            match &b.state {
                Some(st) if !st.iter().any(|p| name.starts_with(p.as_str())) => {
                    return Some(format!(
                        "undeclared definition of {name} by {:?} (state {st:?}, {})",
                        h.excerpt, h.cause
                    ));
                }
                _ => continue,
            }
        }
        if b.names.contains(&name) || b.prefixes.iter().any(|p| name.starts_with(p.as_str())) {
            continue;
        }
        return Some(format!(
            "undeclared definition of {name} by {:?} (bound {:?} {:?}, {})",
            h.excerpt, b.names, b.prefixes, h.cause
        ));
    }
    None
}

/// `PHITEX_HINTS=2`: the node's definitions, for learning the state
/// each command sets.
pub(crate) fn debug_defs(r: &Run<'_, '_>, h: &Hint) {
    if std::env::var("PHITEX_HINTS").is_ok_and(|x| x == "2") {
        let mut fs: Vec<String> = r
            .defs
            .iter()
            .filter_map(|(f, _, _)| match &*r.v.get(f.0) {
                Value::Str(s) if !is_cs_field(s) => {
                    Some(s.split(':').next().unwrap_or(s).to_owned())
                }
                Value::Str(_) => Some("<cs>".to_owned()),
                _ => None,
            })
            .collect();
        fs.sort();
        fs.dedup();
        let first = h
            .excerpt
            .split(|c: char| c == ' ' || c == '{' || c == '#' || c == '=')
            .next()
            .unwrap_or("")
            .to_owned();
        eprintln!("defs {first}: {}", fs.join(","));
    }
}

/// Counts the node into accumulator `k` of [`COUNTERS`].
pub(crate) fn count(r: &mut Run<'_, '_>, k: usize) {
    COUNTS[k].fetch_add(1, Ordering::Relaxed);
    let one = r.v.int(1);
    r.contribute(COUNTERS[k], one);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hint_of(text: &str, at: usize) -> Hint {
        let t = analyse(text, FileKind::Document);
        t.at(at)
    }

    #[test]
    fn def_is_bounded() {
        let h = hint_of("\\def\\foo#1{x#1y}\n\\foo{a}\n", 0);
        let b = h.bound.expect("bounded");
        assert_eq!(b.names, vec!["cs:foo".to_owned()]);
        assert!(b.cats.contains(&(b'\\', 0)));
        assert!(b.cats.contains(&(b'{', 1)));
        assert!(b.means.contains(&("def".to_owned(), 0)));
        // the call: its body defines nothing
        let h = hint_of("\\def\\foo#1{x#1y}\n\\foo{a}\n", 17);
        let b = h.bound.expect("bounded");
        assert!(b.names.is_empty());
        assert!(b.means.contains(&("foo".to_owned(), 1)));
    }

    #[test]
    fn unknown_macro_is_any() {
        let h = hint_of("\\bar\n", 0);
        assert!(h.bound.is_none(), "{h:?}");
    }

    #[test]
    fn global_prefix_defines() {
        let h = hint_of("\\global\\def\\x{}\n", 0);
        let b = h.bound.expect("bounded");
        assert_eq!(b.names, vec!["cs:x".to_owned()]);
    }

    #[test]
    fn round_trip() {
        let h = hint_of("\\let\\a=\\b\n", 0);
        let d = Hint::decode(&h.encode());
        assert_eq!(d.bound, h.bound);
        let a = Hint::any("why", "text");
        assert!(Hint::decode(&a.encode()).bound.is_none());
    }

    #[test]
    fn active_char_is_assumed() {
        let h = hint_of("a~b\n", 0);
        let b = h.bound.expect("bounded: `~` defines nothing in the text");
        assert!(b.means.contains(&("act:126".to_owned(), 1)), "{b:?}");
        assert!(b.cats.contains(&(b'~', 13)));
    }

    #[test]
    fn state_prefixes_cover_the_engine_fields() {
        let md = Hint {
            bound: Some(Bound::default()),
            ..Hint::default()
        }
        .may_define();
        let MayDefine::Prefixes(ps) = md else {
            panic!("prefixes")
        };
        for f in [
            "par:3",
            "pg:1",
            "ifstack",
            "aftertoken",
            "cat:64",
            "count:10",
            "align:open",
            "fontinfo:2",
        ] {
            assert!(ps.iter().any(|p| f.starts_with(p.as_str())), "{f}");
        }
        for f in ["cs:foo", "act:126"] {
            assert!(!ps.iter().any(|p| f.starts_with(p.as_str())), "{f}");
        }
    }
}
