//! pdfTeX part 32b and the object writing of 32d: the PDF output buffer,
//! objects, dictionaries, streams and object streams (pdfTeX §680–§702).
//!
//! Bytes are written the way pdfTeX's buffer writes them, so offsets (the
//! cross-reference table) come out the same. A stream's `/Length` is
//! filled in where pdfTeX seeks back to write it: the bytes from the hole
//! on are held until the stream ends.

use alloc::vec::Vec;

use crate::arith::Scaled;
use crate::host::{FileKind, Host};
use crate::tex::{Jump, Tex};
use crate::track::{Output, Tracker};
use crate::web::*;

/// `pdf_os_max_objs`: objects per object stream.
pub(crate) const PDF_OS_MAX_OBJS: i32 = 100;
/// `pdf_op_buf_size`: pdfTeX flushes its buffer (and feeds the
/// compressor) in pieces of this size.
const PDF_OP_BUF_SIZE: usize = 16384;

/// What a relocation in the bytes stands for (virtual numbers): an
/// object's number, a font's (its `/F` name's digits: the font's number
/// is the order of loading's, `FontArrays::number`), or a stream's
/// `/Length`, which the link fills when it compresses the stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Reloc {
    Obj(i32),
    Font(i32),
    Length,
}

partex_engine::persist_enum!(Reloc {
    Obj(a0),
    Font(a0),
    Length
});

/// The width of a `/Length` hole (pdfTeX's `/Length` and 10 blanks for
/// the digits after its space).
pub(crate) const LENGTH_HOLE: usize = 10;

impl Reloc {
    /// How many bytes the engine printed in its place.
    fn width(self) -> usize {
        match self {
            Reloc::Obj(k) | Reloc::Font(k) => alloc::format!("{k}").len(),
            Reloc::Length => LENGTH_HOLE,
        }
    }
}

/// pdfTeX's output state (pdfTeX §680's globals).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct PdfOut {
    /// The open file (`pdf_file`).
    pub file: Option<crate::host::WriteId>,
    /// Bytes written but not yet given to the host.
    pending: Vec<u8>,
    /// `pdf_gone`: bytes written so far.
    pub gone: i64,
    /// `pdf_op_buf` (its length is `pdf_ptr` outside object streams).
    buf: Vec<u8>,
    /// `pdf_os_buf` (its length is `pdf_ptr` inside object streams).
    os_buf: Vec<u8>,
    pub os_mode: bool,
    pub os_enable: bool,
    pub os_cur_objnum: i32,
    pub os_objidx: i32,
    os_objnum: Vec<i32>,
    os_objoff: Vec<i32>,
    /// `pdf_os_cntr` (statistics).
    pub os_cntr: i32,
    /// A stream is being compressed: its bytes so far.
    zip: Option<Vec<u8>>,
    /// `pdf_stream_length`, `pdf_stream_length_offset`,
    /// `pdf_seek_write_length`.
    stream_length: i64,
    stream_length_offset: i64,
    seek_write_length: bool,
    /// `pdf_save_offset`.
    pub save_offset: i64,
    /// `pdf_last_byte`.
    pub last_byte: u8,
    pub version_written: bool,
    pub fixed_major: i32,
    pub fixed_minor: i32,
    pub fixed_objcompresslevel: i32,
    pub fixed_gamma: i32,
    pub fixed_image_gamma: i32,
    pub fixed_image_hicolor: i32,
    pub fixed_image_apply_gamma: i32,
    pub fixed_draftmode: i32,
    pub fixed_draftmode_set: bool,
    pub fixed_inclusion_copy_font: i32,
    pub fixed_pdfoutput: i32,
    pub fixed_pdfoutput_set: bool,
    pub fixed_decimal_digits: i32,
    /// `scaled_out` of the last `divide_scaled`.
    pub scaled_out: Scaled,
    /// With effects on, object streams are symbolic: the objects' bytes
    /// go out as effects and the link lays the stream out (`effects.rs`),
    /// so what `pdf_os_buf` holds is output, not state.
    pub symbolic: bool,
    /// How much of `pdf_os_buf` went out as effects.
    os_emitted: usize,
    /// Virtual object numbers (`vnum.rs`): an object number printed is a
    /// relocation, the link's to write (`Effect::ObjRef`); object streams
    /// are the link's to fill, and not tracked here.
    pub virt: bool,
    /// The relocations not handed on yet: where in the file (counted as
    /// `gone` counts) or in `pdf_os_buf` the digits of each begin.
    refs: Vec<(i64, Reloc)>,
    os_refs: Vec<(usize, Reloc)>,
    /// With fonts as cells too (`font_refs`), a stream's bytes are given
    /// to the link to compress (`Effect::Deflate`): the relocations in
    /// the stream being made, by where they are in it.
    zrefs: Vec<(usize, Reloc)>,
    /// Fonts' `/F` names are relocations, and streams are compressed by
    /// the link (a machine's, with virtual numbers and fonts as cells).
    pub font_refs: bool,
    /// Print pdfTeX's numbers by this numbering (the job's end).
    pub forced: super::vnum::Forced,
}

partex_engine::persist_struct!(PdfOut {
    file,
    pending,
    gone,
    buf,
    os_buf,
    os_mode,
    os_enable,
    os_cur_objnum,
    os_objidx,
    os_objnum,
    os_objoff,
    os_cntr,
    zip,
    stream_length,
    stream_length_offset,
    seek_write_length,
    save_offset,
    last_byte,
    version_written,
    fixed_major,
    fixed_minor,
    fixed_objcompresslevel,
    fixed_gamma,
    fixed_image_gamma,
    fixed_image_hicolor,
    fixed_image_apply_gamma,
    fixed_draftmode,
    fixed_draftmode_set,
    fixed_inclusion_copy_font,
    fixed_pdfoutput,
    fixed_pdfoutput_set,
    fixed_decimal_digits,
    scaled_out,
    symbolic,
    os_emitted,
    virt,
    refs,
    os_refs,
    zrefs,
    font_refs,
    forced
});

