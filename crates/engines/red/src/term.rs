//! The term: thunks, nodes and the input a thunk holds.
use std::sync::Arc;

use crate::env::Env;
use crate::item::Item;
use crate::tok::Toks;
use crate::val::{Key, Val};

/// A source text: a file or a `\scantokens`/`\read` text.
pub type TextId = u32;

/// Where a text node read: (text, byte range). Edits map onto these.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Span {
    pub text: TextId,
    pub from: u32,
    pub to: u32,
}

/// Tokenizer state at a text position (tex.web's N, M, S).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Line {
    NewLine,
    MidLine,
    SkipBlanks,
}

/// Why a token list is in the input (for `\endtemplate`, errors, `\showcontext`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum TokKind {
    Macro,
    Backed,
    Inserted,
    UTemplate,
    VTemplate,
    Output,
    EveryPar,
    EveryMath,
    EveryDisplay,
    EveryHbox,
    EveryVbox,
    EveryJob,
    EveryCr,
    EveryEof,
    Mark,
    Write,
    AfterGroup,
    AfterAssignment,
    Other,
}

/// One source in the input.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Src {
    Text {
        text: TextId,
        data: Arc<str>,
        pos: u32,
        line: Line,
        /// The current line's end, and the end-of-line char appended (None: `\endlinechar` out of range).
        line_end: u32,
        eol: Option<char>,
        /// `\endinput` seen: the file ends at this line's end.
        ending: bool,
    },
    Toks { toks: Toks, at: u32, kind: TokKind },
}

/// The rest of the input: an immutable list of sources, innermost first.
/// Not a machine stack: a value; taking a token makes a new `Input`.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Input(pub Option<Arc<(Src, Input)>>);

impl Input {
    pub fn push(&self, s: Src) -> Input {
        Input(Some(Arc::new((s, self.clone()))))
    }
    pub fn top(&self) -> Option<&Src> {
        self.0.as_deref().map(|(s, _)| s)
    }
    pub fn below(&self) -> Input {
        self.0.as_deref().map(|(_, b)| b.clone()).unwrap_or_default()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_none()
    }
}

/// `pending(env; text)`.
#[derive(Clone, Debug)]
pub struct Thunk {
    pub env: Env,
    pub input: Input,
}

pub type NodeId = u32;

/// What a node is.
#[derive(Clone, Debug)]
pub enum Op {
    /// A non-expandable command (a primitive's id and code), or a run of characters.
    Command(crate::prim::PrimId),
    Chars,
    /// A macro application: the macro's value and its arguments.
    Apply { f: Arc<crate::val::Macro>, args: Arc<[Toks]> },
    /// An expandable primitive (rewrites input).
    Expand(crate::prim::PrimId),
    /// A conditional: the test's value and both arms as thunks (the untaken one never reduced).
    Select { taken: bool, untaken: Option<Span> },
    /// An output routine applied where the page builder fired.
    Fire { page: u32 },
}

/// What a node produced besides the rest input.
#[derive(Clone, Debug, Default)]
pub struct Out {
    pub delta: crate::env::Delta,
    pub emits: Vec<Emit>,
}

/// A contribution to the current list (or to the job).
#[derive(Clone, Debug, PartialEq)]
pub enum Emit {
    Item(Item),
    /// `\unskip` & co: remove the tail if it is of this kind (0 penalty 1 kern 2 glue 3 box (`\lastbox`)).
    RemoveTail(u8),
    /// A list opens (its mode) / closes; the closer's node gathers the items between.
    Open(crate::env::Mode),
    Close,
    /// A page shipped (`\shipout`).
    Ship(Arc<crate::output::Shipped>),
    /// Text written to a stream at once (`\immediate\write`), or the terminal/log (17+).
    Write { stream: u8, text: Arc<str> },
    /// An error or warning (`diag.rs`).
    Diag(Arc<crate::diag::Diag>),
}

/// A reduced node: its op, the input it consumed, every read with the value
/// seen, its result. In SSA terms: `uses` are its edges to the definitions it
/// read (`DefId`s), `cells` the definitions it made (in the order its delta
/// makes them), `reads` the values it saw (for answered keys, the only record).
#[derive(Clone, Debug)]
pub struct Node {
    pub op: Op,
    pub spans: Vec<Span>,
    pub reads: Vec<(Key, Val)>,
    /// Def-use edges: each name read from before the node, with the definition that reached it.
    pub uses: Vec<(Key, crate::env::DefId)>,
    /// The definitions this node made, in delta order (`Change::Def`,
    /// `Change::Replace`: one each; `Change::OpenList`: four, the list's keys).
    pub cells: Vec<Arc<crate::env::DefCell>>,
    pub out: Out,
    /// The thunk after it.
    pub next: Thunk,
    /// The edit epoch its spans and inputs are in (`Graph::input_now`).
    pub epoch: u32,
}
