//! The graph: a node per command.
//!
//! The text is cut at blank lines (brace depth 0) into **pieces**: places
//! where reading can start, because the reader state there is known
//! (vertical mode, no group, no conditional, no rewritten input). Reading
//! a piece makes a chain of **nodes, one per command**. A node's operands
//! are the reader state it started from (its predecessor's output:
//! position, tokenizer state, rewritten input, mode, group depth, open
//! conditionals) and the exact definitions it read, each from the node
//! that made it: a node earlier in the piece ([`Src::Node`]), a node of an
//! earlier piece ([`Src::Piece`]), or the value before any definition
//! ([`Src::Initial`]). Its results are its definitions, what it emitted
//! and the reader state after it. A macro call is a step inside the node
//! whose command it produces: it rewrites the input and defines nothing.
//!
//! Names resolve over the nodes: the definition of a key reaching node
//! `i` is the latest before `i` still in scope (its group not closed
//! before `i`, or global). A `}` is a node that defines nothing. At `\fi`
//! (or the `\else` ending the taken arm) a node defines each key the arm
//! defined as an explicit φ of the value after the arm and the value
//! before the `\if`. `\par` is a node whose operands are the material the
//! paragraph's nodes emitted and the parameters it read; its result is the
//! paragraph, a value line breaking is a function of.
//!
//! [`run_piece`] runs a piece again from exactly the nodes whose operands
//! may have changed: the dirty nodes (every node reading a name from before
//! the piece whose definition was remade) and the nodes the edited text
//! touches. Everything before the first of them is kept as it is. From
//! there each old node is kept when its input state is the state reached,
//! its text is unchanged and every definition it read is still equal;
//! otherwise a node runs. Once the chain is back on the old one at the same
//! index with no scope changed, old nodes are shared as they are up to the
//! next dirty node.

use std::sync::Arc;

use crate::hash::{FxMap, FxSet};
pub use crate::item::{Item, Para};
use crate::page::IGNORE_DEPTH;
use crate::tfm::Font;
use crate::val::{DimenPar, IntPar, Key, Macro, Meaning, Prim, Tok, Val};
use crate::show::{HALT, NL, RAW, THAT_MAKES_100};

mod align;
mod assign;
mod boxes;
pub(crate) mod diag;
mod etex;
mod expand;
mod fonts;
mod io;
mod lang;
mod material;
mod math;
pub mod output;
mod scan;

pub use expand::JOBNAME;
pub use io::{ends_document, file_text, page_io, whatsits};

/// A piece's identity, stable across edits.
pub type PieceId = u32;

/// Where a piece's reads of names from before it are answered.
pub trait Env {
    /// The build's control sequence names (`Tok::Cs` numbers).
    fn cs(&self) -> &Arc<crate::build::CsTable>;

    /// The definition of `key` reaching node `node` of this piece from
    /// before the piece (or from an element between its nodes), where it
    /// was made, and the nodes it reaches, `lo..hi` (`0..u32::MAX`: all of
    /// them, as when no element between the piece's nodes defines `key`).
    fn reach(&mut self, key: &Key, node: u32) -> (Val, Src, u32, u32);

    /// A definition of `key` made between this piece's nodes `after` and
    /// `node` (an output routine's run where the page builder fired there),
    /// which reaches `node` instead of node `after`'s own: its value and
    /// where it was made.
    fn between(&mut self, _key: &Key, _after: u32, _node: u32) -> Option<(Val, Src)> {
        None
    }

    /// Whether anything between this piece's nodes may define `key`.
    fn has_between(&mut self, _key: &Key) -> bool {
        false
    }

    /// Whether the run may end at a cut the cutter missed ([`Kind::Cut`]):
    /// the sequence may still change.
    fn cut_ok(&mut self) -> bool {
        false
    }

    /// Whether anything between this piece's nodes may define any name
    /// (unknown: yes).
    fn any_between(&mut self) -> bool {
        true
    }

    /// Whether the run goes on after a node it ran (`ran` run so far, the
    /// run holding `words` of material, [`words_of`]): a speculating run
    /// whose predictions are refuted is abandoned; one past TeX's main
    /// memory ends (tex.web 120).
    fn check(&mut self, _ran: u64, words: u64) -> Check {
        if words > MAIN_MEMORY { Check::Exceeded(MAIN_MEMORY) } else { Check::Go }
    }

    /// The bytes of the TFM file `name.tfm`, if there is one (a pure
    /// function of the name during a build).
    fn font_file(&mut self, _name: &str) -> Option<std::sync::Arc<[u8]>> {
        None
    }

    /// The bytes of the input file of exactly this name, if there is one:
    /// what a read of `Key::File(name)` answers (a pure function of the
    /// name during a build).
    fn file(&mut self, _name: &str) -> Option<Arc<[u8]>> {
        None
    }

    /// Where the input file of this name was found, as TeX Live shows it
    /// (`./name` in the current directory, else its path): a pure function
    /// of the name during a build, read with the file (`Key::File`).
    fn file_path(&mut self, name: &str) -> Arc<str> {
        Arc::from(format!("./{name}"))
    }

    /// The job's name (`\jobname`): what a read of `Key::Job` answers.
    fn jobname(&mut self) -> Arc<str> {
        Arc::from(expand::JOBNAME)
    }

    /// The lines of its file before this piece (`Key::LineBase`).
    fn line_base(&mut self) -> i32 {
        0
    }
}

/// web2c's `main_memory` (tex.web 11's `mem_max`): words a run's material
/// may hold ([`words_of`]) before "TeX capacity exceeded".
pub const MAIN_MEMORY: u64 = 5_000_000;

/// What [`Env::check`] says of a run.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Check {
    Go,
    /// The run is dropped (not installed): it is run again.
    Abandon,
    /// "TeX capacity exceeded, sorry [main memory size=N]" (tex.web 94,
    /// 120): the run ends there, a fatal error.
    Exceeded(u64),
}

/// The words of material a node makes as TeX's memory would hold them (an
/// item, a box's items; and one for the node, which the run holds while it
/// runs): the measure of [`MAIN_MEMORY`].
pub fn words_of(n: &Node) -> u64 {
    let items = |v: &[crate::item::Item]| -> u64 {
        v.iter()
            .map(|it| match it {
                crate::item::Item::Box(b) => 1 + b.list.len() as u64,
                _ => 1,
            })
            .sum()
    };
    // (material, and boxes defined: what a loop that runs away piles up;
    // macros and token lists TeX frees when they are defined again)
    let boxes: u64 = n
        .defs()
        .iter()
        .map(|d| match &d.val {
            Val::Box(b) => 1 + b.list.len() as u64,
            _ => 0,
        })
        .sum();
    1 + items(&n.emit) + items(n.vemit()) + boxes
}

/// The input pending in `st`, as words (tex.web 120 counts it in the
/// memory too): the lists waiting, and the top list's tokens left (the
/// lists below are shared from node to node: counted once each).
pub fn pending_words(st: &RState) -> u64 {
    let top = st.pending.top.as_ref().map_or(0, |l| l.end.saturating_sub(l.at));
    st.pending.len() as u64 + top as u64
}

/// The fatal error of a run past TeX's main memory.
pub fn capacity_error(size: u64) -> String {
    format!("TeX capacity exceeded, sorry [main memory size={size}]")
}

/// The tokenizer's state (tex.web 303).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum TState {
    N,
    M,
    S,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    V,
    H,
    /// Math (`RState::inner`: an inline formula or a subformula; else a
    /// display).
    M,
}

/// What opened a group (only to report mismatches: the scoping itself is
/// the reaching definitions over the `Open`/`Close` nodes).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum GroupKind {
    /// `{`.
    Simple,
    /// `\begingroup`.
    SemiSimple,
    /// A box's or a discretionary's braces (`Group::boxg`).
    Box,
    /// An output routine's braces (tex.web 1025: `output_group`).
    Output,
    /// An alignment's group, and each of its entries' (tex.web's
    /// `align_group`; the alignment's carries `Group::align`).
    Align,
    /// `\noalign{...}`.
    NoAlign,
    /// A formula, a subformula, `\left`…`\right`, a `\mathchoice` part
    /// (`Group::boxg`, `math::MathKind`).
    Math,
}

/// An open conditional: the node whose step opened it, which ends may end
/// it now (tex.web 489: 1 while its test is read, 2 `\fi`, 3 `\else` or
/// `\fi`, 4 `\or` too), and its test.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Cond {
    open: u32,
    limit: u8,
    test: bool,
    /// Which conditional (`IfKind::code`, plus 32 after `\unless`:
    /// etex.web's `cur_if`), for `\currentiftype`.
    code: u8,
}

/// A text the reader is in above the piece's (TeX's input levels that
/// read characters, as a value of the reader state): e-TeX's
/// `\scantokens` text, an `\input` file (read in the piece until the core
/// cuts files into pieces: CORE.md, "`\input` (decided)") or a `\read`
/// line. Read with the category codes in force as it is read, line by
/// line, each line ending in the `\endlinechar` in force when it is begun.
/// It lies above the first `at` rewritten token lists: those above it are
/// read first, then it, then those below. Where its reading is: `pos`,
/// `tstate`, `eol` (as the piece's own are kept in `RState`); `eof`: its
/// end was met once and `\everyeof` went in (etex.web §362); `endinput`:
/// `\endinput` was met, it ends with its current line (tex.web 362).
#[derive(Clone, PartialEq, Eq, Debug)]
struct PendingText {
    text: Arc<[char]>,
    at: usize,
    pos: usize,
    tstate: TState,
    eol: i16,
    eof: bool,
    kind: TextKind,
    endinput: bool,
}

/// What a [`PendingText`] is.
#[derive(Clone, PartialEq, Eq, Debug)]
enum TextKind {
    /// `\scantokens`'s pseudo file.
    Scan,
    /// An `\input` file, as the log names it (`./sub.tex`): its end is
    /// `)` in the log.
    File(Arc<str>),
    /// A `\read` line: its end ends the reading (`next_raw` is `None`);
    /// `\read` leaves it.
    Read,
}

/// An open group: its kind and the tokens `\aftergroup` gave it (its
/// close rewrites the input with them).
#[derive(Clone, PartialEq, Eq, Debug)]
struct Group {
    kind: GroupKind,
    after: Vec<Tok>,
    /// A box group's specification and context (tex.web 1083).
    boxg: Option<Arc<material::BoxGroup>>,
    /// The node that opened it (where it was entered, for e-TeX's
    /// `\showgroups`).
    at: u32,
    /// An alignment's preamble and what it began in; a display's
    /// dimensions and, once its alignment is set, its material.
    align: Option<Arc<align::AlignSpec>>,
}

/// A stack in the reader state, shared until changed: handing the state
/// from a node to the next copies nothing.
#[derive(Clone, Debug)]
struct Stack<T>(Option<Arc<Vec<T>>>);

impl<T: Clone> Stack<T> {
    const fn new() -> Self {
        Stack(None)
    }
    fn len(&self) -> usize {
        self.0.as_ref().map_or(0, |v| v.len())
    }
    fn get(&self, i: usize) -> Option<&T> {
        self.0.as_ref().and_then(|v| v.get(i))
    }
    fn is_empty(&self) -> bool {
        self.0.is_none()
    }
    fn last(&self) -> Option<&T> {
        self.0.as_ref().and_then(|v| v.last())
    }
    fn get_mut(&mut self, i: usize) -> Option<&mut T> {
        self.0.as_mut().and_then(|v| Arc::make_mut(v).get_mut(i))
    }
    fn last_mut(&mut self) -> Option<&mut T> {
        let n = self.len();
        n.checked_sub(1).and_then(|i| self.get_mut(i))
    }
    fn push(&mut self, x: T) {
        Arc::make_mut(self.0.get_or_insert_with(Default::default)).push(x);
    }
    fn pop(&mut self) -> Option<T> {
        let v = Arc::make_mut(self.0.as_mut()?);
        let x = v.pop();
        if v.is_empty() {
            self.0 = None;
        }
        x
    }
}

impl Stack<Cond> {
    /// The conditionals open, as a piece entered here sees them: opened
    /// before it (no node of its own: their `\fi` makes no φ there).
    fn outside(&self) -> Stack<Cond> {
        match &self.0 {
            None => Stack(None),
            Some(v) => Stack(Some(Arc::new(
                v.iter()
                    .map(|c| Cond {
                        open: u32::MAX,
                        ..*c
                    })
                    .collect(),
            ))),
        }
    }
}

impl<T: PartialEq> PartialEq for Stack<T> {
    fn eq(&self, other: &Self) -> bool {
        match (&self.0, &other.0) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b) || a == b,
            (None, None) => true,
            _ => false,
        }
    }
}

impl<T: Eq> Eq for Stack<T> {}

/// The token lists the input was rewritten with (innermost on top), each
/// with how far it has been read and whether it was backed up. The top
/// list is held in the state itself (reading a token changes only it);
/// the ones below are a list of cells, each holding the ones below it,
/// shared until changed: handing the state from a node to the next copies
/// the top list's handles only, and a pop leaves the cells below shared.
/// (A value: two are equal when their lists, positions and flags are.)
#[derive(Clone, Default)]
struct Pending {
    top: Option<Level>,
    below: Option<Arc<PCell>>,
    /// The lists, the top one included.
    depth: u32,
}

/// A macro's argument: tokens `start..end` of a list (of the input list it
/// was scanned in, shared, or its own).
#[derive(Clone, PartialEq, Eq, Debug)]
struct Arg {
    list: Arc<[Tok]>,
    start: usize,
    end: usize,
}

impl Arg {
    /// The tokens of `v` (none: an empty part of `filler`).
    fn own(v: Vec<Tok>, filler: &Arc<[Tok]>) -> Arg {
        if v.is_empty() {
            return Arg { list: filler.clone(), start: 0, end: 0 };
        }
        let end = v.len();
        Arg { list: Arc::from(v), start: 0, end }
    }
}

/// A place in the top input list as the reader reads it: the list's
/// next token `k`, or inside the argument its parameter at `k - 1` is
/// read as (argument `n`, its next token `q`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Cursor {
    k: usize,
    arg: Option<(usize, usize)>,
}

impl Cursor {
    fn at(l: &Level) -> Cursor {
        Cursor { k: l.at, arg: l.sub }
    }

    /// Whether level `l`'s argument, if it is reading one, is read.
    fn sub_done(l: &Level) -> bool {
        match (l.sub, &l.args) {
            (Some((n, q)), Some(a)) => q >= a[n].end,
            _ => true,
        }
    }

    /// The next token from here, as the reader reads it (an argument's
    /// tokens where its parameter is), and the place after it.
    fn next<'l>(&mut self, l: &'l Level) -> Option<&'l Tok> {
        loop {
            if let Some((n, q)) = self.arg {
                let a = &l.args.as_ref()?[n];
                if q < a.end {
                    self.arg = Some((n, q + 1));
                    return Some(&a.list[q]);
                }
                self.arg = None;
            }
            if self.k >= l.end {
                return None;
            }
            let t = &l.list[self.k];
            self.k += 1;
            match (t, &l.args) {
                (Tok::Param(n), Some(a)) => {
                    // (a parameter without an argument reads as nothing)
                    if let Some(i) = usize::from(*n).checked_sub(1)
                        && let Some(a) = a.get(i)
                    {
                        self.arg = Some((i, a.start));
                    }
                }
                _ => return Some(t),
            }
        }
    }

    /// The place of the token just read (`self` the place after it).
    fn back(self, l: &Level) -> Cursor {
        match self.arg {
            Some((n, q)) if l.args.as_ref().is_some_and(|a| q > a[n].start) => Cursor { k: self.k, arg: Some((n, q - 1)) },
            _ => Cursor { k: self.k - 1, arg: None },
        }
    }

    /// The tokens from `a` to `b` as the reader reads them: a part of one
    /// list when they are, else their own list.
    fn arg(l: &Level, a: Cursor, b: Cursor) -> Arg {
        let norm = |c: Cursor| match c.arg {
            Some((n, q)) if l.args.as_ref().is_some_and(|x| q >= x[n].end) => Cursor { k: c.k, arg: None },
            _ => c,
        };
        let (a, b) = (norm(a), norm(b));
        // (one parameter: its argument)
        if let (None, None, Some(x)) = (a.arg, b.arg, &l.args)
            && b.k == a.k + 1
            && let Tok::Param(n) = l.list[a.k]
            && let Some(arg) = usize::from(n).checked_sub(1).and_then(|n| x.get(n))
        {
            return arg.clone();
        }
        match (a.arg, b.arg) {
            (None, None) if !l.list[a.k..b.k].iter().any(|t| matches!(t, Tok::Param(_))) || l.args.is_none() => {
                
                return Arg { list: l.list.clone(), start: a.k, end: b.k };
            }
            (Some((n, p)), Some((m, q))) if n == m && a.k == b.k => {
                let x = &l.args.as_ref().expect("an argument")[n];
                
                return Arg { list: x.list.clone(), start: p, end: q };
            }
            _ => {}
        }
        
        let mut v = Vec::new();
        let mut c = a;
        while c != b {
            match c.next(l) {
                Some(t) => v.push(t.clone()),
                None => break,
            }
            c = norm(c);
        }
        let end = v.len();
        Arg { list: Arc::from(v), start: 0, end }
    }
}

