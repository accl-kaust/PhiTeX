//! pdfTeX's PDF-only commands (pdftex.web part 53, "Implement \pdf…";
//! `conv_toks`' PDF cases): what each reads and makes, as a node.
//!
//! - A command that puts something on the page (`\pdfliteral`, links,
//!   annotations, destinations, `\pdfrefximage`, …) appends a whatsit
//!   (`item::Whatsit::Pdf`), which the page's walk turns into stream text
//!   and marks (`pdf::content`).
//! - A command that asks something of the PDF file (`\pdfobj`, `\pdfxform`,
//!   `\pdfximage`, `\pdfoutline`, `\pdfcatalog`, the fonts', the maps', …)
//!   emits a PDF event (`Item::Pdf`), taken out of its lists as
//!   `\shipout` is: the page builder hands the events to the pages in
//!   order, and the join's fold does them between the pages
//!   (`pdf::fold`).
//! - What pdfTeX's commands read back of the file (`\pdflastobj`, an
//!   object's existence, a form's size, `\pdflastxpos` outside a
//!   `\write`, …) is the fold's answer to the event, read as
//!   `Key::PdfAns` (the build answers it from the last fold; CORE.md,
//!   "pdfTeX's read-backs"); a command sets pdfTeX's globals
//!   (`Key::PdfLast`) from it. Before any answer a read gets IniTeX's
//!   initial value.
//! - The files a command reads are `Key::File` reads; the random seed, the
//!   clock and the creation date are the build's givens (`Key::PdfSeed`,
//!   `Key::PdfClock`, `Key::PdfDate`).

use std::fmt::Write as _;
use std::sync::Arc;

use super::{Exec, Mode};
use crate::item::{Item, RUNNING, Whatsit};
use crate::pdf::what::{self, ANSWER_FIELDS, Action, Event, Id, Params, PdfItem, Text, What};
use crate::prim::PDFTEX_PRIMITIVES;
use crate::tfm::Font;
use crate::val::{IntPar, Key, Val};

/// `max_halfword` (web2c).
const MAX_HALFWORD: i32 = 0x0FFF_FFFF;

/// A string as pdfTeX's pool holds it: one byte a character.
fn bytes(s: &str) -> Vec<u8> {
    s.chars().map(|c| u32::from(c).min(255) as u8).collect()
}

fn text(s: &str) -> Text {
    Arc::from(bytes(s))
}

/// `PdfLast` codes (`prim::PDF_ITEMS`).
const LAST_OBJ: u8 = 1;
const LAST_XFORM: u8 = 2;
const LAST_XIMAGE: u8 = 3;
const LAST_XIMAGE_PAGES: u8 = 4;
const LAST_ANNOT: u8 = 5;
const RETVAL: u8 = 8;
const LAST_XIMAGE_COLORDEPTH: u8 = 9;
const LAST_LINK: u8 = 11;

