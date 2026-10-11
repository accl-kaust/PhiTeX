//! The replay converter: a recorded trace and the document's source files
//! to the step format a TeX-free engine replays (see `REPLAY.md` for the
//! format and every convention, and what is approximate).
//!
//! Two streaming passes over the trace. The first joins each step to the
//! text it was read from, modelling TeX's input stack: a macro call
//! pushes its body (as the log prints it), a step matches the next
//! unconsumed atom of the same control sequence or character in the
//! innermost text that has one, the texts above it are finished (unless
//! they wait at a parameter, whose argument comes from below), and the
//! bottom of the stack is the file the log says is being read. The join
//! gives each step a position, an expansion level (for `unfold`) and the
//! syntactic pass's view of it (`consumes`, `claim`, `scope`, `cause`,
//! `may_define`). The second pass writes the steps with their reads and
//! writes.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::{self, BufRead};
use std::rc::Rc;

use phitex_source::pass::{self, AtomKind, Definitions, FileKind, GroupKind, Scope, Status};
use serde::ser::SerializeSeq;
use serde::{Serialize, Serializer};

use crate::hash::fnv1a;
use crate::step::{Kind, Record, Step};

/// The conventions the converter follows, for `meta`.
pub const CONVENTIONS: &[&str] = &[
    "text_order = file_index * 2^32 + byte_offset; files numbered in order of first appearance in the trace",
    "nesting_depth = the group level \\tracinggroups reports at the step",
    "join: a step matches the next unconsumed atom with its name (or character) in the innermost text of a modelled input stack (macro bodies as the log prints them, over the file the log says is read); window 64 atoms forward and 8 back in a body, 512 forward (resynchronised at the log's line) and 8 back in a file",
    "source.position: exact (joined to a file atom), call-site (joined to a macro body: the call's position), line (not joined: the log's line in the step's file), inherited (the previous step's)",
    "unfold.produced: a call's following steps until the next call at the same or a shallower expansion level (the level is the join's input-stack depth)",
    "consumes: macro calls Known(steps until the next macro call), Pending if the pass says the call's extent is pending or the call did not join; primitives Known(steps up to the first step joined past the pass's extent) unless the pass says Pending; null for steps that consume nothing",
    "hints.claim: the pass's status for the joined atom (Known/Predicted with n as in consumes and confirm field cs:\\name), Pending where the join failed; Known(0) for characters and groups",
    "hints.scope: direction from the step (group characters, \\begingroup/\\endgroup, \\begin/\\end); Known when the pass pairs it Known, else Predicted with the field that decides it (catcode:123/125 or cs:\\name)",
    "may_define: field names (cs:\\x) from the pass's definition harvest at the joined atom; a primitive definer's literal target; Prefixes(cs:\\lit) for a \\csname target with literal prefix lit (checked against the trace's writes); a macro call: what its own body defines directly (not through the macros it calls, whose steps carry their own); Any for a definer whose target is not literal or did not join",
    "hints.cause: the pass's cause kind; join-failed where the step did not join",
    "reads keep \"?\" (a read the log does not show); writes drop the value hash",
];

/// How a step is read from the texts: the key of the atom it matches.
const K_CS: u64 = 1 << 32;
const K_CHAR: u64 = 2 << 32;
const K_SPACE: u64 = 3 << 32;
const K_OPEN: u64 = 4 << 32;
const K_CLOSE: u64 = 5 << 32;
const K_PARAM: u64 = 6 << 32;
const K_OTHER: u64 = 7 << 32;

const NONE: u32 = u32::MAX;

/// What may be defined.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
pub enum MayDefine {
    None,
    Names(Vec<String>),
    Prefixes(Vec<String>),
    Any,
}

impl MayDefine {
    fn join(self, o: MayDefine) -> MayDefine {
        match (self, o) {
            (MayDefine::Any, _) | (_, MayDefine::Any) => MayDefine::Any,
            (MayDefine::None, x) | (x, MayDefine::None) => x,
            (MayDefine::Names(mut a), MayDefine::Names(b)) => {
                a.extend(b);
                a.sort();
                a.dedup();
                MayDefine::Names(a)
            }
            (
                MayDefine::Names(mut a) | MayDefine::Prefixes(mut a),
                MayDefine::Names(b) | MayDefine::Prefixes(b),
            ) => {
                a.extend(b);
                a.sort();
                a.dedup();
                MayDefine::Prefixes(a)
            }
        }
    }
}

/// An atom's view from the pass.
#[derive(Clone, Copy, Debug)]
struct AtomInfo {
    /// 0: no item (an operand), 1 Known, 2 Predicted, 3 Pending.
    status: u8,
    consumes: bool,
    /// The atom after the extent.
    end: u32,
    cause: u32,
    may: u32,
    /// 0 none, 1 Known, 2 Predicted.
    scope: u8,
    /// Its arguments are not executed now (a definition's body).
    skip: bool,
}

const NO_INFO: AtomInfo = AtomInfo {
    status: 0,
    consumes: false,
    end: 0,
    cause: NONE,
    may: 0,
    scope: 0,
    skip: false,
};

/// A text the join reads: a file or a macro body.
struct Text {
    keys: Vec<u64>,
    start: Vec<u32>,
    line: Vec<u32>,
    line_starts: Vec<u32>,
    info: Vec<AtomInfo>,
    /// What the whole text defines directly (a body's summary).
    summary: u32,
    dbg: String,
}

impl Text {
    fn line_of(&self, byte: u32) -> u32 {
        u32::try_from(self.line_starts.partition_point(|&s| s <= byte)).unwrap_or(NONE)
    }
}

/// A macro body on the modelled input stack.
struct Frame {
    text: Rc<Text>,
    cursor: usize,
    used: Vec<bool>,
    inst: u64,
    /// The call's position.
    order: u64,
    line: u32,
    file: u32,
}

/// A file of the document.
struct FileState {
    name: String,
    text: Option<Rc<Text>>,
    used: Vec<bool>,
    cursor: usize,
    inst: u64,
    last: u32,
}

/// What pass 1 found for a step.
#[derive(Clone, Copy, Debug)]
struct Info {
    order: u64,
    line: u32,
    file: u32,
    /// 0 exact, 1 call-site, 2 line, 3 inherited.
    precision: u8,
    /// 0 null, 1 Known(next call), 2 Known(claim), 3 Pending.
    consumes: u8,
    /// 0 Known, 1 Predicted, 2 Pending.
    claim: u8,
    claim_n: u32,
    confirm: u32,
    /// 0 none, 1 Known Open, 2 Known Close, 3 Predicted Open, 4 Predicted Close.
    scope: u8,
    scope_confirm: u32,
    may: u32,
    cause: u32,
    next_call: u64,
    unfold_end: u64,
}

