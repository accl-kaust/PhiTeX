//! One run of one node (a paragraph region, a loading command, a whole
//! format): its input (the node's own text, the files it reads inline,
//! the siblings it claims while a span is open), `get_next` (§341–§365)
//! over it, and the definitions, contributions and children it makes,
//! buffered until the run is done.
//!
//! Everything here is local to one run: a small sequential scanner over
//! the node's own text. What TeX keeps between commands is read through
//! fields (`Run::get`), the scope kind and contributions.

use std::rc::Rc;

use crate::input::{Line, MID_LINE, NEW_LINE, Seg};

use phitex_defuse::{Ctx, Effect, Field, Locality, OpId, Stop, Value, ValueId, Values};

use crate::fields::Par;
use crate::runstate::{Loc, Native};
use crate::val::{Mean, Tok, cmd, int_at, int_of, list_of, str_of};

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
    /// A page step's fire position is gone (removed or re-made,
    /// `FieldAt::Gone`): its inputs are stale, so the run publishes
    /// nothing; the plan's re-run re-binds the step, which runs again.
    Gone,
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
    /// A page's finisher and the rest of a split piece (`crate::pages`).
    pub finish: OpId,
    pub resume: OpId,
    /// The build's field handles (`crate::fields`).
    pub fields: std::sync::Arc<crate::fields::Fields>,
    /// The format's definitions the build analyses document texts with
    /// (set by `api::Build::with`; none while a format is built): a
    /// document file `\input` is sliced with them ([`Run::leftover`]).
    pub defs: std::sync::Arc<std::sync::Mutex<crate::hints::Defs>>,
    /// Page-step runs that reached their finish, by page index (not
    /// the attempts that stopped or were cut off): [`crate::Stats`].
    pub steps_done: std::sync::Arc<std::sync::Mutex<Vec<u64>>>,
    /// The package loads' finished runs (`crate::pkgrec`): node, start,
    /// end, for [`crate::Stats`] (concurrency, preamble wall).
    pub pkg_log: std::sync::Arc<std::sync::Mutex<Vec<PkgRun>>>,
    /// The build's package records, for the engine's predicted
    /// definitions ([`crate::pkgrec::LoadHints::prediction`]).
    pub pkg_loads:
        std::sync::Arc<std::sync::Mutex<Option<std::sync::Arc<crate::pkgrec::LoadHints>>>>,
    /// The client's `\openout` name check ([`crate::Options::out_name_ok`]).
    pub out_ok: std::sync::Arc<std::sync::Mutex<Option<crate::api::OutNameOk>>>,
    /// The runs that read at a gone position ([`Halt::Gone`]).
    pub gone: std::sync::Arc<GoneLog>,
}

/// The runs that read at a position that was gone (`FieldAt::Gone`):
/// counted per round, and the nodes whose last finished run saw one
/// (none may be left when the build settles: such a run published
/// nothing).
#[derive(Default)]
pub(crate) struct GoneLog {
    runs: std::sync::atomic::AtomicU64,
    last: std::sync::Mutex<phitex_defuse::FxHashSet<phitex_defuse::NodeId>>,
}

impl GoneLog {
    /// A finished run of `node` (not one that stopped): whether it saw a
    /// gone position.
    pub(crate) fn note(&self, node: phitex_defuse::NodeId, gone: bool) {
        let mut l = self
            .last
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if gone {
            self.runs.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            l.insert(node);
        } else if !l.is_empty() {
            l.remove(&node);
        }
    }

    /// The runs that saw a gone position since the last call.
    pub(crate) fn take_runs(&self) -> u64 {
        self.runs.swap(0, std::sync::atomic::Ordering::Relaxed)
    }

    /// The live nodes whose last finished run saw a gone position (the
    /// others forgotten).
    pub(crate) fn stale(
        &self,
        live: impl Fn(phitex_defuse::NodeId) -> bool,
    ) -> Vec<phitex_defuse::NodeId> {
        let mut l = self
            .last
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        l.retain(|&n| live(n));
        l.iter().copied().collect()
    }
}

/// A package load's finished run: its node, when it began and ended,
/// and whether it ran speculatively.
pub(crate) type PkgRun = (u64, std::time::Instant, std::time::Instant, bool);

/// The lines of a file not reached yet: its text (a string value) and
/// the byte offset of the next line. A line becomes a [`Line`] level
/// when it is reached, so the rest of a file is one level, and one
/// child when a node leaves it (never one per line).
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
    /// The lines before `off` (`line`, §304: the last line taken's
    /// number).
    pub line: u32,
}

impl FileRest {
    pub(crate) fn new(v: &Values, id: ValueId, off: usize, name: ValueId) -> FileRest {
        let text = v.get(id);
        let (pos, line) = match &*text {
            phitex_defuse::Value::Str(s) => s.get(..off).map_or((0, 0), |p| {
                (
                    p.chars().count(),
                    u32::try_from(p.bytes().filter(|&b| b == b'\n').count()).unwrap_or(u32::MAX),
                )
            }),
            _ => (0, 0),
        };
        FileRest {
            id,
            text,
            off,
            name,
            pos,
            line,
        }
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

    pub(crate) fn pos_chars(&self) -> usize {
        self.pos
    }

    pub(crate) fn set_pos_chars(&mut self, p: usize) {
        self.pos = p;
    }

    /// The next line as a segment not yet reached (what a top-level
    /// node's arguments for it decode to, without a value or a string per
    /// line: its bytes are its characters, trailing blanks removed, as
    /// web2c's `input_line`); the offset moves past it.
    pub(crate) fn take_line(&mut self) -> Line {
        let at = self.pos;
        let s = self.str();
        let rest = &s[self.off..];
        let n = rest.find('\n').map_or(rest.len(), |k| k + 1);
        let line = &rest[..n];
        let bytes = line.as_bytes();
        let mut end = bytes.len();
        while end > 0 && matches!(bytes[end - 1], b'\n' | b'\r') {
            end -= 1;
        }
        while end > 0 && matches!(bytes[end - 1], b' ' | b'\t') {
            end -= 1;
        }
        // (the text holds one character per byte of the file, §31: a byte
        // past 127 is one character, never its UTF-8 encoding's bytes)
        let mut buf = Vec::with_capacity(end + 1);
        buf.extend(line[..end].chars().map(u32::from));
        let chars = line.chars().count();
        self.pos += chars;
        self.off += n;
        self.line = self.line.saturating_add(1);
        Line {
            buf,
            loc: 0,
            state: NEW_LINE,
            endch: -2,
            starts: true,
            ends: true,
            origin: Some((self.name, at)),
            lineno: self.line,
            rescanned: false,
        }
    }
}

/// Where `\inputlineno` reads (`line`, §304: the innermost file's).
pub(crate) enum FileLine {
    /// The line's number is known here.
    Known(i64),
    /// A line of a document's piece in the file of this name: the lines
    /// of the file begun before the node, and by it.
    Piece(ValueId),
}

/// Whether `PHITEX_HEARTBEAT=1` (read once a process).
fn heartbeat_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("PHITEX_HEARTBEAT").is_ok_and(|v| v == "1"))
}

/// The size of a run's control-sequence name cache ([`Run::cs_cache`]).
pub(crate) const CS_CACHE: usize = 1024;

/// The page builder's parameters (§980), which a `T_PAGE_PARAMS` marker
/// carries.
const PAGE_PARAMS: [Loc; 4] = [
    Par::vsize as Loc,
    Par::maxdepth as Loc,
    Par::topskip as Loc,
    Par::holdinginserts as Loc,
];

