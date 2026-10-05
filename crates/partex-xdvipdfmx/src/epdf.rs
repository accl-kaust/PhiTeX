//! epdf.c, epdf.h: including a page of a PDF file as a form XObject, and
//! `pdf_copy_clip` (a page's clipping path copied into the output).
//!
//! A `pdf_file *` is the `pf: u32` of `self.o.pdf_open(ident, data)`
//! (pdfread.rs); the file's bytes are `fp.data`.

use crate::dpxutil::DpxStack;
use crate::fmt::Buf;
use crate::obj::STREAM_COMPRESS;
use crate::parse::{parse_ident, skip_white};
use crate::pdfdev::{PdfCoord, PdfTmatrix};
use crate::pdfdoc::PdfPageBoundary;
use crate::pdfdraw::{pdf_concatmatrix, pdf_invertmatrix};
use crate::pdfximage::pdf_ximage_init_form_info;
use crate::prelude::*;

/// `pdfbox_crop`.
pub const PDFBOX_CROP: i32 = 1;
/// `pdfbox_media`.
pub const PDFBOX_MEDIA: i32 = 2;
/// `pdfbox_bleed`.
pub const PDFBOX_BLEED: i32 = 3;
/// `pdfbox_trim`.
pub const PDFBOX_TRIM: i32 = 4;
/// `pdfbox_art`.
pub const PDFBOX_ART: i32 = 5;

/// `enum action` (pdf_copy_clip's operators).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Unknown,
    Discard,
    Path,
    Rect,
    Trans,
    Clip,
    Save,
    Restore,
}

/// `struct operator`.
#[derive(Clone, Copy, Debug)]
pub struct Operator {
    pub token: &'static [u8],
    pub action: Action,
    pub n_args: i32,
}

/// `operators[]`, in C's order (two-character tokens first).
pub static OPERATORS: [Operator; 22] = [
    Operator {
        token: b"b*",
        action: Action::Clip,
        n_args: 0,
    },
    Operator {
        token: b"B*",
        action: Action::Clip,
        n_args: 0,
    },
    Operator {
        token: b"cm",
        action: Action::Trans,
        n_args: 6,
    },
    Operator {
        token: b"f*",
        action: Action::Clip,
        n_args: 0,
    },
    Operator {
        token: b"re",
        action: Action::Rect,
        n_args: 4,
    },
    Operator {
        token: b"W*",
        action: Action::Path,
        n_args: 0,
    },
    Operator {
        token: b"b",
        action: Action::Clip,
        n_args: 0,
    },
    Operator {
        token: b"B",
        action: Action::Clip,
        n_args: 0,
    },
    Operator {
        token: b"c",
        action: Action::Path,
        n_args: 6,
    },
    Operator {
        token: b"f",
        action: Action::Clip,
        n_args: 0,
    },
    Operator {
        token: b"F",
        action: Action::Clip,
        n_args: 0,
    },
    Operator {
        token: b"h",
        action: Action::Path,
        n_args: 0,
    },
    Operator {
        token: b"l",
        action: Action::Path,
        n_args: 2,
    },
    Operator {
        token: b"m",
        action: Action::Path,
        n_args: 2,
    },
    Operator {
        token: b"n",
        action: Action::Path,
        n_args: 0,
    },
    Operator {
        token: b"q",
        action: Action::Save,
        n_args: 0,
    },
    Operator {
        token: b"Q",
        action: Action::Restore,
        n_args: 0,
    },
    Operator {
        token: b"s",
        action: Action::Clip,
        n_args: 0,
    },
    Operator {
        token: b"S",
        action: Action::Clip,
        n_args: 0,
    },
    Operator {
        token: b"v",
        action: Action::Path,
        n_args: 4,
    },
    Operator {
        token: b"W",
        action: Action::Path,
        n_args: 0,
    },
    Operator {
        token: b"y",
        action: Action::Path,
        n_args: 4,
    },
];

/// `enum pdf_page_boundary` from its `int` (`load_options.bbox_type`).
fn page_boundary(v: i32) -> PdfPageBoundary {
    match v {
        1 => PdfPageBoundary::MediaBox,
        2 => PdfPageBoundary::CropBox,
        3 => PdfPageBoundary::ArtBox,
        4 => PdfPageBoundary::TrimBox,
        5 => PdfPageBoundary::BleedBox,
        _ => PdfPageBoundary::Auto,
    }
}

