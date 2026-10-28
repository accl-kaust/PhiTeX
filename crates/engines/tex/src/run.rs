//! One run of one node (a paragraph region, a loading command, a whole
//! format): its input (the node's own text, the files it reads inline,
//! the siblings it claims while a span is open), `get_next` (§341–§365)
//! over it, and the definitions, contributions and children it makes,
//! buffered until the run is done.
//!
//! Everything here is local to one run: a small sequential scanner over
//! the node's own text. What TeX keeps between commands is read through
//! fields (`Run::get`), the scope kind and contributions.

use crate::exec::LockWait as _;
use std::rc::Rc;

use crate::input::{Line, MID_LINE, NEW_LINE, Seg};

use phitex_defuse::{Ctx, Effect, Field, Locality, OpId, Stop, Value, ValueId, Values};

use crate::fields::Par;
use crate::state::{Entry, Loc};
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
    /// The run read or made a number that counts the objects of the
    /// pages shipped before it after passing candidate `k`, where a page
    /// can fire (`crate::pages`, [`Run::order_after_pages`]): it goes
    /// again from its start and ends at `k`, its rest a resume after the
    /// pages firing there.
    Recut(u32),
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
/// The errors the log has (`! ...` lines: an error, an `\errmessage`),
/// counted: a sum the build reads without the log
/// (`crate::Build::error_lines`).
pub(crate) const LOG_ERRORS: &str = "log:errors";

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

/// A page's finisher class ([`Ops::fin_class`]): its hint, its penalty
/// class, its checkpoint index.
pub(crate) type FinClass = (phitex_defuse::ValueId, i32, i64);

/// The operations, as registered.
#[derive(Clone)]
pub(crate) struct Ops {
    pub exec: OpId,
    pub put: OpId,
    /// The page builder (one node), a page's finisher and the rest of a
    /// cut piece (`crate::pages`).
    pub pages: OpId,
    /// The node that places the page builder's where the text begins
    /// ([`crate::pages::Kick`]).
    pub kick: OpId,
    pub finish: OpId,
    pub resume: OpId,
    /// A paragraph's line breaking (`crate::parbreak::Break`).
    pub brk: OpId,
    /// A tool's run that a read makes (`crate::tools::ToolRun`).
    pub tool: OpId,
    /// A `\write18`'s tool node (`crate::tools::ToolNode`).
    pub toolnode: OpId,
    /// A page's PDF side, run once the loop-backs settle
    /// (`crate::pdfout::PdfPage`).
    pub pdfpage: OpId,
    /// The tools' settings ([`crate::Options::bibtex`]).
    pub tool_opts: std::sync::Arc<std::sync::Mutex<crate::tools::ToolOptions>>,
    /// The build's field handles (`crate::fields`).
    pub fields: std::sync::Arc<crate::fields::Fields>,
    /// The engine is XeTeX (set once, before the first evaluation):
    /// `crate::xetex`.
    pub xe: std::sync::Arc<std::sync::OnceLock<crate::xetex::XeTeX>>,
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
    /// The nodes whose run was recorded in the evaluation (`crate::pkgrec`):
    /// their records are merged at its end.
    pub rec_ran: std::sync::Arc<std::sync::Mutex<Vec<u64>>>,
    /// The build's package records, for the engine's predicted
    /// definitions ([`crate::pkgrec::LoadHints::prediction`]).
    pub pkg_loads:
        std::sync::Arc<std::sync::RwLock<Option<std::sync::Arc<crate::pkgrec::LoadHints>>>>,
    /// The document files the client cut (`api::DocFile`): a run reaching
    /// one places its slices as its pieces ([`Run::doc_file_regions`]).
    pub docs: crate::api::DocFiles,
    /// The client's `\openout` name check ([`crate::Options::out_name_ok`]).
    pub out_ok: std::sync::Arc<std::sync::Mutex<Option<crate::api::OutNameOk>>>,
    /// What the last two finishers running each output routine defined,
    /// by the routine's hint value (the earlier run first): the next
    /// finisher's declared names, and its predicted definitions where
    /// both agree (`crate::pages::Finish`).
    pub fin_defs: std::sync::Arc<std::sync::Mutex<crate::FxHashMap<u64, crate::pages::FinDefs>>>,
    /// [`Ops::fin_defs`] as the evaluation began: what the guesses of
    /// what a routine gives back and defines globally read (a guess
    /// from what another node of the same evaluation ran would make run
    /// counts follow the schedule, and hide that dependency from the
    /// engine).
    pub fin_defs_start: std::sync::Arc<std::sync::Mutex<std::sync::Arc<FinTable>>>,
    /// Each page's routine hint and class ([`crate::pages::pen_class`]) as
    /// the page loop last fired it, by the page's key
    /// ([`crate::pages::FinPageKey`]): what its finisher, whose arguments
    /// are the key alone, predicts and declares by
    /// ([`crate::pages::Finish::predicted_defs`]); the loop that places
    /// the finisher writes it before its commit.
    /// With them, the page's index among the pages the loop fired (its
    /// checkpoint's): the page number a finisher's prediction carries
    /// its templates to ([`crate::pages::FinDefs::predict`]).
    pub fin_class: std::sync::Arc<std::sync::Mutex<crate::FxHashMap<crate::pages::FinPageKey, FinClass>>>,
    /// [`crate::Options::finisher_predictions`].
    pub fin_predict: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// [`crate::Options::origins`]: the pages' writers keep their glyphs
    /// (`crate::origins`).
    pub origins: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// The profile units the page loop folded over all its runs
    /// (`crate::pages::Run::fold_unit`; `Stats::units_folded`).
    pub units_folded: std::sync::Arc<std::sync::atomic::AtomicU64>,
    /// The integer registers named by `\countdef` so far, by name (a
    /// LaTeX counter X's register is `\c@X`'s): what a node's text steps
    /// is predicted on them ([`crate::hints::counter_rels`]); a name
    /// defined again, or not yet, is a wrong or a missing guess, never
    /// a wrong value.
    pub counters: std::sync::Arc<std::sync::RwLock<crate::FxHashMap<&'static str, phitex_defuse::Field>>>,
    /// What the last run of a text of each scope shape left
    /// ([`crate::hints::ScopeShape`], by the opener's or closer's name):
    /// whether an opener was left to the engine as a scope, whether a
    /// closer closed the scope its node ran in. The brackets of a node
    /// not yet run are predicted on them ([`crate::hints::predicted_brackets`]).
    pub scope_learned: std::sync::Arc<std::sync::RwLock<crate::FxHashMap<String, bool>>>,
    /// The progress the build watches and its text nodes' offsets, set
    /// before each evaluation: each run holds a slot while it runs
    /// ([`crate::exec::Busy`]).
    pub watch: std::sync::Arc<std::sync::RwLock<Option<crate::exec::Watch>>>,
    /// The format's values made for this build (`crate::fmtenv::Made`):
    /// each handed out again by its node, a root of a collection.
    pub fmt_made: std::sync::Arc<std::sync::RwLock<Option<std::sync::Arc<crate::fmtenv::Made>>>>,
    /// The values the build holds outside the engine as an evaluation
    /// begins (its environment's, the job's, the files'), roots of a
    /// collection in it ([`crate::collect`]).
    pub held: std::sync::Arc<std::sync::RwLock<Vec<ValueId>>>,
    /// [`crate::Options::node_records`].
    pub node_rec: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// The class projection of meanings ([`crate::val::class_projection`],
    /// registered with the engine): a lookup that only stores its token
    /// reads the name's class, not its meaning ([`Run::class_lookup`]).
    pub class_proj: phitex_defuse::ProjId,
    /// The outer projection of meanings ([`crate::val::outer_projection`]):
    /// a token only stepped over reads whether its meaning is `\outer`
    /// ([`Run::outer_lookup`]).
    pub outer_proj: phitex_defuse::ProjId,
    /// The step projection of meanings ([`crate::val::step_projection`]):
    /// a token stepped over in a skipped conditional branch, or fetched
    /// only to be stored or matched, reads only its meaning's class
    /// ([`Run::step_lookup`]).
    pub step_proj: phitex_defuse::ProjId,
    /// The definedness projection of meanings
    /// ([`crate::val::defined_projection`]): `\csname` reads only whether
    /// the name it makes is defined ([`Run::defined_lookup`]).
    pub defined_proj: phitex_defuse::ProjId,
    /// The kind projection of the main list's last item
    /// ([`crate::val::tail_projection`]): a reader of its kind reads that,
    /// not the item ([`Run::list_last_kind`]).
    pub tail_proj: phitex_defuse::ProjId,
    /// The number projection of meanings ([`crate::val::num_projection`]):
    /// `\ifnum` comparing a macro's number reads the comparison's outcome
    /// ([`Run::num_order`]).
    pub num_proj: phitex_defuse::ProjId,
    /// The comma-list projection of meanings ([`crate::val::clist_projection`]):
    /// a membership test of a list macro reads whether the item is in it
    /// ([`Run::clist_has`]).
    pub clist_proj: phitex_defuse::ProjId,
    /// The build's read-time PDF commands, kept for the link by serial
    /// (`crate::pdfout::command`).
    pub pdf_cmds: std::sync::Arc<crate::pdfout::CmdMemo>,
    /// The identities of the objects only PDF bytes name, each with its
    /// node and its place among the node's commands
    /// (`crate::pdfprim::alloc_rel`): one identity, one object.
    pub rel_keys: std::sync::Arc<std::sync::Mutex<crate::FxHashMap<i64, (u64, u64)>>>,
    /// What each node's last run published and read of the state, by
    /// node ([`NodeAudit`]): the state model's tests ask it
    /// (`Build::definitions_by_op`, `Build::reads_by_op`,
    /// `Stats::relations_by_op`).
    pub audit: std::sync::Arc<std::sync::Mutex<crate::FxHashMap<u64, NodeAudit>>>,
}

/// What a node's last run published and read of the state
/// ([`Ops::audit`]): its operation, each item it published (and whether
/// as a relation), and the items it read, counted by family
/// ([`crate::state::Family::prefix`]), those of no family by field.
#[derive(Clone, Default)]
pub(crate) struct NodeAudit {
    pub op: &'static str,
    pub defs: Vec<(Field, bool)>,
    pub reads: Vec<(&'static str, u32)>,
    pub other_reads: Vec<Field>,
}

/// The finishers' table ([`Ops::fin_defs`]).
pub(crate) type FinTable = crate::FxHashMap<u64, crate::pages::FinDefs>;

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
    /// A document file the client cut as it cuts the document's text
    /// (`api::DocFile`): its name as `\input` asked for it (a string
    /// value); its pieces are the cut's slices ([`Run::doc_file_regions`]),
    /// its text read only when a run reads its lines itself
    /// ([`Run::load_written`]).
    pub cut: Option<ValueId>,
    /// Its text read from its units or its cut ([`Run::load_written`]):
    /// an empty text read is a file at its end, not one still to read.
    loaded: bool,
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
            cut: None,
            loaded: false,
        }
    }

    /// A document file the client cut (`api::DocFile`), `\input` as
    /// `key` (a string value), its text not read.
    pub(crate) fn cut(v: &Values, key: ValueId, name: ValueId) -> FileRest {
        let mut f = FileRest::new(v, v.str(""), 0, name);
        f.sliceable = true;
        f.cut = Some(key);
        f
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

    /// A file read inline by the run that opens it, never as paragraph
    /// regions: a tool's output (`crate::tools`), a whole text the
    /// reader holds.
    pub(crate) fn inline(v: &Values, id: ValueId, name: ValueId) -> FileRest {
        let mut f = FileRest::new(v, id, 0, name);
        f.units = Some(std::sync::Arc::from(Vec::new()));
        f.sliceable = false;
        f
    }

    /// Whether its text is still to be read: from its units, or a cut
    /// file's from the file. Once read it is not, even empty: a cut file
    /// whose text read empty (the job writes it, so outside a fixpoint it
    /// is absent) gave an empty line for ever, a compat build with an
    /// `.aux` on disk past 10 GB in 45 s.
    pub(crate) fn lazy(&self) -> bool {
        !self.loaded
            && (self.cut.is_some() || self.units.as_ref().is_some_and(|u| !u.is_empty()))
            && self.str().is_empty()
    }

    /// Its text, read from its units ([`Run::load_written`]).
    pub(crate) fn set_text(&mut self, v: &Values, id: ValueId) {
        self.id = id;
        self.text = v.get(id);
        self.loaded = true;
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

    /// Sets the position in characters (a rollback in the input's tests).
    #[cfg(test)]
    pub(crate) fn set_pos_chars(&mut self, p: usize) {
        self.pos = p;
    }


    /// The next line as a segment not yet reached (what a top-level
    /// node's arguments for it decode to, without a value or a string per
    /// line: its bytes are its characters, trailing blanks removed, as
    /// web2c's `input_line`); the offset moves past it.
    pub(crate) fn take_line(&mut self, xetex: bool) -> Line {
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
        // (XeTeX's `input_line` trims spaces and line ends, not tabs)
        while end > 0 && (bytes[end - 1] == b' ' || (!xetex && bytes[end - 1] == b'\t')) {
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
            origin: Some((self.name, at, ValueId::UNIT)),
            lineno: self.line,
            rescanned: false,
            chars: false,
        }
    }
}

/// One past the largest character code an `\\endlinechar` may append
/// (§360: `0..255`; XeTeX's `0..biggest_usv`).
#[inline]
pub(crate) fn max_char_end(xetex: bool) -> i64 {
    if xetex {
        i64::from(crate::xetex::BIGGEST_USV) + 1
    } else {
        256
    }
}

/// Where `\inputlineno` reads (`line`, §304: the innermost file's).
#[derive(Clone, Copy)]
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
    if crate::exec::prof_flag!("PHITEX_TEX_TIME") {
        let mut m = STOPS.locked();
        *m.get_or_insert_with(Default::default)
            .entry((op, f))
            .or_default() += 1;
    }
}

