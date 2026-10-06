//! pdfTeX part 53: the PDF extension commands (pdfTeX §1529–§1599).
//!
//! These build whatsit nodes and fill the object table; what needs the
//! PDF file itself (objects written at once) waits for the writer.

use alloc::boxed::Box;
use alloc::sync::Arc;

use partex_engine::node::{Action, Dims, Node, PdfId, PdfWhatsit, Tokens, Whatsit};

use super::objtab::{
    Aux, Id, OBJ_TYPE_DEST, OBJ_TYPE_OBJ, OBJ_TYPE_OTHERS, OBJ_TYPE_STRUCT_DEST, OBJ_TYPE_XFORM,
    OBJ_TYPE_XIMAGE, RawObj, SUP_OBJ_TAB_SIZE, XForm,
};
use crate::host::Host;
use crate::tex::{Jump, Tex};
use crate::track::Tracker;
use crate::web::*;

/// A rule dimension not given (`null_flag`).
pub(crate) const RUNNING: i32 = -0o10000000000;

/// The writers' tables command `chr` of `do_pdf_extension` reads and
/// writes (`pdf::val`'s bits; a write is a read too). The `\pdflast…`
/// values are stored whole where they are set.
fn pdf_extension_fields(chr: i32) -> (u64, u64) {
    use super::val::{OBJECTS, WRITING, bit, field};
    match chr {
        PDF_COLORSTACK_NODE => (bit(field::STACKS), 0),
        // (a font's type, which asks the virtual font file)
        PDF_FONT_EXPAND_CODE => (0, bit(field::PDF_FONTS)),
        PDF_OBJ_CODE => (0, OBJECTS | bit(field::OBJ_COUNT)),
        PDF_REFOBJ_NODE | PDF_REFXFORM_NODE | PDF_REFXIMAGE_NODE => (bit(field::OBJS), 0),
        PDF_XFORM_CODE => (0, OBJECTS | bit(field::XFORM_COUNT)),
        // (a PNG with an alpha channel sets the page's group)
        PDF_XIMAGE_CODE => (
            0,
            WRITING | bit(field::XIMAGE_COUNT) | bit(field::EPDF) | bit(field::SHIP),
        ),
        PDF_OUTLINE_CODE => (0, WRITING | bit(field::OUTLINES)),
        PDF_ANNOT_NODE | PDF_START_LINK_NODE | PDF_DEST_NODE => (0, OBJECTS),
        PDF_INFO_CODE => (0, bit(field::INFO_TOKS)),
        PDF_TRAILER_CODE => (0, bit(field::TRAILER_TOKS)),
        PDF_TRAILER_ID_CODE => (0, bit(field::TRAILER_ID_TOKS)),
        PDF_CATALOG_CODE => (
            0,
            WRITING | bit(field::CATALOG_TOKS) | bit(field::CATALOG_OPENACTION),
        ),
        PDF_NAMES_CODE => (0, bit(field::NAMES_TOKS)),
        PDF_FONT_ATTR_CODE => (0, bit(field::FONT_ATTR)),
        PDF_NOBUILTIN_TOUNICODE_CODE => (0, bit(field::NOBUILTIN_TOUNICODE)),
        PDF_SPACE_FONT_CODE => (0, bit(field::SPACE_FONT_NAME)),
        // (whatsits: no table)
        _ => (0, 0),
    }
}

fn running() -> Dims {
    Dims {
        width: RUNNING,
        height: RUNNING,
        depth: RUNNING,
    }
}

impl<H: Host, T: Tracker> Tex<H, T> {
    /// `scan_pdf_ext_toks`: a general text, expanded (like `\special`).
    pub(crate) fn scan_pdf_ext_toks(&mut self) -> Result<Tokens, Jump> {
        self.scan_toks(false, true)?;
        Ok(self.take_def())
    }

    /// `scan_pdf_ext_late_toks`: a general text, not expanded.
    fn scan_pdf_ext_late_toks(&mut self) -> Result<Tokens, Jump> {
        self.scan_toks(false, false)?;
        Ok(self.take_def())
    }

    /// `tokens_to_string` of a kept token list.
    pub(crate) fn tokens_string(&mut self, t: &Tokens) -> alloc::vec::Vec<u8> {
        let old_setting = self.selector();
        self.set_selector(crate::print::NEW_STRING);
        let b = self.pool_ptr;
        let l = i32::try_from(self.pool_size() - self.pool_ptr).unwrap_or(i32::MAX);
        self.show_token_slice(t, l);
        self.set_selector(old_setting);
        let s = self.str_pool[b..self.pool_ptr].to_vec();
        self.pool_ptr = b;
        s
    }

