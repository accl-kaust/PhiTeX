//! Annotations, links, destinations, threads and outlines (pdfTeX
//! §1612–§1618 and the procedures declared for `pdf_hlist_out`), the
//! catalog's, info's, names' and trailer's texts, and their output at
//! the end (pdfTeX §794–§810).

use phitex_typeset::arith::Scaled;
use phitex_typeset::node::RUNNING;

use crate::ship::Frame;
use crate::{
    Action, DestKind, Ident, NAME_TREE_KIDS_MAX, OBJ_TYPE_DEST, OBJ_TYPE_OTHERS, OBJ_TYPE_OUTLINE,
    OBJ_TYPE_PAGE, OBJ_TYPE_STRUCT_DEST, OBJ_TYPE_THREAD, PDF_MAX_LINK_LEVEL, PdfError, PdfState,
    Rect, int, pdf_error,
};

/// An annotation's or link's rectangle and data (`obj_annot_ptr`).
#[derive(Clone, Debug)]
pub(crate) struct Mark {
    rect: Rect,
    /// The annotation's text, or the link's `attr`.
    data: Option<Vec<u8>>,
    /// A link's action.
    action: Option<Action>,
}

/// A destination met on a page (`obj_dest_ptr`).
#[derive(Clone, Debug)]
pub(crate) struct Dest {
    rect: Rect,
    kind: DestKind,
    struct_num: Option<i32>,
    named: bool,
}

impl Dest {}

/// An outline entry (`pdfmem_outline_size`).
#[derive(Clone, Debug, Default)]
pub(crate) struct Outline {
    action: usize,
    title: usize,
    count: i32,
    prev: usize,
    next: usize,
    first: usize,
    last: usize,
    parent: usize,
    attr: Option<Vec<u8>>,
}

/// A bead (`pdfmem_bead_size`).
#[derive(Clone, Debug)]
pub(crate) struct Bead {
    page: usize,
    rect: Rect,
    attr: Option<Vec<u8>>,
    next: usize,
    prev: usize,
    rect_obj: usize,
}

/// A `pdf_link_stack` record.
#[derive(Clone, Debug)]
pub(crate) struct LinkLevel {
    pub level: i32,
    width: Scaled,
    height: Scaled,
    depth: Scaled,
    attr: Option<Vec<u8>>,
    action: Action,
    /// `ref_link_node`: the link object whose rectangle `end_link` ends.
    pub objnum: usize,
}

impl LinkLevel {}

/// The thread being output (`last_thread`, `pdf_thread_wd`, ...).
#[derive(Clone, Debug, Default)]
pub(crate) struct ThreadState {
    /// The bead `last_thread` is.
    pub last: Option<usize>,
    wd: Scaled,
    ht: Scaled,
    pub dp: Scaled,
    id: Option<Ident>,
    pub level: i32,
}

/// `str_less_str`: PDF strings compared as their unescaped bytes.
fn str_less_str(a: &[u8], b: &[u8]) -> bool {
    fn next(s: &[u8], j: &mut usize) -> u8 {
        let e = s.len();
        let mut c = s[*j];
        *j += 1;
        if c == b'\\' && *j < e {
            c = s[*j];
            *j += 1;
            if (b'0'..=b'7').contains(&c) {
                let mut v = u32::from(c - b'0');
                if *j < e && (b'0'..=b'7').contains(&s[*j]) {
                    v = 8 * v + u32::from(s[*j] - b'0');
                    *j += 1;
                    if *j < e && (b'0'..=b'7').contains(&s[*j]) && v < 32 {
                        v = 8 * v + u32::from(s[*j] - b'0');
                        *j += 1;
                    }
                }
                // `packed_ASCII_code`
                c = (v & 0xFF) as u8;
            } else {
                c = match c {
                    b'b' => 8,
                    b'f' => 12,
                    b'n' => 10,
                    b'r' => 13,
                    b't' => 9,
                    _ => c,
                };
            }
        }
        c
    }
    let (mut j1, mut j2) = (0, 0);
    while j1 < a.len() && j2 < b.len() {
        let c1 = next(a, &mut j1);
        let c2 = next(b, &mut j2);
        if c1 < c2 {
            return true;
        }
        if c1 > c2 {
            return false;
        }
    }
    j1 >= a.len() && j2 < b.len()
}

/// `sort_dest_names(l, r)`: pdfTeX's quicksort, whose order of equal
/// names the output shows.
fn sort_dest_names(v: &mut [(Vec<u8>, usize)], l: isize, r: isize) {
    let at = |k: isize| usize::try_from(k).unwrap_or(0);
    let mut i = l;
    let mut j = r;
    let s = v[at(isize::midpoint(l, r))].0.clone();
    loop {
        while str_less_str(&v[at(i)].0, &s) {
            i += 1;
        }
        while str_less_str(&s, &v[at(j)].0) {
            j -= 1;
        }
        if i <= j {
            v.swap(at(i), at(j));
            i += 1;
            j -= 1;
        }
        if i > j {
            break;
        }
    }
    if l < j {
        sort_dest_names(v, l, j);
    }
    if i < r {
        sort_dest_names(v, i, r);
    }
}