/// Counts for `meta` and `INDEX.md`.
#[derive(Clone, Debug, Default, Serialize, serde::Deserialize)]
pub struct Counts {
    pub steps: u64,
    pub fields: u64,
    pub consumes_null: u64,
    pub consumes_known: u64,
    pub consumes_pending: u64,
    pub claim_known: u64,
    pub claim_predicted: u64,
    pub claim_pending: u64,
    pub may_none: u64,
    pub may_names: u64,
    pub may_prefixes: u64,
    pub may_any: u64,
    pub unfolded: u64,
    pub macro_calls: u64,
    /// Steps with a key that joined to no text.
    pub join_failures: u64,
    /// Steps the join cannot key (font selections, assignments outside
    /// any command): positioned by inheritance, not counted as failures.
    pub unkeyed: u64,
    pub joined_exact: u64,
    /// Steps that are the step before shown again after a mode change.
    pub reshown: u64,
    /// Names learned as aliases of primitives.
    pub aliases: u64,
    /// Calls printed under another `\\escapechar` whose body was taken
    /// from a call of the same macro printed with `\\`.
    pub bodies_borrowed: u64,
    /// Such calls with no body to borrow (an empty frame).
    pub bodies_hidden: u64,
    pub joined_body: u64,
    /// `may_define: Any` because the join failed.
    pub may_any_join: u64,
    /// `consumes: Pending` because the join failed.
    pub consumes_pending_join: u64,
    /// A `\csname` prefix the trace's writes contradicted (made `Any`).
    pub prefix_mismatch: u64,
    pub files: u64,
    pub files_read: u64,
    pub bodies: u64,
    pub edits: u64,
}

struct Interner {
    map: HashMap<String, u32>,
    names: Vec<String>,
}

impl Interner {
    fn new() -> Self {
        Self {
            map: HashMap::new(),
            names: Vec::new(),
        }
    }

    fn get(&mut self, s: &str) -> u32 {
        if let Some(&i) = self.map.get(s) {
            return i;
        }
        let i = u32::try_from(self.names.len()).expect("fewer than 2^32 names");
        self.map.insert(s.to_owned(), i);
        self.names.push(s.to_owned());
        i
    }
}

/// The primitives that define a control sequence named by their operand.
const DEFINERS: &[&str] = &[
    "def",
    "gdef",
    "edef",
    "xdef",
    "let",
    "futurelet",
    "chardef",
    "mathchardef",
    "countdef",
    "dimendef",
    "skipdef",
    "muskipdef",
    "toksdef",
    "font",
    "read",
    "readline",
];

/// Whether a body (as the log prints it) uses expl3 names.
fn expl_names(s: &str) -> bool {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' {
            let mut j = i + 1;
            while j < b.len() && (b[j].is_ascii_alphabetic() || b[j] == b'@') {
                j += 1;
            }
            if j > i + 1 && j < b.len() && (b[j] == b'_' || b[j] == b':') {
                return true;
            }
            i = j.max(i + 1);
        } else {
            i += 1;
        }
    }
    false
}

/// Reads a file the log names.
type Loader<'l> = Box<dyn FnMut(&str) -> Option<String> + 'l>;

/// The converter's state.
pub struct Converter<'l> {
    load: Loader<'l>,
    names: Interner,
    strings: Interner,
    defines: Vec<MayDefine>,
    define_ids: HashMap<MayDefine, u32>,
    doc: Definitions,
    files: Vec<FileState>,
    file_ids: HashMap<String, u32>,
    bodies: HashMap<u64, Rc<Text>>,
    stack: Vec<Frame>,
    /// Open claims: (text instance, end atom, step).
    claims: Vec<(u64, u32, u64)>,
    /// Open unfolds: (expansion level, step).
    calls: Vec<(u32, u64)>,
    last_call: Option<u64>,
    infos: Vec<Info>,
    next_inst: u64,
    prev: Option<(u64, u32, u32)>,
    /// Names seen as macro calls, and as primitives.
    macro_names: std::collections::HashSet<u32>,
    prim_names: std::collections::HashSet<u32>,
    /// Aliases learned: a primitive's name to the keys of the names
    /// `\let` to it (`\fi` to `\fi:`), which the log never shows.
    aliases: HashMap<u32, Vec<u64>>,
    /// Bodies by macro: the latest printed with `\\` before, else the
    /// first after (for calls printed under another `\\escapechar`).
    latest_body: HashMap<String, String>,
    pub first_body: HashMap<String, String>,
    pub counts: Counts,
}

impl<'l> Converter<'l> {
    /// A converter reading the document's files through `load` (the name
    /// as the log prints it to the file's text).
    pub fn new(load: impl FnMut(&str) -> Option<String> + 'l) -> Self {
        let mut c = Self {
            load: Box::new(load),
            names: Interner::new(),
            strings: Interner::new(),
            defines: Vec::new(),
            define_ids: HashMap::new(),
            doc: Definitions::new(),
            files: Vec::new(),
            file_ids: HashMap::new(),
            bodies: HashMap::new(),
            stack: Vec::new(),
            claims: Vec::new(),
            calls: Vec::new(),
            last_call: None,
            infos: Vec::new(),
            next_inst: 0,
            prev: None,
            macro_names: std::collections::HashSet::new(),
            prim_names: std::collections::HashSet::new(),
            aliases: HashMap::new(),
            latest_body: HashMap::new(),
            first_body: HashMap::new(),
            counts: Counts::default(),
        };
        let none = c.define(MayDefine::None);
        debug_assert_eq!(none, 0);
        c
    }

    fn define(&mut self, m: MayDefine) -> u32 {
        if let Some(&i) = self.define_ids.get(&m) {
            return i;
        }
        let i = u32::try_from(self.defines.len()).expect("fewer than 2^32");
        self.defines.push(m.clone());
        self.define_ids.insert(m, i);
        i
    }

    fn inst(&mut self) -> u64 {
        self.next_inst += 1;
        self.next_inst
    }

    /// The file's index, reading and analyzing it on first sight.
    fn file(&mut self, name: &str) -> u32 {
        if let Some(&i) = self.file_ids.get(name) {
            return i;
        }
        let i = u32::try_from(self.files.len()).expect("fewer than 2^32 files");
        self.file_ids.insert(name.to_owned(), i);
        self.counts.files += 1;
        let text = (self.load)(name).map(|t| {
            self.counts.files_read += 1;
            let kind = FileKind::from_path(name);
            self.doc.harvest(&t, kind, i);
            Rc::new(self.build(&t, kind, i, 0))
        });
        let used = vec![false; text.as_ref().map_or(0, |t| t.keys.len())];
        let inst = self.inst();
        self.files.push(FileState {
            name: name.to_owned(),
            text,
            used,
            cursor: 0,
            inst,
            last: 0,
        });
        i
    }

    /// A macro body's text (cached by its text).
    fn body(&mut self, body: &str) -> Rc<Text> {
        let h = fnv1a(body.as_bytes());
        if let Some(t) = self.bodies.get(&h) {
            return Rc::clone(t);
        }
        self.counts.bodies += 1;
        let (text, skip) = if expl_names(body) {
            let p = "\\ExplSyntaxOn ";
            (format!("{p}{body}"), p.len())
        } else {
            (body.to_owned(), 0)
        };
        let t = Rc::new(self.build(&text, FileKind::Package, NONE, skip));
        self.bodies.insert(h, Rc::clone(&t));
        t
    }

