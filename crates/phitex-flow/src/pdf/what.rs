//! pdfTeX's PDF-only commands as values (pdftex.web part 53): what a
//! command puts in a list ([`What`], a whatsit the page's walk turns into
//! stream text and marks), and what it asks of the PDF file at its place
//! ([`Event`], a step of the fold, which numbers objects in document
//! order). A command that reads what the fold made (`\pdflastobj` after
//! `\pdfobj`, the dimensions of an image referred to by number) reads the
//! fold's answer to its event (`val::Key::PdfAns`, answered by the build
//! from the last fold, CORE.md "pdfTeX's read-backs").

use std::sync::Arc;

use crate::item::BoxNode;
use crate::tfm::Font;

/// Text as pdfTeX's strings hold it (one byte a character).
pub type Text = Arc<[u8]>;

/// `\pdfliteral`'s and `\special`'s modes (pdftex.web §640).
pub const SET_ORIGIN: u8 = 0;
pub const DIRECT_PAGE: u8 = 1;
pub const DIRECT_ALWAYS: u8 = 2;

/// `\pdfcolorstack`'s commands.
pub const CS_SET: u8 = 0;
pub const CS_PUSH: u8 = 1;
pub const CS_POP: u8 = 2;
pub const CS_CURRENT: u8 = 3;

/// `\pdfdest`'s kinds (`pdf_dest_xyz` … `pdf_dest_fitr`).
pub const DEST_XYZ: u8 = 0;
pub const DEST_FIT: u8 = 1;
pub const DEST_FITH: u8 = 2;
pub const DEST_FITV: u8 = 3;
pub const DEST_FITB: u8 = 4;
pub const DEST_FITBH: u8 = 5;
pub const DEST_FITBV: u8 = 6;
pub const DEST_FITR: u8 = 7;

/// An identifier: `num n` or `name{…}` (destinations, threads).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Id {
    Num(i32),
    Name(Text),
}

/// An action (`scan_action`): `user{…}`, `goto …`, `thread …`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Action {
    /// `pdf_action_page` (0), `goto` (1), `thread` (2), `user` (3).
    pub kind: u8,
    /// `user`'s text; `page`'s text after the number.
    pub text: Text,
    pub file: Option<Text>,
    /// The destination or thread (`page`: `Num` of the page).
    pub id: Id,
    /// `struct`: a number or a name (with `file`, its text).
    pub struct_id: Option<Id>,
    /// `newwindow` (1), `nonewwindow` (2).
    pub new_window: u8,
}

pub const ACTION_PAGE: u8 = 0;
pub const ACTION_GOTO: u8 = 1;
pub const ACTION_THREAD: u8 = 2;
pub const ACTION_USER: u8 = 3;

/// A whatsit of pdfTeX's (pdftex.web §1529–§1599), as a list holds it.
/// Widths, heights and depths are `item::RUNNING` where not given.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum What {
    /// `\pdfliteral`: the text as expanded, and its mode.
    Literal { mode: u8, data: Text },
    /// `\pdfcolorstack n set/push{…}`, `pop`, `current`.
    ColorStack { stack: i32, cmd: u8, data: Option<Text> },
    /// `\pdfsetmatrix{…}`.
    SetMatrix(Text),
    /// `\pdfsave`, `\pdfrestore`.
    Save,
    Restore,
    /// `\pdfrefobj n`.
    RefObj(i32),
    /// `\pdfrefxform n`, `\pdfrefximage n`: the object and its size as
    /// the command read it (a box's material: it has width, height and
    /// depth, pdftex.web §1586).
    RefXform { n: i32, w: i32, h: i32, d: i32 },
    RefXimage { n: i32, w: i32, h: i32, d: i32 },
    /// `\pdfannot`: its object (given or made by the command), size and
    /// text.
    Annot { objnum: i32, w: i32, h: i32, d: i32, data: Text },
    /// `\pdfstartlink`, `\pdfendlink`.
    StartLink { objnum: i32, w: i32, h: i32, d: i32, attr: Option<Text>, action: Arc<Action> },
    EndLink,
    /// `\pdfdest`.
    Dest { id: Id, struct_num: Option<i32>, kind: u8, zoom: Option<i32>, w: i32, h: i32, d: i32 },
    /// `\pdfthread` (`start` false), `\pdfstartthread`, `\pdfendthread`.
    Thread { start: bool, id: Id, w: i32, h: i32, d: i32, attr: Option<Text> },
    EndThread,
    /// `\pdfsavepos`.
    SavePos,
    /// `\pdfrunninglinkoff` (false), `\pdfrunninglinkon`.
    RunningLink(bool),
}

impl What {
    /// The size a list sees: an image's or a form's (pdftex.web's
    /// "Incorporate a whatsit node into an hbox").
    pub fn dims(&self) -> Option<(i32, i32, i32)> {
        match self {
            What::RefXform { w, h, d, .. } | What::RefXimage { w, h, d, .. } => Some((*w, *h, *d)),
            _ => None,
        }
    }
}

/// The parameters an event reads where its command is (what pdfTeX reads
/// when the object is written, if that is then: `check_pdfversion` at the
/// first object, `pdf_compress_level` of an object written now).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct Params {
    pub output: i32,
    pub major: i32,
    pub minor: i32,
    pub compress_level: i32,
    pub obj_compress_level: i32,
    pub draftmode: i32,
    pub decimal_digits: i32,
    pub copy_fonts: i32,
    pub image_hicolor: i32,
    pub image_apply_gamma: i32,
    pub gamma: i32,
    pub image_gamma: i32,
    pub inclusion_errorlevel: i32,
    pub suppress_dup_dest: i32,
    pub suppress_ptex_info: i32,
    pub suppress_pagegroup: i32,
    pub omit_procset: i32,
}

