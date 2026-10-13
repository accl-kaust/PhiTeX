//! The end of the file (pdfTeX §794–§815): fonts, the page tree,
//! outlines, the name tree, threads, the catalog, `/Info`, and the
//! cross-reference table or stream with the trailer.

use std::sync::Arc;

use crate::{
    LiteralMode, OBJ_TYPE_OTHERS, OBJ_TYPE_PAGE, OBJ_TYPE_PAGES, PAGES_TREE_KIDS_MAX, PdfError,
    PdfState, int, pdf_error, substr_of_str,
};

/// The hexadecimal MD5 of `s` (`convertStringToHexString`).
fn md5_hex(s: &[u8]) -> String {
    use std::fmt::Write;
    phitex_codecs::md5::md5(s)
        .iter()
        .fold(String::new(), |mut o, b| {
            let _ = write!(o, "{b:02X}");
            o
        })
}

/// A step's bytes: what it appended to the file and the objects (by
/// number) that begin in them.
#[derive(Clone, Debug)]
pub struct Chunk {
    /// The bytes.
    pub bytes: Arc<[u8]>,
    /// The objects written directly (not in an object stream) that begin
    /// in `bytes`, in order.
    pub objs: Vec<usize>,
}

/// The file's bytes so far, append-only: one [`Chunk`] per
/// [`PdfState::flush`]. The state never reads them back; it only counts
/// them, so a state cloned after step `k` continues from these bytes
/// truncated to `k` chunks.
#[derive(Clone, Debug, Default)]
pub struct PdfBytes {
    chunks: Vec<Chunk>,
    len: usize,
}

impl PdfBytes {
    /// No bytes.
    #[must_use]
    pub fn new() -> PdfBytes {
        PdfBytes::default()
    }

    /// The chunks, one per step.
    #[must_use]
    pub fn chunks(&self) -> &[Chunk] {
        &self.chunks
    }

    /// The number of bytes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether there are no bytes.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The first `steps` chunks only (to continue from a state cloned
    /// after that many flushes).
    pub fn truncate(&mut self, steps: usize) {
        self.chunks.truncate(steps);
        self.len = self.chunks.iter().map(|c| c.bytes.len()).sum();
    }

    /// The chunk that object `n` begins in.
    #[must_use]
    pub fn chunk_of(&self, n: usize) -> Option<usize> {
        self.chunks.iter().position(|c| c.objs.contains(&n))
    }
}

/// The whole file: the bytes flushed, then the end of the file (fonts,
/// pages tree, outlines, names, threads, catalog, `/Info`, the
/// cross-reference table or stream and the trailer); empty without pages
/// (or with `\pdfdraftmode`).
///
/// # Errors
/// A font that cannot be embedded, or what pdfTeX stops at; `bytes` not
/// the ones `state` flushed.
pub fn finish(state: PdfState, bytes: &PdfBytes) -> Result<Vec<u8>, PdfError> {
    if bytes.len() != state.base {
        return Err(PdfError::Unsupported(
            "finish: the bytes are not the ones the state flushed".into(),
        ));
    }
    let Some(tail) = state.finish_tail()? else {
        return Ok(Vec::new());
    };
    let mut out = Vec::with_capacity(bytes.len() + tail.len());
    for c in &bytes.chunks {
        out.extend_from_slice(&c.bytes);
    }
    out.extend_from_slice(&tail);
    Ok(out)
}

impl PdfState {
    /// Moves the bytes written since the last flush to `bytes` as one
    /// chunk (possibly empty): a step's end.
    ///
    /// # Panics
    /// If `bytes` are not the ones this state flushed before.
    pub fn flush(&mut self, bytes: &mut PdfBytes) {
        assert_eq!(bytes.len, self.base, "flush: not this state's bytes");
        let b = std::mem::take(&mut self.out);
        self.base += b.len();
        bytes.len += b.len();
        bytes.chunks.push(Chunk {
            bytes: b.into(),
            objs: std::mem::take(&mut self.chunk_objs),
        });
    }

    /// The number of bytes flushed.
    #[must_use]
    pub fn flushed(&self) -> usize {
        self.base
    }