/// The state hash leaves out where in the file the writer is (`gone` and
/// the offsets kept from it): TeX never observes a byte offset, so two
/// runs whose output differs only in length before here are in the same
/// state (DESIGN.md §5.6). With symbolic object streams it leaves out
/// what they hold too, and where in them objects begin (output, laid out
/// by the link); `stream_length` and `last_byte` are dead between
/// streams.
impl core::hash::Hash for PdfOut {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        let Self {
            file,
            pending,
            gone: _,
            buf,
            os_buf,
            os_mode,
            os_enable,
            os_cur_objnum,
            os_objidx,
            os_objnum,
            os_objoff,
            os_cntr,
            zip,
            // (set when a stream ends and read right then, for its
            // `/Length`: the last stream's, dead between)
            stream_length: _,
            stream_length_offset: _,
            seek_write_length,
            save_offset: _,
            // (never read)
            last_byte: _,
            version_written,
            fixed_major,
            fixed_minor,
            fixed_objcompresslevel,
            fixed_gamma,
            fixed_image_gamma,
            fixed_image_hicolor,
            fixed_image_apply_gamma,
            fixed_draftmode,
            fixed_draftmode_set,
            fixed_inclusion_copy_font,
            fixed_pdfoutput,
            fixed_pdfoutput_set,
            fixed_decimal_digits,
            scaled_out,
            symbolic,
            os_emitted: _,
            virt,
            refs,
            os_refs,
            zrefs,
            font_refs,
            forced: _,
        } = self;
        file.hash(h);
        pending.hash(h);
        buf.hash(h);
        virt.hash(h);
        // (where the relocations are in the bytes held: counted from them)
        let from = self.gone - len64(pending.len());
        for (at, k) in refs {
            (at - from, k).hash(h);
        }
        os_refs.hash(h);
        (zrefs, font_refs).hash(h);
        symbolic.hash(h);
        if !*symbolic {
            os_buf.hash(h);
            os_objoff.hash(h);
        }
        os_mode.hash(h);
        os_enable.hash(h);
        os_cur_objnum.hash(h);
        os_objidx.hash(h);
        os_objnum.hash(h);
        os_cntr.hash(h);
        zip.hash(h);
        seek_write_length.hash(h);
        version_written.hash(h);
        fixed_major.hash(h);
        fixed_minor.hash(h);
        fixed_objcompresslevel.hash(h);
        fixed_gamma.hash(h);
        fixed_image_gamma.hash(h);
        fixed_image_hicolor.hash(h);
        fixed_image_apply_gamma.hash(h);
        fixed_draftmode.hash(h);
        fixed_draftmode_set.hash(h);
        fixed_inclusion_copy_font.hash(h);
        fixed_pdfoutput.hash(h);
        fixed_pdfoutput_set.hash(h);
        fixed_decimal_digits.hash(h);
        scaled_out.hash(h);
    }
}

/// The `OUT` field's version (`pdf::val`): the state hash's fields. The
/// build links its files and the link places every object (DESIGN
/// 7.17.3), so where in the file the writer is (`gone` and the offsets
/// kept from it) is not state, nor, with symbolic object streams, what
/// they hold.
impl super::val::Record for PdfOut {
    fn part_version(&self, _: usize) -> u128 {
        partex_ssa::Version::of(self).0
    }
}

/// `ten_pow`.
pub(crate) use partex_engine::scaled::TEN_POW;

pub(crate) use partex_engine::scaled::divide_scaled;
pub(crate) use partex_engine::scaled::round_xn_over_d;

/// The objects waiting in an open object stream as a rebuild wrote them,
/// where they differ from the previous build's (a link's rectangle moved):
/// output not yet written, not state ([`PdfOut::objstm_differ`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObjStmDiffer {
    cur: i32,
    idx: i32,
    buf: Vec<u8>,
    off: Vec<i32>,
    /// The previous build's buffer's length then.
    old_len: usize,
}

/// An object stream as the job wrote it (`Tex::record_objstms`): where in
/// the file, and its objects uncompressed. A splice that joins a previous
/// build past the stream's write renders it again with the rebuild's
/// objects ([`ObjStmWritten::with_differ`], [`ObjStmWritten::render`]):
/// packing and compressing are output, done after the fact, while the
/// object numbers stay the engine's (TeX observes them).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObjStmWritten {
    /// The stream's object number.
    pub num: i32,
    /// Where its object begins and ends in the file.
    pub begin: i64,
    pub end: i64,
    /// `\pdfcompresslevel` then.
    level: i32,
    objnum: Vec<i32>,
    objoff: Vec<i32>,
    objs: Vec<u8>,
}

impl ObjStmDiffer {
    /// The object stream's number.
    #[must_use]
    pub fn cur(&self) -> i32 {
        self.cur
    }
}

impl ObjStmWritten {
    /// The stream with the objects a rebuild had waiting (`d`) first, and
    /// the previous build's later ones after them, moved; `None` if `d`
    /// is not about this stream.
    #[must_use]
    pub fn with_differ(&self, d: &ObjStmDiffer) -> Option<Self> {
        let n = d.off.len();
        if self.num != d.cur || self.objnum.len() < n || self.objs.len() < d.old_len {
            return None;
        }
        let shift = i32::try_from(d.buf.len()).ok()? - i32::try_from(d.old_len).ok()?;
        let mut objs = d.buf.clone();
        objs.extend_from_slice(&self.objs[d.old_len..]);
        let mut objoff = d.off.clone();
        objoff.extend(self.objoff[n..].iter().map(|&x| x + shift));
        Some(Self {
            objoff,
            objs,
            ..self.clone()
        })
    }

    /// The object's bytes as `pdf_os_write_objstream` writes them, the
    /// stream compressed by `deflate` (zlib's `compress` at a level).
    #[must_use]
    pub fn render(&self, deflate: impl Fn(i32, &[u8]) -> Option<Vec<u8>>) -> Vec<u8> {
        use core::fmt::Write as _;
        let mut data = alloc::string::String::new();
        for (i, (n, off)) in self.objnum.iter().zip(&self.objoff).enumerate() {
            let _ = write!(data, "{n} {off}{}", if i % 10 == 9 { '\n' } else { ' ' });
        }
        let mut data = data.into_bytes();
        if let Some(last) = data.last_mut() {
            *last = b'\n';
        }
        let first = data.len();
        data.extend_from_slice(&self.objs);
        let stream = if self.level > 0 {
            deflate(self.level, &data).unwrap_or_else(|| partex_engine::zlib::stored(&data))
        } else {
            data
        };
        let mut out = alloc::format!(
            "{} 0 obj\n<<\n/Type /ObjStm\n/N {}\n/First {first}\n",
            self.num,
            self.objnum.len()
        )
        .into_bytes();
        // (`/Length` and blanks, the length over the first of them)
        let len = alloc::format!("{}", stream.len());
        let mut length = *b"/Length           ";
        for (i, c) in len.bytes().enumerate() {
            if let Some(b) = length.get_mut(8 + i) {
                *b = c;
            }
        }
        out.extend_from_slice(&length);
        out.push(b'\n');
        if self.level > 0 {
            out.extend_from_slice(b"/Filter /FlateDecode\n");
        }
        out.extend_from_slice(b">>\nstream\n");
        out.extend_from_slice(&stream);
        out.extend_from_slice(b"\nendstream\nendobj\n");
        out
    }
}

impl PdfOut {
    /// How the objects waiting in the open object stream differ from
    /// `old`'s, if only their bytes do (the same stream, the same objects,
    /// none being written).
    pub(crate) fn objstm_differ(&self, old: &Self) -> Option<ObjStmDiffer> {
        // (`os_mode` stays as the last object left it: between objects
        // nothing is being written)
        let same = self.os_mode == old.os_mode
            && self.os_cur_objnum != 0
            && self.os_cur_objnum == old.os_cur_objnum
            && self.os_objidx == old.os_objidx
            && self.os_objnum == old.os_objnum
            && (self.os_buf != old.os_buf || self.os_objoff != old.os_objoff);
        same.then(|| ObjStmDiffer {
            cur: self.os_cur_objnum,
            idx: self.os_objidx,
            buf: self.os_buf.clone(),
            off: self.os_objoff.clone(),
            old_len: old.os_buf.len(),
        })
    }