/// The fields runs stopped on most since the last call, with the
/// readers' kind and counts.
pub(crate) fn take_stops(v: &Values, n: usize) -> Vec<(String, u64)> {
    let m = STOPS.locked().take().unwrap_or_default();
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
    pub ops: &'c Ops,
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
    /// Macro expansions so far: told the engine every 4096 as its work
    /// (`Ctx::tick`), as the commands are.
    pub exps: u64,
    pub hb_at: u64,
    pub hb_done: u64,
    /// Whether the glue `scan_glue` last gave has a spec of its own
    /// (made from a dimension, or a register negated), not the value of
    /// a glue register or parameter (`GlueNode::own_spec`).
    pub glue_own: bool,
    /// Whether the internal glue `scan_something_internal` last gave is a
    /// spec of its own: a `\glueexpr` or `\muexpr` with an operator, or
    /// whose one factor's spec was its own (e-TeX's `scan_expr`: `t:=f`,
    /// `e:=t` keep the factor's spec, an operation makes a new one).
    pub glue_fresh: bool,
    /// The run's progress as the stuck check last saw it
    /// ([`Run::watchdog`]).
    pub(crate) wd: crate::control::Watchdog,
    pub pulled: usize,
    /// In a run resumed from a park, how many segments at the head of the
    /// input's text were levels of the input when it parked (top first,
    /// [`Run::park`]): the levels it had, not text it had yet to reach
    /// ([`Run::file_line_here`] looks through them as through levels).
    pub parked_levels: usize,
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
    /// The run hit a fatal error (`Halt::Fatal`): its claim takes every
    /// node after it in the unit (`Ctx::claim_rest`), not its text
    /// siblings alone.
    pub fatal_hit: bool,
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
    /// The files of `fread` this run read cut ([`Run::read_cut`]): a
    /// record keys on their presence, not their text.
    pub uread: Vec<String>,
    /// Inside `pass_text` (§494): a skipped text's tokens are read only
    /// to classify them (conditional, `\fi`/`\else`, or neither).
    pub in_skip: bool,
    /// The expansions nested now ([`crate::expand::EXPAND_DEPTH`]).
    pub(crate) expand_depth: u32,
    /// Where the conditional text being skipped began (§494's
    /// `skip_line`, [`Run::line_mark`]).
    pub skip_from: (Option<i64>, i64, Option<ValueId>),
    /// The locations a load's run first read from the engine while
    /// skipping (`PHITEX_PKG_TRACE`: which chaining reads are skips).
    pub skip_first: Vec<Loc>,
    /// Set by `\the` and `\number` in a piece around the scan of their
    /// one internal quantity (docs/spec-deferred-reads.md §2): the
    /// outermost `scan_something_internal` takes it, so a register read
    /// inside that scan (a register's number, a conditional) is a read.
    pub defer_ok: bool,
    /// The register a scan under [`Run::defer_ok`] met unread: the
    /// caller makes its symbol ([`crate::defer::symbol_for`]).
    pub defer_hit: Option<Loc>,
    /// Per integer register, its value as first read from the engine,
    /// before any map of this run: what a restore to the register's own
    /// symbol puts back ([`Run::reg_restore`]).
    pub reg_loaded_base: phitex_defuse::FxHashMap<Loc, i32>,
    /// The registers this run defers besides `count:0`
    /// ([`crate::defer::deferrable`]), made once a run.
    pub defer_regs: Option<Vec<Loc>>,
    /// Per integer register the run defined at its base level, the
    /// candidates it had passed at its first definition: a restore takes
    /// the definitions back only where none passed since
    /// ([`crate::defer::restore_reaching`]).
    pub reg_first_def: phitex_defuse::FxHashMap<Loc, u32>,
    /// Whether the node is a package load at the unit's top level, whose
    /// run is recorded (`crate::pkgrec`) and publishes its net effect.
    pub rec_load: bool,
    /// While looking for the command's first token: a node's command
    /// begins in its own text (the next sibling's begins in its own).
    pub no_pull: bool,
    /// The run's state table ([`crate::state`]): every field it read
    /// or defined, native.
    pub st: crate::state::State,
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
    /// The last formula's families' fonts, decoded, with the fonts'
    /// generation they were decoded at ([`Run::mlist_to_hlist`]).
    pub(crate) math_memo: Option<crate::math::MathMemo>,
    /// Set while a token is fetched only to be stored (`\edef`'s and
    /// `\write`'s expansion, §478): an unexpandable meaning not read
    /// in full is read by its class ([`Run::class_lookup`]).
    pub class_reads: bool,
    /// The meanings this run read by class only, with their command
    /// codes (the engine holds the projected reads; this run's own
    /// lookups and the cut checks need them).
    pub memos: crate::state::Memos,
    /// A token list's vector a definition gave back after copying its
    /// body out (`\def`), taken by the next scan ([`Run::scan_toks`]).
    pub toks_buf: Vec<Tok>,
    /// Set while a token is fetched only to be stepped over
    /// (`\expandafter`'s first, §368): a meaning not this run's own is
    /// read only as whether it is `\outer` ([`Run::outer_lookup`]).
    pub outer_reads: bool,
    /// Set while a token is fetched only to be stored or matched, never
    /// expanded (a macro's arguments, a definition's parameter text and
    /// body, a token list's body, §392, §473–§479): a meaning not this
    /// run's own is read only by its class ([`Run::step_lookup`]).
    pub step_reads: bool,
    /// The meanings this run read as outer or not only (kept over a park
    /// like [`Run::class_memo`]).
    /// The meanings this run read through the step projection only, with
    /// the class ([`Run::step_lookup`]; kept over a park like
    /// [`Run::class_memo`]).
    /// The meanings this run read as defined or not only
    /// ([`Run::defined_lookup`]; kept over a park like [`Run::class_memo`]).
    /// Set while a number scan of an `\ifnum` operand looks for its first
    /// token (§440): a macro not this run's own whose body is a number
    /// is not read, the scan keeps it as that macro's number
    /// ([`Run::num_lookup`], [`crate::scan::Num`]).
    pub num_reads: bool,
    /// Where the lines before the run went ([`crate::control::LinesTaint`]).
    pub lt: crate::control::LinesTaint,
    pub contribs: Vec<(Field, ValueId)>,
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
    /// The objects this run made at read time in PDF mode, all, those
    /// counted in the `pdf:objnum` cell, and those only PDF bytes name
    /// (`crate::pdfprim::REL`, counted in no cell): what a number spent
    /// in this run adds to the count before it
    /// (`crate::pdfprim::rt_before`). Not carried by a continuation (its
    /// node counts from its own start).
    pub rt_made: i64,
    pub rt_cells: i64,
    pub rt_rels: i64,
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
    /// The read-back fields the line [`Run::interpret_line`] interpreted
    /// last read, by name, sorted: a file whose line reads a field an
    /// earlier line of it defines is read back as its text
    /// ([`crate::api::written_as_text`]).
    pub interp_reads: ValueId,
    /// The locations the line being interpreted read before it defined
    /// them, if it did ([`Run::interp_reads`]).
    pub interp_first: Vec<Loc>,
    /// [`Run::set_mean`] finds the location it defines.
    pub defining: bool,
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
    /// Whether the last line interpreted ([`Run::interpret_line`]) only
    /// defined: it contributed nothing, typeset nothing, called no
    /// node, left no group or conditional open. A file every line of
    /// which only defines is read back per field ([`Run::read_back`]).
    pub interp_plain: bool,
    /// The files this run wrote a line of that does more than define:
    /// read back as text, their next lines not interpreted.
    pub text_written: Vec<String>,
    /// The files the job writes whose round-one read-back this run made
    /// from the text the client gave for them ([`Run::seed_written`]).
    pub seeded: Vec<Seeded>,
    /// Where each segment of a resumed piece's text was read
    /// ([`Run::cont_load`], by its index in the text): a token level's
    /// file line, for `\inputlineno` ([`Run::file_line_here`]).
    pub cont_lines: Vec<Option<FileLine>>,
    /// A resume's continuation and claim guess, read from the field its
    /// piece defined at the cut (`cont:<piece>:<k>`, [`Run::fold_here`]):
    /// its text as an exec node's first argument is (scope shape, file
    /// lines), and the siblings it takes where the piece's text says so.
    pub cont_val: Option<ValueId>,
    pub cont_guess: Option<usize>,
    /// The continuation of a run that closed the engine scope it ran in
    /// inside a level of its own (a paragraph begun in the scope): its
    /// rest goes on outside the scope as a resume ([`Run::close_resume`]).
    pub close_cont: Option<ValueId>,
    /// A read-back point not known to be one yet: the file this run
    /// looked for in vain from the macro running ([`Run::read_back_at`]).
    /// When the run opens that file for writing, the point was its
    /// read-back point ([`Run::opened_for_writing`]): the values kept meanwhile
    /// ([`Run::pending_pre`]) become the read-back values. Else nothing
    /// of it is published.
    pub read_point_pending: Option<String>,
    /// The files this run looked for in vain after it became a read-back
    /// point ([`Run::read_point_here`]): one it opens for writing later is
    /// read back here next round, where the lines written to it are read
    /// ([`Run::note_absent_read_point`]), as when the file is there.
    pub vain_after_point: Vec<String>,
    /// The values at a pending read-back point of the locations defined
    /// or restored after it ([`Run::note_pre`]).
    pub pending_pre: Vec<(Loc, ValueId)>,
    /// The locations of [`Run::pending_pre`], to look one up there.
    pub pending_locs: phitex_defuse::FxHashSet<Loc>,
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
    /// The locations [`Run::note_pre`] is done with in this run (their
    /// value at the read-back point noted, or none to note).
    pub pre_noted: phitex_defuse::FxHashSet<Loc>,
    /// Where on the input stack [`Run::read_back_at`]'s level was found
    /// last ([`Run::note_pre`]).
    pub read_back_ix: std::cell::Cell<Option<usize>>,
    /// A lookup of the output writer's own (a font's file at a
    /// `\shipout`, `crate::pdfout::file_bytes`), not the text's: a file
    /// looked for in vain there is no read-back point.
    pub writer_lookup: bool,
    /// The outermost macro being expanded at the read-back (its input
    /// level): what the run defines while it is still on the input
    /// stack keeps its value at the read-back ([`Run::note_pre`]).
    pub read_back_at: Option<crate::input::Level>,
    /// This run is the read-back point of a job-written file
    /// ([`Run::mark_read_point`]): what it defines from there to its end
    /// keeps its value there (`wpre:<field>`, [`Run::note_pre`]), not only
    /// while the macro that read the file runs.
    pub read_point_here: bool,
    /// While a line is interpreted ([`Run::interpret_line`]): the node
    /// where its file is read back (`wpoint:<file>`), whose state the
    /// line's reads see.
    pub interp_point: Option<phitex_defuse::NodeId>,
    /// [`Run::revert_read_back`]'s values by location, for reads at
    /// [`Run::interp_point`] (`pre_memo_at`).
    pub pre_memo: phitex_defuse::FxHashMap<Loc, Option<ValueId>>,
    pub pre_memo_at: Option<phitex_defuse::NodeId>,
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
    /// The serial of the writer command the command in progress recorded
    /// ([`crate::pdfout::command`]), until a read-time allocation takes
    /// it as its own ([`crate::pdfout::note_read_time`]); 0: none.
    pub pdf_cmd: u64,
    /// The scope kind the node runs in, once read ([`Run::scope_kind`]):
    /// it does not change during the run.
    #[allow(clippy::option_option)]
    scope_kind_memo: Option<Option<ValueId>>,
    /// What the run publishes, made before it finishes
    /// ([`Run::prepare_publication`]), with the table's state then and
    /// the locations it restored.
    #[allow(clippy::type_complexity)]
    pub(crate) pubs: Option<(((usize, usize, usize, u64), u64), Vec<(Field, crate::state::Pub, Locality)>, Vec<Loc>)>,
    pub ins_pending: Vec<u16>,
    pub fin: Option<crate::pages::FinCtx>,
    /// While a builder folds (the loop, a finisher's replay, the text's
    /// page reads): the piece the page before fired in (raw id, -1 none)
    /// and the insertion classes' registers the builder has from the
    /// routines' give-backs and the text's own markers, in fold order. A
    /// class's registers read where a piece is are those, with what the
    /// text set since that fire, never a finisher's definition by its
    /// place ([`Run::insert_class_at`], docs/spec-settle.md §2.1).
    pub ins_fold: Option<(i64, Vec<(u16, phitex_typeset::page::InsertClass)>)>,
    /// The page loop's keys for this run ([`crate::pages::LoopKeys`]).
    pub loop_keys: crate::pages::LoopKeys,
    pub cut: Option<u32>,
    /// The candidate a second pass ends at ([`Halt::Recut`]).
    pub stop_at: Option<u32>,
    /// While a character run's lookahead expands (§1038): its list and
    /// its effect on the space factor so far, its nodes not on the list
    /// yet; the tail is its last character ([`Run::space_factor`],
    /// [`Run::last_item`]).
    pub chars_live: Option<(i64, i32)>,
    /// A character run's effect on the space factor from the transitions
    /// it holds for the list ([`crate::build::sf_compose`]).
    pub chars_eff: i32,
    /// A `\write18`'s tool node this run ends at ([`Run::tool_command`]):
    /// placed before its resume.
    pub tool_cut: Option<ValueId>,
    /// A read of a file's size (`\\pdffilesize`): a file the job writes
    /// and reads back is read as its size, not its text
    /// ([`Run::read_written_text`]).
    pub(crate) size_read: bool,
    /// The engine reads a scan step's builder made through its run
    /// (`pages::RunReaching`): a prefix folded with none is memoized.
    pub reach_reads: u64,
    /// A finisher's `\box255`, native, from its page's assembly to its
    /// routine ([`crate::pages::Fire::b255`]).
    pub fin_box: Option<(phitex_typeset::node::BoxNode, crate::nodes::Side)>,
    /// A finisher's sealed lines (`crate::pages::Run::seal_items`).
    pub(crate) sealed: crate::pages::Sealing,
    /// The fonts of the lines this run made (by their box values), from
    /// their nodes; `None` for a line not sealed
    /// ([`crate::nodes::unit_shape`]).
    pub(crate) line_fonts: crate::FxHashMap<ValueId, ValueId>,
    /// The font sets of those lines, each as one value.
    pub(crate) font_sets: crate::FxHashMap<u64, (Vec<u32>, ValueId)>,
    /// The main loop's `lig_stack`, kept between words (§1034).
    pub(crate) lig_buf: Vec<crate::build::LigItem>,
    /// The engine is XeTeX ([`Ops::xe`]): `crate::xetex`.
    pub(crate) xe: bool,
    /// XeTeX's input modes of the files this run read lines of, by the
    /// file's name, as first read or as this run set them
    /// (`crate::xetex`, `\XeTeXinputencoding`).
    pub(crate) xenc: Vec<(ValueId, i64)>,
    /// The native fonts this run met, by font (`crate::native`).
    pub(crate) xfonts: Vec<(i64, Option<std::sync::Arc<crate::native::NativeFont>>)>,
    /// The native fonts' last scripts as this run has them
    /// (`crate::native`, `fontdir:<f>`).
    pub(crate) xdirs: Vec<(i64, Option<u32>)>,
    /// XeTeX's `prev_class` and `space_class`: the class of the last
    /// character of the run of characters just made (`char_class_boundary`
    /// where none is), for `\XeTeXinterchartoks` (`crate::xetex`).
    pub(crate) ic_prev: i32,
    pub(crate) ic_space: i32,
}

impl<'c, 'a> Run<'c, 'a> {
    #[allow(clippy::too_many_lines)]
    pub(crate) fn new(cx: &'c mut Ctx<'a>, ops: &'c Ops) -> Self {
        let v = cx.values();
        let op = cx.op();
        if crate::exec::obs_flag!("PHITEX_LOOP_TRACE") {
            eprintln!("lb-run node {:x} op {op:?}", cx.node().raw());
        }
        let xe = ops.xe.get().is_some();
        Run {
            xe,
            xenc: Vec::new(),
            xfonts: Vec::new(),
            xdirs: Vec::new(),
            ic_prev: crate::xetex::CLASS_BOUNDARY,
            ic_space: 0,
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
            exps: 0,
            hb_at: Run::heartbeat_start(),
            glue_own: false,
            glue_fresh: false,
            hb_done: 0,
            wd: crate::control::Watchdog::default(),
            pulled: 0,
            parked_levels: 0,
            pull_at: Vec::new(),
            pull_lev: None,
            pull_orig: Vec::new(),
            pull_box: Vec::new(),
            top: false,
            claim_rest: false,
            fatal_hit: false,
            hint_void: false,
            nest: Vec::new(),
            next_lid: 0,
            last_par: (0, 0),
            doc: false,
            fread: Vec::new(),
            fmiss: Vec::new(),
            uread: Vec::new(),
            in_skip: false,
            expand_depth: 0,
            skip_from: (None, 0, None),
            skip_first: Vec::new(),
            defer_ok: false,
            defer_hit: None,
            reg_loaded_base: phitex_defuse::FxHashMap::default(),
            defer_regs: None,
            reg_first_def: phitex_defuse::FxHashMap::default(),
            rec_load: false,
            no_pull: false,
            st: crate::state::State::new(),
            cat_cache: [u8::MAX; 256],
            cat_gen: u64::MAX,
            code_cache: Box::new([[i32::MIN; 256]; 5]),
            code_gen: u64::MAX,
            font_memo: phitex_defuse::FxHashMap::default(),
            math_memo: None,
            class_reads: false,
            memos: crate::state::Memos::default(),
            toks_buf: Vec::new(),
            outer_reads: false,
            step_reads: false,
            num_reads: false,
            lt: crate::control::LinesTaint::default(),
            contribs: Vec::new(),
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
            rt_made: 0,
            rt_cells: 0,
            rt_rels: 0,
            muts: 0,
            interp: None,
            interp_short: false,
            interp_reads: ValueId::NIL,
            interp_first: Vec::new(),
            defining: false,
            interp_nested: phitex_defuse::FxHashMap::default(),
            interp_eof: false,
            interp_errs: Vec::new(),
            interp_plain: false,
            text_written: Vec::new(),
            seeded: Vec::new(),
            cont_lines: Vec::new(),
            cont_val: None,
            cont_guess: None,
            close_cont: None,
            read_point_pending: None,
            vain_after_point: Vec::new(),
            pending_pre: Vec::new(),
            pending_locs: phitex_defuse::FxHashSet::default(),
            scanner: None,
            outer_call: false,
            interp_base: 0,
            closed: Vec::new(),
            read_back_done: false,
            pre_noted: phitex_defuse::FxHashSet::default(),
            read_back_ix: std::cell::Cell::new(None),
            writer_lookup: false,
            read_back_at: None,
            read_point_here: false,
            interp_point: None,
            pre_memo: phitex_defuse::FxHashMap::default(),
            pre_memo_at: None,
            error_count: 0,
            fatal_logged: false,
            mark: crate::getnext::Mark::default(),
            halt: None,
            cand: 0,
            cand_marks: Vec::new(),
            pdf_cmd: 0,
            scope_kind_memo: None,
            pubs: None,
            ins_pending: Vec::new(),
            fin: None,
            ins_fold: None,
            loop_keys: crate::pages::LoopKeys::default(),
            cut: None,
            stop_at: None,
            chars_live: None,
            chars_eff: i32::MIN,
            tool_cut: None,
            size_read: false,
            fin_box: None,
            sealed: crate::pages::Sealing::default(),
            line_fonts: crate::FxHashMap::default(),
            font_sets: crate::FxHashMap::default(),
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
        match self.st.load_state(loc) {
            1 => Ok(()),
            2 => self.unalias(loc),
            _ => self.load_slow(loc),
        }
    }

    /// Integer register `loc` holds an alias definition's value (a
    /// symbol written to it, `crate::defer::try_symbolic`, or such a
    /// binding a finisher or a resume was given) and the run uses it:
    /// the symbol's digits, forced in a piece, resolved anywhere else
    /// (`crate::defer::force`, `crate::defer::resolve`). The binding the
    /// run publishes stays the alias.
    #[cold]
    #[inline(never)]
    fn unalias(&mut self, loc: Loc) -> R<()> {
        let Some((g, node)) = self.st.alias_at(loc, self.v) else {
            return Ok(());
        };
        let s = crate::defer::Sym { loc: self.loc_of(g), node };
        let x = match self.op_name() {
            "exec" | "resume" => crate::defer::force(self, s)?,
            _ => crate::defer::resolve(self, s)?,
        };
        self.st.resolve_alias(loc, x);
        Ok(())
    }

    /// A job-written line is read as the next round's read-back reads
    /// it: in the state at its read-back point, before this round's
    /// read-back took effect. The first read of a field in the line gets
    /// the value the read-back replaced (`wpre:<field>`, left by
    /// [`Run::read_back`]), or of a field the reading run defined after
    /// the point, its value there ([`Run::note_pre`]); of any other
    /// field, its value at the node that reads the file back
    /// ([`Run::interp_point`], an engine read at that position), so a
    /// line whose effect depends on a field defines it as the reading run
    /// will, not as this run, which holds the previous round's read-back
    /// (a list appended to unless it holds the item) or what the text
    /// defined since the point (a register allocated after it, a command
    /// a conditional in the line tests), would.
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
        // (read before the line defined it: what the lines before it
        // left there, [`Run::interp_reads`]; a meaning about to be
        // defined is not read)
        if !self.defining {
            self.interp_first.push(loc);
        }
        if self.pre_memo_at != self.interp_point {
            self.pre_memo.clear();
            self.pre_memo_at = self.interp_point;
        }
        let pre = if let Some(&p) = self.pre_memo.get(&loc) {
            p
        } else {
            let p = match self.pre_value(loc) {
                Ok(p) => p,
                Err(h) => {
                    // (read again once what it waits on is there)
                    if let Some(d) = &mut self.interp {
                        d.remove(&loc);
                    }
                    return Err(h);
                }
            };
            self.pre_memo.insert(loc, p);
            p
        };
        if let Some(p) = pre {
            if crate::exec::obs_flag!("PHITEX_AUX_TRACE") {
                let name = crate::val::field_name(self.v, self.loc_field(loc));
                eprintln!(
                    "aux-trace: revert {name} for the line's read: {}",
                    if p == ValueId::UNIT {
                        "absent".to_owned()
                    } else {
                        crate::val::show_def(self.v, p)
                    }
                );
            }
            // (read in place of the value, not a definition of the line:
            // only what the line defines on it is published)
            self.st
                .preload(self.v, loc, if p == ValueId::UNIT { None } else { Some(p) });
        }
        Ok(())
    }