/// A macro body's arguments, by parameter number (`#1` first).
type Args = Arc<[Arg]>;

/// One list of [`Pending`].
#[derive(Clone, PartialEq, Eq, Debug)]
struct Level {
    list: Arc<[Tok]>,
    /// A macro's body: its arguments, which its parameter tokens are read
    /// as (each a list pushed above the body where it is met, tex.web 357's
    /// `out_param`); the body itself is the macro's, shared.
    args: Option<Args>,
    at: usize,
    /// Where the list ends (an argument is a part of one).
    end: usize,
    /// Reading the argument the parameter at `at - 1` is (argument `.0`,
    /// its next token `.1`): an argument is read where its parameter is,
    /// in the body's list (tex.web 357 makes it an input level of its own).
    sub: Option<(usize, usize)>,
    backed: bool,
}

struct PCell {
    level: Level,
    below: Option<Arc<PCell>>,
}

impl Pending {
    fn len(&self) -> usize {
        self.depth as usize
    }
    fn push_level(&mut self, l: Level) {
        if let Some(t) = self.top.take() {
            let below = self.below.take();
            self.below = Some(Arc::new(PCell { level: t, below }));
        }
        self.top = Some(l);
        self.depth += 1;
    }
    fn push(&mut self, (list, at, backed): (Arc<[Tok]>, usize, bool)) {
        let end = list.len();
        self.push_level(Level { list, args: None, at, end, sub: None, backed });
    }
    /// A macro's body, to be read with its arguments.
    fn push_body(&mut self, list: Arc<[Tok]>, args: Args) {
        let end = list.len();
        self.push_level(Level { list, args: Some(args), at: 0, end, sub: None, backed: false });
    }
    fn pop(&mut self) {
        if self.top.take().is_none() {
            return;
        }
        self.depth -= 1;
        if let Some(c) = self.below.take() {
            let (level, below) = match Arc::try_unwrap(c) {
                Ok(PCell { level, below }) => (level, below),
                Err(c) => (c.level.clone(), c.below.clone()),
            };
            self.top = Some(level);
            self.below = below;
        }
    }
    fn clear(&mut self) {
        *self = Pending::default();
    }
    /// The top list, how far it has been read, where it ends, whether it
    /// was backed up.
    fn last(&self) -> Option<(&Arc<[Tok]>, usize, usize, bool)> {
        self.top.as_ref().map(|t| (&t.list, t.at, t.end, t.backed))
    }
    /// Whether the top list has been read to its end (none: no list).
    fn top_done(&self) -> Option<bool> {
        self.top.as_ref().map(|t| t.at >= t.end && Cursor::sub_done(t))
    }
    /// The rest of the top list is skipped.
    fn skip_top(&mut self) {
        if let Some(t) = self.top.as_mut() {
            t.at = t.end;
            t.sub = None;
        }
    }
    /// The top list's next token, read (`None`: it has been read to its
    /// end): a macro body's parameter is read as its argument.
    fn next_tok(&mut self) -> Option<Tok> {
        let t = self.top.as_mut()?;
        if t.sub.is_none() && t.at < t.end {
            let x = &t.list[t.at];
            if t.args.is_none() || !matches!(x, Tok::Param(_)) {
                t.at += 1;
                return Some(x.clone());
            }
        }
        let mut c = Cursor { k: t.at, arg: t.sub };
        let x = c.next(t).cloned();
        t.at = c.k;
        t.sub = c.arg;
        x
    }
    fn levels(&self) -> impl Iterator<Item = &Level> {
        self.top.iter().chain(
            std::iter::successors(self.below.as_deref(), |c| c.below.as_deref()).map(|c| &c.level),
        )
    }
}

impl Drop for Pending {
    /// Cell by cell, not recursively (many lists pending).
    fn drop(&mut self) {
        let mut next = self.below.take();
        while let Some(c) = next {
            next = match Arc::try_unwrap(c) {
                Ok(mut c) => c.below.take(),
                Err(_) => None,
            };
        }
    }
}

impl PartialEq for Pending {
    fn eq(&self, other: &Self) -> bool {
        if self.depth != other.depth || self.top != other.top {
            return false;
        }
        let (mut a, mut b) = (self.below.as_ref(), other.below.as_ref());
        while let (Some(x), Some(y)) = (a, b) {
            if Arc::ptr_eq(x, y) {
                return true;
            }
            if x.level != y.level {
                return false;
            }
            (a, b) = (x.below.as_ref(), y.below.as_ref());
        }
        true
    }
}

impl Eq for Pending {}

impl std::fmt::Debug for Pending {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.levels()).finish()
    }
}

/// The value `RState::eol` holds before the line's end-of-line character
/// has been read.
const EOL_UNREAD: i16 = i16::MIN;

/// The reader state between two commands: a value, the next node's
/// operand.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RState {
    pub pos: usize,
    tstate: TState,
    /// Token lists the input was rewritten with, each with how far it has
    /// been read (innermost last).
    /// (and whether it was backed up, tex.web 325's `backed_up`: an error's
    /// context shows it as such; a list read to its end stays until the
    /// next token is read, as TeX's input level does)
    pending: Pending,
    mode: Mode,
    /// The list being built is internal: a box's (TeX's negative modes).
    inner: bool,
    /// Boxes open: the level of the list being built (0: the main vertical
    /// list and its paragraphs).
    level: u16,
    /// A box group just closed: what its packaging node reads.
    boxing: Option<Arc<material::BoxEnd>>,
    /// A `\global` prefix is waiting for its assignment.
    global: bool,
    /// Conditionals open (innermost last).
    conds: Stack<Cond>,
    /// `\long`, `\outer` prefixes waiting for their definition.
    long: bool,
    outer: bool,
    /// The groups open, innermost last.
    groups: Stack<Group>,
    /// The token `\afterassignment` gave: the next assignment rewrites the
    /// input with it.
    after_assignment: Option<Box<Tok>>,
    /// The list being built's `\spacefactor`, `\prevdepth`, `\prevgraf`.
    space_factor: i32,
    prev_depth: i32,
    prev_graf: i32,
    /// The current line's end-of-line character (`\endlinechar` as the line
    /// was begun; -1: none), or `EOL_UNREAD`.
    eol: i16,
    /// `\endinput` was read: the file ends after this line (tex.web 362).
    endinput: bool,
    /// The alignment being read: `align_state`, the current column and
    /// span, how the entry began and ended (tex.web 309, 770).
    al: align::AlignCur,
    /// e-TeX's `\protected` prefix is waiting for its definition.
    protected: bool,
    /// The texts the reader is in above the piece's (`\scantokens`
    /// texts, `\input` files, `\read` lines), innermost last.
    texts: Stack<PendingText>,
    /// A paragraph's language state (tex.web 1091, 1376): `clang`, and
    /// the language and hyphen minima it began with.
    hyph_lang: [u8; 4],
    /// In an output routine's run: what the page builder handed it where
    /// it fired (the same for all its nodes; `output::output_entry`).
    fire: Option<Arc<output::Fire>>,
    /// tex.web 76's `error_count`: the errors since the last paragraph
    /// ended (§1096); at [`FATAL_ERRORS`] the reader has stopped (§82).
    error_count: u8,
}

/// tex.web 82: the hundredth error is fatal.
const FATAL_ERRORS: u8 = 100;

impl RState {
    pub fn start() -> RState {
        RState {
            pos: 0,
            tstate: TState::N,
            pending: Pending::default(),
            mode: Mode::V,
            inner: false,
            level: 0,
            boxing: None,
            global: false,
            conds: Stack::new(),
            long: false,
            outer: false,
            groups: Stack::new(),
            after_assignment: None,
            space_factor: 1000,
            prev_depth: IGNORE_DEPTH,
            prev_graf: 0,
            eol: EOL_UNREAD,
            endinput: false,
            al: align::AlignCur::OFF,
            protected: false,
            texts: Stack::new(),
            hyph_lang: [0, 0, 1, 1],
            fire: None,
            error_count: 0,
        }
    }

    /// The entry of an `\input` file's first piece: the state the
    /// `\input` node left (its group, conditional, mode), at the file's
    /// first line; the token lists waiting are read after the file (they
    /// come back with the rest of the calling line, [`RState::rest`]).
    pub fn file_start(caller: &RState) -> RState {
        RState {
            pos: 0,
            tstate: TState::N,
            pending: Pending::default(),
            eol: EOL_UNREAD,
            endinput: false,
            conds: caller.conds.outside(),
            ..caller.clone()
        }
    }

    /// The entry of the rest of the line that called `\input` (`caller`,
    /// the `\input` node's state): what the file's last piece left
    /// (`file`; none: an empty file), with the caller's line and the token
    /// lists waiting when the file was opened.
    pub fn rest(file: Option<&RState>, caller: &RState) -> RState {
        let base = file.unwrap_or(caller);
        RState {
            pos: 0,
            tstate: caller.tstate,
            pending: caller.pending.clone(),
            eol: caller.eol,
            endinput: caller.endinput,
            conds: base.conds.outside(),
            ..base.clone()
        }
    }

    /// Whether a piece entered in this state may begin: no paragraph,
    /// box or group open (an `\input` inside one is not supported: a
    /// definition made inside would outlive the group).
    pub fn plain_boundary(&self) -> bool {
        self.mode == Mode::V && self.level == 0 && self.groups.is_empty()
    }

    /// A point a piece may begin at with the canonical state (CORE.md,
    /// re-cuts): the outer vertical list at a line's start, nothing open,
    /// pending or prefixed. (The paragraph-level fields a cut piece starts
    /// afresh with, as the cutter's pieces do: `\prevdepth` is the page's
    /// material.)
    pub fn cut_point(&self) -> bool {
        self.mode == Mode::V
            && !self.inner
            && self.level == 0
            && self.boxing.is_none()
            && !(self.global || self.long || self.outer || self.protected)
            && self.conds.is_empty()
            && self.groups.is_empty()
            && self.after_assignment.is_none()
            && self.pending.len() == 0
            && self.texts.is_empty()
            && !self.endinput
            && self.al == align::AlignCur::OFF
            && self.fire.is_none()
            && self.tstate == TState::N
    }

    /// The file ends after the current line (`\endinput`).
    pub fn ends_file(&self) -> bool {
        self.endinput
    }

    /// Inside an output routine (its group is the outermost): its
    /// vertical list is internal (TeX's `-vmode`), not the main one.
    fn in_output(&self) -> bool {
        self.groups
            .0
            .as_ref()
            .is_some_and(|v| v.first().is_some_and(|g| g.kind == GroupKind::Output))
    }
}

/// What kind of command a node was.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Other,
    /// `{`: opens a scope.
    Open,
    /// `}`: closes one (and defines nothing).
    Close,
    /// An `\if...` whose taken arm follows (the conditional is open).
    If,
    /// An `\if...` that skipped to its `\fi` (closed at once).
    IfClosed,
    /// `\else` or `\fi`: ends the conditional, with its φ.
    EndIf,
    /// `\par`: a paragraph.
    Par,
    /// The end of the piece.
    End,
    /// `\input`: the piece's run ends here, the file's pieces follow it.
    Input,
    /// A cut the text's cutter missed (CORE.md, re-cuts): a blank line the
    /// reader passed at a piece's canonical start; the run ends here, the
    /// rest of the text is cut again from here.
    Cut,
}

impl Kind {
    /// Whether the node changes how later names resolve.
    fn scopes(self) -> bool {
        matches!(
            self,
            Kind::Open | Kind::Close | Kind::If | Kind::IfClosed | Kind::EndIf
        )
    }
}

/// Where a definition a node read was made.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Src {
    /// Node `.0` of the same piece.
    Node(u32),
    /// A node of piece `.0`.
    Piece(PieceId),
    /// The value before any definition.
    Initial,
}

/// Two stacks equal but for the node indices their entries hold.
fn same_but_pos<T: Clone>(a: &Stack<T>, b: &Stack<T>, same: impl Fn(&T, &T) -> bool) -> bool {
    match (&a.0, &b.0) {
        (Some(x), Some(y)) => {
            Arc::ptr_eq(x, y)
                || (x.len() == y.len() && x.iter().zip(y.iter()).all(|(p, q)| same(p, q)))
        }
        (None, None) => true,
        _ => false,
    }
}

impl Cond {
    /// Equal but for the node that opened it (a position, not content).
    fn same_but_pos(self, o: Cond) -> bool {
        self.limit == o.limit && self.test == o.test && self.code == o.code
    }
}

impl Group {
    /// Equal but for the node that opened it (a position, not content).
    fn same_but_pos(&self, o: &Group) -> bool {
        self.kind == o.kind && self.after == o.after && self.boxg == o.boxg && self.align == o.align
    }
}

impl RState {
    /// The node indices its open groups and conditionals hold, moved by
    /// `map` (`None`: one of them has no place any more).
    fn moved(&self, map: impl Fn(u32) -> Option<u32>) -> Option<RState> {
        let mut st = self.clone();
        if let Some(v) = st.groups.0.as_mut() {
            for g in Arc::make_mut(v).iter_mut() {
                g.at = map(g.at)?;
            }
        }
        if let Some(v) = st.conds.0.as_mut() {
            for c in Arc::make_mut(v).iter_mut() {
                c.open = map(c.open)?;
            }
        }
        Some(st)
    }

    /// Equal but for the position (where in the text, and the node indices
    /// of where its open groups and conditionals began).
    fn eq_but_pos(&self, o: &RState) -> bool {
        self.tstate == o.tstate
            && self.mode == o.mode
            && self.inner == o.inner
            && self.level == o.level
            && self.boxing == o.boxing
            && self.global == o.global
            && same_but_pos(&self.conds, &o.conds, |a, b| a.same_but_pos(*b))
            && self.long == o.long
            && self.outer == o.outer
            && same_but_pos(&self.groups, &o.groups, Group::same_but_pos)
            && self.after_assignment == o.after_assignment
            && self.space_factor == o.space_factor
            && self.prev_depth == o.prev_depth
            && self.prev_graf == o.prev_graf
            && self.eol == o.eol
            && self.endinput == o.endinput
            && self.al == o.al
            && self.pending == o.pending
            && self.protected == o.protected
            && self.texts == o.texts
            && self.hyph_lang == o.hyph_lang
            && self.fire == o.fire
            && self.error_count == o.error_count
    }

    /// Whether a node going from `self` to `out` begins a paragraph of the
    /// main vertical list. (Inside a box no paragraph start resets what
    /// changed: the box's packaging reads all its nodes.)
    fn starts_para(&self, out: &RState) -> bool {
        self.mode == Mode::V
            && out.mode == Mode::H
            && self.level == 0
            && out.level == 0
            && !out.inner
    }
}

/// A read: an operand.
#[derive(Clone, Debug)]
pub struct Read {
    pub key: Key,
    pub src: Src,
    pub val: Val,
    /// The scope of the definition read, as the reading node sees it
    /// ([`Chain::scope_of`]): what a definition at the level of the one it
    /// replaces (`Def::level`) is made from.
    pub scope: Option<u16>,
    /// Read as of where the reading node's `.0`th open conditional began
    /// (a `\fi`'s φ reads the value from before its `\if`); `None`: as
    /// of the reading node. A depth, not a node: an edit moving the
    /// conditional's opener does not change the read.
    pub cond: Option<u16>,
}