    /// Whether a later state of the previous build still has that object
    /// stream open, with those objects first (it was not written since).
    pub(crate) fn objstm_fits(&self, d: &ObjStmDiffer) -> bool {
        self.os_cur_objnum == d.cur
            && self.os_objidx >= d.idx
            && self.os_buf.len() >= d.old_len
            && self.os_objoff.len() >= d.off.len()
    }

    /// Give a later state of the previous build the rebuild's objects
    /// first, its own later ones moved after them.
    pub(crate) fn import_objstm(&mut self, d: &ObjStmDiffer) -> bool {
        if !self.objstm_fits(d) {
            return false;
        }
        let n = d.off.len();
        let shift = i32::try_from(d.buf.len()).unwrap_or(0) - i32::try_from(d.old_len).unwrap_or(0);
        let mut buf = d.buf.clone();
        buf.extend_from_slice(&self.os_buf[d.old_len..]);
        let mut off = d.off.clone();
        off.extend(self.os_objoff[n..].iter().map(|&x| x + shift));
        self.os_buf = buf;
        self.os_objoff = off;
        true
    }

    /// The fields that differ from `other`'s (`PARTEX_WATCH_DEBUG`).
    pub(crate) fn differences(&self, other: &Self) -> Vec<alloc::string::String> {
        let mut out = Vec::new();
        macro_rules! cmp {
            ($($f:ident),*) => {$(
                if self.$f != other.$f {
                    out.push(alloc::string::String::from(stringify!($f)));
                }
            )*};
        }
        cmp!(
            file,
            os_mode,
            os_enable,
            os_cur_objnum,
            os_objidx,
            os_objnum,
            os_objoff,
            os_cntr,
            zip,
            stream_length,
            seek_write_length,
            last_byte,
            version_written
        );
        for (name, a, b) in [
            ("pending", &self.pending, &other.pending),
            ("buf", &self.buf, &other.buf),
            ("os_buf", &self.os_buf, &other.os_buf),
        ] {
            if a != b {
                let same = a.iter().zip(b.iter()).take_while(|(x, y)| x == y).count();
                out.push(alloc::format!(
                    "{name} ({} vs {} bytes, the same for {same}: {:?} / {:?})",
                    a.len(),
                    b.len(),
                    alloc::string::String::from_utf8_lossy(&a[same..(same + 60).min(a.len())]),
                    alloc::string::String::from_utf8_lossy(&b[same..(same + 60).min(b.len())]),
                ));
            }
        }
        // (the virtual numbers the pending bytes refer to, by where from
        // the pending bytes' start)
        let rel = |o: &Self| -> Vec<(i64, Reloc)> {
            let from = o.gone - len64(o.pending.len());
            o.refs.iter().map(|&(at, k)| (at - from, k)).collect()
        };
        if rel(self) != rel(other) {
            out.push(alloc::format!(
                "refs ({} vs {})",
                self.refs.len(),
                other.refs.len()
            ));
        }
        if self.os_refs != other.os_refs {
            out.push(alloc::string::String::from("os_refs"));
        }
        if (&self.zrefs, &self.font_refs) != (&other.zrefs, &other.font_refs) {
            out.push(alloc::string::String::from("zrefs or font_refs"));
        }
        out
    }

    /// `pdf_offset`.
    /// Follow a splice (see `Tex::relocate_output`): positions from
    /// `old_gone` on move by `delta`; earlier ones are `now`'s.
    pub(crate) fn relocate(
        &mut self,
        now: &Self,
        old_gone: i64,
        delta: i64,
        more: &dyn Fn(i64) -> i64,
    ) {
        let moved = |x: i64, n: i64| {
            if x >= old_gone {
                x + delta + more(x)
            } else {
                n
            }
        };
        self.gone = moved(self.gone, now.gone);
        self.save_offset = moved(self.save_offset, now.save_offset);
        self.stream_length_offset = moved(self.stream_length_offset, now.stream_length_offset);
    }

    pub(crate) fn offset(&self) -> i64 {
        self.gone + len64(self.ptr())
    }

    /// Bytes of the file before `offset()` not yet handed on (pending or
    /// buffered): where the next byte lands, counted from the bytes the
    /// host or the effects have.
    pub(crate) fn unwritten(&self) -> u64 {
        let buf = if self.os_mode { 0 } else { self.buf.len() };
        u64::try_from(self.pending.len() + buf).unwrap_or(0)
    }

    /// Take what is buffered, without flushing it (bytes printed only to
    /// be kept, `finish.rs`).
    pub(crate) fn take_buf(&mut self) -> Vec<u8> {
        core::mem::take(&mut self.buf)
    }

    /// `pdf_ptr`.
    pub(crate) fn ptr(&self) -> usize {
        if self.os_mode {
            self.os_buf.len()
        } else {
            self.buf.len()
        }
    }

    fn cur(&mut self) -> &mut Vec<u8> {
        if self.os_mode {
            &mut self.os_buf
        } else {
            &mut self.buf
        }
    }

    /// `pdf_flush`.
    pub(crate) fn flush(&mut self) {
        if self.os_mode {
            return;
        }
        let b = core::mem::take(&mut self.buf);
        if let Some(z) = &mut self.zip {
            z.extend_from_slice(&b);
        } else if !b.is_empty() {
            if self.fixed_draftmode == 0 {
                self.pending.extend_from_slice(&b);
            }
            self.gone += len64(b.len());
            self.last_byte = b[b.len() - 1];
        }
        self.buf = b;
        self.buf.clear();
    }

    /// `pdf_room(n)` before `n` bytes.
    fn room(&mut self, n: usize) {
        if !self.os_mode && n + self.buf.len() > PDF_OP_BUF_SIZE {
            self.flush();
        }
    }

    /// `pdf_out`.
    pub(crate) fn out(&mut self, c: u8) {
        self.room(1);
        self.cur().push(c);
    }

    /// `pdf_print` of bytes.
    pub(crate) fn print(&mut self, s: &[u8]) {
        for &c in s {
            self.out(c);
        }
    }

    /// `pdf_print_ln`.
    pub(crate) fn print_ln(&mut self, s: &[u8]) {
        self.print(s);
        self.out(b'\n');
    }

    /// `pdf_print_int`.
    pub(crate) fn print_int(&mut self, n: i64) {
        let mut d = [0u8; 24];
        let mut k = 0;
        let mut m = n.unsigned_abs();
        if n < 0 {
            self.out(b'-');
        }
        loop {
            d[k] = u8::try_from(m % 10).unwrap_or(0);
            m /= 10;
            k += 1;
            if m == 0 {
                break;
            }
        }
        self.room(k);
        while k > 0 {
            k -= 1;
            self.cur().push(b'0' + d[k]);
        }
    }

    pub(crate) fn print_int_ln(&mut self, n: i64) {
        self.print_int(n);
        self.out(b'\n');
    }

    /// `pdf_print_fw_int`.
    pub(crate) fn print_fw_int(&mut self, n: i64, w: usize) {
        let mut n = n;
        let mut d = Vec::with_capacity(w);
        for _ in 0..w {
            d.push(u8::try_from(n.rem_euclid(10)).unwrap_or(0));
            n /= 10;
        }
        self.room(w);
        for &c in d.iter().rev() {
            self.cur().push(b'0' + c);
        }
    }