    /// The value location `loc` has at the read-back point of the line
    /// being interpreted ([`Run::revert_read_back`]): `UNIT` for absent,
    /// `None` where the line reads it as this run holds it (no read-back
    /// point known, the point this run itself, a record's cell, the
    /// run's own bookkeeping).
    fn pre_value(&mut self, loc: Loc) -> R<Option<ValueId>> {
        let f = self.loc_field(loc);
        let name = crate::val::field_name(self.v, f);
        if name.starts_with("wpre:") {
            return Ok(None);
        }
        // (a read-back field, or one the reading run defined after the
        // read-back, [`Run::note_pre`]: as this run holds it, the line
        // written in the reading run itself, or no point known)
        let Some(at) = self.interp_point.filter(|&n| n != self.cx.node()) else {
            return self.get(&format!("wpre:{name}"));
        };
        if !is_read_back_definition(&name) {
            return Ok(None);
        }
        // (at the point: the reading run's own `wpre:`, else the field as
        // that node leaves it)
        let pf = self
            .ops
            .fields
            .get_at(self.v, self.cx.values_epoch(), &format!("wpre:{name}"));
        for g in [pf, f] {
            let r = self.cx.read_at(g, phitex_defuse::At::AfterOwn(at));
            if r.is_err() {
                note_stop(self.op_name(), g);
            }
            match r? {
                phitex_defuse::FieldAt::Def(x, by) if g == f || by == Some(at) => return Ok(Some(x)),
                phitex_defuse::FieldAt::Unset if g == f => return Ok(Some(ValueId::UNIT)),
                phitex_defuse::FieldAt::Gone => return Ok(None),
                _ => {}
            }
        }
        Ok(None)
    }

    /// An engine read of field `f` outside the state table (by name): a
    /// record's cell from its record.
    pub(crate) fn eng_read(&mut self, f: Field) -> Result<Option<ValueId>, phitex_defuse::Stop> {
        let r = self.cx.read(f);
        if r.is_err() {
            note_stop(self.op_name(), f);
        }
        if crate::exec::obs_flag!("PHITEX_READS") {
            self.trace_read(f, r.is_err());
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
        } else if op == o.pages {
            "pages"
        } else if op == o.finish {
            "finish"
        } else if op == o.brk {
            "break"
        } else {
            "other"
        }
    }







    /// What the run publishes ([`crate::state::State::publication`]),
    /// made before it finishes with the reads the rule takes: whether the
    /// run stands outside every scope of the document (the scope kind
    /// reaching it, read only where a global write restores what it read,
    /// spec §2.3 rule 1), and the value reaching it of each binding a
    /// continuation carried that it publishes again (rule 2: one equal to
    /// it is none). A stop here runs the node again from its start.
    pub(crate) fn prepare_publication(&mut self) -> R<()> {
        let top = self.rec_load || (self.st.has_global_restore(self.v) && self.scope_kind()?.is_none());
        let (pubs, restored) = self.st.publication(self.v, top);
        let mut out = Vec::with_capacity(pubs.len());
        for p in pubs {
            let f = self.loc_field(p.loc);
            if p.carried
                && let crate::state::Pub::Value(x) = p.what
                && self.eng_read(f)? == Some(x)
            {
                continue;
            }
            out.push((f, p.what, p.at));
        }
        self.pubs = Some((self.pub_key(), out, restored));
        Ok(())
    }

    /// The state the publication was made in ([`Run::prepare_publication`]).
    fn pub_key(&self) -> ((usize, usize, usize, u64), u64) {
        (self.st.sig(), self.st.muts())
    }

    /// What the run publishes: as [`Run::prepare_publication`] made it,
    /// else (the table changed since, or it was not made) made now with
    /// no read: a global restore published where the run's scope is not
    /// known, a carried binding published as it is.
    pub(crate) fn publication(&mut self) -> Vec<(Field, crate::state::Pub, Locality)> {
        if let Some((k, out, _)) = &self.pubs
            && *k == self.pub_key()
        {
            return out.clone();
        }
        let top = self.rec_load || matches!(self.scope_kind_memo, Some(None));
        let (pubs, _) = self.st.publication(self.v, top);
        pubs.into_iter().map(|p| (self.loc_field(p.loc), p.what, p.at)).collect()
    }

    /// The values the run publishes (its relations left out).
    pub(crate) fn published_values(&mut self) -> Vec<(Field, ValueId, Locality)> {
        self.publication()
            .into_iter()
            .filter_map(|(f, w, l)| match w {
                crate::state::Pub::Value(x) => Some((f, x, l)),
                crate::state::Pub::Rel(_) => None,
            })
            .collect()
    }

    #[inline(never)]
    fn load_slow(&mut self, loc: Loc) -> R<()> {
        if self.in_skip && self.rec_load {
            self.skip_first.push(loc);
        }
        // (a finisher's field given where the routine reads it: its
        // value at the candidate, read now, [`Run::fin_lazy_value`])
        if let Some(x) = self.st.lazy(loc) {
            let y = self.fin_lazy_preload(x)?;
            self.st.preload(self.v, loc, y);
            if self.st.load_state(loc) == 2 {
                return self.unalias(loc);
            }
            return Ok(());
        }
        let f = self.loc_field(loc);
        let r = self.st.get(self.cx, self.v, loc, f);
        if r.is_err() {
            note_stop(self.op_name(), f);
        }
        if crate::exec::obs_flag!("PHITEX_READS") {
            self.trace_read(f, r.is_err());
        }
        if crate::exec::obs_flag!("PHITEX_DEF_TRACE")
            && crate::exec::env_str!("PHITEX_DEF_TRACE")
                .is_some_and(|w| w == crate::val::field_name(self.v, f))
        {
            eprintln!(
                "def-trace read {} = {} by node {} op {:?} at {:?} (cmd #{})",
                crate::val::field_name(self.v, f),
                match self.st.engine_value(loc) {
                    Some(Some(x)) => format!("{x:?} {}", crate::val::show_def(self.v, x)),
                    Some(None) => "none".to_owned(),
                    None => "stop".to_owned(),
                },
                self.cx.node().raw(),
                self.cx.op(),
                self.cur.tok,
                self.cmds
            );
        }
        r?;
        if let Some(x) = self.st.engine_value(loc) {
            self.note_loaded_base(loc, x);
        }
        Ok(())
    }

    /// The integer register at location `loc`, if it is one.
    pub(crate) fn count_reg(&self, loc: Loc) -> Option<i32> {
        let l = loc as usize;
        if l.wrapping_sub(crate::fields::REG0) < 256 {
            return i32::try_from(l - crate::fields::REG0).ok();
        }
        if l < crate::fields::NSLOT {
            return None;
        }
        let f = self.st.field(loc)?;
        // (the name read in place: no copy of it per location loaded)
        self.v
            .with_str(f.0, |s| {
                s.strip_prefix("count:").and_then(|n| n.parse::<i32>().ok())
            })
            .flatten()
    }

    /// Whether a step of integer register `loc` by a constant may be a
    /// relation (spec §2.3 rule 3, [`crate::state::State::add`]): a
    /// document piece's write, or a routine's global one (not a format's
    /// or a recorded load's), outside every group of the run unless
    /// global, before the run's first candidate (a page firing inside the
    /// piece runs its routine at the candidate, on the value there), and
    /// not where a job-written file is read back (the read-back keeps the
    /// values its writes replace, [`Run::note_pre`]).
    pub(crate) fn map_ok(&self, loc: Loc, global: bool) -> bool {
        let op = self.cx.op();
        let routine = op == self.ops.finish && global;
        self.count_reg(loc).is_some()
            && (op == self.ops.exec || op == self.ops.resume || routine)
            && (self.doc || routine)
            && !self.rec_load
            && self.interp.is_none()
            && !self.read_back_done
            && !self.read_point_here
            && self.cand_marks.is_empty()
            && (global || self.innermost_group().is_none())
    }


    /// An integer register's engine value as first loaded
    /// ([`Run::reg_loaded_base`]).
    /// Kept for the registers a restore can name: `\count0`–`\count255`
    /// and the deferred ones ([`Run::defer_regs`]).
    fn note_loaded_base(&mut self, loc: Loc, x: Option<ValueId>) {
        if (loc as usize).wrapping_sub(crate::fields::REG0) < 256
            || self.defer_regs.as_ref().is_some_and(|r| r.contains(&loc))
        {
            let v = self.v;
            self.reg_loaded_base.entry(loc).or_insert_with(|| crate::val::i32_of(v, x));
        }
    }

    /// A restore of integer register `loc` to this piece's own symbol of
    /// it (docs/spec-deferred-reads.md §2, [`crate::defer::try_symbolic`]):
    /// the run's map of it the identity, neither a read nor a
    /// definition; where the run loaded it, its value back to the one
    /// first loaded, or, never read (a value its own `set` gave), unread.
    pub(crate) fn reg_restore(&mut self, loc: Loc, global: bool) {
        if crate::exec::obs_flag!("PHITEX_REG_TRACE") {
            eprintln!(
                "reg-restore: node {} register {:?} global {global}",
                self.cx.node().raw(),
                self.count_reg(loc)
            );
        }
        self.st.restore(loc);
        self.muts += 1;
    }

    /// Whether `\the`/`\number` in this run may defer a page-layer
    /// register (docs/spec-deferred-reads.md §2): a document piece's run
    /// (`exec`, `resume`), not a recorded load, a line read back, or a
    /// read-back point. Its candidates do not matter: the symbol resolves
    /// to the definition reaching the piece, which is what its own read
    /// gets wherever in the run it is made (`Ctx::read` is a read at the
    /// node's position). `PHITEX_NO_DEFER=1` turns it off (observe builds).
    pub(crate) fn defer_allowed(&self) -> bool {
        let op = self.cx.op();
        (op == self.ops.exec || op == self.ops.resume)
            && self.doc
            && !self.rec_load
            && self.interp.is_none()
            && !self.read_back_done
            && !self.read_point_here
            && !crate::exec::obs_flag!("PHITEX_NO_DEFER")
    }

    /// A definition of integer register `loc` about to be made: the
    /// first one notes the candidates passed ([`Run::reg_first_def`]).
    pub(crate) fn note_reg_def(&mut self, loc: Loc) {
        if !self.st.defined_after(loc, 0) && self.count_reg(loc).is_some() {
            self.reg_first_def.insert(loc, self.cand);
        }
    }

    /// The outermost quantity of a deferring scan is integer register
    /// `loc` ([`Run::defer_ok`] was set): true, and [`Run::defer_hit`]
    /// set, where it is a page-layer register the run has not loaded and
    /// holds no map of (its own write makes it concrete): no read.
    pub(crate) fn defer_at(&mut self, loc: Loc) -> bool {
        if self.st.is_loaded(loc) || self.st.has_map(loc) || self.st.lazy(loc).is_some() {
            return false;
        }
        if loc as usize >= crate::fields::NSLOT {
            self.loc_field(loc);
        }
        if self.count_reg(loc).is_none() || !crate::defer::deferrable(self, loc) {
            return false;
        }
        self.defer_hit = Some(loc);
        true
    }