impl Exec<'_> {
    /// `scan_pdf_ext_toks` and `tokens_to_string`: a general text,
    /// expanded, as pdfTeX's string.
    fn pdf_text(&mut self) -> Text {
        let toks = self.scan_toks_x();
        let mut s = String::new();
        self.show_toks(&mut s, &toks);
        text(&s)
    }

    /// `pdf_error`: fatal (pdfTeX's `succumb`).
    fn pdf_fatal(&mut self, module: &str, msg: &str) {
        self.overflow(format!("pdfTeX error ({module}): {msg}"));
    }

    /// `check_pdfoutput`: whether PDF output is on; else a fatal error,
    /// or (`is_error` false) a warning and the command is ignored.
    fn pdf_on(&mut self, name: &str, is_error: bool) -> bool {
        if self.int_par(IntPar::Pdfoutput) > 0 {
            return true;
        }
        if is_error {
            self.pdf_fatal(&format!("\\{name}"), "not allowed in DVI mode (\\pdfoutput <= 0)");
        } else {
            self.log_push(&format!("\npdfTeX warning (\\{name}): not allowed in DVI mode (\\pdfoutput <= 0); ignoring it\n"));
        }
        false
    }

    /// The parameters an event reads where its command is.
    fn pdf_params(&mut self) -> Arc<Params> {
        let mut read = |k: &Key| self.get(k);
        Arc::new(crate::pdf::read_params(&mut read))
    }

    /// Emits `event` here; its index among this node's.
    fn pdf_event(&mut self, event: Event) -> u32 {
        let (scope, fire) = self.env.pdf_scope();
        let node = self.chain.nodes.len() as u32;
        let idx = self.pdf_events;
        self.pdf_events += 1;
        let tag = [scope, fire, node, idx];
        self.emit_here(Item::Pdf(Arc::new(PdfItem { tag, event })));
        idx
    }

    /// The fold's answer to this node's event `idx`, field `field` (none
    /// before the first fold).
    fn pdf_answer(&mut self, idx: u32, field: u32) -> Option<i32> {
        let (scope, fire) = self.env.pdf_scope();
        let node = self.chain.nodes.len() as u32;
        match self.get(&Key::PdfAns(scope, fire, node, idx * ANSWER_FIELDS + field)) {
            Val::Int(v) => Some(v),
            _ => None,
        }
    }

    /// pdfTeX's global `k` (`Key::PdfLast`), set: global, never grouped.
    fn pdf_set_last(&mut self, k: u8, v: i32) {
        self.define(Key::PdfLast(k), Val::Int(v), true);
    }

    /// `scan_alt_rule`: `width`, `height`, `depth` in any order, repeated
    /// (`RUNNING` where not given).
    fn pdf_alt_rule(&mut self) -> (i32, i32, i32) {
        let (mut w, mut h, mut d) = (RUNNING, RUNNING, RUNNING);
        loop {
            if self.keyword("width") {
                w = self.scan_dimen();
            } else if self.keyword("height") {
                h = self.scan_dimen();
            } else if self.keyword("depth") {
                d = self.scan_dimen();
            } else {
                return (w, h, d);
            }
        }
    }

    /// `scan_action`. `None`: a fatal error was given.
    fn pdf_action(&mut self) -> Option<Arc<Action>> {
        let mut a = Action {
            kind: what::ACTION_USER,
            text: Arc::from(&b""[..]),
            file: None,
            id: Id::Num(0),
            struct_id: None,
            new_window: 0,
        };
        if self.keyword("user") {
            a.kind = what::ACTION_USER;
        } else if self.keyword("goto") {
            a.kind = what::ACTION_GOTO;
        } else if self.keyword("thread") {
            a.kind = what::ACTION_THREAD;
        } else {
            self.pdf_fatal("ext1", "action type missing");
            return None;
        }
        if a.kind == what::ACTION_USER {
            a.text = self.pdf_text();
            return Some(Arc::new(a));
        }
        if self.keyword("file") {
            a.file = Some(self.pdf_text());
        }
        if self.keyword("struct") {
            if a.kind != what::ACTION_GOTO {
                self.pdf_fatal("ext1", "only GoTo action can be used with `struct'");
                return None;
            }
            if a.file.is_some() || self.keyword("name") {
                a.struct_id = Some(Id::Name(self.pdf_text()));
            } else if self.keyword("num") {
                let v = self.scan_int();
                if v <= 0 {
                    self.pdf_fatal("ext1", "num identifier must be positive");
                    return None;
                }
                a.struct_id = Some(Id::Num(v));
            } else {
                self.pdf_fatal("ext1", "identifier type missing");
                return None;
            }
        }
        if self.keyword("page") {
            if a.kind != what::ACTION_GOTO {
                self.pdf_fatal("ext1", "only GoTo action can be used with `page'");
                return None;
            }
            a.kind = what::ACTION_PAGE;
            let v = self.scan_int();
            if v <= 0 {
                self.pdf_fatal("ext1", "page number must be positive");
                return None;
            }
            a.id = Id::Num(v);
            a.text = self.pdf_text();
        } else if self.keyword("name") {
            a.id = Id::Name(self.pdf_text());
        } else if self.keyword("num") {
            if a.kind == what::ACTION_GOTO && a.file.is_some() {
                self.pdf_fatal("ext1", "`goto' option cannot be used with both `file' and `num'");
                return None;
            }
            let v = self.scan_int();
            if v <= 0 {
                self.pdf_fatal("ext1", "num identifier must be positive");
                return None;
            }
            a.id = Id::Num(v);
        } else {
            self.pdf_fatal("ext1", "identifier type missing");
            return None;
        }
        if self.keyword("newwindow") {
            a.new_window = 1;
            self.optional_space();
        } else if self.keyword("nonewwindow") {
            a.new_window = 2;
            self.optional_space();
        }
        if a.new_window > 0 && ((a.kind != what::ACTION_GOTO && a.kind != what::ACTION_PAGE) || a.file.is_none()) {
            self.pdf_fatal("ext1", "`newwindow'/`nonewwindow' must be used with `goto' and `file' option");
            return None;
        }
        Some(Arc::new(a))
    }

    /// A destination's or thread's identifier (`num n`, `name{…}`).
    fn pdf_id(&mut self) -> Option<Id> {
        if self.keyword("num") {
            let v = self.scan_int();
            if v <= 0 {
                self.pdf_fatal("ext1", "num identifier must be positive");
                return None;
            }
            if v > MAX_HALFWORD {
                self.pdf_fatal("ext1", "number too big");
                return None;
            }
            Some(Id::Num(v))
        } else if self.keyword("name") {
            Some(Id::Name(self.pdf_text()))
        } else {
            self.pdf_fatal("ext1", "identifier type missing");
            None
        }
    }

    fn pdf_whatsit(&mut self, w: What) {
        self.emit_here(Item::Whatsit(Arc::new(Whatsit::Pdf(Arc::new(w)))));
    }

    /// A font identifier, not the null font (else a fatal error).
    fn pdf_font(&mut self) -> Option<Arc<Font>> {
        let f = self.scan_font_ident();
        if f.name == "nullfont" {
            self.pdf_fatal("font", "invalid font identifier");
            return None;
        }
        Some(f)
    }

    /// `pdf_check_obj(t, n)` (fatal if `n` is no object of type `t`), and
    /// its answer's fields (`[there, w, h, d, obj_info]`).
    fn pdf_check(&mut self, kind: u8, n: i32) -> Option<[i32; 5]> {
        let i = self.pdf_event(Event::Check { kind, n });
        // (an answer for another `n` is none yet: `what::ECHO`)
        let a = if self.pdf_answer(i, what::ECHO) == Some(n) { self.pdf_answer(i, 0) } else { None };
        let mut out = [1, 0, 0, 0, 0];
        if let Some(v) = a {
            out[0] = v;
            for (f, o) in out.iter_mut().enumerate().skip(1) {
                *o = self.pdf_answer(i, f as u32).unwrap_or(0);
            }
        }
        if out[0] == 0 {
            self.pdf_fatal("ext1", "cannot find referenced object");
            return None;
        }
        Some(out)
    }

    /// A PDF-only command (`Prim::Pdf(i)`).
    #[allow(clippy::too_many_lines)]
    pub(super) fn pdf_cmd(&mut self, i: u16) {
        let name = PDFTEX_PRIMITIVES[usize::from(i)].0;
        match name {
            "pdfliteral" => {
                if !self.pdf_on(name, true) {
                    return;
                }
                if self.keyword("shipout") {
                    // (its tokens expanded where the page is shipped: CORE.md, gaps)
                    let _ = self.scan_toks_raw();
                    self.err("\\pdfliteral shipout is not supported yet".into());
                    return;
                }
                let mode = if self.keyword("direct") {
                    what::DIRECT_ALWAYS
                } else if self.keyword("page") {
                    what::DIRECT_PAGE
                } else {
                    what::SET_ORIGIN
                };
                let data = self.pdf_text();
                self.pdf_whatsit(What::Literal { mode, data });
            }
            "pdfcolorstack" => {
                if !self.pdf_on(name, true) {
                    return;
                }
                let mut n = self.scan_int();
                let i = self.pdf_event(Event::ColorStacks);
                let used = self.pdf_answer(i, 0).unwrap_or(1);
                if n >= used {
                    self.error(&format!("Unknown color stack number {n}"), &[
                        "Allocate and initialize a color stack with \\pdfcolorstackinit.",
                        "I'll use default color stack 0 here.",
                        "Proceed, with fingers crossed.",
                    ]);
                    n = 0;
                }
                if n < 0 {
                    self.error("Invalid negative color stack number", &[
                        "I'll use default color stack 0 here.",
                        "Proceed, with fingers crossed.",
                    ]);
                    n = 0;
                }
                let cmd = if self.keyword("set") {
                    what::CS_SET
                } else if self.keyword("push") {
                    what::CS_PUSH
                } else if self.keyword("pop") {
                    what::CS_POP
                } else if self.keyword("current") {
                    what::CS_CURRENT
                } else {
                    self.error("Color stack action is missing", &[
                        "The expected actions for \\pdfcolorstack:",
                        "    set, push, pop, current",
                        "I'll ignore the color stack command.",
                    ]);
                    return;
                };
                let data = (cmd <= what::CS_PUSH).then(|| self.pdf_text());
                self.pdf_whatsit(What::ColorStack { stack: n, cmd, data });
            }
            "pdfsetmatrix" => {
                if self.pdf_on(name, true) {
                    let d = self.pdf_text();
                    self.pdf_whatsit(What::SetMatrix(d));
                }
            }
            "pdfsave" => {
                if self.pdf_on(name, true) {
                    self.pdf_whatsit(What::Save);
                }
            }
            "pdfrestore" => {
                if self.pdf_on(name, true) {
                    self.pdf_whatsit(What::Restore);
                }
            }
            "pdfobj" => self.pdf_obj(false),
            "pdfrefobj" => {
                if !self.pdf_on(name, true) {
                    return;
                }
                let n = self.scan_int();
                if self.pdf_check(what::OBJ_TYPE_OBJ, n).is_some() {
                    self.pdf_whatsit(What::RefObj(n));
                }
            }
            "pdfxform" => self.pdf_xform(false),
            "pdfrefxform" => {
                if !self.pdf_on(name, true) {
                    return;
                }
                let n = self.scan_int();
                if let Some(a) = self.pdf_check(what::OBJ_TYPE_XFORM, n) {
                    self.pdf_whatsit(What::RefXform { n, w: a[1], h: a[2], d: a[3] });
                }
            }
            "pdfximage" => self.pdf_ximage(false),
            "pdfrefximage" => {
                if !self.pdf_on(name, true) {
                    return;
                }
                let n = self.scan_int();
                if let Some(a) = self.pdf_check(what::OBJ_TYPE_XIMAGE, n) {
                    self.pdf_whatsit(What::RefXimage { n, w: a[1], h: a[2], d: a[3] });
                }
            }
            "pdfannot" => {
                if !self.pdf_on(name, true) {
                    return;
                }
                if self.keyword("reserveobjnum") {
                    let i = self.pdf_event(Event::Annot { reserve: true, use_num: None });
                    let k = self.pdf_answer(i, 0).unwrap_or(0);
                    self.pdf_set_last(LAST_ANNOT, k);
                    self.optional_space();
                    return;
                }
                let use_num = self.keyword("useobjnum").then(|| self.scan_int());
                let i = self.pdf_event(Event::Annot { reserve: false, use_num });
                let echoed = use_num.is_some_and(|n| self.pdf_answer(i, what::ECHO) == Some(n));
                if echoed && self.pdf_answer(i, 1) == Some(1) {
                    self.pdf_fatal("ext1", "invalid object number");
                    return;
                }
                let k = self.pdf_answer(i, 0).unwrap_or(0);
                let (w, h, d) = self.pdf_alt_rule();
                let data = self.pdf_text();
                self.pdf_whatsit(What::Annot { objnum: k, w, h, d, data });
                self.pdf_set_last(LAST_ANNOT, k);
            }
            "pdfstartlink" => {
                if !self.pdf_on(name, true) {
                    return;
                }
                if self.st.mode == Mode::V {
                    self.pdf_fatal("ext1", "\\pdfstartlink cannot be used in vertical mode");
                    return;
                }
                let i = self.pdf_event(Event::Annot { reserve: true, use_num: None });
                let k = self.pdf_answer(i, 0).unwrap_or(0);
                let (w, h, d) = self.pdf_alt_rule();
                let attr = self.keyword("attr").then(|| self.pdf_text());
                let Some(action) = self.pdf_action() else { return };
                self.pdf_whatsit(What::StartLink { objnum: k, w, h, d, attr, action });
                self.pdf_set_last(LAST_LINK, k);
            }
            "pdfendlink" => {
                if !self.pdf_on(name, true) {
                    return;
                }
                if self.st.mode == Mode::V {
                    self.pdf_fatal("ext1", "\\pdfendlink cannot be used in vertical mode");
                    return;
                }
                self.pdf_whatsit(What::EndLink);
            }
            "pdfoutline" => {
                if !self.pdf_on(name, true) {
                    return;
                }
                let attr = self.keyword("attr").then(|| self.pdf_text());
                let Some(action) = self.pdf_action() else { return };
                let count = if self.keyword("count") { self.scan_int() } else { 0 };
                let title = self.pdf_text();
                let params = self.pdf_params();
                self.pdf_event(Event::Outline { attr, action, count, title, params });
            }
            "pdfdest" => self.pdf_dest(),
            "pdfthread" | "pdfstartthread" => {
                if !self.pdf_on(name, true) {
                    return;
                }
                let (w, h, d) = self.pdf_alt_rule();
                let attr = self.keyword("attr").then(|| self.pdf_text());
                let Some(id) = self.pdf_id() else { return };
                self.pdf_whatsit(What::Thread { start: name == "pdfstartthread", id, w, h, d, attr });
            }
            "pdfendthread" => {
                if self.pdf_on(name, true) {
                    self.pdf_whatsit(What::EndThread);
                }
            }
            "pdfsavepos" => self.pdf_whatsit(What::SavePos),
            "pdfinfo" | "pdftrailer" | "pdftrailerid" => {
                let on = self.pdf_on(name, false);
                let t = self.pdf_text();
                if on {
                    self.pdf_event(match name {
                        "pdfinfo" => Event::Info(t),
                        "pdftrailer" => Event::Trailer(t),
                        _ => Event::TrailerId(t),
                    });
                }
            }
            "pdfnames" => {
                if self.pdf_on(name, true) {
                    let t = self.pdf_text();
                    self.pdf_event(Event::Names(t));
                }
            }
            "pdfcatalog" => {
                let on = self.pdf_on(name, false);
                let toks = self.pdf_text();
                let open = if self.keyword("openaction") {
                    let Some(a) = self.pdf_action() else { return };
                    Some(a)
                } else {
                    None
                };
                if on {
                    let params = self.pdf_params();
                    self.pdf_event(Event::Catalog { toks, open, params });
                }
            }
            "pdfresettimer" => {}
            "pdfsetrandomseed" => {
                let v = self.scan_int();
                let r = crate::pdf::rand::Randoms::seeded(v.wrapping_abs());
                self.define(Key::PdfRandom, Val::Penalties(Arc::from(r.to_ints())), true);
            }
            "pdfincludechars" => {
                if !self.pdf_on(name, true) {
                    return;
                }
                let Some(f) = self.pdf_font() else { return };
                let t = self.pdf_text();
                self.pdf_event(Event::IncludeChars(f, t.to_vec()));
            }
            "pdffontattr" => {
                if !self.pdf_on(name, true) {
                    return;
                }
                let Some(f) = self.pdf_font() else { return };
                let t = self.pdf_text();
                self.pdf_event(Event::FontAttr(f, t));
            }
            "pdfnobuiltintounicode" => {
                if !self.pdf_on(name, true) {
                    return;
                }
                let Some(f) = self.pdf_font() else { return };
                self.pdf_event(Event::NoBuiltinToUnicode(f));
            }
            "pdfmapfile" | "pdfmapline" => {
                if !self.pdf_on(name, true) {
                    return;
                }
                let t = self.pdf_text();
                self.pdf_event(if name == "pdfmapfile" { Event::MapFile(t) } else { Event::MapLine(t) });
            }
            "pdfrunninglinkoff" | "pdfrunninglinkon" => {
                if self.pdf_on(name, true) {
                    self.pdf_whatsit(What::RunningLink(name == "pdfrunninglinkon"));
                }
            }
            _ => self.err(format!("\\{name} is not supported yet")),
        }
    }

    /// `\immediate` before a PDF command (pdftex.web's "Implement
    /// \immediate"): `\pdfobj`, `\pdfxform`, `\pdfximage` write their
    /// object now; whether it was one of them.
    pub(super) fn pdf_immediate(&mut self, i: u16) -> bool {
        match PDFTEX_PRIMITIVES[usize::from(i)].0 {
            "pdfobj" => self.pdf_obj(true),
            "pdfxform" => self.pdf_xform(true),
            "pdfximage" => self.pdf_ximage(true),
            _ => return false,
        }
        true
    }

    /// `\pdfobj`.
    fn pdf_obj(&mut self, immediate: bool) {
        if !self.pdf_on("pdfobj", true) {
            return;
        }
        if self.keyword("reserveobjnum") {
            self.optional_space();
            if immediate {
                self.pdf_fatal("ext1", "`\\pdfobj reserveobjnum' cannot be used with \\immediate");
                return;
            }
            let params = self.pdf_params();
            let e = Event::Obj {
                reserve: true,
                use_num: None,
                stream: false,
                attr: None,
                file: false,
                bytes: None,
                data: Arc::from(&b""[..]),
                immediate: false,
                params,
            };
            let i = self.pdf_event(e);
            let k = self.pdf_answer(i, 0).unwrap_or(0);
            self.pdf_set_last(LAST_OBJ, k);
            return;
        }
        let use_num = self.keyword("useobjnum").then(|| self.scan_int());
        let (stream, attr) = if self.keyword("stream") {
            (true, self.keyword("attr").then(|| self.pdf_text()))
        } else {
            (false, None)
        };
        let file = self.keyword("file");
        let data = self.pdf_text();
        // (a file's bytes read now: pdfTeX reads them where it writes)
        let bytes = if file {
            let n: String = String::from_utf8_lossy(&data).replace('"', "");
            self.find_file(&n).map(|x| x.1)
        } else {
            None
        };
        let params = self.pdf_params();
        let i = self.pdf_event(Event::Obj { reserve: false, use_num, stream, attr, file, bytes, data, immediate, params });
        if self.pdf_answer(i, 1) == Some(1) {
            self.log_push("\npdfTeX warning (\\pdfobj): invalid object number being ignored\n");
            self.pdf_set_last(RETVAL, -1);
        }
        let k = self.pdf_answer(i, 0).unwrap_or(0);
        self.pdf_set_last(LAST_OBJ, k);
    }

    /// `\pdfxform`.
    fn pdf_xform(&mut self, immediate: bool) {
        if !self.pdf_on("pdfxform", true) {
            return;
        }
        let attr = self.keyword("attr").then(|| self.pdf_text());
        let resources = self.keyword("resources").then(|| self.pdf_text());
        let n = self.scan_eight_bit_int();
        let Val::Box(b) = self.get(&Key::Box(n)) else {
            self.pdf_fatal("ext1", "\\pdfxform cannot be used with a void box");
            return;
        };
        self.change_box(n, Val::Void);
        let params = self.pdf_params();
        let i = self.pdf_event(Event::Xform { attr, resources, b, immediate, params });
        let k = self.pdf_answer(i, 0).unwrap_or(0);
        self.pdf_set_last(LAST_XFORM, k);
    }

    /// `\pdfximage` (`scan_image`, `scale_image`).
    fn pdf_ximage(&mut self, immediate: bool) {
        if !self.pdf_on("pdfximage", true) {
            return;
        }
        let given = self.pdf_alt_rule();
        let attr = self.keyword("attr").then(|| self.pdf_text());
        let (named, page) = if self.keyword("named") {
            (Some(self.pdf_text()), 1)
        } else if self.keyword("page") {
            (None, self.scan_int())
        } else {
            (None, 1)
        };
        let colorspace = if self.keyword("colorspace") { self.scan_int() } else { 0 };
        let mut pagebox = if self.keyword("mediabox") {
            1
        } else if self.keyword("cropbox") {
            2
        } else if self.keyword("bleedbox") {
            3
        } else if self.keyword("trimbox") {
            4
        } else if self.keyword("artbox") {
            5
        } else {
            0
        };
        if pagebox == 0 {
            pagebox = self.int_par(IntPar::Pdfpagebox);
        }
        let file = self.pdf_text();
        if pagebox == 0 {
            pagebox = 2;
        }
        let params = self.pdf_params();
        let fname: String = String::from_utf8_lossy(&file).replace('"', "");
        let Some((found, data)) = self.find_file(&fname) else {
            self.pdf_fatal("ext1", &format!("cannot find image file '{fname}'"));
            return;
        };
        let read = crate::pdf::image::read_image(
            &found,
            data,
            page,
            named.as_deref(),
            colorspace,
            pagebox,
            params.major,
            params.minor,
            params.inclusion_errorlevel,
        );
        let image = match read {
            Ok((im, w)) => {
                if let Some(w) = w {
                    self.log_push(&format!("\npdfTeX warning: {w}\n"));
                }
                im
            }
            Err(m) => {
                self.pdf_fatal("ext1", &m);
                return;
            }
        };
        let res = self.int_par(IntPar::Pdfimageresolution);
        let ((w, h, d), warns) = match crate::pdf::image::scale(&image, given, res) {
            Ok(x) => x,
            Err(m) => {
                self.pdf_fatal("ext1", &m);
                return;
            }
        };
        for x in warns {
            self.log_push(&format!("\n{x}\n"));
        }
        let pages = image.pages();
        let depth = image.bits;
        let image = Arc::new(image);
        let i = self.pdf_event(Event::Ximage { image, w, h, d, attr, immediate, params });
        let k = self.pdf_answer(i, 0).unwrap_or(0);
        self.pdf_set_last(LAST_XIMAGE, k);
        self.pdf_set_last(LAST_XIMAGE_PAGES, pages);
        self.pdf_set_last(LAST_XIMAGE_COLORDEPTH, depth);
    }

    /// `\pdfdest`.
    fn pdf_dest(&mut self) {
        if !self.pdf_on("pdfdest", true) {
            return;
        }
        let struct_num = if self.keyword("struct") {
            let v = self.scan_int();
            if v <= 0 {
                self.pdf_fatal("ext1", "struct identifier must be positive");
                return;
            }
            Some(v)
        } else {
            None
        };
        let Some(id) = self.pdf_id() else { return };
        let mut zoom = None;
        let kind = if self.keyword("xyz") {
            if self.keyword("zoom") {
                let v = self.scan_int();
                if v > MAX_HALFWORD {
                    self.pdf_fatal("ext1", "number too big");
                    return;
                }
                zoom = Some(v);
            }
            what::DEST_XYZ
        } else if self.keyword("fitbh") {
            what::DEST_FITBH
        } else if self.keyword("fitbv") {
            what::DEST_FITBV
        } else if self.keyword("fitb") {
            what::DEST_FITB
        } else if self.keyword("fith") {
            what::DEST_FITH
        } else if self.keyword("fitv") {
            what::DEST_FITV
        } else if self.keyword("fitr") {
            what::DEST_FITR
        } else if self.keyword("fit") {
            what::DEST_FIT
        } else {
            self.pdf_fatal("ext1", "destination type missing");
            return;
        };
        self.optional_space();
        let (w, h, d) = if kind == what::DEST_FITR { self.pdf_alt_rule() } else { (RUNNING, RUNNING, RUNNING) };
        let i = self.pdf_event(Event::DestCheck { id: id.clone(), structd: struct_num.is_some() });
        if self.pdf_answer(i, 0) == Some(1) {
            if self.int_par(IntPar::Pdfsuppresswarningdupdest) <= 0 {
                let shown = match &id {
                    Id::Num(n) => format!("num{n}"),
                    Id::Name(t) => format!("name{{{}}}", String::from_utf8_lossy(t)),
                };
                self.log_push(&format!(
                    "\npdfTeX warning (ext4): destination with the same identifier ({shown}) has been already used, duplicate ignored\n"
                ));
            }
            return;
        }
        self.pdf_whatsit(What::Dest { id, struct_num, kind, zoom, w, h, d });
    }

    /// pdfTeX's read-only quantities (`Prim::PdfItem(k)`, `PDF_ITEMS`).
    pub(super) fn pdf_item(&mut self, k: u8) -> i32 {
        match k {
            LAST_OBJ | LAST_XFORM | LAST_XIMAGE | LAST_XIMAGE_PAGES | LAST_ANNOT | RETVAL | LAST_XIMAGE_COLORDEPTH
            | LAST_LINK => match self.get(&Key::PdfLast(k)) {
                Val::Int(v) => v,
                _ => 0,
            },
            // (`\pdflastxpos`, `\pdflastypos`: in a `\write` shipped, where the
            // walk put it; elsewhere the fold's, after the pages before)
            6 | 7 => {
                if let Some((x, y)) = self.pdf_pos {
                    return if k == 6 { x } else { y };
                }
                let i = self.pdf_event(Event::LastPos);
                self.pdf_answer(i, u32::from(k - 6)).unwrap_or(0)
            }
            12 => match self.get(&Key::PdfClock) {
                Val::Int(v) => v,
                _ => 0,
            },
            13 => self.pdf_randoms().random_seed,
            _ => crate::prim::PDF_ITEMS.get(usize::from(k)).map_or(0, |x| x.1),
        }
    }

    /// The random generator's state (seeded from the given seed at first).
    fn pdf_randoms(&mut self) -> crate::pdf::rand::Randoms {
        if let Val::Penalties(v) = self.get(&Key::PdfRandom)
            && let Some(r) = crate::pdf::rand::Randoms::from_ints(&v)
        {
            return r;
        }
        let seed = match self.get(&Key::PdfSeed) {
            Val::Int(s) => s,
            _ => 0,
        };
        crate::pdf::rand::Randoms::seeded(seed)
    }

    /// An expandable PDF command (`Prim::PdfX(i)`, `conv_toks`): its
    /// result's characters are the input.
    #[allow(clippy::too_many_lines)]
    pub(super) fn pdf_expand(&mut self, i: u16) {
        let name = PDFTEX_PRIMITIVES[usize::from(i)].0;
        let mut s = String::new();
        match name {
            "pdfcolorstackinit" => {
                let page = self.keyword("page");
                let mode = if self.keyword("direct") {
                    what::DIRECT_ALWAYS
                } else if self.keyword("page") {
                    what::DIRECT_PAGE
                } else {
                    what::SET_ORIGIN
                };
                let data = self.pdf_text();
                let i = self.pdf_event(Event::ColorStackInit { page, mode, data });
                let mut v = self.pdf_answer(i, 0).unwrap_or(1);
                if v < 0 {
                    self.error("Too many color stacks", &[
                        "The number of color stacks is limited to 32768.",
                        "I'll use the default color stack 0 here.",
                    ]);
                    v = 0;
                }
                let _ = write!(s, "{v}");
            }
            "pdfcreationdate" => {
                if let Val::Name(d) = self.get(&Key::PdfDate) {
                    s.push_str(&d);
                }
            }
            "pdffilemoddate" => {
                // (the file's date: the build's given date, CORE.md)
                let n = self.pdf_text();
                let n: String = String::from_utf8_lossy(&n).replace('"', "");
                if self.find_file(&n).is_some()
                    && let Val::Name(d) = self.get(&Key::PdfDate)
                {
                    s.push_str(&d);
                }
            }
            "pdffiledump" => {
                let mut offset = 0;
                if self.keyword("offset") {
                    offset = self.scan_int();
                    if offset < 0 {
                        self.error(&format!("Bad file offset ({offset})"), &[
                            "A file offset must be between 0 and 2^{31}-1,",
                            "I changed this one to zero.",
                        ]);
                        offset = 0;
                    }
                }
                let mut length = 0;
                if self.keyword("length") {
                    length = self.scan_int();
                    if length < 0 {
                        self.error(&format!("Bad dump length ({length})"), &[
                            "A dump length must be between 0 and 2^{31}-1,",
                            "I changed this one to zero.",
                        ]);
                        length = 0;
                    }
                }
                let n = self.pdf_text();
                let n: String = String::from_utf8_lossy(&n).replace('"', "");
                if length > 0
                    && let Some((_, b)) = self.find_file(&n)
                {
                    let from = (offset as usize).min(b.len());
                    let to = (from + length as usize).min(b.len());
                    for x in &b[from..to] {
                        let _ = write!(s, "{x:02X}");
                    }
                }
            }
            "pdfmdfivesum" => {
                let file = self.keyword("file");
                let t = self.pdf_text();
                let digest = if file {
                    let n: String = String::from_utf8_lossy(&t).replace('"', "");
                    self.find_file(&n).map(|(_, b)| crate::pdf::md5::md5(&b))
                } else {
                    Some(crate::pdf::md5::md5(&t))
                };
                if let Some(d) = digest {
                    for x in d {
                        let _ = write!(s, "{x:02X}");
                    }
                }
            }
            "pdffontname" | "pdffontobjnum" => {
                let Some(f) = self.pdf_font() else { return };
                let i = self.pdf_event(Event::FontInit(f));
                let v = if name == "pdffontname" { self.pdf_answer(i, 1) } else { self.pdf_answer(i, 0) };
                let _ = write!(s, "{}", v.unwrap_or(0));
            }
            "pdffontsize" => {
                let Some(f) = self.pdf_font() else { return };
                crate::print::scaled(&mut s, f.size);
                s.push_str("pt");
            }
            "pdfpageref" => {
                let n = self.scan_int();
                if n <= 0 {
                    self.pdf_fatal("pageref", "invalid page number");
                    return;
                }
                let i = self.pdf_event(Event::PageRef(n));
                let _ = write!(s, "{}", self.pdf_answer(i, 0).unwrap_or(0));
            }
            "pdfxformname" => {
                let n = self.scan_int();
                let Some(a) = self.pdf_check(what::OBJ_TYPE_XFORM, n) else { return };
                let _ = write!(s, "{}", a[4]);
            }
            "pdfximagebbox" => {
                let n = self.scan_int();
                if self.pdf_check(what::OBJ_TYPE_XIMAGE, n).is_none() {
                    return;
                }
                let j = self.scan_int();
                if !(1..=4).contains(&j) {
                    self.pdf_fatal("pdfximagebbox", "invalid parameter");
                    return;
                }
                let i = self.pdf_event(Event::XimageBbox(n, j));
                crate::print::scaled(&mut s, self.pdf_answer(i, 0).unwrap_or(0));
                s.push_str("pt");
            }
            "pdfuniformdeviate" => {
                let x = self.scan_int();
                let mut r = self.pdf_randoms();
                let v = r.unif_rand(x);
                self.define(Key::PdfRandom, Val::Penalties(Arc::from(r.to_ints())), true);
                let _ = write!(s, "{v}");
            }
            "pdfnormaldeviate" => {
                let mut r = self.pdf_randoms();
                let (v, e) = r.norm_rand();
                if let Some(e) = e {
                    self.err(e);
                }
                self.define(Key::PdfRandom, Val::Penalties(Arc::from(r.to_ints())), true);
                let _ = write!(s, "{v}");
            }
            "pdfmatch" => {
                let icase = self.keyword("icase");
                let sub = if self.keyword("subcount") { self.scan_int() } else { -1 };
                let pat = self.pdf_text();
                let txt = self.pdf_text();
                // (C strings: they end at a null byte)
                let cut = |b: &[u8]| -> Vec<u8> { b.iter().copied().take_while(|&c| c != 0).collect() };
                let (pat, txt) = (cut(&pat), cut(&txt));
                match crate::pdf::lib::regex::Regex::new(&pat, icase) {
                    Err(e) => {
                        self.log_push(&format!("\npdfTeX warning: \\pdfmatch: {}\n", String::from_utf8_lossy(e.message())));
                        s.push_str("-1");
                    }
                    Ok(re) => {
                        let count = if sub < 0 { 10 } else { sub as usize };
                        let caps = re.exec(&txt);
                        // (`\pdflastmatch n`'s answers, one a group, NUL-separated)
                        let mut m = String::new();
                        if let Some(c) = &caps {
                            for g in 0..count {
                                if g > 0 {
                                    m.push('\0');
                                }
                                match c.get(g).copied().flatten() {
                                    Some((a, b)) if b >= a => {
                                        let _ = write!(m, "{a}->");
                                        m.extend(txt[a..b].iter().map(|&x| char::from(x)));
                                    }
                                    _ => m.push_str("-1->"),
                                }
                            }
                        }
                        s.push(if caps.is_some() { '1' } else { '0' });
                        let v = if caps.is_some() { format!("{count}\0{m}") } else { String::new() };
                        self.define(Key::PdfMatch, Val::Name(Arc::from(v)), true);
                    }
                }
            }
            "pdflastmatch" => {
                let mut n = self.scan_int();
                if n < 0 {
                    self.error(&format!("Bad match number ({n})"), &[
                        "Since I expected zero or a positive number,",
                        "I changed this one to zero.",
                    ]);
                    n = 0;
                }
                let found = match self.get(&Key::PdfMatch) {
                    Val::Name(v) if !v.is_empty() => {
                        let mut parts = v.split('\0');
                        let count: usize = parts.next().and_then(|c| c.parse().ok()).unwrap_or(0);
                        if (n as usize) < count { parts.nth(n as usize).map(str::to_string) } else { None }
                    }
                    _ => None,
                };
                s.push_str(found.as_deref().unwrap_or("-1->"));
            }
            _ => {
                self.err(format!("\\{name} is not supported yet"));
                return;
            }
        }
        self.rewrite(crate::print::str_toks(&s));
    }
}