fn concat(to: &mut Option<Vec<u8>>, s: &[u8]) {
    to.get_or_insert_with(Vec::new).extend_from_slice(s);
}

impl PdfState {
    // ---- on the page ----

    /// `set_rect_dimens` (w, h, d of the node; `RUNNING` when not given).
    fn set_rect_dimens(
        &mut self,
        fr: &Frame,
        x: Scaled,
        y: Scaled,
        whd: [Scaled; 3],
        margin: Scaled,
    ) -> Rect {
        let [w, h, d] = whd;
        let mut r = Rect {
            left: self.cur_h,
            right: if w == RUNNING {
                x + fr.width
            } else {
                self.cur_h + w
            },
            top: if h == RUNNING {
                y - fr.height
            } else {
                self.cur_v - h
            },
            bottom: if d == RUNNING {
                y + fr.depth
            } else {
                self.cur_v + d
            },
        };
        if self.shipping_page && self.stacks.matrix_used() {
            let ph = self.cur_page_height;
            let [llx, lly, urx, ury] =
                self.stacks
                    .transform_rect(r.left, ph - r.bottom, r.right, ph - r.top);
            r = Rect {
                left: llx,
                bottom: ph - lly,
                right: urx,
                top: ph - ury,
            };
        }
        r.left -= margin;
        r.top -= margin;
        r.right += margin;
        r.bottom += margin;
        r
    }

    /// `do_annot`.
    pub(crate) fn do_annot(
        &mut self,
        objnum: usize,
        whd: [Scaled; 3],
        data: &[u8],
        fr: &Frame,
        x: Scaled,
        y: Scaled,
    ) {
        if !self.shipping_page {
            self.fail(pdf_error("ext4", "annotations cannot be inside an XForm"));
            return;
        }
        if self.doing_leaders {
            return;
        }
        // pdfTeX §1385 (`scan_annot`, `useobjnum`): a number that is not
        // an object of this file is an error, never an index. (A run fed
        // another mode's handles, before the engine runs it again with
        // its own, meets this too.)
        if objnum == 0 || objnum > self.sys_obj_ptr() {
            self.fail(pdf_error("ext1", "invalid object number being reused"));
            return;
        }
        let was = self.is_scheduled(objnum);
        self.rec_font(crate::replay::FontEv::Sched(objnum, was));
        let objnum = if was { self.new_objnum_() } else { objnum };
        let rect = self.set_rect_dimens(fr, x, y, whd, 0);
        self.aux.marks.insert(
            objnum,
            Mark {
                rect,
                data: Some(data.to_vec()),
                action: None,
            },
        );
        self.annot_list.push(objnum);
        self.rec_font(crate::replay::FontEv::Scheduled(objnum));
        self.set_scheduled(objnum);
    }

