//! Effects as values and the link step (DESIGN.md §5.5, §7.6).
//!
//! With effects on ([`Tex::set_effects`]) the engine's outputs do not go
//! to the host as they are produced: log and terminal text, DVI and PDF
//! bytes become [`Effect`] values, which the driver takes
//! ([`Tex::take_effects`]) per region and hands, in program order, to
//! [`link`]. Byte offsets are symbols: a PDF object's offset is where its
//! [`Effect::PdfObject`] mark lands, and the cross-reference section and
//! trailer ([`Effect::PdfXref`]) and the byte counts printed at the end of
//! the job ([`Effect::Length`]) are rendered once every offset is known,
//! as a linker resolves relocations. So a region's effects do not depend
//! on how much output came before it, and a rebuild can reuse them after
//! an edit that changed the length of earlier output.
//!
//! The link lays every file out by prefix sums over the lengths, renders
//! what refers to offsets, and copies the pieces into each file's buffer
//! in parallel (mold's plan). Its output is the file the direct path
//! writes, byte for byte.
//!
//! `\write` files are the exception: they go to the host as before. A job
//! may close a file and read it back (LaTeX's `texsys.aux`, any `.aux`
//! read at `\end{document}`), so what they hold is state, which the
//! host keeps (for the runtime, a file cell), not only output.
//!
//! What is not symbolic yet: object numbers (they are observable,
//! `\pdflastobj`, and appear inside compressed object streams; with holes,
//! §7.5, they become symbols too), and DVI bytes, whose movement
//! optimization depends on where the writer's half-buffer boundaries fall
//! (§607–§615), so a DVI page is not position-independent.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::exec::Executor;
use crate::host::{Host, WriteId};
pub use crate::pdf::xref::{Deflate, XEntry, Xref, XrefStream};

pub mod flow;
mod splice;
use crate::tex::Tex;
use crate::track::Tracker;
pub use splice::{Splice, SpliceOut, SpliceStats, StepChunks};

/// One output of the engine, as a value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Bytes appended to an output file.
    Write {
        file: WriteId,
        bytes: Vec<u8>,
    },
    /// Text for the terminal.
    Term(Vec<u8>),
    /// The file is complete.
    Close(WriteId),
    /// PDF object `num` begins `ahead` bytes after the bytes of `file`
    /// so far (the writer's buffered bytes, which come in later
    /// effects): its offset for the cross-reference section.
    PdfObject {
        file: WriteId,
        num: i32,
        ahead: u64,
    },
    /// The rest of PDF file `file`: its cross-reference section and
    /// trailer, rendered once the offsets are known.
    PdfXref {
        file: WriteId,
        xref: alloc::boxed::Box<Xref>,
    },
    /// `text`, written to `stream`, is the length of `file` as the engine
    /// printed it, taking it to be `assumed` bytes ("Output written on …
    /// bytes"; a line break may fall between the digits). The link writes
    /// the real length in its place: the same number of digits wraps the
    /// same way; a different number is a [`LinkError`]. (The digits are
    /// not in the stream's other bytes, so a reused region's stale guess
    /// never shows.)
    Length {
        stream: Stream,
        file: WriteId,
        assumed: i64,
        text: Vec<u8>,
    },
    /// Object `num` begins in the object stream being filled for PDF file
    /// `file` (symbolic object streams: what the stream holds is output,
    /// not state).
    ObjStmStart {
        file: WriteId,
        num: i32,
    },
    /// Bytes of that object.
    ObjStmBytes {
        file: WriteId,
        bytes: Vec<u8>,
    },
    /// Object stream `num` of `file` is complete: its object, from the
    /// objects begun since the last one, compressed at `level`, is
    /// rendered at the link where this effect is (its mark comes before
    /// it).
    ObjStm {
        file: WriteId,
        num: i32,
        level: i32,
    },
    /// A diagnostic for the host's side channel (`diag.rs`): an error,
    /// warning or note as data, in program order with the rest.
    Diagnostic(alloc::boxed::Box<crate::diag::Diagnostic>),
    /// A page is being shipped out, `\count0` its number (for progress).
    Shipping(i32),
    /// Virtual object numbers (`pdf/vnum.rs`): the digits of object
    /// `num`'s number in `file`, or in the object being written to an
    /// object stream, which the link writes.
    ObjRef {
        file: WriteId,
        num: i32,
    },
    ObjStmRef {
        file: WriteId,
        num: i32,
    },
    /// A numbering event, in program order: the link numbers objects by
    /// them, and closes object streams (at the compression level given,
    /// for `End` and `Flush`).
    Num(crate::pdf::vnum::NumEvent, i32),
    /// With fonts as cells (`PdfOut::font_refs`): font slot `slot` was
    /// loaded (next), which gives its number (tex.web's), in program
    /// order; the null font's is 0.
    FontLoad(i32),
    /// The digits of font slot `slot`'s number in `file` (a `/F` name),
    /// or in the object being written to an object stream.
    FontRef {
        file: WriteId,
        slot: i32,
    },
    ObjStmFontRef {
        file: WriteId,
        slot: i32,
    },
    /// The blanks for the length of the stream the next `Deflate` of
    /// `file` compresses (`pdf/out.rs`: `LENGTH_HOLE` bytes).
    StreamLength {
        file: WriteId,
    },
    /// A stream of `file`: its bytes with relocations (an object's number,
    /// a font's), compressed by the link at `level` (0: as they are).
    Deflate {
        file: WriteId,
        level: i32,
        parts: Vec<(Vec<u8>, Option<crate::pdf::out::Reloc>)>,
    },
    /// File `file` was opened for writing, by the name the engine asked
    /// for and its kind: with the build's files linked from its steps
    /// (DESIGN 7.17.3), the link opens each file by that name.
    Open {
        file: WriteId,
        name: Vec<u8>,
        kind: crate::host::FileKind,
    },
    /// The glyphs a page's or form's content stream shows, with glyph
    /// origins on (`srcmap.rs`, DESIGN 4.4): no bytes of any file.
    Origins(crate::srcmap::StreamOrgs),
    /// Text for the terminal and the log (`log`), relative to their
    /// columns: rendered by the link from the columns the text before it
    /// left ([`flow::render`], DESIGN 3.8).
    Flow {
        log: Option<WriteId>,
        ops: Vec<u8>,
    },
}

