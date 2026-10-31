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
    pub(crate) chunks: Vec<Chunk>,
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
    pub(crate) st: PdfState,
    pub(crate) root: usize,
    /// Its objects' bytes, a chunk like a step's (shared: a tail kept
    /// for an equal state is the same chunk).
    pub(crate) chunk: Chunk,
    /// What an update section's trailer says as the exact file's does
    /// ([`crate::Updater`]): `/Info`'s number, `\pdftrailer`'s entries
    /// and `/ID`.
    pub(crate) info: Option<usize>,
    pub(crate) extra: Arc<[u8]>,
    pub(crate) id: Arc<[u8]>,
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
        self.link_bytes(bytes)
    }

    fn link_bytes(&self, bytes: &PdfBytes) -> Result<Vec<u8>, PdfError> {
        let t0 = std::time::Instant::now();
        let mut st = self.st.clone();
        let t_clone = t0.elapsed();
        let tail = &self.chunk.bytes;
        let tail_objs = &self.chunk.objs;
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
        out.extend_from_slice(tail);
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
        // (a number defined twice is a broken file whatever reader takes
        // it: every file the link writes is checked in debug builds)
        debug_assert!(
            duplicate_objects(&out).is_empty(),
            "the PDF defines objects {:?} more than once",
            duplicate_objects(&out)
        );
        Ok(out)
    }
}

/// The object numbers `pdf` defines more than once, at the top level or
/// in an object stream (PDF 32000-1 §7.5.3, §7.5.7: a number names one
/// object). The file is walked object after object from its header,
/// each stream skipped by its direct `/Length`; a file it cannot walk
/// (an indirect length, bytes it does not know) gives none.
#[must_use]
pub fn duplicate_objects(pdf: &[u8]) -> Vec<usize> {
    let mut seen = std::collections::BTreeMap::<usize, u32>::new();
    let mut i = 0;
    loop {
        while pdf.get(i).copied().is_some_and(pdf_ws) {
            i += 1;
        }
        match pdf.get(i) {
            // (the header and the comment lines after it)
            Some(b'%') => match find(pdf, i, b"\n") {
                Some(k) => i = k + 1,
                None => return Vec::new(),
            },
            Some(c) if c.is_ascii_digit() => {
                let Some((n, next)) = walk_object(pdf, i, &mut seen) else {
                    return Vec::new();
                };
                *seen.entry(n).or_default() += 1;
                i = next;
            }
            // (`xref`, `trailer`, `startxref`: the objects are over)
            _ => break,
        }
    }
    seen.into_iter()
        .filter(|&(_, k)| k > 1)
        .map(|(n, _)| n)
        .collect()
}

fn pdf_ws(c: u8) -> bool {
    matches!(c, b' ' | b'\n' | b'\r' | b'\t' | b'\x0c' | 0)
}

fn find(pdf: &[u8], from: usize, pat: &[u8]) -> Option<usize> {
    pdf.get(from..)?
        .windows(pat.len())
        .position(|w| w == pat)
        .map(|k| from + k)
}

/// The direct integer after `key` in `dict` (none: absent, or an
/// indirect reference).
fn key_int(dict: &[u8], key: &[u8]) -> Option<usize> {
    let k = dict.windows(key.len()).position(|w| w == key)? + key.len();
    let t = dict[k..].iter().position(|&c| !pdf_ws(c))? + k;
    let e = dict[t..]
        .iter()
        .position(|c| !c.is_ascii_digit())
        .map_or(dict.len(), |x| x + t);
    let n = std::str::from_utf8(&dict[t..e]).ok()?.parse().ok()?;
    let rest = &dict[e..];
    let r = rest.iter().position(|&c| !pdf_ws(c)).unwrap_or(rest.len());
    if rest.get(r).is_some_and(u8::is_ascii_digit) {
        return None;
    }
    Some(n)
}

/// The object beginning at `i` (`N G obj`): its number and where the
/// next one may begin; an object stream's members counted in `seen`.
fn walk_object(
    pdf: &[u8],
    i: usize,
    seen: &mut std::collections::BTreeMap<usize, u32>,
) -> Option<(usize, usize)> {
    let j = i + pdf[i..].iter().position(|c| !c.is_ascii_digit())?;
    let n: usize = std::str::from_utf8(&pdf[i..j]).ok()?.parse().ok()?;
    let h = find(pdf, j, b"obj").filter(|&h| h <= j + 8)?;
    let body = h + 3;
    let end = find(pdf, body, b"endobj")?;
    let Some(s) = find(pdf, body, b"stream").filter(|&s| s < end) else {
        return Some((n, end + 6));
    };
    let dict = &pdf[body..s];
    let len = key_int(dict, b"/Length")?;
    let mut d0 = s + 6;
    if pdf.get(d0) == Some(&b'\r') {
        d0 += 1;
    }
    if pdf.get(d0) == Some(&b'\n') {
        d0 += 1;
    }
    let data = pdf.get(d0..d0 + len)?;
    if dict.windows(7).any(|w| w == b"/ObjStm") {
        let (k, first) = (key_int(dict, b"/N")?, key_int(dict, b"/First")?);
        let dec = if dict.windows(12).any(|w| w == b"/FlateDecode") {
            phitex_codecs::inflate::inflate(data)?
        } else {
            data.to_vec()
        };
        let head = std::str::from_utf8(dec.get(..first)?).ok()?;
        let nums: Vec<usize> = head
            .split_ascii_whitespace()
            .filter_map(|t| t.parse().ok())
            .collect();
        for pair in nums.chunks(2).take(k) {
            *seen.entry(pair[0]).or_default() += 1;
        }
    }
    Some((n, find(pdf, d0 + len, b"endobj")? + 6))
}