    /// `pdf_out_bytes`.
    pub(crate) fn out_bytes(&mut self, n: i64, w: usize) {
        let mut n = n;
        let mut d = Vec::with_capacity(w);
        for _ in 0..w {
            d.push(u8::try_from(n.rem_euclid(256)).unwrap_or(0));
            n /= 256;
        }
        self.room(w);
        for &c in d.iter().rev() {
            self.cur().push(c);
        }
    }

    /// `pdf_print_real`: `m / 10^d`.
    pub(crate) fn print_real(&mut self, m: i32, d: i32) {
        let mut m = m;
        let mut d = d;
        if m < 0 {
            self.out(b'-');
            m = -m;
        }
        let p = |d: i32| TEN_POW[usize::try_from(d).unwrap_or(0)];
        self.print_int(i64::from(m / p(d)));
        m %= p(d);
        if m > 0 {
            self.out(b'.');
            d -= 1;
            while m < p(d) {
                self.out(b'0');
                d -= 1;
            }
            while m % 10 == 0 {
                m /= 10;
            }
            self.print_int(i64::from(m));
        }
    }

    /// `pdf_print_octal`.
    pub(crate) fn print_octal(&mut self, n: u8) {
        if n < 8 {
            self.out(b'0');
        }
        if n < 64 {
            self.out(b'0');
        }
        for c in alloc::format!("{n:o}").bytes() {
            self.out(c);
        }
    }

    /// `remove_last_space`.
    pub(crate) fn remove_last_space(&mut self) {
        let b = self.cur();
        if b.last() == Some(&b' ') {
            b.pop();
        }
    }

    /// The last byte in the buffer, if any.
    pub(crate) fn last_in_buf(&self) -> Option<u8> {
        if self.os_mode {
            self.os_buf.last().copied()
        } else {
            self.buf.last().copied()
        }
    }

    /// Bytes the host can have: everything before an open `/Length`.
    pub(crate) fn take_pending(&mut self) -> Vec<u8> {
        if self.seek_write_length {
            let keep = usize::try_from(self.gone - self.stream_length_offset).unwrap_or(0);
            let keep = keep.min(self.pending.len());
            let n = self.pending.len() - keep;
            self.pending.drain(..n).collect()
        } else {
            core::mem::take(&mut self.pending)
        }
    }

    /// `pdf_indirect`.
    pub(crate) fn indirect(&mut self, s: &[u8], o: i32) {
        self.out(b'/');
        self.print(s);
        self.out(b' ');
        self.objnum(o);
        self.print(b" 0 R");
    }

    /// Print object number `k` (virtual numbers: a relocation, or
    /// pdfTeX's number where the job observes them).
    pub(crate) fn objnum(&mut self, k: i32) {
        if let Some(n) = &self.forced.0 {
            let f = n.of(k);
            self.print_int(i64::from(f));
            return;
        }
        if self.virt && k > 0 {
            self.reloc(Reloc::Obj(k));
        }
        self.print_int(i64::from(k));
    }

    /// Whether streams are the link's to compress.
    pub(crate) fn defer_streams(&self) -> bool {
        self.virt && self.font_refs
    }

    /// Note relocation `r` where the next bytes go (in the object stream,
    /// the stream being made, or the file).
    fn reloc(&mut self, r: Reloc) {
        if self.os_mode {
            self.os_refs.push((self.os_buf.len(), r));
        } else if let Some(z) = &self.zip {
            if self.defer_streams() {
                self.zrefs.push((z.len() + self.buf.len(), r));
            }
        } else {
            self.refs.push((self.gone + len64(self.buf.len()), r));
        }
    }

    /// Print font slot `f`'s `/F` name's digits as a relocation (its
    /// number is the link's: `font_refs`).
    pub(crate) fn fontref(&mut self, f: i32) {
        self.reloc(Reloc::Font(f));
        self.print_int(i64::from(f));
    }

    /// [`PdfOut::take_pending`], cut at the relocations: bytes, and the
    /// objects whose numbers go between them (their digits taken out).
    pub(crate) fn take_pending_refs(&mut self) -> Vec<(Vec<u8>, Option<Reloc>)> {
        let from = self.gone - len64(self.pending.len());
        let bytes = self.take_pending();
        if self.refs.is_empty() {
            return alloc::vec![(bytes, None)];
        }
        let to = from + len64(bytes.len());
        let mut out = Vec::new();
        let mut at = 0usize;
        let mut rest = Vec::new();
        for &(p, k) in &self.refs {
            if p >= to {
                rest.push((p, k));
                continue;
            }
            if p < from {
                continue;
            }
            let i = usize::try_from(p - from).unwrap_or(0);
            out.push((bytes[at..i].to_vec(), Some(k)));
            at = (i + k.width()).min(bytes.len());
        }
        out.push((bytes[at..].to_vec(), None));
        self.refs = rest;
        out
    }

    /// `pdf_indirect` of pdfTeX's number `n` itself (never mapped).
    pub(crate) fn indirect_final_ln(&mut self, s: &[u8], n: i32) {
        self.out(b'/');
        self.print(s);
        self.out(b' ');
        self.print_int(i64::from(n));
        self.print(b" 0 R");
        self.out(b'\n');
    }

    pub(crate) fn indirect_ln(&mut self, s: &[u8], o: i32) {
        self.indirect(s, o);
        self.out(b'\n');
    }

    /// `pdf_int_entry`.
    pub(crate) fn int_entry_ln(&mut self, s: &[u8], v: i64) {
        self.out(b'/');
        self.print(s);
        self.out(b' ');
        self.print_int(v);
        self.out(b'\n');
    }

    /// `pdf_print_str`: `s` as a PDF string (as is if it already is
    /// one).
    pub(crate) fn print_str(&mut self, s: &[u8]) {
        if s.is_empty() {
            self.print(b"()");
            return;
        }
        if s[0] == b'(' && s[s.len() - 1] == b')' {
            self.print(s);
            return;
        }
        let hex = s[0] == b'<'
            && s[s.len() - 1] == b'>'
            && s.len().is_multiple_of(2)
            && s[1..s.len() - 1].iter().all(u8::is_ascii_hexdigit);
        if hex {
            self.print(s);
        } else {
            self.out(b'(');
            self.print(s);
            self.out(b')');
        }
    }

    pub(crate) fn str_entry_ln(&mut self, s: &[u8], v: &[u8]) {
        self.out(b'/');
        self.print(s);
        self.out(b' ');
        self.print_str(v);
        self.out(b'\n');
    }
}

impl<H: Host, T: Tracker> Tex<H, T> {
    /// Hand the finished bytes of the PDF file to the host.
    pub(crate) fn pdf_write_pending(&mut self) {
        if self.pdf.out.virt {
            let pieces = self.pdf.out.take_pending_refs();
            if let Some(id) = self.pdf.out.file {
                for (bytes, k) in pieces {
                    self.out_write(id, &bytes);
                    match k {
                        Some(Reloc::Obj(num)) => {
                            self.out_mark(crate::effects::Effect::ObjRef { file: id, num });
                        }
                        Some(Reloc::Font(slot)) => {
                            self.out_mark(crate::effects::Effect::FontRef { file: id, slot });
                        }
                        Some(Reloc::Length) => {
                            self.out_mark(crate::effects::Effect::StreamLength { file: id });
                        }
                        None => {}
                    }
                }
            }
            return;
        }
        let bytes = self.pdf.out.take_pending();
        if let (Some(id), false) = (self.pdf.out.file, bytes.is_empty()) {
            if T::VALUES {
                // (the file's bytes are effects, never read)
                self.tracker.output(Output::Pdf, &bytes);
            }
            self.out_write(id, &bytes);
        }
    }

