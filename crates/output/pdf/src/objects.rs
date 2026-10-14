//! The object table (pdfTeX §694–§702): creating objects, writing them
//! directly or into object streams, objects found by identifier
//! (`get_obj`), and the user's objects: `\pdfobj` (pdfTeX §1544) and
//! `\pdfxform` (pdfTeX §1548).

use phitex_typeset::arith::Scaled;
use phitex_typeset::node::BoxNode;
use std::sync::Arc;

use crate::{
    FontSource, Ident, OBJ_TYPE_DEST, OBJ_TYPE_OBJ, OBJ_TYPE_OTHERS, OBJ_TYPE_PAGE, OBJ_TYPE_XFORM,
    Obj, PDF_OS_MAX_OBJS, PdfError, PdfState, WhatsitSource, int, pdf_error,
};

/// What a `\pdfobj` holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ObjBody {
    /// The object's text.
    Text(Vec<u8>),
    /// `file{...}`: the file's bytes, as the TeX side read them.
    File(Vec<u8>),
}

/// `obj_obj_data` and its flags.
#[derive(Clone, Debug)]
pub(crate) struct RawObj {
    stream: bool,
    attr: Option<Vec<u8>>,
    body: ObjBody,
}

/// A form: the box (until shipped), its attributes and resources, its
/// size, and its number for `/Fm` (`obj_info`).
#[derive(Clone, Debug)]
pub(crate) struct XForm {
    pub boxed: Option<Arc<BoxNode>>,
    pub attr: Option<Vec<u8>>,
    pub resources: Option<Vec<u8>>,
    pub width: Scaled,
    pub height: Scaled,
    pub depth: Scaled,
}

impl PdfState {
    // ---- the table (pdfTeX §698) ----

    /// `pdf_create_obj(t, i)`; a named destination or thread has `i` 0
    /// and its name in `name` (pdfTeX's negative string number).
    pub(crate) fn create_obj_id(&mut self, t: usize, id: &Ident) -> usize {
        let (i, name) = match id {
            Ident::Num(n) => (i64::from(*n), None),
            Ident::Name(s) => (-1, Some(s.clone())),
        };
        let n = self.create_obj(t, i);
        if t != OBJ_TYPE_OTHERS {
            self.index.insert((t, id.clone()), n);
        }
        if let Some(s) = name {
            if t == OBJ_TYPE_DEST {
                // append_dest_name
                self.dest_names.push((s.clone(), n));
            }
            self.aux.names.insert(n, s);
        }
        n
    }

    /// `pdf_create_obj(t, i)`.
    pub(crate) fn create_obj(&mut self, t: usize, i: i64) -> usize {
        self.objs.push(Obj {
            info: i,
            link: 0,
            offset: -2,
            os_idx: 0,
        });
        let n = self.objs.len() - 1;
        self.obj_ptr = n;
        self.rec_font(crate::replay::FontEv::New(t, i, n));
        if t == OBJ_TYPE_PAGE {
            self.index
                .insert((t, Ident::Num(i32::try_from(i).unwrap_or(i32::MAX))), n);
            let mut p = self.head[t];
            if p == 0 || self.objs[p].info < i {
                self.objs[n].link = p;
                self.head[t] = n;
            } else {
                let mut q = p;
                while p != 0 {
                    if self.objs[p].info < i {
                        break;
                    }
                    q = p;
                    p = self.objs[p].link;
                }
                self.objs[q].link = n;
                self.objs[n].link = p;
            }
        } else if t != OBJ_TYPE_OTHERS {
            self.objs[n].link = self.head[t];
            self.head[t] = n;
        }
        n
    }

    /// `pdf_new_objnum`.
    pub(crate) fn new_objnum_(&mut self) -> usize {
        self.create_obj(OBJ_TYPE_OTHERS, 0)
    }

    /// `sys_obj_ptr`.
    pub(crate) fn sys_obj_ptr(&self) -> usize {
        self.objs.len() - 1
    }

    /// `is_obj_written`.
    pub(crate) fn is_written(&self, k: usize) -> bool {
        self.objs[k].offset > -1
    }

    /// `is_obj_scheduled`.
    pub(crate) fn is_scheduled(&self, k: usize) -> bool {
        self.objs[k].offset > -2
    }

    /// `set_obj_scheduled`.
    pub(crate) fn set_scheduled(&mut self, k: usize) {
        if self.objs[k].offset == -2 {
            self.objs[k].offset = -1;
        }
    }

    /// `get_obj(t, i, byname)`: the object of type `t` with identifier
    /// `id`, made if there is none.
    pub(crate) fn get_obj(&mut self, t: usize, id: &Ident) -> usize {
        let k = if let Some(&r) = self.index.get(&(t, id.clone())) {
            r
        } else {
            self.rec_depth(true);
            let k = self.create_obj_id(t, id);
            self.rec_depth(false);
            k
        };
        self.rec_font(crate::replay::FontEv::Get(t, id.clone(), k));
        k
    }

    /// `pdf_os_switch`.
    fn os_switch(&mut self, on: bool) {
        self.os.mode = on && self.os.enable;
    }