impl Dpx {
    /// `get_page_content` (static): the page's contents as one stream
    /// (an array of streams concatenated), or none.
    pub fn get_page_content(&mut self, pf: u32, page: Obj) -> Option<Obj> {
        let _ = pf;
        let c = self.o.lookup_dict(page, b"Contents");
        let mut contents = self.o.deref_obj(c)?;

        if self.o.is_null(Some(contents)) {
            /* empty page */
            self.o.release(contents);
            /* TODO: better don't include anything if the page is empty */
            contents = self.o.new_stream(0);
        } else if self.o.is_array(Some(contents)) {
            /*
             * Concatenate all content streams.
             */
            let content_new = self.o.new_stream(STREAM_COMPRESS);
            for i in 0..self.o.array_length(contents) {
                let item = self.o.get_array(contents, i as i32);
                let Some(content_seg) = self.o.deref_obj(item) else {
                    warn!("Could not read page content stream.");
                    self.o.release(content_new);
                    self.o.release(contents);
                    return None;
                };
                if self.o.is_stream(Some(content_seg)) {
                    self.o.concat_stream(content_new, content_seg);
                } else if !self.o.is_null(Some(content_seg)) {
                    warn!("Page content not a stream object. Broken PDF file?");
                    self.o.release(content_seg);
                    self.o.release(content_new);
                    self.o.release(contents);
                    return None;
                }
                self.o.release(content_seg);
            }
            self.o.release(contents);
            contents = content_new;
        } else {
            if !self.o.is_stream(Some(contents)) {
                warn!("Page content not a stream object. Broken PDF file?");
                self.o.release(contents);
                return None;
            }
            /* Flate the contents if necessary. */
            let content_new = self.o.new_stream(STREAM_COMPRESS);
            self.o.concat_stream(content_new, contents);
            self.o.release(contents);
            contents = content_new;
        }

        Some(contents)
    }
    /// `pdf_include_page`: fills XObject `xobj_id` with page
    /// `options.page_no` of the PDF in `fp` (opened as `ident`); 0 or -1.
    ///
    /// On a `/MarkInfo` without a boolean `/Marked`, C releases `markinfo`
    /// a second time on its error path (a bug; the run then stops with
    /// "Image inclusion failed" unless in compatibility mode): here it is
    /// released once.
    pub fn pdf_include_page(
        &mut self,
        xobj_id: i32,
        fp: &mut MemFile,
        ident: &[u8],
        options: crate::pdfximage::LoadOptions,
    ) -> i32 {
        let mut options = options;
        let Some(pf) = self.o.pdf_open(Some(ident), fp.data.clone()) else {
            return -1;
        };

        let mut info = pdf_ximage_init_form_info();

        if options.page_no == 0 {
            options.page_no = 1;
        }
        let (page, bbox, matrix, resources) = self.pdf_doc_get_page(
            pf,
            options.page_no,
            options.page_name.as_deref(),
            page_boundary(options.bbox_type),
            true,
        );
        info.bbox = bbox;
        info.matrix = matrix;

        let Some(page) = page else {
            /* error_silent */
            self.o.release_opt(resources);
            return -1;
        };

        let catalog = self.o.pdf_file_catalog(pf);
        let mi = catalog.and_then(|c| self.o.lookup_dict(c, b"MarkInfo"));
        let markinfo = self.o.deref_obj(mi);
        if let Some(markinfo) = markinfo {
            let m = self.o.lookup_dict(markinfo, b"Marked");
            let tmp = self.o.deref_obj(m);
            self.o.release(markinfo);
            if !self.o.is_boolean(tmp) {
                self.o.release_opt(tmp);
                /* error */
                warn!("Cannot parse document. Broken PDF file?");
                self.o.release_opt(resources);
                self.o.release(page);
                return -1;
            } else if self.o.boolean_value(tmp.unwrap()) {
                warn!("PDF file is tagged... Ignoring tags.");
            }
            self.o.release_opt(tmp);
        }

        /*
         * Handle page's Group
         */
        let group_obj = self.o.lookup_dict(page, b"Group");
        let group = group_obj.and_then(|g| self.o.import_object(g));
        /*
         * Handle page content stream.
         */
        let contents = self.get_page_content(pf, page);
        self.o.release(page);
        let Some(contents) = contents else {
            error!("typecheck: Invalid object type: 0 7 (line 0)");
        };

        /*
         * Add entries to contents stream dictionary.
         */
        {
            let contents_dict = self.o.stream_dict(contents);
            self.o.put_name(contents_dict, b"Type", b"XObject");
            self.o.put_name(contents_dict, b"Subtype", b"Form");
            self.o.put_number(contents_dict, b"FormType", 1.0);

            let bbox = self.o.new_array();
            for v in [info.bbox.llx, info.bbox.lly, info.bbox.urx, info.bbox.ury] {
                let n = self.o.new_number(v);
                self.o.add_array(bbox, n);
            }

            self.o.put(contents_dict, b"BBox", bbox);

            let matrix = self.o.new_array();
            for v in [
                info.matrix.a,
                info.matrix.b,
                info.matrix.c,
                info.matrix.d,
                info.matrix.e,
                info.matrix.f,
            ] {
                let n = self.o.new_number(v);
                self.o.add_array(matrix, n);
            }

            self.o.put(contents_dict, b"Matrix", matrix);

            let r = resources.and_then(|r| self.o.import_object(r));
            self.o.put_opt(contents_dict, b"Resources", r);
            self.o.release_opt(resources);

            if let Some(group) = group {
                self.o.put(contents_dict, b"Group", group);
            }

            if let Some(d) = options.dict {
                self.o.merge_dict(contents_dict, d);
            }
        }

        /* pdf_close(pf): nothing to do here */

        self.pdf_ximage_set_form(xobj_id, &info, contents);

        0
    }
    /// `get_numbers_from_stack` (static): pops `n` numbers into
    /// `v[0..n]` (last popped first); 0 or -1. Popped objects are released.
    pub fn get_numbers_from_stack(
        &mut self,
        stack: &mut DpxStack<Obj>,
        v: &mut [f64],
        n: i32,
    ) -> i32 {
        let mut error = 0;

        for i in 0..n {
            let Some(obj) = stack.dpx_stack_pop() else {
                error = -1;
                break;
            };
            if !self.o.is_number(Some(obj)) {
                self.o.release(obj);
                error = -1;
                break;
            }
            v[(n - i - 1) as usize] = self.o.number_value(obj);
            self.o.release(obj);
        }
        error
    }
    /// `pdf_copy_clip`: 0 or -1.
    pub fn pdf_copy_clip(
        &mut self,
        fp: &mut MemFile,
        page_index: i32,
        x_user: f64,
        y_user: f64,
    ) -> i32 {
        let mut error = 0;

        let Some(pf) = self.o.pdf_open(None, fp.data.clone()) else {
            return -1;
        };

        let (_, mut m) = self.pdf_dev_currentmatrix();
        pdf_invertmatrix(&mut m);
        m.e += x_user;
        m.f += y_user;

        let (page_tree, _bbox, _mtrx, _) =
            self.pdf_doc_get_page(pf, page_index, None, PdfPageBoundary::Auto, false);
        let Some(page_tree) = page_tree else {
            return -1;
        };

        let contents = self.get_page_content(pf, page_tree);
        self.o.release(page_tree);
        let Some(contents) = contents else {
            return -1;
        };

        self.pdf_doc_add_page_content(b" ");

        let data = self.o.stream_data(contents).to_vec();
        let s = &data[..];
        let endptr = s.len();
        let mut p = 0;
        let mut depth = 0;
        let mut stack: DpxStack<Obj> = DpxStack::dpx_stack_init();

        skip_white(s, &mut p);
        while p < endptr && error == 0 {
            let mut action = Action::Discard;
            let mut n_args = 0;
            let mut buf = Buf::new();

            if depth > 1 {
                if s[p] == b'q' {
                    depth += 1;
                }
                if s[p] == b'Q' {
                    depth -= 1;
                }
                let token = parse_ident(s, &mut p);
                skip_white(s, &mut p);
                if token.is_none() {
                    // C loops here for ever (nothing read, nothing skipped).
                    error!("pdf_copy_clip: unreadable content in a nested q/Q");
                }
                continue;
            }

            let obj = match s[p] {
                b'-' | b'+' | b'.' | b'0'..=b'9' => self.o.parse_pdf_number(s, &mut p),
                b'[' => self.o.parse_pdf_array(s, &mut p, None), /* No indirect reference allowed here */
                b'/' => self.o.parse_pdf_name(s, &mut p),
                b'(' => self.o.parse_pdf_string(s, &mut p),
                b'<' => {
                    if p + 1 < endptr && s[p + 1] == b'<' {
                        self.o.parse_pdf_dict(s, &mut p, None)
                    } else {
                        self.o.parse_pdf_string(s, &mut p)
                    }
                }
                _ => None,
            };
            if let Some(obj) = obj {
                skip_white(s, &mut p);
                stack.dpx_stack_push(obj);
                continue;
            }

            /* operator */
            let token = parse_ident(s, &mut p);
            skip_white(s, &mut p);
            let Some(token) = token else {
                break;
            };
            if let Some(op) = OPERATORS.iter().find(|op| op.token == &token[..]) {
                action = op.action;
                n_args = op.n_args;
            }
            match action {
                Action::Rect => {
                    let mut v = [0.0; 4];

                    error = self.get_numbers_from_stack(&mut stack, &mut v, n_args); /* n_args = 4 */
                    if error == 0 {
                        /* Not sure if this switch is required */
                        if m.b == 0.0 && m.c == 0.0 {
                            /* Use "re" operator */
                            let mut p0 = PdfCoord { x: v[0], y: v[1] };
                            let w = m.a * v[2];
                            let h = m.d * v[3];
                            self.pdf_dev_transform(&mut p0, Some(&m));
                            buf.push(b' ');
                            self.pdf_sprint_coord(&mut buf, &p0);
                            buf.push(b' ');
                            self.pdf_sprint_length(&mut buf, w);
                            buf.push(b' ');
                            self.pdf_sprint_length(&mut buf, h);
                            buf.extend(b" re");
                        } else {
                            /* Converted to lineto */
                            let (w, h) = (v[2], v[3]);
                            let mut p0 = PdfCoord { x: v[0], y: v[1] };
                            let mut p1 = PdfCoord {
                                x: p0.x + w,
                                y: p0.y,
                            };
                            let mut p2 = PdfCoord {
                                x: p1.x,
                                y: p1.y + h,
                            };
                            let mut p3 = PdfCoord { x: p0.x, y: p2.y };
                            self.pdf_dev_transform(&mut p0, Some(&m));
                            self.pdf_dev_transform(&mut p1, Some(&m));
                            self.pdf_dev_transform(&mut p2, Some(&m));
                            self.pdf_dev_transform(&mut p3, Some(&m));
                            buf.push(b' ');
                            self.pdf_sprint_coord(&mut buf, &p0);
                            buf.extend(b" m");
                            buf.push(b' ');
                            self.pdf_sprint_coord(&mut buf, &p1);
                            buf.extend(b" l");
                            buf.push(b' ');
                            self.pdf_sprint_coord(&mut buf, &p2);
                            buf.extend(b" l");
                            buf.push(b' ');
                            self.pdf_sprint_coord(&mut buf, &p3);
                            buf.extend(b" l h");
                        }
                        self.pdf_doc_add_page_content(buf.as_bytes());
                    }
                }
                Action::Path => {
                    let mut v = [0.0; 6];

                    error = self.get_numbers_from_stack(&mut stack, &mut v, n_args);
                    if error == 0 {
                        for i in 0..(n_args / 2) as usize {
                            let mut pt = PdfCoord {
                                x: v[2 * i],
                                y: v[2 * i + 1],
                            };
                            self.pdf_dev_transform(&mut pt, Some(&m));
                            buf.push(b' ');
                            self.pdf_sprint_coord(&mut buf, &pt);
                        }
                        buf.push(b' ');
                        buf.extend(&token);
                        self.pdf_doc_add_page_content(buf.as_bytes());
                    }
                }
                Action::Trans => {
                    let mut v = [0.0; 6];
                    error = self.get_numbers_from_stack(&mut stack, &mut v, n_args);
                    if error == 0 {
                        let t = PdfTmatrix {
                            a: v[0],
                            b: v[1],
                            c: v[2],
                            d: v[3],
                            e: v[4],
                            f: v[5],
                        };
                        pdf_concatmatrix(&mut m, &t);
                    }
                }
                Action::Clip => {
                    if token[0].is_ascii_lowercase() {
                        /* close path */
                        buf.extend(b" h");
                    }
                    if token.len() >= 2 && token[1] == b'*' {
                        buf.extend(b" W* n");
                    } else {
                        buf.extend(b" W n");
                    }
                    self.pdf_doc_add_page_content(buf.as_bytes());
                }
                Action::Save => {
                    depth += 1;
                }
                Action::Restore => {
                    depth -= 1;
                }
                Action::Discard => {
                    /* stack clearing behavior */
                    while let Some(obj) = stack.dpx_stack_pop() {
                        self.o.release(obj);
                    }
                }
                Action::Unknown => {
                    error = -1;
                }
            }
        }
        while let Some(obj) = stack.dpx_stack_pop() {
            self.o.release(obj);
        }
        self.o.release(contents);
        /* pdf_close(pf): nothing to do here */

        error
    }
}