/// A shared [`Read`].
pub type ReadRef = Arc<Read>;

impl Read {
    /// Whether `r` read the definition this one did (by identity, then
    /// equality of the value and of its scope).
    fn same(this: &ReadRef, r: &ReadRef) -> bool {
        Arc::ptr_eq(this, r) || (this.val == r.val && this.scope == r.scope)
    }
}

/// A definition: a result.
#[derive(Clone, Debug)]
pub struct Def {
    pub key: Key,
    pub val: Val,
    pub global: bool,
    /// A definition made at the level of the one it replaces (TeX's
    /// assignments without the save stack, CORE.md Conflict 7): `Some(k)`,
    /// it lives as long as the `k`-th group enclosing its node's own group
    /// is open (`Some(0)`: its own group, as `None`). Counted from the
    /// node, not an index, so it survives nodes moving.
    pub level: Option<u16>,
}

/// A φ at the end of a conditional: the test, and per key the value after
/// the taken arm and the value before the `\if`.
#[derive(Clone, Debug)]
pub struct Phi {
    pub test: bool,
    pub keys: Vec<(Key, Val, Val)>,
}

/// A short list: up to two items in place (what most nodes read and
/// emit), more on the heap.
#[derive(Clone, Debug, Default)]
pub enum Few<T> {
    #[default]
    Zero,
    One(T),
    Two([T; 2]),
    Many(Box<[T]>),
}

impl<T> Few<T> {
    /// Takes the items out of `v` (which keeps its capacity).
    fn take(v: &mut Vec<T>) -> Few<T> {
        match v.len() {
            0 => Few::Zero,
            1 => Few::One(v.pop().expect("one item")),
            2 => {
                let b = v.pop().expect("two items");
                let a = v.pop().expect("two items");
                Few::Two([a, b])
            }
            _ => Few::Many(v.drain(..).collect()),
        }
    }
}

impl<T> std::ops::Deref for Few<T> {
    type Target = [T];
    fn deref(&self) -> &[T] {
        match self {
            Few::Zero => &[],
            Few::One(x) => std::slice::from_ref(x),
            Few::Two(a) => a,
            Few::Many(b) => b,
        }
    }
}

impl<T> std::ops::DerefMut for Few<T> {
    fn deref_mut(&mut self) -> &mut [T] {
        match self {
            Few::Zero => &mut [],
            Few::One(x) => std::slice::from_mut(x),
            Few::Two(a) => a,
            Few::Many(b) => b,
        }
    }
}

/// What few nodes have: definitions, a paragraph (or the vertical
/// material it carries), errors, a φ, vertical material inside a box,
/// what it wrote to the log.
#[derive(Clone, Debug, Default)]
pub struct Rare {
    pub defs: Vec<Def>,
    pub para: Option<Arc<Para>>,
    pub errors: Vec<String>,
    pub phi: Option<Box<Phi>>,
    pub vemit: Vec<Item>,
    pub log: Option<Box<str>>,
    /// The `Close` of an output routine's group: what the page builder
    /// takes back (`output::OutputEnd`).
    pub output: Option<Box<output::OutputEnd>>,
    /// `\input`: the file read (its pieces follow the node's).
    pub input: Option<Arc<FileText>>,
}

/// A file `\input` read: its name and its text.
#[derive(Debug, PartialEq, Eq)]
pub struct FileText {
    pub name: Arc<str>,
    pub text: Arc<str>,
}

/// A command node. Its input state is its predecessor's `out` (the
/// piece's start for the first node): [`Chain::input`].
#[derive(Clone, Debug)]
pub struct Node {
    pub kind: Kind,
    /// The innermost group open at it (its `Open` node's index), if any.
    group: Option<u32>,
    /// The command, for printing.
    pub cmd: Tok,
    /// The reader state after it (its successor's input).
    pub out: RState,
    pub reads: Few<ReadRef>,
    /// The items it emitted into the horizontal list being built (at
    /// `out.level`); into the vertical list inside a box: `vemit` (at the
    /// outer level they are carried by `para`).
    pub emit: Few<Item>,
    pub rare: Option<Box<Rare>>,
}

impl Node {
    pub fn defs(&self) -> &[Def] {
        self.rare.as_ref().map_or(&[], |r| &r.defs)
    }
    pub fn para(&self) -> Option<&Arc<Para>> {
        self.rare.as_ref().and_then(|r| r.para.as_ref())
    }
    pub fn errors(&self) -> &[String] {
        self.rare.as_ref().map_or(&[], |r| &r.errors)
    }
    pub fn phi(&self) -> Option<&Phi> {
        self.rare.as_ref().and_then(|r| r.phi.as_deref())
    }
    pub fn vemit(&self) -> &[Item] {
        self.rare.as_ref().map_or(&[], |r| &r.vemit)
    }
    /// What it wrote to the log (`\message`; `\immediate\write`, ending
    /// in a newline), the diagnostic value.
    pub fn log(&self) -> Option<&str> {
        self.rare.as_ref().and_then(|r| r.log.as_deref())
    }
    /// The file an `\input` node read.
    pub fn input(&self) -> Option<&Arc<FileText>> {
        self.rare.as_ref().and_then(|r| r.input.as_ref())
    }

    /// What an output routine's end left for the page builder (the `Close`
    /// node of its group).
    pub fn output(&self) -> Option<&output::OutputEnd> {
        self.rare.as_ref().and_then(|r| r.output.as_deref())
    }
}

/// A stretch of a chain's nodes, shared with the chain it was made in.
#[derive(Clone, Debug)]
struct Seg {
    v: Arc<Vec<Arc<Node>>>,
    start: u32,
    end: u32,
}

/// A chain's nodes: stretches shared with the chains that made them (a
/// kept stretch is shared by one pointer, not node by node), then the
/// nodes added since.
#[derive(Clone, Debug, Default)]
pub struct Nodes {
    segs: Vec<Seg>,
    /// Where each segment ends in this chain.
    ends: Vec<u32>,
    tail: Vec<Arc<Node>>,
}

/// Past this many stretches, a chain's nodes are made one again.
const MAX_SEGS: usize = 16;