    /// `pdf_os_prepare_obj`.
    fn os_prepare_obj(&mut self, i: usize, level: i32) {
        self.os_switch(level > 0 && self.fixed_ocl >= level);
        if self.os.mode {
            if self.os.cur == 0 {
                self.os.cur = self.new_objnum_();
                // the object stream is not the user's
                self.obj_ptr -= 1;
                self.os.list.clear();
                self.os.buf.clear();
            }
            self.objs[i].os_idx = i32::try_from(self.os.list.len()).unwrap_or(i32::MAX);
            self.objs[i].offset = int(self.os.cur);
            let at = self.os.buf.len();
            self.os.list.push((i, at));
        } else {
            self.objs[i].offset = int(self.offset());
            self.objs[i].os_idx = -1;
            self.chunk_objs.push(i);
        }
    }

    /// `pdf_begin_obj(i, pdf_os_level)`.
    pub(crate) fn begin_obj(&mut self, i: usize, level: i32) {
        self.check_pdfversion();
        self.os_prepare_obj(i, level);
        if !self.os.mode {
            self.print_int(int(i));
            self.print_ln(b" 0 obj");
        } else if self.p.compress_level == 0 {
            self.print(b"% ");
            self.print_int(int(i));
            self.print_ln(b" 0 obj");
        }
    }

    /// `pdf_begin_dict(i, pdf_os_level)`.
    pub(crate) fn begin_dict(&mut self, i: usize, level: i32) {
        self.begin_obj(i, level);
        self.print_ln(b"<<");
    }

    /// `pdf_new_obj(t, i, pdf_os)`.
    pub(crate) fn new_obj(&mut self, t: usize, i: i64, level: i32) -> usize {
        let n = self.create_obj(t, i);
        self.begin_obj(n, level);
        n
    }

    /// `pdf_new_dict(t, i, pdf_os)`.
    pub(crate) fn new_dict(&mut self, t: usize, i: i64, level: i32) -> usize {
        let n = self.create_obj(t, i);
        self.begin_dict(n, level);
        n
    }

    /// `pdf_end_obj`.
    pub(crate) fn end_obj(&mut self) {
        if self.os.mode {
            if self.os.list.len() == PDF_OS_MAX_OBJS {
                self.os_write_objstream();
            }
        } else {
            self.print_ln(b"endobj");
        }
    }

    /// `pdf_end_dict`.
    pub(crate) fn end_dict(&mut self) {
        if self.os.mode {
            self.print_ln(b">>");
            if self.os.list.len() == PDF_OS_MAX_OBJS {
                self.os_write_objstream();
            }
        } else {
            self.print_ln(b">>");
            self.print_ln(b"endobj");
        }
    }

    /// `pdf_os_write_objstream`.
    pub(crate) fn os_write_objstream(&mut self) {
        if self.os.cur == 0 {
            return;
        }
        let p = self.os.buf.len();
        let list = std::mem::take(&mut self.os.list);
        for (j, &(num, off)) in list.iter().enumerate() {
            self.print_int(int(num));
            self.print(b" ");
            self.print_int(int(off));
            if j % 10 == 9 {
                self.pdf_out(b'\n');
            } else {
                self.print(b" ");
            }
        }
        if let Some(last) = self.os.buf.last_mut() {
            *last = b'\n';
        }
        let q = self.os.buf.len();
        let cur = self.os.cur;
        self.begin_dict(cur, 0);
        self.print_ln(b"/Type /ObjStm");
        self.print(b"/N ");
        self.print_int_ln(int(list.len()));
        self.print(b"/First ");
        self.print_int_ln(int(q - p));
        self.begin_stream();
        let data = std::mem::take(&mut self.os.buf);
        self.print(&data[p..q]);
        self.print(&data[..p]);
        self.end_stream();
        self.os.cur = 0;
    }

    /// The objects in the current object stream (to end one at the end:
    /// `pdf_os_switch(true)`, `pdf_os_write_objstream`).
    pub(crate) fn os_flush(&mut self) {
        self.os_switch(true);
        self.os_write_objstream();
        self.os_switch(false);
    }

    // ---- the TeX side's objects ----

    /// `pdf_new_objnum`: an object number for `\pdfannot` (with or
    /// without `reserveobjnum`) and `\pdfstartlink`, given when the
    /// command is read.
    pub fn new_objnum(&mut self) -> usize {
        self.new_objnum_()
    }

    /// `\pdfpageref n`: page `n`'s object, made if no page or reference
    /// made it (`get_obj(obj_type_page, n, false)`).
    pub fn page_ref(&mut self, n: i32) -> usize {
        self.get_obj(OBJ_TYPE_PAGE, &Ident::Num(n))
    }

    /// `\pdfobj reserveobjnum` (pdfTeX §1544): `\pdflastobj`.
    pub fn obj_reserve(&mut self) -> usize {
        self.obj_count += 1;
        self.create_obj(OBJ_TYPE_OBJ, self.obj_count)
    }

