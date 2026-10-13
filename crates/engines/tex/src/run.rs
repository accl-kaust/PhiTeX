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
    pub pageplan: OpId,
    pub brk: OpId,
    /// The build's field handles (`crate::fields`).
    pub fields: std::sync::Arc<crate::fields::Fields>,
    /// The format's definitions the build analyses document texts with
    /// (set by `api::Build::with`; none while a format is built): a
    /// document file `\input` is sliced with them ([`Run::leftover`]).
    pub defs: std::sync::Arc<std::sync::Mutex<crate::hints::Defs>>,
    /// Page-step runs that reached their finish, by page index (not
    /// the attempts that stopped or were cut off): [`crate::Stats`].
    pub steps_done: std::sync::Arc<std::sync::Mutex<Vec<u64>>>,
    /// The page chain's loop-backs as read and published in the current
    /// evaluation: the refuted guesses ([`crate::Stats`]).
    pub lblog: std::sync::Arc<std::sync::Mutex<crate::page::LbLog>>,
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

    /// The next line as a segment not yet reached (what a top-level
    /// node's arguments for it decode to, without a value or a string per
    /// line: its bytes are its characters, trailing blanks removed, as
    /// web2c's `input_line`); the offset moves past it.
    fn take_src(&mut self) -> Src {
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
        let mut buf = Vec::with_capacity(end + 1);
        buf.extend(bytes[..end].iter().map(|&b| u32::from(b)));
        let chars = line.chars().count();
        self.pos += chars;
        self.off += n;
        Src {
            buf,
            loc: 0,
            state: NEW_LINE,
            endch: -2,
            starts: true,
            ends: true,
            origin: Some((self.name, at)),
            rescanned: false,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Src {
    pub buf: Vec<u32>,
    pub loc: usize,
    /// 0 `mid_line`, 1 `skip_blanks`, 2 `new_line` (§303).
    pub state: u8,
    /// -2 until the segment is reached; then `\endlinechar` as read when
    /// it was reached (§360), appended to `buf` when it is a character
    /// (-3: reached without its line's end, which a pulled sibling
    /// brings, [`Run::pull`]).
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

/// The commands an alignment entry watches for (§342, §347, §357):
/// braces, `&` and `\cr`, as a bit mask by command code.
const ALIGN_CMDS: u128 =
    1 << cmd::LEFT_BRACE | 1 << cmd::RIGHT_BRACE | 1 << cmd::TAB_MARK | 1 << cmd::CAR_RET;

pub(crate) const MID_LINE: u8 = 0;
pub(crate) const SKIP_BLANKS: u8 = 1;
pub(crate) const NEW_LINE: u8 = 2;

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
    /// The run's state table ([`crate::runstate`]): every field it read
    /// or defined, native.
    pub st: crate::runstate::State,
    /// The category codes of characters 0–255 as last read (`u8::MAX`:
    /// not read), valid while the table's catcode generation is
    /// `cat_gen`.
    cat_cache: [u8; 256],
    cat_gen: u64,
    pub contribs: Vec<(Field, ValueId)>,
    /// The page builder of a text run as one node, paging inline
    /// (`pageinline.rs`).
    pub pg: Option<Box<crate::pageinline::Inline>>,
    /// A page step whose output routine published leftovers
    /// (`pagefx:<j>`).
    pub pagefx_pub: bool,
    pub kids: Vec<(OpId, Vec<ValueId>)>,
    pub effect: Effect,
    pub cur: Cur,
    /// Conditionals begun in this run: their `if_limit` (§489).
    pub conds: Vec<u8>,
    /// The codes (`cur_if`) of `conds`, parallel to it.
    pub if_codes: Vec<i32>,
    /// The buffer a control sequence's name is scanned into (§356).
    name_buf: String,
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
    /// Flush points met by this run so far ([`Run::flush_point`]).
    pub flushes: usize,
    pub pre_memo: phitex_defuse::FxHashMap<Loc, Option<ValueId>>,
    /// Errors since the last paragraph ended (§76 `error_count`, reset by
    /// `end_graf`, §1096): at 100 the job stops (§82).
    pub error_count: u32,
    /// Whether the job stopped at 100 errors.
    pub fatal_logged: bool,
    /// The input as the command in progress began ([`Run::rewind`]).
    mark: Mark,
    /// A halt raised on the token path ([`Run::raise`]), returned by the
    /// next `get_next` of the command in progress.
    halt: Option<Halt>,
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
            st: crate::runstate::State::new(),
            cat_cache: [u8::MAX; 256],
            cat_gen: u64::MAX,
            contribs: Vec::new(),
            pg: None,
            pagefx_pub: false,
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
            interp_base: 0,
            closed: Vec::new(),
            flushes: 0,
            pre_memo: phitex_defuse::FxHashMap::default(),
            error_count: 0,
            fatal_logged: false,
            mark: Mark::default(),
            halt: None,
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
            let p = if self
                .cx
                .is_loopback(self.keyed(crate::fields::Fam::Wdef, &name))
            {
                self.get(&format!("wpre:{name}"))?
            } else {
                None
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
            eprintln!(
                "aux-trace: count0 := {x} global={global} by node {} op {:?}",
                self.cx.node().raw(),
                self.cx.op()
            );
        }
        let grp = self.innermost_group();
        self.st.set(loc, val, global, grp);
        self.irreversible += 1;
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
            let name = str_of(v, k);
            let wd = self.keyed(crate::fields::Fam::Wdef, &name);
            if !self.cx.is_loopback(wd) {
                self.cx
                    .use_env(self.keyed(crate::fields::Fam::Wdefdecl, &name))?;
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
            }
        }
        Ok(true)
    }

    /// A line written to a file whose read-back only defines, read as
    /// its read-back reads it (under `\makeatletter`), on scratch: the
    /// definitions it makes, `[[field, value, global], ...]`. Nothing
    /// else of it stays in this run (its input, lists, contributions and
    /// definitions are restored), except what it writes to other files
    /// (LaTeX's `\@writefile{toc}{...}`, which writes the `.toc` line as
    /// `\enddocument` reads the `.aux` back): those `streams` entries
    /// (and a document node's `T_WRITE_AT` markers for them) are kept.
    pub(crate) fn interpret_line(&mut self, line: &str) -> R<ValueId> {
        let v = self.v;
        let frames = std::mem::take(&mut self.frames);
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
        self.set("cat:64", v.int(11), false);
        let base = self.st.stamp();
        self.interp = Some(phitex_defuse::FxHashSet::default());
        self.interp_base = base;
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
        let made: Vec<ValueId> = self
            .st
            .defined_since(v, base)
            .into_iter()
            .filter_map(|(loc, x, l)| Some((self.loc_field(loc), x?, l)))
            .filter(|d| !str_of(v, d.0.0).starts_with("cat:"))
            .map(|(f, x, l)| {
                v.list(vec![
                    f.0,
                    x,
                    v.int(i64::from(matches!(l, Locality::Global))),
                ])
            })
            .collect();
        self.interp = None;
        self.frames = frames;
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
        Ok(v.list(made))
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

    /// The fields the run defines at its base level.
    pub(crate) fn def_fields(&mut self) -> Vec<Field> {
        let locs: Vec<Loc> = self.st.defined_locs().collect();
        locs.into_iter().map(|l| self.loc_field(l)).collect()
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
    fn set_cur_flag(&mut self, tok: Tok) {
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
    #[inline(never)]
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

    /// `back_input` (§325): `t` is read next.
    pub(crate) fn back_input(&mut self, t: Tok) {
        // §325: a brace put back is counted again when read
        if let Some(s) = self.astate
            && !t.is_cs_or_active()
        {
            match t.cat() {
                Some(1) => self.set_astate(s - 1),
                Some(2) => self.set_astate(s + 1),
                _ => {}
            }
        }
        // (the token just read from the list on top is that list's
        // previous token again: it is read next by stepping back, the
        // same tokens in the same order without a new list)
        if let Some(Frame::Toks(l, p)) = self.frames.front_mut()
            && *p > 0
            && l[*p - 1] == t
        {
            *p -= 1;
            return;
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
            self.pop_front_marked();
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
                let joined = self.push_back_frames(frames);
                // (the rest of a line already reached is read now, §360:
                // `\endlinechar` as it is now goes after it)
                if let Some(k) = joined {
                    let e = i64::from(self.ipar(Par::endlinechar)?);
                    if let Some(Frame::Src(s)) = self.frames.get_mut(k) {
                        if (0..256).contains(&e) {
                            s.buf.push(u32::try_from(e).unwrap_or(13));
                        }
                        s.endch = e;
                    }
                }
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
    /// it (a control sequence's name may run across nodes). The index of
    /// a joined segment reached before that now ends its line, whose
    /// `\endlinechar` is due ([`Run::pull`]).
    pub(crate) fn push_back_frames(&mut self, frames: Vec<Frame>) -> Option<usize> {
        let mut due = None;
        for f in frames {
            if let (Some(Frame::Src(last)), Frame::Src(next)) = (self.frames.back_mut(), &f)
                && !last.ends
                && !next.starts
            {
                last.buf.extend_from_slice(&next.buf);
                last.ends = next.ends;
                if last.endch != -2 {
                    last.endch = -3;
                    if last.ends {
                        due = Some(self.frames.len() - 1);
                    }
                }
                continue;
            }
            self.frames.push_back(f);
        }
        due
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
    #[inline]
    pub(crate) fn next_tok(&mut self) -> R<Option<Tok>> {
        // (the common case: the next token of the list on top)
        if let Some(Frame::Toks(t, p)) = self.frames.front_mut()
            && let Some(&tok) = t.get(*p)
        {
            *p += 1;
            return Ok(Some(tok));
        }
        self.next_tok_slow()
    }

    #[inline(never)]
    fn next_tok_slow(&mut self) -> R<Option<Tok>> {
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
                        let tok = t[*p];
                        *p += 1;
                        return Ok(Some(tok));
                    }
                    self.pop_front_marked();
                }
                Frame::Src(_) => {
                    if let Some(t) = self.src_next()? {
                        return Ok(Some(t));
                    }
                    self.pop_front_marked();
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
                        let src = f.take_src();
                        self.frames.push_front(Frame::Src(src));
                    } else {
                        self.pop_front_marked();
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
            let (starts, ends) = (s.starts, s.ends);
            // §360: `\endlinechar` as it is when the line is read goes
            // after its last character (an assignment on the line itself
            // reaches the next line only: `\endlinechar=-1` on a line
            // still ends that line)
            let e = if ends {
                i64::from(self.ipar(Par::endlinechar)?)
            } else {
                -1
            };
            // §362: a line is counted as it begins
            if starts {
                self.lines_entered += 1;
                let line = self.v.int(1);
                self.contribute("lines", line);
            }
            if let Some(Frame::Src(s)) = self.frames.front_mut() {
                if (0..256).contains(&e) {
                    s.buf.push(u32::try_from(e).unwrap_or(13));
                }
                s.endch = e;
                if starts {
                    s.state = NEW_LINE;
                }
            }
        }
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
            let done = matches!(self.frames.front(), Some(Frame::Src(s)) if s.loc >= s.buf.len());
            if done {
                return Ok(None);
            }
        }
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
            return Ok(Tok::cs(
                char::from_u32(c).unwrap_or('?').encode_utf8(&mut b),
            ));
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
        self.irreversible += 1;
        true
    }

    /// `get_next` with the meaning (§357–§358).
    // the per-token hot path: inlined on purpose
    #[allow(clippy::inline_always)]
    #[inline(always)]
    pub(crate) fn get_next(&mut self) -> R<bool> {
        let b = self.fetch();
        match self.halt.take() {
            Some(h) => Err(h),
            None => Ok(b),
        }
    }

    /// `get_next` without a `Result` (§341–§357): false at the end of all
    /// input; a halt is raised ([`Self::raise`]).
    #[allow(clippy::inline_always)]
    #[inline(always)]
    fn fetch(&mut self) -> bool {
        // (the common case: a token of the list on top that is not an
        // internal marker, and no alignment entry to watch for)
        let next = match self.frames.front_mut() {
            Some(Frame::Toks(t, p)) => match t.get(*p) {
                Some(&tok) if tok.as_cs().is_none_or(|n| !n.frozen()) => {
                    *p += 1;
                    Some(tok)
                }
                _ => None,
            },
            _ => None,
        };
        if let Some(tok) = next {
            self.set_cur_flag(tok);
            if ALIGN_CMDS.checked_shr(u32::from(self.cur.cmd)).unwrap_or(0) & 1 != 0 {
                match self.align_token() {
                    Ok(true) => return self.fetch_slow(),
                    Ok(false) => {}
                    Err(h) => self.raise(h),
                }
            }
            return true;
        }
        self.fetch_slow()
    }

    #[inline(never)]
    fn fetch_slow(&mut self) -> bool {
        match self.get_next_slow() {
            Ok(b) => b,
            Err(h) => {
                self.raise(h);
                true
            }
        }
    }

    #[inline(never)]
    fn get_next_slow(&mut self) -> R<bool> {
        loop {
            let t = loop {
                let Some(t) = self.next_tok()? else {
                    return Ok(false);
                };
                if let Some(n) = t.as_cs()
                    && n.frozen()
                {
                    if n.starts_with("\u{10FFFE}eof") {
                        // §362: the end of an \input file (or a pseudo file)
                        crate::etex::end_of_file(self, n.name())?;
                        continue;
                    }
                    if n.name() == "\u{10FFFE}uend" {
                        // §324: the end of a u template
                        self.align_u_end();
                        continue;
                    }
                }
                break t;
            };
            if let Some(n) = t.as_cs()
                && n.frozen()
                && let Some(k) = n.strip_prefix("\u{10FFFE}endv:")
            {
                // §789: the end of a v template (`endv`, §380)
                let chr = k.parse().unwrap_or(0);
                self.cur = Cur {
                    tok: t,
                    cmd: cmd::IGNORE,
                    chr,
                };
                return Ok(true);
            }
            if !self.get_next_rest(t)? {
                return Ok(false);
            }
            if matches!(
                self.cur.cmd,
                cmd::LEFT_BRACE | cmd::RIGHT_BRACE | cmd::TAB_MARK | cmd::CAR_RET
            ) && self.align_token()?
            {
                continue;
            }
            return Ok(true);
        }
    }

    fn get_next_rest(&mut self, t: Tok) -> R<bool> {
        if t == crate::expand::notexpanded_tok() {
            // §358
            let Some(t2) = self.next_tok()? else {
                return Ok(false);
            };
            self.set_cur(t2)?;
            if self.cur.cmd > cmd::MAX_COMMAND {
                self.cur.cmd = cmd::RELAX;
                self.cur.chr = crate::val::NO_EXPAND_FLAG;
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
            };
        }
        Ok(())
    }

    /// The token `get_r_token` reads (§1215): a control sequence's
    /// meaning is not looked up unless an alignment of this run watches
    /// for `&` and `\cr` (§342); `\outer` is not checked here (§336
    /// at `normal`).
    pub(crate) fn get_name_token(&mut self) -> R<()> {
        self.bare = self.aopen <= 0;
        let r = self.get_token();
        self.bare = false;
        r
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
            // (a macro: its body goes on top here, no expansion level)
            if (cmd::CALL..cmd::END_TEMPLATE).contains(&self.cur.cmd) {
                self.macro_call()?;
                continue;
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
                            run.push(crate::val::tok_id(v, epoch, *tok));
                            continue;
                        }
                        if !run.is_empty() {
                            out.push(node(std::mem::take(&mut run)));
                        }
                        out.push(node(vec![crate::val::tok_id(v, epoch, *tok)]));
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
                self.begin_command();
                self.command()?;
                self.mark.active = false;
                if self.claim_rest || self.closed_scope() {
                    return Ok(());
                }
            }
            // a region that ends with one group of its own still open
            // (an environment's `\begin`, a macro's `\begingroup` or
            // `\bgroup`), outside every paragraph, box and conditional,
            // leaves it to the engine as the scope the following regions
            // run in (`Run::leave_scope`); more than one, or a text that
            // said it opens no scope, reads on
            if self.doc
                && self.conds.is_empty()
                && self.effect == Effect::None
                && self.nest.len() == 1
                && !self.nest[0].owns
                && matches!(
                    self.nest[0].group,
                    crate::val::group::SIMPLE | crate::val::group::SEMI_SIMPLE
                )
                && !crate::hints::effect_none(self.v, &self.cx.args())
            {
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
        if self.st.defines_any(&[
            Par::vsize as Loc,
            Par::maxdepth as Loc,
            Par::topskip as Loc,
            Par::holdinginserts as Loc,
        ]) {
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
            let item = v.list(vec![v.int(0), marker]);
            let s = v.seq_unit(item);
            self.contribute("items", s);
        }
        // a page step defines nothing to the engine: its definitions are
        // the page chain's delta (`page::step_defs`), so no step reads
        // another
        let op = self.cx.op();
        let t_pub = std::time::Instant::now();
        let published: Vec<(Field, ValueId, Locality)> = self
            .st
            .published(self.v)
            .into_iter()
            .map(|(loc, x, l)| (self.loc_field(loc), x, l))
            .collect();
        if op == self.ops.pagestep || op == self.ops.pagestep_any {
            crate::page::step_defs(&mut self, published);
        } else {
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
        let locs: Vec<Loc> = self.st.defined_locs().collect();
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
/// (a body is written into one, not allocated, [`pooled_toks`]).
const POOL_LEN: usize = 64;

thread_local! {
    static TOK_POOL: std::cell::RefCell<Vec<Vec<Rc<[Tok]>>>> =
        std::cell::RefCell::new(vec![Vec::new(); POOL_LEN]);
}

/// A shared list holding `xs`: a spent one of its length reused if any.
pub(crate) fn pooled_toks(xs: &[Tok]) -> Rc<[Tok]> {
    if xs.len() < POOL_LEN
        && let Some(mut r) = TOK_POOL.with(|p| p.borrow_mut()[xs.len()].pop())
        && let Some(m) = Rc::get_mut(&mut r)
    {
        m.copy_from_slice(xs);
        return r;
    }
    Rc::from(xs)
}

/// A token list read to its end: kept for [`pooled_toks`] if nothing
/// else holds it.
fn recycle_toks(t: Rc<[Tok]>) {
    if !t.is_empty() && t.len() < POOL_LEN && Rc::strong_count(&t) == 1 && Rc::weak_count(&t) == 0 {
        TOK_POOL.with(|p| {
            let mut p = p.borrow_mut();
            let v = &mut p[t.len()];
            if v.len() < 32 {
                v.push(t);
            }
        });
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

enum PFrame {
    Toks(Vec<Tok>),
    Src(Src),
    File(FileRest),
}

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
    frames: Vec<PFrame>,
    pulled: usize,
    top: bool,
    claim_rest: bool,
    nest: Vec<PLevel>,
    next_lid: i64,
    last_par: (i64, i64),
    doc: bool,
    fread: Vec<(String, u64)>,
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
    eof: bool,
    radix: i32,
    cur_order: phitex_typeset::glue::Order,
    pdf_occ: usize,
    irreversible: u64,
}

/// Where an input frame stood when the command began.
enum Snap {
    Toks(Rc<[Tok]>, usize),
    Src {
        loc: usize,
        state: u8,
        endch: i64,
        len: usize,
    },
    File(usize, usize),
    None,
}

/// The command in progress (`Run::begin_command`): the input frames as it
/// found them, the ones it used up, and a fingerprint of the rest of the
/// run's state.
#[derive(Default)]
struct Mark {
    active: bool,
    /// `frames.len()` at the start.
    len: usize,
    /// The frames of the start still in `frames` (its tail).
    orig_left: usize,
    /// The start's frames, front first, as each was when it was reached.
    snaps: Vec<Snap>,
    /// The start's frames the command used up, front first.
    popped: Vec<Frame>,
    sig: u64,
    effect: Option<Effect>,
    astate: Option<i64>,
    aopen: i64,
    radix: i32,
    cur_order: Option<phitex_typeset::glue::Order>,
    no_pull: bool,
}

impl Run<'_, '_> {
    fn snap_front(&self) -> Snap {
        match self.frames.front() {
            Some(Frame::Toks(t, p)) => Snap::Toks(t.clone(), *p),
            Some(Frame::Src(s)) => Snap::Src {
                loc: s.loc,
                state: s.state,
                endch: s.endch,
                len: s.buf.len(),
            },
            Some(Frame::File(f)) => Snap::File(f.off, f.pos),
            None => Snap::None,
        }
    }

    /// What a stopped command must not have changed to be undone by
    /// rewinding its input.
    fn state_sig(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = phitex_defuse::FxHasher::default();
        (
            self.st.sig(),
            self.contribs.len(),
            self.kids.len(),
            self.nest.len(),
            self.conds.len(),
            self.if_codes.len(),
            self.next_lid,
            self.irreversible,
            self.fread.len(),
        )
            .hash(&mut h);
        (
            self.pulled,
            self.eof,
            self.last_par,
            self.claim_rest,
            self.pdf_occ,
            self.lines_entered,
            self.top,
        )
            .hash(&mut h);
        for l in &self.nest {
            (
                l.list.len(),
                l.list.last(),
                l.ag.len(),
                l.save_base,
                l.prev_graf,
                l.fold.sf,
                l.fold.pd,
                l.fold.since_bp,
                l.par,
                l.extra.len(),
            )
                .hash(&mut h);
        }
        h.finish()
    }

    /// A command begins: marks the input and the state it starts from.
    pub(crate) fn begin_command(&mut self) {
        let snap = self.snap_front();
        let sig = self.state_sig();
        let m = &mut self.mark;
        m.active = true;
        m.len = self.frames.len();
        m.orig_left = m.len;
        m.snaps.clear();
        m.snaps.push(snap);
        m.popped.clear();
        m.sig = sig;
        m.effect = Some(self.effect);
        m.astate = self.astate;
        m.aopen = self.aopen;
        m.radix = self.radix;
        m.cur_order = Some(self.cur_order);
        m.no_pull = self.no_pull;
    }

    /// `frames.pop_front()`, keeping a frame the command in progress found
    /// (and the state of the one it reaches next) for [`Run::rewind`].
    fn pop_front_marked(&mut self) {
        let f = self.frames.pop_front();
        if self.mark.active && self.frames.len() < self.mark.orig_left {
            self.mark.orig_left -= 1;
            if let Some(f) = f {
                self.mark.popped.push(f);
            }
            let s = self.snap_front();
            self.mark.snaps.push(s);
        } else if let Some(Frame::Toks(t, _)) = f {
            recycle_toks(t);
        }
    }

    /// After a stop inside a command: puts the input back as the command
    /// found it, so a resumed run executes the command from its start.
    /// False (the run must not park) if the command changed anything
    /// else, or the input in a way not recorded.
    fn rewind(&mut self) -> bool {
        if !self.mark.active
            || self.pg.is_some()
            || self.state_sig() != self.mark.sig
            || Some(self.effect) != self.mark.effect
            || self.frames.len() < self.mark.orig_left
        {
            return false;
        }
        let m = std::mem::take(&mut self.mark);
        for _ in m.orig_left..self.frames.len() {
            self.frames.pop_front();
        }
        for f in m.popped.into_iter().rev() {
            self.frames.push_front(f);
        }
        if self.frames.len() != m.len {
            return false;
        }
        for (i, s) in m.snaps.into_iter().enumerate() {
            match (s, self.frames.get_mut(i)) {
                (Snap::Toks(t, p), Some(Frame::Toks(ft, fp))) => {
                    *ft = t;
                    *fp = p;
                }
                (
                    Snap::Src {
                        loc,
                        state,
                        endch,
                        len,
                    },
                    Some(Frame::Src(x)),
                ) => {
                    if x.buf.len() < len {
                        return false;
                    }
                    x.buf.truncate(len);
                    x.loc = loc;
                    x.state = state;
                    x.endch = endch;
                }
                (Snap::File(off, pos), Some(Frame::File(x))) => {
                    x.off = off;
                    x.pos = pos;
                }
                (Snap::None, None) => {}
                _ => return false,
            }
        }
        self.astate = m.astate;
        self.aopen = m.aopen;
        self.radix = m.radix;
        self.no_pull = m.no_pull;
        if let Some(o) = m.cur_order {
            self.cur_order = o;
        }
        true
    }

    /// The run's state for [`phitex_defuse::Ctx::park`] after a stop:
    /// `None` if the stopped command cannot be rewound.
    pub(crate) fn park(&mut self) -> Option<Parked> {
        if !self.rewind() {
            return None;
        }
        let frames = std::mem::take(&mut self.frames)
            .into_iter()
            .map(|f| match f {
                Frame::Toks(t, p) => PFrame::Toks(t[p.min(t.len())..].to_vec()),
                Frame::Src(s) => PFrame::Src(s),
                Frame::File(f) => PFrame::File(f),
            })
            .collect();
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
            frames,
            pulled: self.pulled,
            top: self.top,
            claim_rest: self.claim_rest,
            nest,
            next_lid: self.next_lid,
            last_par: self.last_par,
            doc: self.doc,
            fread: std::mem::take(&mut self.fread),
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
            eof: self.eof,
            radix: self.radix,
            cur_order: self.cur_order,
            pdf_occ: self.pdf_occ,
            irreversible: self.irreversible,
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
        r.frames = p
            .frames
            .into_iter()
            .map(|f| match f {
                PFrame::Toks(t) => Frame::Toks(t.into(), 0),
                PFrame::Src(s) => Frame::Src(s),
                PFrame::File(f) => Frame::File(f),
            })
            .collect();
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
        r.top = p.top;
        r.claim_rest = p.claim_rest;
        r.next_lid = p.next_lid;
        r.last_par = p.last_par;
        r.doc = p.doc;
        r.fread = p.fread;
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
        r.eof = p.eof;
        r.radix = p.radix;
        r.cur_order = p.cur_order;
        r.pdf_occ = p.pdf_occ;
        r.irreversible = p.irreversible;
        r.resumed = true;
        Ok(r)
    }
}