impl Nodes {
    pub fn len(&self) -> usize {
        self.in_segs() + self.tail.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn in_segs(&self) -> usize {
        self.ends.last().map_or(0, |&e| e as usize)
    }

    pub fn get(&self, i: usize) -> &Arc<Node> {
        let b = self.in_segs();
        if i >= b {
            return &self.tail[i - b];
        }
        let s = self.ends.partition_point(|&e| e as usize <= i);
        let lo = if s == 0 { 0 } else { self.ends[s - 1] as usize };
        let seg = &self.segs[s];
        &seg.v[seg.start as usize + i - lo]
    }

    fn push(&mut self, n: Arc<Node>) {
        self.tail.push(n);
    }

    /// The nodes added since become a stretch others can share.
    fn freeze(&mut self) {
        if self.segs.len() > MAX_SEGS {
            let all: Vec<Arc<Node>> = self.iter().cloned().collect();
            self.segs.clear();
            self.ends.clear();
            self.tail = all;
        }
        if self.tail.is_empty() {
            return;
        }
        let v = Arc::new(std::mem::take(&mut self.tail));
        let end = v.len() as u32;
        self.add_seg(Seg { v, start: 0, end });
    }

    fn add_seg(&mut self, s: Seg) {
        let len = s.end - s.start;
        if len == 0 {
            return;
        }
        if let Some(last) = self.segs.last_mut()
            && Arc::ptr_eq(&last.v, &s.v)
            && last.end == s.start
        {
            last.end = s.end;
            *self.ends.last_mut().expect("a segment") += len;
            return;
        }
        let e = self.in_segs() as u32 + len;
        self.segs.push(s);
        self.ends.push(e);
    }

    /// Appends `other`'s nodes `a..b`, shared (`other` is frozen).
    fn share(&mut self, other: &Nodes, a: usize, b: usize) {
        debug_assert!(other.tail.is_empty(), "a frozen chain");
        if a >= b {
            return;
        }
        self.freeze();
        let mut lo = 0usize;
        for (s, &e) in other.segs.iter().zip(&other.ends) {
            let e = e as usize;
            let (x, y) = (a.max(lo), b.min(e));
            if x < y {
                self.add_seg(Seg {
                    v: s.v.clone(),
                    start: s.start + (x - lo) as u32,
                    end: s.start + (y - lo) as u32,
                });
            }
            lo = e;
            if lo >= b {
                break;
            }
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &Arc<Node>> {
        self.range(0, self.len())
    }

    /// Nodes `a..b`.
    pub fn range(&self, a: usize, b: usize) -> impl Iterator<Item = &Arc<Node>> {
        let mut lo = 0usize;
        let segs = self.segs.iter().zip(&self.ends).filter_map(move |(s, &e)| {
            let e = e as usize;
            let (x, y) = (a.max(lo), b.min(e));
            let r = (x < y).then(|| &s.v[s.start as usize + x - lo..s.start as usize + y - lo]);
            lo = e;
            r
        });
        let (base, n) = (self.in_segs(), self.tail.len());
        let ta = a.clamp(base, base + n) - base;
        let tb = b.clamp(base, base + n) - base;
        segs.flatten().chain(self.tail[ta..tb.max(ta)].iter())
    }
}

impl std::ops::Index<usize> for Nodes {
    type Output = Arc<Node>;
    fn index(&self, i: usize) -> &Arc<Node> {
        self.get(i)
    }
}

/// A piece's chain of nodes, with what resolving names over it needs (each
/// index a cache of the nodes).
#[derive(Clone, Debug, Default)]
pub struct Chain {
    pub nodes: Nodes,
    /// The state the first node reads (none: the canonical start): a file
    /// piece's or a rest-of-line piece's entry edge, or an output
    /// routine's run (`output::output_entry`).
    entry: Option<Arc<RState>>,
    /// Nodes defining each key, in order.
    defs_of: FxMap<Key, Vec<u32>>,
    /// For each `Open` node, the index of its `Close`, once there is one.
    close_of: FxMap<u32, u32>,
    open: Vec<u32>,
    /// Every `Open` and `Close` node, in order.
    scopes: Vec<u32>,
    /// The nodes with a paragraph, and with errors.
    para_at: Vec<u32>,
    err_at: Vec<u32>,
    /// For each `Open` node, the `Open` node of the group enclosing it.
    parent_of: FxMap<u32, u32>,
}

impl Chain {
    /// A chain whose first node reads `entry`.
    pub fn with_entry(entry: RState) -> Chain {
        Chain { entry: Some(Arc::new(entry)), ..Chain::default() }
    }

    /// The state the first node reads.
    fn start(&self) -> RState {
        self.entry.as_deref().map_or_else(RState::start, Clone::clone)
    }

    /// The input state of node `i` (`i == len`: of the next node).
    pub fn input(&self, i: usize) -> RState {
        if i == 0 {
            self.start()
        } else {
            self.nodes[i - 1].out.clone()
        }
    }

    /// The input position of node `i`.
    fn input_pos(&self, i: usize) -> usize {
        if i == 0 { self.entry.as_ref().map_or(0, |e| e.pos) } else { self.nodes[i - 1].out.pos }
    }

    /// Whether node `i` begins a paragraph of the main vertical list.
    fn starts_para(&self, i: usize) -> bool {
        let out = &self.nodes[i].out;
        if i == 0 {
            self.start().starts_para(out)
        } else {
            self.nodes[i - 1].out.starts_para(out)
        }
    }

    /// The definition of `key` reaching node `i` (`i == len`: the next
    /// node): the latest still in scope.
    pub fn resolve(&self, key: &Key, i: usize) -> Option<(u32, Val)> {
        let ds = self.defs_of.get(key)?;
        for &d in ds.iter().rev() {
            if d as usize >= i {
                continue;
            }
            let n = &self.nodes[d as usize];
            let Some(def) = n.defs().iter().rev().find(|x| x.key == *key) else {
                continue;
            };
            let closed = self
                .def_group(n, def)
                .is_some_and(|g| self.close_of.get(&g).is_some_and(|&c| (c as usize) < i));
            if def.global || !closed {
                return Some((d, def.val.clone()));
            }
        }
        None
    }

    /// The group a definition of node `n` is local to (its `Open` node;
    /// `None`: the piece's outer level).
    pub fn def_group(&self, n: &Node, def: &Def) -> Option<u32> {
        let mut g = n.group;
        for _ in 0..def.level.unwrap_or(0) {
            g = g.and_then(|x| self.parent_of.get(&x).copied());
        }
        g
    }

    /// The scope of node `d`'s definition of `key` seen from the next
    /// node: `Some(k)` local to the group `k` levels out from the innermost
    /// one open; `None` global, or made at the piece's outer level.
    pub fn scope_of(&self, d: u32, key: &Key) -> Option<u16> {
        let n = &self.nodes[d as usize];
        let def = n.defs().iter().rev().find(|x| x.key == *key)?;
        if def.global {
            return None;
        }
        let g = self.def_group(n, def)?;
        let p = self.open.iter().rposition(|&o| o == g)?;
        Some((self.open.len() - 1 - p) as u16)
    }

    /// The nodes with a paragraph, in order (`Run::paras` is theirs).
    pub fn para_nodes(&self) -> &[u32] {
        &self.para_at
    }

    /// The groups open at the next node, outermost first (their `Open`
    /// nodes).
    pub fn open_groups(&self) -> &[u32] {
        &self.open
    }

    /// A node about to be added, given the group it is in.
    fn with_group(&self, mut n: Node) -> Arc<Node> {
        n.group = self.open.last().copied();
        if n.kind == Kind::Close {
            // (a close belongs to the group it closes' parent)
            n.group = self.open.len().checked_sub(2).map(|i| self.open[i]);
        }
        Arc::new(n)
    }

    /// Adds a node whose `group` is already right (kept at its old index).
    fn push_arc(&mut self, n: Arc<Node>) {
        let i = self.nodes.len() as u32;
        self.index(&n, i);
        self.nodes.push(n);
    }

    /// Node `n`, at `i`, in the indices.
    fn index(&mut self, n: &Node, i: u32) {
        match n.kind {
            Kind::Open => {
                if let Some(&p) = self.open.last() {
                    self.parent_of.insert(i, p);
                }
                self.open.push(i);
                self.scopes.push(i);
            }
            Kind::Close => {
                if let Some(o) = self.open.pop() {
                    self.close_of.insert(o, i);
                }
                self.scopes.push(i);
            }
            _ => {}
        }
        if let Some(r) = &n.rare {
            for d in &r.defs {
                self.defs_of.entry(d.key.clone()).or_default().push(i);
            }
            if r.para.is_some() {
                self.para_at.push(i);
            }
            if !r.errors.is_empty() {
                self.err_at.push(i);
            }
        }
    }

    /// The first `from` nodes of `old`, shared.
    fn prefix(old: &Chain, from: usize) -> Chain {
        let mut c = Chain { entry: old.entry.clone(), ..Chain::default() };
        c.share(old, 0, from);
        c
    }

    /// Appends `old`'s nodes `a..b`, shared, at the same indices (the chain
    /// has `a` nodes, and how names resolve at `a` is the same in both):
    /// their entries in `old`'s indices are copied, not found again.
    fn share(&mut self, old: &Chain, a: usize, b: usize) {
        debug_assert_eq!(self.nodes.len(), a);
        if a >= b {
            return;
        }
        self.nodes.share(&old.nodes, a, b);
        let within = |v: &[u32]| {
            let s = v.partition_point(|&i| (i as usize) < a);
            let e = v.partition_point(|&i| (i as usize) < b);
            s..e
        };
        for (k, v) in &old.defs_of {
            let r = within(v);
            if !r.is_empty() {
                self.defs_of
                    .entry(k.clone())
                    .or_default()
                    .extend_from_slice(&v[r]);
            }
        }
        for &i in &old.scopes[within(&old.scopes)] {
            if old.nodes[i as usize].kind == Kind::Open {
                if let Some(&p) = self.open.last() {
                    self.parent_of.insert(i, p);
                }
                self.open.push(i);
            } else if let Some(o) = self.open.pop() {
                self.close_of.insert(o, i);
            }
            self.scopes.push(i);
        }
        self.para_at
            .extend_from_slice(&old.para_at[within(&old.para_at)]);
        self.err_at
            .extend_from_slice(&old.err_at[within(&old.err_at)]);
    }
}

/// What running a piece produced.
#[derive(Clone, Debug, Default)]
pub struct Run {
    pub chain: Chain,
    /// Every definition read from before the piece: (key, the first node
    /// reading it, value), one entry per key and value, sorted by key then
    /// node. A key read with two values (a node kept from an earlier run
    /// and one run now) has two entries, and validating them all finds the
    /// stale one.
    pub reads: Vec<(Key, u32, Val)>,
    /// Definitions reaching the piece's end, sorted by key.
    pub exports: Vec<(Key, Val)>,
    pub paras: Vec<Arc<Para>>,
    pub errors: Vec<String>,
    /// Nodes run this time (the others were kept).
    pub ran: u64,
    /// The fonts this piece loads first (`Key::FontLoaded` definitions), in
    /// node order: the document's font numbering is these, piece by piece.
    pub fonts: Vec<Arc<str>>,
    /// The run was abandoned ([`Check::Abandon`]): not a result.
    pub abandoned: bool,
    /// A settled piece's run without its nodes ([`Run::leaned`]): `chain`
    /// is then empty.
    pub lean: Option<Arc<Lean>>,
}

/// What a settled piece keeps of its run once its nodes are dropped (CORE.md,
/// lean pieces): enough for its readers (the run's `reads` and `exports`
/// stay), the page builder (where its paragraphs are, the definitions
/// reaching each node), the log (its part but for its lines), the pieces
/// entered from it and the next run of it (which runs every node again
/// from the text). The dump and the join's page events run it again,
/// reading only.
#[derive(Debug)]
pub struct Lean {
    /// How many nodes it had.
    len: u32,
    para_at: Vec<u32>,
    /// Every definition, by name then node: the name, the node, the first
    /// node it no longer reaches (its group's close + 1; `u32::MAX`: the
    /// end), the value.
    defs: Vec<(Key, u32, u32, Val)>,
    out: RState,
    input: Option<Arc<FileText>>,
    /// Its run ended at a cut the cutter missed ([`Kind::Cut`]).
    cut: bool,
    /// Its reads from before the piece (`Run::reads`) without their values
    /// (its edges are the build's), but for the names the build answers
    /// itself (files, the job, the lines before it), whose values are
    /// compared at the next build.
    reads: Vec<(Key, u32)>,
    given: Vec<(Key, u32, Val)>,
    /// Its part of the log, but for its paragraphs' lines.
    pub plan: Arc<crate::logs::Plan>,
}

impl Run {
    /// The run without its nodes, for a settled piece whose text is
    /// `text` ([`Lean`]).
    #[must_use]
    pub fn leaned(&self, text: &[char]) -> Run {
        let c = &self.chain;
        let mut defs: Vec<(Key, u32, u32, Val)> = Vec::new();
        for (key, ds) in &c.defs_of {
            let mut last = None;
            for &d in ds {
                if last == Some(d) {
                    continue;
                }
                last = Some(d);
                let n = &c.nodes[d as usize];
                let Some(def) = n.defs().iter().rev().find(|x| x.key == *key) else { continue };
                // (`Chain::resolve`: a local definition reaches up to its
                // group's close, that node included)
                let until = if def.global {
                    u32::MAX
                } else {
                    c.def_group(n, def).and_then(|g| c.close_of.get(&g).map(|&x| x + 1)).unwrap_or(u32::MAX)
                };
                defs.push((key.clone(), d, until, def.val.clone()));
            }
        }
        defs.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
        defs.shrink_to_fit();
        let lean = Lean {
            len: c.nodes.len() as u32,
            para_at: c.para_at.clone(),
            defs,
            out: self.out(),
            input: self.input().cloned(),
            cut: self.ends_at_cut(),
            reads: self.reads.iter().map(|(k, i, _)| (k.clone(), *i)).collect(),
            given: self
                .reads
                .iter()
                .filter(|(k, _, _)| matches!(k, Key::File(_) | Key::Job | Key::LineBase))
                .cloned()
                .collect(),
            plan: Arc::new(crate::logs::plan(text, self)),
        };
        Run {
            chain: Chain { entry: c.entry.clone(), ..Chain::default() },
            reads: Vec::new(),
            exports: self.exports.clone(),
            paras: self.paras.clone(),
            errors: self.errors.clone(),
            ran: self.ran,
            fonts: self.fonts.clone(),
            abandoned: false,
            lean: Some(Arc::new(lean)),
        }
    }

    /// How many reads from before the piece it has (`reads`, kept by a
    /// lean run without their values).
    pub fn read_count(&self) -> usize {
        self.lean.as_ref().map_or(self.reads.len(), |l| l.reads.len())
    }

    /// Its `i`-th read from before the piece: the name and its first
    /// reader.
    pub fn read_at(&self, i: usize) -> (&Key, u32) {
        match &self.lean {
            Some(l) => (&l.reads[i].0, l.reads[i].1),
            None => (&self.reads[i].0, self.reads[i].1),
        }
    }

    /// The values it read `key` with (of a lean run: only for the names
    /// the build answers itself).
    pub fn values_read(&self, key: &Key) -> &[(Key, u32, Val)] {
        let v = self.lean.as_ref().map_or(&self.reads[..], |l| &l.given[..]);
        let a = v.partition_point(|x| x.0 < *key);
        let b = a + v[a..].partition_point(|x| x.0 == *key);
        &v[a..b]
    }

    /// How many nodes it has (had, if lean).
    pub fn nodes_len(&self) -> usize {
        self.lean.as_ref().map_or(self.chain.nodes.len(), |l| l.len as usize)
    }

    /// The nodes with a paragraph, in order (`paras` is theirs).
    pub fn para_nodes(&self) -> &[u32] {
        self.lean.as_ref().map_or(self.chain.para_nodes(), |l| &l.para_at)
    }

    /// The definition of `key` its node `i` reads from the piece (`i` the
    /// node count: what reaches the piece's end), and the node making it.
    pub fn resolve(&self, key: &Key, i: usize) -> Option<(u32, Val)> {
        match &self.lean {
            None => self.chain.resolve(key, i),
            Some(l) => {
                let a = l.defs.partition_point(|x| x.0 < *key);
                let b = a + l.defs[a..].partition_point(|x| x.0 == *key);
                l.defs[a..b]
                    .iter()
                    .rev()
                    .find(|(_, d, until, _)| (*d as usize) < i && i < *until as usize)
                    .map(|(_, d, _, v)| (*d, v.clone()))
            }
        }
    }

    /// Whether a node after node `i` may read `key` (from the piece or
    /// from before it): of a lean run, whether the piece reads it from
    /// before it or defines it at all.
    pub fn reads_after(&self, key: &Key, i: u32) -> bool {
        match &self.lean {
            None => (i as usize + 1..self.chain.nodes.len()).any(|j| self.chain.nodes[j].reads.iter().any(|r| r.key == *key)),
            Some(l) => {
                l.reads.binary_search_by(|x| x.0.cmp(key)).is_ok() || l.defs.binary_search_by(|x| x.0.cmp(key)).is_ok()
            }
        }
    }

    /// The state its last node left (the piece's end, or its `\input`).
    pub fn out(&self) -> RState {
        match &self.lean {
            Some(l) => l.out.clone(),
            None => self.chain.input(self.chain.nodes.len()),
        }
    }

    /// Whether its run ended at a cut the cutter missed ([`Kind::Cut`]):
    /// the text after it is cut again.
    pub fn ends_at_cut(&self) -> bool {
        match &self.lean {
            Some(l) => l.cut,
            None => self
                .chain
                .nodes
                .len()
                .checked_sub(1)
                .is_some_and(|i| self.chain.nodes[i].kind == Kind::Cut),
        }
    }

    /// The file its last node, an `\input`, read.
    pub fn input(&self) -> Option<&Arc<FileText>> {
        if let Some(l) = &self.lean {
            return l.input.as_ref();
        }
        let n = self.chain.nodes.len();
        if n == 0 {
            return None;
        }
        let last = &self.chain.nodes[n - 1];
        if last.kind == Kind::Input {
            last.input()
        } else {
            None
        }
    }

    /// The state it was entered in.
    pub fn entry(&self) -> RState {
        self.chain.input(0)
    }
}

/// The node that opened the `k`-th (from the outside) open group
/// (`group`) or conditional at node `i`'s start: where it was entered, for
/// the log's line numbers (e-TeX's `\showgroups`, `\showifs`).
pub fn opener(chain: &Chain, i: usize, group: bool, k: usize) -> Option<usize> {
    let st = chain.input(i);
    let at = if group {
        st.groups.0.as_ref()?.get(k)?.at
    } else {
        st.conds.0.as_ref()?.get(k)?.open
    };
    Some(at as usize)
}

/// The `Open` node of the `k`-th group enclosing node `i` (0: its own
/// innermost), for the log's line numbers.
pub fn enclosing_open(chain: &Chain, i: usize, k: usize) -> Option<usize> {
    let mut g = if i < chain.nodes.len() { chain.nodes[i].group } else { return None };
    for _ in 0..k {
        g = g.and_then(|x| chain.parent_of.get(&x).copied());
    }
    g.map(|x| x as usize)
}

/// The node that began the paragraph at `level` that node `i` is in.
pub fn paragraph_start_at(chain: &Chain, i: usize, level: u16) -> usize {
    material::para_start(chain, i, level)
}

/// The node that began the paragraph node `i` of `chain` ended (or is
/// in): for the log's line numbers.
pub fn paragraph_start(chain: &Chain, i: usize) -> usize {
    material::para_start(chain, i, chain.nodes[i].out.level)
}

/// The node that began the alignment node `i` of `chain` set (its
/// `\halign` or `\valign`): for the log's line numbers.
pub fn alignment_start(chain: &Chain, i: usize) -> usize {
    let level = chain.nodes[i].out.level + 1;
    material::box_start(chain, i.saturating_sub(1), level)
}

/// Cuts the source into pieces at blank lines at brace depth 0. Lines lose
/// their trailing spaces and end in `\r` (TeX's end of line).
pub fn cut(src: &str) -> Vec<Arc<[char]>> {
    cut_from(src, 0, |_| false).0
}

/// [`cut`] from byte `from` of `src` (0, or the start of a piece's first
/// line: the cut there is the same as from the start), giving each piece
/// with the byte offset of its first line. `stop(p)` is asked at each
/// piece start `p` after the first; if it says so, the cut ends before it
/// and `p` is returned too.
pub fn cut_from(
    src: &str,
    from: usize,
    mut stop: impl FnMut(usize) -> bool,
) -> (Vec<Arc<[char]>>, Vec<usize>, Option<usize>) {
    let mut pieces: Vec<Arc<[char]>> = Vec::new();
    let mut starts = vec![from];
    let mut cur: Vec<char> = Vec::new();
    let mut depth: i32 = 0;
    let mut blank_run = false;
    let mut off = from;
    let mut lines = src[from..].split('\n').peekable();
    while let Some(raw) = lines.next() {
        let line_start = off;
        off += raw.len() + 1;
        if raw.is_empty() && lines.peek().is_none() {
            // (the source's final newline)
            break;
        }
        let line = raw.trim_end_matches([' ', '\r']);
        let blank = line.trim_start_matches(' ').is_empty();
        if blank {
            blank_run = true;
        } else {
            if blank_run && depth == 0 && cur.iter().any(|c| !c.is_whitespace()) {
                pieces.push(Arc::from(std::mem::take(&mut cur)));
                if stop(line_start) {
                    return (pieces, starts, Some(line_start));
                }
                starts.push(line_start);
            }
            blank_run = false;
            depth = brace_depth(line, depth);
        }
        cur.extend(line.chars());
        cur.push('\r');
    }
    if cur.is_empty() {
        starts.truncate(pieces.len());
    } else {
        pieces.push(Arc::from(cur));
    }
    (pieces, starts, None)
}

fn brace_depth(line: &str, mut depth: i32) -> i32 {
    let mut it = line.chars();
    while let Some(c) = it.next() {
        match c {
            '\\' => {
                it.next();
            }
            '%' => break,
            '{' => depth += 1,
            '}' => depth = (depth - 1).max(0),
            _ => {}
        }
    }
    depth
}

/// The text edit between a piece's previous text and its new one: the
/// common prefix length, and the end of the changed region in each.
#[derive(Clone, Copy, Debug)]
struct Edit {
    start: usize,
    old_end: usize,
    new_end: usize,
}

impl Edit {
    fn between(old: &[char], new: &[char]) -> Option<Edit> {
        if old == new {
            return None;
        }
        let start = old.iter().zip(new).take_while(|(a, b)| a == b).count();
        let tail = old[start..]
            .iter()
            .rev()
            .zip(new[start..].iter().rev())
            .take_while(|(a, b)| a == b)
            .count();
        Some(Edit {
            start,
            old_end: old.len() - tail,
            new_end: new.len() - tail,
        })
    }

    /// An old position in the new text, if it is outside the changed
    /// region.
    fn map(self, pos: usize) -> Option<usize> {
        if pos < self.start {
            Some(pos)
        } else if pos >= self.old_end {
            Some(pos - self.old_end + self.new_end)
        } else {
            None
        }
    }
}

/// The definitions from before the piece read in this run, each read once
/// for the nodes it reaches (`lo..hi`: all of them unless an element
/// between the piece's nodes defines the name).
type Outside = FxMap<Key, (ReadRef, u32, u32)>;

/// The definition of `k` from before the piece reaching node `node`.
fn outside_read(env: &mut dyn Env, outside: &mut Outside, k: &Key, node: u32) -> ReadRef {
    if let Some((v, lo, hi)) = outside.get(k)
        && (*lo..*hi).contains(&node)
    {
        return v.clone();
    }
    // (what the build answers itself: a file's bytes, the job's name, the
    // lines before the piece)
    let (val, src, lo, hi) = match k {
        Key::File(n) => (env.file(n).map_or(Val::Void, Val::Bytes), Src::Initial, 0, u32::MAX),
        Key::Job => (Val::Name(env.jobname()), Src::Initial, 0, u32::MAX),
        Key::LineBase => (Val::Int(env.line_base()), Src::Initial, 0, u32::MAX),
        _ => env.reach(k, node),
    };
    let r = Arc::new(Read {
        key: k.clone(),
        src,
        val,
        scope: None,
        cond: None,
    });
    outside.insert(k.clone(), (r.clone(), lo, hi));
    r
}

/// The definition of `key` for the next node of `chain`, and where from.
fn resolve_now(chain: &Chain, env: &mut dyn Env, outside: &mut Outside, key: &Key) -> ReadRef {
    let local = chain.resolve(key, chain.nodes.len());
    // (a definition made between the piece's nodes after the local one
    // reaches further: a routine's, where the page builder fired)
    if let Some((d, _)) = local
        && let Some((val, src)) = env.between(key, d, chain.nodes.len() as u32)
    {
        return Arc::new(Read {
            key: key.clone(),
            src,
            val,
            scope: None,
            cond: None,
        });
    }
    match local {
        Some((d, val)) => Arc::new(Read {
            key: key.clone(),
            src: Src::Node(d),
            val,
            scope: chain.scope_of(d, key),
            cond: None,
        }),
        None => outside_read(env, outside, key, chain.nodes.len() as u32),
    }
}

/// `r` read again for the next node of `chain`, whose input state is
/// `state`: as of the node now opening the conditional it read before
/// (`Read::cond`), or as of the next node (`None`: that conditional is
/// not open any more).
fn reread(
    chain: &Chain,
    state: &RState,
    env: &mut dyn Env,
    outside: &mut Outside,
    r: &Read,
) -> Option<ReadRef> {
    let Some(c) = r.cond else {
        return Some(resolve_now(chain, env, outside, &r.key));
    };
    let at = state.conds.get(c as usize)?.open as usize;
    Some(before_read(chain, env, outside, &r.key, at, c))
}

/// The definition of `key` as of node `at`, the opener of the `c`th open
/// conditional (`Read::cond`).
fn before_read(
    chain: &Chain,
    env: &mut dyn Env,
    outside: &mut Outside,
    key: &Key,
    at: usize,
    c: u16,
) -> ReadRef {
    let r = match chain.resolve(key, at) {
        Some((d, val)) => Read {
            key: key.clone(),
            src: Src::Node(d),
            val,
            scope: chain.scope_of(d, key),
            cond: None,
        },
        None => (*outside_read(env, outside, key, at as u32)).clone(),
    };
    Arc::new(Read { cond: Some(c), ..r })
}

/// Runs a piece: `old` is its previous run (and the text that made it),
/// `dirty` the names from before the piece a definition of which was
/// remade: every old node reading one of them is dirty; `entry` the state
/// it is entered in (none: the canonical start). Returns the new chain,
/// with every old node still holding kept.
pub fn run_piece(
    text: &[char],
    env: &mut dyn Env,
    old: Option<(&Run, &[char])>,
    dirty: &[Key],
    entry: Option<&RState>,
) -> Run {
    // (a lean run has no nodes to keep: every node runs again)
    let old = old.filter(|(r, _)| r.lean.is_none());
    let mut outside: Outside = FxMap::default();
    let mut chain = Chain::default();
    let entry: Option<Arc<RState>> = entry
        .filter(|e| **e != RState::start())
        .map(|e| Arc::new(e.clone()));
    // (entered otherwise than the old run was: every node is looked at)
    let entry_changed = old.is_some_and(|(r, _)| r.chain.entry != entry);
    let mut ran = 0u64;
    let edit = old.and_then(|(_, t)| Edit::between(t, text));
    let oldc = old.map(|(r, _)| &r.chain);
    // (the dirty old nodes: those reading a dirty name from before the
    // piece, in order)
    let dirty: Vec<u32> = match oldc {
        Some(oc) if !dirty.is_empty() => oc
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| {
                n.reads.iter().any(|r| {
                    dirty.contains(&r.key) && (!matches!(r.src, Src::Node(_)) || env.has_between(&r.key))
                })
            })
            .map(|(i, _)| i as u32)
            .collect(),
        _ => Vec::new(),
    };
    // (where to start: the first dirty node, or the first the edit touches;
    // everything before it is kept as it is)
    let mut from = dirty.first().map_or(usize::MAX, |&d| d as usize);
    if let (Some(oc), Some(e)) = (oldc, edit) {
        // (positions only grow along a chain)
        let (mut lo, mut hi) = (0, oc.nodes.len());
        while lo < hi {
            let mid = usize::midpoint(lo, hi);
            if oc.nodes[mid].out.pos >= e.start {
                hi = mid;
            } else {
                lo = mid + 1;
            }
        }
        let touched = lo;
        from = from.min(touched);
    }
    let from = if entry_changed {
        0
    } else {
        oldc.map_or(0, |oc| from.min(oc.nodes.len().saturating_sub(1)))
    };
    if let Some(oc) = oldc {
        chain = Chain::prefix(oc, from);
    }
    chain.entry = entry;
    let mut state = chain.input(from);
    let mut scratch = Scratch::default();
    let mut oi = from;
    // (the material the nodes run hold, and what the environment says of
    // the run after each)
    let mut words = 0u64;
    let mut verdict = Check::Go;
    let mut abandoned = false;
    // (a paragraph's node runs again if any node of its paragraph did)
    let mut para_fresh = false;
    // (a node ran or was dropped that changes how later names resolve:
    // later old nodes' reads must be looked at one by one)
    let mut scope_changed = false;
    // (where the old nodes from `from` on are now: runs of old indices
    // `a..b` at new `c..`, in order; an index the open groups and
    // conditionals of a kept node's state hold is moved by it)
    let mut moved: Vec<(u32, u32, u32)> = Vec::new();
    let to_new = |moved: &[(u32, u32, u32)], x: u32| -> Option<u32> {
        if (x as usize) < from {
            return Some(x);
        }
        let k = moved.partition_point(|r| r.1 <= x);
        moved.get(k).filter(|r| r.0 <= x).map(|r| r.2 + (x - r.0))
    };
    loop {
        let mut kept: Option<Node> = None;
        let shared: Option<Arc<Node>> = None;
        // (the old node at this very input, if any: the same command)
        let mut cand: Option<usize> = None;
        let mut copy_to: Option<usize> = None;
        if let Some(oc) = oldc {
            let mapped = |p: usize| edit.map_or(Some(p), |e| e.map(p));
            while oi < oc.nodes.len() && mapped(oc.input_pos(oi)).is_none_or(|p| p < state.pos) {
                let n = &oc.nodes[oi];
                scope_changed |= !n.defs().is_empty() || n.kind.scopes();
                oi += 1;
            }
            if oi < oc.nodes.len() {
                let n = &oc.nodes[oi];
                let inp_pos = oc.input_pos(oi);
                let start_ok = mapped(inp_pos) == Some(state.pos);
                // (the text it read, through the character after it, unchanged)
                let text_ok = edit.is_none_or(|e| n.out.pos < e.start || inp_pos >= e.old_end);
                let par_ok = !(matches!(n.kind, Kind::Par | Kind::End) && para_fresh);
                let state_ok = if oi == 0 {
                    oc.input(0).eq_but_pos(&state)
                } else {
                    oc.nodes[oi - 1].out.eq_but_pos(&state)
                };
                if start_ok && text_ok && state_ok {
                    cand = Some(oi);
                    let reads_ok = par_ok
                        && n.reads.iter().all(|r| {
                            reread(&chain, &state, env, &mut outside, r)
                                .is_some_and(|now| Read::same(&now, r))
                        });
                    if reads_ok {
                        let in_place = edit.is_none() && !scope_changed && oi == chain.nodes.len();
                        if in_place {
                            // (the same node at the same index, its operands
                            // from the same nodes: shared, not copied; and so
                            // on up to the next node that must be looked at)
                            copy_to = Some(
                                dirty
                                    .iter()
                                    .find(|&&d| d as usize > oi)
                                    .map_or(oc.nodes.len(), |&d| d as usize),
                            );
                        } else {
                            let mut n = (**n).clone();
                            if let Some(e) = edit {
                                n.out.pos = e.map(n.out.pos).unwrap_or(n.out.pos);
                            }
                            // (the nodes its state's open groups and
                            // conditionals began at, where they are now: as
                            // the position, they are not content)
                            let at = chain.nodes.len() as u32;
                            let own = oi as u32;
                            if let Some(out) = n.out.moved(|x| {
                                if x == own {
                                    Some(at)
                                } else {
                                    to_new(&moved, x)
                                }
                            }) {
                                n.out = out;
                                // (re-point reads at the nodes now defining them)
                                for r in n.reads.iter_mut() {
                                    *r = reread(&chain, &state, env, &mut outside, r)
                                        .expect("read again above");
                                }
                                kept = Some(n);
                            }
                        }
                    }
                }
            }
        }
        if let Some(stop) = copy_to {
            // (node `oi` was looked at; each node after it up to the next
            // dirty one is shared: the definitions it read in the piece are
            // at the same nodes with the same values (no scope changed), and
            // every node reading a remade definition from before the piece
            // is dirty (a definition remade since, not yet marked, is
            // caught when the run is validated: its reads list every value
            // read); a paragraph's node is looked at if a node of its
            // paragraph ran)
            let oc = oldc.expect("an old chain");
            let mut j = oi;
            while j < stop {
                let n = &oc.nodes[j];
                if j > oi && para_fresh && matches!(n.kind, Kind::Par | Kind::End) {
                    break;
                }
                if oc.starts_para(j) {
                    para_fresh = false;
                }
                j += 1;
            }
            chain.share(oc, oi, j);
            if j > oi {
                moved.push((oi as u32, j as u32, oi as u32));
            }
            if j == oc.nodes.len() {
                break;
            }
            oi = j;
            state = chain.input(j);
            continue;
        }
        let fresh = kept.is_none() && shared.is_none();
        // (the old node this one stands for: kept, or run again at its input)
        let was = if kept.is_some() { Some(oi) } else { cand };
        if let Some(w) = was
            && moved.last().is_none_or(|r| (r.1 as usize) <= w)
        {
            moved.push((w as u32, w as u32 + 1, chain.nodes.len() as u32));
        }
        let n: Arc<Node> = match (shared, kept) {
            (Some(n), _) => {
                oi += 1;
                n
            }
            (None, Some(n)) => {
                // (kept at another index: later nodes' reads in the piece
                // and their groups would name other nodes)
                if oi != chain.nodes.len() && (!n.defs().is_empty() || n.kind.scopes()) {
                    scope_changed = true;
                }
                oi += 1;
                chain.with_group(n)
            }
            (None, None) => {
                ran += 1;
                let mut x = Exec::new(text, &chain, &state, env, &mut outside, &mut scratch);
                // (the old node at this input's paragraph: an equal one is
                // shared)
                x.old_para = cand.and_then(|ci| oldc.and_then(|oc| oc.nodes[ci].para().cloned()));
                x.command();
                let stop = x.stop.take();
                if let Some(Check::Exceeded(size)) = stop {
                    x.overflow(capacity_error(size));
                    x.kind = Kind::End;
                }
                let n = chain.with_group(x.finish());
                words += words_of(&n);
                verdict = match stop {
                    Some(Check::Abandon) => Check::Abandon,
                    Some(_) => Check::Go,
                    None => env.check(ran, words.max(pending_words(&n.out))),
                };
                n
            }
        };
        let mut same_material = !fresh;
        if fresh {
            // (the same command run again, defining the same and leaving the
            // same state, changes nothing about how later names resolve)
            let same_as_old = cand.is_some_and(|ci| {
                let o = &oldc.expect("an old chain").nodes[ci];
                ci == chain.nodes.len()
                    && o.kind == n.kind
                    && o.out == n.out
                    && o.defs().len() == n.defs().len()
                    && o.defs().iter().zip(n.defs()).all(|(a, b)| {
                        a.key == b.key
                            && a.val == b.val
                            && a.global == b.global
                            && a.level == b.level
                    })
            });
            if same_as_old {
                // (and emitting the same, the paragraph's material is the
                // same: a re-run producing equal results stops there)
                let o = &oldc.expect("an old chain").nodes[oi];
                same_material = o.emit[..] == n.emit[..]
                    && o.vemit() == n.vemit()
                    && match (o.para(), n.para()) {
                        (Some(a), Some(b)) => Arc::ptr_eq(a, b) || a == b,
                        (None, None) => true,
                        _ => false,
                    };
                oi += 1;
            } else {
                scope_changed |= !n.defs().is_empty() || n.kind.scopes();
            }
        }
        let starts_para = state.starts_para(&n.out);
        if starts_para {
            para_fresh = !same_material;
        } else if !same_material {
            para_fresh = true;
        }
        let end = matches!(n.kind, Kind::End | Kind::Input | Kind::Cut);
        state = n.out.clone();
        chain.push_arc(n);
        if end {
            break;
        }
        match std::mem::replace(&mut verdict, Check::Go) {
            Check::Go => {}
            Check::Abandon => {
                abandoned = true;
                break;
            }
            Check::Exceeded(size) => {
                let mut x = Exec::new(text, &chain, &state, env, &mut outside, &mut scratch);
                x.overflow(capacity_error(size));
                x.kind = Kind::End;
                let n = chain.with_group(x.finish());
                chain.push_arc(n);
                break;
            }
        }
    }
    // (exports: each key defined in the piece, as it reaches the end)
    chain.nodes.freeze();
    let end = chain.nodes.len();
    let mut exports: Vec<(Key, Val)> = chain
        .defs_of
        .keys()
        .filter_map(|k| chain.resolve(k, end).map(|(_, v)| (k.clone(), v)))
        .collect();
    exports.sort_by(|a, b| a.0.cmp(&b.0));
    let paras = chain
        .para_at
        .iter()
        .filter_map(|&i| chain.nodes[i as usize].para().cloned())
        .collect();
    let errors = chain
        .err_at
        .iter()
        .flat_map(|&i| chain.nodes[i as usize].errors().iter().cloned())
        .collect();
    // (the kept nodes before `from` read what they read in the old run:
    // its entries whose first reader is among them; then each read of the
    // other nodes, its identity seen once)
    let mut reads: Vec<(Key, u32, Val)> = match old {
        Some((r, _)) => r
            .reads
            .iter()
            .filter(|(_, i, _)| (*i as usize) < from)
            .cloned()
            .collect(),
        None => Vec::new(),
    };
    let mut seen: FxSet<usize> = FxSet::default();
    for (d, n) in chain.nodes.range(from, end).enumerate() {
        let i = from + d;
        for r in n.reads.iter() {
            if matches!(r.src, Src::Node(_)) || !seen.insert(Arc::as_ptr(r) as usize) {
                continue;
            }
            reads.push((r.key.clone(), i as u32, r.val.clone()));
        }
    }
    reads.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    // (one entry per (name, value): its first reader — the first of its
    // name's run with that value)
    let mut kept: Vec<(Key, u32, Val)> = Vec::with_capacity(reads.len());
    let mut run_start = 0;
    for e in reads {
        if kept.last().is_none_or(|l| l.0 != e.0) {
            run_start = kept.len();
        } else if kept[run_start..].iter().any(|x| x.2 == e.2) {
            continue;
        }
        kept.push(e);
    }
    let reads = kept;
    let mut fonts: Vec<(u32, Arc<str>)> = chain
        .defs_of
        .iter()
        .filter_map(|(k, v)| match k {
            Key::FontLoaded(f) => v.first().map(|&i| (i, f.clone())),
            _ => None,
        })
        .collect();
    fonts.sort_by_key(|x| x.0);
    let fonts = fonts.into_iter().map(|(_, f)| f).collect();
    Run {
        chain,
        reads,
        exports,
        paras,
        errors,
        ran,
        fonts,
        abandoned,
        lean: None,
    }
}

/// Buffers reused from node to node (their capacity, never their
/// content).
#[derive(Default)]
struct Scratch {
    /// The node's reads so far (read once each, by index), once there are
    /// many.
    seen: FxMap<Key, u32>,
    /// The node's reads of `\catcode` and `\sfcode` of characters below
    /// 256, by index, while their stamp is the node's.
    chars: Vec<(u32, u32)>,
    /// Per control sequence (by its number): the node's read of it, by
    /// index, while its stamp is the node's; and the read the piece keeps
    /// of it ([`Known`]).
    cs: Vec<CsAt>,
    /// The kept reads are those of this epoch (a new one: none kept).
    known_epoch: u32,
    known_len: Option<usize>,
    /// The numbers of the names the piece spelled.
    cs_ids: FxMap<Box<str>, u32>,
    stamp: u32,
    /// The node's reads and what it emitted (their capacity kept).
    reads: Vec<ReadRef>,
    emit: Vec<Item>,
    /// Shared copies of the names read in this piece.
    names: FxSet<Arc<str>>,
    /// The piece's own copy of each font it sets (the font as read, then
    /// the copy, its name the piece's): every character holds its font's
    /// `Arc`, and one `Arc` counted by every thread at once would make
    /// them wait on each other. A copy is equal to the font.
    fonts: Vec<(Arc<Font>, Arc<Font>)>,
    buf: String,
    /// The glue `scan_glue` just gave is an internal quantity's own (for
    /// TeX's shared zero glue, which `\showbox` and the short display
    /// tell apart).
    glue_shared: bool,
    /// The last read of each name made in the piece (from a node of it):
    /// a node reading the same definition, seen the same, shares it (a
    /// read is a value: equal fields, the same read).
    inside: FxMap<Key, ReadRef>,
}

/// A control sequence's entry in [`Scratch::cs`].
#[derive(Clone, Default)]
struct CsAt {
    stamp: u32,
    idx: u32,
    known: Option<Known>,
}

/// A name's read as the piece keeps it: the definition reaching the next
/// node while no node after it defines the name (kept from node to node
/// as the nodes the chain gained since are looked at: `known_len`).
#[derive(Clone)]
struct Known {
    epoch: u32,
    read: ReadRef,
    /// It holds for the nodes before this one (a definition from before
    /// the piece reaching a range of its nodes).
    hi: u32,
    /// The groups opened and closed before it was read (a definition
    /// local to a group is seen by its level from the group open).
    scopes: usize,
}

/// One node's execution: one command, from the state its predecessor left.
struct Exec<'a> {
    text: &'a [char],
    chain: &'a Chain,
    env: &'a mut dyn Env,
    outside: &'a mut Outside,
    scratch: &'a mut Scratch,
    st: RState,
    reads: Vec<ReadRef>,
    defs: Vec<Def>,
    emit: Vec<Item>,
    vemit: Vec<Item>,
    para: Option<Arc<Para>>,
    errors: Vec<String>,
    phi: Option<Box<Phi>>,
    log: Option<Box<str>>,
    kind: Kind,
    cmd: Tok,
    /// The token `next_raw` just gave came after `\noexpand`'s mark.
    noexp: bool,
    /// Expanding a `\write`'s text (no mode: tex.web 1370).
    in_write: bool,
    /// How many of `reads` are in `scratch.seen`.
    seen_upto: usize,
    /// `\input`: the file read (the node ends the piece's run).
    input: Option<Arc<FileText>>,
    /// The command is being looked for (an `\input` expanded now is a
    /// node of its own).
    at_start: bool,
    /// This node's stamp in `scratch.chars`.
    stamp: u32,
    /// The paragraph the old node at this place made, if any: an equal
    /// paragraph is that one (the same `Arc`).
    old_para: Option<Arc<Para>>,
    /// While a rewritten text (`\scantokens`) is read: its characters,
    /// which the tokenizer reads instead of the piece's.
    ptext: Option<Arc<[char]>>,
    /// The output routine's end this node is (`output::OutputEnd`).
    output: Option<Box<output::OutputEnd>>,
    /// Inside `\csname`…`\endcsname` (pdfTeX's `\ifincsname`).
    in_csname: u16,
    /// Input rewrites (macro calls and the like) in this node, and what
    /// the environment said of the run while it expanded on and on: the
    /// node ends at once ([`Exec::rewrite`]).
    steps: u64,
    stop: Option<Check>,
    /// This node made the hundredth error: the build ends here (§82).
    halt: bool,
}

/// Up to this many reads, a node's reads are looked up by a scan.
const FEW_READS: usize = 12;

/// Whether `t` is `\par` (by name: tex.web 392's `par_token`).
fn is_par(t: &Tok) -> bool {
    matches!(t, Tok::Cs(crate::build::CS_PAR))
}

/// The nodes a run makes before it keeps the names it read
/// ([`Scratch::known`]).
const KNOWN_AFTER: usize = 256;

/// [`Known`] for the next node of `chain`: the names the nodes added since
/// defined are read again; all of them, if the chain is not the one they
/// were kept for, or if anything between the piece's nodes may define a
/// name.
fn known_since(scratch: &mut Scratch, chain: &Chain, env: &mut dyn Env) {
    let len = chain.nodes.len();
    // (a short run reads few names twice: none kept)
    if len < KNOWN_AFTER && scratch.known_len.is_none() {
        return;
    }
    let between = env.any_between();
    let keep = match scratch.known_len {
        Some(prev) if prev <= len && !between => {
            for i in prev..len {
                for d in chain.nodes[i].defs() {
                    if let Key::Cs(n) = d.key
                        && let Some(e) = scratch.cs.get_mut(n as usize)
                    {
                        e.known = None;
                    }
                }
            }
            true
        }
        _ => false,
    };
    if !keep {
        scratch.known_epoch = scratch.known_epoch.wrapping_add(1);
    }
    scratch.known_len = (len >= KNOWN_AFTER && !between).then_some(len);
}

/// Where a node found a definition: its own `defs[i]`, or `reads[i]`.
enum Found {
    Own(usize),
    Read(usize),
}

impl<'a> Exec<'a> {
    fn new(
        text: &'a [char],
        chain: &'a Chain,
        input: &RState,
        env: &'a mut dyn Env,
        outside: &'a mut Outside,
        scratch: &'a mut Scratch,
    ) -> Exec<'a> {
        if !scratch.seen.is_empty() {
            scratch.seen.clear();
        }
        known_since(scratch, chain, env);
        if scratch.chars.is_empty() {
            scratch.chars = vec![(0, 0); 512];
        }
        scratch.stamp = scratch.stamp.wrapping_add(1);
        if scratch.stamp == 0 {
            scratch.chars.fill((0, 0));
            for e in &mut scratch.cs {
                e.stamp = 0;
            }
            scratch.stamp = 1;
        }
        let stamp = scratch.stamp;
        let reads = std::mem::take(&mut scratch.reads);
        let emit = std::mem::take(&mut scratch.emit);
        Exec {
            text,
            chain,
            env,
            outside,
            scratch,
            st: input.clone(),
            reads,
            defs: Vec::new(),
            emit,
            vemit: Vec::new(),
            para: None,
            errors: Vec::new(),
            phi: None,
            log: None,
            kind: Kind::Other,
            cmd: Tok::Char(0, '\0'),
            noexp: false,
            in_write: false,
            seen_upto: 0,
            input: None,
            at_start: false,
            stamp,
            old_para: None,
            ptext: None,
            output: None,
            in_csname: 0,
            steps: 0,
            stop: None,
            halt: false,
        }
    }