/// A hash of bytes, a word at a time.
pub(crate) fn hash64(b: &[u8]) -> u64 {
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
        self.page_shown = (crate::Shown::default(), 0, 0);
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

    /// Object streams left to the caller to compress, on or off: each
    /// one's bytes up to its `/Length` a chunk, the stream a
    /// [`Part::Content`] ([`PdfState::take_parts`]), compressed where the
    /// caller has threads (a link's records: the streams of many pages
    /// compressed side by side, not one after another).
    pub fn defer_streams(&mut self, on: bool) {
        self.defer = on;
    }

    /// The parts written since the last take: the object streams left to
    /// compress and the bytes around them, in order.
    pub fn take_parts(&mut self, parts: &mut Vec<crate::Part>) {
        parts.append(&mut self.pending);
        if let Some(c) = self.flush_chunk() {
            parts.push(crate::Part::Bytes(c));
        }
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
        let (root, info) = self.tail_objects()?;
        if self.os.enable {
            self.os_flush();
        }
        if let Some(e) = self.error.take() {
            return Err(e);
        }
        let chunk = Chunk {
            bytes: std::mem::take(&mut self.out).into(),
            objs: std::mem::take(&mut self.chunk_objs),
        };
        // (the trailer's entries as `xref` prints them: `/Info` the last
        // object before the cross-reference section's)
        let info = info.map(|_| self.sys_obj_ptr());
        // (`print_toks_ln`'s bytes, the state left as it is)
        let extra = match &self.trailer_toks {
            Some(t) if !t.is_empty() => [&t[..], b"\n"].concat().into(),
            _ => Arc::from(&b""[..]),
        };
        let id = self.id_bytes().into();
        Ok(Some(Tail {
            st: self,
            root,
            chunk,
            info,
            extra,
            id,
        }))
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
        let id = self.id_bytes();
        self.print(&id);
    }

    /// What [`PdfState::print_id`] prints.
    fn id_bytes(&self) -> Vec<u8> {
        if let Some(t) = &self.trailer_id_toks {
            let s = t.split(|&c| c == 0).next().unwrap_or_default();
            if s.is_empty() {
                Vec::new()
            } else {
                let id = md5_hex(s);
                format!("/ID [<{id}> <{id}>]").into_bytes()
            }
        } else {
            let mut m = self.p.date.clone().into_bytes();
            m.extend_from_slice(self.p.output_file_name.as_bytes());
            let id = md5_hex(&m);
            format!("/ID [<{id}> <{id}>]").into_bytes()
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

#[cfg(test)]
mod tests {
    use super::duplicate_objects;

    #[test]
    fn a_number_defined_twice_is_found_in_object_streams_too() {
        let os_head = b"3 0 4 9 ";
        let os_body = b"<< /A 1 >><< /B 2 >>";
        let mut os = os_head.to_vec();
        os.extend_from_slice(os_body);
        let mut pdf = b"%PDF-1.5\n%\xe2\xe3\xcf\xd3\n1 0 obj\n<< /Length 4 >>\nstream\nabcd\nendstream\nendobj\n2 0 obj\n[1 0 R]\nendobj\n".to_vec();
        pdf.extend_from_slice(
            format!(
                "5 0 obj\n<< /Type /ObjStm /N 2 /First {} /Length {} >>\nstream\n",
                os_head.len(),
                os.len()
            )
            .as_bytes(),
        );
        pdf.extend_from_slice(&os);
        pdf.extend_from_slice(b"\nendstream\nendobj\n");
        assert_eq!(duplicate_objects(&pdf), Vec::<usize>::new());
        let mut twice = pdf.clone();
        twice.extend_from_slice(b"4 0 obj\n<< /Type /Metadata /Length 2 >>\nstream\nxy\nendstream\nendobj\n2 0 obj\n[]\nendobj\n");
        assert_eq!(duplicate_objects(&twice), vec![2, 4]);
        twice.extend_from_slice(b"xref\n0 1\ntrailer\n<< >>\n");
        assert_eq!(duplicate_objects(&twice), vec![2, 4]);
    }
}