    /// Bytes to the PDF file as an effect emitted again (a hit's).
    pub(crate) fn pdf_bytes_raw(&mut self, bytes: &[u8]) {
        if let Some(id) = self.pdf.out.file {
            self.out_write(id, bytes);
        }
    }

    /// pdfTeX §684: `ensure_pdf_open`.
    pub(crate) fn ensure_pdf_open(&mut self) -> Result<(), Jump> {
        if self.output_file_name() != 0 {
            return Ok(());
        }
        if self.job_name() == 0 {
            self.open_log_file()?;
        }
        self.pack_job_name(b".pdf");
        if self.pdf.out.fixed_draftmode == 0 {
            loop {
                let name = self.name_of_file.clone();
                if let Some((id, printed)) = self.open_out(&name, FileKind::Other) {
                    self.pdf.out.file = Some(id);
                    self.name_of_file = printed;
                    break;
                }
                self.prompt_file_name(b"file name for output", b".pdf")?;
            }
        }
        let s = self.make_name_string()?;
        self.set_output_file_name(s);
        Ok(())
    }

    /// pdfTeX §683: `check_pdfversion`: fix the parameters that must not
    /// change once output starts, and write the header.
    pub(crate) fn check_pdfversion(&mut self) -> Result<(), Jump> {
        if self.pdf.out.version_written {
            if self.pdf.out.fixed_minor != self.int_par(PDF_MINOR_VERSION_CODE)
                || self.pdf.out.fixed_major != self.int_par(PDF_MAJOR_VERSION_CODE)
            {
                return self.pdf_error(
                    b"setup",
                    b"PDF version cannot be changed after data is written to the PDF file",
                );
            }
            return Ok(());
        }
        self.pdf.out.version_written = true;
        if self.int_par(PDF_MAJOR_VERSION_CODE) < 1 {
            self.print_err(b"pdfTeX error (invalid pdfmajorversion)");
            self.print_ln();
            self.help(&[
                b"The pdfmajorversion must be 1 or greater.",
                b"I changed this to 1.",
            ]);
            self.int_error(self.int_par(PDF_MAJOR_VERSION_CODE))?;
            self.set_int_par(PDF_MAJOR_VERSION_CODE, 1);
        }
        let minor = self.int_par(PDF_MINOR_VERSION_CODE);
        if !(0..=9).contains(&minor) {
            self.print_err(b"pdfTeX error (invalid pdfminorversion)");
            self.print_ln();
            self.help(&[
                b"The pdfminorversion must be between 0 and 9.",
                b"I changed this to 4.",
            ]);
            self.int_error(minor)?;
            self.set_int_par(PDF_MINOR_VERSION_CODE, 4);
        }
        let p = |t: &Self, c: i32, lo: i32, hi: i32| t.int_par(c).clamp(lo, hi);
        let v = [
            self.int_par(PDF_MAJOR_VERSION_CODE),
            self.int_par(PDF_MINOR_VERSION_CODE),
            p(self, PDF_GAMMA_CODE, 0, 1_000_000),
            p(self, PDF_IMAGE_GAMMA_CODE, 0, 1_000_000),
            p(self, PDF_IMAGE_HICOLOR_CODE, 0, 1),
            p(self, PDF_IMAGE_APPLY_GAMMA_CODE, 0, 1),
            p(self, PDF_OBJCOMPRESSLEVEL_CODE, 0, 3),
            p(self, PDF_DRAFTMODE_CODE, 0, 1),
            p(self, PDF_INCLUSION_COPY_FONT_CODE, 0, 1),
        ];
        let o = &mut *self.pdf.out;
        [
            o.fixed_major,
            o.fixed_minor,
            o.fixed_gamma,
            o.fixed_image_gamma,
            o.fixed_image_hicolor,
            o.fixed_image_apply_gamma,
            o.fixed_objcompresslevel,
            o.fixed_draftmode,
            o.fixed_inclusion_copy_font,
        ] = v;
        if (o.fixed_major > 1 || o.fixed_minor >= 5) && o.fixed_objcompresslevel > 0 {
            o.os_enable = true;
        } else {
            if o.fixed_objcompresslevel > 0 {
                self.pdf_warning(
                    b"Object streams",
                    b"\\pdfobjcompresslevel > 0 requires PDF-1.5 or greater. Object streams disabled now.",
                    true,
                    true,
                );
                self.pdf.out.fixed_objcompresslevel = 0;
            }
            self.pdf.out.os_enable = false;
        }
        self.ensure_pdf_open()?;
        self.fix_pdfoutput()?;
        let o = &mut *self.pdf.out;
        o.print(b"%PDF-");
        o.print_int(i64::from(o.fixed_major));
        o.print(b".");
        o.print_int_ln(i64::from(o.fixed_minor));
        o.print(b"%");
        for c in [208, 212, 197, 216] {
            o.out(c); // 'P', 'T', 'E', 'X' + 128
        }
        o.out(b'\n');
        Ok(())
    }

    /// pdfTeX §747: `fix_pdfoutput`.
    pub(crate) fn fix_pdfoutput(&mut self) -> Result<(), Jump> {
        use super::val::{bit, field};
        self.writer_scope(0, bit(field::OUT), Self::fix_pdfoutput_now)
    }

    /// `fixed_pdfoutput_set` and `fixed_pdfoutput` (read).
    pub(crate) fn pdf_output_fixed(&self) -> (bool, i32) {
        self.writer_read(super::val::field::OUT);
        (
            self.pdf.out.fixed_pdfoutput_set,
            self.pdf.out.fixed_pdfoutput,
        )
    }

    fn fix_pdfoutput_now(&mut self) -> Result<(), Jump> {
        let pdf_output = self.int_par(PDF_OUTPUT_CODE);
        if !self.pdf.out.fixed_pdfoutput_set {
            self.pdf.out.fixed_pdfoutput = pdf_output;
            self.pdf.out.fixed_pdfoutput_set = true;
        } else if self.pdf.out.fixed_pdfoutput != pdf_output {
            return self.pdf_error(
                b"setup",
                b"\\pdfoutput can only be changed before anything is written to the output",
            );
        }
        self.fix_pdf_draftmode()
    }

    /// pdfTeX §748: `fix_pdf_draftmode`.
    fn fix_pdf_draftmode(&mut self) -> Result<(), Jump> {
        let draftmode = self.int_par(PDF_DRAFTMODE_CODE);
        if !self.pdf.out.fixed_draftmode_set {
            self.pdf.out.fixed_draftmode = draftmode;
            self.pdf.out.fixed_draftmode_set = true;
        } else if self.pdf.out.fixed_draftmode != draftmode {
            return self.pdf_error(
                b"setup",
                b"\\pdfdraftmode can only be changed before anything is written to the output",
            );
        }
        if self.pdf.out.fixed_draftmode > 0 {
            self.set_int_par(PDF_COMPRESS_LEVEL_CODE, 0);
            self.pdf.out.fixed_objcompresslevel = 0;
        }
        Ok(())
    }

