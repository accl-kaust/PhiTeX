//! The read-time commands' steps as data ([`Step`]): what each command
//! does on the link's writer, with what it read where it ran, applied
//! by [`Step::apply`]. Data, not a closure, so that a build's commands
//! are saved with it and made again on a load
//! (docs/spec-persisted-graph.md §2.2.4): each kind keeps the values it
//! was made of where its bytes are a value's (a file's text, a form's
//! box, its whatsits' data), and those are saved; the rest is plain
//! bytes and numbers.

use std::sync::Arc;

use phitex_defuse::ValueId;
use phitex_pdf::{Action, FontSource, ImageSpec, ObjBody, PdfState, Whatsit};
use phitex_typeset::node::{BoxNode, Node};

use super::{CmdOut, Forms, Reloc};

/// Where a form's whatsit is made from (`pdfout::writer_whatsit_src`): a
/// `\special`'s node, the writer's data the run made of a pdfTeX
/// whatsit's, or nothing the writer sees.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WhatSrc {
    None,
    Special(ValueId),
    Data(ValueId),
}

/// An object's body (`\pdfobj`): its text, or `file{...}`'s file, whose
/// text value (`None`: no file) its bytes are.
#[derive(Clone, Debug)]
pub(crate) enum Body {
    Text(Vec<u8>),
    File(Option<ValueId>, Vec<u8>),
}

/// One read-time command's step on the link's writer.
#[derive(Clone, Debug)]
pub(crate) enum Step {
    /// `\pdffontname`, `\pdffontobjnum`: the font's tag and object.
    FontNum { fid: u32 },
    /// `\pdfpageref n`.
    PageRef { n: i32 },
    /// `\pdfcolorstackinit`.
    ColorStackInit {
        page: bool,
        direct: bool,
        init: Vec<u8>,
    },
    /// `\pdfobj reserveobjnum`.
    ObjReserve,
    /// `\pdfobj`.
    Obj {
        use_n: Option<i64>,
        stream: bool,
        attr: Option<Vec<u8>>,
        body: Body,
        immediate: bool,
    },
    /// `\pdfannot`'s and `\pdfstartlink`'s `pdf_new_objnum`.
    NewObjNum,
    /// `\pdfoutline`: its action's value (`act`) and the action made of
    /// it.
    Outline {
        attr: Option<Vec<u8>>,
        act: ValueId,
        action: Option<Action>,
        count: i32,
        title: Vec<u8>,
    },
    /// `\pdffontattr`.
    FontAttr { fid: u32, text: Vec<u8> },
    /// `\pdfincludechars`.
    IncludeChars { fid: u32, text: Vec<u8> },
    /// `\pdfmapfile`, `\pdfmapline`.
    Map { file: bool, text: Vec<u8> },
    /// `\pdfglyphtounicode`.
    GlyphToUnicode { glyph: Vec<u8>, uni: Vec<u8> },
    /// `\pdfnobuiltintounicode`.
    NoBuiltinToUnicode { fid: u32 },
    /// `\pdfximage`: the image's parameters (`crate::pdfprim`'s
    /// `image_params`), its specification, the text value its bytes are
    /// (`None`: none found).
    XImage {
        ps: Vec<i32>,
        spec: ImageSpec,
        data: Option<ValueId>,
        immediate: bool,
    },
    /// `\pdfxform`: its box (`b`, decoded: `bx`), its whatsits' sources
    /// and what the writer makes of them (`data`, relocated by `reloc`),
    /// the fonts it uses.
    XForm {
        attr: Option<Vec<u8>>,
        res: Option<Vec<u8>>,
        b: ValueId,
        bx: Box<BoxNode>,
        whats: Vec<WhatSrc>,
        reloc: Vec<(u64, usize)>,
        data: Vec<Option<Whatsit>>,
        ids: Vec<u32>,
        immediate: bool,
    },
}