    fn finish(mut self) -> Node {
        // (vertical material at the outer level is carried by `para`, in
        // the order the build's page fold reads it)
        if self.st.level == 0 && !self.vemit.is_empty() {
            let v = std::mem::take(&mut self.vemit);
            match &mut self.para {
                Some(p) => Arc::make_mut(p).after.extend(v),
                None => {
                    self.para = Some(Arc::new(Para {
                        before: v,
                        ..Para::default()
                    }));
                }
            }
        }
        if self.halt {
            // (§82: at any level, into the page fold's stream)
            let p = self.para.get_or_insert_with(|| Arc::new(Para::default()));
            Arc::make_mut(p).after.push(Item::Halt);
        }
        let reads = Few::take(&mut self.reads);
        let emit = Few::take(&mut self.emit);
        self.scratch.reads = self.reads;
        self.scratch.emit = self.emit;
        let rare = (!self.defs.is_empty()
            || self.para.is_some()
            || !self.errors.is_empty()
            || self.phi.is_some()
            || !self.vemit.is_empty()
            || self.log.is_some()
            || self.input.is_some()
            || self.output.is_some())
        .then(|| {
            Box::new(Rare {
                defs: self.defs,
                para: self.para,
                errors: self.errors,
                phi: self.phi,
                vemit: self.vemit,
                log: self.log,
                input: self.input,
                output: self.output,
            })
        });
        Node {
            kind: self.kind,
            group: None,
            cmd: self.cmd,
            out: self.st,
            reads,
            emit,
            rare,
        }
    }

