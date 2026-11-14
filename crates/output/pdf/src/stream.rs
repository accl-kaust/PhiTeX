//! What the files of pages of their own (a watch's pages streamed while
//! the build runs, each a PDF file apart) keep across files
//! ([`StreamMemo`]): the Type 1 font programs, each made once for every
//! glyph the pages written so far used of its font and again only when
//! a page uses one none before it did; and the images' streams,
//! decoded and compressed once. A page's file is drawn as the build's
//! file draws the page: a font embedded with more glyphs than the page
//! shows shows the same ones; an image's stream is the same bytes.
//! The build's own file never takes these ([`PdfState::stream_with`]).

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex, PoisonError};

use crate::t1::{self, FontDesc};
use crate::{PdfError, PdfState};

/// The memo of the files of pages of their own ([`PdfState::stream_with`]).
#[derive(Default)]
pub struct StreamMemo {
    /// The Type 1 fonts, by their file (its bytes' place: the file is
    /// kept, so no other file is ever at it) and map entry.
    fonts: Mutex<HashMap<FontKey, Vec<FontOf>>>,
    /// The images' streams, by the image's file and what its writing
    /// reads of the writer ([`PngKey`]).
    pngs: Mutex<HashMap<PngKey, Arc<PngMade>>>,
    /// The encoding files' glyph names, by the file's place (the file
    /// kept).
    encs: Mutex<EncMemo>,
}

/// [`StreamMemo::encs`].
type EncMemo = HashMap<(usize, usize), (Arc<[u8]>, crate::enc::GlyphNames)>;

impl std::fmt::Debug for StreamMemo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("StreamMemo")
    }
}

/// How many programs were made and found made (`PHITEX_STREAM_TRACE`
/// reads them: [`StreamMemo::counts`]).
static COUNTS: [std::sync::atomic::AtomicU64; 4] =
    [const { std::sync::atomic::AtomicU64::new(0) }; 4];

