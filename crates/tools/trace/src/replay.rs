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
use serde::Serialize;

use crate::hash::fnv1a;
use crate::step::{Kind, Record, Step};

/// The conventions the converter follows, for `meta`.
pub const CONVENTIONS: &[&str] = &[
    "format v1 (crates/vendor/defuse/examples/common/trace.rs); see REPLAY.md",
    "id = s<trace id>; class = the primitive's or macro's name, char, space, {, }, font, page",
    "pos = file_index * 2^32 + byte_offset (files numbered in order of first appearance), made strictly increasing at the top level",
    "join: a step matches the next unconsumed atom with its name (or character) in the innermost text of a modelled input stack (macro bodies as the log prints them, over the file the log says is read)",
    "children: a call's following steps until one read from the call's own text or below, nested",
    "writes null + may_define: a step with children and no writes of its own; the pass's bound, any where a subtree write breaks it",
    "claim: the pass's Hints::claim, counts 0 (claimed text is no step); predicted/until end at the next sibling's class; null where that would be a cycle",
    "scope: the pass's ScopeHint (V/H/M/D kinds); implicit paragraph starts and ends as pending with starts_implicit/closes_implicit H",
    "contributes / reads_prev_contribution on hlist/vlist by the material's kind (the mode is a guess); fold on page",
    "reads drop \"?\"; env_reads time and files",
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
    /// The pass's [`pass::Hints`], in format v1's terms.
    hints: AtomHints,
}

/// An atom's [`pass::Hints`] (an item's or a mark's), compact.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AtomHints {
    /// 0 none, 1 open, 2 close, 3 pending.
    scope: u8,
    /// The opened scope's kind: 0 none, else `V`, `H`, `M`, `D`.
    kind: u8,
    /// 0 none, 1 starts a paragraph, 2 ends one.
    implicit: u8,
    /// 0 Known (a nonzero extent), 1 Predicted, 2 Pending, 3 no claim.
    claim: u8,
    /// The field a prediction confirms (a string).
    confirm: u32,
    /// A pending claim's cause (a string).
    cause: u32,
    /// What it may define (`Converter::may1`).
    may: u32,
}

const NO_HINTS: AtomHints = AtomHints {
    scope: 0,
    kind: 0,
    implicit: 0,
    claim: 3,
    confirm: NONE,
    cause: NONE,
    may: 0,
};

const NO_INFO: AtomInfo = AtomInfo {
    status: 0,
    consumes: false,
    end: 0,
    cause: NONE,
    may: 0,
    scope: 0,
    skip: false,
    hints: NO_HINTS,
};

/// What a step may define, with localities (format v1's `may_define`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum May1 {
    None,
    Names(Vec<(String, bool)>),
    Prefixes(Vec<(String, bool)>),
    Any,
}

impl May1 {
    fn of(m: &pass::MayDefine) -> May1 {
        let defs = |v: &[pass::Def]| v.iter().map(|d| (d.field.clone(), d.global)).collect();
        match m {
            pass::MayDefine::Nothing => May1::None,
            pass::MayDefine::Names(v) => May1::Names(defs(v)),
            pass::MayDefine::Prefixes(v) => May1::Prefixes(defs(v)),
            pass::MayDefine::Any => May1::Any,
        }
    }

