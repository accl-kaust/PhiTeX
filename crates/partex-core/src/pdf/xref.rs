//! The end of a PDF file as a value (DESIGN.md §7.6): the cross-reference
//! table or stream, the trailer and `startxref`, which name byte offsets
//! and so are rendered at the link step, once the offsets are known.
//!
//! What does not depend on offsets (the trailer's `/Root`, `/Info`,
//! `\pdftrailer` text and `/ID`) is printed by the engine as pdfTeX
//! prints it and kept as bytes; the rest is laid out here the way
//! pdfTeX §794's "Output the cross-reference stream dictionary" and
//! "Output the `obj_tab`" write it.

use alloc::vec::Vec;

/// Compresses a stream at a `\\pdfcompresslevel`; `None` for zlib's
/// stored blocks.
pub type Deflate<'a> = dyn FnMut(i32, &[u8]) -> Option<Vec<u8>> + 'a;

/// One cross-reference entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum XEntry {
    /// Not written: the next free object.
    Free(i32),
    /// Written at the byte offset of object `num`'s mark.
    Byte(i32),
    /// In object stream `stream`, at index `idx`.
    InStream(i32, u8),
    /// Object `num`, in the object stream the link put it in (symbolic
    /// object streams).
    Placed(i32),
}

/// A cross-reference stream (PDF 1.5 with object streams).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct XrefStream {
    /// Its object number (the last object; its offset is the section's).
    pub num: i32,
    /// `obj_ptr`: the last object users see (`/Size` is one more).
    pub obj_ptr: i32,
    /// `\pdfcompresslevel`.
    pub level: i32,
}

/// The cross-reference section and trailer of a PDF file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Xref {
    /// `None`: a classic table.
    pub stream: Option<XrefStream>,
    /// Entries from object 0 on.
    pub entries: Vec<XEntry>,
    /// Stream: the dictionary lines after `/W` (with the newline after
    /// `/ID`). Table: everything from `trailer` to ` >>\n`.
    pub tail: Vec<u8>,
}

fn int(out: &mut Vec<u8>, n: i64) {
    out.extend_from_slice(alloc::format!("{n}").as_bytes());
}

/// pdfTeX's `pdf_out_bytes`: `n` in `w` bytes, big-endian.
fn out_bytes(out: &mut Vec<u8>, n: i64, w: usize) {
    for k in (0..w).rev() {
        out.push(u8::try_from((n >> (8 * k)) & 0xFF).unwrap_or(0));
    }
}

impl Xref {
    /// The bytes of the section, beginning at byte `at` of the file;
    /// `offset(num)` is where object `num` begins, `placed(num)` the
    /// object stream and index of a [`XEntry::Placed`] one, and `deflate`
    /// compresses a stream at a level (`None`: stored blocks).
    pub fn render(
        &self,
        at: i64,
        offset: &mut dyn FnMut(i32) -> i64,
        placed: &mut dyn FnMut(i32) -> (i32, u8),
        deflate: &mut Deflate<'_>,
    ) -> Vec<u8> {
        let mut out = Vec::new();
        let mut value = |e: &XEntry, xref_num: i32| -> (u8, i64, u8) {
            match *e {
                XEntry::Free(link) => (0, i64::from(link), 255),
                XEntry::Byte(n) if n == xref_num => (1, at, 0),
                XEntry::Byte(n) => (1, offset(n), 0),
                XEntry::InStream(s, i) => (2, i64::from(s), i),
                XEntry::Placed(n) => {
                    let (s, i) = placed(n);
                    (2, i64::from(s), i)
                }
            }
        };
        if let Some(s) = &self.stream {
            {
                let width: usize = if at / 256 > 16_777_215 {
                    5
                } else if at > 16_777_215 {
                    4
                } else if at > 65535 {
                    3
                } else {
                    2
                };
                int(&mut out, i64::from(s.num));
                out.extend_from_slice(b" 0 obj\n<<\n/Type /XRef\n/Index [0 ");
                int(&mut out, i64::from(s.obj_ptr + 1));
                out.extend_from_slice(b"]\n/Size ");
                int(&mut out, i64::from(s.obj_ptr + 1));
                out.extend_from_slice(b"\n/W [1 ");
                int(&mut out, i64::try_from(width).unwrap_or(0));
                out.extend_from_slice(b" 1]\n");
                out.extend_from_slice(&self.tail);
                let mut data = Vec::with_capacity(self.entries.len() * (width + 2));
                for e in &self.entries {
                    let (t, v, x) = value(e, s.num);
                    data.push(t);
                    out_bytes(&mut data, v, width);
                    data.push(x);
                }
                // pdfTeX §685's `pdf_begin_stream` and `pdf_end_stream`,
                // the length written over the blanks after `/Length `
                let data = if s.level > 0 {
                    deflate(s.level, &data).unwrap_or_else(|| partex_engine::zlib::stored(&data))
                } else {
                    data
                };
                let mut length = alloc::format!("/Length {}", data.len()).into_bytes();
                length.resize(b"/Length           ".len().max(length.len()), b' ');
                out.extend_from_slice(&length);
                out.push(b'\n');
                if s.level > 0 {
                    out.extend_from_slice(b"/Filter /FlateDecode\n");
                }
                out.extend_from_slice(b">>\nstream\n");
                out.extend_from_slice(&data);
                out.extend_from_slice(b"\nendstream\nendobj\n");
            }
        } else {
            {
                out.extend_from_slice(b"xref\n0 ");
                int(&mut out, i64::try_from(self.entries.len()).unwrap_or(0));
                out.push(b'\n');
                for (k, e) in self.entries.iter().enumerate() {
                    let (t, v, _) = value(e, -1);
                    out.extend_from_slice(alloc::format!("{v:010}").as_bytes());
                    out.extend_from_slice(match (k, t) {
                        (0, _) => b" 65535 f \n",
                        (_, 0) => b" 00000 f \n",
                        _ => b" 00000 n \n",
                    });
                }
                out.extend_from_slice(&self.tail);
            }
        }
        out.extend_from_slice(b"startxref\n");
        int(&mut out, at);
        out.extend_from_slice(b"\n%%EOF\n");
        out
    }
}

partex_engine::persist_enum!(XEntry {
    Free(a0),
    Byte(a0),
    InStream(a0, a1),
    Placed(a0),
});
partex_engine::persist_struct!(XrefStream {
    num,
    obj_ptr,
    level
});
partex_engine::persist_struct!(Xref {
    stream,
    entries,
    tail
});