    /// pdfTeX §1529: `new_whatsit`, appended at once (the arguments are
    /// scanned with it as the tail) and then filled in by `scan`.
    fn append_whatsit(
        &mut self,
        scan: impl FnOnce(&mut Self) -> Result<Option<PdfWhatsit>, Jump>,
    ) -> Result<(), Jump> {
        self.tail_append(Node::Whatsit(Box::new(Whatsit::Pdf(Box::new(
            PdfWhatsit::EndLink,
        )))));
        let w = scan(self)?;
        self.pop_tail();
        if let Some(w) = w {
            self.tail_append(Node::Whatsit(Box::new(Whatsit::Pdf(Box::new(w)))));
        }
        Ok(())
    }

    /// pdfTeX §698: `pdf_create_obj`, with its overflow check.
    pub(crate) fn pdf_create_obj(&mut self, t: usize, i: Id) -> Result<i32, Jump> {
        self.pdf_create_obj_named(t, i, None)
    }

    /// [`Self::pdf_create_obj`], a virtual id named by `name` if given
    /// (an identity TeX defines, which an edit does not move) or else as
    /// `ObjTab::create` names it.
    pub(crate) fn pdf_create_obj_named(
        &mut self,
        t: usize,
        i: Id,
        name: Option<u128>,
    ) -> Result<i32, Jump> {
        if self.pdf.objs.len() > SUP_OBJ_TAB_SIZE {
            self.overflow(b"indirect objects table size", self.pdf.objs.sys_obj_ptr())?;
        }
        let k = match name {
            Some(_) => self.pdf.objs.create_named(t, i, name),
            None => self.pdf.objs.create(t, i),
        };
        if self.pdf.objs.virt {
            self.out_mark(crate::effects::Effect::Num(
                super::vnum::NumEvent::Create(k),
                0,
            ));
        }
        Ok(k)
    }

    /// The object TeX names by number `n` (virtual numbers: the one
    /// pdfTeX numbers `n`, observed; `-1` if none).
    pub(crate) fn given_obj(&mut self, n: i32) -> i32 {
        if self.pdf.objs.virt {
            if n <= 0 {
                return n;
            }
            self.obj_observe();
            self.pdf.objs.of_final(n).unwrap_or(-1)
        } else {
            n
        }
    }

    /// `k <= obj_ptr`: object `k` is one users see.
    fn obj_in_range(&mut self, k: i32) -> bool {
        if self.pdf.objs.virt {
            self.pdf.objs.log_read(k);
            self.pdf.objs.entry(k).is_some()
        } else {
            k <= self.pdf.objs.obj_ptr
        }
    }

    /// `pdf_new_objnum`.
    pub(crate) fn pdf_new_objnum(&mut self) -> Result<i32, Jump> {
        self.pdf_create_obj(OBJ_TYPE_OTHERS, Id::Num(0))
    }

    /// pdfTeX §1545: `pdf_check_obj`.
    pub(crate) fn pdf_check_obj(&mut self, t: usize, n: i32) -> Result<(), Jump> {
        if !self.pdf.objs.on_list(t, n) {
            return self.pdf_error(b"ext1", b"cannot find referenced object");
        }
        Ok(())
    }

    /// pdfTeX §1552: `scan_alt_rule`.
    pub(crate) fn scan_alt_rule(&mut self) -> Result<Dims, Jump> {
        let mut d = running();
        loop {
            if self.scan_keyword(b"width")? {
                self.scan_normal_dimen()?;
                d.width = self.cur_val;
            } else if self.scan_keyword(b"height")? {
                self.scan_normal_dimen()?;
                d.height = self.cur_val;
            } else if self.scan_keyword(b"depth")? {
                self.scan_normal_dimen()?;
                d.depth = self.cur_val;
            } else {
                return Ok(d);
            }
        }
    }