    /// pdfTeX §685: `pdf_begin_stream`.
    pub(crate) fn pdf_begin_stream(&mut self) {
        let level = self.int_par(PDF_COMPRESS_LEVEL_CODE);
        let o = &mut *self.pdf.out;
        if o.defer_streams() {
            // (the link compresses the stream and fills in its length:
            // the blanks for it are a relocation)
            o.print(b"/Length ");
            o.reloc(Reloc::Length);
            o.print_ln(&[b' '; LENGTH_HOLE + 1][..LENGTH_HOLE]);
            if level > 0 {
                o.print_ln(b"/Filter /FlateDecode");
            }
            o.print_ln(b">>");
            o.print_ln(b"stream");
            o.flush();
            o.zip = Some(Vec::new());
            o.zrefs.clear();
            return;
        }
        o.print_ln(b"/Length           ");
        o.seek_write_length = true;
        o.stream_length_offset = o.offset() - 11;
        o.stream_length = 0;
        o.last_byte = 0;
        if level > 0 {
            o.print_ln(b"/Filter /FlateDecode");
            o.print_ln(b">>");
            o.print_ln(b"stream");
            o.flush();
            o.zip = Some(Vec::new());
        } else {
            o.print_ln(b">>");
            o.print_ln(b"stream");
            o.save_offset = o.offset();
        }
    }

    /// pdfTeX §685: `pdf_end_stream`.
    pub(crate) fn pdf_end_stream(&mut self) {
        self.pdf.out.flush();
        // (a stream `pdf_begin_stream` began: an image's bytes are written
        // as they are, its length known before)
        if self.pdf.out.defer_streams() && self.pdf.out.zip.is_some() {
            // (the stream's bytes, with their relocations, for the link to
            // compress at the level (0: as they are), and its length)
            let data = self.pdf.out.zip.take().unwrap_or_default();
            let refs = core::mem::take(&mut self.pdf.out.zrefs);
            let level = self.int_par(PDF_COMPRESS_LEVEL_CODE).max(0);
            let mut parts = Vec::new();
            let mut at = 0;
            for (p, r) in refs {
                parts.push((data[at..p.min(data.len())].to_vec(), Some(r)));
                at = (p + r.width()).min(data.len());
            }
            parts.push((data[at..].to_vec(), None));
            self.pdf_write_pending();
            // (the bytes the file has so far, for "Output written": the
            // stream compressed as it is, which is what the link writes
            // unless a relocation's digits differ; the host keeps it, so
            // the link finds it again then)
            let size = if level > 0 {
                self.host
                    .deflate(level, &data)
                    .map_or(data.len(), |z| z.len())
            } else {
                data.len()
            };
            if let (Some(file), 0) = (self.pdf.out.file, self.pdf.out.fixed_draftmode) {
                self.out_mark(crate::effects::Effect::Deflate { file, level, parts });
                self.pdf.out.gone += len64(size);
            }
            let o = &mut *self.pdf.out;
            o.out(b'\n');
            o.print_ln(b"endstream");
            self.pdf_end_obj();
            self.pdf_write_pending();
            return;
        }
        if let Some(data) = self.pdf.out.zip.take() {
            let level = self.int_par(PDF_COMPRESS_LEVEL_CODE);
            let z = self
                .host
                .deflate(level, &data)
                .unwrap_or_else(|| partex_engine::zlib::stored(&data));
            let o = &mut *self.pdf.out;
            if o.fixed_draftmode == 0 {
                o.pending.extend_from_slice(&z);
            }
            o.gone += len64(z.len());
            o.last_byte = z.last().copied().unwrap_or(0);
            o.stream_length = len64(z.len());
        } else {
            let o = &mut *self.pdf.out;
            o.stream_length = o.offset() - o.save_offset;
        }
        let o = &mut *self.pdf.out;
        if o.seek_write_length && o.fixed_draftmode == 0 {
            // utils.c's `writestreamlength`: the length over the blanks
            let at = usize::try_from(o.stream_length_offset - (o.gone - len64(o.pending.len())))
                .unwrap_or(0);
            for (i, c) in alloc::format!("{}", o.stream_length).bytes().enumerate() {
                if let Some(b) = o.pending.get_mut(at + i) {
                    *b = c;
                }
            }
        }
        o.seek_write_length = false;
        o.out(b'\n');
        o.print_ln(b"endstream");
        self.pdf_end_obj();
        self.pdf_write_pending();
    }

    /// pdfTeX §698: `pdf_os_switch`.
    pub(crate) fn pdf_os_switch(&mut self, pdf_os: bool) {
        let o = &mut *self.pdf.out;
        o.os_mode = pdf_os && o.os_enable;
    }

    /// pdfTeX §698: `pdf_os_prepare_obj`.
    fn pdf_os_prepare_obj(&mut self, i: i32, pdf_os_level: i32) -> Result<(), Jump> {
        let on = pdf_os_level > 0 && self.pdf.out.fixed_objcompresslevel >= pdf_os_level;
        self.pdf_os_switch(on);
        if self.pdf.out.os_mode && self.pdf.out.virt {
            // (virtual numbers: the link fills the streams and numbers
            // them; the object's bytes go out as they come)
            self.pdf.objs.num_event(super::vnum::NumEvent::Start);
            self.pdf_os_emit();
            if let Some(file) = self.pdf.out.file {
                self.out_mark(crate::effects::Effect::Num(super::vnum::NumEvent::Start, 0));
                self.out_mark(crate::effects::Effect::ObjStmStart { file, num: i });
            }
            let o = &mut *self.pdf.out;
            o.os_buf.clear();
            o.os_emitted = 0;
            o.os_refs.clear();
            let e = self.pdf.objs.get_mut(i);
            e.os_idx = 0;
            e.offset = 0;
        } else if self.pdf.out.os_mode {
            if self.pdf.out.os_cur_objnum == 0 {
                let n = self.pdf_new_objnum()?;
                self.pdf.objs.obj_ptr -= 1; // object stream is not accessible to user
                let o = &mut *self.pdf.out;
                o.os_cur_objnum = n;
                o.os_cntr += 1;
                o.os_objidx = 0;
                o.os_buf.clear();
                o.os_emitted = 0;
                o.os_objnum.clear();
                o.os_objoff.clear();
            } else {
                self.pdf.out.os_objidx += 1;
            }
            self.pdf_os_emit();
            if let (true, Some(file)) = (self.pdf.out.symbolic, self.pdf.out.file) {
                self.out_mark(crate::effects::Effect::ObjStmStart { file, num: i });
            }
            let o = &mut *self.pdf.out;
            let e = self.pdf.objs.get_mut(i);
            e.os_idx = o.os_objidx;
            e.offset = i64::from(o.os_cur_objnum);
            o.os_objnum.push(i);
            o.os_objoff
                .push(i32::try_from(o.os_buf.len()).unwrap_or(i32::MAX));
        } else {
            if self.pdf.out.virt {
                // (what is buffered goes first: relocations in it change
                // its length, so the object's mark must not count it)
                self.pdf.out.flush();
                self.pdf_write_pending();
            }
            let off = self.pdf.out.offset();
            let e = self.pdf.objs.get_mut(i);
            e.offset = off;
            e.os_idx = -1;
            if let (Some(file), true) = (self.pdf.out.file, self.effects.is_some()) {
                let ahead = self.pdf.out.unwritten();
                self.out_mark(crate::effects::Effect::PdfObject {
                    file,
                    num: i,
                    ahead,
                });
            }
        }
        Ok(())
    }