/// The current token (`cur_tok`, `cur_cmd`, `cur_chr`, `cur_cs`, §297).
/// Its meaning is read from its slot when a command needs it
/// ([`Run::cur_mean`]), as TeX reads `equiv(cur_cs)` (§389: a macro's
/// body at expansion time).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Cur {
    pub tok: Tok,
    pub cmd: u8,
    pub chr: i32,
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
    /// The input stack ([`crate::input`]).
    pub input: crate::input::Input,
    /// Control sequence names scanned, by a hash of their characters
    /// ([`Run::cs_of_buf`]).
    pub cs_cache: Box<[(u64, Tok)]>,
    /// Lines begun by this run.
    pub lines_read: u64,
    /// Lines begun by this run of document pieces (lines without a
    /// number of their own, `Line::lineno`), by file name: the `lines`
    /// accumulator, contributed once at the end.
    pub doc_lines: Vec<(ValueId, i64)>,
    /// `mode = 0` (§1370): a `\write` (or a literal at `\shipout`)
    /// being expanded, which disables `\prevdepth`, `\spacefactor`,
    /// `\lastskip`, `\prevgraf`.
    pub mode_zero: bool,
    /// `macro_call`'s buffers, kept between calls.
    pub mbufs: crate::expand::MacroBufs,
    /// `PHITEX_HEARTBEAT=1` (read once a process).
    pub heartbeat_on: bool,
    pub pulled: usize,
    /// Where each pulled sibling's text begins in the input's text (its
    /// first segment's index), or `None` when it joined the line before
    /// it: [`Run::give_back_peeked`].
    pub pull_at: Vec<Option<usize>>,
    /// The writer's state this run's last PDF step made, not yet where
    /// other nodes find it ([`crate::pdfout::publish`], at the run's end).
    pub pdf_fold: Option<(u64, std::sync::Arc<crate::pdfout::Fold>)>,
    /// The input level that holds the last pulled sibling's first line,
    /// once reached: where [`Run::park`] finds it.
    pub pull_lev: Option<usize>,
    /// For a resumed run, per pulled sibling: its first line as it was
    /// before its first token was peeked, when the park left that token
    /// and the rest of the line as two text segments
    /// ([`Run::give_back_peeked`]).
    pub pull_orig: Vec<Option<crate::input::Line>>,
    /// Whether the node is the document's text (no parent): the end of
    /// its following siblings is the end of the file.
    pub top: bool,
    /// Whether `\end` takes every following sibling.
    pub claim_rest: bool,
    /// Whether the node's hint was found violated when it started.
    pub hint_void: bool,
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
    /// The files this run looked for and did not find (a package load's
    /// record keys on them, `crate::pkgrec`).
    pub fmiss: Vec<String>,
    /// Inside `pass_text` (§494): a skipped text's tokens are read only
    /// to classify them (conditional, `\fi`/`\else`, or neither).
    pub in_skip: bool,
    /// The locations a load's run first read from the engine while
    /// skipping (`PHITEX_PKG_TRACE`: which chaining reads are skips).
    pub skip_first: Vec<Loc>,
    /// Whether the node is a package load at the unit's top level, whose
    /// run is recorded (`crate::pkgrec`) and publishes its net effect.
    pub rec_load: bool,
    /// While looking for the command's first token: a node's command
    /// begins in its own text (the next sibling's begins in its own).
    pub no_pull: bool,
    /// The run's state table ([`crate::runstate`]): every field it read
    /// or defined, native.
    pub st: crate::runstate::State,
    /// The category codes of characters 0–255 as last read (`u8::MAX`:
    /// not read), valid while the table's catcode generation is
    /// `cat_gen`.
    cat_cache: [u8; 256],
    cat_gen: u64,
    pub contribs: Vec<(Field, ValueId)>,
    /// Set while `\advance` defines its register ([`State::note_kind`]).
    pub in_advance: bool,
    /// A page step that shipped a page (`pdf:shipped:<j>`).
    pub shipped: bool,
    pub kids: Vec<(OpId, Vec<ValueId>)>,
    pub effect: Effect,
    pub cur: Cur,
    /// Conditionals begun in this run: their `if_limit` (§489).
    pub conds: Vec<u8>,
    /// The codes (`cur_if`) of `conds`, parallel to it.
    pub if_codes: Vec<i32>,
    /// The buffer a control sequence's name is scanned into (§356).
    pub(crate) name_buf: String,
    /// The scope kind last decoded (`Run::mode`, once a command): its
    /// value and its fields `(group, sid, lid, mode, depth, extra)`.
    pub kind_memo: Option<(ValueId, KindFields)>,
    /// `align_state` as this run has it (§309), once read.
    pub astate: Option<i64>,
    /// The alignments open in this run (`align_state`'s companion,
    /// §324): run-local, an alignment never crosses a node.
    pub aopen: i64,
    /// Fetching a name to define (`get_r_token`, §1215): its meaning is
    /// not looked up (§357 needs it only for an alignment's watch).
    pub bare: bool,
    /// The incompleat noads of this run's math lists (§1181:
    /// `incompleat_noad`, a field of the list's nest entry; run-local,
    /// as a math list never crosses a node), by list id.
    pub incs: Vec<(i64, ValueId)>,
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
    /// Whether this run continues a parked attempt ([`Run::from_parked`]).
    pub resumed: bool,
    /// Changes to the run's state a stopped command cannot undo (own
    /// definitions, a buffer reduced in place): a stop after one does
    /// not park ([`Run::rewind`]).
    pub irreversible: u64,
    /// While a job-written line is interpreted ([`Run::interpret_line`]):
    /// the read-back fields reverted so far in that line, to the value
    /// they had before this round's read-back (`wpre:<field>`), and
    /// what each location's pre-value is (memoized for the run).
    pub interp: Option<phitex_defuse::FxHashSet<Loc>>,
    /// The interpreted text ran out inside a command: it needs the next
    /// line written to its stream ([`Run::interpret_line`]).
    pub interp_short: bool,
    /// The fields a read-back nested in a line's interpretation set (a
    /// written `\@input{part.aux}`): the part file's definitions, which
    /// that file's own lines publish; the line lists them as keys only.
    pub interp_nested: phitex_defuse::FxHashMap<String, String>,
    /// The interpreted text is a whole file whose end has come (its
    /// stream closed): running out of it is §362's end of file, which
    /// §336 checks ([`Run::file_ended`]), not "needs more".
    pub interp_eof: bool,
    /// The errors §338 gave as the interpreted file ended.
    pub interp_errs: Vec<String>,
    /// `scanner_status` and `warning_index` (§305): what a token list
    /// being scanned is (`SCAN_DEFINING`, `SCAN_MATCHING`,
    /// `SCAN_ABSORBING`) and whose; `None` is `normal`. Kept only for
    /// §338 at the end of an interpreted file.
    pub scanner: Option<(u8, Tok)>,
    /// §339's `long_state:=outer_call`: the `\par` §338 inserted ends
    /// the argument being matched without an error (§392).
    pub outer_call: bool,
    /// The state's stamp as the line's interpretation began: a field the
    /// line defined since is not reverted (`\@writefile` sets
    /// `\@temptokena` and writes `\the\@temptokena`).
    pub interp_base: u64,
    /// The streams a document node closed with `\immediate\closeout`:
    /// closed for this run's later writes, and for the writer from the
    /// close's position (a stream entry), never for the page steps: the
    /// steps run on the state at the end of the text, and TeX's deferred
    /// writes fire before a close that follows their page
    /// (`\enddocument` closes the `.aux` after `\clearpage`).
    pub closed: Vec<i32>,
    /// The run read a job-written file back ([`Run::read_back`]): what
    /// it defines from then on keeps its value there ([`Run::note_pre`]).
    pub read_back_done: bool,
    /// The outermost macro being expanded at the read-back (its input
    /// level): what the run defines while it is still on the input
    /// stack keeps its value at the read-back ([`Run::note_pre`]).
    pub read_back_at: Option<crate::input::Level>,
    pub pre_memo: phitex_defuse::FxHashMap<Loc, Option<ValueId>>,
    /// Errors since the last paragraph ended (§76 `error_count`, reset by
    /// `end_graf`, §1096): at 100 the job stops (§82).
    pub error_count: u32,
    /// Whether the job stopped at 100 errors.
    pub fatal_logged: bool,
    /// The input as the command in progress began ([`Run::rewind`]).
    pub(crate) mark: crate::getnext::Mark,
    /// A halt raised on the token path ([`Run::raise`]), returned by the
    /// next `get_next` of the command in progress.
    pub(crate) halt: Option<Halt>,
    /// A read at the fire position found it gone ([`Halt::Gone`]): the
    /// run publishes nothing, even if a caller swallowed the halt.
    pub(crate) saw_gone: bool,
}