    fn err(&mut self, s: String) {
        if self.halted() {
            return;
        }
        self.errors.push(s);
        if self.count_error() {
            let mut l = String::from(RAW);
            l.push(NL);
            l.push_str(THAT_MAKES_100);
            l.push(HALT);
            self.log_raw(&l);
        }
    }

    /// tex.web 94, `overflow`: "TeX capacity exceeded", a fatal error: as
    /// at the hundredth error nothing more is read and this node's
    /// `Item::Halt` ends the page fold.
    fn overflow(&mut self, s: String) {
        if self.halted() {
            return;
        }
        self.errors.push(s);
        self.st.error_count = FATAL_ERRORS;
        self.halt = true;
        let mut l = String::from(RAW);
        l.push(HALT);
        self.log_raw(&l);
    }

    /// The reader has stopped at its hundredth error (tex.web 82).
    fn halted(&self) -> bool {
        self.st.error_count >= FATAL_ERRORS
    }

    /// tex.web 82: `incr(error_count)`; the hundredth is fatal: whether
    /// this one was. Nothing more is read (the reader's state says so:
    /// `next_raw` gives nothing, the next node ends the piece), and this
    /// node's `Item::Halt` ends the page fold, wherever it is (`jump_out`:
    /// the page so far is not shipped); its log ends with TeX's line.
    fn count_error(&mut self) -> bool {
        self.st.error_count += 1;
        self.halt = self.st.error_count >= FATAL_ERRORS;
        self.halt
    }

    // ------------------------------------------------------------ names

    /// The definition of `key` reaching this node: this node's own, else a
    /// node before it in the piece, else from before the piece.
    fn get(&mut self, key: &Key) -> Val {
        match self.find(key) {
            Found::Own(i) => self.defs[i].val.clone(),
            Found::Read(i) => self.reads[i].val.clone(),
        }
    }

    /// [`Exec::get`], as the read (the value not copied out of it).
    fn lookup(&mut self, key: &Key) -> ReadRef {
        match self.find(key) {
            // (its own definition: not a read)
            Found::Own(i) => Arc::new(Read {
                key: key.clone(),
                src: Src::Node(self.chain.nodes.len() as u32),
                val: self.defs[i].val.clone(),
                scope: None,
                cond: None,
            }),
            Found::Read(i) => self.reads[i].clone(),
        }
    }

    /// Where the definition of `key` reaching this node is: its own, or
    /// one of its reads (read now if it was not).
    fn find(&mut self, key: &Key) -> Found {
        if let Key::Cs(n) = *key {
            return self.find_name(n, key);
        }
        // (a character's code, read again in the node: by the character)
        let slot = match key {
            Key::Cat(c) if (*c as u32) < 256 && self.defs.is_empty() => Some(*c as usize),
            Key::Sfcode(c) if (*c as u32) < 256 && self.defs.is_empty() => Some(256 + *c as usize),
            _ => None,
        };
        if let Some(s) = slot {
            let (stamp, i) = self.scratch.chars[s];
            if stamp == self.stamp {
                return Found::Read(i as usize);
            }
        }
        let f = self.find_slow(key);
        if let (Some(s), Found::Read(i)) = (slot, &f) {
            self.scratch.chars[s] = (self.stamp, *i as u32);
        }
        f
    }

    /// [`Exec::find`] of control sequence `n`: its own definition, its
    /// read in the node (by its number, [`Scratch::cs`]), else read now.
    fn find_name(&mut self, n: u32, key: &Key) -> Found {
        if !self.defs.is_empty()
            && let Some(i) = self.defs.iter().rposition(|d| d.key == *key)
        {
            return Found::Own(i);
        }
        if let Some(i) = self.name_read(n) {
            return Found::Read(i);
        }
        let r = self.resolve_shared(key);
        Found::Read(self.push_read(r))
    }

    /// The node's read of control sequence `n`, if it read it.
    fn name_read(&self, n: u32) -> Option<usize> {
        let e = self.scratch.cs.get(n as usize)?;
        (e.stamp == self.stamp).then_some(e.idx as usize)
    }

    /// Control sequence `n`'s entry in [`Scratch::cs`].
    fn cs_at(&mut self, n: u32) -> &mut CsAt {
        let i = n as usize;
        if i >= self.scratch.cs.len() {
            self.scratch.cs.resize(i + 1, CsAt::default());
        }
        &mut self.scratch.cs[i]
    }

    /// A read of the node, its index (the first of a name is the node's
    /// read of it, [`Exec::name_read`]).
    fn push_read(&mut self, r: ReadRef) -> usize {
        let i = self.reads.len();
        if let Key::Cs(n) = r.key {
            let stamp = self.stamp;
            let e = self.cs_at(n);
            if e.stamp != stamp {
                e.stamp = stamp;
                e.idx = i as u32;
            }
        }
        self.reads.push(r);
        i
    }

    /// The meaning of control sequence `n` (as [`Exec::meaning`] of
    /// `Key::Cs(n)`).
    fn name_meaning(&mut self, n: u32) -> Meaning {
        if self.defs.is_empty()
            && let Some(i) = self.name_read(n)
        {
            return match &self.reads[i].val {
                Val::Meaning(m) => m.clone(),
                _ => Meaning::Undefined,
            };
        }
        self.meaning(&Key::Cs(n))
    }

    /// `x` as text (control sequences by their names).
    fn shown<'s, T>(&'s self, x: &'s T) -> crate::val::Shown<'s, T> {
        crate::val::Shown(x, self.env.cs())
    }

    /// The number of control sequence `name` (the piece's own table
    /// first).
    fn cs_of(&mut self, name: &str) -> u32 {
        if let Some(&n) = self.scratch.cs_ids.get(name) {
            return n;
        }
        let n = self.env.cs().id(name);
        self.scratch.cs_ids.insert(name.into(), n);
        n
    }

    fn find_slow(&mut self, key: &Key) -> Found {
        if let Some(i) = self.defs.iter().rposition(|d| d.key == *key) {
            return Found::Own(i);
        }
        if self.reads.len() <= FEW_READS {
            if let Some(i) = self.reads.iter().position(|r| r.key == *key) {
                return Found::Read(i);
            }
        } else {
            for (j, r) in self.reads[self.seen_upto..].iter().enumerate() {
                self.scratch
                    .seen
                    .entry(r.key.clone())
                    .or_insert((self.seen_upto + j) as u32);
            }
            self.seen_upto = self.reads.len();
            if let Some(&i) = self.scratch.seen.get(key) {
                return Found::Read(i as usize);
            }
        }
        let r = self.resolve_shared(key);
        Found::Read(self.push_read(r))
    }

    /// [`resolve_now`], the read shared with an earlier node's equal one
    /// ([`Scratch::inside`], [`Known`]).
    fn resolve_shared(&mut self, key: &Key) -> ReadRef {
        let chain = self.chain;
        let n = match *key {
            Key::Cs(n) if self.scratch.known_len.is_some() => Some(n),
            _ => None,
        };
        let epoch = self.scratch.known_epoch;
        if let Some(n) = n
            && let Some(k) = self.scratch.cs.get(n as usize).and_then(|e| e.known.as_ref())
            && k.epoch == epoch
            && (chain.nodes.len() as u32) < k.hi
            && (k.read.scope.is_none() || k.scopes == chain.scopes.len())
        {
            return k.read.clone();
        }
        let r = self.resolve_shared_slow(key);
        if let Some(n) = n
            && r.cond.is_none()
        {
            let hi = match r.src {
                Src::Node(_) => u32::MAX,
                _ => self.outside.get(key).map_or(0, |(_, _, hi)| *hi),
            };
            let scopes = chain.scopes.len();
            self.cs_at(n).known = Some(Known { epoch, read: r.clone(), hi, scopes });
        }
        r
    }

    fn resolve_shared_slow(&mut self, key: &Key) -> ReadRef {
        let chain = self.chain;
        let Some((d, val)) = chain.resolve(key, chain.nodes.len()) else {
            return outside_read(self.env, self.outside, key, chain.nodes.len() as u32);
        };
        if self.env.has_between(key) {
            // (a routine's definition may reach further: `resolve_now`'s)
            return resolve_now(chain, self.env, self.outside, key);
        }
        let scope = chain.scope_of(d, key);
        if let Some(r) = self.scratch.inside.get(key)
            && r.src == Src::Node(d)
            && r.scope == scope
            && r.cond.is_none()
            && r.val == val
        {
            return r.clone();
        }
        let r = Arc::new(Read { key: key.clone(), src: Src::Node(d), val, scope, cond: None });
        self.scratch.inside.insert(key.clone(), r.clone());
        r
    }

    fn define(&mut self, key: Key, val: Val, global: bool) {
        self.defs.push(Def {
            key,
            val,
            global,
            level: None,
        });
    }

    fn meaning(&mut self, key: &Key) -> Meaning {
        match self.get(key) {
            Val::Meaning(m) => m,
            _ => Meaning::Undefined,
        }
    }

    fn catcode(&mut self, c: char) -> u8 {
        match self.get(&Key::Cat(c)) {
            Val::Cat(k) => k,
            _ => 12,
        }
    }

    fn int(&mut self, key: &Key) -> i32 {
        match self.get(key) {
            Val::Int(i) => i,
            Val::Glue(g) => g.width,
            _ => 0,
        }
    }

    fn int_par(&mut self, p: IntPar) -> i32 {
        self.int(&Key::IntPar(p))
    }

    fn dimen_par(&mut self, p: DimenPar) -> i32 {
        self.int(&Key::DimenPar(p))
    }

    /// The current font (through its read: the read is shared, the font's
    /// `Arc` is copied once).
    fn font(&mut self) -> Arc<Font> {
        let r = self.lookup(&Key::Font);
        match &r.val {
            Val::Font(f) => self.own_font(f),
            other => panic!("the font key holds {other:?}"),
        }
    }