    /// pdfTeX §1556: `scan_action`.
    pub(crate) fn scan_action(&mut self) -> Result<Action, Jump> {
        let kind = if self.scan_keyword(b"user")? {
            3
        } else if self.scan_keyword(b"goto")? {
            1
        } else if self.scan_keyword(b"thread")? {
            2
        } else {
            return self.pdf_error(b"ext1", b"action type missing");
        };
        let mut a = Action {
            kind,
            tokens: None,
            id: PdfId::Num(0),
            file: None,
            struct_id: None,
            new_window: 0,
        };
        if kind == 3 {
            a.tokens = Some(self.scan_pdf_ext_toks()?);
            return Ok(a);
        }
        if self.scan_keyword(b"file")? {
            a.file = Some(self.scan_pdf_ext_toks()?);
        }
        if self.scan_keyword(b"struct")? {
            if a.kind != 1 {
                return self.pdf_error(b"ext1", b"only GoTo action can be used with `struct'");
            }
            // (with `file`, the identifier is a name without `name`)
            if a.file.is_some() || self.scan_keyword(b"name")? {
                a.struct_id = Some(PdfId::Name(self.scan_pdf_ext_toks()?));
            } else if self.scan_keyword(b"num")? {
                self.scan_int()?;
                if self.cur_val <= 0 {
                    return self.pdf_error(b"ext1", b"num identifier must be positive");
                }
                a.struct_id = Some(PdfId::Num(self.cur_val));
            } else {
                return self.pdf_error(b"ext1", b"identifier type missing");
            }
        }
        if self.scan_keyword(b"page")? {
            if a.kind != 1 {
                return self.pdf_error(b"ext1", b"only GoTo action can be used with `page'");
            }
            a.kind = 0;
            self.scan_int()?;
            if self.cur_val <= 0 {
                return self.pdf_error(b"ext1", b"page number must be positive");
            }
            a.id = PdfId::Num(self.cur_val);
            a.tokens = Some(self.scan_pdf_ext_toks()?);
        } else if self.scan_keyword(b"name")? {
            a.id = PdfId::Name(self.scan_pdf_ext_toks()?);
        } else if self.scan_keyword(b"num")? {
            if a.kind == 1 && a.file.is_some() {
                return self.pdf_error(
                    b"ext1",
                    b"`goto' option cannot be used with both `file' and `num'",
                );
            }
            self.scan_int()?;
            if self.cur_val <= 0 {
                return self.pdf_error(b"ext1", b"num identifier must be positive");
            }
            a.id = PdfId::Num(self.cur_val);
        } else {
            return self.pdf_error(b"ext1", b"identifier type missing");
        }
        if self.scan_keyword(b"newwindow")? {
            a.new_window = 1;
            self.scan_optional_space()?;
        } else if self.scan_keyword(b"nonewwindow")? {
            a.new_window = 2;
            self.scan_optional_space()?;
        }
        if a.new_window > 0 && (!matches!(a.kind, 0 | 1) || a.file.is_none()) {
            return self.pdf_error(
                b"ext1",
                b"`newwindow'/`nonewwindow' must be used with `goto' and `file' option",
            );
        }
        Ok(a)
    }

    /// pdfTeX §1565, §1566: an identifier `num n` or `name {...}` of a
    /// destination or thread.
    fn scan_pdf_id(&mut self) -> Result<PdfId, Jump> {
        if self.scan_keyword(b"num")? {
            self.scan_int()?;
            if self.cur_val <= 0 {
                return self.pdf_error(b"ext1", b"num identifier must be positive");
            }
            if self.cur_val > MAX_HALFWORD {
                return self.pdf_error(b"ext1", b"number too big");
            }
            Ok(PdfId::Num(self.cur_val))
        } else if self.scan_keyword(b"name")? {
            Ok(PdfId::Name(self.scan_pdf_ext_toks()?))
        } else {
            self.pdf_error(b"ext1", b"identifier type missing")
        }
    }

    /// An identifier as the object table keys it.
    pub(crate) fn obj_id(&mut self, id: &PdfId) -> Id {
        match id {
            PdfId::Num(n) => Id::Num(*n),
            PdfId::Name(t) => Id::Name(self.tokens_string(t).into()),
        }
    }

    /// pdfTeX §1537–§1599: the PDF cases of `do_extension`; false if
    /// `cur_chr` is none of them. Each reads and writes the tables of its
    /// command ([`pdf_extension_fields`]).
    pub(crate) fn do_pdf_extension(&mut self) -> Result<bool, Jump> {
        let (reads, writes) = pdf_extension_fields(self.cur_chr);
        self.writer_scope(reads, writes, Self::do_pdf_extension_now)
    }