    /// Field `f` as it reaches node `at` (right after its own run, or
    /// before it with `before`), as
    /// [`phitex_defuse::Ctx::use_field_after_own`] and
    /// [`phitex_defuse::Ctx::use_field_before`] give it: after a piece's
    /// own definitions, its claimed text included, never what it unfolded
    /// or what was placed in its extent (its paragraphs' lines, the
    /// finishers of pages firing in it, which come after it in TeX); an integer
    /// register with the maps contributed after its definition up to `at`
    /// applied, `at`'s own included (they precede its first candidate,
    /// [`crate::regfold`]).
    pub(crate) fn field_at(
        &mut self,
        f: Field,
        at: phitex_defuse::NodeId,
        before: bool,
    ) -> Result<phitex_defuse::FieldAt, phitex_defuse::Stop> {
        let r = if before {
            self.cx.read_at(f, phitex_defuse::At::Before(at))?
        } else {
            self.cx.read_at(f, phitex_defuse::At::AfterOwn(at))?
        };
        Ok(r)
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
        if crate::exec::obs_flag!("PHITEX_AUX_TRACE")
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
        if crate::exec::obs_flag!("PHITEX_DEF_TRACE") {
            self.def_trace(loc, &val, global);
        }
        if self.read_back_done && self.interp.is_none() {
            if crate::exec::obs_flag!("PHITEX_AUX_TRACE") {
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
        self.st.set(loc, val, global, grp);
        // (a page parameter the text sets, also as the text's own
        // (`tpp:`): what the page builder reads of the text, a routine's
        // reaching it through its give-back, docs/spec-settle.md §2.1)
        if crate::state::is_page_param(loc) && self.cx.op() != self.ops.finish {
            let t = self.tpp_loc(loc);
            self.st.set(t, val, global, grp);
        }
        // (and a box register an insertion class can be (0–254), as the
        // text's own (`tbox:`): the builder's class registers after the
        // first page, a routine's emptying of `\box n` reaching it through
        // its give-back, [`Run::insert_class_since`])
        if let Some(n) = crate::fields::box_of(loc)
            && self.cx.op() != self.ops.finish
        {
            let t = self.tbox_loc(n);
            self.st.set(t, val, global, grp);
        }
        self.muts += 1;
    }

    /// The location of box register `n` (0–254) as the text sets it
    /// ([`Run::set_loc`], [`Run::modify_box_reg`]): `tbox:<n>`, its field
    /// interned once per values epoch.
    pub(crate) fn tbox_loc(&mut self, n: usize) -> Loc {
        thread_local! {
            static TBOX: std::cell::RefCell<(u64, u64, Vec<Option<phitex_defuse::Field>>)> =
                const { std::cell::RefCell::new((u64::MAX, u64::MAX, Vec::new())) };
        }
        let v = self.v;
        let epoch = self.cx.values_epoch();
        let f = TBOX.with(|c| {
            let mut c = c.borrow_mut();
            if c.0 != v.instance() || c.1 != epoch {
                *c = (v.instance(), epoch, vec![None; 255]);
            }
            *c.2[n.min(254)].get_or_insert_with(|| self.ops.fields.get_at(v, epoch, &format!("tbox:{n}")))
        });
        self.loc_of(f)
    }

    /// The location of page parameter `loc` as the text sets it
    /// ([`Run::set_loc`]): `tpp:` and the parameter's field.
    pub(crate) fn tpp_loc(&mut self, loc: Loc) -> Loc {
        let v = self.v;
        let name = crate::val::field_name(v, self.loc_field(loc));
        self.loc_of(v.field(&format!("tpp:{name}")))
    }

    /// (observe) `PHITEX_DEF_TRACE=<field>`: each definition of the
    /// field, by which node and command, local or global, in which
    /// group, while a line is interpreted or not.
    #[cold]
    #[inline(never)]
    fn def_trace(&mut self, loc: Loc, val: &Entry, global: bool) {
        let Some(want) = crate::exec::env_str!("PHITEX_DEF_TRACE") else {
            return;
        };
        let f = self.loc_field(loc);
        let name = crate::val::field_name(self.v, f);
        if name != want {
            return;
        }
        eprintln!(
            "def-trace {name} := {} global={global} group {:?} nest {} interp {} by node {} op {:?} at {:?} (cmd #{})",
            self.st.debug_entry(val, self.v),
            self.innermost_group(),
            self.nest.len(),
            self.interp.is_some(),
            self.cx.node().raw(),
            self.cx.op(),
            self.cur.tok,
            self.cmds
        );
    }

    /// A field this run defines after reading a job-written file back:
    /// its value at the read-back point (`wpre:<field>`, once), which a
    /// line written later reads as the read-back does
    /// ([`Run::revert_read_back`]): a macro that reads a file back and
    /// then redefines what the file's lines use (makes a command an
    /// error from then on) leaves the lines read under the meanings at
    /// the read-back point.
    /// The field `wpre:<f>` of field `f` ([`Run::note_pre`]), none for a
    /// field whose value at a read-back point is not kept (`wpre:`,
    /// `wpoint:`, `cat:`, `wread:` and [`REREAD`] fields): made once per
    /// field and values epoch on this thread, not formatted per
    /// definition.
    fn wpre_of(&self, f: Field) -> Option<Field> {
        thread_local! {
            static C: std::cell::RefCell<(u64, u64, phitex_defuse::FxHashMap<u64, Option<Field>>)> =
                std::cell::RefCell::default();
        }
        let v = self.v;
        let (inst, epoch) = (v.instance(), self.cx.values_epoch());
        let hit = C.with(|c| {
            let mut c = c.borrow_mut();
            if c.0 != inst || c.1 != epoch {
                *c = (inst, epoch, phitex_defuse::FxHashMap::default());
            }
            c.2.get(&f.0.raw()).copied()
        });
        if let Some(x) = hit {
            return x;
        }
        let name = crate::val::field_name(v, f);
        let x = (!["wpre:", "wpoint:", "cat:", "wread:", REREAD]
            .iter()
            .any(|p| name.starts_with(p)))
        .then(|| v.field(&format!("wpre:{name}")));
        C.with(|c| {
            let mut c = c.borrow_mut();
            if c.2.len() >= 1 << 16 {
                c.2.clear();
            }
            c.2.insert(f.0.raw(), x);
        });
        x
    }

    /// Whether the macro that read a job-written file back still runs:
    /// its level on the input stack ([`Run::read_back_at`]).
    fn read_back_running(&self) -> bool {
        self.read_back_at.is_some_and(|m| {
            let same = |l: &crate::input::Level| {
                l.kind == m.kind && l.ty == m.ty && l.c == m.c && l.b == m.b
            };
            // (where it was found last: a level stays at its place on the
            // stack while it is there; else looked for again)
            if self
                .read_back_ix
                .get()
                .is_some_and(|i| self.input.levels.get(i).is_some_and(same))
            {
                return true;
            }
            let i = self.input.levels.iter().position(same);
            self.read_back_ix.set(i);
            i.is_some()
        })
    }

    pub(crate) fn note_pre(&mut self, loc: Loc) {
        let v = self.v;
        // (only while the macro that read the file back runs:
        // `\document`'s rest, not the text after it)
        // (a level's position moves as it is read; its start and end
        // stay)
        // (asked only where it decides: the run that is the read-back
        // point notes every definition whether the macro runs or not)
        // (the run that is a file's read-back point: to its end, as the
        // lines read the rest of the state at the end of its node,
        // [`Run::pre_value`])
        if !self.read_point_here && !self.read_back_running() {
            self.read_back_done = false;
            return;
        }
        // (a location noted once: its first definition after the
        // read-back is the one whose value before it counts)
        if self.pre_noted.contains(&loc) {
            return;
        }
        // (one kept at a pending read-back point: its value there kept
        // once, not encoded again at each definition)
        if self.read_point_pending.is_some() && self.pending_locs.contains(&loc) {
            return;
        }
        let f = self.loc_field(loc);
        let Some(pre) = self.wpre_of(f) else {
            self.pre_noted.insert(loc);
            return;
        };
        let pl = self.loc_of(pre);
        if self.st.defined_after(pl, 0) {
            self.pre_noted.insert(loc);
            return;
        }
        // (a stop here stops the run: it runs again from its start)
        if self.load(loc).is_err() {
            return;
        }
        let old = self.st.id_of(loc, v).unwrap_or(ValueId::UNIT);
        if self.read_point_pending.is_some() {
            if self.pending_locs.insert(loc) {
                self.muts += 1;
                self.pending_pre.push((loc, old));
            }
            return;
        }
        self.set_pre(loc, old);
        self.pre_noted.insert(loc);
    }

    /// `wpre:<field>` of location `loc`: its value at the read-back
    /// point, `old` ([`Run::note_pre`]), once.
    fn set_pre(&mut self, loc: Loc, old: ValueId) {
        let v = self.v;
        let f = self.loc_field(loc);
        let name = crate::val::field_name(v, f);
        let pl = self.loc_of(v.field(&format!("wpre:{name}")));
        if self.st.defined_after(pl, 0) {
            return;
        }
        if crate::exec::obs_flag!("PHITEX_AUX_TRACE") {
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
    #[inline]
    pub(crate) fn code(&mut self, table: usize, c: impl TryInto<u32>) -> R<i32> {
        let c = c.try_into().unwrap_or(u32::MAX);
        if self.xe && table == 2 {
            // (XeTeX keeps `\\XeTeXcharclass` in the code's high half:
            // `sf_code(c) mod "10000` is the space factor code)
            return Ok(self.code_slow(2, c)? % 0x1_0000);
        }
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
    pub(crate) fn code_slow(&mut self, table: usize, c: u32) -> R<i32> {
        let l = if let Some(s) = crate::fields::code_slot(table, c) {
            Loc::try_from(s).unwrap_or(0)
        } else {
            let name = ["lc", "uc", "sf", "mc", "dc"][table];
            let f = self.v.field(&format!("{name}:{c}"));
            self.loc_of(f)
        };
        if self.xe && c >= 256 {
            // (XeTeX's INITEX, xetex.web §232: `\sfcode` 1000, the math
            // code the character itself, `\delcode` -1, no case codes)
            let d = match table {
                2 => 1000,
                3 => c.cast_signed(),
                4 => -1,
                _ => 0,
            };
            return Ok(self
                .get_loc(l)?
                .map_or(d, |x| i32::try_from(int_of(self.v, x)).unwrap_or(d)));
        }
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
    pub(crate) fn trace_read(&mut self, f: Field, parked: bool) {
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
        if let Some(want) = crate::exec::obs_str!("PHITEX_READS")
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

    /// Loop-back field `f` ([`crate::ops::lb_read`]): its last
    /// definition on the line, wherever this run is.
    pub(crate) fn lb(&mut self, f: Field) -> R<Option<ValueId>> {
        Ok(crate::ops::lb_read(self.cx, f)?)
    }

    /// Whether file `key`'s read-back field `f` has a value
    /// ([`Run::lb`]): none until the file's node defines it, nor while a
    /// file whose text the client gave is written as given
    /// ([`crate::ops::WSAME`]): that text is read back where it is read
    /// ([`Run::seed_written`]), as a run reads it before any value.
    pub(crate) fn lb_known(&mut self, key: &str, f: Field) -> R<bool> {
        let v = self.v;
        if self.lb(v.field(&format!("{}:{key}", crate::ops::WSAME)))?.and_then(|x| v.as_bool(x)) == Some(true) {
            return Ok(false);
        }
        Ok(self.lb(f)?.is_some())
    }

    /// The file `key` from the client (`file:<key>`): `None` if it is
    /// absent. A file the job writes and reads back is read from its
    /// loop-back fields ([`Run::lb`]).
    pub(crate) fn file(&mut self, key: &str) -> R<Option<ValueId>> {
        // (a file the job writes, read whole, `\\openin`'s lines, a size:
        // its text, `wtext:`, whatever its read-back is, per field or as
        // its text, which only a read-back reads; absent, as a lookup in
        // vain reads it, below)
        let v = self.v;
        let wt = crate::tools::text_field(v, key);
        let ws = crate::tools::size_field(v, key);
        if self.lb_known(key, ws)? {
            let k = self.keyed(crate::fields::Fam::Wkeys, key);
            let w = self.keyed(crate::fields::Fam::Wfile, key);
            if (self.lb_known(key, k)? || self.lb_known(key, w)?)
                && !self.tool_makes(key)?
                && let Some(t) = self
                    .read_written_text(key, wt)?
                    .filter(|&t| t != ValueId::UNIT)
            {
                return Ok(Some(v.list(vec![v.str(&format!("./{key}")), t])));
            }
        }
        // (a file the job writes and reads back per field: its read-back
        // reads no text, but a read of its bytes reads them all, `wtext:`)
        let x = self.file_lazy(key)?;
        let whole = self.written_per_field(key)?;
        let Some(x) = x else {
            return Ok(None);
        };
        let xs = list_of(v, x);
        if whole && let [path, _] = xs[..] {
            let seed = self.seeded(key).map(|sd| sd.text);
            let t = match seed {
                Some(t) if !self.lb_known(key, ws)? => Some(t),
                _ => self.read_written_text(key, wt)?,
            };
            let t = t
                .filter(|&t| t != ValueId::UNIT)
                .unwrap_or_else(|| v.str(""));
            return Ok(Some(v.list(vec![path, t])));
        }
        // (a file the job writes: its text read from its units here)
        if let [path, t, order] = xs[..]
            && t == ValueId::UNIT
        {
            let text = self.written_text(order)?;
            return Ok(Some(v.list(vec![path, text])));
        }
        Ok(Some(x))
    }

    /// Whether `key` is a file the job writes that is read back per
    /// field (its text not read by the read-back) and no tool makes.
    pub(crate) fn written_per_field(&mut self, key: &str) -> R<bool> {
        let k = self.keyed(crate::fields::Fam::Wkeys, key);
        if !self.lb_known(key, k)? {
            let mark = crate::api::text_mark(self.v);
            return Ok(self.seeded(key).is_some_and(|s| s.keys != mark) && !self.tool_makes(key)?);
        }
        Ok(self.per_field(key, k)?.is_some() && !self.tool_makes(key)?)
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

    /// File `key` as a document's `\input` reads it: `[path, 1]` when
    /// the client cut it as it cuts the document's text (`dfile:<key>`,
    /// `api::DocFile`), so this run reads not its text, its pieces the
    /// cut's slices; `None` (the reader reads the text) for a file the
    /// job writes or one not cut.
    pub(crate) fn read_cut(&mut self, key: &str) -> R<Option<ValueId>> {
        let v = self.v;
        if self.get(&format!("wopen:{key}"))?.is_some() {
            return Ok(None);
        }
        let w = self.keyed(crate::fields::Fam::Wfile, key);
        let k = self.keyed(crate::fields::Fam::Wkeys, key);
        // (read as [`Run::file`] reads them: once the file's node defines
        // them this runs again)
        if self.lb_known(key, k)? || self.lb_known(key, w)? {
            return Ok(None);
        }
        // (a file the job writes whose text the client gave: read back,
        // or read whole, not as a document's units, [`Run::seed_written`])
        if self.seeded(key).is_some() || self.seed_text(key)?.is_some() {
            return Ok(None);
        }
        // (a tool's output is read whole, [`Run::file`])
        if self.tool_makes(key)? {
            return Ok(None);
        }
        if crate::exec::obs_flag!("PHITEX_TMP_FTRACE") {
            eprintln!("TMPF units {key} node {}", self.cx.node().raw());
        }
        let Some(x) = self.cx.env(v.field(&format!("dfile:{key}")))? else {
            return Ok(None);
        };
        if list_of(v, x).get(1).is_none_or(|&s| int_of(v, s) == 0) {
            return Ok(None);
        }
        self.muts += 1;
        self.fread.push((key.to_owned(), x.raw()));
        self.uread.push(key.to_owned());
        Ok(Some(x))
    }

    /// A written file's unit's text (`wunit:`, a loop-back `ops::Files`
    /// defines).
    pub(crate) fn unit_text(&mut self, key: ValueId) -> R<String> {
        let v = self.v;
        let name = str_of(v, key);
        let f = v.field(&name);
        let x = self.lb(f)?;
        if crate::exec::obs_flag!("PHITEX_ROUND_TRACE") {
            eprintln!(
                "round-read: {name} {:016x} node {}",
                x.map_or(0, ValueId::raw),
                self.cx.node().raw()
            );
        }
        Ok(x.map(|x| str_of(v, x)).unwrap_or_default())
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
            text.push_str(&self.unit_text(k)?);
        }
        Ok(v.str(&text))
    }

    /// A written file on the input stack read by this run's own lines:
    /// its text from its units ([`FileRest::written`]).
    pub(crate) fn load_written(&mut self, l: crate::input::Level) -> R<()> {
        let v = self.v;
        // (a cut file read here: its text, as a file not cut is read)
        if let Some(key) = self.input.file(l).cut {
            let text = match self.read_file(&str_of(v, key))? {
                Some(x) => list_of(v, x).get(1).copied().unwrap_or_else(|| v.str("")),
                None => v.str(""),
            };
            self.input.file_mut(l).set_text(v, text);
            return Ok(());
        }
        let Some(units) = self.input.file(l).units.clone() else {
            return Ok(());
        };
        let mut text = String::new();
        for &(k, _) in units.iter() {
            text.push_str(&self.unit_text(k)?);
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
            let key = xs.get(1).copied().unwrap_or(ValueId::UNIT);
            let text = self.unit_text(key)?;
            let name = xs.get(2).copied().unwrap_or(ValueId::UNIT);
            let starts = xs.get(3).is_some_and(|&s| int_of(v, s) != 0);
            // (its lines' places from the unit's start)
            let a = slice_args_in_file(v, &text, starts, name, 0, key);
            frames.extend(a.first().map(|&x| list_of(v, x)).unwrap_or_default());
        }
        let mut out = args.to_vec();
        out[0] = v.list(frames);
        Ok(out)
    }

    /// (measure) `PHITEX_READS`: a stop that is not a field read's.
    pub(crate) fn trace_stop(&self, what: &str) {
        if crate::exec::obs_flag!("PHITEX_READS") {
            eprintln!(
                "reads: node {} op {:?} {what} PARKED",
                self.cx.node().raw(),
                self.cx.op()
            );
        }
    }

    /// The text of file `key`, one the job writes
    /// (`wt`, [`crate::tools::text_field`]); where only its size is read
    /// ([`Run::size_read`]), its size instead
    /// ([`crate::tools::size_field`], an integer), so that a lookup's
    /// size depends on the size and not on every byte written.
    fn read_written_text(&mut self, key: &str, wt: Field) -> R<Option<ValueId>> {
        let f = if self.size_read {
            crate::tools::size_field(self.v, key)
        } else {
            wt
        };
        if crate::exec::obs_flag!("PHITEX_READS") {
            self.trace_read(f, false);
        }
        self.lb(f)
    }

    /// File `key`, one the job writes whose text the client gave, looked
    /// up while its loop-backs are not declared ([`Run::file_inner`]):
    /// `None` for another file.
    #[allow(clippy::option_option)]
    fn seeded_lookup(&mut self, key: &str) -> R<Option<Option<ValueId>>> {
        let v = self.v;
        if self.seeded(key).is_some() {
            let reread = self.seed_reread(key)?;
            let sd = self.seeded(key).expect("seeded");
            let path = v.str(&format!("./{key}"));
            return Ok(Some(Some(if sd.keys == crate::api::text_mark(v) || reread {
                v.list(vec![path, sd.text])
            } else {
                v.list(vec![path, v.str("")])
            })));
        }
        // (looked up where it is not read back, an `\\openin` say: its
        // text, the client's, the round's whole text of the file,
        // `wtext:`, once its file's node declares that)
        let Some(t) = self.seed_text(key)? else {
            return Ok(None);
        };
        let wt = crate::tools::text_field(v, key);
        let loopback = self.lb_known(key, crate::tools::size_field(v, key))?;
        let x = self.read_written_text(key, wt)?;
        let t = if loopback {
            x.unwrap_or(ValueId::UNIT)
        } else {
            t
        };
        Ok(Some(
            (t != ValueId::UNIT).then(|| v.list(vec![v.str(&format!("./{key}")), t])),
        ))
    }

    fn file_inner(&mut self, key: &str) -> R<Option<ValueId>> {
        let v = self.v;
        // a file the job writes: its in-memory value (`wfile:<key>`, a
        // loop-back the writer publishes), never the client's file
        let w = self.keyed(crate::fields::Fam::Wfile, key);
        let f = self.keyed(crate::fields::Fam::File, key);
        let k = self.keyed(crate::fields::Fam::Wkeys, key);
        // (neither defined yet: both read, so the file's node defining
        // them runs this again on them)
        let (known_k, known_w) = (self.lb_known(key, k)?, self.lb_known(key, w)?);
        if !known_k && !known_w {
            // (a file the job writes whose text the client gave: read
            // back from it here, as from its loop-backs once they are in
            // effect, [`Run::seed_written`])
            if let Some(x) = self.seeded_lookup(key)? {
                return Ok(x);
            }
        }
        // (a name the job writes that a tool also makes from a file the
        // job writes: the tool's output, as latexmk's tool run after the
        // TeX run that wrote both replaces what the job wrote there)
        let made = if known_k || known_w {
            self.tool_output(key)?
        } else {
            None
        };
        let per_field = if made.is_some() {
            None
        } else {
            self.per_field(key, k)?
        };
        if crate::exec::obs_flag!("PHITEX_ROUND_TRACE") && known_k {
            let (kx, wx) = (self.lb(k)?, self.lb(w)?);
            eprintln!(
                "round-read: wkeys:{key} {:016x} wfile:{key} {:016x} tool {} node {}",
                kx.map_or(0, ValueId::raw),
                wx.map_or(0, ValueId::raw),
                made.is_some(),
                self.cx.node().raw()
            );
        }
        let x = if made.is_some() {
            made
        } else if let Some(keys) = per_field {
            // a file whose read-back only defines: it exists (empty) once
            // the job wrote it ([`Run::read_back`])
            if keys.is_some_and(|x| !list_of(v, x).is_empty()) {
                Some(v.list(vec![v.str(&format!("./{key}")), v.str("")]))
            } else {
                None
            }
        } else if known_w {
            // (its units' order, `ops::Files`; none: absent)
            let x = self.lb(w)?;
            if crate::exec::obs_flag!("PHITEX_AUX_TRACE") {
                eprintln!(
                    "aux-trace: read wfile {key}: {:?} units",
                    x.map(|x| list_of(v, x).get(1).map_or(0, |&l| list_of(v, l).len()))
                );
            }
            x.filter(|&x| !list_of(v, x).is_empty())
                .map(|o| v.list(vec![v.str(&format!("./{key}")), ValueId::UNIT, o]))
        } else {
            if crate::exec::obs_flag!("PHITEX_TMP_FTRACE") {
                eprintln!(
                    "TMPF file {key} node {} op {:?}",
                    self.cx.node().raw(),
                    self.cx.op()
                );
            }
            // (a name a tool makes from a file the job writes: that
            // tool's output, `crate::tools`; else the file system's)
            match self.tool_output(key)? {
                Some(x) => Some(x),
                None => self.cx.env(f)?,
            }
        };
        let x = x.filter(|&x| !crate::val::list_of(v, x).is_empty());
        // (a file looked for in vain: where it would be read back, its
        // next round's lines are read, [`Run::note_read_point`]; one this
        // job writes and reads back per field is known here, another may
        // be opened for writing by this run later, [`Run::opened_for_writing`])
        if x.is_none() && self.interp.is_none() && !self.writer_lookup {
            if per_field.is_some() {
                self.note_read_point();
                self.mark_read_point(key)?;
            } else if self.read_point_here && self.read_point_pending.is_none() {
                // (the run is a read-back point already: the values the
                // lines written to this file read are kept from there)
                if !self.vain_after_point.iter().any(|k| k == key) {
                    self.muts += 1;
                    self.vain_after_point.push(key.to_owned());
                }
            } else if !self.read_back_done || self.read_point_pending.is_some() {
                self.note_read_point();
                if self.read_back_done {
                    self.muts += 1;
                    self.read_point_pending = Some(key.to_owned());
                }
            }
        }
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
        } else if (self.rec_load || self.doc) && !self.fmiss.iter().any(|n| n == key) {
            self.fmiss.push(key.to_owned());
        }
        Ok(x)
    }

    /// File `key` as a tool makes it, where one does ([`crate::tools`]):
    /// the output a `\write18`'s tool node before here made of it
    /// ([`crate::tools::out_field`]), else what a tool makes of it by
    /// default from a file the job writes, defined by that file's node
    /// ([`crate::tools::made_field`], `[path, text]`). `None`: no tool
    /// makes it, or the run made none.
    fn tool_output(&mut self, key: &str) -> R<Option<ValueId>> {
        // (through the run's state: a command this run made before here
        // defined it, `crate::shell`)
        if let Some(x) = self.get(&crate::tools::out_name(key))? {
            return Ok((x != ValueId::UNIT).then_some(x));
        }
        let x = self.lb(crate::tools::made_field(self.v, key))?;
        Ok(x.filter(|&x| x != ValueId::UNIT))
    }

    /// Whether a tool makes file `key` ([`Run::tool_output`]): its text is
    /// then not the file system's units.
    fn tool_makes(&mut self, key: &str) -> R<bool> {
        if let Some(x) = self.get(&crate::tools::out_name(key))? {
            return Ok(x != ValueId::UNIT);
        }
        Ok(self.lb(crate::tools::made_field(self.v, key))?.is_some())
    }

    /// A `\write18` naming a tool (`[tool, output, input, [args...]]`,
    /// [`crate::tools::ToolNode`]): a node placed where the command is.
    /// A piece of the main list ends here, its rest a resume placed after
    /// the node, and the node after the pages firing in the piece before
    /// it, whose `\write`s come before the command (`crate::pages`, the
    /// resume's arguments); a later read of the output
    /// in the rest reads the node's. Any other run (or a piece with
    /// nothing for the main list, or nothing left to read) places the
    /// node as its child, for the reads after the run.
    pub(crate) fn tool_command(&mut self, args: Vec<ValueId>) -> R<()> {
        let op = self.cx.op();
        let piece = (op == self.ops.exec || op == self.ops.resume) && (self.top || self.doc);
        if piece
            && self.tool_cut.is_none()
            && self.has_input()
            && !self.own_outer_items().is_empty()
        {
            self.tool_cut = Some(self.v.list(args));
            self.cut = Some(self.cand);
            return Err(Halt::Cut);
        }
        let op = self.ops.toolnode;
        self.kid(op, args);
        Ok(())
    }

    /// A `streams` entry (`[name]` opens the file empty, `[name, text]`
    /// adds to it, `[name, "", "close"]` closes it) where TeX makes it
    /// ([`crate::api::doc_order`]): from a page step, tagged with its
    /// page (`[piece, nth]`, the piece it fired in and the pages fired in
    /// it before it) and, for a page whose break is inside a piece, that
    /// break (`[page, entry]`, `[page, entry, piece, candidate]`); from a
    /// piece after one of its page-builder candidates, after a marker
    /// `["cand", piece, candidates passed]`, so that a page firing at an
    /// earlier candidate of the piece writes before it (§1370, §1374).
    /// From a document node, also a `T_WRITE_AT` marker on the main list.
    /// The entries also go to the file's stream accumulator
    /// ([`stream_bucket`], read by that file's node alone, `ops::Files`)
    /// and the file's name to the names written ([`WNAMES`]).
    pub(crate) fn contribute_stream(&mut self, e: ValueId) {
        let v = self.v;
        let op = self.cx.op();
        let (marker, tagged) = if op == self.ops.finish {
            // (the page: its piece and the pages fired in it before it,
            // `crate::api::doc_order`)
            let anchor = self
                .fin_page()
                .map_or(-1, |f| i64::from_ne_bytes(f.at.raw().to_ne_bytes()));
            let page = v.list(vec![v.int(anchor), v.int(self.page_nth())]);
            let mut t = vec![page, e];
            // (the piece its break is inside of and the candidate: what
            // the piece wrote after that candidate comes after the page's
            // writes)
            if let Some((pid, k, true)) = self.fin.as_ref().and_then(|c| c.brk) {
                t.extend([v.int(pid), v.int(k)]);
            }
            (None, v.list(t))
        } else if (op == self.ops.exec || op == self.ops.resume) && self.cand > 0 {
            let pid = i64::from_ne_bytes(self.cx.node().raw().to_ne_bytes());
            let m = v.list(vec![
                v.str(crate::api::CAND),
                v.int(pid),
                v.int(i64::from(self.cand)),
            ]);
            (Some(m), e)
        } else {
            (None, e)
        };
        let entries = match marker {
            Some(m) => v.seq_concat(v.seq_unit(m), v.seq_unit(tagged)),
            None => v.seq_unit(tagged),
        };
        self.contribute("streams", entries);
        if let Some(&n) = list_of(v, e).first() {
            let f = stream_bucket(v, &str_of(v, n));
            self.muts += 1;
            self.contribs.push((f, entries));
            self.contribute(WNAMES, v.seq_unit(n));
        }
        // (a routine's writes, made at its `\shipout`, are on no list:
        // what it gives back to the main list holds none, §1374)
        if op != self.ops.finish {
            let at = v.list(vec![v.int(crate::nodes::T_WRITE_AT), e]);
            self.contribute("items", v.seq_unit(v.list(vec![v.int(0), at])));
        }
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
        if crate::exec::obs_flag!("PHITEX_PAGE_TRACE") {
            eprintln!(
                "page-trace: node {} op {:?} params marker vsize {vsize} topskip {}",
                self.cx.node().raw(),
                self.cx.op(),
                topskip.width
            );
        }
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
    pub(crate) fn resolve_page_params(&mut self, raw: &mut [ValueId], reaching: bool) -> R<()> {
        let v = self.v;
        let mut full = None;
        let mut classes: Vec<(u16, ValueId)> = Vec::new();
        for x in raw.iter_mut() {
            let tag = crate::nodes::tag(v, *x);
            if tag == crate::nodes::T_PAGE_PARAMS && crate::val::list_of(v, *x).len() == 1 {
                if !reaching {
                    continue;
                }
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

    /// Whether file `name` is one the job writes and reads back, its
    /// fields defined or its text the client's.
    pub(crate) fn written_known(&mut self, name: &str) -> R<bool> {
        let w = self.keyed(crate::fields::Fam::Wfile, name);
        let k = self.keyed(crate::fields::Fam::Wkeys, name);
        Ok(self.lb_known(name, w)? || self.lb_known(name, k)? || self.seeded(name).is_some())
    }

    /// Whether the lines this run writes to `name` are interpreted as
    /// they are written ([`Run::interpret_line`]): a file is read back per
    /// field while every line written to it only defines, which a line is
    /// guessed to do until this run wrote one that does more. The guess
    /// is the first round's too, before any file node declared the file's
    /// loop-backs: the definitions go with the lines, the file node
    /// publishes them, and the next round reads them, as TeX's next run
    /// reads the file this one wrote.
    pub(crate) fn reads_back_per_field(&self, name: &str) -> bool {
        !self.text_written.iter().any(|n| n == name)
    }

    /// Whether `name`'s read-back is per field this round: its `wkeys:`
    /// loop-back declared and not marked as read back as text
    /// ([`crate::api::text_mark`]). The key set read, its value bound.
    #[allow(clippy::option_option)]
    fn per_field(&mut self, key: &str, k: Field) -> R<Option<Option<ValueId>>> {
        if !self.lb_known(key, k)? {
            return Ok(None);
        }
        let x = self.lb(k)?;
        Ok((x != Some(crate::api::text_mark(self.v))).then_some(x))
    }

    /// The read-back point of a file whose read-back only defines (where
    /// the macro on top reads it back, or looks for it in vain): the
    /// fields this macro defines after it keep their value here for the
    /// lines written later ([`Run::note_pre`]). A file looked for in vain
    /// marks it too: the lines this run writes are read back next round
    /// at this point, under the meanings in force here.
    fn note_read_point(&mut self) {
        self.read_point_pending = None;
        self.pending_pre.clear();
        self.pending_locs.clear();
        self.read_back_at = self
            .input
            .levels
            .iter()
            .find(|l| l.kind == crate::input::kind::TOKS && l.ty == crate::input::ty::MACRO)
            .copied();
        self.read_back_done = self.read_back_at.is_some();
    }

    /// File `name` opened for writing by a run that looked for it in vain
    /// from a macro still running: that lookup was its read-back point
    /// (a macro that looks for the file it then writes: the first round's
    /// lines are read back at that point next round).
    fn note_absent_read_point(&mut self, name: &str) -> R<()> {
        if let Some(i) = self.vain_after_point.iter().position(|k| k == name) {
            self.vain_after_point.swap_remove(i);
            self.muts += 1;
            return self.mark_read_point(name);
        }
        if self.read_point_pending.as_deref() != Some(name) {
            return Ok(());
        }
        self.read_point_pending = None;
        self.pending_locs.clear();
        for (loc, old) in std::mem::take(&mut self.pending_pre) {
            self.set_pre(loc, old);
        }
        self.mark_read_point(name)
    }

    /// This run's node is where file `key` is read back (or looked for
    /// in vain, the file written later): `wpoint:<key>`, its node, which
    /// the lines written to the file read the state at
    /// ([`Run::pre_value`]). The first such point reaching here stands
    /// (a file read back again later, to compare, is no new point).
    pub(crate) fn mark_read_point(&mut self, key: &str) -> R<()> {
        if self.interp.is_some() {
            return Ok(());
        }
        let f = format!("wpoint:{key}");
        if self.get(&f)?.is_some() {
            return Ok(());
        }
        let n = self.cx.node().raw();
        if crate::exec::obs_flag!("PHITEX_AUX_TRACE") {
            eprintln!("aux-trace: read-back point of {key}: node {n}");
        }
        self.set(&f, self.v.int(i64::from_ne_bytes(n.to_ne_bytes())), true);
        self.read_point_here = true;
        self.read_back_done = true;
        Ok(())
    }

    /// File `key`'s lines read back here (per field or as its text, not
    /// a lookup): a second such read before the file is opened for
    /// writing (by this run, or by a piece of the file's text read as
    /// its text the read before placed) reads the lines again, under the
    /// meanings there, so the file is read back as its text ([`REREAD`]).
    pub(crate) fn note_read_back(&mut self, key: &str) -> R<()> {
        if self.interp.is_some() {
            return Ok(());
        }
        let me = self
            .v
            .int(i64::from_ne_bytes(self.cx.node().raw().to_ne_bytes()));
        let f = format!("wread:{key}");
        if self.get(&f)?.is_some()
            && self.get(&format!("wopen:{key}"))?.is_none()
            && self.get(&format!("{REREAD}{key}"))?.is_none()
        {
            self.set(&format!("{REREAD}{key}"), self.v.int(1), true);
        } else if self.get(&f)?.is_none() {
            self.set(&f, me, true);
        }
        Ok(())
    }

    /// The read-back point of file `name` reaching here
    /// ([`Run::mark_read_point`]).
    pub(crate) fn read_point_of(&mut self, name: &str) -> R<Option<phitex_defuse::NodeId>> {
        let v = self.v;
        Ok(self
            .get(&format!("wpoint:{name}"))?
            .and_then(|x| v.as_int(x))
            .map(|n| phitex_defuse::NodeId::from_raw(u64::from_ne_bytes(n.to_ne_bytes()))))
    }

    /// `\input` of a file whose read-back only defines: the
    /// definitions its lines make, from the per-field loop-backs
    /// `wdef:<field>` the writer publishes for the fields listed in
    /// `wkeys:<key>`; nothing of the file's text is read. Whether `key`
    /// is such a file.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn read_back(&mut self, key: &str) -> R<bool> {
        let v = self.v;
        let wk = self.keyed(crate::fields::Fam::Wkeys, key);
        // (read back from the client's text in this run, its fields not
        // defined yet or the file written as given)
        let seed = if self.lb_known(key, wk)? {
            None
        } else {
            self.seeded(key).cloned()
        };
        if seed.is_some() && self.seed_reread(key)? {
            self.note_read_back(key)?;
            return Ok(false);
        }
        let keys = match &seed {
            Some(sd) if sd.keys == crate::api::text_mark(v) => return Ok(false),
            Some(sd) => list_of(v, sd.keys),
            None => {
                let Some(keys) = self.per_field(key, wk)? else {
                    return Ok(false);
                };
                keys.map(|x| list_of(v, x)).unwrap_or_default()
            }
        };
        for k in keys {
            let k = str_of(v, k);
            if k == crate::api::WRITTEN_KEY {
                continue;
            }
            let id = crate::api::wdef_id(key, &k);
            let name = crate::api::wdef_field(&id).to_owned();
            let x = if let Some(sd) = &seed {
                match sd.defs.iter().find(|(i, _)| *i == id) {
                    Some(&(_, x)) if x != ValueId::UNIT => x,
                    _ => continue,
                }
            } else {
                let wd = self.keyed(crate::fields::Fam::Wdef, &id);
                // (a key the file's node has not defined yet: read, so
                // this runs again when it does)
                let Some(x) = self.lb(wd)? else {
                    continue;
                };
                // (`PHITEX_KEYS_SHOW=part`, profile: the value a read-back
                // of a key whose id has `part` finds, by node)
                if cfg!(feature = "profile")
                    && crate::exec::env_str!("PHITEX_KEYS_SHOW").is_some_and(|p| id.contains(p))
                {
                    let xs = list_of(v, x);
                    eprintln!(
                        "keys-read: {id} = {} node {}",
                        xs.first().map_or_else(|| "(none: its initial value)".to_owned(), |&d| crate::val::show_def(v, d)),
                        self.cx.node().raw()
                    );
                }
                x
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
                    if crate::exec::obs_flag!("PHITEX_AUX_TRACE") {
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
                self.read_back_pre(key, &name, *val)?;
                self.set(&name, *val, global);
                if self.interp.is_some() {
                    self.interp_nested.insert(name, id);
                }
            }
        }
        self.note_read_point();
        self.mark_read_point(key)?;
        self.note_read_back(key)?;
        if crate::exec::obs_flag!("PHITEX_AUX_TRACE") {
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

    /// The value the read-back of `name` (`val`, from file `key`)
    /// replaces, for the lines written this round
    /// ([`Run::revert_read_back`]): kept once per run (a file read back
    /// again later in the run keeps the first read-back point's).
    fn read_back_pre(&mut self, key: &str, name: &str, val: ValueId) -> R<()> {
        let v = self.v;
        let pre = format!("wpre:{name}");
        if self.get(&pre)?.is_none() {
            let was = self.get(name)?;
            if crate::exec::obs_flag!("PHITEX_AUX_TRACE") {
                eprintln!(
                    "aux-trace: read-back {key} {name} = {} (was {:?})",
                    crate::val::show_def(v, val),
                    was.map(|x| crate::val::show_def(v, x))
                );
            }
            self.set(&pre, was.unwrap_or(ValueId::UNIT), true);
        }
        Ok(())
    }

    /// File `key`, one the job writes and has not declared yet (round
    /// one), read back from the text the client gave for it as the build
    /// began (`wseeds`, [`crate::Options::written_init`]: the file an
    /// earlier run left), as TeX's next run reads the file the run before
    /// left: here, where it is read back, under the state here, its lines
    /// interpreted as the lines the job writes are ([`Run::interpret_line`]):
    /// per field while every line only defines, else as its text. Its
    /// loop-backs are declared with those values as their round-one values
    /// (in effect once the evaluation is quiet: this run, bound to them,
    /// runs again on them and reads them back as it reads them here), so a
    /// round that writes the file as it was read is the last. Whether
    /// `key` is so read back in this run ([`Run::seeded`]).
    #[allow(clippy::too_many_lines)]
    pub(crate) fn seed_written(&mut self, key: &str) -> R<bool> {
        if self.seeded.iter().any(|s| s.name == key) {
            return Ok(true);
        }
        // (a file this run opened for writing before here is read as it
        // wrote it so far, `Run::read_file`: LaTeX's `\enddocument` reading
        // the `.aux` back to compare its labels)
        if self.get(&format!("wopen:{key}"))?.is_some() {
            return Ok(false);
        }
        let v = self.v;
        let k = self.keyed(crate::fields::Fam::Wkeys, key);
        let w = self.keyed(crate::fields::Fam::Wfile, key);
        // (read: this runs again once the file's node finds the file
        // written otherwise)
        if self.lb_known(key, k)? || self.lb_known(key, w)? {
            return Ok(false);
        }
        let Some(t) = self.seed_text(key)? else {
            return Ok(false);
        };
        let text = str_of(v, t);
        // (the whole text at once, as a run reads it: a line reads what
        // the lines before it defined, hyperref's `\calc@bm@number`
        // counting a bookmark's children line by line; one that does more
        // than define makes the file one read back as its text)
        let mut ds: Vec<(String, ValueId)> = Vec::new();
        let mut as_text = false;
        let mut open = String::new();
        match self.interpret_line(&text)? {
            None => open.clone_from(&text),
            Some(m) if self.interp_plain => crate::api::merge_defs(v, &mut ds, m),
            Some(_) => as_text = true,
        }
        // (a text left open at the file's end: read to its end as the
        // stream's close reads it, its errors given at each read-back)
        if !as_text && !open.is_empty() {
            self.interp_eof = true;
            self.interp_errs.clear();
            let made = self.interpret_line(&open);
            self.interp_eof = false;
            let errs = std::mem::take(&mut self.interp_errs);
            match made? {
                Some(m) => crate::api::merge_defs(v, &mut ds, m),
                None => as_text = true,
            }
            if !errs.is_empty() {
                let e = v.list(vec![
                    v.str(&format!("rberr:{key}")),
                    v.list(errs.iter().map(|e| v.str(e)).collect()),
                    v.int(1),
                ]);
                crate::api::merge_defs(v, &mut ds, v.list(vec![e]));
            }
        }
        // (what this read back: its file's node declares its loop-backs
        // with it, `crate::ops::file_node`, from the runs that hold, a
        // run refuted or run again leaving nothing of it)
        let keys = if as_text {
            ds.clear();
            crate::api::text_mark(v)
        } else {
            crate::api::key_set(v, key, &ds)
        };
        if crate::exec::obs_flag!("PHITEX_AUX_TRACE") {
            eprintln!(
                "aux-trace: {key} read back from the client's text in node {}: {} bytes, {}",
                self.cx.node().raw(),
                text.len(),
                if as_text {
                    "as its text".to_owned()
                } else {
                    format!("{} definitions", ds.len())
                }
            );
        }
        self.muts += 1;
        self.seeded.push(Seeded {
            name: key.to_owned(),
            keys,
            defs: ds
                .into_iter()
                .map(|(f, x)| (crate::api::wdef_id(key, &f), x))
                .collect(),
            text: t,
        });
        Ok(true)
    }

    /// Whether file `key`, read back from the client's text per field
    /// before here, is read again before it is opened for writing: then
    /// its lines are read again under the meanings here, as its text
    /// (hyperref's `\ReadBookmarks`, the `.out` read twice, its outlines
    /// made the second time), as [`Run::note_read_back`] makes a file
    /// read back from what the job wrote.
    fn seed_reread(&mut self, key: &str) -> R<bool> {
        Ok(self.seeded(key).is_some_and(|s| s.keys != crate::api::text_mark(self.v))
            && self.get(&format!("wread:{key}"))?.is_some()
            && self.get(&format!("wopen:{key}"))?.is_none())
    }

    /// The read-back this run made of file `key` from the client's text
    /// ([`Run::seed_written`]), while its loop-backs are not in effect.
    fn seeded(&self, key: &str) -> Option<&Seeded> {
        self.seeded.iter().find(|s| s.name == key)
    }

    /// The text the client gave for file `key`, one the job writes, as
    /// the build began (`wseeds`): `None` for another file, or outside a
    /// fixpoint, or while a written line is interpreted.
    pub(crate) fn seed_text(&mut self, key: &str) -> R<Option<ValueId>> {
        if self.interp.is_some() {
            return Ok(None);
        }
        let v = self.v;
        let Some(names) = self.cx.read(v.field(SEEDS))? else {
            return Ok(None);
        };
        if !list_of(v, names).iter().any(|&n| str_of(v, n) == key) {
            return Ok(None);
        }
        if self
            .cx
            .read(v.field("loopfiles"))?
            .is_none_or(|x| x == ValueId::UNIT)
        {
            return Ok(None);
        }
        Ok(self.cx.read(v.field(&format!("{SEEDS}:{key}")))?)
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
            .defined_since_in_order(v, base, gids)
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
    #[allow(clippy::too_many_lines, reason = "one scratch interpretation: its save, run and restore read as one")]
    pub(crate) fn interpret_line(&mut self, line: &str) -> R<Option<ValueId>> {
        let v = self.v;
        let input = std::mem::replace(&mut self.input, crate::input::Input::new());
        // (the table kept as it changes and put back after, not copied
        // whole for each line: a copy where it keeps changes already)
        let st = (!self.st.checkpoint()).then(|| self.st.clone());
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
        self.interp_first.clear();
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
                chars: false,
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
            // (a line that does more than define is read back as text:
            // its interpretation stops at the first command that does,
            // its definitions not wanted)
            // (a group opened in the line shares its list: only a level
            // with a list of its own, or an item on any, is more; a group
            // still open at the line's end is, below)
            if self.contributed_since(nc)
                || self.kids.len() != nk
                || self.nest.len() <= nest_len
                || self.nest[nest_len + 1..].iter().any(|l| l.owns)
                || self.nest[nest_len..].iter().any(|l| !l.list.is_empty())
            {
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
        // (only definitions: nothing contributed, typeset or called, the
        // line's own list empty and closed, no conditional left open)
        self.interp_plain = res.is_ok()
            && !self.contributed_since(nc)
            && self.kids.len() == nk
            && self.nest.len() == nest_len + 1
            && self.nest.last().is_some_and(|l| l.list.is_empty())
            && self.conds == conds;
        if !self.interp_plain && crate::exec::obs_flag!("PHITEX_AUX_TRACE") {
            self.trace_more_than_define(res.is_ok(), (nc, nk, nest_len), &conds);
        }
        // (what the line read, by name: a line that reads what an earlier
        // line of its file defines depends on the lines before it, which
        // its read-back by fields does not see)
        self.interp = None;
        let mut reads: Vec<String> = std::mem::take(&mut self.interp_first)
            .into_iter()
            .map(|l| str_of(v, self.loc_field(l).0))
            .filter(|n| is_read_back_definition(n))
            .collect();
        reads.sort_unstable();
        reads.dedup();
        self.interp_reads = v.list(reads.iter().map(|n| v.str(n)).collect());
        self.input = input;
        match st {
            // (the outer line's kept changes go on: what this line
            // changed it noted, and the copy puts back)
            Some(st) => {
                let u = self.st.take_undo();
                self.st = st;
                self.st.set_undo(u);
            }
            None => self.st.rollback(),
        }
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

    /// (observe) `PHITEX_AUX_TRACE`: what made the line just interpreted
    /// more than definitions ([`Run::interpret_line`]), against the
    /// contributions, kids and levels it began with (`at`) and the
    /// conditionals open then.
    fn trace_more_than_define(&self, ok: bool, at: (usize, usize, usize), conds: &[u8]) {
        let (nc, nk, nest_len) = at;
        let v = self.v;
        let log = v.field("log");
        let what: Vec<String> = self
            .contribs
            .get(nc..)
            .unwrap_or_default()
            .iter()
            .filter(|&&(f, _)| f != log)
            .map(|&(f, x)| {
                format!(
                    "{} {}",
                    crate::val::field_name(v, f),
                    v.render(x).chars().take(80).collect::<String>()
                )
            })
            .collect();
        eprintln!(
            "aux-trace: the line does more than define: ran to its end {ok}, contributed {what:?}, kids {}, levels {:?} (list {}), conditionals {:?} (were {conds:?})",
            self.kids.len().saturating_sub(nk),
            self.nest.len().checked_sub(nest_len + 1),
            self.nest.last().map_or(0, |l| l.list.len()),
            self.conds,
        );
    }

    /// Whether the run contributed since its `n`th contribution anything
    /// but lines of the log: what a written line writes to the log as it
    /// is read back is no output of the job's the line's definitions
    /// stand for (the log need not match), so such a line still only
    /// defines ([`Run::interpret_line`]).
    fn contributed_since(&self, n: usize) -> bool {
        let log = self.v.field("log");
        let errors = self.v.field(LOG_ERRORS);
        self.contribs
            .get(n..)
            .is_some_and(|c| c.iter().any(|&(f, _)| f != log && f != errors))
    }

    /// File `name` is opened for writing: a lookup of it in vain from
    /// the macro still running was its read-back point
    /// ([`Run::note_absent_read_point`]). Its readers read what its file
    /// node defines ([`crate::ops::Files`]).
    pub(crate) fn opened_for_writing(&mut self, name: &str) -> R<()> {
        if self.interp.is_none() {
            self.note_absent_read_point(name)?;
        }
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

    #[inline]
    fn font_loc_slow(&mut self, f: i64, c: crate::fields::FontCell) -> Loc {
        let fld = self
            .ops
            .fields
            .font_field(self.v, self.cx.values_epoch(), f, c);
        let l = self.loc_of(fld);
        self.st.note_font_loc(l);
        l
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
    /// changes, [`crate::state::State::cat_gen`]).
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
            if self.xe {
                // (XeTeX's INITEX: every character past 255 is other,
                // xetex.web §232; none has a field until set)
                self.get_loc(l)?
                    .map_or(12, |x| i32::try_from(int_of(self.v, x)).unwrap_or(12))
            } else {
                self.int_loc(l)?
            }
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



    /// The fields the run defines: those whose value at its end differs
    /// from the one it loaded, or that it never loaded (a local
    /// assignment that ends where it began defines nothing, as LaTeX's
    /// paragraph hooks setting `\\everypar` and back).
    pub(crate) fn def_fields(&mut self) -> Vec<Field> {
        self.publication().into_iter().map(|d| d.0).collect()
    }


    /// A page node whose declared contributions are all it contributes
    /// ([`crate::pages::PageNode::contribs_complete`]) contributes to no
    /// other field.
    ///
    /// # Panics
    ///
    /// Where it does: the engine passed it by in that field's windows.
    fn check_contribs_complete(&self) {
        let args = self.cx.args();
        let Some(k) = crate::pages::PageNode::of(self.ops, self.v, self.cx.op(), args) else {
            return;
        };
        if !k.contribs_complete() {
            return;
        }
        let declared: Vec<Field> = crate::pages::no_units(self.v)
            .into_iter()
            .map(|c| c.0)
            .collect();
        for (f, _) in &self.contribs {
            assert!(
                declared.contains(f),
                "node {} ({k:?}) contributes to {}, which its declaration leaves out (PageNode::contribs_complete)",
                self.cx.node().raw(),
                crate::val::field_name(self.v, *f)
            );
        }
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
        if kind == "error" || kind == "errmsg" {
            let one = self.v.int(1);
            self.contribute(LOG_ERRORS, one);
        }
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

    /// `print_err("OK")` and `error` after `\\showbox` or `\\showlists`
    /// (§1298): an error, counted for the exit status, never towards
    /// the hundred (`decr(error_count)` outside `error_stop_mode`; a
    /// build runs as `nonstop_mode` does).
    pub(crate) fn log_ok(&mut self) -> R<()> {
        self.diag(true, "OK");
        let nl = i64::from(self.ipar(Par::newlinechar)?);
        let e = self.v.list(vec![self.v.str("error"), self.v.str("OK"), self.v.int(nl)]);
        let s = self.v.seq_unit(e);
        self.contribute("log", s);
        let one = self.v.int(1);
        self.contribute(LOG_ERRORS, one);
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
        let (file, at, unit, seg, loc) = match &src {
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
                    Some((n, a, u)) => (n, i64::try_from(a).unwrap_or(0), u, seg, loc),
                    None => (ValueId::UNIT, -1, ValueId::UNIT, seg, loc),
                }
            }
            None => (ValueId::UNIT, -1, ValueId::UNIT, String::new(), 0),
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
            unit,
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
        // (the meaning defined is not read: [`Run::interp_first`] keeps
        // what the line read)
        self.defining = true;
        let l = self.mean_loc(t);
        self.defining = false;
        if let Some(l) = l {
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
    #[allow(clippy::inline_always)]
    #[inline(always)]
    fn set_cur_cs(&mut self, tok: Tok) -> R<()> {
        // (the common case: a control sequence whose meaning this run
        // has read and decoded: one slot access)
        if !self.bare
            && let Some(i) = tok.cs_index()
            && let Some((c, h)) = self.st.cmd_chr_fast(Loc::try_from(crate::fields::MEAN_BASE + i).unwrap_or(Loc::MAX))
        {
            self.cur = Cur {
                tok,
                cmd: c,
                chr: h,
            };
            return Ok(());
        }
        self.set_cur_cs_rest(tok)
    }

    /// [`Self::set_cur_cs`] past its common case.
    #[inline(never)]
    fn set_cur_cs_rest(&mut self, tok: Tok) -> R<()> {
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
        // (a deferred read's symbol: its meaning is its name's,
        // `crate::defer`, nothing read)
        if crate::defer::is_sym(tok) {
            self.cur = Cur {
                tok,
                cmd: cmd::SYM,
                chr: 0,
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
        // (a token of a skipped conditional branch, or one fetched only to
        // be stored or matched: of its meaning only the class
        // `pass_text`, a definition's parameter scan, an alignment's
        // template insertion and the outer check look at, §494, §476,
        // §479, §342, §336, [`crate::val::step_projection`]; not while a
        // line is interpreted or a load recorded, which read meanings in
        // full; an input marker, a file's end, read in full too)
        if (self.in_skip || self.step_reads)
            && self.interp.is_none()
            && !self.rec_load
            && !tok.is_internal_cs()
            && let Some(k) = self.step_lookup(tok)?
        {
            let code = |base: i64| i32::try_from(k - base).unwrap_or(0);
            let (c, h) = match k {
                k if k >= crate::val::STEP_CR => (cmd::CAR_RET, code(crate::val::STEP_CR)),
                k if k >= crate::val::STEP_TAB => (cmd::TAB_MARK, code(crate::val::STEP_TAB)),
                k if k >= crate::val::STEP_PARAM => (cmd::MAC_PARAM, code(crate::val::STEP_PARAM)),
                crate::val::STEP_IF => (cmd::IF_TEST, 0),
                crate::val::STEP_OUTER => (cmd::OUTER_CALL, 0),
                k if k >= crate::val::STEP_FI => (
                    cmd::FI_OR_ELSE,
                    i32::try_from(k - crate::val::STEP_FI).unwrap_or(0),
                ),
                _ => (cmd::RELAX, 256),
            };
            self.cur = Cur {
                tok,
                cmd: c,
                chr: h,
            };
            return Ok(());
        }
        // (a marker of the input's own, a file's end, is never stepped
        // over by its meaning's class: read in full)
        if self.outer_reads
            && !tok.is_internal_cs()
            && self.outer_lookup(tok)? == Some(false)
        {
            // (not outer: get_next checks nothing of it, §336, and the
            // caller keeps its token only)
            self.cur = Cur {
                tok,
                cmd: cmd::RELAX,
                chr: 256,
            };
            return Ok(());
        }
        // (the first token of an `\ifnum` operand's number scan: a
        // macro whose body is a number, kept as that macro's number, its
        // meaning not read, [`crate::scan::Num`])
        if self.num_reads
            && self.interp.is_none()
            && !self.rec_load
            && !tok.is_internal_cs()
            && self.num_lookup(tok)?
        {
            self.cur = Cur {
                tok,
                cmd: cmd::RELAX,
                chr: crate::scan::NUM_CHR,
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
        if self.st.is_loaded(l) || self.lazy_pre(l) {
            return Ok(None);
        }
        let m = self.memos.get(l);
        if m.has & crate::state::M_CLASS != 0 {
            return Ok(Some(m.class));
        }
        let f = self.loc_field(l);
        let Some(x) = self.cx.read_path(f, phitex_defuse::PathId::of_proj(self.ops.class_proj))? else {
            return Ok(None);
        };
        let Ok(c) = u8::try_from(int_of(self.v, x)) else {
            return Ok(None);
        };
        let e = self.memos.entry(l);
        e.has |= crate::state::M_CLASS;
        e.class = c;
        Ok(Some(c))
    }

    /// The class of `tok`'s meaning a token stepped over in a skipped
    /// conditional branch, or fetched only to be stored or matched, needs,
    /// read through the engine's step projection
    /// ([`crate::val::step_projection`]) where the run has not bound it
    /// itself: a redefinition within the class leaves this run standing.
    /// `None` where it is this run's own, or not defined (read in full).
    #[inline(never)]
    fn step_lookup(&mut self, tok: Tok) -> R<Option<i64>> {
        let Some(l) = self.mean_loc(tok) else {
            return Ok(None);
        };
        if self.st.is_loaded(l) || self.lazy_pre(l) {
            return Ok(None);
        }
        let m = self.memos.get(l);
        if m.has & crate::state::M_STEP != 0 {
            return Ok(Some(m.step));
        }
        let f = self.loc_field(l);
        let Some(x) = self.cx.read_path(f, phitex_defuse::PathId::of_proj(self.ops.step_proj))? else {
            return Ok(None);
        };
        let k = int_of(self.v, x);
        let e = self.memos.entry(l);
        e.has |= crate::state::M_STEP;
        e.step = k;
        Ok(Some(k))
    }

    /// Whether `l` is a finisher's field given where the routine reads
    /// it ([`crate::state::State::lazy`]): the run's own, read in full.
    fn lazy_pre(&self, l: Loc) -> bool {
        self.st.lazy(l).is_some()
    }

    /// Whether `tok`'s meaning is a macro whose body is a number
    /// ([`crate::val::num_projection`]), where the run has not bound it
    /// itself: read as that test alone (the number against -1), so the
    /// reading run depends on the number only where it compares it
    /// ([`Run::num_order`]) or uses it ([`Run::num_value`]).
    #[inline(never)]
    fn num_lookup(&mut self, tok: Tok) -> R<bool> {
        let Some(l) = self.mean_loc(tok) else {
            return Ok(false);
        };
        if self.st.is_loaded(l) || self.lazy_pre(l) {
            return Ok(false);
        }
        if let Some(b) = self.memos.bool_of(l, crate::state::M_NUM) {
            return Ok(b);
        }
        let f = self.loc_field(l);
        let v = self.v;
        let p = self.cx.path(&[
            phitex_defuse::Step::Proj(self.ops.num_proj),
            phitex_defuse::Step::Cmp(v.int(-1)),
        ]);
        let b = self.cx.read_path(f, p)?.is_some_and(|x| int_of(v, x) > 0);
        self.memos.set_bool(l, crate::state::M_NUM, b);
        Ok(b)
    }

    /// The order of the number of `tok`'s meaning (a macro
    /// [`Run::num_lookup`] found to be a number) against `k`: -1, 0 or 1,
    /// read as that alone.
    pub(crate) fn num_order(&mut self, tok: Tok, k: i64) -> R<i64> {
        let v = self.v;
        let Some(l) = self.mean_loc(tok) else {
            return Ok(0);
        };
        let f = self.loc_field(l);
        let p = self.cx.path(&[
            phitex_defuse::Step::Proj(self.ops.num_proj),
            phitex_defuse::Step::Cmp(v.int(k)),
        ]);
        Ok(self.cx.read_path(f, p)?.map_or(0, |x| int_of(v, x)))
    }

    /// The number of `tok`'s meaning (a macro [`Run::num_lookup`] found
    /// to be a number), read as that number.
    pub(crate) fn num_value(&mut self, tok: Tok) -> R<i32> {
        let v = self.v;
        let Some(l) = self.mean_loc(tok) else {
            return Ok(0);
        };
        let f = self.loc_field(l);
        Ok(self
            .cx
            .read_path(f, phitex_defuse::PathId::of_proj(self.ops.num_proj))?
            .map_or(0, |x| i32::try_from(int_of(v, x)).unwrap_or(0)))
    }

    /// Whether `item` is an item of the comma list `tok`'s meaning holds
    /// (a parameterless macro, [`crate::val::clist_projection`]), read
    /// through that projection and the item's membership: a list grown by
    /// other items leaves this run standing. `None` where the meaning is
    /// no such list, or the run has it in full or bound it itself (as
    /// [`Run::defined_lookup`]).
    pub(crate) fn clist_has(&mut self, tok: Tok, item: &[Tok]) -> R<Option<bool>> {
        let v = self.v;
        let Some(l) = self.mean_loc(tok) else {
            return Ok(None);
        };
        if self.st.is_loaded(l) || self.lazy_pre(l) || self.interp.is_some() || self.rec_load {
            return Ok(None);
        }
        let f = self.loc_field(l);
        let not = self.cx.path(&[
            phitex_defuse::Step::Proj(self.ops.clist_proj),
            phitex_defuse::Step::Has(v.str(crate::val::CLIST_NOT)),
        ]);
        if self.cx.read_path(f, not)?.is_none_or(|x| x != v.bool(false)) {
            return Ok(None);
        }
        let key = crate::val::toks_value(v, item);
        let has = self.cx.path(&[phitex_defuse::Step::Proj(self.ops.clist_proj), phitex_defuse::Step::Has(key)]);
        Ok(self.cx.read_path(f, has)?.map(|x| x == v.bool(true)))
    }

    /// Whether `tok`'s meaning is defined, read through the engine's
    /// definedness projection ([`crate::val::defined_projection`]) where
    /// the run has not bound it itself, nor interprets a line or records a
    /// load (which read meanings in full): a redefinition of a defined
    /// name leaves this run standing. `None` where read in full.
    pub(crate) fn defined_lookup(&mut self, tok: Tok) -> R<Option<bool>> {
        let Some(l) = self.mean_loc(tok) else {
            return Ok(None);
        };
        if self.st.is_loaded(l) || self.lazy_pre(l) || self.interp.is_some() || self.rec_load {
            return Ok(None);
        }
        if let Some(d) = self.memos.bool_of(l, crate::state::M_DEFINED) {
            return Ok(Some(d));
        }
        let f = self.loc_field(l);
        let d = match self.cx.read_path(f, phitex_defuse::PathId::of_proj(self.ops.defined_proj))? {
            Some(x) => int_of(self.v, x) != 0,
            None => return Ok(None),
        };
        self.memos.set_bool(l, crate::state::M_DEFINED, d);
        Ok(Some(d))
    }

    /// Whether `tok`'s meaning is an `\outer` macro, read through the
    /// engine's outer projection ([`crate::val::outer_projection`]) where
    /// the run has not bound it itself: a redefinition that keeps it
    /// outer, or not, leaves this run standing. `None` where it is this
    /// run's own (read in full).
    #[inline(never)]
    fn outer_lookup(&mut self, tok: Tok) -> R<Option<bool>> {
        let Some(l) = self.mean_loc(tok) else {
            return Ok(None);
        };
        if self.st.is_loaded(l) || self.lazy_pre(l) {
            return Ok(None);
        }
        if let Some(o) = self.memos.bool_of(l, crate::state::M_OUTER) {
            return Ok(Some(o));
        }
        let f = self.loc_field(l);
        let Some(x) = self.cx.read_path(f, phitex_defuse::PathId::of_proj(self.ops.outer_proj))? else {
            return Ok(None);
        };
        let o = int_of(self.v, x) != 0;
        self.memos.set_bool(l, crate::state::M_OUTER, o);
        Ok(Some(o))
    }

    /// A NUL read from a document line: the mark of a comment of the
    /// canonical text (`api::canonical_text`), read as text where `%` was
    /// no comment (a verbatim's): the comment put back in the line in
    /// place of the mark (its NUL and id) from `doc:mark:<id>`, and the
    /// line read on from there, as TeX reads the document's own text.
    /// Whether it was one.
    #[cold]
    pub(crate) fn splice_mark(&mut self) -> R<bool> {
        let v = self.v;
        let doc = self.input.top_line().is_some_and(|s| {
            s.origin
                .is_some_and(|(n, _, _)| str_of(v, n) == crate::api::DOC_NAME)
        });
        if !doc {
            return Ok(false);
        }
        // (the mark's id, the 16 hex digits after its NUL in the line,
        // `api::CommentMark`: its comment read by that id alone, nothing
        // of where the line is in the file)
        let Some(id) = self.input.top_line().and_then(|s| {
            let ds: String = s
                .buf
                .get(s.loc..s.loc + 16)?
                .iter()
                .filter_map(|&c| char::from_u32(c))
                .collect();
            (ds.len() == 16 && ds.bytes().all(|c| c.is_ascii_hexdigit())).then_some(ds)
        }) else {
            return Ok(false);
        };
        if crate::exec::obs_flag!("PHITEX_SPLICE_TRACE") {
            eprintln!("splice: node {} mark {id}", self.cx.node().raw());
        }
        let field = v.field(&format!("{}{id}", crate::api::DOC_MARKS));
        let Some(text) = self.cx.read(field)?.map(|t| str_of(v, t)) else {
            return Ok(false);
        };
        let Some(s) = self.input.top_line_mut() else {
            return Ok(false);
        };
        let i = s.loc - 1;
        s.buf.splice(i..i + 17, text.chars().map(u32::from));
        s.loc = i;
        Ok(true)
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
        // document slice's are its text; the piece reads its unit; its
        // hint, a record's when its text cannot bound it, keyed by the
        // unit's text, is the one its file's cut registered,
        // `api::file_units`, `crate::pkgrec::node_key`)
        // (a document file the client cut: its slices as the client cut
        // them, the document's text's way, `api::doc_file_pieces`; the
        // client edits these pieces where the file changes, this node
        // one it edits them under)
        if f.lazy()
            && let Some(key) = f.cut
        {
            let loads = self.ops.pkg_loads.read().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
            return crate::api::doc_file_pieces(
                v,
                &self.ops.docs,
                &str_of(v, key),
                self.cx.node(),
                loads.as_deref(),
            );
        }
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
        let defs = self.ops.defs.locked().clone();
        let t = crate::hints::text_hints(&text, phitex_source::pass::FileKind::Document, &defs);
        crate::hints::register_text(&self.ops.hints, f.name, &t);
        // (a region whose text cannot bound what it defines: its
        // record's hint, keyed by its text, `crate::pkgrec`)
        let loads = self.ops.pkg_loads.read().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
        let mut out = Vec::new();
        for &(s, e) in &t.slices {
            if e <= f.off {
                continue;
            }
            let a = s.max(f.off);
            let starts = a == 0 || text.as_bytes()[a - 1] == b'\n';
            let mut args = slice_args_in_file(v, &text[a..e], starts, f.name, a, ValueId::UNIT);
            let own = crate::hints::slice_hint_of(&t, s);
            let rec = loads
                .as_deref()
                .filter(|_| crate::pkgrec::unbounded(&own))
                .and_then(|l| l.hint_for(v, &text[a..e]));
            args.push(v.str(rec.as_deref().unwrap_or(&*own)));
            out.push(args);
        }
        out
    }

    /// The scope kind the node runs in: the definition of [`SCOPE_KIND`]
    /// reaching it, made inside each scope by the node that opened it
    /// ([`Run::leave_scope`]), read once a run (a stop reads it again).
    #[inline]
    pub(crate) fn scope_kind(&mut self) -> R<Option<ValueId>> {
        if let Some(k) = self.scope_kind_memo {
            return Ok(k);
        }
        self.scope_kind_read()
    }

    #[inline(never)]
    fn scope_kind_read(&mut self) -> R<Option<ValueId>> {
        let r = self.cx.read(self.v.field(SCOPE_KIND));
        if r.is_err() {
            self.trace_stop("scope kind");
        }
        let k = r?;
        if crate::exec::prof_flag!("PHITEX_SKIND_LOG") {
            let v = self.v;
            let text: String = self.cx.args().first().map_or_else(String::new, |&a| v.render(a).chars().take(100).collect());
            eprintln!("skindlog: {} {} op {} {text}", self.cx.node().raw(), k.map_or_else(|| "none".to_owned(), |x| v.render(x)), self.op_name());
        }
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
        // the node's text kind (`args[1]`: 1 the job's text, 2 a
        // document's; a resume's flags, its first)
        // (a text's node only: a finisher's runs the output routine, neither
        // the job's text nor a document's, its `args[1]` its page's index)
        let op = self.cx.op();
        let at = usize::from(op != self.ops.resume);
        let k = if op == self.ops.exec || op == self.ops.resume {
            args.get(at).map_or(0, |&t| int_of(self.v, t))
        } else {
            0
        };
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
                    // (a scope closed inside a level of the run's own:
                    // the rest runs outside it from a continuation)
                    if self.closed_scope() && !self.claim_rest && !self.nest.is_empty() {
                        let (c, _) = self.cont_encode_as(true)?;
                        if crate::exec::obs_flag!("PHITEX_CLOSE_TRACE") {
                            eprintln!("close-trace: node {} ends at the close, its rest a resume: {}", self.cx.node().raw(), self.v.render(c).chars().take(300).collect::<String>());
                        }
                        self.close_cont = Some(c);
                    }
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
            if crate::exec::obs_flag!("PHITEX_GIVEBACK_TRACE") && self.doc && self.open_span() {
                eprintln!(
                    "readon: node {} pulled {}: nest {:?}, conds {}, effect {:?}, metas {}",
                    self.cx.node().raw(),
                    self.pulled,
                    self.nest
                        .iter()
                        .map(|l| (l.mode, l.group, l.owns))
                        .collect::<Vec<_>>(),
                    self.conds.len(),
                    self.effect,
                    self.st.sig().0
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
    /// (`Locality::Scope`, `Locality::ScopeAt`).
    pub(crate) fn can_leave_scope(&self) -> bool {
        let op = self.cx.op();
        (op == self.ops.exec || op == self.ops.resume)
            && !self.nest.is_empty()
            && self.nest.len() < usize::from(u8::MAX - 2)
            && self.conds.is_empty()
            && self.effect == Effect::None
            && self.nest.iter().all(|l| !l.owns && l.group != 0)
            && !crate::hints::effect_none(&self.ops.hints, self.v, self.cx.args())
    }

    /// Whether the run closed the engine scope it ran in: it ends after
    /// that command ([`CLOSE`], its reads and definitions inside the
    /// scope), and what it has left of its input runs outside it: as the
    /// following nodes, the conditionals carried ([`Run::leftover`]),
    /// where nothing of its own is open; else, a level of its own open
    /// (a paragraph begun inside the scope), as one resume carrying the
    /// levels, the input and the conditionals and none of the scope's
    /// bindings ([`Run::close_resume`]). (Any run: a run going on past
    /// the close would read on with its own bindings of the closed
    /// scope, §282 not done.)
    pub(crate) fn closed_scope(&self) -> bool {
        self.effect == CLOSE
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
        let x = self.cx.read(v.field(COND_STACK))?;
        // (`PHITEX_COND_TRACE=1`, observe: the conditionals a run takes
        // from the node before it, and those it leaves)
        if crate::exec::obs_flag!("PHITEX_COND_TRACE") {
            eprintln!(
                "cond-trace: node {} loads {} (speculative {})",
                self.cx.node().raw(),
                x.map_or_else(|| "none".to_owned(), |x| v.render(x)),
                self.cx.is_speculative()
            );
        }
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
        if crate::exec::obs_flag!("PHITEX_COND_TRACE") {
            eprintln!(
                "cond-trace: node {} leaves {} (stopped {}, loaded {})",
                self.cx.node().raw(),
                v.render(x),
                self.cond_stop,
                self.cond_loaded
            );
        }
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
        let mut publication = self.publication();
        // (a finisher's sealed lines in what leaves it: their own lists,
        // read now, `crate::pages::Run::unseal`)
        if !self.sealed.is_empty() {
            for d in &mut publication {
                if let crate::state::Pub::Value(x) = d.1 {
                    d.1 = crate::state::Pub::Value(self.unseal(x));
                }
            }
        }
        let published: Vec<(Field, ValueId, Locality)> = publication
            .iter()
            .filter_map(|&(f, w, l)| match w {
                crate::state::Pub::Value(x) => Some((f, x, l)),
                crate::state::Pub::Rel(_) => None,
            })
            .collect();
        self.audit(&publication);
        // (debug, `PHITEX_PUB_TRACE=<a>|<b>…`: the definitions of a run whose
        // first text segment contains one of the texts)
        if let Some(pat) = crate::exec::obs_str!("PHITEX_PUB_TRACE") {
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
        if crate::exec::obs_flag!("PHITEX_ITEMS_TRACE") {
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
        if self.rec_load && crate::exec::obs_flag!("PHITEX_PKG_TRACE") {
            let blanks = self.pulled_only_blanks();
            eprintln!(
                "pkgrec: load run {} finishes: pulled {} (blanks only {blanks}), rest claimed {}, effect {:?}, {} definitions",
                self.cx.node().raw(),
                self.pulled,
                self.claim_rest,
                self.effect,
                published.len()
            );
        }
        // (a document node whose text cannot bound what it defines is
        // recorded as a load is, `crate::pkgrec::is_recorded`)
        let rec = self.rec_load
            || (op == self.ops.exec
                && self.ops.node_rec.load(std::sync::atomic::Ordering::Relaxed)
                && !crate::pkgrec::is_load(self.v, self.cx.args())
                && crate::pkgrec::is_recorded(&self.ops.hints, self.v, self.cx.args()));
        if rec && op == self.ops.exec && !self.claim_rest && self.pulled_only_blanks() {
            self.record_load(&published);
        }
        let published_trace = if crate::exec::obs_flag!("PHITEX_MARK_TRACE") {
            published.clone()
        } else {
            Vec::new()
        };
        {
            let (n, t_list) = (publication.len(), t_pub.elapsed());
            for &(f, w, l) in &publication {
                match w {
                    // (a symbol written to the item: the alias it is,
                    // `crate::defer::try_symbolic`)
                    crate::state::Pub::Value(x) => match phitex_defuse::alias_of(self.v, x) {
                        Some((g, at)) => self.cx.define_alias(f, g, at, l),
                        None => self.cx.define_with(f, x, l),
                    },
                    crate::state::Pub::Rel(c) => self.cx.define_rel_with(f, c, l),
                }
            }
            if n >= 10_000 && crate::exec::prof_flag!("PHITEX_TEX_TIME") {
                eprintln!(
                    "run finish: {n} definitions published: listed {t_list:?}, define_with {:?}",
                    t_pub.elapsed().checked_sub(t_list).unwrap_or_default()
                );
            }
        }
        if !publication.is_empty() {
            let names: Vec<ValueId> = publication.iter().map(|d| d.0.0).collect();
            let l = self.v.list(names);
            let s = self.v.seq_unit(l);
            let defs = self
                .ops
                .fields
                .get_at(self.v, self.cx.values_epoch(), "defs");
            self.muts += 1;
            self.contribs.push((defs, s));
        }
        if crate::exec::obs_flag!("PHITEX_TEX_TRACE") {
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
        if cfg!(feature = "observe") {
            self.check_contribs_complete();
        }
        let contribs_trace: Vec<String> = if crate::exec::obs_flag!("PHITEX_MARK_TRACE") {
            let v = self.v;
            self.contribs.iter().map(|(f, x)| format!("{}+={}", crate::val::field_name(v, *f), x.raw())).collect()
        } else {
            Vec::new()
        };
        // (a finisher's pages leave as their shapes, `pages`: their
        // writers read the lines, `crate::pdfout::PdfPage`)
        let pages_f = self.v.field("pages");
        for (f, x) in std::mem::take(&mut self.contribs) {
            let x = if f == pages_f { x } else { self.unseal(x) };
            self.cx.contribute(f, x);
        }
        // (a page's PDF side is a child of the run that shipped it, named
        // by its place among the run's shipouts, never by what it holds:
        // the pages' keys are a definition of the run's, read by those
        // children (`crate::pdfout::PAGE_KEYS`), so an edit that changes
        // the page runs the same child again instead of making another,
        // and the run keeps its shape)
        let mut keys = Vec::new();
        for (op, args) in std::mem::take(&mut self.kids) {
            if op == self.ops.pdfpage {
                let v = self.v;
                let i = v.int(i64::try_from(keys.len()).unwrap_or(i64::MAX));
                keys.push(args.first().copied().unwrap_or(ValueId::UNIT));
                self.cx.child(op, vec![i]);
            } else {
                self.cx.child(op, args);
            }
        }
        if !keys.is_empty() {
            let v = self.v;
            self.cx.define_with(
                v.field(crate::pdfout::PAGE_KEYS),
                v.list(keys),
                Locality::Global,
            );
        }
        // (a run cut at a candidate: its input left is its continuation,
        // the resume piece's, `crate::pages`)
        let leftover = if self.cut.is_some() || self.close_cont.is_some() {
            Vec::new()
        } else {
            self.leftover()
        };
        self.close_resume();
        for args in leftover {
            if crate::exec::obs_flag!("PHITEX_TEX_TRACE") {
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
        if crate::exec::obs_flag!("PHITEX_DEBUG_CLAIMS") {
            eprintln!(
                "claim: node {} claims {n} (pulled {})",
                self.cx.node().raw(),
                self.pulled
            );
        }
        if crate::exec::obs_flag!("PHITEX_TEX_TRACE") {
            eprintln!(
                "  end {}: claim {n} (pulled {}, rest {}), cut {:?}",
                self.cx.node().raw(),
                self.pulled,
                self.claim_rest,
                self.cut
            );
        }
        // (`PHITEX_CLAIM_TRACE`: each claim a run makes, with its hint's
        // cause and its text)
        if n > 0 && crate::exec::obs_flag!("PHITEX_CLAIM_TRACE") {
            let args = self.cx.args();
            let h = crate::hints::hint_of_args(&self.ops.hints, self.v, args);
            eprintln!(
                "claim-made: {n}\t{}\t{:?}",
                if h.bound.is_some() {
                    "bounded".to_owned()
                } else {
                    h.cause.clone()
                },
                args.first()
                    .map(|&x| self.v.render(x).chars().take(120).collect::<String>())
                    .unwrap_or_default()
            );
        }
        // (what the run took, none included: a declared claim is a
        // guess, and a run that makes no claim is held to it, so the
        // siblings it declared it would take and did not read stayed
        // taken, unread; the engine drops a claim of none where none
        // was declared or held)
        // (a fatal error: everything after this node in the unit, the
        // included files' pieces among it, is dead input)
        if self.fatal_hit {
            self.cx.claim_rest();
        } else {
            self.cx.claim(n);
        }
        if crate::exec::obs_flag!("PHITEX_TEX_TRACE")
            && self.effect != Effect::None
            && crate::hints::effect_none(&self.ops.hints, self.v, self.cx.args())
        {
            eprintln!(
                "  effect {:?} of {} against its hint's none (refuted)",
                self.effect,
                self.cx.node().raw()
            );
        }
        // (a piece's: its brackets were predicted from its text, which
        // the run confirms or puts right, none among them)
        if self.effect != Effect::None || op == self.ops.exec || op == self.ops.resume {
            self.cx.set_effect(self.effect);
        }
        if crate::exec::prof_flag!("PHITEX_SKIND_LOG") && self.cut.is_some() && (op == self.ops.exec || op == self.ops.resume) {
            let shape = crate::ops::scope_shape_of(self.v, &self.text_arg().into_iter().collect::<Vec<_>>(), op == self.ops.resume);
            eprintln!("skindcut: {} op {} effect {:?} shape {shape:?}", self.cx.node().raw(), self.op_name(), self.effect);
        }
        // (a run cut at a candidate, or one that ended the job, ran not
        // to its text's end: its brackets say nothing of its shape's)
        if (op == self.ops.exec || op == self.ops.resume) && self.cut.is_none() && !self.claim_rest {
            let shape = crate::ops::scope_shape_of(self.v, &self.text_arg().into_iter().collect::<Vec<_>>(), op == self.ops.resume);
            let opened = match self.effect {
                Effect::Scope(b) if b.closes == 0 => usize::from(b.opens),
                _ => 0,
            };
            let learn: Vec<(String, bool)> = match shape {
                // (a closer that ran inside a scope: one that found none
                // open, its opener not yet run, says nothing)
                crate::hints::ScopeShape::Close(c) if matches!(self.scope_kind_memo, Some(Some(_))) => vec![(c, self.effect == CLOSE)],
                crate::hints::ScopeShape::Open(os) => {
                    let ok = opened == os.len();
                    os.into_iter().map(|o| (o, ok)).collect()
                }
                _ => Vec::new(),
            };
            if crate::exec::prof_flag!("PHITEX_SKIND_LOG") && !learn.is_empty() {
                let m = self.ops.scope_learned.read().unwrap_or_else(std::sync::PoisonError::into_inner);
                let was: Vec<(String, Option<bool>)> = learn.iter().map(|(k, _)| (k.clone(), m.get(k).copied())).collect();
                let v = self.v;
                let carried: Vec<i64> = if op == self.ops.resume {
                    let c = list_of(v, self.text_arg().unwrap_or(ValueId::UNIT));
                    c.get(1).map(|&l| list_of(v, l).iter().map(|&e| list_of(v, e).get(1).map_or(-1, |&g| int_of(v, g))).collect()).unwrap_or_default()
                } else {
                    Vec::new()
                };
                eprintln!("skindlearn: {} op {} effect {:?} learned before {was:?} carried {carried:?} pulled {}", self.cx.node().raw(), self.op_name(), self.effect, self.pulled);
            }
            if !learn.is_empty() {
                let mut m = self.ops.scope_learned.write().unwrap_or_else(std::sync::PoisonError::into_inner);
                for (k, ok) in learn {
                    if m.get(&k) != Some(&ok) {
                        m.insert(k, ok);
                    }
                }
            }
        }
        // (`PHITEX_SKIND_LOG`: the brackets a run sets, and its text)
        if crate::exec::prof_flag!("PHITEX_SKIND_LOG") && self.effect != Effect::None {
            let v = self.v;
            let text = self.cx.args().first().map_or_else(String::new, |&a| v.render(a).chars().take(160).collect());
            eprintln!("skindeff: {} op {} {:?} {text}", self.cx.node().raw(), self.op_name(), self.effect);
        }
        // (a finisher's output: what its routine gave back, which the
        // next page step reads, `crate::pages::PAGE_OUT`)
        let out = self.fin.as_ref().and_then(|c| c.rtn).unwrap_or(ValueId::UNIT);
        if op == self.ops.finish
            && let Some(key) = self.fin_page().map(|f| f.key)
            && self.fin.as_ref().is_some_and(|c| c.rtn.is_some())
        {
            let v = self.v;
            let x = crate::pages::page_out_value(v, key.piece, key.nth, out);
            self.cx.define_with(key.field(v, crate::pages::PAGE_OUT), x, Locality::Global);
            self.cx.contribute(v.field(crate::pages::FIN_DONE), v.int(1));
        }
        if crate::exec::obs_flag!("PHITEX_MARK_TRACE") {
            let v = self.v;
            let mut pubd: Vec<String> = published_trace
                .iter()
                .map(|(f, x, l)| format!("{}{l:?}={}", crate::val::field_name(v, *f), x.raw()))
                .collect();
            pubd.sort();
            eprintln!(
                "mark-trace-end: node {} out {} claim {n} effect {:?} defs {pubd:?} contribs {contribs_trace:?}",
                self.cx.node().raw(),
                out.raw(),
                self.effect
            );
        }
        out
    }

    /// Notes what the run publishes and read ([`Ops::audit`]).
    fn audit(&mut self, publication: &[(Field, crate::state::Pub, Locality)]) {
        let mut a = NodeAudit {
            op: self.op_name(),
            defs: publication.iter().map(|&(f, w, _)| (f, matches!(w, crate::state::Pub::Rel(_)))).collect(),
            ..NodeAudit::default()
        };
        for l in self.st.used_locs() {
            if let Some(fam) = crate::state::Family::of(l) {
                let p = fam.prefix();
                match a.reads.iter_mut().find(|r| r.0 == p) {
                    Some(r) => r.1 += 1,
                    None => a.reads.push((p, 1)),
                }
            } else {
                let f = self.loc_field(l);
                a.other_reads.push(f);
            }
        }
        self.ops.audit.locked().insert(self.cx.node().raw(), a);
    }

    /// The following siblings this run takes, once its input is read
    /// out: those it pulled, and with `\end` inside groups every text
    /// after them. Declared at the commit, and before the page fold's
    /// read of the page state (`crate::pages`), so that a run parked
    /// there holds no sibling it does not take.
    pub(crate) fn claim_count(&mut self) -> usize {
        let mut n = self.pulled;
        if self.claim_rest {
            // (nothing after this one is read, §93 and §1054: every text
            // sibling after it, a piece, a rest or a box put, is dead
            // input; the end mark, the page builder's kick, the writers and
            // the link after the text are not text and stay: a count that
            // took them left no PDF, and one that stopped at the first
            // sibling of another text operation left the rest to run on)
            while matches!(self.cx.following(n), Some((op, _)) if op == self.ops.exec || op == self.ops.resume || op == self.ops.put) {
                n += 1;
            }
        }
        n
    }

    /// Whether every sibling the run took is blank text (a line's end
    /// left as a node of its own): the run is still its load's alone.
    fn pulled_only_blanks(&mut self) -> bool {
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

    /// A package load's or a document node's record (`crate::pkgrec`):
    /// the files it read (and looked for in vain), the fields it defined
    /// (`published`) and read from the engine, whether it opened no
    /// scope and took no sibling; contributed to `pkgrecs` under its key.
    fn record_load(&mut self, published: &[(Field, ValueId, Locality)]) {
        let v = self.v;
        let Some(key) = crate::pkgrec::args_key(&self.ops.hints, v, self.cx.args()) else {
            return;
        };
        // (the files the job writes and reads back are the document's,
        // never part of a load's key; one read cut is read for that it
        // is, whatever its text)
        let mut files: Vec<(String, u64)> = Vec::new();
        // (a read refuted here refutes the run, whose record is dropped)
        for (n, h) in self.fread.clone() {
            if !self.written_known(&n).unwrap_or(true) {
                let h = if self.uread.contains(&n) {
                    crate::pkgrec::UNITS
                } else {
                    h
                };
                files.push((n, h));
            }
        }
        for n in self.fmiss.clone() {
            if !self.written_known(&n).unwrap_or(true) {
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
        let restored: Vec<Loc> = self.pubs.as_ref().map_or_else(|| self.st.publication(v, true).1, |p| p.2.clone());
        let reads: Vec<String> = self
            .st
            .used_locs()
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
        if crate::exec::obs_flag!("PHITEX_PKG_TRACE") {
            let skips: Vec<String> = self
                .skip_first
                .clone()
                .into_iter()
                .map(|l| crate::val::field_name(v, self.loc_field(l)))
                .collect();
            eprintln!("pkgrec: skip reads of node {node}: {skips:?}");
        }
        let e = crate::pkgrec::entry(v, node, &key, &files, &writes, &reads, flags, &values);
        self.contribute("pkgrecs", v.seq_unit(e));
        self.ops.rec_ran.locked().push(node);
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

/// A file the job writes read back twice by the run at its read-back
/// point (`wreread:<file>`, [`Run::mark_read_point`]): its lines are
/// written as its text, never interpreted per field. A read back per
/// field stands for the lines by the definitions they make under the
/// meanings at one point; that is the file only where every read gives
/// its lines those meanings. A run that reads the file again after
/// redefining what the lines call (a macro that reads a list once to
/// count its entries and again to act on them) reads them under other
/// meanings there, which the definitions made at the first read do not
/// say: the second read would find the first's definitions in place of
/// the lines it acts on.
pub(crate) const REREAD: &str = "wreread:";

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
    // (a cut file: `["d", name, key]`, [`FileRest::cut`])
    if let Some(key) = f.cut {
        return Some(v.list(vec![v.str("d"), f.name, key]));
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
    v.with_list(l, |fs| {
        for &f in fs {
            if out.len() >= limit {
                break;
            }
            out.extend(crate::val::frame_of(v, f, || decode_seg(v, f)));
        }
    });
    out
}

/// The conditionals a node's text begins inside: those its run's
/// predecessor left open when it closed its scope ([`Run::leftover`]),
/// `(if_limit, cur_if)` each.
pub(crate) fn carried_conds(v: &Values, args: &[ValueId]) -> Option<(Vec<u8>, Vec<i32>)> {
    let &l = args.first()?;
    let f = v.with_list(l, |fs| fs.first().copied()).flatten()?;
    let ns: Vec<i64> = v
        .with_list(f, |xs| {
            (xs.first().and_then(|&t| v.with_str(t, |s| s == "c")) == Some(true))
                .then(|| xs[1..].iter().map(|&x| int_of(v, x)).collect())
        })
        .flatten()?;
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
            Some("d") => {
                // (a cut file, not read yet, [`units_frame`])
                if let (Some(&name), Some(&key)) = (xs.get(1), xs.get(2)) {
                    out = Some(Seg::File(FileRest::cut(v, key, name)));
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
                    chars: xs.get(4).is_some_and(|&x| int_of(v, x) & 8 != 0),
                    origin: match (xs.get(5), xs.get(6)) {
                        (Some(&n), Some(&o)) => Some((
                            n,
                            usize::try_from(int_of(v, o)).unwrap_or(0),
                            xs.get(8).copied().unwrap_or(ValueId::UNIT),
                        )),
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
/// byte `at` of the file named `name` (of its unit `unit`, a written
/// file's `wunit:` field's name, where it is one: `at` from the unit's
/// start, the frame
/// `[.., name, offset, 0, unit]`): every segment carries its origin
/// `(name, offset, unit)`; the kind is 2 (a document's piece, not the top).
pub(crate) fn slice_args_in_file(
    v: &Values,
    text: &str,
    starts_line: bool,
    name: ValueId,
    at: usize,
    unit: ValueId,
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
        let mut xs = vec![
            v.str("s"),
            v.str(&seg),
            v.int(i64::from(state)),
            v.int(-2),
            v.int(flags),
            name,
            v.int(i64::try_from(pos).unwrap_or(0)),
        ];
        if unit != ValueId::UNIT {
            xs.extend([v.int(0), unit]);
        }
        frames.push(v.list(xs));
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

/// The env field listing the files the job writes whose text the
/// client gave as the build began ([`crate::Options::written_init`]);
/// `wseeds:<name>` is each one's text.
pub(crate) const SEEDS: &str = "wseeds";

/// A round-one read-back of a file the job writes, from the client's
/// text ([`Run::seed_written`]).
#[derive(Clone)]
pub(crate) struct Seeded {
    pub name: String,
    /// Its `wkeys:` value: the key set, or the text mark.
    pub keys: ValueId,
    /// Its definitions by `wdef:` id (`[value, global]` each; `UNIT`
    /// for a key another file publishes).
    pub defs: Vec<(String, ValueId)>,
    /// Its text.
    pub text: ValueId,
}

/// `scanner_status` (§305) values kept in [`Run::scanner`].
pub(crate) const SCAN_DEFINING: u8 = 2;
pub(crate) const SCAN_MATCHING: u8 = 3;
pub(crate) const SCAN_ABSORBING: u8 = 5;
impl Run<'_, '_> {

    pub(crate) fn file_line_here(&self) -> Option<FileLine> {
        for l in self.input.levels.iter().rev() {
            match l.kind {
                crate::input::kind::TOKS => {}
                crate::input::kind::LINE => {
                    if let Some(x) = line_of(self.input.line(*l)) {
                        return Some(x);
                    }
                }
                _ => {
                    return Some(FileLine::Known(i64::from(self.input.file(*l).line)));
                }
            }
        }
        self.text_line()
    }

    /// [`Run::file_line_here`] below the levels the run holds.
    pub(crate) fn text_line(&self) -> Option<FileLine> {
        // (a run resumed from a park: the levels it had when it parked and
        // has not reached again are below the ones it holds, in order, as
        // in a run that never parked)
        let parked = self.input.text_at..self.parked_levels.min(self.input.text.len());
        for s in self.input.text.get(parked).unwrap_or_default() {
            match s {
                Seg::Toks(_) => {}
                Seg::Line(l) => {
                    if let Some(x) = line_of(l) {
                        return Some(x);
                    }
                }
                Seg::File(f) => return Some(FileLine::Known(i64::from(f.line))),
            }
        }
        // (a resumed piece's: where the segment it reads now, the last it
        // took up, was read, [`Run::cont_load`])
        if let Some(&Some(c)) = self
            .input
            .text_at
            .checked_sub(1)
            .and_then(|i| self.cont_lines.get(i))
        {
            return Some(c);
        }
        let v = self.v;
        let frames = self.text_arg().map(|f| list_of(v, f))?;
        frames.iter().find_map(|&f| frame_line(v, f))
    }

    /// The node's text: an exec node's first argument, a resume's
    /// continuation ([`Run::cont_val`]).
    pub(crate) fn text_arg(&self) -> Option<ValueId> {
        if self.cx.op() == self.ops.resume {
            return self.cont_val;
        }
        self.cx.args().first().copied()
    }

    /// A line of a document's piece in file `name` begun (§362).
    pub(crate) fn doc_line_begun(&mut self, name: ValueId) {
        match self.doc_lines.iter_mut().find(|(n, _)| *n == name) {
            Some((_, k)) => *k += 1,
            None => self.doc_lines.push((name, 1)),
        }
    }
}

/// Where a line was read ([`Run::file_line_here`]): its file's line,
/// or a document piece's file; a `\read` line is no file's (§483), a text
/// without an origin, a format's or INITEX's, one file (`UNIT`).
pub(crate) fn line_of(s: &Line) -> Option<FileLine> {
    if s.lineno > 0 {
        return Some(FileLine::Known(i64::from(s.lineno)));
    }
    (!s.rescanned).then(|| FileLine::Piece(s.origin.map_or(ValueId::UNIT, |o| o.0)))
}

/// Where the tokens of a token frame (`["t", toks, file, line]`,
/// [`Run::leftover`]) were read, if it says.
pub(crate) fn frame_line(v: &Values, f: ValueId) -> Option<FileLine> {
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
}

/// Whether a field the interpretation of a written line set is one of
/// TeX's definitions (§222: the table of equivalents, the hash's
/// meanings), what reading the file back defines. The writer's and the
/// page builder's own state (`pdf:` objects and counters, `pg:` page
/// contents, the streams, `\afterassignment`'s token, the dead cycles,
/// ...) is never a read-back definition, nor are category codes (the
/// read-back reads under `\makeatletter`).
fn is_read_back_definition(name: &str) -> bool {
    const NOT: [&str; 18] = [
        "cat:",
        "wread:",
        REREAD,
        "pdf:",
        "pg:",
        "pp:",
        "write:",
        "wopen:",
        "wpre:",
        "wpoint:",
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

/// The field holding the kind of the scope open at a node
/// ([`Run::scope_kind`]): `[group, sid, list, mode, depth, parent kind,
/// extra…]`, defined inside the scope by the node that leaves its group
/// open, so a close ends it as it ends the group's other locals.
pub(crate) const SCOPE_KIND: &str = "skind";

/// A node that closes the scope it runs in, after its reads and
/// definitions: the run ends there ([`Run::closed_scope`]).
pub(crate) const CLOSE: Effect = Effect::Scope(phitex_defuse::Brackets {
    closes: 1,
    opens: 0,
    before: false,
});