    /// `\pdfcolorstackinit [page] [direct] {...}`: the new stack's
    /// number (`\pdfcolorstackinit` expands to it).
    ///
    /// # Errors
    /// "Too many color stacks".
    pub fn colorstack_init(
        &mut self,
        page: bool,
        direct: bool,
        init: &[u8],
    ) -> Result<usize, PdfError> {
        let mode = if direct {
            LiteralMode::DirectAlways
        } else {
            LiteralMode::SetOrigin
        };
        self.stacks
            .new_stack(init, mode, page)
            .ok_or_else(|| pdf_error("", "Too many color stacks"))
    }

    /// The rest of the file: fonts, the pages tree, outlines, names,
    /// threads, the catalog, the `/Info` dictionary, the cross-reference
    /// table or stream and the trailer; no file at all without pages (or
    /// with `\pdfdraftmode`).
    ///
    /// The writer never flushed (see [`PdfState::flush`]): the whole
    /// file is `finish(self, &PdfBytes::new())`.
    ///
    /// # Errors
    /// A font that cannot be embedded, or what pdfTeX stops at.
    pub fn finish(self) -> Result<Vec<u8>, PdfError> {
        finish(self, &PdfBytes::new())
    }

    /// The end of the file after the bytes flushed: `None` for no file.
    fn finish_tail(mut self) -> Result<Option<Vec<u8>>, PdfError> {
        if self.total_pages == 0 || self.fixed_draftmode != 0 {
            return Ok(None);
        }
        let (root, _) = self.tail_objects()?;
        if self.os.enable {
            self.os_flush();
            self.output_xref_stream(root);
        } else {
            self.output_obj_tab();
            // the trailer
            self.print_ln(b"trailer");
            self.print(b"<< ");
            self.int_entry_ln(b"Size", int(self.sys_obj_ptr()) + 1);
            self.indirect_ln(b"Root", root);
            if self.p.omit_info_dict == 0 {
                self.indirect_ln(b"Info", self.sys_obj_ptr());
            }
            if let Some(t) = self.trailer_toks.take() {
                self.print_toks_ln(&t);
            }
            self.print_id();
            self.print_ln(b" >>");
        }
        self.print_ln(b"startxref");
        if self.os.enable {
            let n = self.objs[self.sys_obj_ptr()].offset;
            self.print_int_ln(n);
        } else {
            self.print_int_ln(int(self.save_offset));
        }
        self.print_ln(b"%%EOF");
        match self.error.take() {
            Some(e) => Err(e),
            None => Ok(Some(self.out)),
        }
    }

    /// The objects at the end of the file (pdfTeX §794 ff.): the fonts,
    /// the pages tree, outlines, names, threads, the catalog and `/Info`;
    /// the catalog's number and `/Info`'s.
    pub(crate) fn tail_objects(&mut self) -> Result<(usize, Option<usize>), PdfError> {
        if self.total_pages % PAGES_TREE_KIDS_MAX != 0 {
            self.objs[self.last_pages].info = self.total_pages % PAGES_TREE_KIDS_MAX;
        }
        // Check for non-existing pages
        let mut k = self.head[OBJ_TYPE_PAGE];
        while !self.aux.pages_made.contains(&k) {
            k = self.objs[k].link;
        }
        self.head[OBJ_TYPE_PAGE] = k;
        // Reverse the linked list of Page and Pages objects
        let mut l = 0;
        while k != 0 {
            let i = self.objs[k].link;
            self.objs[k].link = l;
            l = k;
            k = i;
        }
        self.head[OBJ_TYPE_PAGE] = l;
        let mut k = self.head[OBJ_TYPE_PAGES];
        let pages_tail = k;
        let mut l = 0;
        while k != 0 {
            let i = self.objs[k].link;
            self.objs[k].link = l;
            l = k;
            k = i;
        }
        self.head[OBJ_TYPE_PAGES] = l;
        self.fix_dests();
        self.output_fonts()?;
        self.output_pages_tree(pages_tail);
        let outlines = self.output_outlines();
        let names = self.output_name_tree();
        let threads = self.output_threads();
        // the catalog
        let root = self.new_dict(OBJ_TYPE_OTHERS, 0, 1);
        self.print_ln(b"/Type /Catalog");
        self.indirect_ln(b"Pages", self.last_pages);
        if threads != 0 {
            self.indirect_ln(b"Threads", threads);
        }
        if outlines != 0 {
            self.indirect_ln(b"Outlines", outlines);
        }
        if names != 0 {
            self.indirect_ln(b"Names", names);
        }
        if let Some(t) = self.catalog_toks.take() {
            self.print_toks_ln(&t);
        }
        if self.catalog_openaction != 0 {
            self.indirect_ln(b"OpenAction", self.catalog_openaction);
        }
        self.end_dict();
        let mut info = None;
        if self.p.omit_info_dict == 0 {
            self.print_info();
            info = Some(self.sys_obj_ptr());
        }
        match self.error.take() {
            Some(e) => Err(e),
            None => Ok((root, info)),
        }
    }

