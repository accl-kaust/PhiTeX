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
use crate::runstate::{Entry, Loc};
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
    /// The run reached a page-builder candidate with input after it
    /// (`crate::pages`, spec-final A1): it ends there, its continuation
    /// the resume piece's.
    Cut,
}

impl From<Stop> for Halt {
    fn from(s: Stop) -> Halt {
        Halt::Stop(s)
    }
}

pub(crate) type R<T> = Result<T, Halt>;

/// The conditionals a run left open where it stopped at a document file
/// (spec-redesign §3: a file reached inside the taken branch of open
/// conditionals is sliced like the document): `[[if_limit, cur_if],
/// ...]`, bottom first, read on first touch by the run that ends one
/// ([`Run::load_cond_stack`]).
pub(crate) const COND_STACK: &str = "cond:stack";

/// The names of the files the job's streams write, one entry per stream
/// entry ([`Run::contribute_stream`]): what places the files' nodes
/// (`ops::Files`), unchanged by an edit of what a file holds.
pub(crate) const WNAMES: &str = "wnames";

/// The accumulators the streams' entries are dealt into by their file's
/// name ([`stream_bucket`]): a file's node reads its own one, and takes
/// its file's entries from it.
pub(crate) const STREAM_BUCKETS: u64 = 64;

/// The accumulator file `name`'s stream entries go to
/// (`wstreams:<k>`, [`STREAM_BUCKETS`]).
pub(crate) fn stream_bucket(v: &Values, name: &str) -> Field {
    let k = phitex_defuse::hash_bytes(0x7773_7472_6561_6d00, name.as_bytes()) % STREAM_BUCKETS;
    v.field(&format!("wstreams:{k}"))
}

/// The runs that read a record whole (node, record), kept under
/// `PHITEX_RECORD_CHECK=1` for [`record_check`].
static RECORD_READERS: std::sync::Mutex<Vec<(phitex_defuse::NodeId, usize)>> =
    std::sync::Mutex::new(Vec::new());

/// Debug (`PHITEX_RECORD_CHECK=1`), after an evaluation: each live node
/// that read a record, the record reaching it against the value of each
/// of its cells reaching it; a difference is reported. Inside a run a
/// read may answer with a definer's prediction, which that definer's
/// commit rechecks (a refuted reader runs again), so the record and a
/// cell read in the same run may disagree for a while: only the state
/// the evaluation leaves is compared. A stale record (no cells, its
/// readers read the cells) has nothing to compare, nor has a reader
/// inside a scope whose opener defined the record or a cell only there
/// (the engine shows a node's entry outside its scope only); such
/// readers are counted.
pub(crate) fn record_check(
    eng: &phitex_defuse::Engine,
    start: &phitex_defuse::FxHashMap<Field, ValueId>,
) -> usize {
    let mut readers = std::mem::take(
        &mut *RECORD_READERS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    );
    readers.sort_unstable();
    readers.dedup();
    let v = eng.values();
    let absent = crate::fields::record_absent(v);
    // (the value reaching `n`: a node's definition, else the value the
    // job's environment sets at the document's start, else the supplied
    // environment's; `Err` where the node reaching it defined it only
    // inside the scope it opened, an entry the engine does not show)
    let env: phitex_defuse::FxHashMap<Field, ValueId> = eng.environment().collect();
    let reaching = |n: phitex_defuse::NodeId, f: Field| match eng.reaching_def(n, f) {
        Some(d) => eng.def_value(d, f).map(Some).ok_or(()),
        None => Ok(start.get(&f).or_else(|| env.get(&f)).copied()),
    };
    let (mut bad, mut scoped, mut compared) = (0, 0, 0);
    'readers: for (n, b) in readers {
        if eng.node_info(n).is_none() {
            continue;
        }
        let rf = v.field(crate::fields::RECORDS[b]);
        let rec = match reaching(n, rf) {
            Ok(Some(rec)) => rec,
            Ok(None) => continue,
            Err(()) => {
                scoped += 1;
                continue;
            }
        };
        let rec = list_of(v, rec);
        if rec.len() != crate::fields::record_len(b) {
            continue;
        }
        let mut diffs = Vec::new();
        compared += 1;
        for (i, &x) in rec.iter().enumerate() {
            let cf = v.field(&crate::fields::record_cell_name(b, i));
            let Ok(y) = reaching(n, cf) else {
                compared -= 1;
                scoped += 1;
                continue 'readers;
            };
            let x = (x != absent).then_some(x);
            if x != y {
                diffs.push((cf, x, y));
            }
        }
        for (cf, x, y) in diffs {
            bad += 1;
            eprintln!(
                "record-check: node {} {} record {:?} cell {:?}",
                n.raw(),
                crate::val::field_name(v, cf),
                x.map(|x| v.render(x)),
                y.map(|y| v.render(y))
            );
        }
    }
    eprintln!(
        "record-check: {bad} differences in {compared} readers ({scoped} readers inside a scope its opener defined a cell in: not compared)"
    );
    bad
}

/// The operations, as registered.
#[derive(Clone)]
pub(crate) struct Ops {
    pub exec: OpId,
    pub put: OpId,
    /// A scan step over the pieces' profiles, a page's finisher and the
    /// rest of a cut piece (`crate::pages`).
    pub scan: OpId,
    pub finish: OpId,
    pub resume: OpId,
    /// The build's field handles (`crate::fields`).
    pub fields: std::sync::Arc<crate::fields::Fields>,
    /// The format's definitions the build analyses document texts with
    /// (set by `api::Build::with`; none while a format is built): a
    /// document file `\input` is sliced with them ([`Run::leftover`]).
    pub defs: std::sync::Arc<std::sync::Mutex<crate::hints::Defs>>,
    /// The build's registered text analyses and its nodes' hints
    /// (`crate::hints::Registry`).
    pub hints: std::sync::Arc<crate::hints::Registry>,
    /// The package loads' finished runs (`crate::pkgrec`): node, start,
    /// end, for [`crate::Stats`] (concurrency, preamble wall).
    pub pkg_log: std::sync::Arc<std::sync::Mutex<Vec<PkgRun>>>,
    /// The build's package records, for the engine's predicted
    /// definitions ([`crate::pkgrec::LoadHints::prediction`]).
    pub pkg_loads:
        std::sync::Arc<std::sync::Mutex<Option<std::sync::Arc<crate::pkgrec::LoadHints>>>>,
    /// The client's `\openout` name check ([`crate::Options::out_name_ok`]).
    pub out_ok: std::sync::Arc<std::sync::Mutex<Option<crate::api::OutNameOk>>>,
    /// What the last two finishers running each output routine defined,
    /// by the routine's hint value (the earlier run first): the next
    /// finisher's declared names, and its predicted definitions where
    /// both agree (`crate::pages::Finish`).
    pub fin_defs: std::sync::Arc<std::sync::Mutex<crate::FxHashMap<u64, crate::pages::FinDefs>>>,
    /// The class projection of meanings ([`crate::val::class_projection`],
    /// registered with the engine): a lookup that only stores its token
    /// reads the name's class, not its meaning ([`Run::class_lookup`]).
    pub class_proj: phitex_defuse::ProjId,
    /// The names of fonts found not virtual (`pdfout::do_vf`): a memo
    /// of the build, not a field, so the finisher that found out does
    /// not define something every later finisher reads (an edit
    /// replacing that finisher ran them all again).
    pub not_virtual: std::sync::Arc<std::sync::Mutex<phitex_defuse::FxHashSet<String>>>,
    /// The build's read-time PDF commands, kept for the link by serial
    /// (`crate::pdfout::command`).
    pub pdf_cmds: std::sync::Arc<crate::pdfout::CmdMemo>,
    /// The guess cache of the scan (spec-final §3.4): what the scan's
    /// continuation after page `j` fired defined the last time it ran
    /// (the scan state and its material), the prediction of a
    /// continuation made anew for page `j` (an edit's: its steps run on
    /// it in parallel, the engine confirms it when the real one comes).
    pub scan_guess: std::sync::Arc<std::sync::Mutex<crate::FxHashMap<i64, (ValueId, ValueId)>>>,
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
    /// A file the job writes, as its units (`ops::Files`): each unit's
    /// field and whether it begins a line, and whether the file slices
    /// into pieces; its text is read from the units only when a run
    /// reads its lines itself ([`Run::load_written`]).
    pub units: Option<std::sync::Arc<[(ValueId, bool)]>>,
    pub sliceable: bool,
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
            units: None,
            sliceable: false,
        }
    }

    /// A file the job writes, from its order value (`ops::Files`:
    /// `[sliceable, [[unit field, starts], ...]]`), its text not read.
    pub(crate) fn written(v: &Values, order: ValueId, name: ValueId) -> FileRest {
        let xs = list_of(v, order);
        let units: Vec<(ValueId, bool)> = xs
            .get(1)
            .map(|&l| list_of(v, l))
            .unwrap_or_default()
            .iter()
            .filter_map(|&e| {
                let ys = list_of(v, e);
                Some((*ys.first()?, ys.get(1).is_some_and(|&s| int_of(v, s) != 0)))
            })
            .collect();
        let mut f = FileRest::new(v, v.str(""), 0, name);
        f.sliceable = xs.first().is_some_and(|&s| int_of(v, s) != 0);
        f.units = Some(units.into());
        f
    }

    /// Whether its text is still to be read from its units.
    pub(crate) fn lazy(&self) -> bool {
        self.units.as_ref().is_some_and(|u| !u.is_empty()) && self.str().is_empty()
    }

    /// Its text, read from its units ([`Run::load_written`]).
    pub(crate) fn set_text(&mut self, v: &Values, id: ValueId) {
        self.id = id;
        self.text = v.get(id);
    }

    /// The file's name and the line number of the next line, for
    /// diagnostics ([`Run::heartbeat_tick`]).
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
        self.off < self.str().len() || self.lazy()
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