    /// The pass over a text, to the join's atoms.
    #[allow(clippy::too_many_lines)]
    fn build(&mut self, text: &str, kind: FileKind, file: u32, skip: usize) -> Text {
        let mut local = Definitions::new();
        let (defs, fid) = if file == NONE {
            local.harvest(text, kind, 0);
            (&local, 0)
        } else {
            (&self.doc, file)
        };
        let empty = Definitions::new();
        let p = pass::analyze(
            text,
            kind,
            Scope {
                document: if file == NONE { &self.doc } else { defs },
                file: if file == NONE { NONE } else { fid },
                prelude: &empty,
            },
        );
        // Definition sites in this text, by the defining atom's byte.
        let mut sites: HashMap<usize, Vec<String>> = HashMap::new();
        for (name, f, pos) in defs.sites() {
            if f == fid {
                sites.entry(pos).or_default().push(format!("cs:\\{name}"));
            }
        }
        let mut item_of = vec![NONE; p.atoms.len()];
        for (k, it) in p.items.iter().enumerate() {
            item_of[it.atom] = u32::try_from(k).unwrap_or(NONE);
        }
        let mut brace_status: HashMap<usize, Status> = HashMap::new();
        for b in &p.braces {
            brace_status.insert(b.open, b.status);
            if let Some(c) = b.close {
                brace_status.insert(c, b.status);
            }
        }
        let mut group_status: HashMap<usize, Status> = HashMap::new();
        for g in &p.groups {
            if matches!(
                g.kind,
                GroupKind::BeginGroup | GroupKind::BGroup | GroupKind::Env(_)
            ) {
                for a in [g.open, g.close].into_iter().flatten() {
                    group_status.insert(a, g.status);
                }
            }
        }
        let mut line_starts = vec![0u32];
        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(u32::try_from(i + 1).unwrap_or(NONE));
            }
        }
        let first = p.atoms.partition_point(|a| a.start < skip);
        let n = p.atoms.len() - first;
        let mut t = Text {
            keys: Vec::with_capacity(n),
            start: Vec::with_capacity(n),
            line: Vec::with_capacity(n),
            line_starts,
            info: Vec::with_capacity(n),
            summary: 0,
            dbg: if file == NONE {
                text.chars().take(300).collect()
            } else {
                String::new()
            },
        };
        let mut summary = MayDefine::None;
        #[allow(clippy::needless_range_loop)]
        for a in first..p.atoms.len() {
            let atom = &p.atoms[a];
            let key = match atom.kind {
                AtomKind::Cs => K_CS | u64::from(self.names.get(atom.name(text))),
                AtomKind::Char(c) => K_CHAR | u64::from(u32::from(c)),
                AtomKind::Space => K_SPACE,
                AtomKind::Par => K_CS | u64::from(self.names.get("par")),
                AtomKind::Open => K_OPEN,
                AtomKind::Close => K_CLOSE,
                AtomKind::Param(_) => K_PARAM,
                AtomKind::Verb => K_OTHER,
            };
            let start = u32::try_from(atom.start - skip).unwrap_or(NONE);
            let mut info = NO_INFO;
            if let Some(it) = p.items.get(item_of[a] as usize) {
                info.status = match it.status {
                    Status::Known => 1,
                    Status::Predicted => 2,
                    Status::Pending => 3,
                };
                info.consumes = it.consumes;
                info.end = u32::try_from(it.end.saturating_sub(first)).unwrap_or(NONE);
                info.cause = it.cause.as_ref().map_or(NONE, |c| self.strings.get(c.kind));
            }
            let st = match atom.kind {
                AtomKind::Open | AtomKind::Close => brace_status.get(&atom.tok).copied(),
                _ => group_status.get(&a).copied(),
            };
            info.scope = match st {
                None => 0,
                Some(Status::Known) => 1,
                Some(_) => 2,
            };
            if atom.kind == AtomKind::Cs {
                let name = atom.name(text);
                let may = if let Some(names) = sites.get(&atom.start) {
                    let mut v = names.clone();
                    v.sort();
                    v.dedup();
                    info.skip = info.status != 3;
                    MayDefine::Names(v)
                } else if DEFINERS.contains(&name) {
                    info.skip =
                        matches!(name, "def" | "gdef" | "edef" | "xdef") && info.status != 3;
                    target(&p.atoms, a + 1, text)
                } else {
                    MayDefine::None
                };
                summary = summary.join(may.clone());
                info.may = self.define(may);
            }
            t.keys.push(key);
            t.start.push(start);
            t.line.push(t.line_of(start));
            t.info.push(info);
        }
        t.summary = self.define(summary);
        t
    }

    /// The keys a step can match.
    fn keys(&mut self, s: &Step) -> Vec<u64> {
        // (a token-list parameter TeX inserts itself, `\output->`, is read
        // from no text)
        if s.kind == Kind::Macro && s.reads.first().is_some_and(|r| r.starts_with("param:")) {
            return Vec::new();
        }
        match s.kind {
            Kind::Primitive | Kind::Macro | Kind::If => {
                let Some(n) = s.name.strip_prefix('\\') else {
                    return Vec::new();
                };
                if n.is_empty() {
                    return Vec::new();
                }
                let id = self.names.get(n);
                let mut v = vec![K_CS | u64::from(id)];
                if s.kind == Kind::Macro {
                    self.macro_names.insert(id);
                } else {
                    self.prim_names.insert(id);
                    if let Some(a) = self.aliases.get(&id) {
                        v.extend_from_slice(a);
                    }
                }
                // (an active character prints as `\~`)
                let mut ch = n.chars();
                if let (Some(c), None) = (ch.next(), ch.next())
                    && !c.is_alphabetic()
                {
                    v.push(K_CHAR | u64::from(u32::from(c)));
                }
                v
            }
            Kind::Char => {
                if s.name == "blank space" {
                    vec![K_SPACE]
                } else {
                    s.ch.map_or_else(Vec::new, |c| vec![K_CHAR | u64::from(u32::from(c))])
                }
            }
            Kind::GroupBegin => vec![K_OPEN, K_CS | u64::from(self.names.get("bgroup"))],
            Kind::GroupEnd => vec![K_CLOSE, K_CS | u64::from(self.names.get("egroup"))],
            Kind::Font => Vec::new(),
        }
    }

    /// A call's body: as printed, or (printed under another escape
    /// character) the same macro's body printed with `\\` elsewhere.
    fn body_of(&mut self, s: &Step) -> String {
        let Some(c) = s.macro_call.as_ref() else {
            return String::new();
        };
        if !c.body_hidden {
            if !c.body.is_empty() || !self.latest_body.contains_key(&c.r#macro) {
                self.latest_body.insert(c.r#macro.clone(), c.body.clone());
            }
            return c.body.clone();
        }
        if let Some(b) = self
            .latest_body
            .get(&c.r#macro)
            .or_else(|| self.first_body.get(&c.r#macro))
        {
            self.counts.bodies_borrowed += 1;
            return b.clone();
        }
        self.counts.bodies_hidden += 1;
        String::new()
    }

    /// Pops frame `i`, closing its claims at step `id`.
    fn pop_frame(&mut self, i: usize, id: u64) {
        let f = self.stack.remove(i);
        self.close_claims(id, |inst, _| inst == f.inst);
    }

    fn close_claims(&mut self, id: u64, mut done: impl FnMut(u64, u32) -> bool) {
        let infos = &mut self.infos;
        self.claims.retain(|&(inst, end, cid)| {
            if done(inst, end) {
                if let Some(info) = infos.get_mut(usize::try_from(cid).unwrap_or(usize::MAX)) {
                    info.claim_n = u32::try_from(id - cid - 1).unwrap_or(NONE);
                }
                false
            } else {
                true
            }
        });
    }

    /// Pass 1 over one step.
    ///
    /// # Panics
    ///
    /// When steps come out of order (ids must be dense, from 0).
    #[allow(clippy::too_many_lines)]
    pub fn join(&mut self, s: &Step) {
        let id = s.id;
        debug_assert_eq!(usize::try_from(id).ok(), Some(self.infos.len()));
        self.counts.steps += 1;
        if s.reshown
            && let Some(prev) = self.infos.last_mut()
        {
            // The token the step before showed, read again in the new mode
            // (TeX backs it up): the same position; this one acts, so its
            // claim moves here and the one before only switched modes.
            let mut info = *prev;
            prev.consumes = 0;
            prev.claim = 0;
            prev.claim_n = 0;
            prev.confirm = NONE;
            prev.cause = NONE;
            prev.may = 0;
            prev.scope = 0;
            info.next_call = 0;
            info.unfold_end = 0;
            for c in &mut self.claims {
                if c.2 + 1 == id {
                    c.2 = id;
                }
            }
            self.counts.reshown += 1;
            if !self.calls.is_empty() {
                self.counts.unfolded += 1;
            }
            self.prev = Some((info.order, info.line, info.file));
            self.infos.push(info);
            return;
        }
        let keys = self.keys(s);
        let file = s.file.as_deref().map(|f| self.file(f));
        // Where the step joins: (frame index or None for the file, atom).
        let mut found: Option<(Option<usize>, usize)> = None;
        if !keys.is_empty() {
            for i in (0..self.stack.len()).rev().take(64) {
                let f = &self.stack[i];
                if let Some(j) = find(&f.text.keys, &f.used, f.cursor, &keys, 4096, 8) {
                    found = Some((Some(i), j));
                    break;
                }
            }
            if found.is_none()
                && let Some(fi) = file
            {
                let fs = &self.files[fi as usize];
                if let Some(t) = &fs.text {
                    let mut j = find(&t.keys, &fs.used, fs.cursor, &keys, 512, 8);
                    if j.is_none()
                        && let Some(l) = s.line
                        && let Some(&ls) = t.line_starts.get((l as usize).saturating_sub(1))
                    {
                        let from = t.start.partition_point(|&b| b < ls);
                        if from > fs.cursor {
                            j = find(&t.keys, &fs.used, from, &keys, 512, 0);
                        }
                    }
                    found = j.map(|j| (None, j));
                }
            }
        }
        // A primitive read through a name `\let` to it shows under the
        // primitive's name: the innermost text's next control sequence, if
        // it is no macro and no other primitive, is taken for an alias.
        if found.is_none()
            && matches!(s.kind, Kind::Primitive | Kind::If)
            && let Some(&k0) = keys.first()
        {
            let at = |keys: &[u64], used: &[bool], cursor: usize| {
                (cursor..keys.len().min(cursor + 4))
                    .find(|&j| !used[j] && keys[j] & !0xffff_ffff == K_CS)
            };
            let cand = if let Some(f) = self.stack.last() {
                at(&f.text.keys, &f.used, f.cursor)
                    .map(|j| (Some(self.stack.len() - 1), j, f.text.keys[j]))
            } else if let Some(fi) = file
                && let Some(t) = &self.files[fi as usize].text
            {
                let fs = &self.files[fi as usize];
                at(&t.keys, &fs.used, fs.cursor).map(|j| (None, j, t.keys[j]))
            } else {
                None
            };
            if let Some((frame, j, key)) = cand {
                let name = u32::try_from(key & 0xffff_ffff).unwrap_or(NONE);
                if !self.macro_names.contains(&name) && !self.prim_names.contains(&name) {
                    let prim = u32::try_from(k0 & 0xffff_ffff).unwrap_or(NONE);
                    self.aliases.entry(prim).or_default().push(key);
                    self.counts.aliases += 1;
                    found = Some((frame, j));
                }
            }
        }
        // Finish the texts above the one joined (unless they wait at a
        // parameter with more after it).
        if let Some((frame, _)) = found {
            let keep_from = frame.map_or(0, |i| i + 1);
            let mut k = self.stack.len();
            while k > keep_from {
                k -= 1;
                if !waits_at_param(&self.stack[k]) {
                    self.pop_frame(k, id);
                }
            }
        }
        let mut info = Info {
            order: 0,
            line: s.line.unwrap_or(0),
            file: file.unwrap_or(NONE),
            precision: 3,
            consumes: 0,
            claim: 0,
            claim_n: 0,
            confirm: NONE,
            scope: 0,
            scope_confirm: NONE,
            may: 0,
            cause: NONE,
            next_call: 0,
            unfold_end: 0,
        };
        let mut level = u32::try_from(self.stack.len()).unwrap_or(NONE);
        let mut atom_info = None;
        let mut joined_at = None;
        match found {
            Some((Some(i), j)) => {
                // (the frame may have moved down when frames above were
                // popped: it is the one with the instance found)
                let i = i.min(self.stack.len() - 1);
                let f = &mut self.stack[i];
                f.used[j] = true;
                if j >= f.cursor {
                    f.cursor = j + 1;
                }
                let ai = f.text.info[j];
                if ai.skip && ai.end as usize > f.cursor {
                    f.cursor = ai.end as usize;
                }
                info.order = f.order;
                info.line = f.line;
                info.file = f.file;
                info.precision = 1;
                atom_info = Some(ai);
                joined_at = Some((f.inst, j));
                level = u32::try_from(i + 1).unwrap_or(NONE);
                self.counts.joined_body += 1;
            }
            Some((None, j)) => {
                let fi = file.expect("joined in a file");
                let fs = &mut self.files[fi as usize];
                let t = fs.text.as_ref().expect("a text");
                fs.used[j] = true;
                if j >= fs.cursor {
                    fs.cursor = j + 1;
                }
                let ai = t.info[j];
                if ai.skip && ai.end as usize > fs.cursor {
                    fs.cursor = ai.end as usize;
                }
                fs.last = fs.last.max(t.start[j]);
                info.order = (u64::from(fi) << 32) | u64::from(t.start[j]);
                info.line = t.line[j];
                info.precision = 0;
                atom_info = Some(ai);
                joined_at = Some((fs.inst, j));
                level = 0;
                self.counts.joined_exact += 1;
            }
            None => {
                if keys.is_empty() {
                    self.counts.unkeyed += 1;
                } else {
                    self.counts.join_failures += 1;
                    if std::env::var_os("PHITEX_REPLAY_DEBUG").is_some() && id.is_multiple_of(97) {
                        eprintln!("FAIL {id} {} depth {}", s.name, self.stack.len());
                        for f in self.stack.iter().rev().take(3) {
                            eprintln!(
                                "   cur {} / {}: {}",
                                f.cursor,
                                f.text.keys.len(),
                                f.text.dbg
                            );
                        }
                    }
                }
                if let Some(f) = self.stack.last() {
                    info.order = f.order;
                    info.line = f.line;
                    info.file = f.file;
                    info.precision = 1;
                } else if let Some(fi) = file {
                    let fs = &self.files[fi as usize];
                    let mut pos = fs.last;
                    if let (Some(t), Some(l)) = (&fs.text, s.line)
                        && let Some(&ls) = t.line_starts.get((l as usize).saturating_sub(1))
                    {
                        pos = pos.max(ls);
                    }
                    info.order = (u64::from(fi) << 32) | u64::from(pos);
                    info.precision = 2;
                } else if let Some((o, l, f)) = self.prev {
                    info.order = o;
                    info.line = l;
                    info.file = f;
                }
            }
        }
        if let Some((inst, j)) = joined_at {
            let j = u32::try_from(j).unwrap_or(NONE);
            self.close_claims(id, |i, end| i == inst && end <= j);
        }
        // Finished texts at the top go.
        while let Some(f) = self.stack.last()
            && f.cursor >= f.text.keys.len()
        {
            let k = self.stack.len() - 1;
            self.pop_frame(k, id);
        }

        // The pass's view.
        let is_cs = matches!(s.kind, Kind::Primitive | Kind::Macro | Kind::If);
        let joined = found.is_some();
        let name = s.name.strip_prefix('\\').unwrap_or(&s.name);
        let confirm_cs = || format!("cs:{}", s.name);
        if is_cs && !keys.is_empty() {
            match atom_info {
                Some(ai) if ai.status > 0 => {
                    info.claim = ai.status - 1;
                    if ai.status == 2 {
                        info.confirm = self.strings.get(&confirm_cs());
                    }
                    info.cause = ai.cause;
                    if ai.status == 2 && ai.cause == NONE {
                        info.cause = self.strings.get("predicted");
                    }
                    if ai.consumes && ai.status != 3 {
                        let (inst, j) = joined_at.expect("joined");
                        if ai.end as usize > j + 1 {
                            self.claims.push((inst, ai.end, id));
                        }
                    }
                }
                Some(_) => {}
                None => {
                    info.claim = 2;
                    info.cause = self.strings.get("join-failed");
                }
            }
        }
        info.consumes = match s.kind {
            Kind::Macro => match atom_info {
                Some(ai) if ai.status != 3 => 1,
                Some(_) => 3,
                None => {
                    self.counts.consumes_pending_join += 1;
                    3
                }
            },
            Kind::Primitive | Kind::If => match atom_info {
                Some(ai) if ai.status == 3 && ai.consumes => 3,
                Some(ai) if ai.consumes => 2,
                None if s.consumes.is_some() => {
                    self.counts.consumes_pending_join += 1;
                    3
                }
                _ => 0,
            },
            _ => 0,
        };
        // Scope.
        let dir = match s.kind {
            Kind::GroupBegin => Some((true, "catcode:123".to_owned())),
            Kind::GroupEnd => Some((false, "catcode:125".to_owned())),
            Kind::Primitive if name == "begingroup" => Some((true, confirm_cs())),
            Kind::Primitive if name == "endgroup" => Some((false, confirm_cs())),
            Kind::Macro if name == "begin" => Some((true, confirm_cs())),
            Kind::Macro if name == "end" => Some((false, confirm_cs())),
            _ => None,
        };
        if let Some((open, confirm)) = dir {
            let known = atom_info.is_some_and(|ai| ai.scope == 1);
            info.scope = match (known, open) {
                (true, true) => 1,
                (true, false) => 2,
                (false, true) => 3,
                (false, false) => 4,
            };
            if !known {
                info.scope_confirm = self.strings.get(&confirm);
            }
        }
        // What it may define.
        let definer = DEFINERS.contains(&name) && s.kind == Kind::Primitive;
        info.may = match atom_info {
            Some(ai) if ai.may != 0 => ai.may,
            _ if definer && !joined => {
                self.counts.may_any_join += 1;
                self.define(MayDefine::Any)
            }
            _ => 0,
        };
        let body_text = self.body_of(s);
        if s.kind == Kind::Macro && info.may == 0 && !body_text.is_empty() {
            let t = self.body(&body_text);
            info.may = t.summary;
        }
        if let MayDefine::Prefixes(ps) = &self.defines[info.may as usize]
            && definer
            && !s.writes.iter().any(|w| {
                w.field.starts_with("cs:") && ps.iter().any(|p| w.field.starts_with(p.as_str()))
            })
            && s.writes.iter().any(|w| w.field.starts_with("cs:"))
        {
            self.counts.prefix_mismatch += 1;
            info.may = self.define(MayDefine::Any);
        }

        // Unfolds: a call ends the open calls at its level or deeper.
        if s.kind == Kind::Macro {
            self.counts.macro_calls += 1;
            let infos = &mut self.infos;
            self.calls.retain(|&(l, c)| {
                if l >= level {
                    infos[usize::try_from(c).expect("an index")].unfold_end = id;
                    false
                } else {
                    true
                }
            });
            if let Some(c) = self.last_call.replace(id) {
                infos[usize::try_from(c).expect("an index")].next_call = id;
            }
        }
        if !self.calls.is_empty() {
            self.counts.unfolded += 1;
        }
        self.prev = Some((info.order, info.line, info.file));
        self.infos.push(info);
        if s.kind == Kind::Macro {
            self.calls.push((level, id));
            let text = self.body(&body_text);
            let inst = self.inst();
            self.stack.push(Frame {
                used: vec![false; text.keys.len()],
                text,
                cursor: 0,
                inst,
                order: info.order,
                line: info.line,
                file: info.file,
            });
            if self.stack.len() > 64 {
                self.pop_frame(0, id);
            }
        }
    }

    /// Ends pass 1.
    ///
    /// # Panics
    ///
    /// Never: the open calls are steps joined.
    pub fn finish_join(&mut self) {
        let n = self.infos.len() as u64;
        self.close_claims(n, |_, _| true);
        for &(_, c) in &self.calls {
            self.infos[usize::try_from(c).expect("an index")].unfold_end = n;
        }
        self.calls.clear();
        if let Some(c) = self.last_call.take() {
            self.infos[usize::try_from(c).expect("an index")].next_call = n;
        }
    }

    /// The counts of pass 1's results.
    #[must_use]
    pub fn tally(&self) -> Counts {
        let mut c = self.counts.clone();
        for i in &self.infos {
            match i.consumes {
                0 => c.consumes_null += 1,
                3 => c.consumes_pending += 1,
                _ => c.consumes_known += 1,
            }
            match i.claim {
                0 => c.claim_known += 1,
                1 => c.claim_predicted += 1,
                _ => c.claim_pending += 1,
            }
            match self.defines[i.may as usize] {
                MayDefine::None => c.may_none += 1,
                MayDefine::Names(_) => c.may_names += 1,
                MayDefine::Prefixes(_) => c.may_prefixes += 1,
                MayDefine::Any => c.may_any += 1,
            }
        }
        c
    }

    /// The output record of step `s` (pass 2).
    ///
    /// # Panics
    ///
    /// When `s` was not joined in pass 1.
    #[must_use]
    pub fn out_step<'s>(&'s self, s: &'s Step) -> OutStep<'s> {
        let info = &self.infos[usize::try_from(s.id).expect("an index")];
        let consumes = match info.consumes {
            0 => None,
            1 => Some(OutConsumes::Known(info.next_call.saturating_sub(s.id + 1))),
            2 => Some(OutConsumes::Known(u64::from(info.claim_n))),
            _ => Some(OutConsumes::Pending),
        };
        let n = u64::from(info.claim_n);
        let claim = match info.claim {
            0 => OutClaim::Known(n),
            1 => OutClaim::Predicted(n, self.string(info.confirm)),
            _ => OutClaim::Pending,
        };
        let scope = match info.scope {
            1 => Some(OutScope::Known(Dir::Open)),
            2 => Some(OutScope::Known(Dir::Close)),
            3 => Some(OutScope::Predicted(
                Dir::Open,
                self.string(info.scope_confirm),
            )),
            4 => Some(OutScope::Predicted(
                Dir::Close,
                self.string(info.scope_confirm),
            )),
            _ => None,
        };
        let unfold = (s.kind == Kind::Macro).then(|| Unfold {
            produced: Produced(s.id + 1..info.unfold_end.max(s.id + 1)),
        });
        OutStep {
            id: s.id,
            text_order: info.order,
            nesting_depth: s.depth,
            reads: &s.reads,
            writes: s
                .writes
                .iter()
                .map(|w| OutWrite {
                    name: &w.field,
                    global: w.global,
                })
                .collect(),
            consumes,
            may_define: &self.defines[info.may as usize],
            unfold,
            hints: Hints {
                claim,
                scope,
                cause: (info.cause != NONE).then(|| self.string(info.cause)),
            },
            source: OutSource {
                file: self
                    .files
                    .get(info.file as usize)
                    .map_or("", |f| f.name.as_str()),
                line: info.line,
                position: ["exact", "call-site", "line", "inherited"][info.precision as usize],
            },
        }
    }

    fn string(&self, i: u32) -> &str {
        self.strings
            .names
            .get(i as usize)
            .map_or("?", String::as_str)
    }
}

/// The definition target after a primitive definer at atom `j`.
#[allow(clippy::many_single_char_names)]
fn target(atoms: &[pass::Atom], j: usize, text: &str) -> MayDefine {
    let mut k = j;
    while atoms.get(k).is_some_and(|a| a.kind == AtomKind::Space) {
        k += 1;
    }
    let Some(a) = atoms.get(k) else {
        return MayDefine::Any;
    };
    match a.kind {
        AtomKind::Cs => {
            let n = a.name(text);
            if n == "csname" {
                let mut lit = String::new();
                for b in &atoms[k + 1..] {
                    match b.kind {
                        AtomKind::Char(c) => lit.push(c),
                        _ => break,
                    }
                }
                if lit.is_empty() {
                    MayDefine::Any
                } else {
                    MayDefine::Prefixes(vec![format!("cs:\\{lit}")])
                }
            } else if matches!(n, "expandafter" | "noexpand" | "global" | "the") {
                MayDefine::Any
            } else {
                MayDefine::Names(vec![format!("cs:\\{n}")])
            }
        }
        AtomKind::Char(c) if !c.is_alphanumeric() && !c.is_whitespace() => {
            MayDefine::Names(vec![format!("cs:{c}")])
        }
        _ => MayDefine::Any,
    }
}

fn waits_at_param(f: &Frame) -> bool {
    f.text.keys.get(f.cursor) == Some(&K_PARAM) && f.cursor + 1 < f.text.keys.len()
}

/// The first unconsumed atom from `cursor` (forward `fwd` atoms, then
/// back `back`) whose key is one of `want`.
fn find(
    keys: &[u64],
    used: &[bool],
    cursor: usize,
    want: &[u64],
    fwd: usize,
    back: usize,
) -> Option<usize> {
    // (back first: an atom skipped over by `\expandafter` comes before
    // the cursor)
    let end = keys.len().min(cursor.saturating_add(fwd));
    (cursor.saturating_sub(back)..cursor.min(keys.len()))
        .rev()
        .chain(cursor..end)
        .find(|&j| !used[j] && want.contains(&keys[j]))
}

/// `consumes`.
#[derive(Debug, Serialize)]
pub enum OutConsumes {
    Known(u64),
    Pending,
}

#[derive(Debug, Serialize)]
pub enum OutClaim<'a> {
    Known(u64),
    Predicted(u64, &'a str),
    Pending,
}

#[derive(Debug, Serialize)]
pub enum Dir {
    Open,
    Close,
}

#[derive(Debug, Serialize)]
pub enum OutScope<'a> {
    Known(Dir),
    Predicted(Dir, &'a str),
}

/// A range of step ids, written as a list.
#[derive(Debug)]
pub struct Produced(pub std::ops::Range<u64>);

impl Serialize for Produced {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut seq = s.serialize_seq(usize::try_from(self.0.end - self.0.start).ok())?;
        for i in self.0.clone() {
            seq.serialize_element(&i)?;
        }
        seq.end()
    }
}

#[derive(Debug, Serialize)]
pub struct Unfold {
    pub produced: Produced,
}

#[derive(Debug, Serialize)]
pub struct Hints<'a> {
    pub claim: OutClaim<'a>,
    pub scope: Option<OutScope<'a>>,
    pub cause: Option<&'a str>,
}

#[derive(Debug, Serialize)]
pub struct OutWrite<'a> {
    pub name: &'a str,
    pub global: bool,
}

