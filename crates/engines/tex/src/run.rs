//! One run of one node (a paragraph region, a loading command, a whole
//! format): its input (the node's own text, the files it reads inline,
//! the siblings it claims while a span is open), `get_next` (§341–§365)
//! over it, and the definitions, contributions and children it makes,
//! buffered until the run is done.
//!
//! Everything here is local to one run: a small sequential scanner over
//! the node's own text. What TeX keeps between commands is read through
//! fields (`Run::get`), the scope kind and contributions.

use std::collections::VecDeque;
use std::rc::Rc;

use phitex_defuse::{Ctx, Effect, Field, FxHashMap, Locality, OpId, Stop, Value, ValueId, Values};

use crate::val::{Mean, Tok, cmd, int_of, list_of, str_of};

/// Why a run cannot finish its command now.
pub(crate) enum Halt {
    /// A value is not known yet (the engine re-runs the node).
    Stop(Stop),
    /// The command needs more input than the node can claim: a child at
    /// the end of its parent's text (the claim at the call measures what
    /// a body's tail takes, so this is a bug when it happens).
    Starve,
    /// The input ended for good inside a command (§360: in batch mode,
    /// "*** (job aborted, no legal \end found)").
    Fatal,
}

impl From<Stop> for Halt {
    fn from(s: Stop) -> Halt {
        Halt::Stop(s)
    }
}

pub(crate) type R<T> = Result<T, Halt>;

/// The operations, as registered.
#[derive(Clone)]
pub(crate) struct Ops {
    pub exec: OpId,
    pub put: OpId,
    pub pagestep: OpId,
    pub pagestep_any: OpId,
    pub breaks: OpId,
    /// The build's field handles (`crate::fields`).
    pub fields: std::sync::Arc<crate::fields::Fields>,
    /// The format's definitions the build analyses document texts with
    /// (set by `api::Build::with`; none while a format is built): a
    /// document file `\input` is sliced with them ([`Run::leftover`]).
    pub defs: std::sync::Arc<std::sync::Mutex<crate::hints::Defs>>,
}

/// A level of the input stack.
#[derive(Clone, Debug)]
pub(crate) enum Frame {
    /// A token list and the position in it.
    Toks(Rc<[Tok]>, usize),
    /// A line of a file (§300): its characters (with `\endlinechar`
    /// appended once read), position and state.
    Src(Src),
    /// The lines of a file not reached yet: its text (a string value)
    /// and the byte offset of the next line. A line becomes a [`Src`]
    /// frame when it is reached, so the rest of a file is one frame, and
    /// one child when a node leaves it (never one per line).
    File(FileRest),
}

#[derive(Clone, Debug)]
pub(crate) struct FileRest {
    pub id: ValueId,
    pub text: std::sync::Arc<phitex_defuse::Value>,
    pub off: usize,
    /// The file's name, for positions (a string value).
    pub name: ValueId,
    /// The characters before `off` (kept as lines are taken: counting
    /// them from the start at every line made reading a file quadratic).
    pos: usize,
}

impl FileRest {
    pub(crate) fn new(v: &Values, id: ValueId, off: usize, name: ValueId) -> FileRest {
        let text = v.get(id);
        let pos = match &*text {
            phitex_defuse::Value::Str(s) => s.get(..off).map_or(0, |p| p.chars().count()),
            _ => 0,
        };
        FileRest {
            id,
            text,
            off,
            name,
            pos,
        }
    }

    /// The position of the next line in the file (its characters are the
    /// file's bytes).
    fn byte_pos(&self) -> usize {
        self.pos
    }

    /// The file's name and the line number of the next line, for
    /// diagnostics ([`Run::heartbeat`]).
    pub(crate) fn where_(&self, v: &Values) -> (String, usize) {
        let line = self.str()[..self.off].matches('\n').count() + 1;
        (crate::val::str_of(v, self.name), line)
    }

    fn str(&self) -> &str {
        match &*self.text {
            phitex_defuse::Value::Str(s) => s,
            _ => "",
        }
    }

    /// Whether lines are left.
    pub(crate) fn has_lines(&self) -> bool {
        self.off < self.str().len()
    }