    /// `\pdfobj [useobjnum n] [stream [attr{...}]] [file]{...}` (pdfTeX
    /// §1544): the object's number (`\pdflastobj`) and whether
    /// `useobjnum` was refused (`\pdfretval` becomes -1, "invalid
    /// object number being ignored").
    pub fn obj(
        &mut self,
        use_objnum: Option<i64>,
        stream: bool,
        attr: Option<Vec<u8>>,
        body: ObjBody,
    ) -> (usize, bool) {
        let mut refused = false;
        let mut k = None;
        if let Some(n) = use_objnum {
            match usize::try_from(n) {
                Ok(n) if n > 0 && n <= self.obj_ptr && !self.aux.raw.contains_key(&n) => {
                    k = Some(n);
                }
                _ => refused = true,
            }
        }
        let k = k.unwrap_or_else(|| {
            self.obj_count += 1;
            self.create_obj(OBJ_TYPE_OBJ, self.obj_count)
        });
        self.aux.raw.insert(
            k,
            Arc::new(RawObj {
                stream,
                attr: if stream { attr } else { None },
                body,
            }),
        );
        (k, refused)
    }

    /// `\immediate\pdfobj`: `pdf_write_obj(pdf_last_obj)`.
    ///
    /// # Errors
    /// "`\pdfobj reserveobjnum` cannot be used with \immediate".
    pub fn write_obj(&mut self, n: usize) -> Result<(), PdfError> {
        if !self.aux.raw.contains_key(&n) {
            return Err(pdf_error(
                "ext1",
                "`\\pdfobj reserveobjnum' cannot be used with \\immediate",
            ));
        }
        self.pdf_write_obj(n);
        Ok(())
    }

    /// `pdf_write_obj` (pdfTeX §1554).
    pub(crate) fn pdf_write_obj(&mut self, n: usize) {
        let Some(o) = self.aux.raw.get(&n).cloned() else {
            return;
        };
        if o.stream {
            self.begin_dict(n, 0);
            if let Some(a) = &o.attr {
                self.print_toks_ln(a);
            }
            self.begin_stream();
        } else {
            self.begin_obj(n, 1);
        }
        match &o.body {
            ObjBody::File(data) => {
                if !data.is_empty() {
                    self.print(data);
                    if !o.stream && self.buf().last().is_some_and(|&c| c != 10) {
                        self.pdf_out(10);
                    }
                }
            }
            ObjBody::Text(s) => {
                if o.stream {
                    self.print(s);
                } else {
                    self.print_ln(s);
                }
            }
        }
        // `delete_toks`: the text is gone once written
        if let Some(r) = self.aux.raw.get_mut(&n).map(Arc::make_mut) {
            r.body = ObjBody::Text(Vec::new());
            r.attr = None;
        }
        if o.stream {
            self.end_stream();
        } else {
            self.end_obj();
        }
    }

    /// Whether `n` is a `\pdfobj` (`pdf_check_obj(obj_type_obj, n)`).
    #[must_use]
    pub fn is_obj(&self, n: usize) -> bool {
        self.in_list(OBJ_TYPE_OBJ, n)
    }

    /// Whether `n` is a `\pdfxform` (`pdf_check_obj(obj_type_xform, n)`).
    #[must_use]
    pub fn is_xform(&self, n: usize) -> bool {
        self.in_list(OBJ_TYPE_XFORM, n)
    }

    /// `pdf_check_obj(t, n)`: whether `n` is in the list of type `t`.
    pub(crate) fn in_list(&self, t: usize, n: usize) -> bool {
        let mut k = self.head[t];
        while k != 0 && k != n {
            k = self.objs[k].link;
        }
        k != 0
    }

    /// `\pdfxform [attr{...}] [resources{...}] n` (pdfTeX §1548): the
    /// form's object number (`\pdflastxform`) for box `b`.
    pub fn xform(
        &mut self,
        attr: Option<Vec<u8>>,
        resources: Option<Vec<u8>>,
        b: BoxNode,
    ) -> usize {
        self.xform_count += 1;
        let k = self.create_obj(OBJ_TYPE_XFORM, self.xform_count);
        self.aux.xforms.insert(
            k,
            XForm {
                width: b.width,
                height: b.height,
                depth: b.depth,
                boxed: Some(Arc::new(b)),
                attr,
                resources,
            },
        );
        k
    }

    /// The size of form `n` (width, height, depth).
    #[must_use]
    pub fn xform_size(&self, n: usize) -> Option<(Scaled, Scaled, Scaled)> {
        self.aux
            .xforms
            .get(&n)
            .map(|x| (x.width, x.height, x.depth))
    }

    /// `\immediate\pdfxform`: the form is shipped now
    /// (`pdf_ship_out(obj_xform_box(pdf_last_xform), false)`).
    ///
    /// # Errors
    /// What shipping the form meets.
    pub fn write_xform(
        &mut self,
        n: usize,
        fonts: &dyn FontSource,
        whatsits: &mut WhatsitSource<'_>,
    ) -> Result<(), PdfError> {
        self.cur_form = n;
        let b = self
            .aux
            .xforms
            .get_mut(&n)
            .and_then(|x| x.boxed.take())
            .ok_or_else(|| pdf_error("ext1", "\\pdfxform cannot be used with a void box"))?;
        self.ship(&b, false, fonts, whatsits)
    }
}