    /// pdfTeX §698: `pdf_begin_obj`.
    pub(crate) fn pdf_begin_obj(&mut self, i: i32, pdf_os_level: i32) -> Result<(), Jump> {
        self.check_pdfversion()?;
        self.pdf_os_prepare_obj(i, pdf_os_level)?;
        let level = self.int_par(PDF_COMPRESS_LEVEL_CODE);
        let o = &mut *self.pdf.out;
        if !o.os_mode {
            o.objnum(i);
            o.print_ln(b" 0 obj");
        } else if level == 0 {
            o.print(b"% ");
            o.objnum(i);
            o.print_ln(b" 0 obj");
        }
        Ok(())
    }

    /// pdfTeX §698: `pdf_new_obj`.
    pub(crate) fn pdf_new_obj(&mut self, t: usize, i: i32, pdf_os: i32) -> Result<i32, Jump> {
        let k = self.pdf_create_obj(t, super::objtab::Id::Num(i))?;
        self.pdf_begin_obj(k, pdf_os)?;
        Ok(k)
    }

    /// pdfTeX §698: `pdf_end_obj`.
    pub(crate) fn pdf_end_obj(&mut self) {
        if self.pdf.out.os_mode && self.pdf.out.virt {
            self.pdf_os_end_virt();
        } else if self.pdf.out.os_mode {
            if self.pdf.out.os_objidx == PDF_OS_MAX_OBJS - 1 {
                self.pdf_os_write_objstream();
            }
        } else {
            self.pdf.out.print_ln(b"endobj");
        }
    }

    /// pdfTeX §698: `pdf_begin_dict`.
    pub(crate) fn pdf_begin_dict(&mut self, i: i32, pdf_os_level: i32) -> Result<(), Jump> {
        self.pdf_begin_obj(i, pdf_os_level)?;
        self.pdf.out.print_ln(b"<<");
        Ok(())
    }

    /// pdfTeX §698: `pdf_new_dict`.
    pub(crate) fn pdf_new_dict(&mut self, t: usize, i: i32, pdf_os: i32) -> Result<i32, Jump> {
        let k = self.pdf_create_obj(t, super::objtab::Id::Num(i))?;
        self.pdf_begin_dict(k, pdf_os)?;
        Ok(k)
    }

    /// pdfTeX §698: `pdf_end_dict`.
    pub(crate) fn pdf_end_dict(&mut self) {
        self.pdf.out.print_ln(b">>");
        if self.pdf.out.os_mode && self.pdf.out.virt {
            self.pdf_os_end_virt();
        } else if self.pdf.out.os_mode {
            if self.pdf.out.os_objidx == PDF_OS_MAX_OBJS - 1 {
                self.pdf_os_write_objstream();
            }
        } else {
            self.pdf.out.print_ln(b"endobj");
        }
    }

    /// What the file's buffer holds goes out (an object stream the link
    /// closes here comes after it).
    fn pdf_flush_main(&mut self) {
        let o = &mut *self.pdf.out;
        let os = o.os_mode;
        o.os_mode = false;
        o.flush();
        o.os_mode = os;
        self.pdf_write_pending();
    }

    /// An object in an object stream ends (virtual numbers): its bytes go
    /// out, and the link closes the stream after its hundredth object.
    fn pdf_os_end_virt(&mut self) {
        self.pdf_os_emit();
        self.pdf_flush_main();
        self.pdf.objs.num_event(super::vnum::NumEvent::End);
        let level = self.int_par(PDF_COMPRESS_LEVEL_CODE);
        self.out_mark(crate::effects::Effect::Num(
            super::vnum::NumEvent::End,
            level,
        ));
    }

    /// pdfTeX §699: `pdf_os_write_objstream`.
    pub(crate) fn pdf_os_write_objstream(&mut self) {
        if self.pdf.out.virt {
            // (the end of the job: the link closes the open stream)
            self.pdf_os_emit();
            self.pdf_flush_main();
            self.pdf.objs.num_event(super::vnum::NumEvent::Flush);
            let level = self.int_par(PDF_COMPRESS_LEVEL_CODE);
            self.out_mark(crate::effects::Effect::Num(
                super::vnum::NumEvent::Flush,
                level,
            ));
            return;
        }
        if self.pdf.out.os_cur_objnum == 0 {
            return;
        }
        // Symbolic: the objects went out as effects; what the file has
        // before the stream goes out now, the stream is written as
        // pdfTeX does (for the writer's position) and its bytes become
        // one `ObjStm` effect, which the link renders from the objects.
        let symbolic = match self.pdf.out.file {
            Some(file) if self.pdf.out.symbolic && self.pdf.out.zip.is_none() => {
                self.pdf_os_emit();
                let o = &mut *self.pdf.out;
                o.os_mode = false;
                o.flush();
                o.os_mode = true;
                self.pdf_write_pending();
                Some((file, self.effects.as_ref().map_or(0, Vec::len)))
            }
            _ => None,
        };
        let num = self.pdf.out.os_cur_objnum;
        self.pdf_os_write_objstream_bytes();
        if let Some((file, n)) = symbolic {
            // (all of the stream, `endobj` included, in the effects)
            self.pdf.out.flush();
            self.pdf_write_pending();
            let level = self.int_par(PDF_COMPRESS_LEVEL_CODE);
            if let Some(e) = &mut self.effects {
                // (the stream's bytes, after its object's mark, give way
                // to the symbol)
                let tail: Vec<crate::effects::Effect> = e
                    .drain(n..)
                    .filter(|x| {
                        !matches!(x, crate::effects::Effect::Write { file: f, .. } if *f == file)
                    })
                    .collect();
                e.extend(tail);
                e.push(crate::effects::Effect::ObjStm { file, num, level });
            }
        }
    }

    /// Hand what `pdf_os_buf` gained since the last call to the effects
    /// (symbolic object streams).
    pub(crate) fn pdf_os_emit(&mut self) {
        let o = &mut *self.pdf.out;
        let Some(file) = o
            .file
            .filter(|_| o.symbolic && (o.os_cur_objnum != 0 || o.virt))
        else {
            return;
        };
        if o.os_emitted >= o.os_buf.len() {
            return;
        }
        let from = o.os_emitted;
        o.os_emitted = o.os_buf.len();
        if o.os_refs.is_empty() {
            let bytes = o.os_buf[from..].to_vec();
            self.out_mark(crate::effects::Effect::ObjStmBytes { file, bytes });
            return;
        }
        // (cut at the relocations, their digits taken out)
        let refs = core::mem::take(&mut o.os_refs);
        let mut pieces = Vec::new();
        let mut at = from;
        for &(p, k) in &refs {
            if p < from {
                continue;
            }
            pieces.push((o.os_buf[at..p].to_vec(), Some(k)));
            at = (p + k.width()).min(o.os_buf.len());
        }
        pieces.push((o.os_buf[at..].to_vec(), None));
        for (bytes, k) in pieces {
            if !bytes.is_empty() {
                self.out_mark(crate::effects::Effect::ObjStmBytes { file, bytes });
            }
            match k {
                Some(Reloc::Obj(num)) => {
                    self.out_mark(crate::effects::Effect::ObjStmRef { file, num });
                }
                Some(Reloc::Font(slot)) => {
                    self.out_mark(crate::effects::Effect::ObjStmFontRef { file, slot });
                }
                Some(Reloc::Length) | None => {}
            }
        }
    }