/// Where text goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stream {
    File(WriteId),
    Term,
}

/// The files and terminal text a run's effects make.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Linked {
    /// Each file's bytes, by the id the host gave it.
    pub files: BTreeMap<u32, Vec<u8>>,
    /// The files that were closed.
    pub closed: Vec<WriteId>,
    pub term: Vec<u8>,
    /// The diagnostics, in program order.
    pub diagnostics: Vec<crate::diag::Diagnostic>,
    /// The pages shipped out, by `\count0`, in order.
    pub pages: Vec<i32>,
    /// The files opened, in order: each one's id, name and kind.
    pub opened: Vec<(WriteId, Vec<u8>, crate::host::FileKind)>,
}

/// Why effects could not be linked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkError {
    /// A byte count printed by the engine has a different number of
    /// digits than the real one: the text around it would wrap
    /// differently, so the region that printed it must run again.
    LengthDigits {
        file: WriteId,
        assumed: i64,
        actual: i64,
    },
    /// An object's offset is needed but no mark placed it.
    Unplaced { file: WriteId, num: i32 },
}

/// A byte count to fill in: where (a file's piece `Ok(owned index)`, or
/// `Err(terminal offset)`), whose length, the guess and the printed text.
type LengthAt<'a> = (Result<usize, usize>, WriteId, i64, &'a [u8]);

/// A piece of a file: bytes from an effect, or rendered at the link.
enum Piece<'a> {
    Bytes(&'a [u8]),
    Owned(usize),
}

impl<H: Host, T: Tracker> Tex<H, T> {
    /// Turn effects on or off (off, outputs go to the host as they are
    /// produced). Switching on drops nothing; switching off hands nothing
    /// pending to the host.
    pub fn set_effects(&mut self, on: bool) {
        self.effects = on.then(Vec::new);
        self.pdf.out.symbolic = on;
    }

    /// With effects on, whether object streams are symbolic (on by
    /// default): their objects go out as effects and the link lays the
    /// stream out, so that what a stream holds is not state.
    pub fn set_symbolic_object_streams(&mut self, on: bool) {
        self.pdf.out.symbolic = on && self.effects.is_some();
        self.pdf.objs.symbolic = self.pdf.out.symbolic;
    }

    /// Hash references to the strings the run made by their characters
    /// (a machine's `Rest`: see `Tex::canon_strings`).
    pub fn set_canon_strings(&mut self, on: bool) {
        self.canon_strings = on;
    }

    /// Place control sequences made in the `hash_extra` region by their
    /// names (see `Tex::cs_by_name`).
    pub fn set_cs_by_name(&mut self, on: bool) {
        self.cs_by_name = on;
    }

    /// Make the hash table's names a machine's cells (see
    /// `Tex::name_cells`); with names placed by name only.
    pub fn set_name_cells(&mut self, on: bool) {
        self.name_cells = on;
    }

    /// Place and find the run's names by probing (see `Tex::probe_names`;
    /// with names as cells and placed by name).
    pub fn set_probe_names(&mut self, on: bool) {
        self.probe_names = on;
    }

    /// Make the fonts a machine's cells (see `Tex::font_cells`); with
    /// virtual object numbers, fonts' `/F` names are relocations and
    /// streams are compressed by the link.
    pub fn set_font_cells(&mut self, on: bool) {
        self.font_cells = on;
        self.pdf.out.font_refs = on;
    }

    /// With effects on, make the PDF object table's entries, lookup trees
    /// and destination names a machine's cells (`pdf/objtab.rs`:
    /// `ObjLog`), not `Rest`'s.
    pub fn set_obj_cells(&mut self, on: bool) {
        self.pdf.objs.log.on = on && self.effects.is_some();
    }

    /// With the object table as cells, virtual object numbers
    /// (`pdf/vnum.rs`); before any object is made.
    pub fn set_virtual_objects(&mut self, on: bool) {
        let on = on && self.pdf.objs.log.on;
        self.pdf.objs.set_virt(on);
        self.pdf.out.virt = on;
    }

    /// Whether effects are on.
    pub fn effects_on(&self) -> bool {
        self.effects.is_some()
    }

    /// The effects produced since the last call (everything buffered is
    /// flushed first, so a region's effects are complete at its end).
    pub fn take_effects(&mut self) -> Vec<Effect> {
        if self.effects.is_none() {
            return Vec::new();
        }
        self.pdf_os_emit();
        self.flush_outputs();
        self.effects
            .as_mut()
            .map(core::mem::take)
            .unwrap_or_default()
    }

    /// Bytes for output file `id`: an effect, or straight to the host.
    pub(crate) fn out_write(&mut self, id: WriteId, bytes: &[u8]) {
        match &mut self.effects {
            Some(e) => {
                if bytes.is_empty() {
                    return;
                }
                if let Some(Effect::Write { file, bytes: last }) = e.last_mut()
                    && *file == id
                {
                    last.extend_from_slice(bytes);
                } else {
                    e.push(Effect::Write {
                        file: id,
                        bytes: bytes.to_vec(),
                    });
                }
            }
            None => self.host.write(id, bytes),
        }
    }