    /// Whether a write of `field` is within the bound.
    fn covers(&self, field: &str) -> bool {
        match self {
            May1::None => false,
            May1::Names(v) => v.iter().any(|(f, _)| f == field),
            May1::Prefixes(v) => v.iter().any(|(f, _)| field.starts_with(f.as_str())),
            May1::Any => true,
        }
    }
}

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
    /// Format v1's `class` (a string).
    class: u32,
    /// The pass's hints for the joined atom (or the trace's own view).
    h: AtomHints,
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
    /// Format v1: top-level steps (the rest are nested `children`).
    #[serde(default)]
    pub top_level: u64,
    /// Claims written: `known`, `predicted`, `until`, and `null` (a
    /// predicted or pending claim with no delimiter to name).
    #[serde(default)]
    pub claim1_known: u64,
    #[serde(default)]
    pub claim1_predicted: u64,
    #[serde(default)]
    pub claim1_until: u64,
    #[serde(default)]
    pub claim1_null: u64,
    /// Stream claims whose next step is no sibling leaf or reads what the
    /// claimer's subtree writes.
    #[serde(default)]
    pub claim_fallback: u64,
    /// Pending definition sets (`writes: null`) by bound.
    #[serde(default)]
    pub may1_none: u64,
    #[serde(default)]
    pub may1_names: u64,
    #[serde(default)]
    pub may1_prefixes: u64,
    #[serde(default)]
    pub may1_any: u64,
    /// ... of them `any` because the pass's bound missed a write of the
    /// subtree.
    #[serde(default)]
    pub may_unsound: u64,
    /// Steps with children and writes of their own (no bound can be
    /// given: the engine takes `any`).
    #[serde(default)]
    pub may_own_writes: u64,
    /// Top-level closes at depth 0, written `pending`.
    #[serde(default)]
    pub scope_unmatched: u64,
    /// Paragraph effects dropped for an explicit scope effect.
    #[serde(default)]
    pub implicit_dropped: u64,
    /// Top-level `pos` moved forward to keep it increasing.
    #[serde(default)]
    pub pos_clamped: u64,
    #[serde(default)]
    pub contributes: u64,
    #[serde(default)]
    pub reads_prev: u64,
    #[serde(default)]
    pub folds: u64,
    #[serde(default)]
    pub env_steps: u64,
    /// Predicted claims written without their confirm field: no step
    /// before the delimiter defines it (a primitive's or the format's
    /// meaning), and an undefined field never ends the claim.
    #[serde(default)]
    pub confirm_dropped: u64,
    /// Openers' contributions written as a step before the opener.
    #[serde(default)]
    pub opener_contributions: u64,
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
    may1: Vec<May1>,
    may1_ids: HashMap<May1, u32>,
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
    file_insts: std::collections::HashSet<u64>,
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
            may1: Vec::new(),
            may1_ids: HashMap::new(),
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
            file_insts: std::collections::HashSet::new(),
            first_body: HashMap::new(),
            counts: Counts::default(),
        };
        let none = c.define(MayDefine::None);
        debug_assert_eq!(none, 0);
        let none = c.define1(May1::None);
        debug_assert_eq!(none, 0);
        c
    }

    fn define1(&mut self, m: May1) -> u32 {
        if let Some(&i) = self.may1_ids.get(&m) {
            return i;
        }
        let i = u32::try_from(self.may1.len()).expect("fewer than 2^32");
        self.may1.push(m.clone());
        self.may1_ids.insert(m, i);
        i
    }

    /// An item's or mark's hints, compact.
    fn hints_of(&mut self, h: &pass::Hints) -> AtomHints {
        let kind = |k: &pass::ScopeKind| match k {
            pass::ScopeKind::VBox => 1,
            pass::ScopeKind::HBox => 2,
            pass::ScopeKind::Math => 3,
            pass::ScopeKind::Display => 4,
            _ => 0,
        };
        let (scope, k) = match &h.scope {
            pass::ScopeHint::None => (0, 0),
            pass::ScopeHint::Open(k)
            | pass::ScopeHint::Predicted {
                effect: pass::Effect::Open(k),
                ..
            } => (1, kind(k)),
            pass::ScopeHint::Close
            | pass::ScopeHint::Predicted {
                effect: pass::Effect::Close,
                ..
            } => (2, 0),
            pass::ScopeHint::Pending { .. } => (3, 0),
        };
        let mut out = AtomHints {
            scope,
            kind: k,
            implicit: match h.implicit {
                pass::Implicit::None => 0,
                pass::Implicit::StartsParagraph => 1,
                pass::Implicit::EndsParagraph => 2,
            },
            claim: 3,
            confirm: NONE,
            cause: NONE,
            may: 0,
        };
        match &h.claim {
            pass::Claim::Known(0) => {}
            pass::Claim::Known(_) => out.claim = 0,
            pass::Claim::Predicted { confirm, .. } => {
                out.claim = 1;
                let f = match confirm {
                    pass::Assumption::MeaningIs { cs, .. } => format!("cs:\\{cs}"),
                    pass::Assumption::CatcodeIs { byte, .. } => format!("catcode:{byte}"),
                };
                out.confirm = self.strings.get(&f);
            }
            pass::Claim::Pending { cause } => {
                out.claim = 2;
                out.cause = self.strings.get(cause);
            }
        }
        out.may = self.define1(May1::of(&h.may_define));
        out
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
        self.file_insts.insert(inst);
        self.files.push(FileState {
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
        let mut mark_of = vec![NONE; p.atoms.len()];
        for (k, m) in p.marks.iter().enumerate() {
            mark_of[m.atom] = u32::try_from(k).unwrap_or(NONE);
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
                info.hints = self.hints_of(&it.hints);
            } else if let Some(m) = p.marks.get(mark_of[a] as usize) {
                info.hints = self.hints_of(&m.hints);
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

    fn close_claims(&mut self, id: u64, done: impl FnMut(u64, u32) -> bool) {
        close_claims(&mut self.claims, &mut self.infos, id, done);
    }
}

/// Ends the claims `done` says, at step `id`.
fn close_claims(
    claims: &mut Vec<(u64, u32, u64)>,
    infos: &mut [Info],
    id: u64,
    mut done: impl FnMut(u64, u32) -> bool,
) {
    claims.retain(|&(inst, end, cid)| {
        if done(inst, end) {
            if let Some(info) = infos.get_mut(usize::try_from(cid).unwrap_or(usize::MAX)) {
                info.claim_n = u32::try_from(id.saturating_sub(cid + 1)).unwrap_or(NONE);
            }
            false
        } else {
            true
        }
    });
}

impl Converter<'_> {
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
            prev.h = NO_HINTS;
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
            class: 0,
            h: NO_HINTS,
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
            // (a step read from another file ends the claims in files: an
            // `\input`'s file name is its argument, not the file)
            let in_file = found.is_some_and(|(f, _)| f.is_none());
            let files = &self.file_insts;
            close_claims(&mut self.claims, &mut self.infos, id, |i, end| {
                (i == inst && end <= j) || (in_file && i != inst && files.contains(&i))
            });
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

        // Format v1's view: the class, and the pass's hints for the joined
        // atom; where nothing joined, what the trace itself shows.
        let class = match s.kind {
            Kind::Macro if s.name == "\\output" && keys.is_empty() => "page".to_owned(),
            Kind::Primitive | Kind::If | Kind::Macro => s.name.clone(),
            Kind::Char if s.name == "blank space" => "space".to_owned(),
            Kind::Char => "char".to_owned(),
            Kind::GroupBegin => "{".to_owned(),
            Kind::GroupEnd => "}".to_owned(),
            Kind::Font => "font".to_owned(),
        };
        info.class = self.strings.get(&class);
        if let Some(ai) = atom_info {
            info.h = ai.hints;
        } else {
            info.h.scope = match s.kind {
                Kind::GroupBegin => 1,
                Kind::GroupEnd => 2,
                Kind::Primitive if name == "begingroup" => 1,
                Kind::Primitive if name == "endgroup" => 2,
                _ => 0,
            };
            if is_cs && !keys.is_empty() {
                info.h.claim = 2;
                info.h.cause = self.strings.get("join-failed");
            }
            if s.kind == Kind::Macro || definer {
                info.h.may = self.define1(May1::Any);
            }
        }

        // Unfolds: a step read from a call's own text or below (the text
        // after the call, or an argument written there) ends the call.
        {
            let infos = &mut self.infos;
            self.calls.retain(|&(l, c)| {
                if l >= level {
                    infos[usize::try_from(c).expect("an index")].unfold_end = id;
                    false
                } else {
                    true
                }
            });
        }
        if s.kind == Kind::Macro {
            self.counts.macro_calls += 1;
            let infos = &mut self.infos;
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

    /// Pass 1.5's first half: each step's subtree end (a macro call's
    /// unfold, clipped to its parent's) and its top-level ancestor.
    #[must_use]
    fn tree(&self) -> Tree {
        let n = self.infos.len();
        let mut end = vec![0u32; n];
        let mut top = vec![0u32; n];
        let mut stack: Vec<u32> = Vec::new();
        let mut top_level = 0;
        for i in 0..n {
            while let Some(&t) = stack.last()
                && end[t as usize] as usize <= i
            {
                stack.pop();
            }
            let mut e = usize::try_from(self.infos[i].unfold_end)
                .unwrap_or(usize::MAX)
                .max(i + 1);
            if let Some(&p) = stack.last() {
                e = e.min(end[p as usize] as usize);
            }
            end[i] = u32::try_from(e).expect("fewer than 2^32 steps");
            let me = u32::try_from(i).expect("fewer than 2^32 steps");
            top[i] = stack.first().copied().unwrap_or(me);
            if stack.is_empty() {
                top_level += 1;
            }
            if e > i + 1 {
                stack.push(me);
            }
        }
        Tree {
            end,
            top,
            top_level,
            unsound: Vec::new(),
            delim: HashMap::new(),
        }
    }

    /// Whether step `i` claims as a stream (`predicted`, or a pending
    /// claim whose cause is a delimiter the trace shows).
    fn stream_claimer(&self, i: usize) -> bool {
        let h = self.infos[i].h;
        h.claim == 1 || (h.claim == 2 && delimited(self.string(h.cause)))
    }

    /// Pass 1.5's second half over one step: definition bounds checked
    /// against the writes of the subtrees they bound, and each stream
    /// claim's delimiter (the next sibling, a leaf that reads nothing the
    /// claimer's subtree writes).
    #[allow(clippy::many_single_char_names)]
    fn shape(&mut self, s: &Step, t: &mut Tree, st: &mut ShapeState) {
        let i = usize::try_from(s.id).expect("an index");
        let me = u32::try_from(i).expect("fewer than 2^32 steps");
        if t.unsound.is_empty() {
            t.unsound = vec![false; self.infos.len()];
        }
        while let Some(&(a, _)) = st.stack.last()
            && t.end[a as usize] as usize <= i
        {
            let (a, set) = st.stack.pop().expect("one");
            if let Some(set) = set {
                st.resolve.push((a, st.stack.len(), set));
            }
        }
        let mut field = |f: &str| -> u32 {
            if let Some(&k) = st.fields.get(f) {
                return k;
            }
            let k = u32::try_from(st.fields.len()).expect("fewer than 2^32 fields");
            st.fields.insert(f.to_owned(), k);
            k
        };
        let reads: Vec<u32> = s.reads.iter().map(|r| field(r)).collect();
        let writes: Vec<u32> = s.writes.iter().map(|w| field(&w.field)).collect();
        let leaf = t.end[i] as usize == i + 1;
        if st.stack.is_empty()
            && self.infos[i].h.scope == 1
            && material(self.string(self.infos[i].class))
        {
            t.top_level += 1;
        }
        for (c, depth, set) in std::mem::take(&mut st.resolve) {
            // (a claimer with a pending definition set binds the
            // delimiter's reads only once it runs: a read its bound
            // covers would wait on the claim it ends)
            let cu = c as usize;
            let pending_set = t.end[cu] as usize > cu + 1;
            let reads_claim = |r: &str| {
                pending_set
                    && (t.unsound[cu]
                        || st.own_writes.contains(&c)
                        || self.may1[self.infos[cu].h.may as usize].covers(r))
            };
            let blocked = s.reads.iter().any(|r| reads_claim(r));
            // (a reader of the list reads the claimer's contribution)
            let class = self.string(self.infos[i].class);
            let blocked = blocked || LIST_READERS.contains(&class) || class == "page";
            if depth == st.stack.len() && leaf && !blocked && !reads.iter().any(|r| set.contains(r))
            {
                let confirm = self.string(self.infos[cu].h.confirm);
                // (the confirm field is read at the delimiter: one the
                // claim itself may define would wait on the claim)
                let defined = st
                    .fields
                    .get(confirm)
                    .is_some_and(|f| st.written.contains(f) && !set.contains(f))
                    && !reads_claim(confirm);
                t.delim.insert(c, (self.infos[i].class, defined));
            } else {
                self.counts.claim_fallback += 1;
            }
        }
        if !writes.is_empty() {
            for (a, set) in &mut st.stack {
                let may = &self.may1[self.infos[*a as usize].h.may as usize];
                if s.writes.iter().any(|w| !may.covers(&w.field)) {
                    t.unsound[*a as usize] = true;
                }
                if let Some(set) = set {
                    set.extend(writes.iter().copied());
                }
            }
        }
        st.written.extend(writes.iter().copied());
        if !leaf && !s.writes.is_empty() && self.stream_claimer(i) {
            st.own_writes.insert(me);
        }
        let set = self.stream_claimer(i).then(|| {
            writes
                .iter()
                .copied()
                .collect::<std::collections::HashSet<u32>>()
        });
        if leaf {
            if let Some(set) = set {
                st.resolve.push((me, st.stack.len(), set));
            }
        } else {
            st.stack.push((me, set));
        }
    }

    #[allow(clippy::many_single_char_names)]
    /// Pass 2 over one step: its format v1 record, nested in its parent's
    /// `children` (the record stays open while its own children follow).
    #[allow(clippy::too_many_lines)]
    fn emit<W: io::Write>(
        &mut self,
        s: &Step,
        t: &Tree,
        st: &mut EmitState,
        out: &mut W,
    ) -> io::Result<()> {
        let i = usize::try_from(s.id).expect("an index");
        while let Some(&(e, _)) = st.stack.last()
            && e as usize <= i
        {
            st.stack.pop();
            out.write_all(b"]}")?;
            if st.stack.is_empty() {
                out.write_all(b"\n")?;
            }
        }
        if let Some(top) = st.stack.last_mut() {
            if top.1 > 0 {
                out.write_all(b",")?;
            }
            top.1 += 1;
        }
        let top_level = st.stack.is_empty();
        let info = self.infos[i];
        let class = self.string(info.class).to_owned();
        let has_children = t.end[i] as usize > i + 1;
        let mut o = String::with_capacity(256);
        let _ = write!(o, "{{\"id\":\"s{i}\",\"class\":{}", q(&class));
        // pos: the text order, strictly increasing at the top level.
        let mut pos = i64::try_from(info.order).unwrap_or(i64::MAX);
        if top_level {
            if let Some(last) = st.last_pos
                && pos <= last
            {
                pos = last + 1;
                self.counts.pos_clamped += 1;
            }
            st.prev_pos = st.last_pos;
            st.last_pos = Some(pos);
        }
        let _ = write!(o, ",\"pos\":{pos}");
        // reads, and the environment's.
        let mut env: Vec<&str> = Vec::new();
        if let Some(e) = env_of(&class) {
            env.push(e);
        }
        let mut reads: Vec<&str> = Vec::new();
        for r in &s.reads {
            if r == "?" {
                continue;
            }
            if let Some(e) = r.strip_prefix("param:\\").and_then(env_of_param) {
                if !env.contains(&e) {
                    env.push(e);
                }
                continue;
            }
            if !reads.contains(&r.as_str()) {
                reads.push(r);
            }
        }
        o.push_str(",\"reads\":[");
        for (k, r) in reads.iter().enumerate() {
            if k > 0 {
                o.push(',');
            }
            o.push_str(&q(r));
        }
        o.push(']');
        // writes, or a pending set and its bound.
        if has_children && s.writes.is_empty() {
            let may = if t.unsound[i] {
                self.counts.may_unsound += 1;
                May1::Any
            } else {
                self.may1[info.h.may as usize].clone()
            };
            match &may {
                May1::None => self.counts.may1_none += 1,
                May1::Names(_) => self.counts.may1_names += 1,
                May1::Prefixes(_) => self.counts.may1_prefixes += 1,
                May1::Any => self.counts.may1_any += 1,
            }
            o.push_str(",\"writes\":null,\"may_define\":");
            o.push_str(&may_json(&may));
        } else {
            if has_children {
                self.counts.may_own_writes += 1;
            }
            o.push_str(",\"writes\":[");
            for (k, w) in s.writes.iter().enumerate() {
                if k > 0 {
                    o.push(',');
                }
                let _ = write!(
                    o,
                    "{{\"field\":{},\"locality\":\"{}\"}}",
                    q(&w.field),
                    if w.global { "global" } else { "local" }
                );
            }
            o.push(']');
        }
        // scope.
        let (mut sc, kind, mut imp) = (info.h.scope, info.h.kind, info.h.implicit);
        if imp != 0 && sc != 0 {
            self.counts.implicit_dropped += 1;
            imp = 0;
        }
        if imp != 0 {
            sc = 3;
        }
        if top_level {
            if sc == 2 && st.depth == Some(0) {
                self.counts.scope_unmatched += 1;
                sc = 3;
            }
            st.depth = match (sc, st.depth) {
                (0, d) => d,
                (1, Some(d)) => Some(d + 1),
                (2, Some(d)) => Some(d - 1),
                _ => None,
            };
        }
        match sc {
            1 => {
                o.push_str(",\"scope\":\"open\"");
                if kind != 0 {
                    let k = ["V", "H", "M", "D"][usize::from(kind - 1)];
                    let _ = write!(o, ",\"scope_kind\":\"{k}\"");
                }
            }
            2 => o.push_str(",\"scope\":\"close\""),
            3 => {
                o.push_str(",\"scope\":\"pending\"");
                match imp {
                    1 => o.push_str(",\"starts_implicit\":\"H\""),
                    2 => o.push_str(",\"closes_implicit\":\"H\""),
                    _ => {}
                }
            }
            _ => {}
        }
        // the current list: contributions, reads of the one before, the
        // page builder's fold.
        let name = class.as_str();
        let h_only = H_MATERIAL.contains(&name) || name == "char";
        let v_only = V_MATERIAL.contains(&name);
        if h_only || imp == 1 {
            st.in_par = true;
        }
        if v_only {
            st.in_par = false;
        }
        let list = if st.in_par { "hlist" } else { "vlist" };
        // (an opener contributes nothing: what it opens, a box or a
        // formula, goes to the list when it closes, §1086, §1196)
        let contributes = if h_only {
            Some("hlist")
        } else if v_only {
            Some("vlist")
        } else if EITHER_MATERIAL.contains(&name) || (name == "space" && st.in_par) {
            Some(list)
        } else {
            None
        };
        // (an opener's contribution is a step of its own before it: the
        // engine takes no contribution from a step that opens a scope)
        let mut pre = String::new();
        if let Some(c) = contributes {
            self.counts.contributes += 1;
            if sc == 1 {
                self.counts.opener_contributions += 1;
                let mut cpos = pos;
                if top_level {
                    if st.prev_pos.is_none_or(|p| pos - 1 > p) {
                        cpos = pos - 1;
                    } else {
                        o = o.replacen(
                            &format!(",\"pos\":{pos}"),
                            &format!(",\"pos\":{}", pos + 1),
                            1,
                        );
                        st.last_pos = Some(pos + 1);
                    }
                }
                let _ = write!(
                    pre,
                    "{{\"id\":\"s{i}c\",\"class\":\"contribution\",\"pos\":{cpos},\"reads\":[],\"writes\":[],\"contributes\":\"{c}\"}}{}",
                    if top_level { "\n" } else { "," }
                );
            } else {
                let _ = write!(o, ",\"contributes\":\"{c}\"");
            }
        }
        if LIST_READERS.contains(&name) {
            self.counts.reads_prev += 1;
            let _ = write!(o, ",\"reads_prev_contribution\":\"{list}\"");
        }
        if imp == 2 || name == "\\par" {
            st.in_par = false;
        }
        if name == "page" {
            self.counts.folds += 1;
            o.push_str(",\"fold\":{\"sequence\":\"vlist\",\"step_class\":\"page\"}");
        }
        // claim.
        let delim = t.delim.get(&u32::try_from(i).expect("an index"));
        match (info.h.claim, delim) {
            (0, _) => {
                self.counts.claim1_known += 1;
                o.push_str(",\"claim\":{\"known\":0}");
            }
            (1, Some(&(c, defined))) => {
                self.counts.claim1_predicted += 1;
                let confirm = if defined {
                    q(self.string(info.h.confirm))
                } else {
                    self.counts.confirm_dropped += 1;
                    String::new()
                };
                let _ = write!(
                    o,
                    ",\"claim\":{{\"predicted\":{{\"n\":0,\"until\":{{\"class\":{},\"inert\":false,\"confirm\":[{confirm}]}}}}}}",
                    q(self.string(c)),
                );
            }
            (2, Some(&(c, _))) => {
                self.counts.claim1_until += 1;
                let _ = write!(
                    o,
                    ",\"claim\":{{\"until\":{{\"class\":{},\"inert\":false,\"confirm\":[]}}}}",
                    q(self.string(c))
                );
            }
            (1 | 2, None) => {
                self.counts.claim1_null += 1;
                o.push_str(",\"claim\":null,\"value_dep\":true");
            }
            _ => {}
        }
        if !env.is_empty() {
            self.counts.env_steps += 1;
            o.push_str(",\"env_reads\":[");
            for (k, e) in env.iter().enumerate() {
                if k > 0 {
                    o.push(',');
                }
                o.push_str(&q(e));
            }
            o.push(']');
            for e in env {
                if !st.env.contains(&e) {
                    st.env.push(e);
                }
            }
        }
        let cause = match (info.h.claim, delim) {
            (2, None) if info.h.cause != NONE => info.h.cause,
            _ => info.cause,
        };
        if cause != NONE {
            let _ = write!(o, ",\"cause\":{}", q(self.string(cause)));
        }
        if has_children {
            o.push_str(",\"children\":[");
            st.stack.push((t.end[i], 0));
        } else {
            o.push('}');
            if top_level {
                o.push('\n');
            }
        }
        out.write_all(pre.as_bytes())?;
        out.write_all(o.as_bytes())
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

/// The nesting of the output and what pass 1.5 decided per step.
pub struct Tree {
    /// Each step's subtree end (exclusive).
    end: Vec<u32>,
    /// Each step's top-level ancestor (itself at the top level).
    top: Vec<u32>,
    top_level: u64,
    /// A pending definition set whose bound a write in its subtree breaks.
    unsound: Vec<bool>,
    /// A stream claimer's delimiter class (a string), and whether its
    /// confirm field has a definition before the delimiter.
    delim: HashMap<u32, (u32, bool)>,
}

#[derive(Default)]
struct ShapeState {
    /// Open subtrees, with the fields a stream claimer's subtree writes.
    stack: Vec<(u32, Option<std::collections::HashSet<u32>>)>,
    /// Claimers whose delimiter is the next step: (claimer, stack depth,
    /// fields its subtree writes).
    resolve: Vec<(u32, usize, std::collections::HashSet<u32>)>,
    fields: HashMap<String, u32>,
    /// Claimers with children and writes of their own (their bound is
    /// `any`).
    own_writes: std::collections::HashSet<u32>,
    /// The fields written so far.
    written: std::collections::HashSet<u32>,
}

#[derive(Default)]
struct EmitState {
    /// Open records: (subtree end, children written).
    stack: Vec<(u32, u32)>,
    depth: Option<i64>,
    last_pos: Option<i64>,
    /// The top-level `pos` before the current step's.
    prev_pos: Option<i64>,
    /// Whether a paragraph is being built (approximate: see `REPLAY.md`).
    in_par: bool,
    env: Vec<&'static str>,
}

/// A JSON string.
fn q(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_owned())
}

fn may_json(m: &May1) -> String {
    let list = |key: &str, v: &[(String, bool)]| {
        let items: Vec<String> = v
            .iter()
            .map(|(f, g)| {
                format!(
                    "{{\"{key}\":{},\"locality\":\"{}\"}}",
                    q(f),
                    if *g { "global" } else { "local" }
                )
            })
            .collect();
        items.join(",")
    };
    match m {
        May1::None => "\"none\"".to_owned(),
        May1::Any => "\"any\"".to_owned(),
        May1::Names(v) => format!("{{\"names\":[{}]}}", list("name", v)),
        May1::Prefixes(v) => format!("{{\"prefixes\":[{}]}}", list("prefix", v)),
    }
}

/// Whether a pending claim's cause is a delimiter the trace shows (the
/// claim ends where the trace's next step begins).
fn delimited(cause: &str) -> bool {
    cause.contains("delimit") || cause.contains("verbatim") || cause.contains("parameter text")
}

/// The environment a primitive reads: the clock or the file system.
fn env_of(class: &str) -> Option<&'static str> {
    match class {
        "\\time" | "\\day" | "\\month" | "\\year" | "\\pdfcreationdate" => Some("time"),
        "\\input" | "\\openin" | "\\read" | "\\readline" | "\\pdffilesize" | "\\pdffilemoddate"
        | "\\pdfmdfivesum" | "\\pdffiledump" | "\\pdfximage" => Some("files"),
        _ => None,
    }
}

/// The environment a parameter read is (`\time` and its kin).
fn env_of_param(p: &str) -> Option<&'static str> {
    matches!(p, "time" | "day" | "month" | "year").then_some("time")
}

/// Whether a step of this class adds material to a list.
fn material(class: &str) -> bool {
    class == "char"
        || class == "space"
        || H_MATERIAL.contains(&class)
        || V_MATERIAL.contains(&class)
        || EITHER_MATERIAL.contains(&class)
}

/// Horizontal material (tex.web §1090: it starts a paragraph in
/// vertical mode).
const H_MATERIAL: &[&str] = &[
    "\\char",
    "\\hskip",
    "\\hfil",
    "\\hfill",
    "\\hss",
    "\\hfilneg",
    "\\unhbox",
    "\\unhcopy",
    "\\discretionary",
    "\\-",
    "\\accent",
    "\\ ",
    "\\vrule",
    "\\valign",
    "\\noboundary",
    "\\indent",
    "\\noindent",
    "\\quitvmode",
];

/// Vertical material (§1094: it ends the paragraph in horizontal mode).
const V_MATERIAL: &[&str] = &[
    "\\vskip",
    "\\vfil",
    "\\vfill",
    "\\vss",
    "\\vfilneg",
    "\\hrule",
    "\\unvbox",
    "\\unvcopy",
    "\\halign",
];

/// Material the current mode's list takes.
const EITHER_MATERIAL: &[&str] = &[
    "\\kern",
    "\\penalty",
    "\\hbox",
    "\\vbox",
    "\\vtop",
    "\\box",
    "\\copy",
    "\\mark",
    "\\marks",
    "\\special",
    "\\write",
    "\\openout",
    "\\closeout",
    "\\immediate",
    "\\insert",
    "\\vadjust",
    "\\leaders",
    "\\cleaders",
    "\\xleaders",
    "\\pdfliteral",
    "\\pdfrefxform",
    "\\pdfrefximage",
    "\\pdfdest",
    "\\pdfstartlink",
    "\\pdfendlink",
    "\\pdfannot",
    "\\pdfsave",
    "\\pdfrestore",
    "\\pdfsetmatrix",
    "\\pdfcolorstack",
    "\\pdfsavepos",
];

/// The current list's readers (the catalogue's F2): the contribution
/// before (or the whole list, `\par`).
const LIST_READERS: &[&str] = &[
    "\\lastbox",
    "\\unpenalty",
    "\\unkern",
    "\\unskip",
    "\\lastpenalty",
    "\\lastkern",
    "\\lastskip",
    "\\lastnodetype",
    "\\prevgraf",
    "\\spacefactor",
    "\\prevdepth",
    "\\/",
    "\\displaylimits",
    "\\limits",
    "\\nolimits",
    "\\above",
    "\\over",
    "\\atop",
    "\\abovewithdelims",
    "\\overwithdelims",
    "\\atopwithdelims",
    "\\middle",
    "\\par",
    "\\badness",
    "\\showlists",
];

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

/// Converts the trace `open` reads (opened four times: four passes) to
/// format v1 on `out`, reading source files through `load`, and makes the
/// records of the planned `edits` (`graph`, the edit-impact analysis's,
/// gives the expectations of edits retargeted to a top-level call).
/// Returns the provenance object (for `meta.json`), the edit records and
/// their provenance (one per edit, for `edits.meta.jsonl`).
///
/// # Errors
///
/// The trace's or the output's I/O errors, a malformed trace.
pub fn convert<R: BufRead, W: io::Write>(
    mut open: impl FnMut() -> io::Result<R>,
    load: impl FnMut(&str) -> Option<String>,
    prov: &Provenance,
    edits: &[EditPlan],
    graph: Option<&crate::analyze::Graph>,
    out: &mut W,
) -> io::Result<Converted> {
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
    // Pass 1.5: the nesting, the bounds, the delimiters.
    let mut tree = c.tree();
    let mut shape = ShapeState::default();
    steps(open()?, |s| {
        c.shape(&s, &mut tree, &mut shape);
        Ok(())
    })?;
    drop(shape);
    let meta = meta.unwrap_or(crate::step::Meta {
        main: String::new(),
        engine: String::new(),
        conservative: false,
        version: None,
        command: None,
    });
    // Pass 2: the records, nested.
    let wanted: std::collections::HashSet<u64> = edits.iter().map(|e| e.target).collect();
    let mut target_writes: HashMap<u64, Vec<crate::step::Write>> = HashMap::new();
    let mut st = EmitState {
        depth: Some(0),
        ..EmitState::default()
    };
    // (the meta line comes first: it declares the whole environment a
    // step may read, `meta.json` names the part the steps read)
    let env = ["files", "time"];
    writeln!(
        out,
        "{{\"meta\":{{\"version\":1,\"document\":{},\"steps\":{},\"sequences\":[\"vlist\",\"hlist\"],\"env\":{{{}}}}}}}",
        q(&prov.document),
        tree.top_level,
        env.iter()
            .map(|e| format!("{}:1", q(e)))
            .collect::<Vec<_>>()
            .join(",")
    )?;
    steps(open()?, |s| {
        if wanted.contains(&s.id) {
            target_writes.insert(s.id, s.writes.clone());
        }
        c.emit(&s, &tree, &mut st, out)
    })?;
    while st.stack.pop().is_some() {
        out.write_all(b"]}")?;
        if st.stack.is_empty() {
            out.write_all(b"\n")?;
        }
    }
    out.flush()?;
    let mut counts = c.tally();
    counts.fields = fields.len() as u64;
    counts.top_level = tree.top_level;
    let (edit_values, edit_meta) = edit_records(&c, edits, &tree, &target_writes, graph);
    counts.edits = edit_values.len() as u64;
    let meta_v = serde_json::json!({
        "format": 1,
        "document": prov.document,
        "main": meta.main,
        "engine": meta.engine,
        "engine_command": meta.command,
        "pdftex_version": meta.version,
        "trace_commit": prov.trace_commit,
        "conservative": meta.conservative,
        "conventions": CONVENTIONS,
        "env_read": st.env,
        "counts": counts,
    });
    Ok(Converted {
        meta: meta_v,
        edits: edit_values,
        edits_meta: edit_meta,
    })
}

/// What [`convert`] returns beside the trace.
pub struct Converted {
    /// The provenance and counts (`meta.json`).
    pub meta: serde_json::Value,
    /// The edits, format v1 (`edits.jsonl`).
    pub edits: Vec<serde_json::Value>,
    /// Each edit's provenance (`edits.meta.jsonl`).
    pub edits_meta: Vec<serde_json::Value>,
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

/// Chooses `count` edits over the edit-impact sample's lines (the same
/// `count` lines the analysis samples, seed 1): line by line in the
/// sample's order, round after round while the document has fewer lines,
/// each a form and one of the line's writing steps drawn with a fixed seed
/// (2), no (form, step) twice; fewer when the lines' writers run out.
#[must_use]
pub fn plan_edits(g: &crate::analyze::Graph, count: usize) -> Vec<EditPlan> {
    let mut rng = fastrand::Rng::with_seed(2);
    let samples = g.edit_samples(count);
    let mut seen = std::collections::HashSet::new();
    let mut plans = Vec::new();
    let capacity: usize = samples.iter().map(|e| 3 * e.writers.len()).sum();
    let mut misses = 0;
    while plans.len() < count.min(capacity) && misses < 100 * count {
        for e in &samples {
            if plans.len() >= count {
                break;
            }
            let form = rng.u8(0..3);
            let target = e.writers[rng.usize(0..e.writers.len())];
            if !seen.insert((form, target)) {
                misses += 1;
                continue;
            }
            plans.push(EditPlan {
                form,
                target: u64::from(target),
                line: e.line,
                impact_line: e.impact,
                impact_step: g.dependents_of(target, &e.steps),
            });
        }
    }
    plans
}

/// The edits, format v1, and their provenance. An edit names a top-level
/// step: a sampled step inside a call is reached through its top-level
/// ancestor (see `REPLAY.md`), and an ancestor an earlier edit deleted
/// moves to the top-level step before it.
fn edit_records(
    c: &Converter<'_>,
    edits: &[EditPlan],
    t: &Tree,
    writes: &HashMap<u64, Vec<crate::step::Write>>,
    graph: Option<&crate::analyze::Graph>,
) -> (Vec<serde_json::Value>, Vec<serde_json::Value>) {
    let mut deleted: std::collections::HashSet<u32> = std::collections::HashSet::new();
    let (mut out, mut meta) = (Vec::new(), Vec::new());
    for (k, e) in edits.iter().enumerate() {
        let target = u32::try_from(e.target).expect("an index");
        let mut anchor = t.top[target as usize];
        while deleted.contains(&anchor) && anchor > 0 {
            anchor = t.top[anchor as usize - 1];
        }
        if deleted.contains(&anchor) {
            continue;
        }
        let direct = anchor == target;
        let ws: Vec<serde_json::Value> = writes
            .get(&e.target)
            .map(|w| {
                w.iter()
                    .map(|w| {
                        serde_json::json!({"field": w.field,
                            "locality": if w.global { "global" } else { "local" }})
                    })
                    .collect()
            })
            .unwrap_or_default();
        let (op, v, expect) = match e.form {
            0 if direct => (
                "change_writes",
                serde_json::json!({"op": "change_writes", "step": format!("s{target}"),
                    "writes": ws}),
                e.impact_step,
            ),
            2 => {
                deleted.insert(anchor);
                let expect = if direct {
                    e.impact_step
                } else {
                    let sub: Vec<u32> = (anchor..t.end[anchor as usize]).collect();
                    graph.map_or(e.impact_step, |g| g.dependents_of_steps(&sub))
                };
                (
                    "delete",
                    serde_json::json!({"op": "delete", "step": format!("s{anchor}")}),
                    expect,
                )
            }
            _ => {
                let new = serde_json::json!({"id": format!("e{k}"), "class": "inserted",
                    "pos": i64::try_from(c.infos[anchor as usize].order).unwrap_or(0),
                    "reads": [], "writes": ws, "cause": "inserted"});
                (
                    "insert_after",
                    serde_json::json!({"op": "insert_after", "step": format!("s{anchor}"),
                        "new": new}),
                    e.impact_step,
                )
            }
        };
        let mut v = v;
        v["expect"] = serde_json::json!({ "steps_reached": expect });
        out.push(v);
        let form = ["change_writes", "insert_after", "delete"][usize::from(e.form.min(2))];
        meta.push(serde_json::json!({
            "edit": out.len(),
            "op": op,
            "form": form,
            "target": format!("s{target}"),
            "anchor": format!("s{anchor}"),
            "line": e.line,
            "expected_impact": e.impact_line,
            "expected_impact_step": e.impact_step,
        }));
    }
    (out, meta)
}

/// The `INDEX.md` table over replay directories' `meta.json`s.
#[must_use]
pub fn index(metas: &[(String, serde_json::Value)]) -> String {
    let mut s = String::from(
        "| document | steps | top-level | fields | claim known / predicted / until / null % | pending sets: may_define none / names / prefixes / any % (unsound) | join failures (% of steps) | pos clamped | edits |\n|---|---|---|---|---|---|---|---|---|\n",
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
        let sets = c.may1_none + c.may1_names + c.may1_prefixes + c.may1_any;
        let _ = writeln!(
            s,
            "| `{name}` | {n} | {} | {} | {:.1} / {:.1} / {:.2} / {:.1} | {:.1} / {:.1} / {:.1} / {:.1} ({}) | {} ({:.2}%) | {} | {} |",
            c.top_level,
            c.fields,
            pct(c.claim1_known, n),
            pct(c.claim1_predicted, n),
            pct(c.claim1_until, n),
            pct(c.claim1_null, n),
            pct(c.may1_none, sets),
            pct(c.may1_names, sets),
            pct(c.may1_prefixes, sets),
            pct(c.may1_any, sets),
            c.may_unsound,
            c.join_failures,
            pct(c.join_failures, n),
            c.pos_clamped,
            c.edits,
        );
    }
    s
}

#[cfg(test)]
mod tests {
    use super::{May1, delimited, expl_names, may_json};

    #[test]
    fn expl_bodies_are_told() {
        assert!(expl_names("\\exp_after:wN \\use_i:nn"));
        assert!(!expl_names("\\def \\x {a_b:c}"));
    }

    #[test]
    fn bounds_are_written_with_localities() {
        let m = May1::Names(vec![("cs:\\x".to_owned(), true)]);
        assert_eq!(
            may_json(&m),
            r#"{"names":[{"name":"cs:\\x","locality":"global"}]}"#
        );
        assert!(m.covers("cs:\\x") && !m.covers("cs:\\y"));
        let p = May1::Prefixes(vec![("cs:\\e".to_owned(), false)]);
        assert!(p.covers("cs:\\e12"));
        assert_eq!(may_json(&May1::None), "\"none\"");
        assert!(delimited("delimiter not in enclosing group"));
        assert!(!delimited("macro defined elsewhere"));
    }
}