    /// The trailer's `/ID` entry (`print_ID`).
    pub(crate) fn id_entry(&mut self) -> Vec<u8> {
        let at = self.out.len();
        self.print_id();
        self.out.split_off(at)
    }

    /// `print_ID` or `print_ID_alt`.
    fn print_id(&mut self) {
        if let Some(t) = self.trailer_id_toks.take() {
            let s = t.split(|&c| c == 0).next().unwrap_or_default();
            if !s.is_empty() {
                let id = md5_hex(s);
                self.print(format!("/ID [<{id}> <{id}>]").as_bytes());
            }
        } else {
            let mut m = self.p.date.clone().into_bytes();
            m.extend_from_slice(self.p.output_file_name.as_bytes());
            let id = md5_hex(&m);
            self.print(format!("/ID [<{id}> <{id}>]").as_bytes());
        }
    }

    /// `pdf_print_info`.
    fn print_info(&mut self) {
        self.new_dict(OBJ_TYPE_OTHERS, 0, 3);
        let s = self.info_toks.take();
        let given = |k: &[u8]| s.as_deref().is_some_and(|s| substr_of_str(k, s));
        let creator = given(b"/Creator");
        let producer = given(b"/Producer");
        let creation = given(b"/CreationDate");
        let moddate = given(b"/ModDate");
        let trapped = given(b"/Trapped");
        if !producer {
            self.print_ln(b"/Producer (pdfTeX-1.40.29)");
        }
        if let Some(s) = &s {
            self.print_toks_ln(s);
        }
        if !creator {
            self.print_ln(b"/Creator (TeX)");
        }
        if self.p.info_omit_date == 0 {
            let date = self.p.date.clone();
            if !creation {
                self.print(format!("/CreationDate ({date})\n").as_bytes());
            }
            if !moddate {
                self.print(format!("/ModDate ({date})\n").as_bytes());
            }
        }
        if !trapped {
            self.print_ln(b"/Trapped /False");
        }
        if self.p.suppress_ptex_info % 2 == 0 {
            let banner = self.p.banner.clone();
            if self.p.ptex_use_underscore > 0 {
                self.print(b"/PTEX_Fullbanner ");
            } else {
                self.print(b"/PTEX.Fullbanner ");
            }
            self.print_str(banner.as_bytes());
            self.pdf_out(b'\n');
        }
        self.end_dict();
    }

    /// "Build a linked list of free objects".
    fn link_free_objects(&mut self) {
        let mut l = 0;
        self.objs[0].offset = -2;
        for k in 1..=self.sys_obj_ptr() {
            if !self.is_written(k) {
                self.objs[l].link = k;
                l = k;
            }
        }
        self.objs[l].link = 0;
    }

    /// "Output the `obj_tab`".
    fn output_obj_tab(&mut self) {
        self.link_free_objects();
        self.save_offset = self.offset();
        let obj_ptr = self.obj_ptr;
        self.print_ln(b"xref");
        self.print(b"0 ");
        self.print_int_ln(int(obj_ptr) + 1);
        self.print_fw_int(int(self.objs[0].link), 10);
        self.print_ln(b" 65535 f ");
        for k in 1..=obj_ptr {
            if self.is_written(k) {
                self.print_fw_int(self.objs[k].offset, 10);
                self.print_ln(b" 00000 n ");
            } else {
                self.print_fw_int(int(self.objs[k].link), 10);
                self.print_ln(b" 00000 f ");
            }
        }
    }