    /// The piece's own copy of font `f` (`Scratch::fonts`).
    fn own_font(&mut self, f: &Arc<Font>) -> Arc<Font> {
        if let Some((_, o)) = self.scratch.fonts.iter().find(|(s, o)| Arc::ptr_eq(s, f) || Arc::ptr_eq(o, f)) {
            return o.clone();
        }
        let mut c = (**f).clone();
        c.key = intern(&mut self.scratch.names, &f.key);
        let o = Arc::new(c);
        self.scratch.fonts.push((f.clone(), o.clone()));
        o
    }

    /// Font `f`'s identity as the piece's own name (for the keys of its
    /// parameters).
    fn font_key(&mut self, f: &Font) -> Arc<str> {
        intern(&mut self.scratch.names, &f.key)
    }

    /// `\fontdimen n` of the current font.
    fn font_param(&mut self, n: u32) -> i32 {
        let f = self.font();
        let k = self.font_key(&f);
        self.int(&Key::FontDimen(k, n))
    }

    fn tok_meaning(&mut self, t: &Tok) -> Meaning {
        match t {
            Tok::Char(c, x) => Meaning::Char(*c, *x),
            Tok::Cs(n) => self.name_meaning(*n),
            _ => t.key().map_or(Meaning::Undefined, |k| self.meaning(&k)),
        }
    }

    fn cs(&mut self, name: &str) -> Tok {
        Tok::Cs(self.cs_of(name))
    }

    /// Sets the node's kind from a conditional's step, unless its command
    /// already gave it a kind that matters more (a group, a paragraph).
    fn cond_kind(&mut self, k: Kind) {
        if matches!(
            self.kind,
            Kind::Other | Kind::If | Kind::IfClosed | Kind::EndIf
        ) {
            self.kind = k;
        }
    }

    // ------------------------------------------------------------ the input

    fn back(&mut self, t: Tok) {
        if self.st.al.on {
            // (tex.web 325: a brace read again is counted again)
            align::uncount(&mut self.st.al, &t);
        }
        // (tex.web 325: lists read to their end go first)
        self.end_read_lists();
        self.st.pending.push((Arc::from([t]), 0, true));
    }

    /// The token lists read to their end go (not those under a rewritten
    /// text): before a list is backed up (tex.web 325) and before a
    /// macro's body is read (390: a macro calling itself last, as plain's
    /// `\loop` does, keeps the input as long as it was).
    fn end_read_lists(&mut self) {
        let floor = self.st.texts.last().map_or(0, |x| x.at);
        while self.st.pending.len() > floor && self.st.pending.top_done() == Some(true) {
            self.st.pending.pop();
        }
    }

    /// Rewrites the input: `toks`, then the rest. A node expanding on and
    /// on (a macro calling itself) asks the environment now and then
    /// whether the run goes on ([`Env::check`]); if not, its input ends
    /// here.
    fn rewrite(&mut self, toks: Vec<Tok>) {
        if !toks.is_empty() {
            self.st.pending.push((Arc::from(toks), 0, false));
        }
        self.step();
    }

    /// A step of the input (a rewrite, a macro's body read): now and then
    /// the environment says whether the run goes on ([`Env::check`]); if
    /// not, the input ends here.
    fn step(&mut self) {
        self.steps += 1;
        if self.steps.is_multiple_of(1 << 12) && self.stop.is_none() {
            let words = pending_words(&self.st) + (self.emit.len() + self.vemit.len()) as u64;
            match self.env.check(self.steps, words) {
                Check::Go => {}
                c => {
                    self.stop = Some(c);
                    self.st.pending.clear();
                    self.st.texts = Stack::new();
                    self.ptext = None;
                    self.st.pos = self.text.len();
                }
            }
        }
    }

    /// The current line's end-of-line character (tex.web 360: the value of
    /// `\endlinechar` when the line was begun), if active.
    fn eol(&mut self) -> Option<char> {
        if self.st.eol == EOL_UNREAD {
            self.read_eol();
        }
        u8::try_from(self.st.eol).ok().map(char::from)
    }

    /// Reads `\endlinechar` for the line being begun.
    fn read_eol(&mut self) {
        let v = self.int_par(IntPar::Endlinechar);
        self.st.eol = if (0..=255).contains(&v) { v as i16 } else { -1 };
    }

    /// The reader has passed a line's end: the next line begins (its
    /// end-of-line character is read when the line is: as its first
    /// character is).
    fn new_line(&mut self) {
        self.st.tstate = TState::N;
        self.st.eol = EOL_UNREAD;
    }

    /// The characters the tokenizer reads: the piece's, or a rewritten
    /// text's while one is read.
    fn txt(&self) -> &[char] {
        match &self.ptext {
            Some(t) => t,
            None => self.text,
        }
    }

    /// Moves to `np`; past a line's end, the next line begins (and a text
    /// whose `\endinput` was met ends, tex.web 362).
    fn advance(&mut self, np: usize) {
        let crossed = self.txt()[self.st.pos..np].contains(&'\r');
        self.st.pos = np;
        if crossed {
            self.new_line();
            if self.ptext.is_some() && self.st.texts.last().is_some_and(|x| x.endinput) {
                self.st.pos = self.txt().len();
            } else if self.ptext.is_none() && self.st.endinput {
                // (`\endinput` in a file piece: the file ends with the
                // line, tex.web 362)
                self.st.pos = self.text.len();
            }
        }
    }

    /// The character at `pos`, the position after it and its category
    /// (tex.web 355): a line's end is its end-of-line character (none if
    /// inactive), and `^^` reduces the character or the two hex digits
    /// after it.
    fn char_at(&mut self, pos: usize) -> Option<(char, usize, u8)> {
        let raw = *self.txt().get(pos)?;
        let c = if raw == '\r' { self.eol()? } else { raw };
        let cat = self.catcode(c);
        // (`^^`: the second circumflex on the line, and a character after it)
        if raw == '\r' || cat != 7 || self.txt().get(pos + 1) != Some(&c) {
            return Some((c, pos + 1, cat));
        }
        let at = |x: &mut Self, p: usize| -> Option<char> {
            let ch = x.txt().get(p).copied();
            match ch {
                Some('\r') => x.eol(),
                other => other,
            }
        };
        let Some(c2) = at(self, pos + 2) else {
            return Some((c, pos + 1, cat));
        };
        if u32::from(c2) >= 128 {
            return Some((c, pos + 1, cat));
        }
        let hex = |ch: char| ch.is_ascii_digit() || ('a'..='f').contains(&ch);
        let (r, np) = match at(self, pos + 3) {
            Some(c3) if hex(c2) && hex(c3) && self.txt().get(pos + 2) != Some(&'\r') => (
                (c2.to_digit(16).unwrap_or(0) << 4) | c3.to_digit(16).unwrap_or(0),
                pos + 4,
            ),
            _ => {
                let v = u32::from(c2);
                (if v < 64 { v + 64 } else { v - 64 }, pos + 3)
            }
        };
        let r = char::from_u32(r).unwrap_or('\0');
        Some((r, np, self.catcode(r)))
    }

    /// The position just after the current line's end.
    fn line_end_after(&self, pos: usize) -> usize {
        let text = self.txt();
        text[pos..]
            .iter()
            .position(|&c| c == '\r')
            .map_or(text.len(), |k| pos + k + 1)
    }

    /// The next token, unexpanded (tex.web 343-357). After `\noexpand`'s
    /// mark, the token after it, with `noexp` set. In an alignment, the
    /// braces are counted and an entry's end inserts its v template
    /// (tex.web 342: `align_state`).
    fn next_raw(&mut self) -> Option<Tok> {
        let t = self.next_raw0()?;
        if self.st.al.on { self.align_token(t) } else { Some(t) }
    }

    /// [`Exec::next_raw`] before an alignment sees the token.
    fn next_raw0(&mut self) -> Option<Tok> {
        self.noexp = false;
        if self.halted() {
            return None;
        }
        loop {
            // (the token lists above the innermost rewritten text; a list
            // read to its end stays until the next read, tex.web 325)
            let floor = self.st.texts.last().map_or(0, |x| x.at);
            while self.st.pending.len() > floor {
                if let Some(t) = self.st.pending.next_tok() {
                    if t == Tok::DontExpand {
                        // (the marked token, as the wrapper will see it)
                        let n = self.next_raw0();
                        self.noexp = n.as_ref().is_some_and(|x| x.key().is_some());
                        return n;
                    }
                    return Some(t);
                }
                self.st.pending.pop();
            }
            if self.st.texts.is_empty() {
                return self.tokenize();
            }
            // (a `\read` line's end ends the reading)
            if self.st.texts.last().is_some_and(|x| x.kind == TextKind::Read) {
                return self.text_token();
            }
            if let Some(t) = self.text_token() {
                return Some(t);
            }
        }
    }

    /// The next token of the innermost rewritten text, read as the piece's
    /// text is; at its end `\everyeof` goes in once (etex.web §362), then
    /// the text is done (`None`: read on below it).
    fn text_token(&mut self) -> Option<Tok> {
        let x = self.st.texts.last()?.clone();
        let outer = (self.st.pos, self.st.tstate, self.st.eol);
        (self.st.pos, self.st.tstate, self.st.eol) = (x.pos, x.tstate, x.eol);
        let prev = self.ptext.replace(x.text.clone());
        let t = self.tokenize();
        self.ptext = prev;
        let here = (self.st.pos, self.st.tstate, self.st.eol);
        (self.st.pos, self.st.tstate, self.st.eol) = outer;
        let Some(x) = self.st.texts.last_mut() else { return t };
        (x.pos, x.tstate, x.eol) = here;
        if t.is_some() || x.kind == TextKind::Read {
            return t;
        }
        // (`\endinput` forces the end: no `\everyeof`)
        if !x.eof && !x.endinput {
            x.eof = true;
            let every = match self.get(&Key::TokPar(crate::val::TokPar::Everyeof)) {
                Val::Toks(l) => l,
                _ => Arc::new(Vec::new()),
            };
            if !every.is_empty() {
                self.rewrite(every.to_vec());
                return None;
            }
        }
        if let Some(PendingText { kind: TextKind::File(_), .. }) = self.st.texts.pop() {
            // (an `\input` file is closed, tex.web 362)
            self.log_raw(&format!("{}{}", crate::show::RAW, ')'));
        }
        None
    }

    /// The next token of the text being read (the piece's, or a rewritten
    /// one's: `txt`), unexpanded (tex.web 343-357).
    fn tokenize(&mut self) -> Option<Tok> {
        loop {
            let pos = self.st.pos;
            let raw = *self.txt().get(pos)?;
            if self.st.eol == EOL_UNREAD {
                self.read_eol();
            }
            if raw == '\r' && self.eol().is_none() {
                // (an inactive end-of-line character: the line just ends)
                self.advance(pos + 1);
                continue;
            }
            let (c, np, cat) = self.char_at(pos)?;
            match cat {
                0 => {
                    // (a control sequence: a run of letters, or one character)
                    let line_end = self.line_end_after(pos);
                    let first = if np < line_end {
                        self.char_at(np)
                    } else {
                        None
                    };
                    let Some((c2, mut q, cat2)) = first else {
                        self.advance(np);
                        self.st.tstate = TState::S;
                        return Some(self.cs(""));
                    };
                    if cat2 == 11 {
                        self.scratch.buf.clear();
                        self.scratch.buf.push(c2);
                        while q < line_end {
                            let Some((c3, q3, cat3)) = self.char_at(q) else {
                                break;
                            };
                            if cat3 != 11 {
                                break;
                            }
                            self.scratch.buf.push(c3);
                            q = q3;
                        }
                        self.st.tstate = TState::S;
                        let buf = std::mem::take(&mut self.scratch.buf);
                        let t = Tok::Cs(self.cs_of(&buf));
                        self.scratch.buf = buf;
                        self.advance(q);
                        return Some(t);
                    }
                    self.st.tstate = if cat2 == 10 { TState::S } else { TState::M };
                    let mut b = [0u8; 4];
                    let t = self.cs(c2.encode_utf8(&mut b));
                    self.advance(q);
                    return Some(t);
                }
                5 => {
                    // (the rest of the line is discarded)
                    let ts = self.st.tstate;
                    let end = self.line_end_after(pos);
                    self.advance(end.max(np));
                    match ts {
                        TState::N => return Some(self.cs("par")),
                        TState::M => return Some(Tok::Char(10, ' ')),
                        TState::S => {}
                    }
                }
                10 => {
                    let ts = self.st.tstate;
                    self.st.tstate = TState::S;
                    self.advance(np);
                    if ts == TState::M {
                        return Some(Tok::Char(10, ' '));
                    }
                }
                14 => {
                    let end = self.line_end_after(pos);
                    self.advance(end.max(np));
                }
                9 => self.advance(np),
                15 => {
                    self.advance(np);
                    self.err("Text line contains an invalid character".into());
                }
                13 => {
                    self.st.tstate = TState::M;
                    self.advance(np);
                    return Some(Tok::Active(c));
                }
                _ => {
                    self.st.tstate = TState::M;
                    self.advance(np);
                    return Some(Tok::Char(cat, c));
                }
            }
        }
    }

    // ------------------------------------------------------------ macros

    /// A macro call: matches the arguments and rewrites the input with the
    /// body (tex.web 389-399). A step: it defines nothing.
    fn call(&mut self, name: &Tok, m: &Macro) {
        let mut args: [Option<Arg>; 10] = Default::default();
        let mut k = 0;
        let p = &m.params;
        let mut i = 0;
        while i < p.len() {
            if let Tok::Param(n) = p[i] {
                let mut j = i + 1;
                while j < p.len() && !matches!(p[j], Tok::Param(_)) {
                    j += 1;
                }
                let delim = &p[i + 1..j];
                // (from the list the reader is in, in one go, when nothing
                // in it is read otherwise; else token by token)
                let arg = if delim.is_empty() {
                    match self.bulk_undelimited(m.long) {
                        Some(a) => Some(a),
                        None => {  self.undelimited(name, m.long).map(|v| Arg::own(v, &m.body)) }
                    }
                } else {
                    match self.bulk_delimited(delim, m.long) {
                        Some(a) => Some(a),
                        None => {  self.delimited(name, delim, m.long).map(|v| Arg::own(v, &m.body)) }
                    }
                };
                let Some(arg) = arg else { return };
                k = usize::from(n);
                args[k] = Some(arg);
                i = j;
            } else {
                match self.next_raw() {
                    Some(x) if x == p[i] => {}
                    other => {
                        self.err(format!("Use of {} doesn't match its definition", self.shown(name)));
                        if let Some(x) = other {
                            self.back(x);
                        }
                        return;
                    }
                }
                i += 1;
            }
        }
        // (the body is read where it is, the macro's; its parameters are
        // the arguments, tex.web 390, 357)
        self.end_read_lists();
        if !m.body.is_empty() {
            if k == 0 {
                self.st.pending.push((m.body.clone(), 0, false));
            } else {
                let args: Args = args
                    .into_iter()
                    .skip(1)
                    .take(k)
                    .map(|a| a.unwrap_or_else(|| Arg { list: m.body.clone(), start: 0, end: 0 }))
                    .collect();
                self.st.pending.push_body(m.body.clone(), args);
            }
        }
        self.step();
    }

    /// The top input list, where the reader reads next, if it reads it as
    /// it is: no other text or alignment in the way (its parameters are
    /// its arguments, [`Cursor`]).
    fn plain_top(&mut self) -> Option<&Level> {
        let floor = self.st.texts.last().map_or(0, |x| x.at);
        if self.halted() || self.st.al.on {
            return None;
        }
        // (the lists read to their end go, as the reader's next read
        // takes them away)
        while self.st.pending.len() > floor && self.st.pending.top_done() == Some(true) {
            self.st.pending.pop();
        }
        if self.st.pending.len() <= floor {
            return None;
        }
        self.st.pending.top.as_ref()
    }

    /// The reader reads on from `to` in the top list: where it is, and
    /// the argument it is in, read to `to` (the argument's list stays
    /// until the next read even when read to its end, as the reader's).
    fn read_to(&mut self, to: Cursor) {
        let Some(t) = self.st.pending.top.as_mut() else { return };
        t.at = to.k;
        t.sub = to.arg;
    }