/// What a command asks of the PDF file at its place, in document order
/// (an [`Item::Pdf`](crate::item::Item::Pdf) of the material: the page
/// builder hands the events to the pages in the order it meets them, the
/// join's fold does them between the pages). The fold's answers to an
/// event are `val::Key::PdfAns` read by its command (fields listed with
/// each; [`ANSWER_FIELDS`] at most).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// `\pdfobj`: `reserveobjnum` (made, nothing else), or an object
    /// (`useobjnum k`: that number), written now with `\immediate`.
    /// Answer: \[the object's number, 1 if `k` was invalid].
    Obj {
        reserve: bool,
        use_num: Option<i32>,
        stream: bool,
        attr: Option<Text>,
        /// `file`: `data` is a file's name, `bytes` its contents (read by
        /// the command; `None` if there is no such file).
        file: bool,
        bytes: Option<Arc<[u8]>>,
        data: Text,
        immediate: bool,
        params: Arc<Params>,
    },
    /// `\pdfxform`: a box as a form (written now with `\immediate`, else
    /// at the end of the first page using it). Answer: \[its number].
    Xform { attr: Option<Text>, resources: Option<Text>, b: Arc<BoxNode>, immediate: bool, params: Arc<Params> },
    /// `\pdfximage`: an image read (its size scaled by the command), written
    /// now with `\immediate`. Answer: \[its number].
    Ximage {
        image: Arc<super::image::Image>,
        w: i32,
        h: i32,
        d: i32,
        attr: Option<Text>,
        immediate: bool,
        params: Arc<Params>,
    },
    /// `\pdfrefobj n` (kind 7), `\pdfrefxform n` (8), `\pdfrefximage n` (9):
    /// whether object `n` is one (`pdf_check_obj`); for a form or an image,
    /// its width, height, depth. Answer: \[1 if it is, w, h, d].
    Check { kind: u8, n: i32 },
    /// `\pdfannot reserveobjnum`, `\pdfannot [useobjnum k]`,
    /// `\pdfstartlink`: the annotation's number. Answer: \[the number, 1 if
    /// `k` is invalid].
    Annot { reserve: bool, use_num: Option<i32> },
    /// `\pdfdest`'s check (`find_obj`: a destination of that name shipped
    /// already). Answer: \[1 if it is a duplicate].
    DestCheck { id: Id, structd: bool },
    /// `\pdfoutline`: an entry, its action and title written now.
    Outline { attr: Option<Text>, action: Arc<Action>, count: i32, title: Text, params: Arc<Params> },
    /// `\pdfcatalog`, with `openaction` (written now).
    Catalog { toks: Text, open: Option<Arc<Action>>, params: Arc<Params> },
    /// `\pdfinfo`, `\pdfnames`, `\pdftrailer`, `\pdftrailerid`.
    Info(Text),
    Names(Text),
    Trailer(Text),
    TrailerId(Text),
    /// `\pdfcolorstackinit [page] [direct|page] {…}` (the literal mode).
    /// Answer: \[its number, or -1 if there are too many].
    ColorStackInit { page: bool, mode: u8, data: Text },
    /// `\pdfcolorstack n`'s check. Answer: \[`colorstackused`].
    ColorStacks,
    /// A font's PDF object made if it is not (`pdf_init_font`:
    /// `\pdffontobjnum`, `\pdffontname`). Answer: \[its object number, the
    /// font whose name it is shown with (`ff`)].
    FontInit(Arc<Font>),
    /// `\pdfincludechars`: the font initialized, characters marked used.
    IncludeChars(Arc<Font>, Vec<u8>),
    /// `\pdffontattr`, `\pdfnobuiltintounicode`.
    FontAttr(Arc<Font>, Text),
    NoBuiltinToUnicode(Arc<Font>),
    /// `\pdfmapfile`, `\pdfmapline`.
    MapFile(Text),
    MapLine(Text),
    /// `\pdfpageref n`: the page's object (made if it is not yet).
    /// Answer: \[its number].
    PageRef(i32),
    /// `\pdfximagebbox n k`. Answer: \[the value].
    XimageBbox(i32, i32),
    /// `\pdflastxpos`, `\pdflastypos` read where no page is being shipped.
    /// Answer: \[x, y].
    LastPos,
}

/// Where an event's command is: scope (0 a piece's node; 1 an output
/// routine's), the fire (the routine's ordinal among the job's fires; 0
/// for a piece), the node, the event's index among the node's.
pub type Tag = [u32; 4];

/// An event in the material.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PdfItem {
    pub tag: Tag,
    pub event: Event,
}

/// The answer fields of an event (`val::Key::PdfAns`'s last part is
/// `index * ANSWER_FIELDS + field`).
pub const ANSWER_FIELDS: u32 = 8;

/// The field an answer echoes its event's number in (`Check`'s `n`, `Annot`'s
/// `useobjnum`): an answer read for another number (a round that changed
/// it) is no answer yet, so a stale one never fails the build.
pub const ECHO: u32 = 7;

/// Object types (`obj_type_obj` …) [`Event::Check`] names.
pub const OBJ_TYPE_OBJ: u8 = 7;
pub const OBJ_TYPE_XFORM: u8 = 8;
pub const OBJ_TYPE_XIMAGE: u8 = 9;