    /// pdfTeX §699: `pdf_os_write_objstream`, the bytes.
    fn pdf_os_write_objstream_bytes(&mut self) {
        let o = &mut *self.pdf.out;
        let p = o.os_buf.len();
        let mut j = 0;
        for i in 0..o.os_objnum.len() {
            let (n, off) = (o.os_objnum[i], o.os_objoff[i]);
            o.print_int(i64::from(n));
            o.print(b" ");
            o.print_int(i64::from(off));
            if j == 9 {
                o.out(b'\n');
                j = 0;
            } else {
                o.print(b" ");
                j += 1;
            }
        }
        if let Some(last) = o.os_buf.last_mut() {
            *last = b'\n';
        }
        let q = o.os_buf.len();
        let objs = core::mem::take(&mut o.os_buf);
        let n = o.os_objidx + 1;
        let cur = o.os_cur_objnum;
        let record = (self.record_objstms && o.fixed_draftmode == 0).then(|| ObjStmWritten {
            num: cur,
            // (in the file, not the object stream the last object went to)
            begin: o.gone + len64(o.buf.len()),
            end: 0,
            level: 0,
            objnum: o.os_objnum.clone(),
            objoff: o.os_objoff.clone(),
            objs: objs[..p].to_vec(),
        });
        // `pdf_begin_dict(pdf_os_cur_objnum, 0)` switches to the file
        let _ = self.pdf_begin_dict(cur, 0);
        let o = &mut *self.pdf.out;
        o.print_ln(b"/Type /ObjStm");
        o.print(b"/N ");
        o.print_int_ln(i64::from(n));
        o.print(b"/First ");
        o.print_int_ln(len64(q - p));
        self.pdf_begin_stream();
        let o = &mut *self.pdf.out;
        o.room(q - p);
        o.buf.extend_from_slice(&objs[p..q]);
        let mut i = 0;
        while i < p {
            let q = (i + PDF_OP_BUF_SIZE).min(p);
            o.room(q - i);
            o.buf.extend_from_slice(&objs[i..q]);
            i = q;
        }
        let level = self.int_par(PDF_COMPRESS_LEVEL_CODE);
        self.pdf_end_stream();
        if let Some(mut r) = record {
            r.end = self.pdf.out.gone + len64(self.pdf.out.buf.len());
            r.level = level;
            self.objstms_written.push(r);
        }
        let o = &mut *self.pdf.out;
        o.os_cur_objnum = 0;
        o.os_buf = objs;
        o.os_buf.clear();
        o.os_emitted = 0;
    }

    /// pdfTeX §690: `pdf_print_bp`.
    pub(crate) fn pdf_print_bp(&mut self, s: Scaled) -> Result<(), Jump> {
        let dd = self.pdf.out.fixed_decimal_digits;
        let (q, out) = self.divide_scaled(s, super::ONE_HUNDRED_BP, dd + 2)?;
        self.pdf.out.scaled_out = out;
        self.pdf.out.print_real(q, dd);
        Ok(())
    }

    /// pdfTeX §690: `pdf_print_mag_bp`.
    pub(crate) fn pdf_print_mag_bp(&mut self, s: Scaled) -> Result<(), Jump> {
        self.prepare_mag()?;
        let mag = self.int_par(MAG_CODE);
        let s = if mag == 1000 {
            s
        } else {
            round_xn_over_d(s, mag, 1000)
        };
        self.pdf_print_bp(s)
    }

    /// `divide_scaled` with pdfTeX's errors.
    pub(crate) fn divide_scaled(
        &mut self,
        s: Scaled,
        m: Scaled,
        dd: i32,
    ) -> Result<(Scaled, Scaled), Jump> {
        match divide_scaled(s, m, dd) {
            Some(r) => {
                self.pdf.out.scaled_out = r.1;
                Ok(r)
            }
            None if m == 0 => self.pdf_error(b"arithmetic", b"divided by zero"),
            None => self.pdf_error(b"arithmetic", b"number too big"),
        }
    }
}

/// A length as a file offset.
fn len64(n: usize) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

impl PdfOut {
    /// Where `self` differs from `other` (for debugging machine mode's
    /// convergence).
    pub(crate) fn difference(&self, other: &Self) -> alloc::string::String {
        use alloc::string::String;
        use core::fmt::Write as _;
        let mut s = String::new();
        let bytes = |name: &str, a: &[u8], b: &[u8], s: &mut String| {
            if a == b {
                return;
            }
            let at = a
                .iter()
                .zip(b)
                .position(|(x, y)| x != y)
                .unwrap_or(a.len().min(b.len()));
            let from = at.saturating_sub(60);
            let show = |x: &[u8]| {
                String::from_utf8_lossy(&x[from.min(x.len())..(at + 60).min(x.len())]).into_owned()
            };
            let _ = write!(
                s,
                "{name}: len {} vs {} at {at}: {:?} vs {:?}; ",
                a.len(),
                b.len(),
                show(a),
                show(b)
            );
        };
        bytes("pending", &self.pending, &other.pending, &mut s);
        bytes("buf", &self.buf, &other.buf, &mut s);
        bytes("os_buf", &self.os_buf, &other.os_buf, &mut s);
        if self.os_objoff != other.os_objoff {
            s += "os_objoff; ";
        }
        if self.os_objnum != other.os_objnum {
            s += "os_objnum; ";
        }
        let a = (
            self.os_objidx,
            self.os_cur_objnum,
            self.stream_length,
            self.last_byte,
        );
        let b = (
            other.os_objidx,
            other.os_cur_objnum,
            other.stream_length,
            other.last_byte,
        );
        if a != b {
            let _ = write!(s, "scalars {a:?} vs {b:?}; ");
        }
        if self.zip != other.zip {
            s += "zip; ";
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn divide_scaled_like_pdftex() {
        // 1pt = 65536sp in bp with 3 digits: 0.996
        assert_eq!(divide_scaled(65536, 6_578_176, 5), Some((996, 65519)));
        assert_eq!(divide_scaled(-65536, 6_578_176, 5).map(|r| r.0), Some(-996));
        assert_eq!(divide_scaled(1, 0, 3), None);
    }

    #[test]
    fn print_real_trims_zeros() {
        let mut o = PdfOut::default();
        o.print_real(996, 3);
        o.out(b' ');
        o.print_real(-1500, 3);
        o.out(b' ');
        o.print_real(2000, 3);
        o.out(b' ');
        o.print_real(5, 3);
        o.flush();
        assert_eq!(o.take_pending(), b"0.996 -1.5 2 0.005");
    }
}