    /// `do_link`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn do_link(
        &mut self,
        objnum: usize,
        whd: [Scaled; 3],
        attr: Option<&Vec<u8>>,
        action: &Action,
        fr: &Frame,
        x: Scaled,
        y: Scaled,
    ) {
        if !self.shipping_page {
            self.fail(pdf_error(
                "ext4",
                "link annotations cannot be inside an XForm",
            ));
            return;
        }
        // (pdfTeX numbers a link with `pdf_new_objnum`: a number that is
        // not an object of this file comes from another mode's handles,
        // a run the engine repeats with its own)
        if objnum == 0 || objnum > self.sys_obj_ptr() {
            self.fail(pdf_error("ext1", "invalid object number being reused"));
            return;
        }
        let was = self.is_scheduled(objnum);
        self.rec_font(crate::replay::FontEv::Sched(objnum, was));
        let objnum = if was { self.new_objnum_() } else { objnum };
        if self.link_stack.len() >= PDF_MAX_LINK_LEVEL {
            self.fail(pdf_error(
                "",
                "TeX capacity exceeded, sorry [pdf link stack size=10]",
            ));
            return;
        }
        self.link_stack.push(LinkLevel {
            level: self.cur_s,
            width: whd[0],
            height: whd[1],
            depth: whd[2],
            attr: attr.cloned(),
            action: action.clone(),
            objnum,
        });
        let rect = self.set_rect_dimens(fr, x, y, whd, self.p.link_margin);
        self.aux.marks.insert(
            objnum,
            Mark {
                rect,
                data: attr.cloned(),
                action: Some(action.clone()),
            },
        );
        self.link_list.push(objnum);
        self.rec_font(crate::replay::FontEv::Scheduled(objnum));
        self.set_scheduled(objnum);
    }

    /// `end_link`.
    pub(crate) fn end_link(&mut self) {
        let Some(top) = self.link_stack.last().cloned() else {
            self.fail(pdf_error(
                "ext4",
                "pdf_link_stack empty, \\pdfendlink used without \\pdfstartlink?",
            ));
            return;
        };
        if top.width == RUNNING {
            let margin = self.p.link_margin;
            let ph = self.cur_page_height;
            let new = if self.shipping_page && self.stacks.matrix_used() {
                let [llx, lly, urx, ury] = self.stacks.recalculate(self.cur_h + margin);
                Some(Rect {
                    left: llx - margin,
                    top: ph - ury - margin,
                    right: urx + margin,
                    bottom: ph - lly + margin,
                })
            } else {
                None
            };
            let cur_h = self.cur_h;
            if let Some(m) = self.aux.marks.get_mut(&top.objnum) {
                match new {
                    Some(r) => m.rect = r,
                    None => m.rect.right = cur_h + margin,
                }
            }
        }
        self.link_stack.pop();
    }

    /// `append_link`: link `i` continues in this hbox.
    pub(crate) fn append_link(&mut self, fr: &Frame, x: Scaled, y: Scaled, i: usize) {
        let l = self.link_stack[i].clone();
        let rect = self.set_rect_dimens(fr, x, y, [l.width, l.height, l.depth], self.p.link_margin);
        let k = self.create_obj(OBJ_TYPE_OTHERS, 0);
        self.link_stack[i].objnum = k;
        self.aux.marks.insert(
            k,
            Mark {
                rect,
                data: l.attr,
                action: Some(l.action),
            },
        );
        self.link_list.push(k);
    }

    /// `do_dest`.
    pub(crate) fn do_dest(
        &mut self,
        id: &Ident,
        struct_num: Option<i32>,
        kind: DestKind,
        fr: &Frame,
        x: Scaled,
        y: Scaled,
    ) {
        if !self.shipping_page {
            self.fail(pdf_error("ext4", "destinations cannot be inside an XForm"));
            return;
        }
        if self.doing_leaders {
            return;
        }
        let t = if struct_num.is_none() {
            OBJ_TYPE_DEST
        } else {
            OBJ_TYPE_STRUCT_DEST
        };
        let k = self.get_obj(t, id);
        if self.aux.dests.contains_key(&k) {
            // "has been already used, duplicate ignored"
            return;
        }
        let margin = self.p.dest_margin;
        let matrix = self.stacks.matrix_used();
        let running = [RUNNING; 3];
        let mut rect = Rect::default();
        match kind {
            DestKind::Xyz(_)
            | DestKind::FitH
            | DestKind::FitBH
            | DestKind::FitV
            | DestKind::FitBV
                if matrix =>
            {
                rect = self.set_rect_dimens(fr, x, y, running, margin);
            }
            DestKind::Xyz(_) => {
                rect.left = self.cur_h;
                rect.top = self.cur_v;
            }
            DestKind::FitH | DestKind::FitBH => rect.top = self.cur_v,
            DestKind::FitV | DestKind::FitBV => rect.left = self.cur_h,
            DestKind::Fit | DestKind::FitB => {}
            DestKind::FitR(w, h, d) => rect = self.set_rect_dimens(fr, x, y, [w, h, d], margin),
        }
        self.aux.dests.insert(
            k,
            Some(Dest {
                rect,
                kind,
                struct_num,
                named: matches!(id, Ident::Name(_)),
            }),
        );
        self.dest_list.push(k);
    }

    /// `append_bead`.
    fn append_bead(&mut self, id: &Ident, rect: Rect, attr: Option<Vec<u8>>) {
        if !self.shipping_page {
            self.fail(pdf_error("ext4", "threads cannot be inside an XForm"));
            return;
        }
        let t = self.get_obj(OBJ_TYPE_THREAD, id);
        let b = self.new_objnum_();
        let mut bead = Bead {
            page: self.last_page,
            rect,
            attr,
            next: b,
            prev: b,
            rect_obj: 0,
        };
        if let Some(&a) = self.aux.thread_first.get(&t) {
            let c = self.aux.beads[&a].prev;
            bead.prev = c;
            bead.next = a;
            if let Some(x) = self.aux.beads.get_mut(&a) {
                x.prev = b;
            }
            if let Some(x) = self.aux.beads.get_mut(&c) {
                x.next = b;
            }
        } else {
            self.aux.thread_first.insert(t, b);
        }
        self.aux.beads.insert(b, bead);
        self.bead_list.push(b);
        self.thread.last = Some(b);
    }

    /// `do_thread`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn do_thread(
        &mut self,
        start: bool,
        id: &Ident,
        whd: [Scaled; 3],
        attr: Option<&Vec<u8>>,
        fr: &Frame,
        x: Scaled,
        y: Scaled,
    ) {
        if self.doing_leaders {
            return;
        }
        if start {
            self.thread.wd = whd[0];
            self.thread.ht = whd[1];
            self.thread.dp = whd[2];
            self.thread.id = Some(id.clone());
            self.thread.level = self.cur_s;
        }
        let rect = self.set_rect_dimens(fr, x, y, whd, self.p.thread_margin);
        self.append_bead(id, rect, attr.cloned());
    }

    /// `append_thread`: the thread continues in this vbox.
    pub(crate) fn append_thread(&mut self, fr: &Frame, x: Scaled, y: Scaled) {
        let whd = [self.thread.wd, self.thread.ht, self.thread.dp];
        let Some(id) = self.thread.id.clone() else {
            return;
        };
        let rect = self.set_rect_dimens(fr, x, y, whd, self.p.thread_margin);
        self.append_bead(&id, rect, None);
    }

    /// `end_thread`.
    pub(crate) fn end_thread(&mut self) {
        if self.thread.level != self.cur_s {
            self.fail(pdf_error(
                "ext4",
                "\\pdfendthread ended up in different nesting level than \\pdfstartthread",
            ));
            return;
        }
        if self.thread.dp == RUNNING
            && let Some(b) = self.thread.last
        {
            let v = self.cur_v + self.p.thread_margin;
            if let Some(x) = self.aux.beads.get_mut(&b) {
                x.rect.bottom = v;
            }
        }
        self.thread.last = None;
    }

    /// `pdf_print_rect_spec`.
    fn print_rect_spec(&mut self, r: Rect) {
        self.print_mag_bp(r.left - self.origin_h);
        self.pdf_out(b' ');
        self.print_mag_bp(self.origin_v - r.bottom);
        self.pdf_out(b' ');
        self.print_mag_bp(r.right - self.origin_h);
        self.pdf_out(b' ');
        self.print_mag_bp(self.origin_v - r.top);
    }

    /// `pdf_rectangle`.
    fn rectangle(&mut self, r: Rect) {
        self.print(b"/Rect [");
        self.print_rect_spec(r);
        self.print_ln(b"]");
    }

    /// "Write out pending PDF marks" (pdfTeX §1627ff).
    #[allow(clippy::too_many_lines)]
    pub(crate) fn write_page_marks(&mut self) {
        self.origin_h = 0;
        self.origin_v = self.cur_page_height;
        for k in self.annot_list.clone() {
            let Some(m) = self.aux.marks.remove(&k) else {
                continue;
            };
            self.begin_dict(k, 1);
            self.print_ln(b"/Type /Annot");
            if let Some(d) = &m.data {
                self.print_toks_ln(d);
            }
            self.rectangle(m.rect);
            self.end_dict();
        }
        for k in self.link_list.clone() {
            let Some(m) = self.aux.marks.remove(&k) else {
                continue;
            };
            let Some(action) = m.action else {
                continue;
            };
            let user = matches!(action, Action::User(_));
            self.begin_dict(k, 1);
            self.print_ln(b"/Type /Annot");
            if !user {
                self.print_ln(b"/Subtype /Link");
            }
            if let Some(a) = &m.data {
                self.print_toks_ln(a);
            }
            self.rectangle(m.rect);
            if !user {
                self.print(b"/A ");
            }
            if let Err(e) = self.write_action(&action) {
                self.fail(e);
            }
            self.end_dict();
        }
        for k in self.dest_list.clone() {
            if self.is_written(k) {
                self.fail(pdf_error(
                    "ext5",
                    "destination has been already written (this shouldn't happen)",
                ));
                continue;
            }
            // (written: only that it was met stays)
            let Some(Some(d)) = self.aux.dests.insert(k, None) else {
                continue;
            };
            let dict = d.named && d.struct_num.is_none();
            if dict {
                self.begin_dict(k, 1);
                self.print(b"/D ");
            } else {
                self.begin_obj(k, 1);
            }
            self.pdf_out(b'[');
            match d.struct_num {
                None => self.print_int(int(self.last_page)),
                Some(n) => self.print_int(i64::from(n)),
            }
            self.print(b" 0 R ");
            let r = d.rect;
            let (oh, ov) = (self.origin_h, self.origin_v);
            match d.kind {
                DestKind::Xyz(zoom) => {
                    self.print(b"/XYZ ");
                    self.print_mag_bp(r.left - oh);
                    self.pdf_out(b' ');
                    self.print_mag_bp(ov - r.top);
                    self.pdf_out(b' ');
                    match zoom {
                        None => self.print(b"null"),
                        Some(z) => {
                            self.print_int(i64::from(z / 1000));
                            self.pdf_out(b'.');
                            self.print_int(i64::from(z % 1000));
                        }
                    }
                }
                DestKind::Fit => self.print(b"/Fit"),
                DestKind::FitH => {
                    self.print(b"/FitH ");
                    self.print_mag_bp(ov - r.top);
                }
                DestKind::FitV => {
                    self.print(b"/FitV ");
                    self.print_mag_bp(r.left - oh);
                }
                DestKind::FitB => self.print(b"/FitB"),
                DestKind::FitBH => {
                    self.print(b"/FitBH ");
                    self.print_mag_bp(ov - r.top);
                }
                DestKind::FitBV => {
                    self.print(b"/FitBV ");
                    self.print_mag_bp(r.left - oh);
                }
                DestKind::FitR(..) => {
                    self.print(b"/FitR ");
                    self.print_rect_spec(r);
                }
            }
            self.print_ln(b"]");
            if dict {
                self.end_dict();
            } else {
                self.end_obj();
            }
        }
        for k in self.bead_list.clone() {
            let n = self.new_obj(OBJ_TYPE_OTHERS, 0, 1);
            self.pdf_out(b'[');
            let r = self.aux.beads[&k].rect;
            self.print_rect_spec(r);
            self.print_ln(b"]");
            if let Some(b) = self.aux.beads.get_mut(&k) {
                b.rect_obj = n;
            }
            self.end_obj();
        }
    }

    /// `write_action` (pdfTeX §1618).
    #[allow(clippy::too_many_lines)]
    pub(crate) fn write_action(&mut self, a: &Action) -> Result<(), PdfError> {
        if let Action::User(t) = a {
            self.print_toks_ln(t);
            return Ok(());
        }
        self.print(b"<< ");
        let (file, new_window, struct_id) = match a {
            Action::Page {
                file,
                new_window,
                struct_id,
                ..
            }
            | Action::Goto {
                file,
                new_window,
                struct_id,
                ..
            } => (file.clone(), *new_window, struct_id.clone()),
            Action::Thread { file, .. } => (file.clone(), 0, None),
            Action::User(_) => (None, 0, None),
        };
        if let Some(f) = &file {
            self.print(b"/F ");
            if f.first() == Some(&b'(') && f.last() == Some(&b')') {
                self.print(f);
            } else {
                self.print_str(f);
            }
            self.print(b" ");
            if new_window > 0 {
                self.print(b"/NewWindow ");
                self.print(if new_window == 1 { b"true " } else { b"false " });
            }
        }
        match a {
            Action::Page { page, spec, .. } => {
                if file.is_none() {
                    self.print(b"/S /GoTo /D [");
                    let k = self.get_obj(OBJ_TYPE_PAGE, &Ident::Num(*page));
                    self.print_int(int(k));
                    self.print(b" 0 R");
                } else {
                    self.print(b"/S /GoToR /D [");
                    self.print_int(i64::from(*page) - 1);
                }
                self.pdf_out(b' ');
                self.print(spec);
                self.pdf_out(b']');
            }
            Action::Goto { id, .. } => {
                let mut d = 0;
                if file.is_none() {
                    self.print(b"/S /GoTo ");
                    d = self.get_obj(OBJ_TYPE_DEST, id);
                } else {
                    self.print(b"/S /GoToR ");
                }
                if let Ident::Name(n) = id {
                    self.print(b"/D ");
                    self.print_str(n);
                } else if file.is_none() {
                    self.indirect(b"D", d);
                } else {
                    return Err(pdf_error(
                        "ext4",
                        "`goto' option cannot be used with both `file' and `num'",
                    ));
                }
            }
            Action::Thread { id, .. } => {
                self.print(b"/S /Thread ");
                let mut d = 0;
                if file.is_none() {
                    d = self.get_obj(OBJ_TYPE_THREAD, id);
                }
                match id {
                    Ident::Name(n) => {
                        self.print(b"/D ");
                        self.print_str(n);
                    }
                    Ident::Num(_) if file.is_none() => self.indirect(b"D", d),
                    Ident::Num(n) => {
                        self.print(b"/D ");
                        self.print_int(i64::from(*n));
                    }
                }
            }
            Action::User(_) => {}
        }
        if let Some(sid) = &struct_id {
            self.pdf_out(b' ');
            if file.is_none() {
                let k = self.get_obj(OBJ_TYPE_STRUCT_DEST, sid);
                self.indirect(b"SD", k);
            } else {
                self.print(b"/SD ");
                match sid {
                    Ident::Name(n) => self.print(n),
                    Ident::Num(n) => self.print_int(i64::from(*n)),
                }
            }
        }
        self.print_ln(b" >>");
        Ok(())
    }

    // ---- when the commands are read ----

    /// `outline_list_count`.
    fn outline_list_count(&self, mut p: usize) -> i32 {
        let mut k = 1;
        while self.aux.outlines[&p].prev != 0 {
            k += 1;
            p = self.aux.outlines[&p].prev;
        }
        k
    }

    fn ol(&mut self, k: usize) -> &mut Outline {
        self.aux.outlines.get_or_insert_with(k, Outline::default)
    }

    /// `\pdfoutline [attr{...}] <action> [count n] {title}` (pdfTeX
    /// §1560): the action, the entry and its title are objects now.
    ///
    /// # Errors
    /// What `write_action` refuses.
    pub fn outline(
        &mut self,
        attr: Option<Vec<u8>>,
        action: &Action,
        count: i32,
        title: &[u8],
    ) -> Result<(), PdfError> {
        let j = self.new_obj(OBJ_TYPE_OTHERS, 0, 1);
        self.write_action(action)?;
        self.end_obj();
        let k = self.create_obj(OBJ_TYPE_OUTLINE, 0);
        self.new_obj(OBJ_TYPE_OTHERS, 0, 1);
        self.print_str(title);
        self.pdf_out(b'\n');
        self.end_obj();
        let title_obj = self.obj_ptr;
        let parent = self.parent_outline;
        self.aux.outlines.insert(
            k,
            Outline {
                action: j,
                title: title_obj,
                count,
                parent,
                attr,
                ..Outline::default()
            },
        );
        if self.first_outline == 0 {
            self.first_outline = k;
        }
        if self.last_outline == 0 {
            if parent != 0 {
                self.ol(parent).first = k;
            }
        } else {
            let last = self.last_outline;
            self.ol(last).next = k;
            self.ol(k).prev = last;
        }
        self.last_outline = k;
        if count != 0 {
            self.parent_outline = k;
            self.last_outline = 0;
        } else if parent != 0
            && self.outline_list_count(k) == self.aux.outlines[&parent].count.abs()
        {
            let mut j = self.last_outline;
            loop {
                let p = self.parent_outline;
                self.ol(p).last = j;
                j = p;
                self.parent_outline = self.aux.outlines[&p].parent;
                let pp = self.parent_outline;
                if pp == 0 || self.outline_list_count(j) < self.aux.outlines[&pp].count.abs() {
                    break;
                }
            }
            self.last_outline = if self.parent_outline == 0 {
                self.first_outline
            } else {
                self.aux.outlines[&self.parent_outline].first
            };
            while self.aux.outlines[&self.last_outline].next != 0 {
                self.last_outline = self.aux.outlines[&self.last_outline].next;
            }
        }
        Ok(())
    }

    /// `\pdfcatalog{...} [openaction <action>]` (pdfTeX §1564).
    ///
    /// # Errors
    /// "duplicate of openaction", or what `write_action` refuses.
    pub fn catalog(&mut self, toks: &[u8], openaction: Option<&Action>) -> Result<(), PdfError> {
        concat(&mut self.catalog_toks, toks);
        if let Some(a) = openaction {
            if self.catalog_openaction != 0 {
                return Err(pdf_error("ext1", "duplicate of openaction"));
            }
            let n = self.new_obj(OBJ_TYPE_OTHERS, 0, 1);
            self.catalog_openaction = n;
            self.write_action(a)?;
            self.end_obj();
        }
        Ok(())
    }

    /// `\pdfinfo{...}`.
    pub fn info(&mut self, toks: &[u8]) {
        concat(&mut self.info_toks, toks);
    }

    /// `\pdfnames{...}`.
    pub fn names(&mut self, toks: &[u8]) {
        concat(&mut self.names_toks, toks);
    }

    /// `\pdftrailer{...}`.
    pub fn trailer(&mut self, toks: &[u8]) {
        concat(&mut self.trailer_toks, toks);
    }

    /// `\pdftrailerid{...}`.
    pub fn trailer_id(&mut self, toks: &[u8]) {
        concat(&mut self.trailer_id_toks, toks);
    }

    // ---- at the end ----

    /// `pdf_fix_dest` for every destination never met (pdfTeX §796).
    pub(crate) fn fix_dests(&mut self) {
        let mut k = self.head[OBJ_TYPE_DEST];
        while k != 0 {
            if !self.aux.dests.contains_key(&k) {
                self.begin_obj(k, 1);
                self.pdf_out(b'[');
                self.print_int(int(self.head[OBJ_TYPE_PAGE]));
                self.print_ln(b" 0 R /Fit]");
                self.end_obj();
            }
            k = self.objs[k].link;
        }
    }

    /// `open_subentries`.
    fn open_subentries(&mut self, p: usize) -> i32 {
        let mut k = 0;
        let first = self.aux.outlines[&p].first;
        if first != 0 {
            let mut l = first;
            loop {
                k += 1;
                let c = self.open_subentries(l);
                if self.aux.outlines[&l].count > 0 {
                    k += c;
                }
                self.ol(l).parent = p;
                let r = self.aux.outlines[&l].next;
                if r == 0 {
                    self.ol(p).last = l;
                }
                l = r;
                if l == 0 {
                    break;
                }
            }
        }
        let o = self.ol(p);
        o.count = if o.count > 0 { k } else { -k };
        k
    }

    /// "Output outlines" (pdfTeX §804): the `/Outlines` object, or 0.
    pub(crate) fn output_outlines(&mut self) -> usize {
        if self.first_outline == 0 {
            return 0;
        }
        let outlines = self.new_dict(OBJ_TYPE_OTHERS, 0, 1);
        let mut l = self.first_outline;
        let mut k = 0;
        loop {
            k += 1;
            let a = self.open_subentries(l);
            if self.aux.outlines[&l].count > 0 {
                k += a;
            }
            self.ol(l).parent = outlines;
            l = self.aux.outlines[&l].next;
            if l == 0 {
                break;
            }
        }
        self.print_ln(b"/Type /Outlines");
        self.indirect_ln(b"First", self.first_outline);
        self.indirect_ln(b"Last", self.last_outline);
        self.int_entry_ln(b"Count", i64::from(k));
        self.end_dict();
        let mut k = self.head[OBJ_TYPE_OUTLINE];
        while k != 0 {
            let o = self.aux.outlines[&k].clone();
            if o.parent == self.parent_outline {
                if o.prev == 0 {
                    self.first_outline = k;
                }
                if o.next == 0 {
                    self.last_outline = k;
                }
            }
            self.begin_dict(k, 1);
            self.indirect_ln(b"Title", o.title);
            self.indirect_ln(b"A", o.action);
            for (key, v) in [
                (&b"Parent"[..], o.parent),
                (b"Prev", o.prev),
                (b"Next", o.next),
                (b"First", o.first),
                (b"Last", o.last),
            ] {
                if v != 0 {
                    self.indirect_ln(key, v);
                }
            }
            if o.count != 0 {
                self.int_entry_ln(b"Count", i64::from(o.count));
            }
            if let Some(a) = &o.attr {
                self.print_toks_ln(a);
            }
            self.end_dict();
            k = self.objs[k].link;
        }
        outlines
    }

    /// "Output name tree" (pdfTeX §805): the `/Names` object, or 0.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn output_name_tree(&mut self) -> usize {
        let mut dests = 0;
        let n = self.dest_names.len();
        if n > 0 {
            // at the end: the names leave the state
            let mut names: Vec<(Vec<u8>, usize)> = (*self.dest_names).clone();
            self.dest_names.clear();
            let r = isize::try_from(n).unwrap_or(isize::MAX) - 1;
            sort_dest_names(&mut names, 0, r);
            // obj_info and obj_aux of the nodes: the limits
            let mut limits: std::collections::BTreeMap<usize, (Vec<u8>, Vec<u8>)> =
                std::collections::BTreeMap::new();
            let mut names_head = 0;
            let mut names_tail = 0;
            let mut k = 0;
            let mut is_names = true;
            let mut b = 0;
            'done: loop {
                let mut l;
                loop {
                    l = self.create_obj(OBJ_TYPE_OTHERS, 0);
                    if b == 0 {
                        b = l;
                    }
                    if names_head == 0 {
                        names_head = l;
                    } else {
                        self.objs[names_tail].link = l;
                    }
                    names_tail = l;
                    self.objs[names_tail].link = 0;
                    // "Output the current node in this level"
                    self.begin_dict(l, 1);
                    let mut j = 0;
                    let lim;
                    if is_names {
                        let first = names[k].0.clone();
                        self.print(b"/Names [");
                        loop {
                            let (s, o) = names[k].clone();
                            self.print_str(&s);
                            self.pdf_out(b' ');
                            self.print_int(int(o));
                            self.print(b" 0 R ");
                            j += 1;
                            k += 1;
                            if j == NAME_TREE_KIDS_MAX || k == n {
                                break;
                            }
                        }
                        self.remove_last_space();
                        self.print_ln(b"]");
                        lim = (first, names[k - 1].0.clone());
                        if k == n {
                            is_names = false;
                            k = names_head;
                            b = 0;
                        }
                    } else {
                        let first = limits[&k].0.clone();
                        let mut last;
                        self.print(b"/Kids [");
                        loop {
                            self.print_int(int(k));
                            self.print(b" 0 R ");
                            j += 1;
                            last = limits[&k].1.clone();
                            k = self.objs[k].link;
                            if j == NAME_TREE_KIDS_MAX || k == b || self.objs[k].link == 0 {
                                break;
                            }
                        }
                        self.remove_last_space();
                        self.print_ln(b"]");
                        if k == b {
                            b = 0;
                        }
                        lim = (first, last);
                    }
                    self.print(b"/Limits [");
                    self.print_str(&lim.0);
                    self.pdf_out(b' ');
                    self.print_str(&lim.1);
                    self.print_ln(b"]");
                    self.end_dict();
                    limits.insert(l, lim);
                    if b == 0 {
                        break;
                    }
                }
                if k == l {
                    dests = l;
                    break 'done;
                }
            }
        }
        if dests != 0 || self.names_toks.is_some() {
            let t = self.new_dict(OBJ_TYPE_OTHERS, 0, 1);
            if dests != 0 {
                self.indirect_ln(b"Dests", dests);
            }
            if let Some(s) = self.names_toks.take() {
                self.print_toks_ln(&s);
            }
            self.end_dict();
            t
        } else {
            0
        }
    }

    /// `thread_title`.
    fn thread_title(&mut self, t: usize) {
        self.print(b"/Title (");
        if let Some(s) = self.aux.names.get(&t).cloned() {
            self.print(&s);
        } else {
            self.print_int(self.objs[t].info);
        }
        self.print_ln(b")");
    }

    /// `out_thread` (with `pdf_fix_thread`).
    fn out_thread(&mut self, t: usize) {
        let Some(&first) = self.aux.thread_first.get(&t) else {
            // pdf_fix_thread
            let a = self.new_dict(OBJ_TYPE_OTHERS, 0, 0);
            self.indirect_ln(b"T", t);
            self.indirect_ln(b"V", a);
            self.indirect_ln(b"N", a);
            self.indirect_ln(b"P", self.head[OBJ_TYPE_PAGE]);
            self.print(b"/R [0 0 ");
            self.print_bp(self.p.page_width);
            self.pdf_out(b' ');
            self.print_bp(self.p.page_height);
            self.print_ln(b"]");
            self.end_dict();
            self.begin_dict(t, 1);
            self.print_ln(b"/I << ");
            self.thread_title(t);
            self.print_ln(b">>");
            self.indirect_ln(b"F", a);
            self.end_dict();
            return;
        };
        self.begin_dict(t, 1);
        let mut a = first;
        let mut last_attr = None;
        loop {
            if let Some(x) = &self.aux.beads[&a].attr {
                last_attr = Some(x.clone());
            }
            a = self.aux.beads[&a].next;
            if a == first {
                break;
            }
        }
        if let Some(s) = last_attr {
            self.print_ln(&s);
        } else {
            self.print_ln(b"/I << ");
            self.thread_title(t);
            self.print_ln(b">>");
        }
        self.indirect_ln(b"F", a);
        self.end_dict();
        loop {
            let bead = self.aux.beads[&a].clone();
            self.begin_dict(a, 1);
            if a == first {
                self.indirect_ln(b"T", t);
            }
            self.indirect_ln(b"V", bead.prev);
            self.indirect_ln(b"N", bead.next);
            self.indirect_ln(b"P", bead.page);
            self.indirect_ln(b"R", bead.rect_obj);
            self.end_dict();
            a = bead.next;
            if a == first {
                break;
            }
        }
    }

    /// "Output article threads" (pdfTeX §806): the `/Threads` array, or 0.
    pub(crate) fn output_threads(&mut self) -> usize {
        if self.head[OBJ_TYPE_THREAD] == 0 {
            return 0;
        }
        let threads = self.new_obj(OBJ_TYPE_OTHERS, 0, 1);
        self.pdf_out(b'[');
        let mut k = self.head[OBJ_TYPE_THREAD];
        while k != 0 {
            self.print_int(int(k));
            self.print(b" 0 R ");
            k = self.objs[k].link;
        }
        self.remove_last_space();
        self.print_ln(b"]");
        self.end_obj();
        let mut k = self.head[OBJ_TYPE_THREAD];
        while k != 0 {
            self.out_thread(k);
            k = self.objs[k].link;
        }
        threads
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_unescaped() {
        assert!(str_less_str(b"a", b"b"));
        assert!(str_less_str(b"a", b"ab"));
        assert!(!str_less_str(b"\\101", b"A"));
        assert!(!str_less_str(b"A", b"\\101"));
    }

    #[test]
    fn sorts_as_quicksort() {
        let mut v: Vec<(Vec<u8>, usize)> = [b"c", b"a", b"b"]
            .iter()
            .enumerate()
            .map(|(i, s)| (s.to_vec(), i))
            .collect();
        sort_dest_names(&mut v, 0, 2);
        assert_eq!(v.iter().map(|x| x.1).collect::<Vec<_>>(), [1, 2, 0]);
    }
}
