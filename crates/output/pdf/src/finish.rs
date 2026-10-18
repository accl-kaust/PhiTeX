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
    /// in `bytes`, in order, with their offsets in `bytes`.
    pub objs: Vec<(usize, usize)>,
}

/// The file's bytes so far, append-only: one [`Chunk`] per non-empty
/// [`PdfState::flush`]. The state never reads them back nor knows their
/// lengths: an object's offset is kept within its chunk, by the chunk's
/// ordinal, so a state does not change when an earlier chunk's bytes do
/// (an edited page re-rendered: the steps after it see an equal state).
/// [`finish`] places the chunks by the prefix sums of their lengths.
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

    /// `c` appended (a chunk a step flushed, kept apart and assembled
    /// in order by the caller).
    pub fn push(&mut self, c: Chunk) {
        self.len += c.bytes.len();
        self.chunks.push(c);
    }

    /// The chunk that object `n` begins in.
    #[must_use]
    pub fn chunk_of(&self, n: usize) -> Option<usize> {
        self.chunks
            .iter()
            .position(|c| c.objs.iter().any(|&(m, _)| m == n))
    }
}

/// The whole file: the bytes flushed, then the end of the file (fonts,
/// pages tree, outlines, names, threads, catalog, `/Info`, the
/// cross-reference table or stream and the trailer); empty without pages
/// (or with `\pdfdraftmode`). [`PdfState::tail`] then [`Tail::link`].
///
/// # Errors
/// A font that cannot be embedded, or what pdfTeX stops at; `bytes` not
/// the ones `state` flushed.
pub fn finish(state: PdfState, bytes: &PdfBytes) -> Result<Vec<u8>, PdfError> {
    match state.tail()? {
        Some(t) => t.link(bytes),
        None => Ok(Vec::new()),
    }
}

/// The objects at the end of the file written as one more chunk, with
/// offsets within it like any step's: a pure function of the state
/// (whatever the chunks before it are), so a caller can keep it for an
/// equal state. [`Tail::link`] places the chunks and writes the
/// cross-reference section and the trailer.
#[derive(Clone, Debug)]
pub struct Tail {
    st: PdfState,
    root: usize,
}

impl Tail {
    /// The file: `bytes` (the chunks the state flushed, in order) and
    /// the tail's chunk copied, the objects' offsets the chunks' prefix
    /// sums plus their offsets within them, then the cross-reference
    /// table or stream and the trailer.
    ///
    /// # Errors
    /// `bytes` not the ones the state flushed.
    pub fn link(&self, bytes: &PdfBytes) -> Result<Vec<u8>, PdfError> {
        self.link_hashed(bytes).map(|x| x.0)
    }

    /// [`Tail::link`] with a hash of the file's bytes, made from its
    /// chunks' hashes (each chunk's hashed once, while a link keeps it:
    /// an edit hashes its new chunks only).
    ///
    /// # Errors
    /// As [`Tail::link`].
    pub fn link_hashed(&self, bytes: &PdfBytes) -> Result<(Vec<u8>, u64), PdfError> {
        let out = self.link_bytes(bytes)?;
        let t0 = std::time::Instant::now();
        let h = chunk_hashes(&bytes.chunks, &out[bytes.len..]);
        if crate::phase::on() {
            eprintln!("pdf tail link: hashed {:?}", t0.elapsed());
        }
        Ok((out, h))
    }

    fn link_bytes(&self, bytes: &PdfBytes) -> Result<Vec<u8>, PdfError> {
        let t0 = std::time::Instant::now();
        let mut st = self.st.clone();
        let t_clone = t0.elapsed();
        let tail = std::mem::take(&mut st.out);
        let tail_objs = std::mem::take(&mut st.chunk_objs);
        if bytes.chunks.len() != st.flushes || st.base != 0 {
            return Err(PdfError::Unsupported(
                "finish: the bytes are not the ones the state flushed".into(),
            ));
        }
        // the objects' offsets: each chunk's offsets of its objects plus
        // the chunk's place, the prefix sum of the lengths before it (a
        // chunk copied, not written again)
        let mut sum = 0usize;
        for (c, objs) in bytes
            .chunks
            .iter()
            .map(|c| (c.bytes.len(), c.objs.as_slice()))
            .chain(std::iter::once((tail.len(), tail_objs.as_slice())))
        {
            for &(n, at) in objs {
                if n < st.objs.len() {
                    st.objs[n].offset = int(sum + at);
                }
            }
            sum += c;
        }
        // the cross-reference section written after the tail's objects:
        // its offsets are the file's
        st.base = sum;
        st.linking = true;
        let t_off = t0.elapsed();
        let end = st.xref(self.root)?;
        let t_xref = t0.elapsed();
        let mut out = Vec::with_capacity(st.base + end.len());
        for c in &bytes.chunks {
            out.extend_from_slice(&c.bytes);
        }
        out.extend_from_slice(&tail);
        out.extend_from_slice(&end);
        if crate::phase::on() {
            eprintln!(
                "pdf tail link: state copied {t_clone:?}, offsets {:?}, xref {:?}, bytes {:?} ({} chunks, {} objects)",
                t_off.saturating_sub(t_clone),
                t_xref.saturating_sub(t_off),
                t0.elapsed().saturating_sub(t_xref),
                bytes.chunks.len(),
                st.objs.len()
            );
        }
        Ok(out)
    }
}