    /// "Output the cross-reference stream dictionary".
    fn output_xref_stream(&mut self, root: usize) {
        let x = self.new_dict(OBJ_TYPE_OTHERS, 0, 0);
        let off = self.objs[x].offset;
        let w: usize = if off / 256 > 16_777_215 {
            5
        } else if off > 16_777_215 {
            4
        } else if off > 65535 {
            3
        } else {
            2
        };
        self.link_free_objects();
        let obj_ptr = self.obj_ptr;
        self.print_ln(b"/Type /XRef");
        self.print(b"/Index [0 ");
        self.print_int(int(obj_ptr) + 1);
        self.print_ln(b"]");
        self.int_entry_ln(b"Size", int(obj_ptr) + 1);
        self.print(b"/W [1 ");
        self.print_int(int(w));
        self.print_ln(b" 1]");
        self.indirect_ln(b"Root", root);
        if self.p.omit_info_dict == 0 {
            self.indirect_ln(b"Info", obj_ptr - 1);
        }
        if let Some(t) = self.trailer_toks.take() {
            self.print_toks_ln(&t);
        }
        self.print_id();
        self.pdf_out(b'\n');
        self.begin_stream();
        let bytes = |v: i64| -> Vec<u8> {
            let b = v.to_be_bytes();
            b[8 - w..].to_vec()
        };
        for k in 0..=self.sys_obj_ptr() {
            let o = self.objs[k];
            if !self.is_written(k) {
                self.pdf_out(0);
                self.print(&bytes(int(o.link)));
                self.pdf_out(255);
            } else if o.os_idx == -1 {
                self.pdf_out(1);
                self.print(&bytes(o.offset));
                self.pdf_out(0);
            } else {
                self.pdf_out(2);
                self.print(&bytes(o.offset));
                self.pdf_out(u8::try_from(o.os_idx).unwrap_or(0));
            }
        }
        self.end_stream();
    }

    /// "Output pages tree" (pdfTeX §802–§803).
    fn output_pages_tree(&mut self, mut pages_tail: usize) {
        let a = self.sys_obj_ptr() + 1;
        let mut l = self.head[OBJ_TYPE_PAGES];
        let mut k = self.head[OBJ_TYPE_PAGE];
        let mut b = 0usize;
        loop {
            let mut i: i64 = 0;
            let mut c = 0usize;
            let is_root = self.objs[l].link == 0;
            loop {
                if !is_root {
                    if i % PAGES_TREE_KIDS_MAX == 0 {
                        self.last_pages = self.new_objnum_();
                        if c == 0 {
                            c = self.last_pages;
                        }
                        self.objs[pages_tail].link = self.last_pages;
                        pages_tail = self.last_pages;
                        self.objs[self.last_pages].link = 0;
                        self.objs[self.last_pages].info = self.objs[l].info;
                    } else {
                        let lp = self.last_pages;
                        self.objs[lp].info += self.objs[l].info;
                    }
                }
                // §803
                self.begin_dict(l, 1);
                self.print_ln(b"/Type /Pages");
                self.int_entry_ln(b"Count", self.objs[l].info);
                if !is_root {
                    self.indirect_ln(b"Parent", self.last_pages);
                }
                self.print(b"/Kids [");
                let mut j: i64 = 0;
                loop {
                    self.print_int(int(k));
                    self.print(b" 0 R ");
                    k = self.objs[k].link;
                    j += 1;
                    if (l < a && j == self.objs[l].info)
                        || k == 0
                        || (k == b && b != 0)
                        || j == PAGES_TREE_KIDS_MAX
                    {
                        break;
                    }
                }
                self.remove_last_space();
                self.print_ln(b"]");
                if k == 0 {
                    k = self.head[OBJ_TYPE_PAGES];
                    self.head[OBJ_TYPE_PAGES] = 0;
                }
                if is_root && let Some(a) = self.p.pages_attr.clone() {
                    self.print_toks_ln(&a);
                }
                self.end_dict();
                i += 1;
                l = self.objs[l].link;
                if l == c {
                    break;
                }
            }
            b = c;
            if l == 0 {
                break;
            }
        }
    }
}
