//! What a whatsit node is to the writer: the data of pdfTeX's whatsit
//! nodes (pdfTeX §1342ff, "data structure for ..." in pdfTeX §695), as
//! the TeX side holds it when the node is shipped.

use phitex_typeset::arith::Scaled;

/// `pdf_literal_mode` (pdfTeX §695).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LiteralMode {
    /// `set_origin`: end the text, move the origin to the current point.
    SetOrigin,
    /// `direct_page` (`\pdfliteral page`): end the text only.
    DirectPage,
    /// `direct_always` (`\pdfliteral direct`): end the string only.
    DirectAlways,
    /// `scan_special`: a `\special` whose `PDF:` prefix says the mode.
    ScanSpecial,
}

/// A `\pdfcolorstack` command (`colorstack_set` ... `colorstack_current`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ColorStackCmd {
    /// `set{...}`.
    Set(Vec<u8>),
    /// `push{...}`.
    Push(Vec<u8>),
    /// `pop`.
    Pop,
    /// `current`.
    Current,
}

/// A destination's or a thread's identifier: `num n` or `name{...}`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Ident {
    /// `num n`.
    Num(i32),
    /// `name{...}`, the text as written.
    Name(Vec<u8>),
}

/// An action (`scan_action`, pdfTeX §1546): what `write_action` writes.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    /// `user{...}`: the text as it is.
    User(Vec<u8>),
    /// `goto page n {...}`.
    Page {
        /// `file{...}`.
        file: Option<Vec<u8>>,
        /// The page number.
        page: i32,
        /// The destination's text after the page.
        spec: Vec<u8>,
        /// `newwindow` (1), `nonewwindow` (2) or neither (0).
        new_window: u8,
        /// `struct ...`.
        struct_id: Option<Ident>,
    },
    /// `goto num n` or `goto name{...}`.
    Goto {
        /// `file{...}`.
        file: Option<Vec<u8>>,
        /// The destination.
        id: Ident,
        /// `newwindow` (1), `nonewwindow` (2) or neither (0).
        new_window: u8,
        /// `struct ...`.
        struct_id: Option<Ident>,
    },
    /// `thread num n` or `thread name{...}`.
    Thread {
        /// `file{...}`.
        file: Option<Vec<u8>>,
        /// The thread.
        id: Ident,
    },
}

/// A destination's type and its data (`pdf_dest_type`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DestKind {
    /// `xyz`, with `zoom n` or not.
    Xyz(Option<i32>),
    /// `fit`.
    Fit,
    /// `fith`.
    FitH,
    /// `fitv`.
    FitV,
    /// `fitb`.
    FitB,
    /// `fitbh`.
    FitBH,
    /// `fitbv`.
    FitBV,
    /// `fitr` with its rule specification (width, height, depth; each
    /// [`RUNNING`](phitex_typeset::node::RUNNING) when not given).
    FitR(Scaled, Scaled, Scaled),
}

/// A whatsit node as the writer meets it in a shipped list. The
/// dimensions of annotations, links and threads are those of the rule
/// specification, each [`RUNNING`](phitex_typeset::node::RUNNING) when
/// not given.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Whatsit {
    /// `\special{...}` (`pdf_special`): only a `PDF:` or `pdf:` text
    /// is written.
    Special(Vec<u8>),
    /// `\pdfliteral` (`pdf_out_literal`).
    Literal(LiteralMode, Vec<u8>),
    /// `\pdfcolorstack n ...` (`pdf_out_colorstack`).
    ColorStack(usize, ColorStackCmd),
    /// `\pdfsetmatrix{...}` (`pdf_out_setmatrix`).
    SetMatrix(Vec<u8>),
    /// `\pdfsave`.
    Save,
    /// `\pdfrestore`.
    Restore,
    /// `\pdfrefobj n`.
    RefObj(usize),
    /// `\pdfrefxform n`: the TeX side follows the node with an empty box
    /// of the form's size, which moves the position past it.
    RefXForm(usize),
    /// `\pdfrefximage n`: as [`Whatsit::RefXForm`].
    RefXImage(usize),
    /// `\pdfannot`: its object number, width, height, depth and text.
    Annot {
        /// `pdf_annot_objnum`: a number of this file, or the client's
        /// read-time identity for one (64 bits on every target).
        objnum: u64,
        /// Width.
        width: Scaled,
        /// Height.
        height: Scaled,
        /// Depth.
        depth: Scaled,
        /// The annotation's text.
        data: Vec<u8>,
    },
    /// `\pdfstartlink`.
    StartLink {
        /// `pdf_link_objnum`: as [`Whatsit::Annot`]'s.
        objnum: u64,
        /// Width.
        width: Scaled,
        /// Height.
        height: Scaled,
        /// Depth.
        depth: Scaled,
        /// `attr{...}`.
        attr: Option<Vec<u8>>,
        /// The action.
        action: Action,
    },
    /// `\pdfendlink`.
    EndLink,
    /// `\pdfdest`: the identifier, `struct n` (a structure destination)
    /// and the type.
    Dest {
        /// The identifier.
        id: Ident,
        /// `struct n`.
        struct_num: Option<i32>,
        /// The type.
        kind: DestKind,
    },
    /// `\pdfthread` (`start` false) or `\pdfstartthread` (`start` true).
    Thread {
        /// `\pdfstartthread`.
        start: bool,
        /// The thread.
        id: Ident,
        /// Width.
        width: Scaled,
        /// Height.
        height: Scaled,
        /// Depth.
        depth: Scaled,
        /// `attr{...}`.
        attr: Option<Vec<u8>>,
    },
    /// `\pdfendthread`.
    EndThread,
    /// `\pdfsavepos`: nothing is written; the TeX side reads the
    /// position.
    SavePos,
    /// `\pdfinterwordspaceon`.
    InterwordSpaceOn,
    /// `\pdfinterwordspaceoff`.
    InterwordSpaceOff,
    /// `\pdffakespace`.
    FakeSpace,
    /// `\pdfrunninglinkoff`.
    RunningLinkOff,
    /// `\pdfrunninglinkon`.
    RunningLinkOn,
}

/// The whatsits of a shipped list: the node's handle and whether leaders
/// are being output, answered with the node's data (or `None` for a
/// whatsit the writer does not see, such as `\write`).
pub type WhatsitSource<'a> = dyn FnMut(u32, bool) -> Option<Whatsit> + 'a;