    /// `Host::open_write`; with effects on, the file's open is an effect
    /// too, naming it for the link. A step that runs again opens the file
    /// on the handle its last run had, if the host can (`Tracker::reopen`).
    pub(crate) fn open_out(
        &mut self,
        name: &[u8],
        kind: crate::host::FileKind,
    ) -> Option<(WriteId, Vec<u8>)> {
        let r = match self.tracker.reopen(name, kind) {
            Some(id) => self.host.open_write_again(name, kind, id),
            None => self.host.open_write(name, kind),
        };
        if let (Some((id, _)), Some(e)) = (&r, &mut self.effects) {
            e.push(Effect::Open {
                file: *id,
                name: name.to_vec(),
                kind,
            });
        }
        r
    }

    /// Bytes for `\write` file `id`: to the host at once, since the job
    /// may read the file back, and, when the build links its files from
    /// its steps (effects on with values, DESIGN 7.17.3), an effect too.
    pub(crate) fn write_file_bytes(&mut self, id: WriteId, bytes: &[u8]) {
        self.host.write(id, bytes);
        if !T::VALUES || bytes.is_empty() {
            return;
        }
        if let Some(e) = &mut self.effects {
            if let Some(Effect::Write { file, bytes: last }) = e.last_mut()
                && *file == id
            {
                last.extend_from_slice(bytes);
            } else {
                e.push(Effect::Write {
                    file: id,
                    bytes: bytes.to_vec(),
                });
            }
        }
    }

    /// `\write` file `id` is complete (and, linked, an effect).
    pub(crate) fn write_file_close(&mut self, id: WriteId) {
        self.host.close(id);
        if T::VALUES
            && let Some(e) = &mut self.effects
        {
            e.push(Effect::Close(id));
        }
    }

    /// Output file `id` is complete.
    pub(crate) fn out_close(&mut self, id: WriteId) {
        match &mut self.effects {
            Some(e) => e.push(Effect::Close(id)),
            None => self.host.close(id),
        }
    }

    /// Terminal text.
    pub(crate) fn out_term(&mut self, bytes: &[u8]) {
        match &mut self.effects {
            Some(e) => {
                if let Some(Effect::Term(last)) = e.last_mut() {
                    last.extend_from_slice(bytes);
                } else {
                    e.push(Effect::Term(bytes.to_vec()));
                }
            }
            None => self.host.term_write(bytes),
        }
    }

    /// A diagnostic for the host.
    pub(crate) fn out_diagnostic(&mut self, d: crate::diag::Diagnostic) {
        match &mut self.effects {
            Some(e) => e.push(Effect::Diagnostic(alloc::boxed::Box::new(d))),
            None => self.host.diagnostic(&d),
        }
    }

    /// Push a symbolic effect (effects on only).
    pub(crate) fn out_mark(&mut self, e: Effect) {
        if let Some(v) = &mut self.effects {
            v.push(e);
        }
    }
}

/// An object stream being filled: where its objects begin, and their
/// bytes.
type ObjStmFill = (Vec<(i32, usize)>, Vec<u8>);

/// Object stream `num` with the objects `objs` (numbers and where each
/// begins in `data`), as pdfTeX writes it (§699 `pdf_os_write_objstream`,
/// then `pdf_begin_dict`, `pdf_begin_stream` and `pdf_end_stream`).
fn render_objstm(
    num: i32,
    level: i32,
    objs: &[(i32, usize)],
    mut data: Vec<u8>,
    deflate: &mut crate::pdf::xref::Deflate<'_>,
) -> Vec<u8> {
    use core::fmt::Write as _;
    let mut head = alloc::string::String::new();
    for (j, (n, off)) in objs.iter().enumerate() {
        let _ = write!(head, "{n} {off}");
        head.push(if j % 10 == 9 { '\n' } else { ' ' });
    }
    let mut head = head.into_bytes();
    if let Some(last) = head.last_mut() {
        *last = b'\n';
    } else if let Some(last) = data.last_mut() {
        *last = b'\n';
    }
    let first = head.len();
    head.append(&mut data);
    let stream = if level > 0 {
        deflate(level, &head).unwrap_or_else(|| partex_engine::zlib::stored(&head))
    } else {
        head
    };
    let length = alloc::format!("{}", stream.len());
    let mut out = alloc::format!(
        "{num} 0 obj\n<<\n/Type /ObjStm\n/N {}\n/First {first}\n/Length {length}{}\n",
        objs.len(),
        " ".repeat(10usize.saturating_sub(length.len()))
    )
    .into_bytes();
    if level > 0 {
        out.extend_from_slice(b"/Filter /FlateDecode\n");
    }
    out.extend_from_slice(b">>\nstream\n");
    out.extend_from_slice(&stream);
    out.extend_from_slice(b"\nendstream\nendobj\n");
    out
}

/// With virtual object numbers (`pdf/vnum.rs`): pdfTeX's numbering,
/// replayed from the `Num` events of every region, and the fonts' numbers
/// (the order they were loaded in, the null font 0). `None` without
/// virtual numbers.
fn numbering_of(chunks: &[&[Effect]]) -> Option<(crate::pdf::vnum::Numbering, BTreeMap<i32, i32>)> {
    let all = || chunks.iter().flat_map(|c| c.iter());
    if !all().any(|e| matches!(e, Effect::Num(..))) {
        return None;
    }
    let mut n = crate::pdf::vnum::Numbering::default();
    let mut fonts: BTreeMap<i32, i32> = BTreeMap::new();
    for e in all() {
        match e {
            Effect::Num(ev, _) => {
                n.step(*ev);
            }
            Effect::FontLoad(f) if *f != 0 => {
                let k = i32::try_from(fonts.len()).unwrap_or(i32::MAX) + 1;
                fonts.insert(*f, k);
            }
            _ => {}
        }
    }
    Some((n, fonts))
}