    fn do_pdf_extension_now(&mut self) -> Result<bool, Jump> {
        match self.cur_chr {
            PDF_LITERAL_NODE => {
                // pdfTeX §1538
                self.check_pdfoutput(b"\\pdfliteral", true)?;
                self.append_whatsit(|t| {
                    let late = t.scan_keyword(b"shipout")?;
                    let mode = if t.scan_keyword(b"direct")? {
                        2
                    } else {
                        u8::from(t.scan_keyword(b"page")?)
                    };
                    let data = if late {
                        t.scan_pdf_ext_late_toks()?
                    } else {
                        t.scan_pdf_ext_toks()?
                    };
                    Ok(Some(PdfWhatsit::Literal { late, mode, data }))
                })?;
            }
            PDF_COLORSTACK_NODE => self.implement_colorstack()?,
            PDF_FONT_EXPAND_CODE => self.read_expand_font()?, // pdfTeX's `read_expand_font`
            PDF_SETMATRIX_NODE => {
                self.check_pdfoutput(b"\\pdfsetmatrix", true)?;
                self.append_whatsit(|t| {
                    Ok(Some(PdfWhatsit::SetMatrix {
                        data: t.scan_pdf_ext_toks()?,
                    }))
                })?;
            }
            PDF_SAVE_NODE => {
                self.check_pdfoutput(b"\\pdfsave", true)?;
                self.append_whatsit(|_| Ok(Some(PdfWhatsit::Save)))?;
            }
            PDF_RESTORE_NODE => {
                self.check_pdfoutput(b"\\pdfrestore", true)?;
                self.append_whatsit(|_| Ok(Some(PdfWhatsit::Restore)))?;
            }
            PDF_OBJ_CODE => self.implement_pdfobj()?,
            PDF_REFOBJ_NODE => {
                // pdfTeX §1546
                self.check_pdfoutput(b"\\pdfrefobj", true)?;
                self.scan_int()?;
                let n = self.given_obj(self.cur_val);
                self.pdf_check_obj(OBJ_TYPE_OBJ, n)?;
                self.append_whatsit(|_| Ok(Some(PdfWhatsit::RefObj { objnum: n })))?;
            }
            PDF_XFORM_CODE => self.implement_pdfxform()?,
            PDF_XIMAGE_CODE => self.implement_pdfximage()?,
            PDF_OUTLINE_CODE => self.implement_pdfoutline()?,
            PDF_REFXFORM_NODE | PDF_REFXIMAGE_NODE => {
                // pdfTeX §1549, §1554
                let form = self.cur_chr == PDF_REFXFORM_NODE;
                let name: &[u8] = if form {
                    b"\\pdfrefxform"
                } else {
                    b"\\pdfrefximage"
                };
                self.check_pdfoutput(name, true)?;
                self.scan_int()?;
                let n = self.given_obj(self.cur_val);
                self.pdf_check_obj(
                    if form {
                        OBJ_TYPE_XFORM
                    } else {
                        OBJ_TYPE_XIMAGE
                    },
                    n,
                )?;
                let (width, height, depth) = if form {
                    self.xform_dims(n)
                } else {
                    self.ximage_dims(n)
                };
                let dims = Dims {
                    width,
                    height,
                    depth,
                };
                self.append_whatsit(|_| {
                    Ok(Some(if form {
                        PdfWhatsit::RefXForm { objnum: n, dims }
                    } else {
                        PdfWhatsit::RefXImage { objnum: n, dims }
                    }))
                })?;
            }
            PDF_ANNOT_NODE => self.implement_pdfannot()?,
            PDF_START_LINK_NODE => {
                // pdfTeX §1560
                self.check_pdfoutput(b"\\pdfstartlink", true)?;
                if self.mode().abs() == VMODE {
                    return self
                        .pdf_error(b"ext1", b"\\pdfstartlink cannot be used in vertical mode")
                        .map(|()| true);
                }
                let k = self.pdf_new_objnum()?;
                self.append_whatsit(|t| {
                    let dims = t.scan_alt_rule()?;
                    let attr = if t.scan_keyword(b"attr")? {
                        Some(t.scan_pdf_ext_toks()?)
                    } else {
                        None
                    };
                    let action = Arc::new(t.scan_action()?);
                    Ok(Some(PdfWhatsit::StartLink {
                        dims,
                        attr,
                        action,
                        objnum: k,
                    }))
                })?;
                self.set_pdf_last(crate::pdf::PdfLast::Link, k);
            }
            PDF_END_LINK_NODE => {
                self.check_pdfoutput(b"\\pdfendlink", true)?;
                if self.mode().abs() == VMODE {
                    return self
                        .pdf_error(b"ext1", b"\\pdfendlink cannot be used in vertical mode")
                        .map(|()| true);
                }
                self.append_whatsit(|_| Ok(Some(PdfWhatsit::EndLink)))?;
            }
            PDF_DEST_NODE => self.implement_pdfdest()?,
            PDF_THREAD_NODE | PDF_START_THREAD_NODE => {
                // pdfTeX §1567, §1568
                let start = self.cur_chr == PDF_START_THREAD_NODE;
                let name: &[u8] = if start {
                    b"\\pdfstartthread"
                } else {
                    b"\\pdfthread"
                };
                self.check_pdfoutput(name, true)?;
                self.append_whatsit(|t| {
                    let dims = t.scan_alt_rule()?;
                    let attr = if t.scan_keyword(b"attr")? {
                        Some(t.scan_pdf_ext_toks()?)
                    } else {
                        None
                    };
                    let id = t.scan_pdf_id()?;
                    Ok(Some(PdfWhatsit::Thread {
                        start,
                        dims,
                        attr,
                        id,
                    }))
                })?;
            }
            PDF_END_THREAD_NODE => {
                self.check_pdfoutput(b"\\pdfendthread", true)?;
                self.append_whatsit(|_| Ok(Some(PdfWhatsit::EndThread)))?;
            }
            PDF_SNAP_REF_POINT_NODE => {
                self.check_pdfoutput(b"\\pdfsnaprefpoint", true)?;
                self.append_whatsit(|_| Ok(Some(PdfWhatsit::SnapRefPoint)))?;
            }
            PDF_SNAPY_NODE => {
                // pdfTeX §1573, §1574: `new_snap_node` (appended after scanning)
                self.check_pdfoutput(b"\\pdfsnapy", true)?;
                self.scan_glue(GLUE_VAL)?;
                if self.cur_glue.width < 0 {
                    return self
                        .pdf_error(b"ext1", b"negative snap glue")
                        .map(|()| true);
                }
                let glue = self.cur_glue;
                self.tail_append(Node::Whatsit(Box::new(Whatsit::Pdf(Box::new(
                    PdfWhatsit::SnapY {
                        glue,
                        final_skip: 0,
                    },
                )))));
            }
            PDF_SNAPY_COMP_NODE => {
                self.check_pdfoutput(b"\\pdfsnapycomp", true)?;
                self.append_whatsit(|t| {
                    t.scan_int()?;
                    Ok(Some(PdfWhatsit::SnapYComp {
                        ratio: t.cur_val.clamp(0, 1000),
                    }))
                })?;
            }
            PDF_SAVE_POS_NODE => {
                // pdfTeX §1576 (allowed in DVI mode too)
                self.append_whatsit(|_| Ok(Some(PdfWhatsit::SavePos)))?;
            }
            PDF_INFO_CODE | PDF_TRAILER_CODE | PDF_TRAILER_ID_CODE => {
                // pdfTeX §1578, §1581, §1582
                let c = self.cur_chr;
                let name: &[u8] = match c {
                    PDF_INFO_CODE => b"\\pdfinfo",
                    PDF_TRAILER_CODE => b"\\pdftrailer",
                    _ => b"\\pdftrailerid",
                };
                self.check_pdfoutput(name, false)?;
                let t = self.scan_pdf_ext_toks()?;
                if self.int_par(PDF_OUTPUT_CODE) > 0 {
                    let to = match c {
                        PDF_INFO_CODE => &mut self.pdf.info_toks,
                        PDF_TRAILER_CODE => &mut self.pdf.trailer_toks,
                        _ => &mut self.pdf.trailer_id_toks,
                    };
                    super::concat(to, &t);
                }
            }
            PDF_CATALOG_CODE => {
                // pdfTeX §1579
                self.check_pdfoutput(b"\\pdfcatalog", false)?;
                let t = self.scan_pdf_ext_toks()?;
                if self.int_par(PDF_OUTPUT_CODE) > 0 {
                    super::concat(&mut self.pdf.catalog_toks, &t);
                }
                if self.scan_keyword(b"openaction")? {
                    self.tracker
                        .pdf_word_access(super::word::CATALOG_OPENACTION, false);
                    if self.pdf.catalog_openaction != 0 {
                        return self
                            .pdf_error(b"ext1", b"duplicate of openaction")
                            .map(|()| true);
                    }
                    let a = self.scan_action()?;
                    let o = self.pdf_new_obj(super::objtab::OBJ_TYPE_OTHERS, 0, 1)?;
                    if self.int_par(PDF_OUTPUT_CODE) > 0 {
                        self.tracker
                            .pdf_word_access(super::word::CATALOG_OPENACTION, true);
                        self.pdf.catalog_openaction = o;
                    }
                    self.write_action(&a)?;
                    self.pdf_end_obj();
                }
            }
            PDF_NAMES_CODE => {
                // pdfTeX §1580
                self.check_pdfoutput(b"\\pdfnames", true)?;
                let t = self.scan_pdf_ext_toks()?;
                super::concat(&mut self.pdf.names_toks, &t);
            }
            PDF_FONT_ATTR_CODE => {
                // pdfTeX §1589
                self.check_pdfoutput(b"\\pdffontattr", true)?;
                self.scan_font_ident()?;
                let k = self.cur_val;
                if k == NULL_FONT {
                    return self
                        .pdf_error(b"font", b"invalid font identifier")
                        .map(|()| true);
                }
                let t = self.scan_pdf_ext_toks()?;
                let s = self.tokens_string(&t);
                self.pdf.font_attr.insert(k, s);
            }
            PDF_NOBUILTIN_TOUNICODE_CODE => {
                // pdfTeX §1593
                self.check_pdfoutput(b"\\pdfnobuiltintounicode", true)?;
                self.scan_font_ident()?;
                let k = self.cur_val;
                if k == NULL_FONT {
                    return self
                        .pdf_error(b"font", b"invalid font identifier")
                        .map(|()| true);
                }
                self.pdf.nobuiltin_tounicode.insert(k);
            }
            PDF_INTERWORD_SPACE_ON_NODE
            | PDF_INTERWORD_SPACE_OFF_NODE
            | PDF_FAKE_SPACE_NODE
            | PDF_RUNNING_LINK_OFF_NODE
            | PDF_RUNNING_LINK_ON_NODE => {
                // pdfTeX §1594–§1598
                let (name, w): (&[u8], PdfWhatsit) = match self.cur_chr {
                    PDF_INTERWORD_SPACE_ON_NODE => {
                        (b"\\pdfinterwordspaceon", PdfWhatsit::InterwordSpaceOn)
                    }
                    PDF_INTERWORD_SPACE_OFF_NODE => {
                        (b"\\pdfinterwordspaceoff", PdfWhatsit::InterwordSpaceOff)
                    }
                    PDF_FAKE_SPACE_NODE => (b"\\pdffakespace", PdfWhatsit::FakeSpace),
                    PDF_RUNNING_LINK_OFF_NODE => {
                        (b"\\pdfrunninglinkoff", PdfWhatsit::RunningLinkOff)
                    }
                    _ => (b"\\pdfrunninglinkon", PdfWhatsit::RunningLinkOn),
                };
                self.check_pdfoutput(name, true)?;
                self.append_whatsit(|_| Ok(Some(w)))?;
            }
            PDF_SPACE_FONT_CODE => {
                // pdfTeX §1599
                self.check_pdfoutput(b"\\pdfspacefont", true)?;
                let t = self.scan_pdf_ext_toks()?;
                let name = self.tokens_string(&t);
                *self.pdf.space_font_name = Some(name);
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    /// pdfTeX §1539: `\pdfcolorstack`.
    fn implement_colorstack(&mut self) -> Result<(), Jump> {
        self.check_pdfoutput(b"\\pdfcolorstack", true)?;
        self.append_whatsit(|t| {
            t.scan_int()?;
            let used = i32::try_from(t.colorstacks_used()).unwrap_or(i32::MAX);
            if t.cur_val >= used {
                t.print_err(b"Unknown color stack number ");
                t.print_int(t.cur_val);
                t.help(&[
                    b"Allocate and initialize a color stack with \\pdfcolorstackinit.",
                    b"I'll use default color stack 0 here.",
                    b"Proceed, with fingers crossed.",
                ]);
                t.error()?;
                t.cur_val = 0;
            }
            if t.cur_val < 0 {
                t.print_err(b"Invalid negative color stack number");
                t.help(&[
                    b"I'll use default color stack 0 here.",
                    b"Proceed, with fingers crossed.",
                ]);
                t.error()?;
                t.cur_val = 0;
            }
            let stack = t.cur_val;
            let cmd = if t.scan_keyword(b"set")? {
                0
            } else if t.scan_keyword(b"push")? {
                1
            } else if t.scan_keyword(b"pop")? {
                2
            } else if t.scan_keyword(b"current")? {
                3
            } else {
                t.print_err(b"Color stack action is missing");
                t.help(&[
                    b"The expected actions for \\pdfcolorstack:",
                    b"    set, push, pop, current",
                    b"I'll ignore the color stack command.",
                ]);
                t.error()?;
                return Ok(None);
            };
            let data = if cmd <= 1 {
                Some(t.scan_pdf_ext_toks()?)
            } else {
                None
            };
            Ok(Some(PdfWhatsit::ColorStack { stack, cmd, data }))
        })
    }

    /// utils.c's `colorstackused`.
    pub(crate) fn colorstacks_used(&mut self) -> usize {
        self.pdf.stacks.used()
    }

    /// pdfTeX §1544: `\pdfobj`.
    fn implement_pdfobj(&mut self) -> Result<(), Jump> {
        self.check_pdfoutput(b"\\pdfobj", true)?;
        if self.scan_keyword(b"reserveobjnum")? {
            self.scan_optional_space()?;
            self.pdf.obj_count += 1;
            let n = self.pdf.obj_count;
            let k = self.pdf_create_obj(OBJ_TYPE_OBJ, Id::Num(n))?;
            self.set_pdf_last(crate::pdf::PdfLast::Obj, k);
            return Ok(());
        }
        let mut k = -1;
        if self.scan_keyword(b"useobjnum")? {
            self.scan_int()?;
            k = self.given_obj(self.cur_val);
            if k <= 0 || !self.obj_in_range(k) || self.pdf.objs.get(k).aux != Aux::None {
                self.pdf_warning(
                    b"\\pdfobj",
                    b"invalid object number being ignored",
                    true,
                    true,
                );
                self.set_pdf_last(crate::pdf::PdfLast::Retval, -1);
                k = -1;
            }
        }
        if k < 0 {
            self.pdf.obj_count += 1;
            let n = self.pdf.obj_count;
            k = self.pdf_create_obj(OBJ_TYPE_OBJ, Id::Num(n))?;
        }
        // (`obj_data_ptr(k)` is set before the text is scanned)
        self.pdf.objs.get_mut(k).aux = Aux::Int(1);
        let mut o = RawObj {
            data: partex_engine::node::Tokens::default(),
            is_stream: false,
            stream_attr: None,
            is_file: false,
        };
        if self.scan_keyword(b"stream")? {
            o.is_stream = true;
            if self.scan_keyword(b"attr")? {
                o.stream_attr = Some(self.scan_pdf_ext_toks()?);
            }
        }
        o.is_file = self.scan_keyword(b"file")?;
        o.data = self.scan_pdf_ext_toks()?;
        self.pdf.objs.get_mut(k).aux = Aux::Obj(Box::new(o));
        self.set_pdf_last(crate::pdf::PdfLast::Obj, k);
        Ok(())
    }

    /// pdfTeX §1548: `\pdfxform`.
    fn implement_pdfxform(&mut self) -> Result<(), Jump> {
        self.check_pdfoutput(b"\\pdfxform", true)?;
        self.pdf.xform_count += 1;
        let n = self.pdf.xform_count;
        let k = self.pdf_create_obj(OBJ_TYPE_XFORM, Id::Num(n))?;
        self.pdf.objs.get_mut(k).aux = Aux::Int(1);
        let attr = if self.scan_keyword(b"attr")? {
            Some(self.scan_pdf_ext_toks()?)
        } else {
            None
        };
        let resources = if self.scan_keyword(b"resources")? {
            Some(self.scan_pdf_ext_toks()?)
        } else {
            None
        };
        self.scan_register_num()?;
        let Some(b) = self.take_box(self.cur_val) else {
            return self.pdf_error(b"ext1", b"\\pdfxform cannot be used with a void box");
        };
        let b = Arc::unwrap_or_clone(b);
        self.pdf.objs.get_mut(k).aux = Aux::XForm(Box::new(XForm {
            width: b.width,
            height: b.height,
            depth: b.depth,
            boxed: Some(b),
            attr,
            resources,
        }));
        self.set_pdf_last(crate::pdf::PdfLast::XForm, k);
        Ok(())
    }

    /// pdfTeX §1558: `\pdfannot`.
    fn implement_pdfannot(&mut self) -> Result<(), Jump> {
        self.check_pdfoutput(b"\\pdfannot", true)?;
        if self.scan_keyword(b"reserveobjnum")? {
            let k = self.pdf_new_objnum()?;
            self.set_pdf_last(crate::pdf::PdfLast::Annot, k);
            return self.scan_optional_space();
        }
        let k = if self.scan_keyword(b"useobjnum")? {
            self.scan_int()?;
            let k = self.given_obj(self.cur_val);
            if k <= 0 || !self.obj_in_range(k) || self.pdf.objs.get(k).aux != Aux::None {
                return self.pdf_error(b"ext1", b"invalid object number");
            }
            k
        } else {
            self.pdf_new_objnum()?
        };
        self.append_whatsit(|t| {
            let dims = t.scan_alt_rule()?;
            let data = t.scan_pdf_ext_toks()?;
            Ok(Some(PdfWhatsit::Annot {
                dims,
                data,
                objnum: k,
            }))
        })?;
        self.set_pdf_last(crate::pdf::PdfLast::Annot, k);
        Ok(())
    }

    /// pdfTeX §1565: `\pdfdest`.
    fn implement_pdfdest(&mut self) -> Result<(), Jump> {
        self.check_pdfoutput(b"\\pdfdest", true)?;
        let mut dup = None;
        self.append_whatsit(|t| {
            let (struct_num, j) = if t.scan_keyword(b"struct")? {
                t.scan_int()?;
                if t.cur_val <= 0 {
                    return t.pdf_error(b"ext1", b"struct identifier must be positive");
                }
                (Some(t.cur_val), OBJ_TYPE_STRUCT_DEST)
            } else {
                (None, OBJ_TYPE_DEST)
            };
            let id = t.scan_pdf_id()?;
            let mut zoom = None;
            let kind = if t.scan_keyword(b"xyz")? {
                if t.scan_keyword(b"zoom")? {
                    t.scan_int()?;
                    if t.cur_val > MAX_HALFWORD {
                        return t.pdf_error(b"ext1", b"number too big");
                    }
                    zoom = Some(t.cur_val);
                }
                0
            } else if t.scan_keyword(b"fitbh")? {
                5
            } else if t.scan_keyword(b"fitbv")? {
                6
            } else if t.scan_keyword(b"fitb")? {
                4
            } else if t.scan_keyword(b"fith")? {
                2
            } else if t.scan_keyword(b"fitv")? {
                3
            } else if t.scan_keyword(b"fitr")? {
                7
            } else if t.scan_keyword(b"fit")? {
                1
            } else {
                return t.pdf_error(b"ext1", b"destination type missing");
            };
            t.scan_optional_space()?;
            let dims = if kind == 7 {
                t.scan_alt_rule()?
            } else {
                running()
            };
            let key = t.obj_id(&id);
            let k = t.pdf.objs.find(j, &key);
            if k != 0 && t.pdf.objs.get(k).aux != Aux::None {
                dup = Some(id);
                return Ok(None);
            }
            Ok(Some(PdfWhatsit::Dest {
                dims,
                struct_num,
                id,
                kind,
                zoom,
            }))
        })?;
        if let Some(id) = dup {
            self.warn_dest_dup(&id, b"ext4", b"has been already used, duplicate ignored");
        }
        Ok(())
    }

    /// pdfTeX §1564: `warn_dest_dup`.
    pub(crate) fn warn_dest_dup(&mut self, id: &PdfId, s1: &[u8], s2: &[u8]) {
        if self.int_par(PDF_SUPPRESS_WARNING_DUP_DEST_CODE) > 0 {
            return;
        }
        self.pdf_warning(s1, b"destination with the same identifier (", true, false);
        match id {
            PdfId::Name(t) => {
                self.print_str(b"name");
                self.print_mark(t);
            }
            PdfId::Num(n) => {
                self.print_str(b"num");
                self.print_int(*n);
            }
        }
        self.print_str(b") ");
        self.print_str(s2);
        self.print_ln();
        self.show_context();
    }
}