/// The engine reads that stopped a run, by the reader's kind and field
/// (spec-final §9b: what the raises are on), counted while
/// `PHITEX_TEX_TIME` is set.
type StopTable = phitex_defuse::FxHashMap<(&'static str, Field), u64>;
static STOPS: std::sync::Mutex<Option<StopTable>> = std::sync::Mutex::new(None);

/// A run of kind `op` stopped reading `f`.
#[cold]
pub(crate) fn note_stop(op: &'static str, f: Field) {
    if crate::exec::env_flag!("PHITEX_TEX_TIME") {
        let mut m = STOPS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *m.get_or_insert_with(Default::default)
            .entry((op, f))
            .or_default() += 1;
    }
}

/// The fields runs stopped on most since the last call, with the
/// readers' kind and counts.
pub(crate) fn take_stops(v: &Values, n: usize) -> Vec<(String, u64)> {
    let m = STOPS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take()
        .unwrap_or_default();
    let mut xs: Vec<(String, u64)> = m
        .into_iter()
        .map(|((op, f), k)| (format!("{op} {}", crate::val::field_name(v, f)), k))
        .collect();
    xs.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    xs.truncate(n);
    xs
}

/// The size of a run's control-sequence name cache ([`Run::cs_cache`]).
pub(crate) const CS_CACHE: usize = 1024;

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
    /// The commands this run began ([`Run::command`]), the count the
    /// heartbeat's next tick is at (`u64::MAX`: off) and the count its
    /// last tick took in.
    pub cmds: u64,
    pub hb_at: u64,
    pub hb_done: u64,
    pub pulled: usize,
    /// Where each pulled sibling's text begins in the input's text (its
    /// first segment's index), or `None` when it joined the line before
    /// it: [`Run::give_back_peeked`].
    pub pull_at: Vec<Option<usize>>,
    /// The input level that holds the last pulled sibling's first line,
    /// once reached: where [`Run::park`] finds it.
    pub pull_lev: Option<usize>,
    /// For a resumed run, per pulled sibling: its first line as it was
    /// before its first token was peeked, when the park left that token
    /// and the rest of the line as two text segments
    /// ([`Run::give_back_peeked`]).
    pub pull_orig: Vec<Option<crate::input::Line>>,
    /// Per pulled sibling: whether a box of the run's own was open when
    /// it was pulled (a figure's float: one node, like a picture).
    pub pull_box: Vec<bool>,
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
    pub(crate) cat_cache: [u8; 256],
    pub(crate) cat_gen: u64,
    /// The code tables (`lc`, `uc`, `sf`, `mc`, `dc`) of characters
    /// 0–255 as last read (`i32::MIN`: not read), valid while the
    /// table's generation is `code_gen` (the catcodes' generation, which
    /// every code change bumps too).
    code_cache: Box<[[i32; 256]; 5]>,
    code_gen: u64,
    /// Fonts decoded in this run, with the table's change generation
    /// they were decoded at ([`Run::font_entry`]).
    pub font_memo: phitex_defuse::FxHashMap<u32, (u64, Option<std::rc::Rc<crate::fonts::Entry>>)>,
    /// Set while a token is fetched only to be stored (`\edef`'s and
    /// `\write`'s expansion, §478): an unexpandable meaning not read
    /// in full is read by its class ([`Run::class_lookup`]).
    pub class_reads: bool,
    /// The meanings this run read by class only, with their command
    /// codes (the engine holds the projected reads; this run's own
    /// lookups and the cut checks need them).
    pub class_memo: phitex_defuse::FxHashMap<Loc, u8>,
    pub contribs: Vec<(Field, ValueId)>,
    /// Set while `\advance` defines its register ([`State::note_kind`]).
    pub in_advance: bool,
    pub kids: Vec<(OpId, Vec<ValueId>)>,
    pub effect: Effect,
    pub cur: Cur,
    /// Conditionals begun in this run: their `if_limit` (§489).
    pub conds: Vec<u8>,
    /// The codes (`cur_if`) of `conds`, parallel to it.
    pub if_codes: Vec<i32>,
    /// Whether this run read the conditionals a node before it left open
    /// ([`COND_STACK`], [`Run::load_cond_stack`]).
    pub cond_loaded: bool,
    /// Whether this run stopped at a document file with conditionals
    /// open ([`Run::run_all`]): it leaves them as [`COND_STACK`].
    pub cond_stop: bool,
    /// The buffer a control sequence's name is scanned into (§356).
    pub(crate) name_buf: String,
    /// `\endlinechar` as last read, with the table's change generation
    /// then ([`Run::endlinechar`]).
    pub(crate) elc: Option<(u64, i32)>,
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
    pub muts: u64,
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
    /// The pages (`crate::pages`): finish candidates passed so far, the
    /// candidate the run stops at (its second pass after a cut), what it
    /// had done at its last candidate, the insertion classes put on the
    /// main list since, a finisher's page, and the cut its fold found.
    pub cand: u32,
    pub cand_marks: Vec<crate::pages::CandMark>,
    /// The scope kind the node runs in, once read ([`Run::scope_kind`]):
    /// it does not change during the run.
    #[allow(clippy::option_option)]
    scope_kind_memo: Option<Option<ValueId>>,
    pub ins_pending: Vec<u16>,
    pub fin: Option<crate::pages::FinCtx>,
    pub cut: Option<u32>,
    /// The records read ([`crate::fields::RECORDS`]), by record: `None`
    /// not read yet; `Some(None)` undefined or stale (its cells are read
    /// one by one); `Some(Some(cells))` each cell's value.
    #[allow(clippy::option_option)]
    pub(crate) recs: [Option<Option<Rc<[ValueId]>>>; crate::fields::NREC],
    /// The records the run defines at its end ([`Run::ready_records`]).
    pub(crate) rec_out: Vec<(Field, ValueId, Locality)>,
    /// The engine values of record cells read for a stale record
    /// ([`Run::engine_cell`]).
    pub(crate) cell_eng: phitex_defuse::FxHashMap<Loc, Option<ValueId>>,
    /// [`Run::rec_out`] is made, and the state table's signature then.
    pub(crate) rec_ready: bool,
    pub(crate) rec_absent: Option<ValueId>,
    pub(crate) rec_sig: (usize, usize, usize, u64),
    /// The engine reads a scan step's builder made through its run
    /// (`pages::RunReaching`): a prefix folded with none is memoized.
    pub reach_reads: u64,
    /// A finisher's `\box255`, native, from its page's assembly to its
    /// routine ([`crate::pages::Fire::b255`]).
    pub fin_box: Option<(phitex_typeset::node::BoxNode, crate::nodes::Side)>,
    /// The main loop's `lig_stack`, kept between words (§1034).
    pub(crate) lig_buf: Vec<crate::build::LigItem>,
}