    /// An undelimited argument taken from the top input list in one go
    /// (tex.web 392–399, as [`Exec::undelimited`] reads it), if it is all
    /// there; else nothing is read.
    fn bulk_undelimited(&mut self, long: bool) -> Option<Arg> {
        let top = self.plain_top()?;
        let plain = |t: &Tok| !matches!(t, Tok::Param(_) | Tok::DontExpand) && (long || !is_par(t));
        let mut c = Cursor::at(top);
        let first = loop {
            let t = c.next(top)?;
            if !plain(t) {
                return None;
            }
            if t.cat() != Some(10) {
                break t;
            }
        };
        let (from, to, after) = if first.cat() == Some(1) {
            let from = c;
            let mut depth = 1;
            loop {
                let before = c;
                let u = c.next(top)?;
                if !plain(u) {
                    return None;
                }
                match u.cat() {
                    Some(1) => depth += 1,
                    Some(2) => {
                        depth -= 1;
                        if depth == 0 {
                            break (from, before, c);
                        }
                    }
                    _ => {}
                }
            }
        } else {
            (c.back(top), c, c)
        };
        let arg = Cursor::arg(top, from, to);
        self.read_to(after);
        Some(arg)
    }

    /// A delimited argument taken from the top input list in one go (as
    /// [`Exec::delimited`] reads it), if it is all there; else nothing is
    /// read.
    fn bulk_delimited(&mut self, delim: &[Tok], long: bool) -> Option<Arg> {
        const MAX: usize = 8;
        let d = delim.len();
        if d > MAX || delim.last().is_some_and(|t| t.cat() == Some(1)) {
            return None;
        }
        let top = self.plain_top()?;
        let plain = |t: &Tok| !matches!(t, Tok::Param(_) | Tok::DontExpand) && (long || !is_par(t));
        let start = Cursor::at(top);
        let mut c = start;
        // (the last tokens read, and where each began)
        let mut last: [Option<(&Tok, Cursor)>; MAX] = [None; MAX];
        let mut n = 0usize;
        let mut depth = 0i32;
        loop {
            let before = c;
            let u = c.next(top)?;
            if !plain(u) {
                return None;
            }
            match u.cat() {
                Some(1) => depth += 1,
                Some(2) => depth -= 1,
                _ => {}
            }
            last[n % MAX] = Some((u, before));
            n += 1;
            if depth == 0 && n >= d && (0..d).all(|i| last[(n - d + i) % MAX].is_some_and(|(t, _)| *t == delim[i])) {
                break;
            }
        }
        let end = last[(n - d) % MAX].map_or(c, |(_, at)| at);
        let mut arg = Cursor::arg(top, start, end);
        // (an argument that is one group loses its braces, tex.web 400)
        let toks = &arg.list[arg.start..arg.end];
        if toks.len() >= 2 && toks[0].cat() == Some(1) && toks[toks.len() - 1].cat() == Some(2) {
            let mut dd = 0;
            let mut whole = true;
            for (k, t) in toks.iter().enumerate() {
                match t.cat() {
                    Some(1) => dd += 1,
                    Some(2) => dd -= 1,
                    _ => {}
                }
                if dd == 0 && k + 1 < toks.len() {
                    whole = false;
                    break;
                }
            }
            if whole {
                arg.start += 1;
                arg.end -= 1;
            }
        }
        self.read_to(c);
        Some(arg)
    }

    /// A `\par` in a short macro's argument ends the call (tex.web 392).
    fn runaway(&mut self, name: &Tok, t: &Tok, long: bool) -> bool {
        if !long && is_par(t) {
            self.err(format!("Paragraph ended before {} was complete", self.shown(name)));
            self.back(t.clone());
            return true;
        }
        false
    }

    fn undelimited(&mut self, name: &Tok, long: bool) -> Option<Vec<Tok>> {
        loop {
            let Some(t) = self.next_raw() else {
                self.err("File ended while scanning an argument".into());
                return Some(Vec::new());
            };
            if self.runaway(name, &t, long) {
                return None;
            }
            match t.cat() {
                Some(10) => {}
                Some(1) => return self.group_body(name, long),
                _ => return Some(vec![t]),
            }
        }
    }

    fn group_body(&mut self, name: &Tok, long: bool) -> Option<Vec<Tok>> {
        let mut out = Vec::new();
        let mut depth = 1;
        while let Some(t) = self.next_raw() {
            if self.runaway(name, &t, long) {
                return None;
            }
            match t.cat() {
                Some(1) => depth += 1,
                Some(2) => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(out);
                    }
                }
                _ => {}
            }
            out.push(t);
        }
        self.err("File ended while scanning an argument".into());
        Some(out)
    }

    fn delimited(&mut self, name: &Tok, delim: &[Tok], long: bool) -> Option<Vec<Tok>> {
        let mut buf: Vec<Tok> = Vec::new();
        let mut depth = 0;
        // (`#{`: the left brace delimits, and stays in the input)
        let brace_delim = delim.last().is_some_and(|t| t.cat() == Some(1));
        loop {
            let Some(t) = self.next_raw() else {
                self.err("File ended while scanning a delimited argument".into());
                break;
            };
            if self.runaway(name, &t, long) {
                return None;
            }
            let at_zero = depth == 0;
            match t.cat() {
                Some(1) => depth += 1,
                Some(2) => depth -= 1,
                _ => {}
            }
            buf.push(t);
            let check = if brace_delim { at_zero } else { depth == 0 };
            if check && buf.len() >= delim.len() && buf[buf.len() - delim.len()..] == *delim {
                // (a `#{` brace is consumed: the body ends with it)
                buf.truncate(buf.len() - delim.len());
                break;
            }
        }
        if buf.len() >= 2 && buf[0].cat() == Some(1) && buf[buf.len() - 1].cat() == Some(2) {
            let mut d = 0;
            let mut whole = true;
            for (k, t) in buf.iter().enumerate() {
                match t.cat() {
                    Some(1) => d += 1,
                    Some(2) => d -= 1,
                    _ => {}
                }
                if d == 0 && k + 1 < buf.len() {
                    whole = false;
                    break;
                }
            }
            if whole {
                buf.pop();
                buf.remove(0);
            }
        }
        Some(buf)
    }

    // ------------------------------------------------------------ commands

    /// Whether the reader is at a line's start after a blank line with
    /// more than blank lines after it: where the cutter would have begun a
    /// piece had it read the catcodes the reader does.
    fn missed_cut(&self) -> bool {
        let t = self.text;
        let p = self.st.pos;
        if p < 2 || p >= t.len() || t[p - 1] != '\r' {
            return false;
        }
        let mut q = p - 1;
        while q > 0 && t[q - 1] == ' ' {
            q -= 1;
        }
        (q == 0 || t[q - 1] == '\r') && t[p..].iter().any(|&c| c != ' ' && c != '\r')
    }

    /// One command.
    fn command(&mut self) {
        // (a piece after the end of the job: TeX never reads it)
        if self.chain.nodes.is_empty() && self.st.fire.is_none() && self.int(&Key::Ended) != 0 {
            self.st.pending.clear();
            self.st.texts = Stack::new();
            self.ptext = None;
            self.st.pos = self.text.len();
            self.kind = Kind::End;
            return;
        }
        // (a blank line passed where a piece may begin: the cutter missed
        // a cut here)
        if self.st.pos > 0
            && self.ptext.is_none()
            && self.st.cut_point()
            && self.missed_cut()
            && self.env.cut_ok()
        {
            self.kind = Kind::Cut;
            return;
        }
        self.at_start = true;
        let next = self.next_x_m(true);
        self.at_start = false;
        let Some((t, m)) = next else {
            if self.input.is_some() {
                self.kind = Kind::Input;
            } else {
                self.end();
            }
            return;
        };
        self.cmd = t.clone();
        let (global, long, outer) = (
            std::mem::take(&mut self.st.global),
            std::mem::take(&mut self.st.long),
            std::mem::take(&mut self.st.outer),
        );
        let protected = std::mem::take(&mut self.st.protected);
        if self.st.mode == Mode::M && self.math_command(&t, &m) {
            return;
        }
        match m {
            Meaning::Prim(p) if p.conditional() => self.conditional(p, &t),
            Meaning::Prim(Prim::Global | Prim::Long | Prim::Outer | Prim::Protected) => {
                let Meaning::Prim(p) = m else { unreachable!() };
                self.prefix(p, global, long, outer, protected);
            }
            _ if assign::prefixable(&m) => self.prefixed(&t, &m, global, long, outer, protected),
            Meaning::Prim(Prim::Mat(x)) => self.mat(x, &t),
            Meaning::Prim(Prim::Math(x)) => self.math_prim(x, &t),
            Meaning::Prim(p) => self.prim(p),
            Meaning::Char(cat, c) => self.char_tok(cat, c, &t),
            Meaning::CharGiven(c) => self.char_cmd(c, &t),
            Meaning::MathCharGiven(_) => self.insert_dollar_sign(&t),
            Meaning::Undefined => self.err(format!("Undefined control sequence {}", self.shown(&t))),
            Meaning::Macro(_)
            | Meaning::CountRef(_)
            | Meaning::DimenRef(_)
            | Meaning::SkipRef(_)
            | Meaning::MuskipRef(_)
            | Meaning::ToksRef(_)
            | Meaning::Font(_) => {}
        }
    }

    /// The end of the piece: its last paragraph, its open groups and
    /// conditionals.
    fn end(&mut self) {
        if self.halted() {
            self.kind = Kind::End;
            return;
        }
        self.end_par();
        self.kind = Kind::End;
        // (groups and conditionals open where the piece was entered are
        // its caller's, closed after it)
        let entry = self.chain.input(0);
        if self.st.groups.len() > entry.groups.len() {
            self.err("Missing } inserted at the end of a piece".into());
        }
        if self.st.conds.len() > entry.conds.len() {
            self.err("Incomplete \\if at the end of a piece".into());
        }
    }

    /// A group opens: a scope (tex.web 1063).
    fn open_group(&mut self, g: GroupKind) {
        self.st.groups.push(Group {
            kind: g,
            after: Vec::new(),
            boxg: None,
            at: self.chain.nodes.len() as u32,
            align: None,
        });
        self.kind = Kind::Open;
    }

    /// This node closes the innermost group. What it defined locally
    /// before, reading up to the closing command (`\csname`'s `\relax`
    /// in `\expandafter\endgroup`, tex.web 372), was inside that group
    /// and ends with it; a definition at an enclosing group's level
    /// (`Def::level`, counted from the node's own group) is one level
    /// nearer, the node's group being the one outside.
    fn closes(&mut self) {
        self.defs.retain_mut(|d| {
            d.global
                || match d.level {
                    None | Some(0) => false,
                    Some(k) => {
                        d.level = Some(k - 1);
                        true
                    }
                }
        });
        self.kind = Kind::Close;
    }

    /// Where a box's diagnostic says it was made (tex.web 663).
    fn box_place(&self) -> crate::show::Place {
        if self.st.in_output() {
            crate::show::Place::Output
        } else {
            crate::show::Place::Here
        }
    }

    /// The innermost group closes: it defines nothing; the tokens
    /// `\aftergroup` gave it rewrite the input (tex.web 282).
    fn close_group(&mut self) {
        let after = self.st.groups.pop().map(|g| g.after).unwrap_or_default();
        self.rewrite(after);
        self.closes();
    }

    fn char_tok(&mut self, cat: u8, c: char, t: &Tok) {
        match cat {
            1 => self.open_group(GroupKind::Simple),
            2 => match self.st.groups.last().map(|g| g.kind) {
                Some(GroupKind::Simple) => self.close_group(),
                Some(GroupKind::Box) => self.close_box(t),
                Some(GroupKind::Math) => self.math_right_brace(),
                Some(GroupKind::Output) => self.output_close(t),
                Some(GroupKind::SemiSimple) => self.err("Extra }, or forgotten \\endgroup".into()),
                Some(GroupKind::Align) => self.missing_cr(t),
                Some(GroupKind::NoAlign) => self.noalign_close(t),
                None => self.err("Too many }'s".into()),
            },
            4 => self.align_error(t),
            10 => {
                if self.st.mode == Mode::H {
                    self.space_cmd();
                }
            }
            11 | 12 => self.char_cmd(u32::from(c), t),
            3 => self.math_shift(t),
            7 | 8 => self.sub_sup_cmd(cat == 7, t),
            _ => self.err(format!(
                "Character {c} of category {cat} is not supported yet"
            )),
        }
    }

    /// A primitive that is neither expanded, a conditional nor an
    /// assignment.
    fn prim(&mut self, p: Prim) {
        match p {
            Prim::Par => self.par_cmd(),
            Prim::Indent | Prim::Noindent => self.start_par(p == Prim::Indent),
            // (pdfTeX: `start_par` with chr 2, a paragraph in vertical mode
            // only)
            Prim::QuitVmode => {
                if self.st.mode == Mode::V {
                    self.start_par(true);
                }
            }
            // (pdfTeX: two general texts, expanded; the table they fill is
            // read only when writing PDF)
            Prim::GlyphToUnicode => {
                let _ = self.scan_toks_x();
                let _ = self.scan_toks_x();
            }
            Prim::Space => self.ex_space(),
            Prim::Char => {
                let t = self.cmd.clone();
                self.char_cmd_num(&t);
            }
            Prim::BeginGroup => self.open_group(GroupKind::SemiSimple),
            Prim::EndGroup => match self.st.groups.last().map(|g| g.kind) {
                Some(GroupKind::SemiSimple) => self.close_group(),
                Some(GroupKind::Math) => {
                    let t = self.cmd.clone();
                    self.math_off_save(&t);
                }
                Some(GroupKind::Simple | GroupKind::Box | GroupKind::Output | GroupKind::Align | GroupKind::NoAlign) => {
                    // (tex.web 1064: the missing `}` goes in first)
                    self.err("Missing } inserted".into());
                    let t = self.cmd.clone();
                    self.rewrite(vec![Tok::Char(2, '}'), t]);
                }
                None => self.err("Extra \\endgroup".into()),
            },
            Prim::AfterGroup => {
                if let Some(t) = self.next_raw()
                    && let Some(g) = self.st.groups.last_mut()
                {
                    g.after.push(t);
                }
            }
            Prim::AfterAssignment => self.st.after_assignment = self.next_raw().map(Box::new),
            Prim::IgnoreSpaces => {
                // (the next non-blank token, expanded, is the next command)
                loop {
                    match self.next_x_m(false) {
                        Some((t, _)) if t.cat() == Some(10) => {}
                        Some((t, _)) => {
                            self.back(t);
                            break;
                        }
                        None => break,
                    }
                }
            }
            Prim::Message | Prim::ErrMessage => self.message(p == Prim::ErrMessage),
            Prim::SetLanguage => self.set_language(),
            Prim::Show(c) => self.show_cmd(c),
            Prim::Immediate => self.immediate(),
            Prim::Write => self.write_cmd(),
            Prim::OpenOut | Prim::CloseOut | Prim::Special => self.extension(p),
            Prim::InStream(open) => self.open_in(open),
            Prim::Input => self.start_input(),
            Prim::Uppercase | Prim::Lowercase => self.case_shift(p == Prim::Uppercase),
            Prim::EndCsName => self.err("Extra \\endcsname".into()),
            Prim::End | Prim::Dump => self.stop_job(p == Prim::Dump),
            Prim::Align(a) => self.align_cmd(a),
            Prim::Relax => {}
            Prim::Marks | Prim::Discards(_) | Prim::ShowE(_) | Prim::Lr(_) => self.etex_command(p),
            Prim::LastItem(_)
            | Prim::SpaceFactor
            | Prim::PrevDepth
            | Prim::PrevGraf
            | Prim::PageInt(_)
            | Prim::PageDimen(_)
            | Prim::BoxDimen(_)
            | Prim::PdfItem(_) => self.err(format!("You can't use `\\{}' here", p.name())),
            _ => self.err(format!("\\{} is not supported yet", p.name())),
        }
    }
}

/// The shared copy of a name.
fn intern(names: &mut FxSet<Arc<str>>, s: &str) -> Arc<str> {
    if let Some(a) = names.get(s) {
        return a.clone();
    }
    let a: Arc<str> = Arc::from(s);
    names.insert(a.clone());
    a
}