/// The numbers `fx`'s resolution writes, in order (objects' pdfTeX
/// numbers, fonts' numbers): what else its resolved effects depend on.
fn region_inputs(
    fx: &[Effect],
    n: &crate::pdf::vnum::Numbering,
    fonts: &BTreeMap<i32, i32>,
) -> Vec<i32> {
    use crate::pdf::out::Reloc;
    let font = |f: i32| fonts.get(&f).copied().unwrap_or(f);
    let mut v = Vec::new();
    for e in fx {
        match e {
            Effect::ObjRef { num, .. }
            | Effect::ObjStmRef { num, .. }
            | Effect::PdfObject { num, .. }
            | Effect::ObjStmStart { num, .. } => v.push(n.of(*num)),
            Effect::FontRef { slot, .. } | Effect::ObjStmFontRef { slot, .. } => {
                v.push(font(*slot));
            }
            Effect::Deflate { parts, .. } => {
                for (_, r) in parts {
                    match r {
                        Some(Reloc::Obj(k)) => v.push(n.of(*k)),
                        Some(Reloc::Font(f)) => v.push(font(*f)),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    v
}

/// The effects `fx` with pdfTeX's numbers (`numbering_of`): each
/// relocation written as its digits, objects renumbered, and each object
/// stream closed where its hundredth object ends (or the job does), as
/// its object's mark and `ObjStm` (`again`, the numbering so far, and
/// `file`, whose object stream is being filled, carry from region to
/// region). With it, whether a stream's length is still to be written
/// (its stream in a later region).
fn resolve_region<'a>(
    fx: impl Iterator<Item = &'a Effect>,
    (n, fonts): (&crate::pdf::vnum::Numbering, &BTreeMap<i32, i32>),
    again: &mut crate::pdf::vnum::Numbering,
    file: &mut Option<WriteId>,
    deflate: &mut crate::pdf::xref::Deflate<'_>,
) -> (Vec<Effect>, bool) {
    use crate::pdf::out::{LENGTH_HOLE, Reloc};
    use crate::pdf::vnum::Stream as S;
    let font = |f: i32| fonts.get(&f).copied().unwrap_or(f);
    let digits = |k: i32| alloc::format!("{}", n.of(k)).into_bytes();
    let reloc = |r: Reloc| match r {
        Reloc::Obj(k) => digits(k),
        Reloc::Font(f) => alloc::format!("{}", font(f)).into_bytes(),
        Reloc::Length => Vec::new(),
    };
    // (where each file's stream length goes, once its stream is
    // compressed: an index into `out`)
    let mut holes: BTreeMap<u32, usize> = BTreeMap::new();
    let mut out = Vec::new();
    for e in fx {
        match e {
            Effect::Num(ev, level) => {
                if let (S::Closed(num), Some(file)) = (again.step(*ev), *file) {
                    out.push(Effect::PdfObject {
                        file,
                        num,
                        ahead: 0,
                    });
                    out.push(Effect::ObjStm {
                        file,
                        num,
                        level: *level,
                    });
                }
            }
            Effect::ObjRef { file, num } => {
                let bytes = digits(*num);
                if let Some(Effect::Write {
                    file: f,
                    bytes: last,
                }) = out.last_mut()
                    && f == file
                {
                    last.extend_from_slice(&bytes);
                } else {
                    out.push(Effect::Write { file: *file, bytes });
                }
            }
            Effect::ObjStmRef { file, num } => out.push(Effect::ObjStmBytes {
                file: *file,
                bytes: digits(*num),
            }),
            Effect::ObjStmFontRef { file, slot } => out.push(Effect::ObjStmBytes {
                file: *file,
                bytes: alloc::format!("{}", font(*slot)).into_bytes(),
            }),
            Effect::FontRef { file, slot } => {
                let bytes = alloc::format!("{}", font(*slot)).into_bytes();
                if let Some(Effect::Write {
                    file: f,
                    bytes: last,
                }) = out.last_mut()
                    && f == file
                {
                    last.extend_from_slice(&bytes);
                } else {
                    out.push(Effect::Write { file: *file, bytes });
                }
            }
            Effect::StreamLength { file } => {
                holes.insert(file.0, out.len());
                out.push(Effect::Write {
                    file: *file,
                    bytes: alloc::vec![b' '; LENGTH_HOLE],
                });
            }
            Effect::Deflate { file, level, parts } => {
                let mut data = Vec::new();
                for (b, r) in parts {
                    data.extend_from_slice(b);
                    if let Some(r) = r {
                        data.extend_from_slice(&reloc(*r));
                    }
                }
                let z = if *level > 0 {
                    deflate(*level, &data).unwrap_or_else(|| partex_engine::zlib::stored(&data))
                } else {
                    data
                };
                // (pdfTeX writes the length over the blanks, from the
                // first)
                if let Some(i) = holes.remove(&file.0)
                    && let Some(Effect::Write { bytes, .. }) = out.get_mut(i)
                {
                    for (d, c) in bytes.iter_mut().zip(alloc::format!("{}", z.len()).bytes()) {
                        *d = c;
                    }
                }
                out.push(Effect::Write {
                    file: *file,
                    bytes: z,
                });
            }
            Effect::FontLoad(_) => {}
            Effect::PdfObject { file, num, ahead } => out.push(Effect::PdfObject {
                file: *file,
                num: n.of(*num),
                ahead: *ahead,
            }),
            Effect::ObjStmStart { file: f, num } => {
                *file = Some(*f);
                out.push(Effect::ObjStmStart {
                    file: *f,
                    num: n.of(*num),
                });
            }
            Effect::Write { file: f, bytes } => {
                if let Some(Effect::Write {
                    file: g,
                    bytes: last,
                }) = out.last_mut()
                    && g == f
                {
                    last.extend_from_slice(bytes);
                } else {
                    out.push(e.clone());
                }
            }
            _ => out.push(e.clone()),
        }
    }
    (out, !holes.is_empty())
}

/// With virtual object numbers (`pdf/vnum.rs`), the effects with
/// pdfTeX's numbers ([`resolve_region`] over them all). `None` without
/// virtual numbers.
fn resolve_numbers(
    chunks: &[&[Effect]],
    deflate: &mut crate::pdf::xref::Deflate<'_>,
) -> Option<Vec<Effect>> {
    let (n, fonts) = numbering_of(chunks)?;
    let mut again = crate::pdf::vnum::Numbering::default();
    let mut file = None;
    let (out, _) = resolve_region(
        chunks.iter().flat_map(|c| c.iter()),
        (&n, &fonts),
        &mut again,
        &mut file,
        deflate,
    );
    Some(out)
}

/// Link the effects of a run, `chunks` in program order, into files.
/// `deflate` compresses a stream at a level (a cross-reference stream is
/// rendered here), `None` for zlib's stored blocks.
///
/// # Errors
///
/// When a byte count's digits changed ([`LinkError::LengthDigits`]) or an
/// offset has no mark.
pub fn link<E: Executor>(
    chunks: &[&[Effect]],
    exec: &E,
    deflate: &mut crate::pdf::xref::Deflate<'_>,
) -> Result<Linked, LinkError> {
    // (virtual object numbers: pdfTeX's numbers written in first)
    match resolve_numbers(chunks, deflate) {
        Some(v) => layout(&[v.as_slice()], exec, deflate),
        None => layout(chunks, exec, deflate),
    }
}

/// Each region's effects with pdfTeX's numbers, as the last
/// [`link_cached`] resolved them, and what that depended on.
#[derive(Clone, Debug, Default)]
pub struct LinkCache {
    regions: BTreeMap<u64, Resolved>,
    /// Regions taken from the cache by the last link, and resolved again.
    pub reused: usize,
    pub resolved: usize,
}

#[derive(Clone, Debug)]
struct Resolved {
    /// The object stream counters at the region's entry (`sys`, `cur`,
    /// `idx`) and the file whose stream is filled.
    entry: (i32, i32, i32, Option<u32>),
    /// The numbers it writes ([`region_inputs`]).
    inputs: Vec<i32>,
    out: Vec<Effect>,
}

/// [`link`], each region's resolved effects kept in `cache` by its key:
/// a region whose effects did not change since (`touched` says it did)
/// and whose entry counters and written numbers are the same is taken
/// from the cache, so resolving costs the regions that changed (and a
/// look at the numbers each one writes), not every stream's bytes again.
/// The same output as [`link`] of the regions' effects.
///
/// # Errors
///
/// As [`link`].
pub fn link_cached<E: Executor>(
    chunks: &[(u64, &[Effect])],
    touched: &dyn Fn(u64) -> bool,
    cache: &mut LinkCache,
    exec: &E,
    deflate: &mut crate::pdf::xref::Deflate<'_>,
) -> Result<Linked, LinkError> {
    link_cached_timed(
        chunks,
        touched,
        cache,
        exec,
        deflate,
        &|| 0,
        &mut LinkTimes::default(),
    )
}

/// What a link's phases cost ([`link_cached_timed`]), in the clock's
/// units: the numbering (`numbering_of`), resolving the regions, the
/// layout's pass over the effects (object streams and the
/// cross-reference section rendered, compressed), the byte counts, and
/// the copy into the files' buffers.
#[derive(Clone, Copy, Debug, Default)]
pub struct LinkTimes {
    pub numbering: u64,
    pub resolve: u64,
    pub layout: u64,
    /// Within `layout`: the object streams and the cross-reference
    /// section rendered.
    pub objstm: u64,
    pub xref: u64,
    pub lengths: u64,
    pub copy: u64,
}

/// [`link_cached`], its phases timed by `clock` into `times`.
///
/// # Errors
///
/// As [`link`].
pub fn link_cached_timed<E: Executor>(
    chunks: &[(u64, &[Effect])],
    touched: &dyn Fn(u64) -> bool,
    cache: &mut LinkCache,
    exec: &E,
    deflate: &mut crate::pdf::xref::Deflate<'_>,
    clock: &dyn Fn() -> u64,
    times: &mut LinkTimes,
) -> Result<Linked, LinkError> {
    let t0 = clock();
    let all: Vec<&[Effect]> = chunks.iter().map(|(_, c)| *c).collect();
    let numbered = numbering_of(&all);
    let t1 = clock();
    times.numbering += t1 - t0;
    let Some((n, fonts)) = numbered else {
        cache.regions.clear();
        return layout_timed(&all, exec, deflate, clock, times);
    };
    let mut again = crate::pdf::vnum::Numbering::default();
    let mut file: Option<WriteId> = None;
    let mut next: BTreeMap<u64, Resolved> = BTreeMap::new();
    let (mut reused, mut resolved) = (0, 0);
    for &(k, fx) in chunks {
        let entry = (again.sys, again.cur, again.idx, file.map(|f| f.0));
        let inputs = region_inputs(fx, &n, &fonts);
        if let Some(r) = cache.regions.remove(&k)
            && !touched(k)
            && r.entry == entry
            && r.inputs == inputs
        {
            // (the counters and the file moved on as its resolution did)
            for e in fx {
                match e {
                    Effect::Num(ev, _) => {
                        again.step(*ev);
                    }
                    Effect::ObjStmStart { file: f, .. } => file = Some(*f),
                    _ => {}
                }
            }
            next.insert(k, r);
            reused += 1;
            continue;
        }
        let (out, pending) =
            resolve_region(fx.iter(), (&n, &fonts), &mut again, &mut file, deflate);
        if pending {
            // (a stream's length in one region and its stream in the next:
            // not cut, so resolved as one)
            cache.regions.clear();
            return link(&all, exec, deflate);
        }
        next.insert(k, Resolved { entry, inputs, out });
        resolved += 1;
    }
    cache.regions = next;
    cache.reused = reused;
    cache.resolved = resolved;
    let slices: Vec<&[Effect]> = chunks
        .iter()
        .filter_map(|(k, _)| cache.regions.get(k).map(|r| r.out.as_slice()))
        .collect();
    times.resolve += clock() - t1;
    layout_timed(&slices, exec, deflate, clock, times)
}

/// The files of effects whose numbers are resolved: offsets and byte
/// counts by prefix sums, object streams and the cross-reference section
/// rendered, the pieces copied.
fn layout<E: Executor>(
    chunks: &[&[Effect]],
    exec: &E,
    deflate: &mut crate::pdf::xref::Deflate<'_>,
) -> Result<Linked, LinkError> {
    layout_timed(chunks, exec, deflate, &|| 0, &mut LinkTimes::default())
}

/// [`layout`], its phases timed.
fn layout_timed<E: Executor>(
    chunks: &[&[Effect]],
    exec: &E,
    deflate: &mut crate::pdf::xref::Deflate<'_>,
    clock: &dyn Fn() -> u64,
    times: &mut LinkTimes,
) -> Result<Linked, LinkError> {
    let t0 = clock();
    // 1. Layout: every file's pieces and length, object offsets and the
    //    positions of byte counts, in one pass over the effects (not the
    //    bytes).
    let mut pieces: BTreeMap<u32, Vec<Piece<'_>>> = BTreeMap::new();
    let mut sizes: BTreeMap<u32, usize> = BTreeMap::new();
    let mut offsets: BTreeMap<(u32, i32), i64> = BTreeMap::new();
    let mut owned: Vec<Vec<u8>> = Vec::new();
    let mut term: Vec<u8> = Vec::new();
    let mut diagnostics = Vec::new();
    let mut pages = Vec::new();
    // (where a byte count goes, filled in once every length is known: a
    // file's piece `Ok(owned index)`, or `Err(terminal offset)`)
    let mut lengths: Vec<LengthAt<'_>> = Vec::new();
    let mut closed = Vec::new();
    let mut opened = Vec::new();
    // (each file's object stream being filled: where its objects begin,
    // and their bytes)
    let mut objstm: BTreeMap<u32, ObjStmFill> = BTreeMap::new();
    // (where each object in a stream went: the stream's number and index)
    let mut placed: BTreeMap<(u32, i32), (i32, u8)> = BTreeMap::new();
    for e in chunks.iter().flat_map(|c| c.iter()) {
        match e {
            Effect::Write { file, bytes } => {
                pieces.entry(file.0).or_default().push(Piece::Bytes(bytes));
                *sizes.entry(file.0).or_default() += bytes.len();
            }
            Effect::Term(t) => term.extend_from_slice(t),
            Effect::Diagnostic(d) => diagnostics.push((**d).clone()),
            Effect::Shipping(c) => pages.push(*c),
            Effect::Close(f) => closed.push(*f),
            Effect::Open { file, name, kind } => opened.push((*file, name.clone(), *kind)),
            // (glyph origins: read by `Tex::origins`, not linked; the rest
            // resolved before: `resolve_numbers`)
            Effect::Origins(_)
            | Effect::Flow { .. }
            | Effect::ObjRef { .. }
            | Effect::ObjStmRef { .. }
            | Effect::Num(..)
            | Effect::FontLoad(_)
            | Effect::FontRef { .. }
            | Effect::ObjStmFontRef { .. }
            | Effect::StreamLength { .. }
            | Effect::Deflate { .. } => {}
            Effect::ObjStmStart { file, num } => {
                let (objs, data) = objstm.entry(file.0).or_default();
                objs.push((*num, data.len()));
            }
            Effect::ObjStmBytes { file, bytes } => {
                objstm.entry(file.0).or_default().1.extend_from_slice(bytes);
            }
            Effect::ObjStm { file, num, level } => {
                let (objs, data) = objstm.remove(&file.0).unwrap_or_default();
                for (i, (obj, _)) in objs.iter().enumerate() {
                    placed.insert((file.0, *obj), (*num, u8::try_from(i).unwrap_or(0)));
                }
                let t = clock();
                let bytes = render_objstm(*num, *level, &objs, data, deflate);
                times.objstm += clock() - t;
                *sizes.entry(file.0).or_default() += bytes.len();
                pieces
                    .entry(file.0)
                    .or_default()
                    .push(Piece::Owned(owned.len()));
                owned.push(bytes);
            }
            Effect::PdfObject { file, num, ahead } => {
                let at = sizes.get(&file.0).copied().unwrap_or(0) as u64 + ahead;
                offsets.insert((file.0, *num), i64::try_from(at).unwrap_or(i64::MAX));
            }
            Effect::PdfXref { file, xref } => {
                let at = i64::try_from(sizes.get(&file.0).copied().unwrap_or(0)).unwrap_or(0);
                let missing = core::cell::Cell::new(None);
                let note = |num| missing.set(missing.get().or(Some(num)));
                let t = clock();
                let bytes = xref.render(
                    at,
                    &mut |num| {
                        let o = offsets.get(&(file.0, num)).copied();
                        if o.is_none() {
                            note(num);
                        }
                        o.unwrap_or(0)
                    },
                    &mut |num| {
                        let p = placed.get(&(file.0, num)).copied();
                        if p.is_none() {
                            note(num);
                        }
                        p.unwrap_or((0, 0))
                    },
                    deflate,
                );
                times.xref += clock() - t;
                if let Some(num) = missing.get() {
                    return Err(LinkError::Unplaced { file: *file, num });
                }
                *sizes.entry(file.0).or_default() += bytes.len();
                pieces
                    .entry(file.0)
                    .or_default()
                    .push(Piece::Owned(owned.len()));
                owned.push(bytes);
            }
            Effect::Length {
                stream,
                file,
                assumed,
                text,
            } => match stream {
                Stream::File(f) => {
                    *sizes.entry(f.0).or_default() += text.len();
                    pieces
                        .entry(f.0)
                        .or_default()
                        .push(Piece::Owned(owned.len()));
                    lengths.push((Ok(owned.len()), *file, *assumed, text));
                    owned.push(Vec::new());
                }
                Stream::Term => {
                    lengths.push((Err(term.len()), *file, *assumed, text));
                    term.extend_from_slice(text);
                }
            },
        }
    }
    let t1 = clock();
    times.layout += t1 - t0;
    // 2. Byte counts, now that every length is known (the same number of
    //    digits, so no size above changes).
    for (place, file, assumed, text) in lengths {
        let actual = i64::try_from(sizes.get(&file.0).copied().unwrap_or(0)).unwrap_or(i64::MAX);
        let (old, new) = (
            alloc::format!("{assumed}").into_bytes(),
            alloc::format!("{actual}").into_bytes(),
        );
        if old.len() != new.len() {
            return Err(LinkError::LengthDigits {
                file,
                assumed,
                actual,
            });
        }
        let mut digits = new.iter();
        let real: Vec<u8> = text
            .iter()
            .map(|&b| {
                if b == b'\n' {
                    b
                } else {
                    digits.next().copied().unwrap_or(b)
                }
            })
            .collect();
        match place {
            Ok(i) => owned[i] = real,
            Err(at) => term[at..at + real.len()].copy_from_slice(&real),
        }
    }
    let t2 = clock();
    times.lengths += t2 - t1;
    // 3. Copy every piece into its file, in parallel.
    let mut files: BTreeMap<u32, Vec<u8>> = sizes
        .iter()
        .map(|(&f, &n)| (f, alloc::vec![0u8; n]))
        .collect();
    let mut jobs: Vec<(&mut [u8], &[u8])> = Vec::new();
    let mut total = 0;
    for (f, buf) in &mut files {
        let mut rest: &mut [u8] = buf;
        for p in pieces.get(f).map_or(&[][..], Vec::as_slice) {
            let src: &[u8] = match p {
                Piece::Bytes(b) => b,
                Piece::Owned(i) => &owned[*i],
            };
            let (dst, tail) = rest.split_at_mut(src.len());
            jobs.push((dst, src));
            rest = tail;
            total += src.len();
        }
    }
    if total >= 1 << 20 {
        exec.map(jobs, |(dst, src)| dst.copy_from_slice(src));
    } else {
        for (dst, src) in jobs {
            dst.copy_from_slice(src);
        }
    }
    times.copy += clock() - t2;
    Ok(Linked {
        files,
        closed,
        term,
        diagnostics,
        pages,
        opened,
    })
}

// (effects outlive the process in a persisted build: `machine_store.rs`)
partex_engine::persist_enum!(Stream { File(a0), Term });
partex_engine::persist_enum!(Effect {
    Write { file, bytes },
    Term(a0),
    Close(a0),
    PdfObject { file, num, ahead },
    PdfXref { file, xref },
    Length {
        stream,
        file,
        assumed,
        text
    },
    ObjStmStart { file, num },
    ObjStmBytes { file, bytes },
    ObjStm { file, num, level },
    Diagnostic(a0),
    Shipping(a0),
    ObjRef { file, num },
    ObjStmRef { file, num },
    Num(a0, a1),
    FontLoad(a0),
    FontRef { file, slot },
    ObjStmFontRef { file, slot },
    StreamLength { file },
    Deflate { file, level, parts },
    Open { file, name, kind },
    Origins(a0),
    Flow { log, ops },
});

#[cfg(test)]
mod persist_tests {
    use super::{Effect, Stream};
    use crate::host::WriteId;
    use crate::pdf::vnum::NumEvent;
    use partex_engine::persist::{Loader, Persist, Saver};

    /// Every kind of effect survives a save and load (a persisted build
    /// holds them): a new kind fails the count below until it is here.
    #[test]
    fn every_effect_round_trips() {
        let f = WriteId(3);
        let all = alloc::vec![
            Effect::Write {
                file: f,
                bytes: b"ab".to_vec()
            },
            Effect::Term(b"t".to_vec()),
            Effect::Close(f),
            Effect::PdfObject {
                file: f,
                num: 4,
                ahead: 9
            },
            Effect::PdfXref {
                file: f,
                xref: alloc::boxed::Box::new(crate::pdf::xref::Xref {
                    stream: None,
                    entries: alloc::vec::Vec::new(),
                    tail: b"trailer".to_vec(),
                }),
            },
            Effect::Length {
                stream: Stream::Term,
                file: f,
                assumed: 12,
                text: b"12".to_vec()
            },
            Effect::ObjStmStart { file: f, num: 5 },
            Effect::ObjStmBytes {
                file: f,
                bytes: b"x".to_vec()
            },
            Effect::ObjStm {
                file: f,
                num: 6,
                level: 9
            },
            Effect::Diagnostic(alloc::boxed::Box::new(crate::diag::Diagnostic {
                severity: crate::diag::Severity::Error,
                code: "undefined-control-sequence",
                message: b"m".to_vec(),
                help: alloc::vec![b"h".to_vec()],
                frames: alloc::vec::Vec::new(),
                suggestions: alloc::vec::Vec::new(),
                boxed: None,
            })),
            Effect::Shipping(2),
            Effect::ObjRef { file: f, num: 7 },
            Effect::ObjStmRef { file: f, num: 8 },
            Effect::Num(NumEvent::Create(3), 0),
            Effect::Num(NumEvent::Start, 0),
            Effect::Num(NumEvent::End, 9),
            Effect::Num(NumEvent::Flush, 9),
            Effect::FontLoad(12),
            Effect::FontRef { file: f, slot: 12 },
            Effect::ObjStmFontRef { file: f, slot: 13 },
            Effect::StreamLength { file: f },
            Effect::Deflate {
                file: f,
                level: 9,
                parts: alloc::vec![
                    (b"BT /F".to_vec(), Some(crate::pdf::out::Reloc::Font(12))),
                    (b" 9 Tf".to_vec(), Some(crate::pdf::out::Reloc::Obj(7))),
                    (b"ET".to_vec(), None),
                ],
            },
            Effect::Open {
                file: f,
                name: b"x.aux".to_vec(),
                kind: crate::host::FileKind::Other,
            },
            Effect::Origins(crate::srcmap::StreamOrgs {
                form: 4,
                glyphs: alloc::vec![1, 2, crate::srcmap::FORM | 7].into(),
            }),
            Effect::Flow {
                log: Some(f),
                ops: alloc::vec![super::flow::NL, 3],
            },
        ];
        let mut kinds = alloc::collections::BTreeSet::new();
        for e in &all {
            kinds.insert(match e {
                Effect::Write { .. } => 0,
                Effect::Term(_) => 1,
                Effect::Close(_) => 2,
                Effect::PdfObject { .. } => 3,
                Effect::PdfXref { .. } => 4,
                Effect::Length { .. } => 5,
                Effect::ObjStmStart { .. } => 6,
                Effect::ObjStmBytes { .. } => 7,
                Effect::ObjStm { .. } => 8,
                Effect::Diagnostic(_) => 9,
                Effect::Shipping(_) => 10,
                Effect::ObjRef { .. } => 11,
                Effect::ObjStmRef { .. } => 12,
                Effect::Num(..) => 13,
                Effect::FontLoad(_) => 14,
                Effect::FontRef { .. } => 15,
                Effect::ObjStmFontRef { .. } => 16,
                Effect::StreamLength { .. } => 17,
                Effect::Deflate { .. } => 18,
                Effect::Open { .. } => 19,
                Effect::Origins(_) => 20,
                Effect::Flow { .. } => 21,
            });
        }
        assert_eq!(kinds.len(), 22);
        let mut s = Saver::new();
        all.save(&mut s);
        let bytes = s.into_bytes();
        let back: alloc::vec::Vec<Effect> = Persist::load(&mut Loader::new(&bytes)).expect("loads");
        assert_eq!(back, all);
    }
}

#[cfg(test)]
mod cached_tests {
    use alloc::vec;
    use alloc::vec::Vec;

    use super::{Effect, LinkCache, link, link_cached};
    use crate::host::WriteId;
    use crate::pdf::vnum::NumEvent;

    fn text(b: &[u8]) -> Effect {
        Effect::Write {
            file: WriteId(1),
            bytes: b.to_vec(),
        }
    }

    fn obj(v: i32) -> Effect {
        Effect::Num(NumEvent::Create(v), 0)
    }

    fn cite(v: i32) -> Effect {
        Effect::ObjRef {
            file: WriteId(1),
            num: v,
        }
    }

    fn linked(fx: &[(u64, Vec<Effect>)], touched: &[u64], cache: &mut LinkCache) -> Vec<u8> {
        let chunks: Vec<(u64, &[Effect])> = fx.iter().map(|(k, v)| (*k, v.as_slice())).collect();
        let plain: Vec<&[Effect]> = fx.iter().map(|(_, v)| v.as_slice()).collect();
        let mut deflate = |_: i32, d: &[u8]| Some(d.to_vec());
        let want = link(&plain, &crate::exec::Sequential, &mut deflate).unwrap();
        let got = link_cached(
            &chunks,
            &|k| touched.contains(&k),
            cache,
            &crate::exec::Sequential,
            &mut deflate,
        )
        .unwrap();
        assert_eq!(got, want);
        got.files[&1].clone()
    }

    /// Regions taken from the cache give what a link of them all does: a
    /// region not touched, but whose objects' numbers moved (an object
    /// made in a region before it), is resolved again.
    #[test]
    fn cached_regions_link_as_all_do() {
        let mut fx = vec![
            (10, vec![obj(100), text(b"a "), cite(100)]),
            (
                20,
                vec![obj(200), text(b" b "), cite(200), text(b" "), cite(100)],
            ),
            (30, vec![text(b" c "), cite(200)]),
        ];
        let mut cache = LinkCache::default();
        assert_eq!(linked(&fx, &[10, 20, 30], &mut cache), b"a 1 b 2 1 c 2");
        assert_eq!((cache.resolved, cache.reused), (3, 0));
        // (nothing touched: all taken again)
        assert_eq!(linked(&fx, &[], &mut cache), b"a 1 b 2 1 c 2");
        assert_eq!((cache.resolved, cache.reused), (0, 3));
        // (an object made first: every later number moves)
        fx[0].1.insert(0, obj(50));
        fx[0].1.push(cite(50));
        assert_eq!(linked(&fx, &[10], &mut cache), b"a 21 b 3 2 c 3");
        assert_eq!((cache.resolved, cache.reused), (3, 0));
        // (a text changed: that region alone)
        fx[2].1[0] = text(b" C ");
        assert_eq!(linked(&fx, &[30], &mut cache), b"a 21 b 3 2 C 3");
        assert_eq!((cache.resolved, cache.reused), (1, 2));
    }
}