fn count(i: usize) {
    COUNTS[i].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

impl StreamMemo {
    /// The fonts' programs made and found made, the images' streams made
    /// and found made, since the process began.
    #[must_use]
    pub fn counts() -> [u64; 4] {
        COUNTS
            .each_ref()
            .map(|c| c.load(std::sync::atomic::Ordering::Relaxed))
    }
}

/// A Type 1 font of the files of pages of their own: the glyphs (by name;
/// and by code, a font not reencoded) its pages used so far, and the
/// program made for them.
struct StreamFont {
    /// Kept: the key is its place.
    _file: Arc<[u8]>,
    glyphs: BTreeSet<Vec<u8>>,
    codes: BTreeSet<usize>,
    /// How many times `glyphs` and `codes` grew.
    grown: u64,
    made: Option<Arc<Program>>,
}

/// A Type 1 font's program made for `glyphs` and `codes`
/// ([`StreamFont`]): the program with its subset tag's six bytes to be
/// put in, the descriptor the file gave (its values set from the file,
/// those it left unset), and the program compressed, by its tag and the
/// writer's level.
struct Program {
    /// [`StreamFont::grown`] when it was made.
    grown: u64,
    /// Its subset tag where no other font of the file has it.
    tag: Option<[u8; 6]>,
    e: t1::Embedded,
    desc: FontDesc,
    z: Mutex<Vec<Packed>>,
}

impl Program {
    /// The program with subset tag `tag`, compressed at `level` (as it
    /// is, level 0): once for each.
    fn packed(&self, tag: [u8; 6], level: i32) -> Arc<[u8]> {
        let mut z = self.z.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((_, _, b)) = z.iter().find(|x| x.0 == tag && x.1 == level) {
            return b.clone();
        }
        let mut p = self.e.bytes.clone();
        if let Some((at, _, _)) = &self.e.tag_at {
            p[*at..*at + 6].copy_from_slice(&tag);
        }
        let b: Arc<[u8]> = if level > 0 {
            Arc::from(crate::compress_memo(&p, level))
        } else {
            Arc::from(p)
        };
        z.push((tag, level, b.clone()));
        b
    }
}

/// A Type 1 font's key ([`StreamMemo::fonts`]): its file's place and
/// length; its map entries apart.
type FontKey = (usize, usize);

/// A Type 1 font of a file by its map entry ([`StreamMemo::fonts`]).
type FontOf = (crate::map::MapEntry, Arc<Mutex<StreamFont>>);

/// A program compressed ([`Program::z`]): by its subset tag and the
/// writer's level.
type Packed = ([u8; 6], i32, Arc<[u8]>);

/// An image's key ([`StreamMemo::pngs`]): its file's digest and length,
/// the PDF version and `\pdfimagehicolor` fixed, the compression level.
type PngKey = (u128, usize, i32, i32, i32, i32);

/// A PNG image's streams as its object writes them ([`PdfState::write_png`]),
/// compressed at the level of its key (as they are, level 0), and the
/// image as decoded (its colour type and depth).
pub(crate) struct PngMade {
    pub(crate) color_type: u8,
    pub(crate) bit_depth: u8,
    pub(crate) streams: Vec<Arc<[u8]>>,
}

impl PdfState {
    /// This writer's PNGs' streams taken from `memo`, made there where
    /// they are not (the build's link: the pages' files made them).
    pub fn png_memo_with(&mut self, memo: Arc<StreamMemo>) {
        self.png_memo = Some(memo);
    }

    /// This writer writes a page's file of its own (a page streamed): its
    /// Type 1 fonts' programs and its images' streams taken from `memo`,
    /// made there where they are not ([`StreamMemo`]).
    pub fn stream_with(&mut self, memo: Arc<StreamMemo>) {
        self.png_memo = Some(memo.clone());
        self.stream_memo = Some(memo);
    }

    /// The program of Type 1 font `file` (map entry and descriptor
    /// `fd_fm`, `desc`) for a page's file of its own: made for every glyph
    /// the pages written so far used of it, the page's own among them
    /// ([`StreamMemo`]); `desc` made the descriptor that program gives,
    /// its values the file leaves unset its own. The program's lengths
    /// (`/Length1`, `2`, `3`) and its stream, compressed at the writer's
    /// level.
    pub(crate) fn stream_t1(
        &mut self,
        memo: &StreamMemo,
        file: &Arc<[u8]>,
        fm: &crate::map::MapEntry,
        desc: &mut FontDesc,
    ) -> Result<([usize; 3], Arc<[u8]>), PdfError> {
        let font = {
            let mut m = memo.fonts.lock().unwrap_or_else(PoisonError::into_inner);
            let e = m.entry((file.as_ptr() as usize, file.len())).or_default();
            if let Some((_, f)) = e.iter().find(|(f, _)| f == fm) {
                f.clone()
            } else {
                let f = Arc::new(Mutex::new(StreamFont {
                    _file: file.clone(),
                    glyphs: BTreeSet::new(),
                    codes: BTreeSet::new(),
                    grown: 0,
                    made: None,
                }));
                e.push((fm.clone(), f.clone()));
                f
            }
        };
        let made = {
            let mut f = font.lock().unwrap_or_else(PoisonError::into_inner);
            let f = &mut *f;
            let mut more = false;
            for g in &desc.gl_tree {
                if !f.glyphs.contains(g) {
                    f.glyphs.insert(g.clone());
                    more = true;
                }
            }
            for &c in desc.tx_tree.iter().flatten() {
                more |= f.codes.insert(c);
            }
            f.grown += u64::from(more);
            match &f.made {
                Some(m) if m.grown == f.grown => {
                    count(1);
                    m.clone()
                }
                _ => {
                    count(0);
                    // (the program a function of the file, the map entry
                    // and the glyphs alone: the descriptor's values from
                    // none set, the subset tags from none taken, the
                    // other subroutines' last argument from pdfTeX's
                    // start; the build's own writer's state untouched)
                    let mut d = FontDesc {
                        fontname: desc.fontname.clone(),
                        subset_tag: None,
                        font_dim: [(0, false); 11],
                        gl_tree: f.glyphs.clone(),
                        tx_tree: (!f.codes.is_empty()).then(|| f.codes.clone()),
                        builtin: None,
                        all_glyphs: false,
                    };
                    let mut last_arg = 3;
                    let e = t1::write_t1(file, fm, &mut d, &mut BTreeSet::new(), &mut last_arg)?;
                    let tag = e
                        .tag_at
                        .as_ref()
                        .map(|(_, g, n)| t1::make_subset_tag(g, n, &mut BTreeSet::new()));
                    let m = Arc::new(Program {
                        grown: f.grown,
                        tag,
                        e,
                        desc: d,
                        z: Mutex::default(),
                    });
                    f.made = Some(m.clone());
                    m
                }
            }
        };
        // (the descriptor: the file's values over the page's; the file's
        // glyph names only where a `/ToUnicode` is made of them)
        let mut d = FontDesc {
            fontname: made.desc.fontname.clone(),
            subset_tag: made.desc.subset_tag,
            font_dim: made.desc.font_dim,
            gl_tree: made.desc.gl_tree.clone(),
            tx_tree: made.desc.tx_tree.clone(),
            builtin: (self.fixed_gen_tounicode > 0)
                .then(|| made.desc.builtin.clone())
                .flatten(),
            all_glyphs: made.desc.all_glyphs,
        };
        for (o, &i) in d.font_dim.iter_mut().zip(&desc.font_dim) {
            if !o.1 {
                *o = i;
            }
        }
        // (its tag, as `make_subset_tag` chooses it: the first unless another
        // font of the file took that)
        if let Some((_, glyphs, name)) = &made.e.tag_at {
            d.subset_tag = Some(match made.tag {
                Some(t) if self.subset_tags.insert(t) => t,
                _ => t1::make_subset_tag(glyphs, name, &mut self.subset_tags),
            });
        }
        *desc = d;
        let bytes = made.packed(desc.subset_tag.unwrap_or_default(), self.p.compress_level);
        let e = &made.e;
        Ok(([e.length1, e.length2, e.length3], bytes))
    }

    /// The glyph names of encoding file `bytes` ([`crate::enc::load_enc`]),
    /// read once for the pages' files.
    pub(crate) fn stream_enc(
        memo: &StreamMemo,
        bytes: &Arc<[u8]>,
    ) -> Result<crate::enc::GlyphNames, PdfError> {
        let key = (bytes.as_ptr() as usize, bytes.len());
        let mut m = memo.encs.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((_, n)) = m.get(&key) {
            return Ok(n.clone());
        }
        let n = crate::enc::load_enc(bytes)?;
        m.insert(key, (bytes.clone(), n.clone()));
        Ok(n)
    }

    /// The streams of PNG image `data` as [`PdfState::write_png`] writes
    /// them, from `memo` or made by `make` (uncompressed) and kept there,
    /// compressed at the writer's level.
    pub(crate) fn stream_png(
        &self,
        memo: &StreamMemo,
        data: &Arc<[u8]>,
        make: impl FnOnce() -> Result<(u8, u8, Vec<Vec<u8>>), PdfError>,
    ) -> Result<Arc<PngMade>, PdfError> {
        let level = self.p.compress_level;
        // (by the file's bytes: each page's file reads the image again)
        let key = (
            {
                let mut h = crate::digest::H128::new();
                h.write(data);
                h.finish()
            },
            data.len(),
            self.fixed_major,
            self.fixed_minor,
            self.fixed_image_hicolor,
            level,
        );
        if let Some(m) = memo
            .pngs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&key)
        {
            count(3);
            return Ok(m.clone());
        }
        count(2);
        let (color_type, bit_depth, raw) = make()?;
        let streams = raw
            .into_iter()
            .map(|s| -> Arc<[u8]> {
                if level > 0 {
                    Arc::from(crate::compress_memo(&s, level))
                } else {
                    Arc::from(s)
                }
            })
            .collect();
        let m = Arc::new(PngMade {
            color_type,
            bit_depth,
            streams,
        });
        memo.pngs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(key, m.clone());
        Ok(m)
    }

    /// A stream of `bytes` as made by a writer at this one's level
    /// (compressed where it compresses: [`StreamMemo`]'s), its
    /// `/Length` and `/Filter` as [`PdfState::begin_stream`] and
    /// [`PdfState::end_stream`] write them.
    pub(crate) fn put_made_stream(&mut self, bytes: &[u8]) {
        self.begin_stream();
        if self.zip.take().is_some() {
            self.out.extend_from_slice(bytes);
            let digits = bytes.len().to_string();
            let at = self.stream_length_offset - self.base;
            self.out[at..at + digits.len()].copy_from_slice(digits.as_bytes());
            self.end_raw_stream();
        } else {
            self.print(bytes);
            self.end_stream();
        }
    }
}