impl Step {
    /// The step on writer `st` (its forms `forms`, fonts `fs`; `reloc`:
    /// the read-time numbers the link numbered otherwise).
    #[allow(clippy::too_many_lines)]
    pub(crate) fn apply(
        &self,
        st: &mut PdfState,
        forms: &mut Forms,
        fs: &dyn FontSource,
        reloc: &Reloc,
    ) -> CmdOut {
        match self {
            Step::FontNum { fid } => match (st.font_name(*fid, fs), st.font_objnum(*fid, fs)) {
                (Ok(n), Ok(o)) => CmdOut {
                    nums: vec![n, o],
                    err: None,
                },
                _ => CmdOut::default(),
            },
            Step::PageRef { n } => CmdOut::num(st.page_ref(*n)),
            Step::ColorStackInit { page, direct, init } => {
                match st.colorstack_init(*page, *direct, init) {
                    Ok(n) => CmdOut::num(n),
                    Err(e) => CmdOut::err(Some(e)),
                }
            }
            Step::ObjReserve => CmdOut::num(st.obj_reserve()),
            Step::Obj {
                use_n,
                stream,
                attr,
                body,
                immediate,
            } => {
                let n = use_n.map(|n| {
                    u64::try_from(n)
                        .ok()
                        .and_then(|k| reloc.get(&k))
                        .and_then(|&m| i64::try_from(m).ok())
                        .unwrap_or(n)
                });
                let body = match body {
                    Body::Text(t) => ObjBody::Text(t.clone()),
                    Body::File(_, b) => ObjBody::File(b.clone()),
                };
                let (n, refused) = st.obj(n, *stream, attr.clone(), body);
                let err = if *immediate {
                    st.write_obj(n).err()
                } else {
                    None
                };
                CmdOut {
                    nums: vec![i64::try_from(n).unwrap_or(0), i64::from(refused)],
                    err: err.map(|e| e.to_string()),
                }
            }
            Step::NewObjNum => CmdOut::num(st.new_objnum()),
            Step::Outline {
                attr,
                action,
                count,
                title,
                ..
            } => CmdOut::err(
                action
                    .as_ref()
                    .and_then(|a| st.outline(attr.clone(), a, *count, title).err()),
            ),
            Step::FontAttr { fid, text } => {
                st.font_attr(*fid, text, fs);
                CmdOut::default()
            }
            Step::IncludeChars { fid, text } => {
                let _ = st.include_chars(*fid, text, fs);
                CmdOut::default()
            }
            Step::Map { file, text } => {
                if *file {
                    st.map_file(text, fs);
                } else {
                    st.map_line(text);
                }
                CmdOut::default()
            }
            Step::GlyphToUnicode { glyph, uni } => {
                st.glyph_to_unicode(glyph, uni);
                CmdOut::default()
            }
            Step::NoBuiltinToUnicode { fid } => {
                st.no_builtin_tounicode(*fid, fs);
                CmdOut::default()
            }
            Step::XImage {
                ps,
                spec,
                immediate,
                ..
            } => {
                crate::pdfprim::image_params(st, ps);
                match st.ximage(spec.clone()) {
                    Ok(n) => {
                        let err = if *immediate {
                            st.write_ximage(n, fs).err()
                        } else {
                            None
                        };
                        CmdOut {
                            nums: vec![i64::try_from(n).unwrap_or(0)],
                            err: err.map(|e| e.to_string()),
                        }
                    }
                    Err(e) => CmdOut::err(Some(e)),
                }
            }
            Step::XForm {
                attr,
                res,
                bx,
                data,
                ids,
                immediate,
                ..
            } => {
                let base = forms.next_handle();
                let mut bx = bx.clone();
                crate::pdfprim::remap_whatsits(&mut bx.list, base);
                for (k, d) in data.iter().enumerate() {
                    if let Some(d) = d {
                        forms
                            .whats
                            .insert(base + u32::try_from(k).unwrap_or(0), d.clone());
                    }
                }
                forms.fonts.extend(ids.iter().copied());
                let n = st.xform(attr.clone(), res.clone(), *bx);
                let mut err = None;
                if *immediate {
                    let whats = &forms.whats;
                    let mut wh = |h: u32, _leaders: bool| whats.get(&h).cloned();
                    err = st.write_xform(n, fs, &mut wh).err();
                }
                CmdOut {
                    nums: vec![i64::try_from(n).unwrap_or(0)],
                    err: err.map(|e| e.to_string()),
                }
            }
        }
    }

    /// The values it was made of.
    pub(crate) fn values(&self, keep: &mut dyn FnMut(ValueId)) {
        match self {
            Step::Obj {
                body: Body::File(Some(t), _),
                ..
            }
            | Step::XImage { data: Some(t), .. } => keep(*t),
            Step::Outline { act, .. } => keep(*act),
            Step::XForm { b, whats, .. } => {
                keep(*b);
                for w in whats {
                    if let WhatSrc::Special(x) | WhatSrc::Data(x) = w {
                        keep(*x);
                    }
                }
            }
            _ => {}
        }
    }
}

/// A form's box `b` decoded, as `\pdfxform` decodes it.
pub(crate) fn form_box(v: &phitex_defuse::Values, b: ValueId) -> Option<Box<BoxNode>> {
    let mut side = crate::nodes::Side::default();
    match crate::nodes::decode(v, b, &mut side) {
        Some(Node::Box(bx)) => Some(bx),
        _ => None,
    }
}

/// The writer's whatsits of a form's sources, relocated by `reloc`
/// (`pdfout::form_whatsit_nums`).
pub(crate) fn form_data(
    v: &phitex_defuse::Values,
    whats: &[WhatSrc],
    reloc: &[(u64, usize)],
) -> Vec<Option<Whatsit>> {
    let map: Reloc = reloc.iter().copied().collect();
    whats
        .iter()
        .map(|w| {
            let d = match *w {
                WhatSrc::None => None,
                WhatSrc::Special(x) => Some(super::special_whatsit(v, x)),
                WhatSrc::Data(d) => super::whatsit(v, d),
            };
            if map.is_empty() {
                d
            } else {
                d.map(|d| super::relocated(d, &map))
            }
        })
        .collect()
}

/// The bytes of a file's text value `t` as `\pdfobj file` reads them.
pub(crate) fn file_body(v: &phitex_defuse::Values, t: Option<ValueId>) -> Vec<u8> {
    t.map(|t| crate::pdfprim::bytes(&crate::val::str_of(v, t)))
        .unwrap_or_default()
}

/// The bytes of an image file's text value `t` (`pdfout::text_bytes`).
pub(crate) fn image_data(v: &phitex_defuse::Values, t: Option<ValueId>) -> Arc<[u8]> {
    t.map_or_else(|| Arc::from(&[][..]), |t| super::text_bytes(v, t))
}