impl<'c, 'a> Run<'c, 'a> {
    #[allow(clippy::too_many_lines)]
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
            cmds: 0,
            hb_at: Run::heartbeat_start(),
            hb_done: 0,
            pulled: 0,
            pull_at: Vec::new(),
            pull_lev: None,
            pull_orig: Vec::new(),
            pull_box: Vec::new(),
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
            code_cache: Box::new([[i32::MIN; 256]; 5]),
            code_gen: u64::MAX,
            font_memo: phitex_defuse::FxHashMap::default(),
            class_reads: false,
            class_memo: phitex_defuse::FxHashMap::default(),
            contribs: Vec::new(),
            in_advance: false,
            kids: Vec::new(),
            effect: Effect::None,
            cur: Cur {
                tok: Tok::c(0),
                cmd: 0,
                chr: 0,
            },
            conds: Vec::new(),
            if_codes: Vec::new(),
            cond_loaded: false,
            cond_stop: false,
            name_buf: String::new(),
            elc: None,
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
            muts: 0,
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
            cand: 0,
            cand_marks: Vec::new(),
            scope_kind_memo: None,
            ins_pending: Vec::new(),
            fin: None,
            cut: None,
            recs: std::array::from_fn(|_| None),
            rec_out: Vec::new(),
            rec_ready: false,
            rec_absent: None,
            cell_eng: phitex_defuse::FxHashMap::default(),
            rec_sig: (0, 0, 0, 0),
            fin_box: None,
            lig_buf: Vec::new(),
            reach_reads: 0,
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

    /// An engine read of field `f` outside the state table (by name): a
    /// record's cell from its record.
    pub(crate) fn eng_read(&mut self, f: Field) -> Result<Option<ValueId>, phitex_defuse::Stop> {
        if let Some((b, i)) = self.ops.fields.record_of_field(f) {
            let cells = match self.record(b) {
                Ok(c) => c,
                Err(Halt::Stop(s)) => return Err(s),
                Err(_) => None,
            };
            if let Some(cells) = cells {
                let x = cells[i];
                return Ok((x != self.absent()).then_some(x));
            }
        }
        let r = self.cx.use_field(f);
        if r.is_err() {
            note_stop(self.op_name(), f);
        }
        r
    }