    /// The next line, with its end of line; the offset moves past it.
    fn take_line(&mut self) -> String {
        let s = self.str();
        let rest = &s[self.off..];
        let n = rest.find('\n').map_or(rest.len(), |k| k + 1);
        let line = rest[..n].to_owned();
        self.pos += line.chars().count();
        self.off += n;
        line
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Src {
    pub buf: Vec<u32>,
    pub loc: usize,
    /// 0 `mid_line`, 1 `skip_blanks`, 2 `new_line` (§303).
    pub state: u8,
    /// -2 until the segment is reached (`\endlinechar` not yet applied).
    pub endch: i64,
    /// Whether the segment begins a line (it is counted), and whether it
    /// ends one (`\endlinechar` goes after it).
    pub starts: bool,
    pub ends: bool,
    /// Where `buf[0]` is in a file: the file's name and byte position
    /// (lines of `\input` files; for diagnostics only).
    pub origin: Option<(ValueId, usize)>,
    /// Text read under the catcodes in force when it is read, which no
    /// analysis of a file saw: a pseudo file's lines (pdfTeX §1766) and
    /// a `\read` line (§483).
    pub rescanned: bool,
}

pub(crate) const MID_LINE: u8 = 0;
pub(crate) const SKIP_BLANKS: u8 = 1;
pub(crate) const NEW_LINE: u8 = 2;

/// The current token (`cur_tok`, `cur_cmd`, `cur_chr`, `cur_cs`).
#[derive(Clone, Debug)]
pub(crate) struct Cur {
    pub tok: Tok,
    pub cmd: u8,
    pub chr: i32,
    pub mean: Mean,
}

impl Cur {
    pub(crate) fn is_cs(&self) -> bool {
        self.tok.is_cs_or_active()
    }
}

/// A decoded scope kind: `(group, sid, lid, mode, depth, extra)`.
pub(crate) type KindFields = (i64, i64, i64, i32, i64, Vec<ValueId>);

#[allow(clippy::struct_excessive_bools)]
pub(crate) struct Run<'c, 'a> {
    /// Encodings through the worker's cache at this run's values epoch
    /// while the run lives ([`crate::val::EpochScope`]).
    _scope: crate::val::EpochScope,
    pub cx: &'c mut Ctx<'a>,
    pub v: &'a Values,
    pub ops: Ops,
    pub frames: VecDeque<Frame>,
    pub pulled: usize,
    /// Whether the node is the document's text (no parent): the end of
    /// its following siblings is the end of the file.
    pub top: bool,
    /// Whether `\end` takes every following sibling.
    pub claim_rest: bool,
    /// The semantic nest (§211): the lists and groups this run has open
    /// ([`crate::nest`]), level 0 being the engine's scope.
    pub nest: Vec<crate::nest::Level>,
    /// The last local list id given out (negative: never a node's).
    pub next_lid: i64,
    /// The `(language, packed hyphen minima)` of the paragraph being
    /// broken (`end_graf` pops its level before `line_break`, §1096).
    pub last_par: (i64, i64),
    /// Whether the node is a piece of a document's text (its segments
    /// carry their origin): a document file `\input` there is sliced
    /// into regions ([`Run::leftover`]); a format's or a package's node
    /// reads every file inline.
    pub doc: bool,
    /// The files this run opened: name and a hash of the text read
    /// (flushed to the `fread` accumulator by [`Run::finish`]).
    pub fread: Vec<(String, u64)>,
    /// While looking for the command's first token: a node's command
    /// begins in its own text (the next sibling's begins in its own).
    pub no_pull: bool,
    /// The category codes this run read (§232), by character below
    /// 256, plus one (0: not read yet); emptied whenever `own` changes.
    cats: [u8; 256],
    pub own: FxHashMap<Field, Option<ValueId>>,
    pub defs: Vec<(Field, ValueId, Locality)>,
    /// Where each field's definitions are in `defs`, latest last: a
    /// redefinition within the same group replaces in place instead of
    /// growing `defs` (and no linear search finds the earlier one).
    pub def_ix: FxHashMap<Field, Vec<usize>>,
    /// Per field, the stamp of its latest global definition inside a
    /// group of this run: saved entries stamped before it are inert.
    pub ginert: FxHashMap<Field, u64>,
    /// The clock of `ginert` and the saved entries (`nest.rs`).
    pub stamp: u64,
    pub contribs: Vec<(Field, ValueId)>,
    /// The page builder of a text run as one node, paging inline
    /// (`pageinline.rs`).
    pub pg: Option<Box<crate::pageinline::Inline>>,
    pub kids: Vec<(OpId, Vec<ValueId>)>,
    pub effect: Effect,
    pub cur: Cur,
    /// Conditionals begun in this run: their `if_limit` (§489).
    pub conds: Vec<u8>,
    /// The codes (`cur_if`) of `conds`, parallel to it.
    pub if_codes: Vec<i32>,
    mcache: FxHashMap<ValueId, Mean>,
    /// The meaning fields of the control sequences this run looked up,
    /// by [`crate::val::CsId::index`], and of the active characters.
    tfields: Vec<Option<Field>>,
    afields: FxHashMap<u32, Field>,
    /// The buffer a control sequence's name is scanned into (§356).
    name_buf: String,
    /// The scope kind last decoded (`Run::mode`, once a command): its
    /// value and its fields `(group, sid, lid, mode, depth, extra)`.
    pub kind_memo: Option<(ValueId, KindFields)>,
    /// `align_state` as this run has it (§309), once read.
    pub astate: Option<i64>,
    /// Lines entered by this run (for `\inputlineno`).
    pub lines_entered: i64,
    /// Whether the input is exhausted for good (the file ended).
    pub eof: bool,
    /// `radix` (§438) and `cur_order` (§447).
    pub radix: i32,
    pub cur_order: phitex_typeset::glue::Order,
    /// The objects this run made at read time in relaxed PDF mode (their
    /// owner keys: the node and the occurrence).
    pub pdf_occ: usize,
}

impl<'c, 'a> Run<'c, 'a> {
    pub(crate) fn new(cx: &'c mut Ctx<'a>, ops: Ops) -> Self {
        let v = cx.values();
        let op = cx.op();
        let code = if op == ops.exec {
            1
        } else if op == ops.put {
            2
        } else {
            0
        };
        crate::exec::note(code, cx.node().raw());
        Run {
            _scope: crate::val::EpochScope::enter(v, cx.values_epoch()),
            cx,
            v,
            ops,
            frames: VecDeque::new(),
            pulled: 0,
            top: false,
            claim_rest: false,
            nest: Vec::new(),
            next_lid: 0,
            last_par: (0, 0),
            doc: false,
            fread: Vec::new(),
            no_pull: false,
            own: FxHashMap::default(),
            cats: [0; 256],
            defs: Vec::new(),
            def_ix: FxHashMap::default(),
            ginert: FxHashMap::default(),
            stamp: 0,
            contribs: Vec::new(),
            pg: None,
            kids: Vec::new(),
            effect: Effect::None,
            cur: Cur {
                tok: Tok::C(0),
                cmd: 0,
                chr: 0,
                mean: Mean::Undefined,
            },
            conds: Vec::new(),
            if_codes: Vec::new(),
            mcache: FxHashMap::default(),
            tfields: Vec::new(),
            afields: FxHashMap::default(),
            name_buf: String::new(),
            kind_memo: None,
            astate: None,
            lines_entered: 0,
            eof: false,
            radix: 0,
            cur_order: phitex_typeset::glue::Order::Normal,
            pdf_occ: 0,
        }
    }

    /// Forgets the category codes read: a definition may have changed one.
    pub(crate) fn forget_cats(&mut self) {
        self.cats = [0; 256];
    }

    // ------------------------------------------------------------ fields

    pub(crate) fn get_f(&mut self, f: Field) -> R<Option<ValueId>> {
        if let Some(&x) = self.own.get(&f) {
            return Ok(x);
        }
        let r = self.cx.use_field(f);
        if crate::exec::env_flag!("PHITEX_READS") {
            self.trace_read(f, r.is_err());
        }
        Ok(r?)
    }

    /// `PHITEX_READS`: a read under a head a paragraph region declares
    /// (where a following region's walk may park), or any read that
    /// parked, with the node and its operation.
    #[cold]
    fn trace_read(&mut self, f: Field, parked: bool) {
        const HEADS: [&str; 6] = [
            "pg:",
            "parshape",
            "box:",
            "lvlfix",
            "badness",
            "markclasses",
        ];
        let name = crate::val::field_name(self.v, f);
        if parked || HEADS.iter().any(|h| name.starts_with(h)) {
            eprintln!(
                "reads: node {} op {:?} {name}{}",
                self.cx.node().raw(),
                self.cx.op(),
                if parked { " PARKED" } else { "" }
            );
        }
    }

    /// The file `key` from the client (`file:<key>`): `None` if it is
    /// absent; until the client supplies it or says it is absent, the
    /// node waits ([`phitex_defuse::Ctx::use_env`]). A file the job
    /// writes and reads back is a loop-back field: the previous round's.
    pub(crate) fn file(&mut self, key: &str) -> R<Option<ValueId>> {
        let v = self.v;
        // a file the job writes: its in-memory value (`wfile:<key>`, a
        // loop-back the writer publishes), never the client's file
        let w = v.field(&format!("wfile:{key}"));
        let f = v.field(&format!("file:{key}"));
        let k = v.field(&format!("wkeys:{key}"));
        let x = if self.cx.is_loopback(k) {
            // a file whose read-back only defines: it exists (empty) once
            // a line of it defines something ([`Run::read_back`])
            let keys = self.cx.use_field(k)?;
            if keys.is_some_and(|x| !list_of(v, x).is_empty()) {
                Some(v.list(vec![v.str(&format!("./{key}")), v.str("")]))
            } else {
                None
            }
        } else if self.cx.is_loopback(w) {
            self.cx.use_field(w)?
        } else if self.cx.is_loopback(f) {
            self.get_f(f)?
        } else {
            self.cx.use_env(f)?
        };
        let x = x.filter(|&x| !crate::val::list_of(v, x).is_empty());
        if let Some(t) = x.and_then(|x| crate::val::list_of(v, x).get(1).copied()) {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            std::hash::Hash::hash(&str_of(v, t), &mut h);
            self.fread
                .push((key.to_owned(), std::hash::Hasher::finish(&h)));
        }
        Ok(x)
    }

    /// A `streams` entry (`[name]` opens the file empty, `[name, text]`
    /// adds to it): from a page step, tagged with its page (`[j, entry]`,
    /// the writer places it with page `j`); from a document node, also a
    /// `T_WRITE_AT` marker on the main list, which the page builder's
    /// fold places among the pages (§1370, §1374).
    pub(crate) fn contribute_stream(&mut self, e: ValueId) {
        let v = self.v;
        let op = self.cx.op();
        if op == self.ops.pagestep || op == self.ops.pagestep_any {
            let j = i64::try_from(self.page_index()).unwrap_or(0);
            self.contribute("streams", v.seq_unit(v.list(vec![v.int(j), e])));
        } else if self.pg.is_some() {
            // a text paging inline (`pageinline.rs`): its entries are in
            // TeX's order as they are made
            self.contribute("streams", v.seq_unit(e));
        } else {
            self.contribute("streams", v.seq_unit(e));
            let at = v.list(vec![v.int(crate::nodes::T_WRITE_AT), e]);
            self.contribute("items", v.seq_unit(v.list(vec![v.int(0), at])));
        }
    }

    /// Whether file `name` is one the job writes and reads back, with its
    /// loop-backs declared.
    pub(crate) fn written_known(&self, name: &str) -> bool {
        let v = self.v;
        self.cx.is_loopback(v.field(&format!("wfile:{name}")))
            || self.cx.is_loopback(v.field(&format!("wkeys:{name}")))
    }

    /// `\input` of a file whose read-back only defines (`.aux`): the
    /// definitions its lines make, from the per-field loop-backs
    /// `wdef:<field>` the writer publishes for the fields listed in
    /// `wkeys:<key>`; nothing of the file's text is read. Whether `key`
    /// is such a file.
    pub(crate) fn read_back(&mut self, key: &str) -> R<bool> {
        let v = self.v;
        let wk = v.field(&format!("wkeys:{key}"));
        if !self.cx.is_loopback(wk) {
            return Ok(false);
        }
        let keys = self.cx.use_field(wk)?.map(|x| list_of(v, x)).unwrap_or_default();
        for k in keys {
            let name = str_of(v, k);
            let wd = v.field(&format!("wdef:{name}"));
            if !self.cx.is_loopback(wd) {
                self.cx.use_env(v.field(&format!("wdefdecl:{name}")))?;
            }
            if !self.cx.is_loopback(wd) {
                continue;
            }
            let Some(x) = self.cx.use_field(wd)? else {
                continue;
            };
            let xs = list_of(v, x);
            if let [val, g] = xs.as_slice() {
                let global = int_of(v, *g) != 0;
                self.set(&name, *val, global);
            }
        }
        Ok(true)
    }

    /// A line written to a file whose read-back only defines, read as
    /// its read-back reads it (under `\makeatletter`), on scratch: the
    /// definitions it makes, `[[field, value, global], ...]`; nothing of
    /// it stays in this run (its input, lists, contributions and
    /// definitions are restored).
    pub(crate) fn interpret_line(&mut self, line: &str) -> R<ValueId> {
        let v = self.v;
        let frames = std::mem::take(&mut self.frames);
        let own = self.own.clone();
        let defs = self.defs.clone();
        let (nc, nk) = (self.contribs.len(), self.kids.len());
        let nest_len = self.nest.len();
        let saved: Vec<_> = self.nest.iter().map(|l| l.saved.clone()).collect();
        let conds = self.conds.clone();
        let codes = self.if_codes.clone();
        let cur = self.cur.clone();
        let (eof, no_pull, claim) = (self.eof, self.no_pull, self.claim_rest);
        let effect = self.effect;
        self.eof = true;
        self.no_pull = true;
        self.set("cat:64", v.int(11), false);
        let base = self.defs.len();
        for l in line.split('\n') {
            let mut buf: Vec<u32> = l.chars().map(u32::from).collect();
            while buf.last() == Some(&32) {
                buf.pop();
            }
            self.frames.push_back(Frame::Src(Src {
                buf,
                loc: 0,
                state: NEW_LINE,
                endch: -2,
                starts: false,
                ends: true,
                origin: None,
                rescanned: true,
            }));
        }
        let mut res = Ok(());
        while self.has_input() {
            if let Err(h) = self.command() {
                res = Err(h);
                break;
            }
        }
        let made: Vec<ValueId> = self.defs[base.min(self.defs.len())..]
            .iter()
            .filter(|d| !str_of(v, d.0.0).starts_with("cat:"))
            .map(|&(f, x, l)| {
                v.list(vec![
                    f.0,
                    x,
                    v.int(i64::from(matches!(l, Locality::Global))),
                ])
            })
            .collect();
        self.frames = frames;
        self.own = own;
        self.defs = defs;
        self.contribs.truncate(nc);
        self.kids.truncate(nk);
        self.nest.truncate(nest_len);
        for (l, sv) in self.nest.iter_mut().zip(saved) {
            l.saved = sv;
        }
        self.conds = conds;
        self.if_codes = codes;
        self.cur = cur;
        self.eof = eof;
        self.no_pull = no_pull;
        self.claim_rest = claim;
        self.effect = effect;
        self.cats = [0; 256];
        self.kind_memo = None;
        res?;
        Ok(v.list(made))
    }

    /// File `name` is opened for writing: unless its `wfile:<name>`
    /// loop-back is declared, the run asks the client to declare it
    /// (`wfiledecl:<name>`, answered by `api::Graph::settle`).
    pub(crate) fn written_decl(&mut self, name: &str) -> R<()> {
        let v = self.v;
        if !self.written_known(name) {
            self.cx.use_env(v.field(&format!("wfiledecl:{name}")))?;
        }
        Ok(())
    }

    pub(crate) fn get(&mut self, name: &str) -> R<Option<ValueId>> {
        let f = self.ops.fields.get_at(self.v, self.cx.values_epoch(), name);
        self.get_f(f)
    }

    pub(crate) fn int(&mut self, name: &str) -> R<i64> {
        Ok(self.get(name)?.map_or(0, |x| int_of(self.v, x)))
    }

    pub(crate) fn int32(&mut self, name: &str) -> R<i32> {
        Ok(crate::val::i32_of(self.v, self.get(name)?))
    }

    pub(crate) fn param(&mut self, name: &str) -> R<i32> {
        let f = self.ops.fields.param(self.v, name);
        Ok(crate::val::i32_of(self.v, self.get_f(f)?))
    }

    pub(crate) fn glue_param(&mut self, name: &str) -> R<phitex_typeset::glue::Glue> {
        let f = self.ops.fields.param(self.v, name);
        let x = self.get_f(f)?;
        Ok(crate::val::glue_of(self.v, x))
    }

    pub(crate) fn cat(&mut self, c: u32) -> R<u8> {
        let slot = usize::try_from(c).ok().filter(|&i| i < 256);
        if let Some(i) = slot
            && self.cats[i] != 0
        {
            return Ok(self.cats[i] - 1);
        }
        let f = self.ops.fields.cat(self.v, c);
        let x = self.get_f(f)?;
        let k = u8::try_from(x.map_or(0, |x| int_of(self.v, x))).unwrap_or(12);
        if let Some(i) = slot
            && k < u8::MAX
        {
            self.cats[i] = k + 1;
        }
        Ok(k)
    }

    /// Defines `name` (§277–§279: `eq_define` / `geq_define`).
    pub(crate) fn set(&mut self, name: &str, val: ValueId, global: bool) {
        let f = self.ops.fields.get_at(self.v, self.cx.values_epoch(), name);
        self.set_f(f, val, global);
    }

    pub(crate) fn set_f(&mut self, f: Field, val: ValueId, global: bool) {
        // a local definition inside a group of this run is undone at the
        // group's close (`nest.rs`); a global one is retained through it
        if global {
            self.make_inert(f);
        } else {
            self.save_local(f);
        }
        self.own.insert(f, Some(val));
        self.cats = [0; 256];
        let loc = if global {
            Locality::Global
        } else {
            Locality::Local
        };
        // the definitions since the innermost group opened are the
        // group's (`nest.rs` pop_level truncates to its opening): one
        // per field there; outside every group, one per field at all. A
        // local after a global is kept apart, in a group (its close drops
        // the local, the global stays, §283) and at level 0 (the engine's
        // scope sees the global, then the local)
        let floor = self.nest.iter().rev().find(|l| l.group != 0).map_or(0, |l| l.defs_len);
        let ix = self.def_ix.entry(f).or_default();
        if let Some(&i) = ix.last()
            && i >= floor
            && !(self.defs[i].2 == Locality::Global && loc == Locality::Local)
        {
            self.defs[i] = (f, val, loc);
            return;
        }
        ix.push(self.defs.len());
        self.defs.push((f, val, loc));
    }

    pub(crate) fn contribute(&mut self, name: &str, val: ValueId) {
        let f = self.v.field(name);
        self.contribs.push((f, val));
    }

    pub(crate) fn kid(&mut self, op: OpId, args: Vec<ValueId>) {
        self.kids.push((op, args));
    }

    /// A log event (laid out by the client, [`crate::log`]).
    pub(crate) fn log(&mut self, kind: &str, text: &str) -> R<()> {
        if kind == "error" {
            self.diag(true, text);
        }
        let nl = self.int("param:newlinechar")?;
        let e = self
            .v
            .list(vec![self.v.str(kind), self.v.str(text), self.v.int(nl)]);
        let s = self.v.seq_unit(e);
        self.contribute("log", s);
        Ok(())
    }

    /// A diagnostic for [`crate::Build::diagnostics`]: where the input is
    /// (the current line of a file, or the node's own text), and the
    /// message. `error` false: a warning.
    pub(crate) fn diag(&mut self, error: bool, msg: &str) {
        let v = self.v;
        let src = self.frames.iter().find_map(|f| match f {
            Frame::Src(s) => Some(s.clone()),
            _ => None,
        });
        let (file, at, seg, loc) = match &src {
            Some(s) => {
                let mut buf = s.buf.clone();
                if s.endch >= 0 && buf.len() > s.loc && s.ends {
                    // the end of line character TeX appended
                    buf.pop();
                }
                let seg: String = buf
                    .iter()
                    .map(|&c| char::from_u32(c).unwrap_or('?'))
                    .collect();
                let loc = s.loc.min(buf.len());
                match s.origin {
                    Some((n, a)) => (n, i64::try_from(a).unwrap_or(0), seg, loc),
                    None => (ValueId::UNIT, -1, seg, loc),
                }
            }
            None => (ValueId::UNIT, -1, String::new(), 0),
        };
        let node = i64::from_ne_bytes(self.cx.node().raw().to_ne_bytes());
        let e = v.list(vec![
            v.int(i64::from(error)),
            v.str(msg),
            file,
            v.int(at),
            v.int(node),
            v.str(&seg),
            v.int(i64::try_from(loc).unwrap_or(0)),
        ]);
        let s = v.seq_unit(e);
        self.contribute("diags", s);
    }

    // ----------------------------------------------------------- meanings

    pub(crate) fn mean_of(&mut self, t: &Tok) -> R<Mean> {
        // a token's meaning field, interned once a run
        // (a control sequence's by its interned number: no name is
        // hashed on the token path)
        let hit = match t {
            Tok::S(c) => self.tfields.get(c.index()).copied().flatten(),
            Tok::C(x) => self.afields.get(x).copied(),
        };
        let f = if hit.is_some() {
            hit
        } else {
            let f = self
                .ops
                .fields
                .meaning_at(self.v, self.cx.values_epoch(), t);
            if let Some(f) = f {
                match t {
                    Tok::S(c) => {
                        let i = c.index();
                        if i >= self.tfields.len() {
                            self.tfields.resize(i + 1, None);
                        }
                        self.tfields[i] = Some(f);
                    }
                    Tok::C(x) => {
                        self.afields.insert(*x, f);
                    }
                }
            }
            f
        };
        match f {
            Some(f) => {
                let x = self.get_f(f)?;
                Ok(self.decode_mean(x))
            }
            None => Ok(Mean::Char(t.cat().unwrap_or(0), t.chr())),
        }
    }

    pub(crate) fn decode_mean(&mut self, x: Option<ValueId>) -> Mean {
        let Some(x) = x else { return Mean::Undefined };
        if let Some(m) = self.mcache.get(&x) {
            return m.clone();
        }
        let m = Mean::decode(self.v, Some(x));
        self.mcache.insert(x, m.clone());
        m
    }

    pub(crate) fn set_cur(&mut self, tok: Tok) -> R<()> {
        let mean = self.mean_of(&tok)?;
        let (c, h) = match (&tok, &mean) {
            (Tok::C(_), _) if !tok.is_cs_or_active() => (
                tok.cat().unwrap_or(0),
                i32::try_from(tok.chr()).unwrap_or(0),
            ),
            _ => mean.cmd_chr(),
        };
        self.cur = Cur {
            tok,
            cmd: c,
            chr: h,
            mean,
        };
        Ok(())
    }

    // -------------------------------------------------------------- input

    /// `back_input` (§325): `t` is read next.
    pub(crate) fn back_input(&mut self, t: Tok) {
        // §325: a brace put back is counted again when read
        if let Some(s) = self.astate
            && matches!(t, Tok::C(_))
            && !t.is_cs_or_active()
        {
            match t.cat() {
                Some(1) => self.set_astate(s - 1),
                Some(2) => self.set_astate(s + 1),
                _ => {}
            }
        }
        self.drop_spent_lists();
        self.frames.push_front(Frame::Toks(Rc::from([t]), 0));
    }

    /// `begin_token_list` (§323): `toks` are read next.
    pub(crate) fn push_toks(&mut self, toks: impl Into<Rc<[Tok]>>) {
        let toks: Rc<[Tok]> = toks.into();
        if !toks.is_empty() {
            self.drop_spent_lists();
            self.frames.push_front(Frame::Toks(toks, 0));
        }
    }

    /// Ends the token lists read to their end before another goes on
    /// top (§325 and §390, "conserve stack space"): without it a tail
    /// recursive macro leaves one spent list per round, and the input
    /// stack grows with the number of rounds.
    fn drop_spent_lists(&mut self) {
        while let Some(Frame::Toks(t, p)) = self.frames.front() {
            if *p < t.len() {
                break;
            }
            self.frames.pop_front();
        }
    }

    /// Takes the next sibling's text into the input (§362: the next line,
    /// or the rest of this one).
    pub(crate) fn pull(&mut self) -> R<bool> {
        if self.eof || self.no_pull {
            return Ok(false);
        }
        match self.cx.following(self.pulled) {
            Some((op, args)) if op == self.ops.exec => {
                self.pulled += 1;
                let frames = decode_frames(self.v, &args);
                self.push_back_frames(frames);
                Ok(true)
            }
            _ => {
                if self.top {
                    self.eof = true;
                    Ok(false)
                } else {
                    Err(Halt::Starve)
                }
            }
        }
    }

    /// Appends frames; a segment continuing the line of the last one joins
    /// it (a control sequence's name may run across nodes).
    pub(crate) fn push_back_frames(&mut self, frames: Vec<Frame>) {
        for f in frames {
            if let (Some(Frame::Src(last)), Frame::Src(next)) = (self.frames.back_mut(), &f)
                && !last.ends
                && !next.starts
            {
                last.buf.extend_from_slice(&next.buf);
                last.ends = next.ends;
                continue;
            }
            self.frames.push_back(f);
        }
    }

    /// Whether the first character of the next sibling's text, if it
    /// continues this line, is a letter (§356: a control word's name
    /// goes on).
    fn next_continues_name(&mut self) -> R<bool> {
        let Some(Frame::Src(last)) = self.frames.back() else {
            return Ok(false);
        };
        if last.ends {
            return Ok(false);
        }
        let Some((op, args)) = self.cx.following(self.pulled) else {
            return Ok(false);
        };
        if op != self.ops.exec {
            return Ok(false);
        }
        match first_frame(self.v, &args).as_ref() {
            Some(Frame::Src(s)) if !s.starts => match s.buf.first() {
                Some(&c) => Ok(matches!(self.cat(c)?, 7 | 11)),
                None => Ok(false),
            },
            _ => Ok(false),
        }
    }

    /// Whether the next sibling's text continues this line.
    fn next_is_continuation(&self) -> bool {
        let Some(Frame::Src(last)) = self.frames.back() else {
            return false;
        };
        if last.ends || self.frames.len() != 1 {
            return false;
        }
        match self.cx.following(self.pulled) {
            Some((op, args)) if op == self.ops.exec => {
                matches!(first_frame(self.v, &args), Some(Frame::Src(s)) if !s.starts)
            }
            _ => false,
        }
    }

    /// Whether there is input left without claiming more siblings.
    pub(crate) fn has_input(&self) -> bool {
        // (a document file's lines left for the region children stop
        // the run: what follows the file, its end marker included, is
        // read after them, [`Run::leftover`])
        for f in &self.frames {
            let more = match f {
                Frame::Toks(t, p) => *p < t.len(),
                Frame::Src(s) => s.loc < s.buf.len() || (s.ends && s.endch < -1),
                Frame::File(f) => {
                    if !f.has_lines() {
                        continue;
                    }
                    if self.doc && !self.open_span() && self.doc_file(f) {
                        return false;
                    }
                    true
                }
            };
            if more {
                return true;
            }
        }
        false
    }

    /// `get_next` (§341): the next token, its meaning not yet looked up.
    /// `None` at the end of all input.
    pub(crate) fn next_tok(&mut self) -> R<Option<Tok>> {
        loop {
            let Some(front) = self.frames.front_mut() else {
                if self.pull()? {
                    continue;
                }
                return Ok(None);
            };
            match front {
                Frame::Toks(t, p) => {
                    if *p < t.len() {
                        let tok = t[*p].clone();
                        *p += 1;
                        return Ok(Some(tok));
                    }
                    self.frames.pop_front();
                }
                Frame::Src(_) => {
                    if let Some(t) = self.src_next()? {
                        return Ok(Some(t));
                    }
                    self.frames.pop_front();
                }
                Frame::File(_) => {
                    // (a document file's lines are paragraph-region
                    // children, [`Run::leftover`], at a command's start:
                    // [`Run::has_input`] stops the run there; a command
                    // still scanning, `\count1=\input n`, reads on into
                    // the file, §362)
                    let Some(Frame::File(f)) = self.frames.front_mut() else {
                        unreachable!()
                    };
                    if f.has_lines() {
                        let at = f.byte_pos();
                        let name = f.name;
                        let line = f.take_line();
                        let args = line_args(self.v, &line);
                        for mut fr in decode_frames(self.v, &args).into_iter().rev() {
                            if let Frame::Src(s) = &mut fr {
                                s.origin = Some((name, at));
                            }
                            self.frames.push_front(fr);
                        }
                    } else {
                        self.frames.pop_front();
                    }
                }
            }
        }
    }

    /// Whether a file `\input` opened is a document's (its rest becomes
    /// paragraph regions), not a format's (run line by line).
    fn doc_file(&self, f: &FileRest) -> bool {
        crate::hints::file_kind_of(self.v, f.id, f.name) == phitex_source::pass::FileKind::Document
    }

    /// `get_next` on the current line (§343–§356); `None` when the line
    /// is finished.
    #[allow(clippy::too_many_lines)]
    fn src_next(&mut self) -> R<Option<Tok>> {
        // §362: a line is read when it is reached
        if let Some(Frame::Src(s)) = self.frames.front()
            && s.endch == -2
        {
            // §362: a line is counted as it begins
            let starts = s.starts;
            if starts {
                self.lines_entered += 1;
                let line = self.v.int(1);
                self.contribute("lines", line);
            }
            if let Some(Frame::Src(s)) = self.frames.front_mut() {
                s.endch = -3;
                if starts {
                    s.state = NEW_LINE;
                }
            }
        }
        self.end_line_char()?;
        loop {
            let (c, state) = {
                let Some(Frame::Src(s)) = self.frames.front_mut() else {
                    return Ok(None);
                };
                if s.loc >= s.buf.len() {
                    return Ok(None);
                }
                let c = s.buf[s.loc];
                s.loc += 1;
                (c, s.state)
            };
            // the line's end is reached after its last character (§360)
            let mut c = c;
            // reswitch
            loop {
                let cat = self.cat(c)?;
                match cat {
                    // §345
                    x if x == 9 || (x == 10 && state != MID_LINE) => break,
                    0 => return self.scan_cs().map(Some),
                    13 => {
                        self.set_state(MID_LINE);
                        return Ok(Some(Tok::ch(13, c)));
                    }
                    7 => {
                        // `^^` cut by a node boundary
                        let near_end = matches!(self.frames.front(), Some(Frame::Src(s)) if s.loc + 2 > s.buf.len());
                        if near_end && self.next_is_continuation() {
                            self.pull()?;
                        }
                        if let Some(x) = self.sup_mark(c) {
                            c = x;
                            continue;
                        }
                        self.set_state(MID_LINE);
                        return Ok(Some(Tok::ch(7, c)));
                    }
                    15 => break, // §346: an invalid character is skipped
                    10 => {
                        // §349
                        self.set_state(SKIP_BLANKS);
                        return Ok(Some(Tok::ch(10, 32)));
                    }
                    5 => {
                        self.finish_line();
                        return Ok(match state {
                            MID_LINE => Some(Tok::ch(10, 32)), // §348
                            SKIP_BLANKS => break,              // §350
                            _ => Some(Tok::cs("par")),         // §351
                        });
                    }
                    14 => {
                        self.finish_line();
                        break;
                    }
                    _ => {
                        self.set_state(MID_LINE);
                        return Ok(Some(Tok::ch(cat, c)));
                    }
                }
            }
            // goto switch
            self.end_line_char()?;
            let done = matches!(self.frames.front(), Some(Frame::Src(s)) if s.loc >= s.buf.len());
            if done {
                return Ok(None);
            }
        }
    }

    /// At the end of a segment that ends its line: `\endlinechar` goes
    /// after it (§360; read here, where the line's end is reached).
    fn end_line_char(&mut self) -> R<()> {
        let due = matches!(self.frames.front(), Some(Frame::Src(s)) if s.ends && s.endch == -3 && s.loc >= s.buf.len());
        if due {
            let e = self.int("param:endlinechar")?;
            if let Some(Frame::Src(s)) = self.frames.front_mut() {
                if (0..256).contains(&e) {
                    s.buf.push(u32::try_from(e).unwrap_or(13));
                }
                s.endch = e;
            }
        }
        Ok(())
    }

    fn set_state(&mut self, st: u8) {
        if let Some(Frame::Src(s)) = self.frames.front_mut() {
            s.state = st;
        }
    }

    /// The rest of the line goes, its end too (§347, §348).
    fn finish_line(&mut self) {
        if let Some(Frame::Src(s)) = self.frames.front_mut() {
            s.loc = s.buf.len();
            if s.endch < -1 {
                // the end has not been reached: nothing will follow
                s.endch = -1;
            }
        }
    }

    /// §352: an expanded character `^^A` or `^^df` after the `sup_mark`
    /// `c` just read.
    fn sup_mark(&mut self, c: u32) -> Option<u32> {
        let Some(Frame::Src(s)) = self.frames.front_mut() else {
            return None;
        };
        let limit = s.buf.len().checked_sub(1)?;
        let loc = s.loc;
        if loc < s.buf.len() && s.buf[loc] == c && loc < limit {
            let c1 = s.buf[loc + 1];
            if c1 < 0o200 {
                s.loc = loc + 2;
                if is_hex(c1) && s.loc <= limit {
                    let c2 = s.buf[s.loc];
                    if is_hex(c2) {
                        s.loc += 1;
                        return Some(hex(c1) * 16 + hex(c2));
                    }
                }
                return Some(if c1 < 0o100 { c1 + 0o100 } else { c1 - 0o100 });
            }
        }
        None
    }

    /// §354–§356: a control sequence after an escape character.
    fn scan_cs(&mut self) -> R<Tok> {
        loop {
            // a name cut by a node boundary goes on in the next node
            if self.frames.len() == 1
                && let Some(Frame::Src(s)) = self.frames.front()
                && !s.ends
            {
                let rest: Vec<u32> = s.buf[s.loc.min(s.buf.len())..].to_vec();
                let mut letters = true;
                for &c in &rest {
                    if !matches!(self.cat(c)?, 7 | 11) {
                        letters = false;
                        break;
                    }
                }
                let last_sup = match rest.last() {
                    Some(&c) => self.cat(c)? == 7,
                    None => false,
                };
                let more = if rest.is_empty() || (letters && last_sup) {
                    // `\` or `^^` at the cut: what follows is the name
                    self.next_is_continuation()
                } else {
                    letters && self.next_continues_name()?
                };
                if more && self.pull()? {
                    continue;
                }
            }
            let (loc, limit_plus) = match self.frames.front() {
                Some(Frame::Src(s)) => (s.loc, s.buf.len()),
                _ => return Ok(Tok::cs("")),
            };
            if loc >= limit_plus {
                return Ok(Tok::cs("")); // null_cs
            }
            // start_cs
            let mut k = loc;
            let c0 = self.buf(k);
            let mut cat = self.cat(c0)?;
            k += 1;
            self.set_state(if cat == 11 || cat == 10 {
                SKIP_BLANKS
            } else {
                MID_LINE
            });
            if cat == 11 && k < limit_plus {
                // §356
                loop {
                    let c = self.buf(k);
                    cat = self.cat(c)?;
                    k += 1;
                    if cat != 11 || k >= limit_plus {
                        break;
                    }
                }
                if self.reduce_expanded(k, cat) {
                    continue;
                }
                if cat != 11 {
                    k -= 1;
                }
                if k > loc + 1 {
                    // (the name is built in a buffer kept by the run:
                    // a known name allocates nothing, §259)
                    let mut name = std::mem::take(&mut self.name_buf);
                    name.clear();
                    name.extend((loc..k).map(|i| char::from_u32(self.buf(i)).unwrap_or('?')));
                    if let Some(Frame::Src(s)) = self.frames.front_mut() {
                        s.loc = k;
                    }
                    let t = Tok::cs(&name);
                    self.name_buf = name;
                    return Ok(t);
                }
            } else if self.reduce_expanded(k, cat) {
                continue;
            }
            let c = self.buf(loc);
            if let Some(Frame::Src(s)) = self.frames.front_mut() {
                s.loc = loc + 1;
            }
            let mut b = [0; 4];
            return Ok(Tok::cs(char::from_u32(c).unwrap_or('?').encode_utf8(&mut b)));
        }
    }

    fn buf(&self, k: usize) -> u32 {
        match self.frames.front() {
            Some(Frame::Src(s)) => s.buf.get(k).copied().unwrap_or(0),
            _ => 0,
        }
    }

    /// §355: with `buffer[k-1]` of category `cat`, reduces an expanded code
    /// there; true if it did (`goto start_cs`).
    fn reduce_expanded(&mut self, k: usize, cat: u8) -> bool {
        let Some(Frame::Src(s)) = self.frames.front_mut() else {
            return false;
        };
        let limit = s.buf.len() - 1;
        if k > limit || cat != 7 || k >= limit {
            return false;
        }
        let cur = s.buf[k - 1];
        if s.buf[k] != cur {
            return false;
        }
        let c = s.buf[k + 1];
        if c >= 0o200 {
            return false;
        }
        let mut d = 2;
        if is_hex(c) && k + 2 <= limit && is_hex(s.buf[k + 2]) {
            d = 3;
        }
        s.buf[k - 1] = if d > 2 {
            hex(c) * 16 + hex(s.buf[k + 2])
        } else if c < 0o100 {
            c + 0o100
        } else {
            c - 0o100
        };
        s.buf.drain(k..k + d);
        true
    }

    /// `get_next` with the meaning (§357–§358).
    pub(crate) fn get_next(&mut self) -> R<bool> {
        loop {
            let t = loop {
                let Some(t) = self.next_tok()? else {
                    return Ok(false);
                };
                if let Tok::S(n) = &t {
                    if n.starts_with("\u{10FFFE}eof") {
                        // §362: the end of an \input file (or a pseudo file)
                        crate::etex::end_of_file(self, n.name())?;
                        continue;
                    }
                    if &**n == "\u{10FFFE}uend" {
                        // §324: the end of a u template
                        self.align_u_end()?;
                        continue;
                    }
                }
                break t;
            };
            if let Tok::S(n) = &t
                && let Some(k) = n.strip_prefix("\u{10FFFE}endv:")
            {
                // §789: the end of a v template (`endv`, §380)
                let chr = k.parse().unwrap_or(0);
                self.cur = Cur {
                    tok: t.clone(),
                    cmd: cmd::IGNORE,
                    chr,
                    mean: Mean::Prim("\u{10FFFE}endv", cmd::IGNORE, chr),
                };
                return Ok(true);
            }
            if !self.get_next_rest(t)? {
                return Ok(false);
            }
            if self.align_token()? {
                continue;
            }
            return Ok(true);
        }
    }

    fn get_next_rest(&mut self, t: Tok) -> R<bool> {
        if let Tok::S(n) = &t
            && &**n == "\u{10FFFE}notexpanded"
        {
            // §358
            let Some(t2) = self.next_tok()? else {
                return Ok(false);
            };
            self.set_cur(t2)?;
            if self.cur.cmd > cmd::MAX_COMMAND {
                self.cur.cmd = cmd::RELAX;
                self.cur.chr = crate::val::NO_EXPAND_FLAG;
                self.cur.mean = Mean::NotExpanded;
            }
            return Ok(true);
        }
        self.set_cur(t)?;
        Ok(true)
    }

    /// `get_next`, at the end of all input a `\relax` (the end of a
    /// document without `\end` is the client's to report).
    pub(crate) fn get_next_or_end(&mut self) -> R<()> {
        if !self.get_next()? {
            if self.eof {
                return Err(Halt::Fatal);
            }
            self.cur = Cur {
                tok: Tok::cs("\u{10FFFE}relax"),
                cmd: cmd::RELAX,
                chr: 256,
                mean: Mean::Prim("relax", cmd::RELAX, 256),
            };
        }
        Ok(())
    }

    /// `get_token` (§365).
    pub(crate) fn get_token(&mut self) -> R<()> {
        self.get_next_or_end()
    }

    /// `get_x_token` (§380).
    pub(crate) fn get_x_token(&mut self) -> R<()> {
        loop {
            self.get_next_or_end()?;
            if self.cur.cmd <= cmd::MAX_COMMAND {
                return Ok(());
            }
            if self.cur.cmd >= cmd::END_TEMPLATE && self.cur.cmd != cmd::DONT_EXPAND {
                return Ok(());
            }
            self.expand()?;
        }
    }

    /// `x_token` (§381).
    pub(crate) fn x_token(&mut self) -> R<()> {
        while self.cur.cmd > cmd::MAX_COMMAND {
            self.expand()?;
            self.get_next_or_end()?;
        }
        Ok(())
    }

    // ------------------------------------------------------------ the end

    /// What is left of the node's own and claimed text, and the tokens a
    /// command put back or an expansion produced, as child nodes: one per
    /// token (a run of characters is one node), and the text not yet read
    /// as one node per segment.
    pub(crate) fn leftover(&self) -> Vec<Vec<ValueId>> {
        // a run reads everything of its own (expansion tokens run inline,
        // the slice's text is the node's command): what is left is the
        // rest of a sibling's text pulled for an operand (its own command,
        // §1030: a child), and the rest of a file `\input` opened (the
        // file-rest child)
        let v = self.v;
        let epoch = self.cx.values_epoch();
        let tag = |n: &str| self.ops.fields.get_at(v, epoch, n).0;
        let (tag_s, tag_f) = (tag("s"), tag("f"));
        let mut out: Vec<Vec<ValueId>> = Vec::new();
        let tag_t = tag("t");
        // (a document's leftovers stay a document's, [`Run::run_all`])
        let kind = if self.doc { 2 } else { 0 };
        let mut run: Vec<ValueId> = Vec::new();
        let node = |toks: Vec<ValueId>| -> Vec<ValueId> {
            let l = v.list(toks);
            vec![v.list(vec![v.list(vec![tag_t, l])]), v.int(kind)]
        };
        for f in &self.frames {
            match f {
                Frame::Toks(t, p) => {
                    for tok in &t[*p..] {
                        let ch = matches!(tok.cat(), Some(10..=12));
                        if ch {
                            run.push(crate::val::tok_id(v, epoch, tok));
                            continue;
                        }
                        if !run.is_empty() {
                            out.push(node(std::mem::take(&mut run)));
                        }
                        out.push(node(vec![crate::val::tok_id(v, epoch, tok)]));
                    }
                }
                Frame::Src(s) => {
                    if !run.is_empty() {
                        out.push(node(std::mem::take(&mut run)));
                    }
                    let end_due = s.ends && s.endch < -1;
                    if s.loc >= s.buf.len() && !end_due {
                        continue;
                    }
                    let text: String = s.buf[s.loc.min(s.buf.len())..]
                        .iter()
                        .map(|&c| char::from_u32(c).unwrap_or('?'))
                        .collect();
                    let starts = s.starts && s.endch == -2;
                    let mut xs = vec![
                        tag_s,
                        v.str(&text),
                        v.int(i64::from(s.state)),
                        v.int(-2),
                        v.int(
                            i64::from(starts)
                                | (i64::from(end_due) << 1)
                                | (i64::from(s.rescanned) << 2),
                        ),
                    ];
                    if let Some((name, at)) = s.origin {
                        let at = at + s.loc.min(s.buf.len());
                        xs.push(name);
                        xs.push(v.int(i64::try_from(at).unwrap_or(0)));
                    }
                    out.push(vec![v.list(vec![v.list(xs)]), v.int(kind)]);
                }
                Frame::File(f) => {
                    if !run.is_empty() {
                        out.push(node(std::mem::take(&mut run)));
                    }
                    if f.has_lines() && self.doc && self.doc_file(f) {
                        out.extend(self.doc_file_regions(f));
                    } else if f.has_lines() {
                        let fr = v.list(vec![
                            tag_f,
                            f.id,
                            v.int(i64::try_from(f.off).unwrap_or(0)),
                            f.name,
                        ]);
                        out.push(vec![v.list(vec![fr]), v.int(0)]);
                    }
                }
            }
        }
        if !run.is_empty() {
            out.push(node(run));
        }
        out
    }

    /// The region children of the rest of a document file `\input` by a
    /// document node: the file analysed as the document's own text (once,
    /// with the format's definitions, registered under the file's name
    /// for its fragments' hints), each slice with its hint.
    fn doc_file_regions(&self, f: &FileRest) -> Vec<Vec<ValueId>> {
        let v = self.v;
        let text = str_of(v, f.id);
        let defs = self
            .ops
            .defs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let t = crate::hints::text_hints(&text, phitex_source::pass::FileKind::Document, &defs);
        crate::hints::register_text(f.name, &t);
        let mut out = Vec::new();
        for &(s, e) in &t.slices {
            if e <= f.off {
                continue;
            }
            let a = s.max(f.off);
            let starts = a == 0 || text.as_bytes()[a - 1] == b'\n';
            let mut args = slice_args_in_file(v, &text[a..e], starts, f.name, a);
            args.push(crate::hints::hint_arg(v, &t, s));
            out.push(args);
        }
        out
    }

    /// The scope kind the node runs in: the engine's.
    pub(crate) fn scope_kind(&mut self) -> R<Option<ValueId>> {
        Ok(self.cx.scope_kind()?)
    }

    /// Runs the node's input to its end (§1030's loop): every command of
    /// its text, of the files it reads inline and of the tokens the
    /// expansions leave. A format's or a package's node is its whole
    /// text. A document region's paragraph or group the slice did not
    /// close (the pass predicted its end wrong) reads on into the
    /// following siblings, claimed, until it is closed or the text ends:
    /// correct, and serial for exactly that span.
    pub(crate) fn run_all(&mut self) -> R<()> {
        let args = self.cx.args();
        // the node's text kind (`args[1]`: 1 the job's text, 2 a document's)
        let k = args.get(1).map_or(0, |&t| int_of(self.v, t));
        self.top = k & 1 != 0;
        self.doc = k & 2 != 0;
        // a job's text run as one node (no format: INITEX's regime) pages
        // inline, in TeX's order (`pageinline.rs`)
        if self.top && !self.doc && self.pg.is_none() {
            self.pg = Some(Box::new(crate::pageinline::Inline::new()));
        }
        loop {
            while self.has_input() {
                self.command()?;
                if self.claim_rest {
                    return Ok(());
                }
            }
            // a paragraph, a group or a conditional still open reads on
            // into the next sibling, claimed
            if self.open_span() && !self.eof && self.pull()? {
                continue;
            }
            if !self.doc && !self.nest.is_empty() {
                // a format or package text ended inside a group: the
                // groups end with it (nothing after reads them), the
                // node reports the error
                self.diag(true, "format: group left open at end");
                for l in std::mem::take(&mut self.nest) {
                    if l.owns {
                        for x in l.list {
                            self.append(0, x);
                        }
                    }
                }
            }
            return Ok(());
        }
    }

    /// Whether the run has something open that must close in this run:
    /// a level of the nest or a conditional.
    fn open_span(&self) -> bool {
        !self.nest.is_empty() || !self.conds.is_empty()
    }

    /// Ends the run: definitions, contributions, children (what the
    /// command made, then the input left), the claim and the scope effect.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn finish(mut self) -> ValueId {
        crate::pageinline::finish(&mut self);
        // a page parameter this run set reaches the page fold as a
        // marker in the main list (page.rs `breaks`)
        if self.defs.iter().any(|&(f, _, _)| {
            matches!(
                crate::val::field_name(self.v, f).as_str(),
                "param:vsize" | "param:maxdepth" | "param:topskip" | "param:holdinginserts"
            )
        }) {
            let v = self.v;
            let vsize = self.param("vsize").unwrap_or(0);
            let maxdepth = self.param("maxdepth").unwrap_or(0);
            let topskip = self.glue_param("topskip").unwrap_or_default();
            let holding = self.param("holdinginserts").unwrap_or(0);
            let marker = v.list(vec![
                v.int(crate::nodes::T_PAGE_PARAMS),
                v.int(i64::from(vsize)),
                v.int(i64::from(maxdepth)),
                crate::val::glue_value(v, &topskip),
                v.int(i64::from(holding)),
            ]);
            let item = v.list(vec![v.int(0), marker]);
            let s = v.seq_unit(item);
            self.contribute("items", s);
        }
        for (f, x, l) in std::mem::take(&mut self.defs) {
            self.cx.define_with(f, x, l);
        }
        if !self.own.is_empty() {
            let names: Vec<ValueId> = self.own.keys().map(|f| f.0).collect();
            let l = self.v.list(names);
            let s = self.v.seq_unit(l);
            let defs = self
                .ops
                .fields
                .get_at(self.v, self.cx.values_epoch(), "defs");
            self.contribs.push((defs, s));
        }
        if crate::exec::env_flag!("PHITEX_TEX_TRACE") {
            let v = self.v;
            let items = v.field("items");
            for (f, x) in &self.contribs {
                if *f == items {
                    for e in v.seq_to_vec(*x) {
                        eprintln!(
                            "  item lid {} {}",
                            crate::val::int_at(v, e, 0),
                            v.render(
                                crate::val::list_of(v, e)
                                    .get(1)
                                    .copied()
                                    .unwrap_or(ValueId::UNIT)
                            )
                            .chars()
                            .take(60)
                            .collect::<String>()
                        );
                    }
                }
            }
            for (_, a) in &self.kids {
                eprintln!(
                    "  kid {}",
                    a.first()
                        .map(|&x| v.render(x).chars().take(80).collect::<String>())
                        .unwrap_or_default()
                );
            }
            eprintln!("  effect {:?}", self.effect);
        }
        if !self.fread.is_empty() {
            let v = self.v;
            let es: Vec<ValueId> = std::mem::take(&mut self.fread)
                .into_iter()
                .map(|(n, h)| v.list(vec![v.str(&n), crate::val::int_id(v, h.cast_signed())]))
                .collect();
            for e in es {
                self.contribute("fread", v.seq_unit(e));
            }
        }
        for (f, x) in std::mem::take(&mut self.contribs) {
            self.cx.contribute(f, x);
        }
        for (op, args) in std::mem::take(&mut self.kids) {
            self.cx.child(op, args);
        }
        for args in self.leftover() {
            if crate::exec::env_flag!("PHITEX_TEX_TRACE") {
                let v = self.v;
                eprintln!(
                    "  follow {}",
                    args.first()
                        .map(|&x| v.render(x).chars().take(90).collect::<String>())
                        .unwrap_or_default()
                );
            }
            self.cx.follow(self.ops.exec, args);
        }
        let mut n = self.pulled;
        if self.claim_rest {
            while matches!(self.cx.following(n), Some((op, _)) if op == self.ops.exec) {
                n += 1;
            }
        }
        self.cx.claim(n);
        self.cx.set_effect(self.effect);
        ValueId::UNIT
    }

    /// Ends a run that cannot finish: nothing defined, and the need, for
    /// the client (a file it has not supplied yet; or a child that ran out
    /// of text, which is a bug).
    pub(crate) fn stall(self, need: &str, what: ValueId) -> ValueId {
        let v = self.v;
        let id = v.int(i64::from_ne_bytes(self.cx.node().raw().to_ne_bytes()));
        let e = v.list(vec![v.str(need), id, what]);
        self.cx.contribute(v.field("needs"), v.seq_unit(e));
        self.cx.claim(0);
        self.cx.set_effect(Effect::None);
        ValueId::UNIT
    }
}

fn is_hex(c: u32) -> bool {
    (u32::from(b'0')..=u32::from(b'9')).contains(&c)
        || (u32::from(b'a')..=u32::from(b'f')).contains(&c)
}

fn hex(c: u32) -> u32 {
    if c <= u32::from(b'9') {
        c - u32::from(b'0')
    } else {
        c - u32::from(b'a') + 10
    }
}

/// The frames of an `Exec` node's arguments.
pub(crate) fn decode_frames(v: &Values, args: &[ValueId]) -> Vec<Frame> {
    decode_frames_upto(v, args, usize::MAX)
}

/// The first of a node's frames, the rest not decoded.
pub(crate) fn first_frame(v: &Values, args: &[ValueId]) -> Option<Frame> {
    decode_frames_upto(v, args, 1).into_iter().next()
}

fn decode_frames_upto(v: &Values, args: &[ValueId], limit: usize) -> Vec<Frame> {
    let mut out = Vec::new();
    let Some(&l) = args.first() else { return out };
    let Value::List(fs) = &*v.get(l) else {
        return out;
    };
    for &f in fs {
        if out.len() >= limit {
            break;
        }
        out.extend(crate::val::frame_of(v, f, || decode_frame(v, f)));
    }
    out
}

/// One frame of a node's arguments (a frame is an immutable value, so
/// its decoding is kept by id, [`crate::val::frame_of`]).
fn decode_frame(v: &Values, f: ValueId) -> Option<Frame> {
    let mut out = None;
    {
        let Value::List(xs) = &*v.get(f) else {
            return None;
        };
        match xs.first().map(|&t| str_of(v, t)).as_deref() {
            Some("t") => {
                let toks = crate::val::toks_of(v, xs.get(1).copied());
                out = Some(Frame::Toks(Rc::from(toks), 0));
            }
            Some("f") => {
                if let (Some(&id), Some(&o)) = (xs.get(1), xs.get(2)) {
                    let off = usize::try_from(int_of(v, o)).unwrap_or(0);
                    let name = xs.get(3).copied().unwrap_or(ValueId::UNIT);
                    out = Some(Frame::File(FileRest::new(v, id, off, name)));
                }
            }
            Some("s") => {
                let text = xs.get(1).map(|&t| str_of(v, t)).unwrap_or_default();
                out = Some(Frame::Src(Src {
                    buf: text.chars().map(u32::from).collect(),
                    loc: 0,
                    state: u8::try_from(xs.get(2).map_or(2, |&x| int_of(v, x))).unwrap_or(2),
                    endch: xs.get(3).map_or(-2, |&x| int_of(v, x)),
                    starts: xs.get(4).is_none_or(|&x| int_of(v, x) & 1 != 0),
                    ends: xs.get(4).is_none_or(|&x| int_of(v, x) & 2 != 0),
                    rescanned: xs.get(4).is_some_and(|&x| int_of(v, x) & 4 != 0),
                    origin: match (xs.get(5), xs.get(6)) {
                        (Some(&n), Some(&o)) => {
                            Some((n, usize::try_from(int_of(v, o)).unwrap_or(0)))
                        }
                        _ => None,
                    },
                }));
            }
            _ => {}
        }
    }
    out
}

/// A line already read (`\read`, §483): `\endlinechar` applied.
pub(crate) fn src_frame(line: &str, endlinechar: i64) -> Frame {
    let bytes = line.as_bytes();
    let mut end = bytes.len();
    while end > 0 && matches!(bytes[end - 1], b'\n' | b'\r') {
        end -= 1;
    }
    while end > 0 && matches!(bytes[end - 1], b' ' | b'\t') {
        end -= 1;
    }
    let mut buf: Vec<u32> = line[..end].chars().map(u32::from).collect();
    if (0..256).contains(&endlinechar) {
        buf.push(u32::try_from(endlinechar).unwrap_or(13));
    }
    Frame::Src(Src {
        buf,
        loc: 0,
        state: NEW_LINE,
        endch: endlinechar,
        starts: true,
        ends: true,
        origin: None,
        rescanned: true,
    })
}

/// A source line as a top-level node's arguments: its characters with
/// trailing spaces removed (web2c's `input_line`), not yet read.
pub(crate) fn line_args(v: &Values, line: &str) -> Vec<ValueId> {
    let bytes = line.as_bytes();
    let mut end = bytes.len();
    while end > 0 && matches!(bytes[end - 1], b'\n' | b'\r') {
        end -= 1;
    }
    while end > 0 && matches!(bytes[end - 1], b' ' | b'\t') {
        end -= 1;
    }
    let text: String = bytes[..end].iter().map(|&b| char::from(b)).collect();
    let frame = v.list(vec![
        v.str("s"),
        v.str(&text),
        v.int(i64::from(NEW_LINE)),
        v.int(-2),
        v.int(3),
    ]);
    vec![v.list(vec![frame]), v.int(1)]
}

/// [`slice_args`] for a piece of a file `\input` read, beginning at
/// byte `at` of the file named `name`: every segment carries its origin
/// `(name, offset)`; the kind is 2 (a document's piece, not the top).
pub(crate) fn slice_args_in_file(
    v: &Values,
    text: &str,
    starts_line: bool,
    name: ValueId,
    at: usize,
) -> Vec<ValueId> {
    let mut frames = Vec::new();
    let mut starts = starts_line;
    let mut rest = text;
    let mut pos = at;
    while !rest.is_empty() {
        let (seg, ends, next) = match rest.find('\n') {
            Some(k) => (&rest[..k], true, &rest[k + 1..]),
            None => (rest, false, ""),
        };
        let used = rest.len() - next.len();
        let seg: String = if ends {
            seg.trim_end_matches([' ', '\t', '\r']).to_owned()
        } else {
            seg.to_owned()
        };
        let state = if starts { NEW_LINE } else { MID_LINE };
        let flags = i64::from(starts) | (i64::from(ends) << 1);
        frames.push(v.list(vec![
            v.str("s"),
            v.str(&seg),
            v.int(i64::from(state)),
            v.int(-2),
            v.int(flags),
            name,
            v.int(i64::try_from(pos).unwrap_or(0)),
        ]));
        starts = ends;
        rest = next;
        pos += used;
    }
    vec![v.list(frames), v.int(2)]
}

/// The text of a node from the document: a slice of its lines, as
/// segments (each the part of one line the slice holds); the node's args
/// are `[frames, top, hint]`.
pub(crate) fn slice_args(v: &Values, text: &str, starts_line: bool) -> Vec<ValueId> {
    let mut frames = Vec::new();
    let mut starts = starts_line;
    let mut rest = text;
    while !rest.is_empty() {
        let (seg, ends, next) = match rest.find('\n') {
            Some(k) => (&rest[..k], true, &rest[k + 1..]),
            None => (rest, false, ""),
        };
        let seg: String = if ends {
            seg.trim_end_matches([' ', '\t', '\r']).to_owned()
        } else {
            seg.to_owned()
        };
        let state = if starts { NEW_LINE } else { MID_LINE };
        let flags = i64::from(starts) | (i64::from(ends) << 1);
        frames.push(v.list(vec![
            v.str("s"),
            v.str(&seg),
            v.int(i64::from(state)),
            v.int(-2),
            v.int(flags),
        ]));
        starts = ends;
        rest = next;
    }
    vec![v.list(frames), v.int(1)]
}