#[derive(Debug, Serialize)]
pub struct OutSource<'a> {
    pub file: &'a str,
    pub line: u32,
    pub position: &'a str,
}

/// A step in the replay format.
#[derive(Debug, Serialize)]
pub struct OutStep<'a> {
    pub id: u64,
    pub text_order: u64,
    pub nesting_depth: u32,
    pub reads: &'a [String],
    pub writes: Vec<OutWrite<'a>>,
    pub consumes: Option<OutConsumes>,
    pub may_define: &'a MayDefine,
    pub unfold: Option<Unfold>,
    pub hints: Hints<'a>,
    pub source: OutSource<'a>,
}

/// Reads the steps of a trace.
pub fn steps<R: BufRead>(
    reader: R,
    mut f: impl FnMut(Step) -> io::Result<()>,
) -> io::Result<Option<crate::step::Meta>> {
    let mut meta = None;
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Record>(&line).map_err(io::Error::other)? {
            Record::Meta { meta: m } => meta = Some(m),
            Record::Step(s) => f(s)?,
        }
    }
    Ok(meta)
}

/// What the replay meta says beyond the counts.
#[derive(Clone, Debug, Default)]
pub struct Provenance {
    pub document: String,
    pub trace_commit: String,
}

/// Converts the trace `open` reads (opened three times: three passes) to the
/// replay format on `out`, reading source files through `load`, and makes
/// the records of the planned `edits`. Returns the meta object (written
/// first) and the edit records.
///
/// # Errors
///
/// The trace's or the output's I/O errors, a malformed trace.
pub fn convert<R: BufRead, W: io::Write>(
    mut open: impl FnMut() -> io::Result<R>,
    load: impl FnMut(&str) -> Option<String>,
    prov: &Provenance,
    edits: &[EditPlan],
    out: &mut W,
) -> io::Result<(serde_json::Value, Vec<serde_json::Value>)> {
    let mut c = Converter::new(load);
    // Pass 0: each macro's first body printed with `\\`.
    steps(open()?, |s| {
        if let Some(m) = &s.macro_call
            && !m.body_hidden
            && !c.first_body.contains_key(&m.r#macro)
        {
            c.first_body.insert(m.r#macro.clone(), m.body.clone());
        }
        Ok(())
    })?;
    let mut fields: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut expected = 0u64;
    let meta = steps(open()?, |s| {
        if s.id != expected {
            return Err(io::Error::other(format!("step ids not dense at {}", s.id)));
        }
        expected += 1;
        for r in &s.reads {
            if !fields.contains(r) {
                fields.insert(r.clone());
            }
        }
        for w in &s.writes {
            if !fields.contains(&w.field) {
                fields.insert(w.field.clone());
            }
        }
        c.join(&s);
        Ok(())
    })?;
    c.finish_join();
    let meta = meta.unwrap_or(crate::step::Meta {
        main: String::new(),
        engine: String::new(),
        conservative: false,
        version: None,
        command: None,
    });
    let mut counts = c.tally();
    counts.fields = fields.len() as u64;
    counts.edits = edits.len() as u64;
    let meta_v = serde_json::json!({
        "document": prov.document,
        "main": meta.main,
        "engine": meta.engine,
        "engine_command": meta.command,
        "pdftex_version": meta.version,
        "trace_commit": prov.trace_commit,
        "conservative": meta.conservative,
        "conventions": CONVENTIONS,
        "counts": counts,
    });
    serde_json::to_writer(&mut *out, &serde_json::json!({ "meta": meta_v }))?;
    out.write_all(b"\n")?;
    let wanted: std::collections::HashSet<u64> = edits.iter().map(|e| e.target).collect();
    let mut targets: HashMap<u64, serde_json::Value> = HashMap::new();
    steps(open()?, |s| {
        let o = c.out_step(&s);
        if wanted.contains(&s.id) {
            targets.insert(s.id, serde_json::to_value(&o).map_err(io::Error::other)?);
        }
        serde_json::to_writer(&mut *out, &o).map_err(io::Error::other)?;
        out.write_all(b"\n")
    })?;
    out.flush()?;
    let edit_values = edit_records(edits, &targets, counts.steps);
    Ok((meta_v, edit_values))
}

/// An edit chosen from the edit-impact sample.
#[derive(Clone, Debug)]
pub struct EditPlan {
    /// 0 `change_writes`, 1 `insert_after`, 2 `delete`.
    pub form: u8,
    pub target: u64,
    pub line: u32,
    /// The analysis's impact for the line (all its writers).
    pub impact_line: u64,
    /// The impact of the target step's writes alone.
    pub impact_step: u64,
}

/// Chooses the edits: one per sampled line, the form and the line's
/// writing step drawn with a fixed seed (2).
#[must_use]
pub fn plan_edits(g: &crate::analyze::Graph, samples: usize) -> Vec<EditPlan> {
    let mut rng = fastrand::Rng::with_seed(2);
    g.edit_samples(samples)
        .into_iter()
        .map(|e| {
            let form = rng.u8(0..3);
            let target = e.writers[rng.usize(0..e.writers.len())];
            EditPlan {
                form,
                target: u64::from(target),
                line: e.line,
                impact_line: e.impact,
                impact_step: g.dependents_of(target, &e.steps),
            }
        })
        .collect()
}

fn edit_records(
    edits: &[EditPlan],
    targets: &HashMap<u64, serde_json::Value>,
    steps: u64,
) -> Vec<serde_json::Value> {
    edits
        .iter()
        .enumerate()
        .map(|(k, e)| {
            let mut v = match e.form {
                0 => serde_json::json!({ "change_writes": e.target }),
                1 => {
                    let mut step = targets.get(&e.target).cloned().unwrap_or_default();
                    // A new step after the target writing the same fields:
                    // it shadows the target's writes for the steps after.
                    let names: Vec<String> = step["writes"]
                        .as_array()
                        .map(|w| {
                            w.iter()
                                .filter_map(|x| x["name"].as_str())
                                .filter(|n| n.starts_with("cs:"))
                                .map(str::to_owned)
                                .collect()
                        })
                        .unwrap_or_default();
                    step["id"] = (steps + k as u64).into();
                    step["reads"] = serde_json::json!([]);
                    step["consumes"] = serde_json::Value::Null;
                    step["unfold"] = serde_json::Value::Null;
                    step["may_define"] = if names.is_empty() {
                        "None".into()
                    } else {
                        serde_json::json!({ "Names": names })
                    };
                    step["hints"] = serde_json::json!({"claim": {"Known": 0}, "scope": null, "cause": "inserted"});
                    serde_json::json!({ "insert_after": e.target, "step": step })
                }
                _ => serde_json::json!({ "delete": e.target }),
            };
            v["line"] = e.line.into();
            v["expected_impact"] = e.impact_line.into();
            v["expected_impact_step"] = e.impact_step.into();
            v
        })
        .collect()
}

/// Checks a replay file: the meta line, then steps with dense ids and
/// every field of the format in its shape. Returns the step count.
///
/// # Errors
///
/// The first violation, with its line.
pub fn validate<R: BufRead>(reader: R) -> Result<u64, String> {
    let mut lines = reader.lines();
    let first = lines.next().ok_or("empty")?.map_err(|e| e.to_string())?;
    let meta: serde_json::Value = serde_json::from_str(&first).map_err(|e| format!("meta: {e}"))?;
    let m = meta.get("meta").ok_or("line 1 is not {\"meta\": ...}")?;
    for k in [
        "document",
        "engine",
        "pdftex_version",
        "trace_commit",
        "conventions",
        "counts",
    ] {
        if m.get(k).is_none() {
            return Err(format!("meta lacks {k}"));
        }
    }
    let declared = m["counts"]["steps"].as_u64().ok_or("meta.counts.steps")?;
    let mut n = 0u64;
    let mut produced_max: Vec<(u64, u64)> = Vec::new();
    for (k, line) in lines.enumerate() {
        let line = line.map_err(|e| e.to_string())?;
        let at = k + 2;
        let v: serde_json::Value =
            serde_json::from_str(&line).map_err(|e| format!("line {at}: {e}"))?;
        check_step(&v, n).map_err(|e| format!("line {at}: {e}"))?;
        if let Some(p) = v["unfold"]["produced"].as_array()
            && let Some(last) = p.last().and_then(serde_json::Value::as_u64)
        {
            produced_max.push((n, last));
        }
        n += 1;
    }
    if n != declared {
        return Err(format!("{n} steps, meta says {declared}"));
    }
    if let Some((id, last)) = produced_max.iter().find(|(_, l)| *l >= n) {
        return Err(format!("step {id} produces {last}, past the last step"));
    }
    Ok(n)
}

/// One step's shape; `id` must be `expect`.
///
/// # Errors
///
/// What is wrong.
#[allow(clippy::too_many_lines, clippy::many_single_char_names)]
pub fn check_step(v: &serde_json::Value, expect: u64) -> Result<(), String> {
    use serde_json::Value;
    let o = v.as_object().ok_or("not an object")?;
    let keys = [
        "id",
        "text_order",
        "nesting_depth",
        "reads",
        "writes",
        "consumes",
        "may_define",
        "unfold",
        "hints",
        "source",
    ];
    for k in keys {
        if !o.contains_key(k) {
            return Err(format!("no {k}"));
        }
    }
    if o.len() != keys.len() {
        return Err("unexpected keys".to_owned());
    }
    if o["id"].as_u64() != Some(expect) {
        return Err(format!("id {} is not {expect}", o["id"]));
    }
    o["text_order"].as_u64().ok_or("text_order")?;
    let d = o["nesting_depth"].as_u64().ok_or("nesting_depth")?;
    u32::try_from(d).map_err(|_| "nesting_depth not u32")?;
    let reads = o["reads"].as_array().ok_or("reads")?;
    if !reads.iter().all(Value::is_string) {
        return Err("reads: not strings".to_owned());
    }
    for w in o["writes"].as_array().ok_or("writes")? {
        let w = w.as_object().ok_or("writes: not objects")?;
        if w.len() != 2 || !w["name"].is_string() || !w["global"].is_boolean() {
            return Err("writes: {name, global}".to_owned());
        }
    }
    let known_n = |x: &Value| -> bool {
        x.as_object()
            .is_some_and(|m| m.len() == 1 && m.get("Known").is_some_and(Value::is_u64))
    };
    match &o["consumes"] {
        Value::Null => {}
        Value::String(s) if s == "Pending" => {}
        x if known_n(x) => {}
        x => return Err(format!("consumes {x}")),
    }
    match &o["may_define"] {
        Value::String(s) if s == "None" || s == "Any" => {}
        Value::Object(m)
            if m.len() == 1
                && ["Names", "Prefixes"].iter().any(|k| {
                    m.get(*k)
                        .and_then(Value::as_array)
                        .is_some_and(|a| !a.is_empty() && a.iter().all(Value::is_string))
                }) => {}
        x => return Err(format!("may_define {x}")),
    }
    match &o["unfold"] {
        Value::Null => {}
        Value::Object(m) if m.len() == 1 => {
            let p = m
                .get("produced")
                .and_then(Value::as_array)
                .ok_or("unfold.produced")?;
            let mut prev = expect;
            for x in p {
                let x = x.as_u64().ok_or("unfold.produced: ids")?;
                if x <= prev {
                    return Err("unfold.produced: not increasing after the step".to_owned());
                }
                prev = x;
            }
        }
        x => return Err(format!("unfold {x}")),
    }
    let h = o["hints"].as_object().ok_or("hints")?;
    if h.len() != 3 {
        return Err("hints: {claim, scope, cause}".to_owned());
    }
    let dir = |x: &Value| x.as_str().is_some_and(|s| s == "Open" || s == "Close");
    match &h["claim"] {
        Value::String(s) if s == "Pending" => {}
        x if known_n(x) => {}
        Value::Object(m)
            if m.len() == 1
                && m.get("Predicted")
                    .and_then(Value::as_array)
                    .is_some_and(|a| a.len() == 2 && a[0].is_u64() && a[1].is_string()) => {}
        x => return Err(format!("hints.claim {x}")),
    }
    match &h["scope"] {
        Value::Null => {}
        Value::Object(m) if m.len() == 1 && m.get("Known").is_some_and(dir) => {}
        Value::Object(m)
            if m.len() == 1
                && m.get("Predicted")
                    .and_then(Value::as_array)
                    .is_some_and(|a| a.len() == 2 && dir(&a[0]) && a[1].is_string()) => {}
        x => return Err(format!("hints.scope {x}")),
    }
    if !(h["cause"].is_null() || h["cause"].is_string()) {
        return Err("hints.cause".to_owned());
    }
    let s = o["source"].as_object().ok_or("source")?;
    if !s.get("file").is_some_and(Value::is_string)
        || !s.get("line").is_some_and(Value::is_u64)
        || !s.get("position").is_some_and(Value::is_string)
    {
        return Err("source: {file, line, position}".to_owned());
    }
    Ok(())
}

/// Checks an edits file against the steps of its replay file.
///
/// # Errors
///
/// The first violation.
pub fn validate_edits<R: BufRead>(reader: R, steps: u64) -> Result<u64, String> {
    let mut n = 0;
    for (k, line) in reader.lines().enumerate() {
        let line = line.map_err(|e| e.to_string())?;
        let v: serde_json::Value =
            serde_json::from_str(&line).map_err(|e| format!("edit {k}: {e}"))?;
        let id = ["change_writes", "insert_after", "delete"]
            .iter()
            .find_map(|f| v.get(*f).and_then(serde_json::Value::as_u64))
            .ok_or(format!("edit {k}: no form"))?;
        if id >= steps {
            return Err(format!("edit {k}: step {id} out of range"));
        }
        if let Some(s) = v.get("step") {
            let sid = s["id"].as_u64().ok_or(format!("edit {k}: step id"))?;
            check_step(s, sid).map_err(|e| format!("edit {k}: {e}"))?;
        }
        if v.get("expected_impact")
            .and_then(serde_json::Value::as_u64)
            .is_none()
        {
            return Err(format!("edit {k}: expected_impact"));
        }
        n += 1;
    }
    Ok(n)
}

/// The `INDEX.md` table over replay directories' `meta.json`s.
#[must_use]
pub fn index(metas: &[(String, serde_json::Value)]) -> String {
    let mut s = String::from(
        "| document | steps | fields | consumes null / Known / Pending % | may_define None / Names / Prefixes / Any % | unfolded % | join failures (% of steps) | unkeyed | edits |\n|---|---|---|---|---|---|---|---|---|\n",
    );
    #[allow(clippy::cast_precision_loss)]
    let pct = |a: u64, b: u64| {
        if b == 0 {
            0.0
        } else {
            100.0 * a as f64 / b as f64
        }
    };
    for (name, m) in metas {
        let Ok(c) = serde_json::from_value::<Counts>(m["counts"].clone()) else {
            continue;
        };
        let n = c.steps;
        let _ = writeln!(
            s,
            "| `{name}` | {n} | {} | {:.1} / {:.1} / {:.1} | {:.1} / {:.2} / {:.3} / {:.2} | {:.1} | {} ({:.2}%) | {} | {} |",
            c.fields,
            pct(c.consumes_null, n),
            pct(c.consumes_known, n),
            pct(c.consumes_pending, n),
            pct(c.may_none, n),
            pct(c.may_names, n),
            pct(c.may_prefixes, n),
            pct(c.may_any, n),
            pct(c.unfolded, n),
            c.join_failures,
            pct(c.join_failures, n),
            c.unkeyed,
            c.edits,
        );
    }
    s
}

#[cfg(test)]
mod tests {
    use super::{check_step, expl_names, validate};
    use serde_json::json;

    fn step() -> serde_json::Value {
        json!({"id": 0, "text_order": 5, "nesting_depth": 0, "reads": ["count:1"],
            "writes": [{"name": "cs:\\x", "global": false}], "consumes": {"Known": 0},
            "may_define": {"Names": ["cs:\\x"]}, "unfold": null,
            "hints": {"claim": {"Predicted": [0, "cs:\\def"]}, "scope": null, "cause": null},
            "source": {"file": "a.tex", "line": 1, "position": "exact"}})
    }

    #[test]
    fn the_validator_checks_shapes() {
        assert_eq!(check_step(&step(), 0), Ok(()));
        assert!(check_step(&step(), 1).is_err());
        let mut s = step();
        s["consumes"] = json!({"Known": -1});
        assert!(check_step(&s, 0).is_err());
        let mut s = step();
        s["may_define"] = json!({"Names": []});
        assert!(check_step(&s, 0).is_err());
        let mut s = step();
        s["unfold"] = json!({"produced": [0]});
        assert!(check_step(&s, 0).is_err());
        let mut s = step();
        s["hints"]["scope"] = json!({"Known": "Sideways"});
        assert!(check_step(&s, 0).is_err());
        let file = format!(
            "{}\n{}\n",
            json!({"meta": {"document": "d", "engine": "e", "pdftex_version": null,
                "trace_commit": "c", "conventions": [], "counts": {"steps": 1}}}),
            step()
        );
        assert_eq!(validate(file.as_bytes()), Ok(1));
        assert!(validate(file.replace("\"steps\":1", "\"steps\":2").as_bytes()).is_err());
    }

    #[test]
    fn expl_bodies_are_told() {
        assert!(expl_names("\\exp_after:wN \\use_i:nn"));
        assert!(!expl_names("\\def \\x {a_b:c}"));
    }
}