impl<'c, 'a> Run<'c, 'a> {
    pub(crate) fn new(cx: &'c mut Ctx<'a>, ops: Ops) -> Self {
        let v = cx.values();
        let op = cx.op();
        if crate::exec::env_flag!("PHITEX_LOOP_TRACE") {
            eprintln!("lb-run node {:x} op {op:?}", cx.node().raw());
        }
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
            input: crate::input::Input::new(),
            cs_cache: vec![(0, Tok::c(0)); CS_CACHE].into_boxed_slice(),
            lines_read: 0,
            doc_lines: Vec::new(),
            mode_zero: false,
            mbufs: crate::expand::MacroBufs::default(),
            heartbeat_on: heartbeat_enabled(),
            pulled: 0,
            pull_at: Vec::new(),
            pdf_fold: None,
            pull_lev: None,
            pull_orig: Vec::new(),
            top: false,
            claim_rest: false,
            hint_void: false,
            nest: Vec::new(),
            next_lid: 0,
            last_par: (0, 0),
            doc: false,
            fread: Vec::new(),
            fmiss: Vec::new(),
            in_skip: false,
            skip_first: Vec::new(),
            rec_load: false,
            no_pull: false,
            st: crate::runstate::State::new(),
            cat_cache: [u8::MAX; 256],
            cat_gen: u64::MAX,
            contribs: Vec::new(),
            in_advance: false,
            shipped: false,
            kids: Vec::new(),
            effect: Effect::None,
            cur: Cur {
                tok: Tok::c(0),
                cmd: 0,
                chr: 0,
            },
            conds: Vec::new(),
            if_codes: Vec::new(),
            name_buf: String::new(),
            kind_memo: None,
            astate: None,
            aopen: 0,
            bare: false,
            incs: Vec::new(),
            lines_entered: 0,
            eof: false,
            radix: 0,
            cur_order: phitex_typeset::glue::Order::Normal,
            pdf_occ: 0,
            resumed: false,
            irreversible: 0,
            interp: None,
            interp_short: false,
            interp_nested: phitex_defuse::FxHashMap::default(),
            interp_eof: false,
            interp_errs: Vec::new(),
            scanner: None,
            outer_call: false,
            interp_base: 0,
            closed: Vec::new(),
            read_back_done: false,
            read_back_at: None,
            pre_memo: phitex_defuse::FxHashMap::default(),
            error_count: 0,
            fatal_logged: false,
            mark: crate::getnext::Mark::default(),
            halt: None,
            saw_gone: false,
        }
    }

    /// The field of location `loc`.
    pub(crate) fn loc_field(&mut self, loc: Loc) -> Field {
        if let Some(f) = self.st.field(loc) {
            return f;
        }
        let f = self
            .ops
            .fields
            .field_of(self.v, self.cx.values_epoch(), loc as usize);
        self.st.set_field(loc, f);
        f
    }

    /// The location of field `f` (a fixed one, a meaning's, or the
    /// spill's).
    pub(crate) fn loc_of(&mut self, f: Field) -> Loc {
        match self.ops.fields.loc_of(self.v, self.cx.values_epoch(), f) {
            Some(l) => {
                let l = Loc::try_from(l).unwrap_or(0);
                if self.st.field(l).is_none() {
                    self.st.set_field(l, f);
                }
                l
            }
            None => self.st.spill_loc(f),
        }
    }

    /// Location `loc` read: from the engine on its first read in the run.
    #[inline]
    pub(crate) fn load(&mut self, loc: Loc) -> R<()> {
        if self.interp.is_some() {
            self.revert_read_back(loc)?;
        }
        if self.st.is_loaded(loc) {
            return Ok(());
        }
        self.load_slow(loc)
    }

    /// A job-written line is read as the next round's read-back reads
    /// it: in the state before this round's read-back took effect. The
    /// first read of a read-back field in the line gets the value the
    /// read-back replaced (`wpre:<field>`, left by [`Run::read_back`]),
    /// and of a field the reading run defined after it, its value there
    /// ([`Run::note_pre`]),
    /// so a line whose effect depends on the field (biblatex's
    /// `\xifinlist`-guarded `\listxadd`, `\ifcsundef`-guarded
    /// `\csgdef`) defines it as the reading run will, not as this run,
    /// which already holds the previous round's value, would.
    #[inline(never)]
    fn revert_read_back(&mut self, loc: Loc) -> R<()> {
        let Some(done) = &mut self.interp else {
            return Ok(());
        };
        if !done.insert(loc) {
            return Ok(());
        }
        if self.st.defined_after(loc, self.interp_base) {
            return Ok(());
        }
        let pre = if let Some(&p) = self.pre_memo.get(&loc) {
            p
        } else {
            let f = self.loc_field(loc);
            let name = crate::val::field_name(self.v, f);
            // (a read-back field, or one the reading run defined after
            // the read-back: [`Run::note_pre`])
            let p = if name.starts_with("wpre:") {
                None
            } else {
                self.get(&format!("wpre:{name}"))?
            };
            self.pre_memo.insert(loc, p);
            p
        };
        if let Some(p) = pre {
            if crate::exec::env_flag!("PHITEX_AUX_TRACE") {
                let name = crate::val::field_name(self.v, self.loc_field(loc));
                eprintln!("aux-trace: revert {name} for the line's read");
            }
            // (read in place of the value, not a definition of the line:
            // only what the line defines on it is published)
            self.st
                .preload(loc, if p == ValueId::UNIT { None } else { Some(p) });
        }
        Ok(())
    }

    #[inline(never)]
    fn load_slow(&mut self, loc: Loc) -> R<()> {
        let f = self.loc_field(loc);
        if self.in_skip && self.rec_load {
            self.skip_first.push(loc);
        }
        let r = self.cx.use_field(f);
        crate::exec::widths::read(f);
        if crate::exec::env_flag!("PHITEX_READS") {
            self.trace_read(f, r.is_err());
        }
        self.st.fill(loc, r?);
        Ok(())
    }

    /// The engine value of location `loc` in the run's view.
    pub(crate) fn get_loc(&mut self, loc: Loc) -> R<Option<ValueId>> {
        self.load(loc)?;
        Ok(self.st.id_of(loc, self.v))
    }

    /// Location `loc` as an integer.
    #[inline]
    pub(crate) fn int_loc(&mut self, loc: Loc) -> R<i32> {
        self.load(loc)?;
        Ok(self.st.int_of(loc, self.v))
    }

    /// Location `loc` as an integer, widened.
    pub(crate) fn cint(&mut self, loc: Loc) -> R<i64> {
        Ok(self.get_loc(loc)?.map_or(0, |x| int_of(self.v, x)))
    }

    /// Defines the state cell at `loc` ([`crate::fields::cell`]).
    pub(crate) fn set_cell(&mut self, loc: Loc, val: ValueId, global: bool) {
        self.set_loc(loc, Native::Raw(val), global);
    }

    /// Location `loc` as glue.
    pub(crate) fn glue_loc(&mut self, loc: Loc) -> R<phitex_typeset::glue::Glue> {
        self.load(loc)?;
        Ok(self.st.glue_of(loc, self.v))
    }

    /// Location `loc` as a token list.
    #[allow(dead_code)]
    pub(crate) fn toks_loc(&mut self, loc: Loc) -> R<Rc<[Tok]>> {
        self.load(loc)?;
        Ok(self.st.toks_of(loc, self.v))
    }

    /// Defines location `loc` (§277–§279: `eq_define` / `geq_define`).
    pub(crate) fn set_loc(&mut self, loc: Loc, val: Native, global: bool) {
        if crate::exec::env_flag!("PHITEX_AUX_TRACE")
            && self.interp.is_none()
            && loc == self.reg_loc(0, 0)
        {
            let x = match &val {
                Native::Raw(x) => self.v.render(*x),
                _ => "?".to_owned(),
            };
            let cmd = self
                .cur
                .tok
                .as_cs()
                .map_or_else(|| format!("{:?}", self.cur.tok), |n| n.name().to_owned());
            let int = match &val {
                Native::Int(i) => i.to_string(),
                _ => String::new(),
            };
            eprintln!(
                "aux-trace: count0 := {x}{int} global={global} by node {} op {:?} at \\{cmd}",
                self.cx.node().raw(),
                self.cx.op()
            );
        }
        if self.read_back_done && self.interp.is_none() {
            if crate::exec::env_flag!("PHITEX_AUX_TRACE") {
                let f = self.loc_field(loc);
                eprintln!(
                    "aux-trace: after read-back node {} defines {}",
                    self.cx.node().raw(),
                    crate::val::field_name(self.v, f)
                );
            }
            self.note_pre(loc);
        }
        let grp = self.innermost_group();
        self.st.note_kind(loc, self.in_advance);
        self.st.set(loc, val, global, grp);
        self.irreversible += 1;
    }


    /// A field this run defines after reading a job-written file back:
    /// its value at the read-back point (`wpre:<field>`, once), which a
    /// line written later reads as the read-back does
    /// ([`Run::revert_read_back`]). LaTeX's `\document` disables the
    /// preamble commands (`\@newl@bel` among them) right after it reads
    /// the `.aux`; a `\newlabel` line is read before that.
    fn note_pre(&mut self, loc: Loc) {
        let v = self.v;
        // (only while the macro that read the file back runs:
        // `\document`'s rest, not the text after it)
        // (a level's position moves as it is read; its start and end
        // stay)
        let still = self.read_back_at.is_some_and(|m| {
            self.input
                .levels
                .iter()
                .any(|l| l.kind == m.kind && l.ty == m.ty && l.c == m.c && l.b == m.b)
        });
        if !still {
            self.read_back_done = false;
            return;
        }
        let f = self.loc_field(loc);
        let name = crate::val::field_name(v, f);
        if name.starts_with("wpre:") || name.starts_with("cat:") {
            return;
        }
        let pre = v.field(&format!("wpre:{name}"));
        let pl = self.loc_of(pre);
        if self.st.defined_after(pl, 0) {
            return;
        }
        // (a stop here stops the run: it runs again from its start)
        if self.load(loc).is_err() {
            return;
        }
        let old = self.st.id_of(loc, v).unwrap_or(ValueId::UNIT);
        if crate::exec::env_flag!("PHITEX_AUX_TRACE") {
            eprintln!(
                "aux-trace: wpre {name} = {} (node {})",
                crate::val::show_def(v, old),
                self.cx.node().raw()
            );
        }
        let grp = self.innermost_group();
        self.st.set(pl, Native::Raw(old), true, grp);
    }

    /// The parameter `p`.
    pub(crate) fn par(&mut self, p: Par) -> R<Option<ValueId>> {
        self.get_loc(p as Loc)
    }

    /// The integer or dimension parameter `p`.
    #[inline]
    pub(crate) fn ipar(&mut self, p: Par) -> R<i32> {
        self.int_loc(p as Loc)
    }

    /// The glue parameter `p`.
    pub(crate) fn gpar(&mut self, p: Par) -> R<phitex_typeset::glue::Glue> {
        self.glue_loc(p as Loc)
    }

    /// Defines the parameter `p`.
    pub(crate) fn set_par(&mut self, p: Par, val: ValueId, global: bool) {
        self.set_loc(p as Loc, Native::Raw(val), global);
    }

    /// The field of the primitive quantity at `(c, chr)` (§1224, §1228).
    pub(crate) fn quantity(&self, c: u8, chr: i32) -> Field {
        match self.ops.fields.quantity_slot(c, chr) {
            Some(s) => self.ops.fields.slot_field(s),
            None => self.v.field(&crate::val::quantity_field(c, chr)),
        }
    }

    /// The location of the primitive quantity at `(c, chr)`.
    pub(crate) fn quantity_loc(&mut self, c: u8, chr: i32) -> Loc {
        if let Some(s) = self.ops.fields.quantity_slot(c, chr) {
            Loc::try_from(s).unwrap_or(0)
        } else {
            let f = self.v.field(&crate::val::quantity_field(c, chr));
            self.loc_of(f)
        }
    }

    /// The value of the field of the primitive quantity at `(c, chr)`.
    pub(crate) fn qget(&mut self, c: u8, chr: i32) -> R<Option<ValueId>> {
        let l = self.quantity_loc(c, chr);
        self.get_loc(l)
    }

    /// The field `<fam>:<key>` ([`crate::fields::Fam`]).
    pub(crate) fn keyed(&self, fam: crate::fields::Fam, key: &str) -> Field {
        self.ops
            .fields
            .keyed(self.v, self.cx.values_epoch(), fam, key)
    }

    /// The field of register `n` of kind `r` (as [`Self::reg`]).
    pub(crate) fn reg_field(&self, r: usize, n: i32) -> Field {
        match crate::fields::reg_slot(r, n) {
            Some(s) => self.ops.fields.slot_field(s),
            None => self.v.field(&format!(
                "{}:{n}",
                ["count", "dimen", "skip", "muskip", "toks"][r]
            )),
        }
    }

    /// The location of register `n` of kind `r`.
    pub(crate) fn reg_loc(&mut self, r: usize, n: i32) -> Loc {
        if let Some(s) = crate::fields::reg_slot(r, n) {
            Loc::try_from(s).unwrap_or(0)
        } else {
            let f = self.reg_field(r, n);
            self.loc_of(f)
        }
    }

    /// Character `c`'s entry in code table `table` (0 `lc`, 1 `uc`, 2
    /// `sf`, 3 `mc`, 4 `dc`).
    pub(crate) fn code(&mut self, table: usize, c: impl TryInto<u32>) -> R<i32> {
        let c = c.try_into().unwrap_or(u32::MAX);
        let l = if let Some(s) = crate::fields::code_slot(table, c) {
            Loc::try_from(s).unwrap_or(0)
        } else {
            let name = ["lc", "uc", "sf", "mc", "dc"][table];
            let f = self.v.field(&format!("{name}:{c}"));
            self.loc_of(f)
        };
        self.int_loc(l)
    }

    /// Integer register `n` of kind `r` (0 count, 1 dimen).
    pub(crate) fn reg_int(&mut self, r: usize, n: impl TryInto<i32>) -> R<i32> {
        let l = self.reg_loc(r, n.try_into().unwrap_or(-1));
        self.int_loc(l)
    }

    /// Field `f` in the run's view.
    pub(crate) fn read_f(&mut self, f: Field) -> R<Option<ValueId>> {
        self.get_f(f)
    }

    /// Register `n` of kind `r` (0 count, 1 dimen, 2 skip, 3 muskip, 4 toks).
    pub(crate) fn reg(&mut self, r: usize, n: i32) -> R<Option<ValueId>> {
        let l = self.reg_loc(r, n);
        self.get_loc(l)
    }

    // ------------------------------------------------------------ fields

    pub(crate) fn get_f(&mut self, f: Field) -> R<Option<ValueId>> {
        let l = self.loc_of(f);
        self.get_loc(l)
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
        // (`PHITEX_READS=<field>`: that field's reads, with the current
        // token and the token lists being read)
        if let Ok(want) = std::env::var("PHITEX_READS")
            && want == name
        {
            let v = self.v;
            let mut ctx = Vec::new();
            for l in self.input.levels.iter().rev().take(6) {
                if l.kind == crate::input::kind::TOKS {
                    let toks: Vec<String> = self
                        .input
                        .arena
                        .get(l.rest())
                        .iter()
                        .take(8)
                        .map(|t| v.render(t.encode(v)))
                        .collect();
                    ctx.push(format!("t{}:{}", l.ty, toks.join(" ")));
                } else {
                    ctx.push(format!("k{}", l.kind));
                }
            }
            let bt = std::backtrace::Backtrace::force_capture().to_string();
            let frames: Vec<&str> = bt
                .lines()
                .map(str::trim)
                .filter(|l| {
                    l.starts_with("at ")
                        || l.split(':')
                            .next()
                            .is_some_and(|n| n.parse::<u32>().is_ok())
                })
                .map(|l| if l.len() > 90 { &l[..90] } else { l })
                .skip(2)
                .take(30)
                .collect();
            eprintln!(
                "reads: node {} {name} at {} (pulled {}) | {} | bt {}",
                self.cx.node().raw(),
                v.render(self.cur.tok.encode(v)),
                self.pulled,
                ctx.join(" | "),
                frames.join(" < ")
            );
        }
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
        let r = self.file_inner(key);
        if r.is_err() {
            self.trace_stop(&format!("file {key}"));
        }
        r
    }

    /// (measure) `PHITEX_READS`: a stop that is not a field read's.
    pub(crate) fn trace_stop(&self, what: &str) {
        if crate::exec::env_flag!("PHITEX_READS") {
            eprintln!(
                "reads: node {} op {:?} {what} PARKED",
                self.cx.node().raw(),
                self.cx.op()
            );
        }
    }

    fn file_inner(&mut self, key: &str) -> R<Option<ValueId>> {
        let v = self.v;
        // a file the job writes: its in-memory value (`wfile:<key>`, a
        // loop-back the writer publishes), never the client's file
        let w = self.keyed(crate::fields::Fam::Wfile, key);
        let f = self.keyed(crate::fields::Fam::File, key);
        let k = self.keyed(crate::fields::Fam::Wkeys, key);
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
            let x = self.cx.use_field(w)?;
            if crate::exec::env_flag!("PHITEX_AUX_TRACE") {
                let t = x.and_then(|x| list_of(v, x).get(1).map(|&t| str_of(v, t)));
                eprintln!(
                    "aux-trace: read wfile {key}: {:?} bytes",
                    t.map(|t| t.len())
                );
            }
            x
        } else if self.cx.is_loopback(f) {
            self.get_f(f)?
        } else {
            self.cx.use_env(f)?
        };
        let x = x.filter(|&x| !crate::val::list_of(v, x).is_empty());
        // (the text's hash: its value's id, a content hash, so a font's
        // or map's bytes are not copied and hashed again at every read)
        if let Some(t) = x.and_then(|x| crate::val::list_of(v, x).get(1).copied()) {
            self.fread.push((key.to_owned(), t.raw()));
        } else if self.rec_load && !self.fmiss.iter().any(|n| n == key) {
            self.fmiss.push(key.to_owned());
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
        self.contribute("streams", v.seq_unit(e));
        let at = v.list(vec![v.int(crate::nodes::T_WRITE_AT), e]);
        self.contribute("items", v.seq_unit(v.list(vec![v.int(0), at])));
    }

    /// The page parameters in force, as a `T_PAGE_PARAMS` marker on the
    /// main list (page.rs `breaks`, §980).
    pub(crate) fn put_page_params(&mut self) {
        let v = self.v;
        let vsize = self.ipar(Par::vsize).unwrap_or(0);
        let maxdepth = self.ipar(Par::maxdepth).unwrap_or(0);
        let topskip = self.gpar(Par::topskip).unwrap_or_default();
        let holding = self.ipar(Par::holdinginserts).unwrap_or(0);
        let marker = v.list(vec![
            v.int(crate::nodes::T_PAGE_PARAMS),
            v.int(i64::from(vsize)),
            v.int(i64::from(maxdepth)),
            crate::val::glue_value(v, &topskip),
            v.int(i64::from(holding)),
        ]);
        self.contribute("items", v.seq_unit(v.list(vec![v.int(0), marker])));
    }

    /// Whether file `name` is one the job writes and reads back, with its
    /// loop-backs declared.
    pub(crate) fn written_known(&self, name: &str) -> bool {
        self.cx
            .is_loopback(self.keyed(crate::fields::Fam::Wfile, name))
            || self
                .cx
                .is_loopback(self.keyed(crate::fields::Fam::Wkeys, name))
    }

    /// `\input` of a file whose read-back only defines (`.aux`): the
    /// definitions its lines make, from the per-field loop-backs
    /// `wdef:<field>` the writer publishes for the fields listed in
    /// `wkeys:<key>`; nothing of the file's text is read. Whether `key`
    /// is such a file.
    pub(crate) fn read_back(&mut self, key: &str) -> R<bool> {
        let v = self.v;
        let wk = self.keyed(crate::fields::Fam::Wkeys, key);
        if !self.cx.is_loopback(wk) {
            return Ok(false);
        }
        let keys = self
            .cx
            .use_field(wk)?
            .map(|x| list_of(v, x))
            .unwrap_or_default();
        for k in keys {
            let id = crate::api::wdef_id(key, &str_of(v, k));
            let name = crate::api::wdef_field(&id).to_owned();
            let wd = self.keyed(crate::fields::Fam::Wdef, &id);
            if !self.cx.is_loopback(wd) {
                self.cx
                    .use_env(self.keyed(crate::fields::Fam::Wdefdecl, &id))?;
            }
            if !self.cx.is_loopback(wd) {
                continue;
            }
            let Some(x) = self.cx.use_field(wd)? else {
                continue;
            };
            let xs = list_of(v, x);
            // (the errors the file's text gives as it ends inside a
            // command, §338: given again at every read-back, as TeX
            // gives them at every `\input` of the file)
            if name.starts_with("rberr:") {
                // (a file read as its text here, one this run writes,
                // gives them as its text ends, §362)
                if self.interp.is_some() {
                    self.interp_nested.insert(name, id);
                } else if self.get(&format!("wopen:{key}"))?.is_none()
                    && let Some(&ms) = xs.first()
                {
                    if crate::exec::env_flag!("PHITEX_AUX_TRACE") {
                        eprintln!(
                            "aux-trace: read-back {key} gives {name} in node {}",
                            self.cx.node().raw()
                        );
                    }
                    for m in list_of(v, ms) {
                        self.log("error", &str_of(v, m))?;
                    }
                }
                continue;
            }
            if let [val, g] = xs.as_slice() {
                let global = int_of(v, *g) != 0;
                // the value the read-back replaces, for the lines
                // written this round ([`Run::revert_read_back`]): once
                // per run (LaTeX reads the `.aux` again at `\enddocument`)
                let pre = format!("wpre:{name}");
                if self.get(&pre)?.is_none() {
                    let was = self.get(&name)?;
                    if crate::exec::env_flag!("PHITEX_AUX_TRACE") {
                        eprintln!(
                            "aux-trace: read-back {key} {name} = {} (was {:?})",
                            crate::val::show_def(v, *val),
                            was.map(|x| crate::val::show_def(v, x))
                        );
                    }
                    self.set(&pre, was.unwrap_or(ValueId::UNIT), true);
                }
                self.set(&name, *val, global);
                if self.interp.is_some() {
                    self.interp_nested.insert(name, id);
                }
            }
        }
        self.read_back_at = self
            .input
            .levels
            .iter()
            .find(|l| l.kind == crate::input::kind::TOKS && l.ty == crate::input::ty::MACRO)
            .copied();
        self.read_back_done = self.read_back_at.is_some();
        if crate::exec::env_flag!("PHITEX_AUX_TRACE") {
            eprintln!(
                "aux-trace: read back {key} in node {} ({:?}): levels {:?}",
                self.cx.node().raw(),
                self.input.text.first().map(|sg| match sg {
                    Seg::Line(l) => l
                        .buf
                        .iter()
                        .take(40)
                        .filter_map(|&c| char::from_u32(c))
                        .collect::<String>(),
                    _ => String::new(),
                }),
                self.input
                    .levels
                    .iter()
                    .map(|l| (l.kind, l.ty))
                    .collect::<Vec<_>>()
            );
        }
        Ok(true)
    }

    /// The definitions an interpreted text made since stamp `base`,
    /// `[field, value, global]` each; a read-back nested in it (`nested`)
    /// gives its fields as keys only, `[field]`.
    fn made_since(
        &mut self,
        base: u64,
        nested: &phitex_defuse::FxHashMap<String, String>,
        gids: u32,
    ) -> Vec<ValueId> {
        let v = self.v;
        let mut made: Vec<ValueId> = self
            .st
            .defined_since(v, base, gids)
            .into_iter()
            .filter_map(|(loc, x, l)| Some((self.loc_field(loc), x?, l)))
            .filter(|d| is_read_back_definition(&str_of(v, d.0.0)))
            .map(|(f, x, l)| {
                // (a nested read-back's definition: its key only, naming
                // the file whose loop-back holds the value)
                if let Some(id) = nested.get(&str_of(v, f.0)) {
                    return v.list(vec![v.str(id)]);
                }
                v.list(vec![
                    f.0,
                    x,
                    v.int(i64::from(matches!(l, Locality::Global))),
                ])
            })
            .collect();
        // (a nested read-back's errors: its key only, as its fields)
        made.extend(
            nested
                .iter()
                .filter(|(n, _)| n.starts_with("rberr:"))
                .map(|(_, id)| v.list(vec![v.str(id)])),
        );
        made
    }

    /// A line written to a file whose read-back only defines, read as
    /// its read-back reads it (under `\makeatletter`), on scratch: the
    /// definitions it makes, `[[field, value, global], ...]`. Nothing
    /// else of it stays in this run (its input, lists, contributions and
    /// definitions are restored), except what it writes to other files
    /// (LaTeX's `\@writefile{toc}{...}`, which writes the `.toc` line as
    /// `\enddocument` reads the `.aux` back): those `streams` entries
    /// (and a document node's `T_WRITE_AT` markers for them) are kept.
    pub(crate) fn interpret_line(&mut self, line: &str) -> R<Option<ValueId>> {
        let v = self.v;
        let input = std::mem::replace(&mut self.input, crate::input::Input::new());
        let st = self.st.clone();
        let (nc, nk) = (self.contribs.len(), self.kids.len());
        let nest_len = self.nest.len();
        let conds = self.conds.clone();
        let codes = self.if_codes.clone();
        let cur = self.cur;
        let (eof, no_pull, claim) = (self.eof, self.no_pull, self.claim_rest);
        let effect = self.effect;
        self.eof = true;
        self.no_pull = true;
        // (interpreting from here: the run's own bookkeeping, as
        // [`Run::note_pre`], is not the line's)
        self.interp = Some(phitex_defuse::FxHashSet::default());
        self.set("cat:64", v.int(11), false);
        // (on a list of its own, dropped after: what a line typesets,
        // an error's text, goes nowhere)
        self.push_list(-1, crate::val::group::VBOX, Vec::new());
        let base = self.st.stamp();
        // (the groups the line opens are numbered after this one, the
        // list's: what it defines at its own level is the line's)
        let gids = self.st.gid_mark();
        self.interp_base = base;
        // (the text's lines: a final newline ends the last line, it does
        // not begin an empty one, which would be a `\par`)
        for l in line.strip_suffix('\n').unwrap_or(line).split('\n') {
            let mut buf: Vec<u32> = l.chars().map(u32::from).collect();
            while buf.last() == Some(&32) {
                buf.pop();
            }
            self.input.text.push(Seg::Line(Line {
                buf,
                loc: 0,
                state: NEW_LINE,
                endch: -2,
                starts: false,
                ends: true,
                origin: None,
                lineno: 0,
                rescanned: true,
            }));
        }
        let mut res = Ok(());
        self.interp_short = false;
        self.interp_nested.clear();
        let scanner = self.scanner.take();
        while self.has_input() {
            if let Err(h) = self.command() {
                res = Err(h);
                break;
            }
        }
        // (the text ended inside a command: as `\input` of the file
        // would, the next line written goes on with it)
        let short = std::mem::take(&mut self.interp_short) && matches!(res, Err(Halt::Fatal));
        if short {
            res = Ok(());
        }
        self.scanner = scanner;
        self.outer_call = false;
        let nested = std::mem::take(&mut self.interp_nested);
        let made = self.made_since(base, &nested, gids);
        self.interp = None;
        self.input = input;
        self.st = st;
        let streams = v.field("streams");
        let items = v.field("items");
        let written: Vec<(Field, ValueId)> = self
            .contribs
            .drain(nc..)
            .filter(|&(f, x)| {
                f == streams
                    || f == items
                        && v.seq_to_vec(x).iter().all(|&e| {
                            list_of(v, e)
                                .get(1)
                                .is_some_and(|&n| int_at(v, n, 0) == crate::nodes::T_WRITE_AT)
                        })
            })
            .collect();
        self.contribs.extend(written);
        self.kids.truncate(nk);
        self.nest.truncate(nest_len);
        self.conds = conds;
        self.if_codes = codes;
        self.cur = cur;
        self.eof = eof;
        self.no_pull = no_pull;
        self.claim_rest = claim;
        self.effect = effect;
        self.kind_memo = None;
        res?;
        Ok((!short).then(|| v.list(made)))
    }

    /// File `name` is opened for writing: unless its `wfile:<name>`
    /// loop-back is declared, the run asks the client to declare it
    /// (`wfiledecl:<name>`, answered by `api::Graph::settle`).
    pub(crate) fn written_decl(&mut self, name: &str) -> R<()> {
        if !self.written_known(name) {
            self.cx
                .use_env(self.keyed(crate::fields::Fam::Wfiledecl, name))?;
        }
        Ok(())
    }

    pub(crate) fn get(&mut self, name: &str) -> R<Option<ValueId>> {
        let f = self.ops.fields.get_at(self.v, self.cx.values_epoch(), name);
        self.read_f(f)
    }

    pub(crate) fn int(&mut self, name: &str) -> R<i64> {
        Ok(self.get(name)?.map_or(0, |x| int_of(self.v, x)))
    }

    pub(crate) fn int32(&mut self, name: &str) -> R<i32> {
        Ok(crate::val::i32_of(self.v, self.get(name)?))
    }

    /// The parameter `name` (a name known only at run time; the fixed
    /// ones read [`Self::ipar`]).
    pub(crate) fn param(&mut self, name: &str) -> R<i32> {
        let l = if let Some(s) = self.ops.fields.par_slot(name) {
            Loc::try_from(s).unwrap_or(0)
        } else {
            let f = self.ops.fields.param(self.v, name);
            self.loc_of(f)
        };
        self.int_loc(l)
    }

    pub(crate) fn glue_param(&mut self, name: &str) -> R<phitex_typeset::glue::Glue> {
        let l = if let Some(s) = self.ops.fields.par_slot(name) {
            Loc::try_from(s).unwrap_or(0)
        } else {
            let f = self.ops.fields.param(self.v, name);
            self.loc_of(f)
        };
        self.glue_loc(l)
    }

    /// The category code of character `c` (§232), through the run's
    /// copy of the catcode table (refreshed when any category code
    /// changes, [`crate::runstate::State::cat_gen`]).
    #[inline]
    pub(crate) fn cat(&mut self, c: u32) -> R<u8> {
        if let Ok(i) = usize::try_from(c)
            && i < 256
        {
            let g = self.st.cat_gen();
            if self.cat_gen != g {
                self.cat_cache = [u8::MAX; 256];
                self.cat_gen = g;
            }
            let x = self.cat_cache[i];
            if x != u8::MAX {
                return Ok(x);
            }
            let x = self.cat_slow(c)?;
            self.cat_cache[i] = x;
            return Ok(x);
        }
        self.cat_slow(c)
    }

    #[inline(never)]
    fn cat_slow(&mut self, c: u32) -> R<u8> {
        let x = if c < 256 {
            self.int_loc(Loc::try_from(crate::fields::CAT0).unwrap_or(0) + c)?
        } else {
            let f = self.ops.fields.cat(self.v, c);
            let l = self.loc_of(f);
            self.int_loc(l)?
        };
        Ok(u8::try_from(x).unwrap_or(12))
    }

    /// Defines `name` (§277–§279: `eq_define` / `geq_define`).
    pub(crate) fn set(&mut self, name: &str, val: ValueId, global: bool) {
        let f = self.ops.fields.get_at(self.v, self.cx.values_epoch(), name);
        self.set_f(f, val, global);
    }

    pub(crate) fn set_f(&mut self, f: Field, val: ValueId, global: bool) {
        let l = self.loc_of(f);
        self.set_loc(l, Native::Raw(val), global);
    }

    /// The fields the run publishes: its definitions at the base level
    /// but a local one equal to the value it read (the same binding).
    pub(crate) fn published_fields(&mut self) -> Vec<Field> {
        let locs: Vec<Loc> = self
            .st
            .published(self.v, self.rec_load)
            .into_iter()
            .map(|x| x.0)
            .collect();
        locs.into_iter().map(|l| self.loc_field(l)).collect()
    }

    /// The fields the run defines: those whose value at its end differs
    /// from the one it loaded, or that it never loaded (a local
    /// assignment that ends where it began defines nothing, as LaTeX's
    /// paragraph hooks setting `\\everypar` and back).
    pub(crate) fn def_fields(&mut self) -> Vec<Field> {
        let locs = self.defined_now();
        locs.into_iter().map(|l| self.loc_field(l)).collect()
    }

    /// [`Run::def_fields`]' locations, as the state table publishes them.
    fn defined_now(&self) -> Vec<Loc> {
        self.st
            .published(self.v, self.rec_load)
            .into_iter()
            .map(|(l, _, _)| l)
            .collect()
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
        let stop = kind == "error" && {
            self.error_count += 1;
            self.error_count >= 100
        };
        let nl = i64::from(self.ipar(Par::newlinechar)?);
        let e = self
            .v
            .list(vec![self.v.str(kind), self.v.str(text), self.v.int(nl)]);
        let s = self.v.seq_unit(e);
        self.contribute("log", s);
        if stop {
            // §82: history is fatal_error_stop and the job ends
            let e = self.v.list(vec![
                self.v.str("wlog"),
                self.v.str("(That makes 100 errors; please try again.)"),
                self.v.int(nl),
            ]);
            let s = self.v.seq_unit(e);
            self.contribute("log", s);
            self.fatal_logged = true;
            return Err(Halt::Fatal);
        }
        Ok(())
    }

    /// `fatal_error` (§93): `s` logged, and the job ends there (§1335).
    /// A build has no terminal: it runs as `nonstop_mode` does, so every
    /// read from the terminal and every file error ends the job (§71,
    /// §484, §530).
    pub(crate) fn fatal<T>(&mut self, s: &str) -> R<T> {
        self.log("error", s)?;
        self.fatal_logged = true;
        Err(Halt::Fatal)
    }

    /// The engine's own diagnostic (a hint's regime violated, an
    /// undeclared definition): for [`crate::Build::diagnostics`] only.
    /// It is not TeX's error: not in the log, not in the error count
    /// (§76's hundred), and it decides neither the output nor the exit.
    pub(crate) fn engine_diag(&mut self, msg: &str) {
        self.diag_kind(2, msg);
    }

    /// A diagnostic for [`crate::Build::diagnostics`]: where the input is
    /// (the current line of a file, or the node's own text), and the
    /// message. `error` false: a warning.
    pub(crate) fn diag(&mut self, error: bool, msg: &str) {
        self.diag_kind(i64::from(error), msg);
    }

    /// [`Self::diag`] of kind `kind`: 0 a warning, 1 TeX's error, 2 the
    /// engine's diagnostic.
    fn diag_kind(&mut self, kind: i64, msg: &str) {
        let v = self.v;
        let src = self
            .input
            .levels
            .iter()
            .rev()
            .find_map(|l| (l.kind == crate::input::kind::LINE).then(|| self.input.line(*l).clone()))
            .or_else(|| {
                self.input.text_rest().iter().find_map(|s| match s {
                    Seg::Line(l) => Some(l.clone()),
                    _ => None,
                })
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
            v.int(kind),
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

    /// The location of token `t`'s meaning, if it has one.
    #[inline]
    pub(crate) fn mean_loc(&mut self, t: Tok) -> Option<Loc> {
        let l = if let Some(l) = crate::fields::mean_loc(t) {
            Loc::try_from(l).ok()?
        } else {
            let f = self
                .ops
                .fields
                .meaning_at(self.v, self.cx.values_epoch(), t)?;
            self.loc_of(f)
        };
        if self.interp.is_some() {
            // (a meaning read bypasses `load` once the slot is loaded)
            let _ = self.revert_read_back(l);
        }
        Some(l)
    }

    #[inline]
    pub(crate) fn mean_of(&mut self, t: Tok) -> R<Mean> {
        match self.mean_loc(t) {
            Some(l) => {
                if !self.st.is_loaded(l) {
                    self.load_slow(l)?;
                }
                Ok(self.st.mean_of(l, self.v))
            }
            None => Ok(Mean::Char(t.cat().unwrap_or(0), t.chr())),
        }
    }

    /// The current token's macro when its slot is loaded with one (the
    /// common case of `macro_call`: no decoding, no `Mean` built); `None`
    /// sends the caller through [`Self::cur_mean`].
    #[inline]
    pub(crate) fn cur_macro(&mut self) -> Option<Rc<crate::val::Macro>> {
        let t = self.cur.tok;
        if !t.is_cs_or_active() || self.cur.cmd == cmd::RELAX {
            return None;
        }
        let l = Loc::try_from(crate::fields::mean_loc(t)?).ok()?;
        self.st.macro_at(l)
    }

    /// The current token's meaning, read from its slot now (§389, §508):
    /// a character's own, `\noexpand`'s marker, a `v` template's end.
    pub(crate) fn cur_mean(&mut self) -> R<Mean> {
        let t = self.cur.tok;
        if !t.is_cs_or_active() {
            return Ok(Mean::Char(t.cat().unwrap_or(0), t.chr()));
        }
        if self.cur.cmd == cmd::RELAX && self.cur.chr == crate::val::NO_EXPAND_FLAG {
            return Ok(Mean::NotExpanded);
        }
        if self.cur.cmd == cmd::IGNORE
            && t.as_cs()
                .is_some_and(|n| n.frozen() && n.starts_with("\u{10FFFE}endv"))
        {
            return Ok(Mean::Prim("\u{10FFFE}endv", cmd::IGNORE, self.cur.chr));
        }
        self.mean_of(t)
    }

    /// Defines token `t`'s meaning (`\def`, `\let`, …): kept native,
    /// encoded only if the run publishes it.
    pub(crate) fn set_mean(&mut self, t: Tok, m: Mean, global: bool) {
        if let Some(l) = self.mean_loc(t) {
            self.set_loc(l, Native::Mean(m), global);
        }
    }

    pub(crate) fn decode_mean(&mut self, x: Option<ValueId>) -> Mean {
        Mean::decode(self.v, x)
    }

    pub(crate) fn set_cur(&mut self, tok: Tok) -> R<()> {
        self.set_cur_flag(tok);
        match self.halt.take() {
            Some(h) => Err(h),
            None => Ok(()),
        }
    }

    /// The token path's halt (an engine read that must wait, the text
    /// ended): kept until the command's next `get_next` returns it; the
    /// current token is the sentinel meanwhile (`cmd::HALT`).
    #[cold]
    pub(crate) fn raise(&mut self, h: Halt) {
        if self.halt.is_none() {
            self.halt = Some(h);
        }
        self.cur = Cur {
            tok: crate::expand::halt_tok(),
            cmd: cmd::HALT,
            chr: 0,
        };
    }

    /// [`Self::set_cur`] without a `Result`: a halt is raised.
    #[allow(clippy::inline_always)]
    #[inline(always)]
    pub(crate) fn set_cur_flag(&mut self, tok: Tok) {
        if !tok.is_cs_or_active() {
            // (a character's meaning is its own: no meaning stored)
            self.cur.tok = tok;
            self.cur.cmd = tok.cat().unwrap_or(0);
            self.cur.chr = i32::try_from(tok.chr()).unwrap_or(0);
            return;
        }
        if let Err(h) = self.set_cur_cs(tok) {
            self.raise(h);
        }
    }

    /// [`Self::set_cur`] for a control sequence or active character.
    #[inline]
    fn set_cur_cs(&mut self, tok: Tok) -> R<()> {
        if self.bare {
            // (a name about to be defined: its present meaning is never
            // used, so it is not read)
            self.cur = Cur {
                tok,
                cmd: cmd::RELAX,
                chr: 256,
            };
            return Ok(());
        }
        // (the common case: a control sequence whose meaning this run
        // has read and decoded: one slot access)
        if let Some(i) = tok.cs_index()
            && let Ok(l) = Loc::try_from(crate::fields::MEAN_BASE + i)
            && let Some((c, h)) = self.st.cmd_chr_fast(l)
        {
            self.cur = Cur {
                tok,
                cmd: c,
                chr: h,
            };
            return Ok(());
        }
        self.set_cur_cs_slow(tok)
    }

    #[inline(never)]
    fn set_cur_cs_slow(&mut self, tok: Tok) -> R<()> {
        let (c, h) = match self.mean_loc(tok) {
            Some(l) => {
                if !self.st.is_loaded(l) {
                    self.load_slow(l)?;
                }
                self.st.cmd_chr_of(l, self.v)
            }
            None => (
                tok.cat().unwrap_or(0),
                i32::try_from(tok.chr()).unwrap_or(0),
            ),
        };
        self.cur = Cur {
            tok,
            cmd: c,
            chr: h,
        };
        Ok(())
    }

    // -------------------------------------------------------------- input

    // ------------------------------------------------------------ the end

    /// What is left of the node's own and claimed text, and the tokens a
    /// command put back or an expansion produced, as child nodes: the
    /// token levels between two texts as one node, and the text not yet
    /// read as one node per segment.
    #[allow(clippy::too_many_lines)]
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
        // (the token levels' node reads `\inputlineno` of the file they
        // are read in: its line, or its name for a document's piece)
        let here = match self.file_line_here() {
            Some(FileLine::Known(n)) => vec![ValueId::UNIT, v.int(n)],
            Some(FileLine::Piece(name)) => vec![name, v.int(-1)],
            None => Vec::new(),
        };
        let node = |toks: Vec<ValueId>| -> Vec<ValueId> {
            let l = v.list(toks);
            let mut t = vec![tag_t, l];
            t.extend(here.iter().copied());
            vec![v.list(vec![v.list(t)]), v.int(kind)]
        };
        // (the token levels left, in reading order, are one node: TeX
        // reads them one after the other, and a brace or a group they
        // open and close stays inside one run, waiting on no scope)
        let toks_left = |toks: &[Tok], _: &mut Vec<Vec<ValueId>>, run: &mut Vec<ValueId>| {
            run.extend(toks.iter().map(|&tok| crate::val::tok_id(v, epoch, tok)));
        };
        let line_left = |s: &Line, out: &mut Vec<Vec<ValueId>>| {
            let end_due = s.ends && s.endch < -1;
            if s.loc >= s.buf.len() && !end_due {
                return;
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
                    i64::from(starts) | (i64::from(end_due) << 1) | (i64::from(s.rescanned) << 2),
                ),
            ];
            if let Some((name, at)) = s.origin {
                let at = at + s.loc.min(s.buf.len());
                xs.push(name);
                xs.push(v.int(i64::try_from(at).unwrap_or(0)));
                // (a line of a file read here: its number; a document
                // piece's is counted, never in the arguments)
                if s.lineno > 0 {
                    xs.push(v.int(i64::from(s.lineno)));
                }
            }
            out.push(vec![v.list(vec![v.list(xs)]), v.int(kind)]);
        };
        let file_left = |f: &FileRest, out: &mut Vec<Vec<ValueId>>| {
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
        };
        for l in self.input.levels.iter().rev() {
            match l.kind {
                crate::input::kind::TOKS => {
                    toks_left(self.input.arena.get(l.rest()), &mut out, &mut run);
                }
                crate::input::kind::LINE => {
                    if !run.is_empty() {
                        out.push(node(std::mem::take(&mut run)));
                    }
                    line_left(self.input.line(*l), &mut out);
                }
                _ => {
                    if !run.is_empty() {
                        out.push(node(std::mem::take(&mut run)));
                    }
                    file_left(self.input.file(*l), &mut out);
                }
            }
        }
        for s in self.input.text_rest() {
            match s {
                Seg::Toks(t) => toks_left(t, &mut out, &mut run),
                Seg::Line(l) => {
                    if !run.is_empty() {
                        out.push(node(std::mem::take(&mut run)));
                    }
                    line_left(l, &mut out);
                }
                Seg::File(f) => {
                    if !run.is_empty() {
                        out.push(node(std::mem::take(&mut run)));
                    }
                    file_left(f, &mut out);
                }
            }
        }
        if !run.is_empty() {
            out.push(node(run));
        }
        // (conditionals open when the run closed its scope go on in the
        // first of them, `carried_conds`)
        if self.closed_scope()
            && !self.conds.is_empty()
            && let Some(first) = out.first_mut()
            && let Some(fs) = first.first_mut()
        {
            let mut xs = vec![tag("c")];
            xs.extend(self.conds.iter().map(|&c| v.int(i64::from(c))));
            xs.push(v.int(-1));
            xs.extend(self.if_codes.iter().map(|&c| v.int(i64::from(c))));
            let mut frames = vec![v.list(xs)];
            frames.extend(list_of(v, *fs));
            *fs = v.list(frames);
        }
        out
    }
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
        let r = self.cx.scope_kind();
        if r.is_err() {
            self.trace_stop("scope kind");
        }
        Ok(r?)
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
        loop {
            while self.has_input() {
                if self.give_back_peeked()? {
                    return Ok(());
                }
                if self.input.arena.len() > self.input.compact_at.max(crate::input::ARENA_COMPACT) {
                    self.input.compact();
                }
                self.begin_command();
                self.command()?;
                self.mark.active = false;
                if self.claim_rest || self.closed_scope() {
                    return Ok(());
                }
                // (one scope per node, `Effect::Scope` carrying one kind:
                // a region in vertical mode inside one group of its own
                // that is about to open another leaves the first to the
                // engine here, and the rest of its input follows inside)
                if self.can_leave_scope() && self.nest[0].mode.abs() == 1 && self.next_opens_group()
                {
                    self.leave_scope()?;
                    return Ok(());
                }
            }
            // a region that ends with one group of its own still open
            // (an environment's `\begin`, a macro's `\begingroup` or
            // `\bgroup`), outside every paragraph, box and conditional,
            // leaves it to the engine as the scope the following regions
            // run in (`Run::leave_scope`); more than one, or a text that
            // said it opens no scope, reads on
            if self.can_leave_scope() {
                self.leave_scope()?;
                return Ok(());
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
                let nest = std::mem::take(&mut self.nest);
                self.abandon_levels(&nest);
                for l in nest {
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

    /// Whether the run is a document region with exactly one group of
    /// its own open (sharing the list below it), nothing else open, no
    /// scope effect yet, and a text that does not say it opens none: the
    /// group can be left to the engine as a scope (`Run::leave_scope`).
    fn can_leave_scope(&self) -> bool {
        self.doc
            && self.conds.is_empty()
            && self.effect == Effect::None
            && self.nest.len() == 1
            && !self.nest[0].owns
            && matches!(
                self.nest[0].group,
                crate::val::group::SIMPLE | crate::val::group::SEMI_SIMPLE
            )
            && !crate::hints::effect_none(self.v, &self.cx.args())
            && self.text_opens_group()
            // (the scope's definitions are all the run's, `before` it:
            // a local one the run made at its base level before the
            // group opened, as `\document`'s before the `\begin` it
            // reads on into, lies outside the group and outlives it,
            // so the run reads on with the group open instead)
            && !self.st.has_base_local_def(self.v)
    }

    /// Whether the node's own text opens a group it does not close (an
    /// unmatched `\\begin{...}`, `\\begingroup` or `\\bgroup`; a brace
    /// of the text is never split between slices, `hints::slices_of`):
    /// the group its run leaves open is the text's, not one a command
    /// left open while it still reads on (a loading command's, a
    /// macro's).
    fn text_opens_group(&self) -> bool {
        let text: String = decode_segs(self.v, &self.cx.args())
            .iter()
            .filter_map(|f| match f {
                Seg::Line(s) => Some(
                    s.buf
                        .iter()
                        .map(|&c| char::from_u32(c).unwrap_or('?'))
                        .collect::<String>(),
                ),
                _ => None,
            })
            .collect();
        let n = |p: &str| i64::try_from(text.matches(p).count()).unwrap_or(0);
        (n("\\begin{") - n("\\end{"))
            + (n("\\begingroup") - n("\\endgroup"))
            + (n("\\bgroup") - n("\\egroup"))
            > 0
    }

    /// Whether the text the run reads next begins (after blanks) with a
    /// command that opens a group: `\\begin`, `\\begingroup`,
    /// `\\bgroup` (the document's text only).
    fn next_opens_group(&self) -> bool {
        // (the first input with something left: a line of the text, or
        // anything else, which is no group of the text)
        let mut line = None;
        for l in self.input.levels.iter().rev() {
            let more = match l.kind {
                crate::input::kind::TOKS => l.a < l.b,
                crate::input::kind::LINE => {
                    let x = self.input.line(*l);
                    if x.has_more() {
                        line = Some(x);
                    }
                    x.has_more()
                }
                _ => self.input.file(*l).has_lines(),
            };
            if more {
                break;
            }
        }
        if line.is_none()
            && self.input.levels.is_empty()
            && let Some(Seg::Line(x)) = self.input.text_rest().iter().find(|s| match s {
                Seg::Toks(t) => !t.is_empty(),
                Seg::Line(l) => l.has_more(),
                Seg::File(f) => f.has_lines(),
            })
        {
            line = Some(x);
        }
        let Some(s) = line else {
            return false;
        };
        let rest = &s.buf[s.loc.min(s.buf.len())..];
        let k = rest
            .iter()
            .position(|&c| !matches!(c, 32 | 9 | 10 | 13))
            .unwrap_or(rest.len());
        let word: String = rest[k..]
            .iter()
            .take(12)
            .map(|&c| char::from_u32(c).unwrap_or('?'))
            .collect();
        ["\\begingroup", "\\bgroup", "\\begin{"]
            .iter()
            .any(|p| word.starts_with(p))
    }

    /// Whether a document run closed the engine scope it ran in, with
    /// nothing of its own open but conditionals: it ends after that
    /// command (`Effect::Close`, its reads and definitions inside the
    /// scope), and what it has left of its input runs outside it as the
    /// following nodes, the conditionals carried ([`Run::leftover`]).
    fn closed_scope(&self) -> bool {
        self.doc && self.effect == Effect::Close && self.nest.is_empty()
    }

    /// Whether the run has something open that must close in this run:
    /// a level of the nest or a conditional.
    pub(crate) fn open_span(&self) -> bool {
        !self.nest.is_empty() || !self.conds.is_empty()
    }

    /// Ends the run: definitions, contributions, children (what the
    /// command made, then the input left), the claim and the scope effect.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn finish(mut self) -> ValueId {
        crate::pdfout::publish(self.pdf_fold.take());
        // (a command node's, empty too: what its text declares,
        // `Exec::static_contribs`)
        if !self.doc_lines.is_empty() || self.cx.op() == self.ops.exec {
            let m = lines_value(self.v, &self.doc_lines);
            self.contribute("lines", m);
        }
        // a page parameter this run set and no item of it carried
        if self.st.take_page_params() {
            self.put_page_params();
        }
        let op = self.cx.op();
        let t_pub = std::time::Instant::now();
        let published: Vec<(Field, ValueId, Locality)> = self
            .st
            .published(self.v, self.rec_load)
            .into_iter()
            .map(|(loc, x, l)| (self.loc_field(loc), x, l))
            .collect();
        // (debug, `PHITEX_PUB_TRACE=<a>|<b>…`: the definitions of a run whose
        // first text segment contains one of the texts)
        if let Ok(pat) = std::env::var("PHITEX_PUB_TRACE") {
            let v = self.v;
            let first = self
                .cx
                .args()
                .first()
                .and_then(|&l| list_of(v, l).first().map(|&f| v.render(f)))
                .unwrap_or_default();
            if pat.split('|').any(|p| first.contains(p)) {
                let node = self.cx.node().raw();
                eprintln!(
                    "pub: node {node} pulled {} claim_rest {} text {}",
                    self.pulled,
                    self.claim_rest,
                    first.chars().take(400).collect::<String>()
                );
                for (f, x, l) in &published {
                    eprintln!(
                        "pub: node {node} {:?} {} = {}",
                        l,
                        crate::val::field_name(v, *f),
                        v.render(*x).chars().take(160).collect::<String>()
                    );
                }
                for (f, x) in &self.contribs {
                    eprintln!(
                        "pub: node {node} contrib {} += {}",
                        crate::val::field_name(v, *f),
                        v.render(*x).chars().take(300).collect::<String>()
                    );
                }
                eprintln!(
                    "pub: node {node} effect {:?} kids {} follow {}",
                    self.effect,
                    self.kids.len(),
                    self.leftover().len()
                );
            }
        }
        // (debug, `PHITEX_ITEMS_TRACE=1`: every run's items, after its
        // text's first segment and its claim)
        if crate::exec::env_flag!("PHITEX_ITEMS_TRACE") {
            let v = self.v;
            let first = self
                .cx
                .args()
                .first()
                .and_then(|&l| list_of(v, l).first().map(|&f| v.render(f)))
                .unwrap_or_default();
            let items = v.field("items");
            let mut out = format!(
                "items: node {} op {:?} pulled {} rest {} effect {:?} text {}\n",
                self.cx.node().raw(),
                op,
                self.pulled,
                self.claim_rest,
                self.effect,
                first.chars().take(120).collect::<String>()
            );
            for (f, x) in &self.contribs {
                if *f == items {
                    for e in v.seq_to_vec(*x) {
                        out.push_str("items:   ");
                        out.extend(v.render(e).chars().take(400));
                        out.push('\n');
                    }
                }
            }
            eprint!("{out}");
        }
        // (a load whose run went on into its siblings is not one load:
        // no record of it)
        if self.rec_load && crate::exec::env_flag!("PHITEX_PKG_TRACE") {
            eprintln!(
                "pkgrec: load run {} finishes: pulled {} (blanks only {}), rest claimed {}, effect {:?}, {} definitions",
                self.cx.node().raw(),
                self.pulled,
                self.pulled_only_blanks(),
                self.claim_rest,
                self.effect,
                published.len()
            );
        }
        if self.rec_load && op == self.ops.exec && !self.claim_rest && self.pulled_only_blanks() {
            self.record_load(&published);
        }
        {
            let (n, t_list) = (published.len(), t_pub.elapsed());
            for (f, x, l) in published {
                crate::exec::widths::def(f);
                self.cx.define_with(f, x, l);
            }
            if n >= 10_000 && crate::exec::env_flag!("PHITEX_TEX_TIME") {
                eprintln!(
                    "run finish: {n} definitions published: listed {t_list:?}, define_with {:?}",
                    t_pub.elapsed().checked_sub(t_list).unwrap_or_default()
                );
            }
        }
        let locs: Vec<Loc> = self.defined_now();
        if !locs.is_empty() {
            let names: Vec<ValueId> = locs.into_iter().map(|l| self.loc_field(l).0).collect();
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

    /// Whether every sibling the run took is blank text (a line's end
    /// left as a node of its own): the run is still its load's alone.
    fn pulled_only_blanks(&self) -> bool {
        (0..self.pulled).all(|i| match self.cx.following(i) {
            Some((op, args)) if op == self.ops.exec => args.first().is_some_and(|&fr| {
                list_of(self.v, fr).into_iter().all(|f| {
                    let xs = list_of(self.v, f);
                    xs.first().map(|&t| str_of(self.v, t)).as_deref() == Some("s")
                        && xs
                            .get(1)
                            .is_some_and(|&t| str_of(self.v, t).trim().is_empty())
                })
            }),
            _ => false,
        })
    }

    /// A package load's record (`crate::pkgrec`): the files it read (and
    /// looked for in vain), the fields it defined (`published`) and read
    /// from the engine, whether it opened no scope and took no sibling;
    /// contributed to `pkgrecs`, and its run's times logged.
    fn record_load(&mut self, published: &[(Field, ValueId, Locality)]) {
        let v = self.v;
        // (the files the job writes and reads back are the document's,
        // never part of a load's key)
        let mut files: Vec<(String, u64)> = self
            .fread
            .iter()
            .filter(|(n, _)| !self.written_known(n))
            .cloned()
            .collect();
        files.extend(
            self.fmiss
                .iter()
                .filter(|n| !self.written_known(n))
                .map(|n| (n.clone(), 0)),
        );
        files.dedup();
        let writes: Vec<String> = published
            .iter()
            .map(|&(f, _, _)| crate::val::field_name(v, f))
            .collect();
        // (a field the load put back as it found it is no read of its
        // predecessor's either: only the net effect is recorded)
        let restored = self.st.restored_locs(v);
        let reads: Vec<String> = self
            .st
            .read_locs()
            .into_iter()
            .filter(|l| !restored.contains(l))
            .map(|l| crate::val::field_name(v, self.loc_field(l)))
            .filter(|n| !n.starts_with("file:"))
            .collect();
        let flags = i64::from(self.effect == Effect::None) | 2;
        let node = self.cx.node().raw();
        // (the values published: a later build's prediction of this
        // load's outputs, `crate::pkgrec::LoadHints::prediction`)
        let values: Vec<(String, bool, ValueId)> = published
            .iter()
            .map(|&(f, x, l)| (crate::val::field_name(v, f), l == Locality::Global, x))
            .collect();
        if crate::exec::env_flag!("PHITEX_PKG_TRACE") {
            let skips: Vec<String> = self
                .skip_first
                .clone()
                .into_iter()
                .map(|l| crate::val::field_name(v, self.loc_field(l)))
                .collect();
            eprintln!("pkgrec: skip reads of node {node}: {skips:?}");
        }
        let e = crate::pkgrec::entry(v, node, &files, &writes, &reads, flags, &values);
        self.contribute("pkgrecs", v.seq_unit(e));
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

/// Spent token lists by length, for the next macro body of that length
/// The segments of an `Exec` node's arguments.
pub(crate) fn decode_segs(v: &Values, args: &[ValueId]) -> Vec<Seg> {
    decode_segs_upto(v, args, usize::MAX)
}

/// The first of a node's segments, the rest not decoded.
pub(crate) fn first_seg(v: &Values, args: &[ValueId]) -> Option<Seg> {
    decode_segs_upto(v, args, 1).into_iter().next()
}

fn decode_segs_upto(v: &Values, args: &[ValueId], limit: usize) -> Vec<Seg> {
    let mut out = Vec::new();
    let Some(&l) = args.first() else { return out };
    let Value::List(fs) = &*v.get(l) else {
        return out;
    };
    for &f in fs {
        if out.len() >= limit {
            break;
        }
        out.extend(crate::val::frame_of(v, f, || decode_seg(v, f)));
    }
    out
}

/// The conditionals a node's text begins inside: those its run's
/// predecessor left open when it closed its scope ([`Run::leftover`]),
/// `(if_limit, cur_if)` each.
pub(crate) fn carried_conds(v: &Values, args: &[ValueId]) -> Option<(Vec<u8>, Vec<i32>)> {
    let &l = args.first()?;
    let Value::List(fs) = &*v.get(l) else {
        return None;
    };
    let &f = fs.first()?;
    let Value::List(xs) = &*v.get(f) else {
        return None;
    };
    if xs.first().map(|&t| str_of(v, t)).as_deref() != Some("c") {
        return None;
    }
    let ns: Vec<i64> = xs[1..].iter().map(|&x| int_of(v, x)).collect();
    let k = ns.iter().position(|&x| x < 0)?;
    let conds = ns[..k]
        .iter()
        .map(|&c| u8::try_from(c).unwrap_or(0))
        .collect();
    let codes = ns[k + 1..]
        .iter()
        .map(|&c| i32::try_from(c).unwrap_or(0))
        .collect();
    Some((conds, codes))
}

/// One frame of a node's arguments (a frame is an immutable value, so
/// One segment of a node's arguments (a segment is an immutable value,
/// so its decoding is kept by id, [`crate::val::frame_of`]).
fn decode_seg(v: &Values, f: ValueId) -> Option<Seg> {
    let mut out = None;
    {
        let Value::List(xs) = &*v.get(f) else {
            return None;
        };
        match xs.first().map(|&t| str_of(v, t)).as_deref() {
            Some("t") => {
                let toks = crate::val::toks_of(v, xs.get(1).copied());
                out = Some(Seg::Toks(toks));
            }
            Some("f") => {
                if let (Some(&id), Some(&o)) = (xs.get(1), xs.get(2)) {
                    let off = usize::try_from(int_of(v, o)).unwrap_or(0);
                    let name = xs.get(3).copied().unwrap_or(ValueId::UNIT);
                    out = Some(Seg::File(FileRest::new(v, id, off, name)));
                }
            }
            Some("s") => {
                let text = xs.get(1).map(|&t| str_of(v, t)).unwrap_or_default();
                out = Some(Seg::Line(Line {
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
                    lineno: xs
                        .get(7)
                        .map_or(0, |&x| u32::try_from(int_of(v, x)).unwrap_or(0)),
                }));
            }
            _ => {}
        }
    }
    out
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

// ------------------------------------------------------------- parking

/// A [`crate::nest::Level`] without `Rc`.
struct PLevel {
    mode: i32,
    group: i64,
    lid: i64,
    owns: bool,
    list: Vec<ValueId>,
    fold: crate::build::FoldState,
    par: Option<(i64, i64)>,
    prev_graf: i64,
    extra: Vec<ValueId>,
    gid: u32,
    save_base: usize,
    def_base: usize,
    open_stamp: u64,
    ag: Vec<Tok>,
}

/// A run stopped at a read, at the start of the command the read
/// abandoned ([`Run::rewind`]): every owned field of [`Run`] but the
/// caches, which the resumed run rebuilds ([`phitex_defuse::Ctx::park`]).
#[allow(clippy::struct_excessive_bools)]
pub(crate) struct Parked {
    /// The values' epoch the ids below belong to.
    epoch: u64,
    /// The input left, top first: the levels' rests, then the text.
    segs: Vec<Seg>,
    pulled: usize,
    /// Where each pulled sibling's text begins, as an index into `segs`'
    /// text part: [`Run::pull_at`], kept so a resumed run can still give a
    /// peeked sibling back.
    pull_at: Vec<Option<usize>>,
    pull_orig: Vec<Option<crate::input::Line>>,
    top: bool,
    claim_rest: bool,
    hint_void: bool,
    nest: Vec<PLevel>,
    next_lid: i64,
    last_par: (i64, i64),
    doc: bool,
    fread: Vec<(String, u64)>,
    fmiss: Vec<String>,
    skip_first: Vec<Field>,
    rec_load: bool,
    no_pull: bool,
    st: crate::runstate::PState,
    contribs: Vec<(Field, ValueId)>,
    kids: Vec<(OpId, Vec<ValueId>)>,
    effect: Effect,
    conds: Vec<u8>,
    if_codes: Vec<i32>,
    astate: Option<i64>,
    aopen: i64,
    lines_entered: i64,
    lines_read: u64,
    doc_lines: Vec<(ValueId, i64)>,
    eof: bool,
    radix: i32,
    cur_order: phitex_typeset::glue::Order,
    pdf_occ: usize,
    irreversible: u64,
    read_back_done: bool,
    read_back_at: Option<crate::input::Level>,
}

impl Run<'_, '_> {
    /// The run's state for [`phitex_defuse::Ctx::park`] after a stop:
    /// `None` if the stopped command cannot be rewound.
    #[allow(clippy::too_many_lines, reason = "one state field a line")]
    pub(crate) fn park(&mut self) -> Option<Parked> {
        if !self.rewind() {
            return None;
        }
        crate::pdfout::publish(self.pdf_fold.take());
        let mut segs = Vec::new();
        let input = std::mem::replace(&mut self.input, crate::input::Input::new());
        for l in input.levels.iter().rev() {
            match l.kind {
                crate::input::kind::TOKS => {
                    segs.push(Seg::Toks(input.arena.get(l.rest()).to_vec()));
                }
                crate::input::kind::LINE => segs.push(Seg::Line(input.line(*l).clone())),
                _ => segs.push(Seg::File(input.file(*l).clone())),
            }
        }
        // where each pulled sibling's text begins in the resumed run's
        // text: the levels' rests come first, then the text not reached
        let nlev = input.levels.len();
        let last = self.pull_at.len().wrapping_sub(1);
        let mut pull_at = Vec::with_capacity(self.pull_at.len());
        let mut pull_orig = Vec::with_capacity(self.pull_at.len());
        for (i, p) in self.pull_at.iter().enumerate() {
            let (at, orig) = match *p {
                Some(at) if at >= input.text_at => (Some(nlev + at - input.text_at), None),
                // the last sibling's first line reached, its first token
                // read and put back on top of it: the token and the rest
                // of the line become two segments
                Some(at) if i == last && at + 1 == input.text_at => {
                    match (self.pull_lev, &input.text[at]) {
                        (Some(j), Seg::Line(orig))
                            if j + 1 < nlev
                                && input.levels[j].kind == crate::input::kind::LINE
                                && input.levels[j + 1].kind == crate::input::kind::TOKS
                                && input.levels[j + 1].ty == crate::input::ty::BACKED_UP
                                && input.levels[j + 1].b - input.levels[j + 1].a == 1 =>
                        {
                            (Some(nlev - 2 - j), Some(orig.clone()))
                        }
                        _ => (None, None),
                    }
                }
                _ => (None, None),
            };
            pull_at.push(at);
            pull_orig.push(orig);
        }
        segs.extend(input.text_rest().iter().cloned());
        let nest = std::mem::take(&mut self.nest)
            .into_iter()
            .map(|l| PLevel {
                mode: l.mode,
                group: l.group,
                lid: l.lid,
                owns: l.owns,
                list: l.list,
                fold: l.fold,
                par: l.par,
                prev_graf: l.prev_graf,
                extra: l.extra,
                gid: l.gid,
                save_base: l.save_base,
                def_base: l.def_base,
                open_stamp: l.open_stamp,
                ag: l.ag,
            })
            .collect();
        Some(Parked {
            epoch: self.cx.values_epoch(),
            segs,
            pulled: self.pulled,
            pull_at,
            pull_orig,
            top: self.top,
            claim_rest: self.claim_rest,
            hint_void: self.hint_void,
            nest,
            next_lid: self.next_lid,
            last_par: self.last_par,
            doc: self.doc,
            fread: std::mem::take(&mut self.fread),
            fmiss: std::mem::take(&mut self.fmiss),
            skip_first: std::mem::take(&mut self.skip_first)
                .into_iter()
                .map(|l| self.loc_field(l))
                .collect(),
            rec_load: self.rec_load,
            no_pull: self.no_pull,
            st: self.st.park(self.v),
            contribs: std::mem::take(&mut self.contribs),
            kids: std::mem::take(&mut self.kids),
            effect: self.effect,
            conds: std::mem::take(&mut self.conds),
            if_codes: std::mem::take(&mut self.if_codes),
            astate: self.astate,
            aopen: self.aopen,
            lines_entered: self.lines_entered,
            lines_read: self.lines_read,
            doc_lines: self.doc_lines.clone(),
            eof: self.eof,
            radix: self.radix,
            cur_order: self.cur_order,
            pdf_occ: self.pdf_occ,
            irreversible: self.irreversible,
            read_back_done: self.read_back_done,
            read_back_at: self.read_back_at,
        })
    }
}

impl<'c, 'a> Run<'c, 'a> {
    /// A run continuing a parked attempt at the command it stopped in;
    /// `Err(cx)` if the token is of another values epoch (its ids may be
    /// swept): the run then starts from the top.
    pub(crate) fn from_parked(
        cx: &'c mut Ctx<'a>,
        ops: Ops,
        p: Parked,
    ) -> Result<Self, &'c mut Ctx<'a>> {
        if p.epoch != cx.values_epoch() {
            return Err(cx);
        }
        let mut r = Run::new(cx, ops);
        r.input.text = p.segs;
        r.input.text_at = 0;
        r.nest = p
            .nest
            .into_iter()
            .map(|l| {
                let mut x = crate::nest::Level::new(l.mode, l.group, l.lid, l.owns, l.extra);
                x.list = l.list;
                x.fold = l.fold;
                x.par = l.par;
                x.prev_graf = l.prev_graf;
                x.gid = l.gid;
                x.save_base = l.save_base;
                x.def_base = l.def_base;
                x.open_stamp = l.open_stamp;
                x.ag = l.ag;
                x
            })
            .collect();
        r.pulled = p.pulled;
        r.pull_at = p.pull_at;
        r.pull_orig = p.pull_orig;
        r.top = p.top;
        r.claim_rest = p.claim_rest;
        r.hint_void = p.hint_void;
        r.next_lid = p.next_lid;
        r.last_par = p.last_par;
        r.doc = p.doc;
        r.fread = p.fread;
        r.fmiss = p.fmiss;
        r.skip_first = p.skip_first.into_iter().map(|f| r.loc_of(f)).collect();
        r.rec_load = p.rec_load;
        r.no_pull = p.no_pull;
        r.st = crate::runstate::State::unpark(p.st);
        r.contribs = p.contribs;
        r.kids = p.kids;
        r.effect = p.effect;
        r.conds = p.conds;
        r.if_codes = p.if_codes;
        r.astate = p.astate;
        r.aopen = p.aopen;
        r.lines_entered = p.lines_entered;
        r.lines_read = p.lines_read;
        r.doc_lines.clone_from(&p.doc_lines);
        r.eof = p.eof;
        r.radix = p.radix;
        r.cur_order = p.cur_order;
        r.pdf_occ = p.pdf_occ;
        r.irreversible = p.irreversible;
        r.read_back_done = p.read_back_done;
        r.read_back_at = p.read_back_at;
        r.resumed = true;
        Ok(r)
    }
}

/// `scanner_status` (§305) values kept in [`Run::scanner`].
pub(crate) const SCAN_DEFINING: u8 = 2;
pub(crate) const SCAN_MATCHING: u8 = 3;
pub(crate) const SCAN_ABSORBING: u8 = 5;
impl Run<'_, '_> {
    /// The innermost file being read (§304): the top file level of the
    /// input stack (a line of it, or the rest of a file read here), else
    /// the file the node's token levels were left in (its `t` frame).
    pub(crate) fn file_line_here(&self) -> Option<FileLine> {
        for l in self.input.levels.iter().rev() {
            match l.kind {
                crate::input::kind::TOKS => {}
                crate::input::kind::LINE => {
                    let s = self.input.line(*l);
                    if s.lineno > 0 {
                        return Some(FileLine::Known(i64::from(s.lineno)));
                    }
                    // (a `\read` line is no file's, §483; a text
                    // without an origin, a format's or INITEX's, is one
                    // file, UNIT)
                    if !s.rescanned {
                        return Some(FileLine::Piece(s.origin.map_or(ValueId::UNIT, |o| o.0)));
                    }
                }
                _ => {
                    return Some(FileLine::Known(i64::from(self.input.file(*l).line)));
                }
            }
        }
        let v = self.v;
        let frames = self.cx.args().first().map(|&f| list_of(v, f))?;
        frames.iter().find_map(|&f| {
            let xs = list_of(v, f);
            if xs.len() < 4 || xs.first().map(|&t| str_of(v, t)).as_deref() != Some("t") {
                return None;
            }
            let n = int_of(v, xs[3]);
            Some(if n >= 0 {
                FileLine::Known(n)
            } else {
                FileLine::Piece(xs[2])
            })
        })
    }

    /// A line of a document's piece in file `name` begun (§362).
    pub(crate) fn doc_line_begun(&mut self, name: ValueId) {
        match self.doc_lines.iter_mut().find(|(n, _)| *n == name) {
            Some((_, k)) => *k += 1,
            None => self.doc_lines.push((name, 1)),
        }
    }
}

/// Whether a field the interpretation of a written line set is one of
/// TeX's definitions (§222: the table of equivalents, the hash's
/// meanings), what reading the file back defines. The writer's and the
/// page builder's own state (`pdf:` objects and counters, `pg:` page
/// contents, the streams, `\afterassignment`'s token, the dead cycles,
/// ...) is never a read-back definition, nor are category codes (the
/// read-back reads under `\makeatletter`).
fn is_read_back_definition(name: &str) -> bool {
    const NOT: [&str; 15] = [
        "cat:",
        "pdf:",
        "pg:",
        "pp:",
        "write:",
        "wopen:",
        "wpre:",
        "read:",
        "aftertoken",
        "deadcycles",
        "lvlfix",
        "badness",
        "inspen",
        "markclasses",
        "marks",
    ];
    !NOT.iter().any(|p| name.starts_with(p))
}

/// A `lines` contribution: the lines begun by file name, sorted by name.
pub(crate) fn lines_value(v: &Values, lines: &[(ValueId, i64)]) -> ValueId {
    let mut xs: Vec<(String, ValueId, i64)> =
        lines.iter().map(|&(n, k)| (str_of(v, n), n, k)).collect();
    xs.sort_by(|a, b| a.0.cmp(&b.0));
    v.list(
        xs.into_iter()
            .map(|(_, n, k)| v.list(vec![n, v.int(k)]))
            .collect(),
    )
}

/// The lines a node's own text begins, by file, as its run counts them
/// ([`Run::doc_line_begun`]): its segments not reached yet that begin a
/// line, are no `\read` line and carry no number of their own.
pub(crate) fn text_lines(v: &Values, args: &[ValueId]) -> ValueId {
    let mut lines: Vec<(ValueId, i64)> = Vec::new();
    for f in args.first().map(|&a| list_of(v, a)).unwrap_or_default() {
        let xs = list_of(v, f);
        if xs.first().map(|&t| str_of(v, t)).as_deref() != Some("s") {
            continue;
        }
        let flags = xs.get(4).map_or(3, |&x| int_of(v, x));
        let unread = xs.get(3).map_or(-2, |&x| int_of(v, x)) == -2;
        let lineno = xs.get(7).map_or(0, |&x| int_of(v, x));
        if !unread || flags & 1 == 0 || flags & 4 != 0 || lineno != 0 {
            continue;
        }
        let name = if xs.len() >= 7 { xs[5] } else { ValueId::UNIT };
        match lines.iter_mut().find(|(n, _)| *n == name) {
            Some((_, k)) => *k += 1,
            None => lines.push((name, 1)),
        }
    }
    lines_value(v, &lines)
}