/// A hash of bytes, a word at a time.
fn hash64(b: &[u8]) -> u64 {
    let mut h = 0x243f_6a88_85a3_08d3u64 ^ b.len() as u64;
    let (words, rest) = b.as_chunks::<8>();
    for w in words {
        h = (h ^ u64::from_le_bytes(*w))
            .wrapping_mul(0x9e37_79b9_7f4a_7c15)
            .rotate_left(29);
    }
    for &c in rest {
        h = (h ^ u64::from(c)).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    }
    h
}

/// The file's hash from its chunks' and the rest's (`tail`): each chunk's
/// hash kept by its bytes' address while a link has it.
fn chunk_hashes(chunks: &[Chunk], tail: &[u8]) -> u64 {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock, PoisonError};
    type Memo = Mutex<HashMap<usize, (Arc<[u8]>, u64)>>;
    static MEMO: OnceLock<Memo> = OnceLock::new();
    let mut m = MEMO
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let mut kept = HashMap::with_capacity(chunks.len());
    let mut hs = Vec::with_capacity(chunks.len() + 1);
    for c in chunks {
        let k = c.bytes.as_ptr() as usize;
        let h = match m.remove(&k) {
            Some((a, h)) if Arc::ptr_eq(&a, &c.bytes) => h,
            _ => hash64(&c.bytes),
        };
        hs.push(h);
        kept.insert(k, (c.bytes.clone(), h));
    }
    *m = kept;
    hs.push(hash64(tail));
    let words: Vec<u8> = hs.iter().flat_map(|h| h.to_le_bytes()).collect();
    hash64(&words)
}

impl PdfState {
    /// Moves the bytes written since the last flush to `bytes` as one
    /// chunk (possibly empty): a step's end.
    ///
    /// # Panics
    /// If `bytes` are not the ones this state flushed before.
    pub fn flush(&mut self, bytes: &mut PdfBytes) {
        assert_eq!(
            bytes.chunks.len(),
            self.flushes,
            "flush: not this state's bytes"
        );
        if let Some(c) = self.flush_chunk() {
            bytes.push(c);
        }
    }

    /// The bytes written since the last flush as the next chunk, `None`
    /// if there are none (no chunk counted): a step's end, its bytes
    /// kept by the caller, who gives [`finish`] every chunk in order.
    pub fn flush_chunk(&mut self) -> Option<Chunk> {
        // (the positions within the chunk a step's stream used, and the
        // page it showed, are the step's: not the state after it)
        self.stream_length_offset = 0;
        self.save_offset = 0;
        self.page_shown = (Vec::new(), 0, 0);
        if self.out.is_empty() {
            return None;
        }
        let b = std::mem::take(&mut self.out);
        self.flushes += 1;
        Some(Chunk {
            bytes: b.into(),
            objs: std::mem::take(&mut self.chunk_objs),
        })
    }

    /// The number of chunks flushed.
    #[must_use]
    pub fn flushed(&self) -> usize {
        self.flushes
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

    /// The objects at the end of the file, written after the chunks
    /// flushed (fonts, the pages tree, outlines, names, threads, the
    /// catalog, `/Info`, the last object stream): `None` for no file.
    ///
    /// # Errors
    /// A font that cannot be embedded, or what pdfTeX stops at.
    pub fn tail(mut self) -> Result<Option<Tail>, PdfError> {
        if self.total_pages == 0 || self.fixed_draftmode != 0 {
            return Ok(None);
        }
        let (root, _) = self.tail_objects()?;
        if self.os.enable {
            self.os_flush();
        }
        if let Some(e) = self.error.take() {
            return Err(e);
        }
        Ok(Some(Tail { st: self, root }))
    }

    /// The cross-reference table or stream, the trailer and `startxref`,
    /// the objects' offsets the file's.
    fn xref(&mut self, root: usize) -> Result<Vec<u8>, PdfError> {
        if self.os.enable {
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
            None => Ok(std::mem::take(&mut self.out)),
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
        let t0 = std::time::Instant::now();
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
        let t_head = t0.elapsed();
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
        let t_rows = t0.elapsed();
        let n = self.zip.as_ref().map_or(0, Vec::len);
        self.end_stream_with(crate::compress_resumed);
        if crate::phase::on() {
            eprintln!(
                "pdf xref stream: head {t_head:?}, rows {:?}, compressed {:?} ({n} bytes, level {})",
                t_rows.saturating_sub(t_head),
                t0.elapsed().saturating_sub(t_rows),
                self.p.compress_level
            );
        }
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