    /// This run's operation's kind.
    pub(crate) fn op_name(&self) -> &'static str {
        let op = self.cx.op();
        let o = &self.ops;
        if op == o.exec {
            "exec"
        } else if op == o.resume {
            "resume"
        } else if op == o.scan {
            "scan"
        } else if op == o.finish {
            "finish"
        } else {
            "other"
        }
    }

    /// The value of an absent cell in a record (made once a run).
    fn absent(&mut self) -> ValueId {
        if let Some(a) = self.rec_absent {
            return a;
        }
        let a = crate::fields::record_absent(self.v);
        self.rec_absent = Some(a);
        a
    }

    /// Record `b`'s cells, read once a run (`None`: undefined or stale,
    /// its cells read one by one).
    pub(crate) fn record(&mut self, b: usize) -> R<Option<Rc<[ValueId]>>> {
        if let Some(c) = &self.recs[b] {
            return Ok(c.clone());
        }
        let f = self.ops.fields.slot_field(crate::fields::record_loc(b));
        let x = self.cx.use_field(f)?;
        crate::exec::widths::read(f);
        let v = self.v;
        let cells: Option<Rc<[ValueId]>> = x
            .map(|x| list_of(v, x))
            .filter(|xs| xs.len() == crate::fields::record_len(b))
            .map(Rc::from);
        // (debug, `PHITEX_RECORD_CHECK=1`: the record's readers, whose
        // record is compared with its cells after the evaluation,
        // [`record_check`])
        if cells.is_some() && crate::exec::env_flag!("PHITEX_RECORD_CHECK") {
            RECORD_READERS
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((self.cx.node(), b));
        }
        self.recs[b] = Some(cells.clone());
        Ok(cells)
    }

    /// The engine's value of cell `loc` reaching the run (what it read,
    /// or a read of the cell's own field).
    fn engine_cell(&mut self, loc: Loc) -> R<Option<ValueId>> {
        if let Some(x) = self.st.engine_value(loc) {
            return Ok(x);
        }
        if let Some(&x) = self.cell_eng.get(&loc) {
            return Ok(x);
        }
        let f = self.loc_field(loc);
        let x = self.cx.use_field(f)?;
        self.cell_eng.insert(loc, x);
        Ok(x)
    }

    /// Reads what [`Run::record_after`] of record `b` reads: the record,
    /// or (undefined or stale) its cells' engine values.
    pub(crate) fn prepare_record(&mut self, b: usize) -> R<()> {
        if self.record(b)?.is_none() {
            for i in 0..crate::fields::record_len(b) {
                let loc = Loc::try_from(crate::fields::record_cell(b, i)).unwrap_or(0);
                self.engine_cell(loc)?;
            }
        }
        Ok(())
    }

    /// Record `b` as the run leaves it at its base level, given `defs`,
    /// the definitions to publish: the record read, or (undefined or
    /// stale) its cells' engine values, with the cells the run defines
    /// replaced. `None` with the record unchanged.
    fn record_after(
        &mut self,
        b: usize,
        defs: &[(Loc, ValueId, Locality)],
    ) -> R<Option<(ValueId, bool)>> {
        let mine: Vec<(usize, ValueId, Locality)> = defs
            .iter()
            .filter_map(|&(l, x, lc)| {
                crate::fields::record_of(l as usize)
                    .filter(|r| r.0 == b)
                    .map(|r| (r.1, x, lc))
            })
            .collect();
        if mine.is_empty() {
            return Ok(None);
        }
        let v = self.v;
        let a = self.absent();
        let rec = self.record(b)?;
        let (base, fresh): (Vec<ValueId>, bool) = if let Some(c) = rec {
            (c.to_vec(), true)
        } else {
            let mut xs = Vec::with_capacity(crate::fields::record_len(b));
            for i in 0..crate::fields::record_len(b) {
                let loc = Loc::try_from(crate::fields::record_cell(b, i)).unwrap_or(0);
                xs.push(self.engine_cell(loc)?.unwrap_or(a));
            }
            (xs, false)
        };
        let mut cells = base.clone();
        for &(i, x, _) in &mine {
            cells[i] = x;
        }
        // (debug, `PHITEX_REC_TRACE=1`: each record a run defines, the
        // cells it changes)
        if crate::exec::env_flag!("PHITEX_REC_TRACE") {
            let ch: Vec<String> = (0..cells.len())
                .filter(|&i| cells[i] != base[i])
                .map(|i| {
                    format!(
                        "{} {} -> {}",
                        crate::fields::record_cell_name(b, i),
                        v.render(base[i]),
                        v.render(cells[i])
                    )
                })
                .collect();
            eprintln!(
                "rec-trace: node {} op {:?} {} (fresh {fresh}): {}",
                self.cx.node().raw(),
                self.cx.op(),
                crate::fields::RECORDS[b],
                ch.join(", ")
            );
        }
        let global = mine.iter().any(|m| m.2 == Locality::Global);
        if fresh && cells == base {
            return Ok(None);
        }
        Ok(Some((v.list(cells), global)))
    }

    /// A continuation's open-group bindings the run republishes locally
    /// at its base level are no definitions where they equal the value
    /// reaching the run (read from the engine here: a record's cell
    /// from its record): only a binding that differs is published, so a
    /// resume re-made by an edit is no new definition of what it left
    /// as it was (per cell, and so per record).
    pub(crate) fn drop_unchanged_republished(&mut self) -> R<()> {
        for (loc, x) in self.st.cont_republished(self.v) {
            let f = self.loc_field(loc);
            if self.eng_read(f)? == Some(x) {
                self.st.drop_publ(loc);
            }
        }
        Ok(())
    }

    /// The records the run defines at its end, made before it finishes
    /// (they may read): each record a cell of which the run publishes,
    /// as the run leaves it. A global definition of a cell where a scope
    /// is open (the record holds the scope's local values of its other
    /// cells, which a global record would keep past the scope's end)
    /// makes the record stale (its readers read the cells one by one)
    /// until a definition of a cell outside every scope makes it whole.
    /// (This is the hole the engine's `define_in_scope` will close: a
    /// global definition of the record's outermost value, the scopes'
    /// local records kept.)
    pub(crate) fn ready_records(&mut self) -> R<()> {
        self.rec_out.clear();
        self.rec_ready = false;
        let defs = self.st.published(self.v, self.rec_load);
        if !defs
            .iter()
            .any(|d| crate::fields::record_of(d.0 as usize).is_some())
        {
            self.rec_ready = true;
            self.rec_sig = self.st.sig();
            return Ok(());
        }
        let v = self.v;
        // (cells the run defines in the frames it opens after itself,
        // one per group it left open, `Locality::ScopeAt`: the record is
        // stale in each such frame, its readers there read the cells;
        // the rest below is made from the other definitions)
        let defs = {
            let mut defs = defs;
            let mut stale: Vec<(usize, Locality)> = Vec::new();
            for d in &defs {
                if let (Locality::ScopeAt(_), Some(r)) =
                    (d.2, crate::fields::record_of(d.0 as usize))
                    && !stale.contains(&(r.0, d.2))
                {
                    stale.push((r.0, d.2));
                }
            }
            for (b, l) in stale {
                let f = self.ops.fields.slot_field(crate::fields::record_loc(b));
                self.rec_out.push((f, v.list(Vec::new()), l));
            }
            defs.retain(|d| !matches!(d.2, Locality::ScopeAt(_)));
            defs
        };
        let mut inside = None;
        for b in 0..crate::fields::RECORDS.len() {
            let f = self.ops.fields.slot_field(crate::fields::record_loc(b));
            // (cells the run defines in the scope it opens after itself,
            // `Locality::Scope`: the record outside is made from the
            // other definitions, below, and the scope's is the record
            // reaching the run with every definition, the scope's last)
            let of_b = |d: &(Loc, ValueId, Locality)| {
                crate::fields::record_of(d.0 as usize).is_some_and(|r| r.0 == b)
            };
            let scoped = defs.iter().any(|d| d.2 == Locality::Scope && of_b(d));
            if scoped {
                let outer: Vec<(Loc, ValueId, Locality)> = defs
                    .iter()
                    .copied()
                    .filter(|d| d.2 != Locality::Scope)
                    .collect();
                let mut all = outer.clone();
                all.extend(defs.iter().copied().filter(|d| d.2 == Locality::Scope));
                let has_outer = outer.iter().any(of_b);
                match self.record_after(b, &all)? {
                    Some((x, _)) => self.rec_out.push((f, x, Locality::Scope)),
                    // (the scope's cells as the record reaching the run,
                    // the run's own record outside differing: stale in
                    // the scope, its readers read the cells)
                    None if has_outer => {
                        self.rec_out.push((f, v.list(Vec::new()), Locality::Scope));
                    }
                    None => {}
                }
                if !has_outer {
                    continue;
                }
            }
            let defs_b: Vec<(Loc, ValueId, Locality)> = if scoped {
                defs.iter()
                    .copied()
                    .filter(|d| d.2 != Locality::Scope)
                    .collect()
            } else {
                Vec::new()
            };
            let defs = if scoped { &defs_b } else { &defs };
            let global = defs.iter().any(|d| {
                d.2 == Locality::Global
                    && crate::fields::record_of(d.0 as usize).is_some_and(|r| r.0 == b)
            });
            if global {
                let at = if let Some(i) = inside {
                    i
                } else {
                    let opens_before = matches!(
                        self.effect,
                        Effect::Scope(k) if k.before && k.opens > 0
                    );
                    let i = opens_before || self.scope_kind()?.is_some();
                    inside = Some(i);
                    i
                };
                if at {
                    self.rec_out.push((f, v.list(Vec::new()), Locality::Global));
                    continue;
                }
            }
            let Some((x, global)) = self.record_after(b, defs)? else {
                continue;
            };
            let l = if global {
                Locality::Global
            } else {
                Locality::Local
            };
            self.rec_out.push((f, x, l));
        }
        self.rec_ready = true;
        self.rec_sig = self.st.sig();
        Ok(())
    }

    #[inline(never)]
    fn load_slow(&mut self, loc: Loc) -> R<()> {
        if self.in_skip && self.rec_load {
            self.skip_first.push(loc);
        }
        // (a record's cell: read from the record, once a run)
        if let Some((b, i)) = crate::fields::record_of(loc as usize)
            && let Some(cells) = self.record(b)?
        {
            let x = cells[i];
            let x = (x != self.absent()).then_some(x);
            self.st.fill(loc, x);
            return Ok(());
        }
        let f = self.loc_field(loc);
        let r = self.cx.use_field(f);
        if r.is_err() {
            note_stop(self.op_name(), f);
        }
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
        self.set_loc(loc, Entry::raw(Some(val)), global);
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
    pub(crate) fn set_loc(&mut self, loc: Loc, val: Entry, global: bool) {
        if crate::exec::env_flag!("PHITEX_AUX_TRACE")
            && self.interp.is_none()
            && loc == self.reg_loc(0, 0)
        {
            let x = self.st.debug_entry(&val, self.v);
            let cmd = self
                .cur
                .tok
                .as_cs()
                .map_or_else(|| format!("{:?}", self.cur.tok), |n| n.name().to_owned());
            eprintln!(
                "aux-trace: count0 := {x} global={global} by node {} op {:?} at \\{cmd}",
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
        self.muts += 1;
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
        self.st.set(pl, Entry::raw(Some(old)), true, grp);
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
        self.set_loc(p as Loc, Entry::raw(Some(val)), global);
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
        if c < 256 && table < 5 && self.interp.is_none() {
            let g = self.st.cat_gen();
            if self.code_gen != g {
                *self.code_cache = [[i32::MIN; 256]; 5];
                self.code_gen = g;
            }
            let x = self.code_cache[table][c as usize];
            if x != i32::MIN {
                return Ok(x);
            }
            let x = self.code_slow(table, c)?;
            self.code_cache[table][c as usize] = x;
            return Ok(x);
        }
        self.code_slow(table, c)
    }

    #[inline(never)]
    fn code_slow(&mut self, table: usize, c: u32) -> R<i32> {
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
        if let Some(want) = crate::exec::env_str!("PHITEX_READS")
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
        let Some(x) = self.file_lazy(key)? else {
            return Ok(None);
        };
        // (a file the job writes: its text read from its units here)
        let v = self.v;
        let xs = list_of(v, x);
        if let [path, t, order] = xs[..]
            && t == ValueId::UNIT
        {
            let text = self.written_text(order)?;
            return Ok(Some(v.list(vec![path, text])));
        }
        Ok(Some(x))
    }

    /// [`Run::file`], a file the job writes as `[path, UNIT, order]`
    /// (its units' order, `ops::Files`), its text not read.
    pub(crate) fn file_lazy(&mut self, key: &str) -> R<Option<ValueId>> {
        let r = self.file_inner(key);
        if r.is_err() {
            self.trace_stop(&format!("file {key}"));
        }
        r
    }

    /// File `key` as a document's `\input` reads it: its units' order
    /// (`units:<key>`, `api::file_units`, `[path, UNIT, order]` as a
    /// written file's) when the file is cut into units, so this run reads
    /// no unit's text; `None` (the reader reads the text) for a file the
    /// job writes or one not cut.
    pub(crate) fn read_units(&mut self, key: &str) -> R<Option<ValueId>> {
        let v = self.v;
        if self.get(&format!("wopen:{key}"))?.is_some() {
            return Ok(None);
        }
        let w = self.keyed(crate::fields::Fam::Wfile, key);
        let k = self.keyed(crate::fields::Fam::Wkeys, key);
        if self.cx.is_loopback(k) || self.cx.is_loopback(w) {
            return Ok(None);
        }
        // (bound as [`Run::file`] binds them: a run that writes the file
        // later declares its loop-back and this runs again)
        let _ = self.cx.use_field(k)?;
        let _ = self.cx.use_field(w)?;
        if crate::exec::env_flag!("PHITEX_TMP_FTRACE") {
            eprintln!("TMPF units {key} node {}", self.cx.node().raw());
        }
        let Some(x) = self.cx.use_env(v.field(&format!("units:{key}")))? else {
            return Ok(None);
        };
        let Some(&order) = list_of(v, x).get(2) else {
            return Ok(None);
        };
        if list_of(v, order).first().is_none_or(|&s| int_of(v, s) == 0) {
            return Ok(None);
        }
        self.muts += 1;
        self.fread.push((key.to_owned(), order.raw()));
        Ok(Some(x))
    }

    /// A unit's text and its offset in its file: a written file's
    /// (`wunit:`, a loop-back `ops::Files` publishes, its text) or a
    /// file's cut by the client (`unit:`, the environment's `[text,
    /// offset]`, `api::file_units`).
    pub(crate) fn unit_text(&mut self, key: ValueId) -> R<(String, usize)> {
        let v = self.v;
        let name = str_of(v, key);
        let f = v.field(&name);
        if name.starts_with("unit:") {
            if crate::exec::env_flag!("PHITEX_TMP_FTRACE") {
                eprintln!("TMPF unit {name} node {}", self.cx.node().raw());
            }
            let Some(x) = self.cx.use_env(f)? else {
                return Ok((String::new(), 0));
            };
            let xs = list_of(v, x);
            let text = xs.first().map(|&t| str_of(v, t)).unwrap_or_default();
            let at = xs
                .get(1)
                .map_or(0, |&o| usize::try_from(int_of(v, o)).unwrap_or(0));
            return Ok((text, at));
        }
        Ok((
            self.cx
                .use_field(f)?
                .map(|x| str_of(v, x))
                .unwrap_or_default(),
            0,
        ))
    }

    /// A written file's text from its order (`ops::Files`): its units
    /// read, each a field of its own.
    pub(crate) fn written_text(&mut self, order: ValueId) -> R<ValueId> {
        let v = self.v;
        let entries = list_of(v, order)
            .get(1)
            .map(|&l| list_of(v, l))
            .unwrap_or_default();
        let mut text = String::new();
        for e in entries {
            let Some(&k) = list_of(v, e).first() else {
                continue;
            };
            text.push_str(&self.unit_text(k)?.0);
        }
        Ok(v.str(&text))
    }

    /// A written file on the input stack read by this run's own lines:
    /// its text from its units ([`FileRest::written`]).
    pub(crate) fn load_written(&mut self, l: crate::input::Level) -> R<()> {
        let v = self.v;
        let Some(units) = self.input.file(l).units.clone() else {
            return Ok(());
        };
        let mut text = String::new();
        for &(k, _) in units.iter() {
            text.push_str(&self.unit_text(k)?.0);
        }
        let id = v.str(&text);
        self.input.file_mut(l).set_text(v, id);
        Ok(())
    }

    /// A piece's arguments with its units' frames (`["w", field, file,
    /// starts]`, [`Run::doc_file_regions`]) read: each the unit's text as
    /// a document slice's frames of its file. Others as they are.
    pub(crate) fn resolve_units(&mut self, args: &[ValueId]) -> R<Vec<ValueId>> {
        let v = self.v;
        let Some(&l) = args.first() else {
            return Ok(args.to_vec());
        };
        let fs = list_of(v, l);
        let is_w = |f: ValueId| list_of(v, f).first().is_some_and(|&t| str_of(v, t) == "w");
        if !fs.iter().any(|&f| is_w(f)) {
            return Ok(args.to_vec());
        }
        let mut frames = Vec::with_capacity(fs.len());
        for f in fs {
            if !is_w(f) {
                frames.push(f);
                continue;
            }
            let xs = list_of(v, f);
            let (text, at) = self.unit_text(xs.get(1).copied().unwrap_or(ValueId::UNIT))?;
            let name = xs.get(2).copied().unwrap_or(ValueId::UNIT);
            let starts = xs.get(3).is_some_and(|&s| int_of(v, s) != 0);
            let a = slice_args_in_file(v, &text, starts, name, at);
            frames.extend(a.first().map(|&x| list_of(v, x)).unwrap_or_default());
        }
        let mut out = args.to_vec();
        out[0] = v.list(frames);
        Ok(out)
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
        // (neither declared yet: both bound, so a run that writes the
        // file later in the evaluation, declaring its loop-back,
        // `Run::written_decl`, runs this again on it)
        if !self.cx.is_loopback(k) && !self.cx.is_loopback(w) {
            let _ = self.cx.use_field(k)?;
            let _ = self.cx.use_field(w)?;
        }
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
            // (its units' order, `ops::Files`; none: absent)
            let x = self.cx.use_field(w)?;
            if crate::exec::env_flag!("PHITEX_AUX_TRACE") {
                eprintln!(
                    "aux-trace: read wfile {key}: {:?} units",
                    x.map(|x| list_of(v, x).get(1).map_or(0, |&l| list_of(v, l).len()))
                );
            }
            x.filter(|&x| !list_of(v, x).is_empty())
                .map(|o| v.list(vec![v.str(&format!("./{key}")), ValueId::UNIT, o]))
        } else if self.cx.is_loopback(f) {
            self.get_f(f)?
        } else {
            if crate::exec::env_flag!("PHITEX_TMP_FTRACE") {
                eprintln!(
                    "TMPF file {key} node {} op {:?}",
                    self.cx.node().raw(),
                    self.cx.op()
                );
            }
            self.cx.use_env(f)?
        };
        let x = x.filter(|&x| !crate::val::list_of(v, x).is_empty());
        // (the text's hash: its value's id, a content hash, so a font's
        // or map's bytes are not copied and hashed again at every read;
        // a written file's, its units' order's)
        if let Some(t) = x.and_then(|x| {
            let xs = crate::val::list_of(v, x);
            xs.get(1)
                .copied()
                .filter(|&t| t != ValueId::UNIT)
                .or_else(|| xs.get(2).copied())
        }) {
            self.muts += 1;
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
    /// The entry also goes to its file's stream accumulator
    /// ([`stream_bucket`], read by that file's node alone, `ops::Files`)
    /// and the file's name to the names written ([`WNAMES`]).
    pub(crate) fn contribute_stream(&mut self, e: ValueId) {
        let v = self.v;
        self.contribute("streams", v.seq_unit(e));
        let xs = list_of(v, e);
        let entry = match xs.first() {
            Some(&j) if v.as_int(j).is_some() => xs.get(1).map(|&y| list_of(v, y)),
            _ => Some(xs),
        };
        if let Some(&n) = entry.as_ref().and_then(|x| x.first()) {
            let f = stream_bucket(v, &str_of(v, n));
            self.muts += 1;
            self.contribs.push((f, v.seq_unit(e)));
            self.contribute(WNAMES, v.seq_unit(n));
        }
        let at = v.list(vec![v.int(crate::nodes::T_WRITE_AT), e]);
        self.contribute("items", v.seq_unit(v.list(vec![v.int(0), at])));
    }

    /// The page parameters in force, as a `T_PAGE_PARAMS` marker on the
    /// main list (page.rs `breaks`, §980).
    pub(crate) fn put_page_params(&mut self) {
        let v = self.v;
        let marker = self.page_params_marker().unwrap_or_else(|_| {
            v.list(vec![
                v.int(crate::nodes::T_PAGE_PARAMS),
                v.int(0),
                v.int(0),
                crate::val::glue_value(v, &phitex_typeset::glue::Glue::ZERO),
                v.int(0),
            ])
        });
        self.contribute("items", v.seq_unit(v.list(vec![v.int(0), marker])));
    }

    /// The page parameters in force (§980), as a `T_PAGE_PARAMS` marker:
    /// `[tag, vsize, maxdepth, topskip, holdinginserts]`.
    pub(crate) fn page_params_marker(&mut self) -> R<ValueId> {
        let v = self.v;
        let vsize = self.ipar(Par::vsize)?;
        let maxdepth = self.ipar(Par::maxdepth)?;
        let topskip = self.gpar(Par::topskip)?;
        let holding = self.ipar(Par::holdinginserts)?;
        Ok(v.list(vec![
            v.int(crate::nodes::T_PAGE_PARAMS),
            v.int(i64::from(vsize)),
            v.int(i64::from(maxdepth)),
            crate::val::glue_value(v, &topskip),
            v.int(i64::from(holding)),
        ]))
    }

    /// The page parameters reaching the fold, as a `T_PAGE_PARAMS`
    /// marker without values (`[tag]`): the fold reads them where it is
    /// ([`Run::resolve_page_params`]), so a piece that leaves them alone
    /// does not read them (an output routine setting one would refute
    /// every piece after it that had run ahead).
    pub(crate) fn put_page_params_reaching(&mut self) {
        let v = self.v;
        let marker = v.list(vec![v.int(crate::nodes::T_PAGE_PARAMS)]);
        self.contribute("items", v.seq_unit(v.list(vec![v.int(0), marker])));
    }

    /// The `[tag]` page parameter markers of `raw`
    /// ([`Run::put_page_params_reaching`]) and the `[tag, n]` insertion
    /// class markers (`Run::finish_candidate`) given the values in force
    /// in this run.
    pub(crate) fn resolve_page_params(&mut self, raw: &mut [ValueId]) -> R<()> {
        let v = self.v;
        let mut full = None;
        let mut classes: Vec<(u16, ValueId)> = Vec::new();
        for x in raw.iter_mut() {
            let tag = crate::nodes::tag(v, *x);
            if tag == crate::nodes::T_PAGE_PARAMS && crate::val::list_of(v, *x).len() == 1 {
                let m = if let Some(m) = full {
                    m
                } else {
                    let m = self.page_params_marker()?;
                    full = Some(m);
                    m
                };
                *x = m;
            } else if tag == crate::nodes::T_INS_PARAMS {
                let xs = crate::val::list_of(v, *x);
                if xs.len() == 2 {
                    let n = u16::try_from(int_of(v, xs[1])).unwrap_or(0);
                    let m = if let Some(&(_, m)) = classes.iter().find(|c| c.0 == n) {
                        m
                    } else {
                        let m = self.ins_params_marker(n)?;
                        classes.push((n, m));
                        m
                    };
                    *x = m;
                }
            }
        }
        Ok(())
    }

    /// Whether file `name` is one the job writes and reads back, with its
    /// loop-backs declared.
    pub(crate) fn written_known(&mut self, name: &str) -> bool {
        let w = self.keyed(crate::fields::Fam::Wfile, name);
        let k = self.keyed(crate::fields::Fam::Wkeys, name);
        self.cx.is_loopback(w) || self.cx.is_loopback(k)
    }

    /// Whether `name`'s `wkeys:` loop-back is declared (a file whose
    /// read-back only defines, [`Run::read_back`]). Not yet: for such a
    /// file the field is bound, so this run goes again once the opening
    /// run's declaration ([`Run::written_decl`]) is in effect.
    pub(crate) fn keys_loopback(&mut self, name: &str) -> R<bool> {
        let k = self.keyed(crate::fields::Fam::Wkeys, name);
        if self.cx.is_loopback(k) {
            return Ok(true);
        }
        if crate::api::defines_only(name) {
            let _ = self.cx.use_field(k)?;
        }
        Ok(false)
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
            // (a key whose loop-back the files node has not declared
            // yet: the field bound, so this runs again when it does)
            if !self.cx.is_loopback(wd) {
                let _ = self.cx.use_field(wd)?;
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
    /// definitions are restored): what it writes to other files (LaTeX's
    /// `\@writefile{toc}{...}`) is written where the run reads the text
    /// back, as `\enddocument` does.
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
        // (what the line writes to other files is not kept: TeX writes
        // it as `\enddocument` reads the file back, which this run does
        // where it reads the text it wrote, `start_input`)
        self.muts += 1;
        self.contribs.truncate(nc);
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

    /// File `name` is opened for writing: where the build keeps the
    /// files the job writes in memory (`loopfiles`, a watch's fixpoint),
    /// its loop-back is declared here, from the run (spec-redesign §1
    /// `files(round)`): `wkeys:<name>` for a read-back that only defines,
    /// else `wfile:<name>`, none until the files node publishes one. A
    /// reader that read the file before binds the field
    /// ([`Run::file`]) and runs again within the evaluation; no
    /// evaluation waits on the client for it.
    pub(crate) fn written_decl(&mut self, name: &str) -> R<()> {
        if self.written_known(name) {
            return Ok(());
        }
        let v = self.v;
        // (read as the files node reads it: no wait where a build has
        // none, a prelude's)
        if self
            .cx
            .use_field(v.field("loopfiles"))?
            .is_none_or(|x| x == ValueId::UNIT)
        {
            return Ok(());
        }
        let fam = if crate::api::defines_only(name) {
            crate::fields::Fam::Wkeys
        } else {
            crate::fields::Fam::Wfile
        };
        let f = self.keyed(fam, name);
        self.cx.declare_loopback(f, v.list(Vec::new()));
        Ok(())
    }

    pub(crate) fn get(&mut self, name: &str) -> R<Option<ValueId>> {
        let f = self.ops.fields.get_at(self.v, self.cx.values_epoch(), name);
        self.read_f(f)
    }

    pub(crate) fn int32(&mut self, name: &str) -> R<i32> {
        Ok(crate::val::i32_of(self.v, self.get(name)?))
    }

    /// The location of font `f`'s cell `c` (spec-runstate §2): a fixed
    /// one ([`crate::fields::font_loc`]), or for a font past them, the
    /// spill's by its field.
    #[inline]
    pub(crate) fn font_loc(&mut self, f: impl TryInto<i64>, c: crate::fields::FontCell) -> Loc {
        let f = f.try_into().unwrap_or(-1);
        match crate::fields::font_loc(f, c) {
            Some(l) => Loc::try_from(l).unwrap_or(0),
            None => self.font_loc_slow(f, c),
        }
    }

    #[cold]
    fn font_loc_slow(&mut self, f: i64, c: crate::fields::FontCell) -> Loc {
        let name = crate::fields::font_cell_name(f, c);
        let fld = self
            .ops
            .fields
            .get_at(self.v, self.cx.values_epoch(), &name);
        self.loc_of(fld)
    }

    /// Font `f`'s cell `c` ([`Run::font_loc`]).
    pub(crate) fn font_get(
        &mut self,
        f: impl TryInto<i64>,
        c: crate::fields::FontCell,
    ) -> R<Option<ValueId>> {
        let l = self.font_loc(f, c);
        self.get_loc(l)
    }

    /// Font `f`'s cell `c` as an integer ([`Run::font_loc`]).
    pub(crate) fn font_int(&mut self, f: impl TryInto<i64>, c: crate::fields::FontCell) -> R<i32> {
        let l = self.font_loc(f, c);
        self.int_loc(l)
    }

    /// Defines font `f`'s cell `c` ([`Run::font_loc`]).
    pub(crate) fn font_set(
        &mut self,
        f: impl TryInto<i64>,
        c: crate::fields::FontCell,
        val: ValueId,
        global: bool,
    ) {
        let l = self.font_loc(f, c);
        self.set_loc(l, Entry::raw(Some(val)), global);
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
        self.set_loc(l, Entry::raw(Some(val)), global);
    }

    /// The fields the run publishes: its definitions at the base level
    /// but a local one equal to the value it read (the same binding),
    /// and the records of the cells among them.
    pub(crate) fn published_fields(&mut self) -> Vec<Field> {
        let locs: Vec<Loc> = self
            .st
            .published(self.v, self.rec_load)
            .into_iter()
            .map(|x| x.0)
            .collect();
        let mut fs: Vec<Field> = locs.into_iter().map(|l| self.loc_field(l)).collect();
        self.ops.fields.add_record_fields(&mut fs);
        fs
    }

    /// The records the run defines ([`Run::ready_records`], made before
    /// the run finishes). A run that publishes a cell and could not make
    /// its record (no chance to read before finishing) makes it stale:
    /// its readers read the cells one by one, as they are.
    fn records_at_finish(&mut self) -> Vec<(Field, ValueId, Locality)> {
        if self.rec_ready && self.rec_sig == self.st.sig() {
            return std::mem::take(&mut self.rec_out);
        }
        let v = self.v;
        let mut out = Vec::new();
        let defs = self.st.published(v, self.rec_load);
        for b in 0..crate::fields::RECORDS.len() {
            let mut mine: Vec<Locality> = defs
                .iter()
                .filter(|d| crate::fields::record_of(d.0 as usize).is_some_and(|r| r.0 == b))
                .map(|d| d.2)
                .collect();
            // (stale in each frame the run opens with cells of it)
            let f = self.ops.fields.slot_field(crate::fields::record_loc(b));
            let mut frames: Vec<Locality> = mine
                .iter()
                .copied()
                .filter(|l| matches!(l, Locality::ScopeAt(_)))
                .collect();
            frames.sort_unstable();
            frames.dedup();
            for l in frames {
                out.push((f, v.list(Vec::new()), l));
            }
            mine.retain(|l| !matches!(l, Locality::ScopeAt(_)));
            if mine.is_empty() {
                continue;
            }
            let l = if mine.contains(&Locality::Global) {
                Locality::Global
            } else if mine.iter().all(|&l| l == Locality::Scope) {
                Locality::Scope
            } else {
                Locality::Local
            };
            out.push((f, v.list(Vec::new()), l));
        }
        out
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
        self.muts += 1;
        self.contribs.push((f, val));
    }

    pub(crate) fn kid(&mut self, op: OpId, args: Vec<ValueId>) {
        self.muts += 1;
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

    /// The current token's macro (spec-final §2.1: `macro_call` reads
    /// the slot's macro by its handle, never a `Mean`): one slot access
    /// when it is loaded with one; else read and decoded once. `None`
    /// when the meaning is no macro, or a halt was raised.
    #[inline]
    pub(crate) fn cur_macro(&mut self) -> Option<Rc<crate::val::Macro>> {
        let t = self.cur.tok;
        if !t.is_cs_or_active() || self.cur.cmd == cmd::RELAX {
            return None;
        }
        if let Some(l) = crate::fields::mean_loc(t).and_then(|l| Loc::try_from(l).ok())
            && let Some(m) = self.st.macro_at(l)
        {
            return Some(m);
        }
        self.cur_macro_slow(t)
    }

    #[inline(never)]
    fn cur_macro_slow(&mut self, t: Tok) -> Option<Rc<crate::val::Macro>> {
        let l = self.mean_loc(t)?;
        if !self.st.is_loaded(l)
            && let Err(h) = self.load_slow(l)
        {
            self.raise(h);
            return None;
        }
        match self.st.mean_of(l, self.v) {
            Mean::Macro(m) => Some(m),
            _ => None,
        }
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
            let e = self.st.mean_entry(m);
            self.set_loc(l, e, global);
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
        if self.class_reads
            && let Some(c) = self.class_lookup(tok)?
        {
            self.cur = Cur {
                tok,
                cmd: c,
                chr: 0,
            };
            return Ok(());
        }
        self.set_cur_cs_slow(tok)
    }

    /// A class read (spec-redesign §8): the command code of `tok`'s
    /// meaning when only its class is needed (an unexpandable meaning
    /// or a `\protected` macro, which a storing lookup keeps as its
    /// token), read through the engine's projection
    /// ([`crate::val::class_projection`]): a redefinition within the
    /// class (another `\chardef` value, another protected body) leaves
    /// this run standing. `None` where the meaning is needed in full
    /// (expandable, undefined, this run's own binding).
    #[inline(never)]
    fn class_lookup(&mut self, tok: Tok) -> R<Option<u8>> {
        let Some(l) = self.mean_loc(tok) else {
            return Ok(None);
        };
        if self.st.is_loaded(l) {
            return Ok(None);
        }
        if let Some(&c) = self.class_memo.get(&l) {
            return Ok(Some(c));
        }
        let f = self.loc_field(l);
        let Some(x) = self.cx.use_field_proj(f, self.ops.class_proj)? else {
            return Ok(None);
        };
        let Ok(c) = u8::try_from(int_of(self.v, x)) else {
            return Ok(None);
        };
        self.class_memo.insert(l, c);
        Ok(Some(c))
    }

    /// A NUL read from a document line: the mark of a comment of the
    /// canonical text (`api::canonical_text`), read as text where `%` was
    /// no comment (a verbatim's): the comment put back in the line in
    /// its place from `doc:marks` (the line's, by its number, §304's
    /// `line`), and the line read on from there, as TeX reads the
    /// document's own text. Whether it was one.
    #[cold]
    pub(crate) fn splice_mark(&mut self) -> R<bool> {
        let v = self.v;
        let doc = self.input.top_line().is_some_and(|s| {
            s.origin
                .is_some_and(|(n, _)| str_of(v, n) == crate::api::DOC_NAME)
        });
        if !doc {
            return Ok(false);
        }
        let Some(marks) = self.cx.use_env(v.field(crate::api::DOC_MARKS))? else {
            return Ok(false);
        };
        let n = i64::from(self.input_line_no()?);
        let Some(text) = list_of(v, marks).into_iter().find_map(|e| {
            let xs = list_of(v, e);
            (xs.first().map(|&x| int_of(v, x)) == Some(n))
                .then(|| xs.get(1).map(|&t| str_of(v, t)))
                .flatten()
        }) else {
            return Ok(false);
        };
        let Some(s) = self.input.top_line_mut() else {
            return Ok(false);
        };
        let i = s.loc - 1;
        s.buf.splice(i..=i, text.chars().map(u32::from));
        s.loc = i;
        Ok(true)
    }

    /// The fields this run read by class only ([`Run::class_lookup`]):
    /// for the cut checks, which take them as read (a routine's global
    /// definition of one is taken to change what the run saw).
    pub(crate) fn class_read_fields(&self) -> Vec<Field> {
        let mut ls: Vec<Loc> = self.class_memo.keys().copied().collect();
        ls.sort_unstable();
        ls.into_iter().filter_map(|l| self.st.field(l)).collect()
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
        let tag_f = tag("f");
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
            if let Some(f) = crate::pages::line_frame(v, s) {
                out.push(vec![v.list(vec![f]), v.int(kind)]);
            }
        };
        let file_left = |f: &FileRest, out: &mut Vec<Vec<ValueId>>| {
            if f.has_lines() && self.doc && self.doc_file(f) {
                out.extend(self.doc_file_regions(f));
            } else if let Some(u) = units_frame(v, f) {
                // (a file read as units, not read here: the one node its
                // rest is reads it as a file, its units loaded when it
                // reads a line, `Run::load_written`)
                out.push(vec![v.list(vec![u]), v.int(0)]);
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
        // (a file the job writes, not read here: one piece per unit, its
        // arguments the unit's field (its text's hash) alone, as a
        // document slice's are its text; the piece reads its unit)
        if f.lazy()
            && let Some(units) = &f.units
        {
            return units
                .iter()
                .map(|&(k, starts)| {
                    let w = v.list(vec![v.str("w"), k, f.name, v.int(i64::from(starts))]);
                    vec![v.list(vec![w]), v.int(2)]
                })
                .collect();
        }
        let text = str_of(v, f.id);
        let defs = self
            .ops
            .defs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let t = crate::hints::text_hints(&text, phitex_source::pass::FileKind::Document, &defs);
        crate::hints::register_text(&self.ops.hints, f.name, &t);
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

    /// The scope kind the node runs in: the engine's, read once a run
    /// (a stop reads it again).
    pub(crate) fn scope_kind(&mut self) -> R<Option<ValueId>> {
        if let Some(k) = self.scope_kind_memo {
            return Ok(k);
        }
        let r = self.cx.scope_kind();
        if r.is_err() {
            self.trace_stop("scope kind");
        }
        let k = r?;
        self.scope_kind_memo = Some(k);
        Ok(k)
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
                    if self.can_leave_scope() {
                        self.leave_scope()?;
                    }
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
            }
            // a document file reached with conditionals of the run's own
            // open and no other level: the run stops there, its
            // conditionals left as a field the nodes after it read where
            // they end them ([`COND_STACK`])
            if self.doc && self.nest.is_empty() && !self.conds.is_empty() && self.next_is_doc_file()
            {
                self.cond_stop = true;
                return Ok(());
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
            if crate::exec::env_flag!("PHITEX_GIVEBACK_TRACE") && self.doc && self.open_span() {
                eprintln!(
                    "readon: node {} pulled {}: nest {:?}, conds {}, effect {:?}, baselocal {}",
                    self.cx.node().raw(),
                    self.pulled,
                    self.nest
                        .iter()
                        .map(|l| (l.mode, l.group, l.owns))
                        .collect::<Vec<_>>(),
                    self.conds.len(),
                    self.effect,
                    self.st.has_base_local_def(self.v)
                );
            }
            if self.open_span() && !self.eof && self.pull()? {
                continue;
            }
            if !self.doc && !self.nest.is_empty() {
                // a format or package text ended inside a group: the
                // groups end with it (nothing after reads them), the
                // node reports the error
                self.diag(true, "format: group left open at end");
                self.muts += 1;
                let nest = std::mem::take(&mut self.nest);
                self.abandon_levels(&nest);
                for l in nest {
                    if l.owns {
                        for x in crate::nest::into_values(self.v, l.list) {
                            self.append(0, x);
                        }
                    }
                }
            }
            return Ok(());
        }
    }

    /// Whether the run's own open levels can be left to the engine as
    /// nested scopes (`Run::leave_scope`): a text piece or a resume, at
    /// least one level open and every one a group sharing the list below
    /// it (a list of the run's own cannot be handed over), no conditional
    /// open, no scope effect yet, and a text that does not say it opens
    /// none. Each group is a frame; a field the run defines at its level
    /// and a group inside it has an entry for each
    /// (`Ctx::define_in_scope`, `Ctx::define_in_scope_at`).
    pub(crate) fn can_leave_scope(&self) -> bool {
        let op = self.cx.op();
        (op == self.ops.exec || op == self.ops.resume)
            && !self.nest.is_empty()
            && self.nest.len() < usize::from(u8::MAX - 2)
            && self.conds.is_empty()
            && self.effect == Effect::None
            && self.nest.iter().all(|l| !l.owns && l.group != 0)
            && !crate::hints::effect_none(&self.ops.hints, self.v, &self.cx.args())
    }

    /// Whether the run closed the engine scope it ran in, with nothing
    /// of its own open but conditionals: it ends after that command
    /// (`Effect::Close`, its reads and definitions inside the scope), and
    /// what it has left of its input runs outside it as the following
    /// nodes, the conditionals carried ([`Run::leftover`]). (Any run: a
    /// run going on past the close would read on with its own bindings
    /// of the closed scope, §282 not done.)
    fn closed_scope(&self) -> bool {
        self.effect == Effect::Close && self.nest.is_empty()
    }

    /// The conditionals a node before this one left open
    /// ([`COND_STACK`]), read where this run ends one it did not begin
    /// (its own stack empty): below its own, as TeX's one stack has them.
    /// A document's run only.
    pub(crate) fn load_cond_stack(&mut self) -> R<()> {
        if self.cond_loaded || !self.doc || !self.conds.is_empty() {
            return Ok(());
        }
        let v = self.v;
        let x = self.cx.use_field(v.field(COND_STACK))?;
        self.cond_loaded = true;
        self.muts += 1;
        for e in x.map(|x| list_of(v, x)).unwrap_or_default() {
            if let [l, c] = list_of(v, e)[..] {
                self.conds.push(u8::try_from(int_of(v, l)).unwrap_or(0));
                self.if_codes.push(i32::try_from(int_of(v, c)).unwrap_or(0));
            }
        }
        Ok(())
    }

    /// The conditionals left open ([`COND_STACK`]): by a run that stopped
    /// at a document file inside them, or one that read the stack before
    /// it (what it ended taken off; none where it closed its scope, the
    /// rest carried to its first child, [`Run::leftover`]).
    fn publish_cond_stack(&mut self) {
        if !self.cond_stop && !self.cond_loaded {
            return;
        }
        let v = self.v;
        let x = if self.closed_scope() {
            v.list(Vec::new())
        } else {
            v.list(
                self.conds
                    .iter()
                    .zip(&self.if_codes)
                    .map(|(&l, &c)| v.list(vec![v.int(i64::from(l)), v.int(i64::from(c))]))
                    .collect(),
            )
        };
        self.cx
            .define_with(v.field(COND_STACK), x, phitex_defuse::Locality::Global);
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
        self.publish_cond_stack();
        // (a command node's, empty too: what its text declares,
        // `Exec::static_contribs`)
        if !self.doc_lines.is_empty() || self.cx.op() == self.ops.exec {
            let m = lines_value(self.v, &self.doc_lines);
            let f = self
                .ops
                .fields
                .slot_field(crate::fields::cell::LINES as usize);
            self.contribs.push((f, m));
        }
        // a page parameter this run set and no item of it carried
        if self.st.take_page_params() {
            self.put_page_params();
        }
        let op = self.cx.op();
        let t_pub = std::time::Instant::now();
        let mut published: Vec<(Field, ValueId, Locality)> = self
            .st
            .published(self.v, self.rec_load)
            .into_iter()
            .map(|(loc, x, l)| (self.loc_field(loc), x, l))
            .collect();
        let recs = self.records_at_finish();
        published.extend(recs.iter().copied());
        // (debug, `PHITEX_PUB_TRACE=<a>|<b>…`: the definitions of a run whose
        // first text segment contains one of the texts)
        if let Some(pat) = crate::exec::env_str!("PHITEX_PUB_TRACE") {
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
            let mut names: Vec<ValueId> = locs.into_iter().map(|l| self.loc_field(l).0).collect();
            names.extend(recs.iter().map(|r| r.0.0));
            let l = self.v.list(names);
            let s = self.v.seq_unit(l);
            let defs = self
                .ops
                .fields
                .get_at(self.v, self.cx.values_epoch(), "defs");
            self.muts += 1;
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
        // (a run cut at a candidate: its input left is its continuation,
        // the resume piece's, `crate::pages`)
        let leftover = if self.cut.is_some() {
            Vec::new()
        } else {
            self.leftover()
        };
        for args in leftover {
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
        let n = self.claim_count();
        if crate::exec::env_flag!("PHITEX_DEBUG_CLAIMS") {
            eprintln!(
                "claim: node {} claims {n} (pulled {})",
                self.cx.node().raw(),
                self.pulled
            );
        }
        if crate::exec::env_flag!("PHITEX_TEX_TRACE") {
            eprintln!(
                "  end {}: claim {n} (pulled {}, rest {}), cut {:?}",
                self.cx.node().raw(),
                self.pulled,
                self.claim_rest,
                self.cut
            );
        }
        // (nothing to claim, no effect: no call, the engine's default)
        if n != 0 {
            self.cx.claim(n);
        }
        if crate::exec::env_flag!("PHITEX_TEX_TRACE")
            && self.effect != Effect::None
            && crate::hints::effect_none(&self.ops.hints, self.v, &self.cx.args())
        {
            eprintln!(
                "  effect {:?} of {} against its hint's none (refuted)",
                self.effect,
                self.cx.node().raw()
            );
        }
        if self.effect != Effect::None {
            self.cx.set_effect(self.effect);
        }
        ValueId::UNIT
    }

    /// The following siblings this run takes, once its input is read
    /// out: those it pulled, and with `\end` inside groups every text
    /// after them. Declared at the commit, and before the page fold's
    /// read of the page state (`crate::pages`), so that a run parked
    /// there holds no sibling it does not take.
    pub(crate) fn claim_count(&self) -> usize {
        let mut n = self.pulled;
        if self.claim_rest {
            while matches!(self.cx.following(n), Some((op, _)) if op == self.ops.exec) {
                n += 1;
            }
        }
        n
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
        let mut files: Vec<(String, u64)> = Vec::new();
        for (n, h) in self.fread.clone() {
            if !self.written_known(&n) {
                files.push((n, h));
            }
        }
        for n in self.fmiss.clone() {
            if !self.written_known(&n) {
                files.push((n, 0));
            }
        }
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
        // ("took no sibling" only when it pulled none: a load that read
        // a blank sibling (an operand's look-ahead) claims it, and its
        // record's hint leaves that claim to the run, as the first
        // build did, so the graph does not depend on the record's
        // presence)
        let flags = i64::from(self.effect == Effect::None) | (i64::from(self.pulled == 0) << 1);
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
        ValueId::UNIT
    }
}

/// Spent token lists by length, for the next macro body of that length
/// The segments of an `Exec` node's arguments.
/// A file read as units whose text is not read yet (`f.lazy()`), as one
/// frame `["u", name, [sliceable, [[unit field, starts], ...]]]`: decoded
/// as that file again ([`FileRest::written`]), so a run reaching it reads
/// it as a file (a document's run stops there, its units the region
/// children, [`Run::leftover`]), never as its units' lines inline.
pub(crate) fn units_frame(v: &Values, f: &FileRest) -> Option<ValueId> {
    if !f.lazy() {
        return None;
    }
    let units = f.units.as_ref()?;
    let es = units
        .iter()
        .map(|&(k, starts)| v.list(vec![k, v.int(i64::from(starts))]))
        .collect();
    let order = v.list(vec![v.int(i64::from(f.sliceable)), v.list(es)]);
    Some(v.list(vec![v.str("u"), f.name, order]))
}

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
            Some("u") => {
                // (a file read as units, not read yet, [`units_frame`])
                if let (Some(&name), Some(&order)) = (xs.get(1), xs.get(2)) {
                    out = Some(Seg::File(FileRest::written(v, order, name)));
                }
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
    list: Vec<crate::nest::Item>,
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
    pull_box: Vec<bool>,
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
    cond_loaded: bool,
    cond_stop: bool,
    astate: Option<i64>,
    aopen: i64,
    lines_entered: i64,
    lines_read: u64,
    doc_lines: Vec<(ValueId, i64)>,
    eof: bool,
    radix: i32,
    cur_order: phitex_typeset::glue::Order,
    pdf_occ: usize,
    muts: u64,
    read_back_done: bool,
    read_back_at: Option<crate::input::Level>,
    cand: u32,
    cand_marks: Vec<crate::pages::CandMark>,
    ins_pending: Vec<u16>,
    fin: Option<crate::pages::FinCtx>,
}

impl Run<'_, '_> {
    /// The run's state for [`phitex_defuse::Ctx::park`] after a stop:
    /// `None` if the stopped command cannot be rewound.
    #[allow(clippy::too_many_lines, reason = "one state field a line")]
    pub(crate) fn park(&mut self) -> Option<Parked> {
        if !self.rewind() {
            return None;
        }
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
            pull_box: self.pull_box.clone(),
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
            cond_loaded: self.cond_loaded,
            cond_stop: self.cond_stop,
            astate: self.astate,
            aopen: self.aopen,
            lines_entered: self.lines_entered,
            lines_read: self.lines_read,
            doc_lines: self.doc_lines.clone(),
            eof: self.eof,
            radix: self.radix,
            cur_order: self.cur_order,
            pdf_occ: self.pdf_occ,
            muts: self.muts,
            read_back_done: self.read_back_done,
            read_back_at: self.read_back_at,
            cand: self.cand,
            cand_marks: std::mem::take(&mut self.cand_marks),
            ins_pending: std::mem::take(&mut self.ins_pending),
            fin: self.fin.take(),
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
        r.pull_box = p.pull_box;
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
        r.cond_loaded = p.cond_loaded;
        r.cond_stop = p.cond_stop;
        r.astate = p.astate;
        r.aopen = p.aopen;
        r.lines_entered = p.lines_entered;
        r.lines_read = p.lines_read;
        r.doc_lines.clone_from(&p.doc_lines);
        r.eof = p.eof;
        r.radix = p.radix;
        r.cur_order = p.cur_order;
        r.pdf_occ = p.pdf_occ;
        r.muts = p.muts;
        r.read_back_done = p.read_back_done;
        r.read_back_at = p.read_back_at;
        r.cand = p.cand;
        r.cand_marks = p.cand_marks;
        r.ins_pending = p.ins_pending;
        r.fin = p.fin;
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
