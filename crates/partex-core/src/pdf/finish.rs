//! pdfTeX §794–§810: finishing the PDF file: fonts, the pages tree, the
//! name tree, the catalog, the info dictionary and the cross-reference
//! table.

use alloc::vec::Vec;

use super::objtab::{
    Aux, Id, OBJ_TYPE_DEST, OBJ_TYPE_FONT, OBJ_TYPE_OTHERS, OBJ_TYPE_PAGE, OBJ_TYPE_PAGES,
    OBJ_TYPE_STRUCT_DEST,
};
use super::ship::PAGES_TREE_KIDS_MAX;
use crate::host::Host;
use crate::tex::{Jump, Tex};
use crate::track::Tracker;
use crate::web::*;

/// `name_tree_kids_max`.
const NAME_TREE_KIDS_MAX: usize = 6;

/// pdfTeX's `\pdftexversion` and `\pdftexrevision`.
const PDFTEX_VERSION: i32 = 140;
const PDFTEX_REVISION: &[u8] = b"29";

/// `str_less_str`'s characters: PDF string escapes resolved.
fn pdf_string_chars(s: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    let e = s.len();
    let mut j = 0;
    while j < e {
        let mut c = s[j];
        j += 1;
        if c == b'\\' && j < e {
            c = s[j];
            j += 1;
            if (b'0'..=b'7').contains(&c) {
                c -= b'0';
                if j < e && (b'0'..=b'7').contains(&s[j]) {
                    c = 8 * c + s[j] - b'0';
                    j += 1;
                    if j < e && (b'0'..=b'7').contains(&s[j]) && c < 32 {
                        c = 8 * c + s[j] - b'0';
                        j += 1;
                    }
                }
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
        out.push(c);
    }
    out
}

/// `str_less_str`.
fn str_less_str(a: &[u8], b: &[u8]) -> bool {
    pdf_string_chars(a) < pdf_string_chars(b)
}

/// `sort_dest_names`: pdfTeX's quicksort (names comparing equal keep its
/// order).
fn sort_dest_names(v: &mut [(alloc::sync::Arc<[u8]>, i32)], l: isize, r: isize) {
    let (mut i, mut j) = (l, r);
    let at = |k: isize| usize::try_from(k).unwrap_or(0);
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

/// The seed of the virtual ids of the objects made at the end of the job
/// (a step's is the hash of its position, `Machine::at`).
const END_OF_JOB_SEED: u64 = 0x656e_645f_6f66_5f6a; // "end_of_j"

impl<H: Host, T: Tracker> Tex<H, T> {
    /// pdfTeX §794: "Finish the PDF file" (reading and writing every
    /// table of the writer).
    pub(crate) fn finish_pdf_file(&mut self) -> Result<(), Jump> {
        use super::val::PDF_ALL;
        // (SSA mode: the objects the end of the job makes are named by its
        // seed and their order from here, as an applied call's are, not by
        // the step's count: the job may end in the step that shipped the
        // last page (plain's `\bye`, whose `\supereject` fires the output
        // routine in the same expansion as its `\end`), whose objects took
        // the first counts, and the step's count begun again named the
        // catalog and the pages tree as the page's own objects, so the
        // link found no mark for one of them)
        let seq = self.pdf.objs.ssa_call_seed(u128::from(END_OF_JOB_SEED));
        let r = self.writer_scope(PDF_ALL, PDF_ALL, Self::finish_pdf_file_now);
        self.pdf.objs.ssa_call_seed_end(seq);
        r
    }

    fn finish_pdf_file_now(&mut self) -> Result<(), Jump> {
        crate::progress::BOARD.phase(crate::progress::Phase::FinishPdf);
        if self.pdf.objs.virt && !self.pdf.objs.ssa.on {
            // (machine mode: the objects the end of the job makes are
            // named by their order from here, not by where the input is,
            // which in LaTeX is the `.aux` read back at `\end{document}`,
            // one line longer after a `\label`: else every one of them,
            // hundreds, got another virtual id, so the numbering log and
            // every effect naming them changed, `pdf/vnum.rs`)
            self.pdf.objs.vseed = END_OF_JOB_SEED;
            self.pdf.objs.vcount = 0;
        }
        let total_pages = self.pdf.ship.total_pages;
        if total_pages == 0 {
            self.print_nl(b"No pages of output.");
            if self.pdf.out.gone > 0 {
                self.pdftex_warn(b"dangling objects discarded, no output file produced.");
            }
            return Ok(());
        }
        let draft = self.pdf.out.fixed_draftmode != 0;
        if !draft {
            self.pdf.out.flush();
            if total_pages % PAGES_TREE_KIDS_MAX != 0 {
                let lp = self.pdf.ship.last_pages;
                self.pdf.objs.get_mut(lp).info = Id::Num(total_pages % PAGES_TREE_KIDS_MAX);
            }
            self.check_nonexisting_pages();
            let pages_tail = self.reverse_page_lists();
            self.fix_dests()?;
            self.output_fonts()?;
            self.output_pages_tree(pages_tail)?;
            let outlines = self.output_outlines()?;
            let names_tree = self.output_name_tree()?;
            let root = self.pdf_new_dict(OBJ_TYPE_OTHERS, 0, 1)?;
            self.pdf.out.print_ln(b"/Type /Catalog");
            let lp = self.pdf.ship.last_pages;
            self.pdf.out.indirect_ln(b"Pages", lp);
            if outlines != 0 {
                self.pdf.out.indirect_ln(b"Outlines", outlines);
            }
            if names_tree != 0 {
                self.pdf.out.indirect_ln(b"Names", names_tree);
            }
            if let Some(t) = self.pdf.catalog_toks.take() {
                self.pdf_print_text_ln(&t);
            }
            self.tracker
                .pdf_word_access(super::word::CATALOG_OPENACTION, false);
            if self.pdf.catalog_openaction != 0 {
                let a = self.pdf.catalog_openaction;
                self.pdf.out.indirect_ln(b"OpenAction", a);
            }
            self.pdf_end_dict();
            let omit_info = self.int_par(PDF_OMIT_INFO_DICT_CODE) != 0;
            if !omit_info {
                self.pdf_print_info()?;
            }
            let root_info = omit_info;
            if self.effects.is_some() {
                self.xref_effect(root, root_info)?;
            } else if self.pdf.out.os_enable {
                self.pdf_os_switch(true);
                self.pdf_os_write_objstream();
                self.pdf.out.flush();
                self.pdf_os_switch(false);
                self.output_xref_stream(root, root_info)?;
                self.pdf.out.flush();
            } else {
                self.output_obj_tab();
                // the trailer
                let sys = self.pdf.objs.sys_obj_ptr();
                let o = &mut *self.pdf.out;
                o.print_ln(b"trailer");
                o.print(b"<< ");
                o.int_entry_ln(b"Size", i64::from(sys + 1));
                o.indirect_ln(b"Root", root);
                if !omit_info {
                    o.indirect_ln(b"Info", sys);
                }
                if let Some(t) = self.pdf.trailer_toks.take() {
                    self.pdf_print_text_ln(&t);
                }
                self.print_trailer_id();
                self.pdf.out.print_ln(b" >>");
            }
            if self.effects.is_none() {
                let at = if self.pdf.out.os_enable {
                    let sys = self.pdf.objs.sys_obj_ptr();
                    self.pdf.objs.get(sys).offset
                } else {
                    self.pdf.out.save_offset
                };
                let o = &mut *self.pdf.out;
                o.print_ln(b"startxref");
                o.print_int_ln(at);
                o.print_ln(b"%%EOF");
                o.flush();
                self.pdf_write_pending();
            }
            self.print_nl(b"Output written on ");
            self.print_file_name(0, self.output_file_name(), 0);
            self.print_str(b" (");
            self.print_int(total_pages);
            self.print_str(b" page");
            if total_pages != 1 {
                self.print_char(b's');
            }
            self.print_str(b", ");
            let bytes = i32::try_from(self.pdf.out.offset()).unwrap_or(i32::MAX);
            self.print_length(self.pdf.out.file, bytes);
            self.print_str(b" bytes).");
        }
        if let Some(id) = self.pdf.out.file.take() {
            self.out_close(id);
        }
        if draft {
            self.pdf_warning(
                b"",
                b"\\pdfdraftmode enabled, not changing output pdf",
                true,
                true,
            );
        }
        Ok(())
    }

    /// pdfTeX §1333's "PDF statistics" (sizes of pdfTeX's tables, which
    /// partex does not have: the numbers are what pdfTeX would start
    /// with).
    pub(crate) fn pdf_statistics(&mut self) {
        use super::val::PDF_ALL;
        self.writer_scope(PDF_ALL, 0, Self::pdf_statistics_now);
    }

    fn pdf_statistics_now(&mut self) {
        let (obj_ptr, os_cntr, os_objidx) = if self.pdf.objs.virt {
            self.obj_observe();
            let n = self.pdf.objs.numbering();
            (n.obj_ptr, n.streams, n.idx)
        } else {
            let o = &self.pdf.out;
            (self.pdf.objs.obj_ptr, o.os_cntr, o.os_objidx)
        };
        let names = self.pdf.objs.dest_count();
        let grow = |used: usize, start: usize| {
            let mut size = start;
            while size < used {
                size += size / 5;
            }
            size
        };
        let used = usize::try_from(obj_ptr).unwrap_or(0);
        let streams = if os_cntr > 0 {
            let n = (os_cntr - 1) * super::out::PDF_OS_MAX_OBJS + os_objidx + 1;
            let s = if os_cntr > 1 { "s" } else { "" };
            alloc::format!(" {n} compressed objects within {os_cntr} object stream{s}\n")
        } else {
            alloc::string::String::new()
        };
        let text = alloc::format!(
            "\nPDF statistics:\n {obj_ptr} PDF objects out of {} (max. 8388607)\n{streams} {names} named destinations out of {} (max. 500000)\n 1 words of extra memory for PDF output out of 10000 (max. 10000000)\n",
            grow(used, 1000),
            grow(names, 1000),
        );
        self.wlog_bytes(text.as_bytes());
    }

    /// "Check for non-existing pages".
    fn check_nonexisting_pages(&mut self) {
        let mut k = self.pdf.objs.head(OBJ_TYPE_PAGE);
        while k != 0 && self.pdf.objs.get(k).aux == Aux::None {
            self.pdf_warning(b"dest", b"Page ", true, false);
            let n = self.pdf.objs.get(k).info.num();
            self.print_int(n);
            self.print_str(b" has been referenced but does not exist!");
            self.print_ln();
            self.print_ln();
            k = self.pdf.objs.get(k).link;
        }
        self.pdf.objs.set_head(OBJ_TYPE_PAGE, k);
    }

    /// "Reverse the linked list of Page and Pages objects"; returns
    /// `pages_tail`.
    fn reverse_page_lists(&mut self) -> i32 {
        let reverse = |t: &mut super::objtab::ObjTab, ty: usize| {
            let mut k = t.head(ty);
            let mut l = 0;
            while k != 0 {
                let i = t.get(k).link;
                t.get_mut(k).link = l;
                l = k;
                k = i;
            }
            t.set_head(ty, l);
        };
        reverse(&mut self.pdf.objs, OBJ_TYPE_PAGE);
        let tail = self.pdf.objs.head(OBJ_TYPE_PAGES);
        reverse(&mut self.pdf.objs, OBJ_TYPE_PAGES);
        tail
    }

    /// An identifier as `pdf_fix_dest` prints it.
    fn print_obj_id(&mut self, k: i32) {
        match self.pdf.objs.get(k).info.clone() {
            Id::Name(s) => {
                self.print_str(b"name{");
                self.print_str(&s);
                self.print_str(b"}");
            }
            Id::Num(n) => {
                self.print_str(b"num");
                self.print_int(n);
            }
        }
    }

    /// "Check for non-existing destinations" (and structure
    /// destinations).
    fn fix_dests(&mut self) -> Result<(), Jump> {
        let mut w = self.pdf.objs.walk(OBJ_TYPE_DEST);
        while let Some(k) = w.next(&self.pdf.objs) {
            if !matches!(self.pdf.objs.get(k).aux, Aux::Dest(_)) {
                self.pdf_warning(b"dest", b"", true, false);
                self.print_obj_id(k);
                self.print_str(b" has been referenced but does not exist, replaced by a fixed one");
                self.print_ln();
                self.print_ln();
                self.pdf_begin_obj(k, 1)?;
                self.pdf.out.out(b'[');
                let p = self.pdf.objs.head(OBJ_TYPE_PAGE);
                self.pdf.out.objnum(p);
                self.pdf.out.print_ln(b" 0 R /Fit]");
                self.pdf_end_obj();
            }
        }
        let mut w = self.pdf.objs.walk(OBJ_TYPE_STRUCT_DEST);
        while let Some(k) = w.next(&self.pdf.objs) {
            if !matches!(self.pdf.objs.get(k).aux, Aux::Dest(_)) {
                self.pdf_warning(b"structure dest", b"", false, false);
                self.print_obj_id(k);
                self.print_str(b" has been referenced but does not exist");
                self.print_ln();
                self.print_ln();
            }
        }
        Ok(())
    }

    /// "Output fonts definition".
    fn output_fonts(&mut self) -> Result<(), Jump> {
        // (the glyphs each font used: the ships' appends, made whole)
        self.glyphs_union();
        // (the glyphs used so far are read: a machine's `Glyphs` cell)
        self.glyphs_read = true;
        // (in the order the fonts were loaded, pdfTeX's numbers')
        let n = self.pdf.ship.fonts.len();
        for k in self.fonts.in_order() {
            if crate::fonts::fx(k) >= n {
                continue;
            }
            let pf = self.pdf_font_ref(k).clone();
            if pf.used && pf.map.as_ref().is_some_and(Option::is_some) && pf.num < 0 {
                let i = -pf.num;
                for c in 0..=255u8 {
                    if pf.marked(c) {
                        self.mark_glyph(i, c);
                    }
                }
                let (ai, ak) = (
                    self.pdf.font_attr.get(&i).cloned().unwrap_or_default(),
                    self.pdf.font_attr.get(&k).cloned().unwrap_or_default(),
                );
                if ai.is_empty() && !ak.is_empty() {
                    self.pdf.font_attr.insert(i, ak);
                } else if ak.is_empty() && !ai.is_empty() {
                    self.pdf.font_attr.insert(k, ai);
                } else if !ai.is_empty() && !ak.is_empty() && ai != ak {
                    self.pdf_warning(b"\\pdffontattr", b"fonts ", true, false);
                    self.print_font_identifier(i);
                    self.print_str(b" and ");
                    self.print_font_identifier(k);
                    self.print_str(
                        b" have conflicting attributes; I will ignore the attributes assigned to ",
                    );
                    self.print_font_identifier(i);
                    self.print_ln();
                    self.print_ln();
                }
            }
        }
        self.pdf.fontw.gen_tounicode = self.int_par(PDF_GEN_TOUNICODE_CODE);
        let mut w = self.pdf.objs.walk(OBJ_TYPE_FONT);
        while let Some(k) = w.next(&self.pdf.objs) {
            let f = self.pdf.objs.get(k).info.num();
            self.do_pdf_font(k, f)?;
        }
        self.write_fontstuff()
    }

    /// "Output pages tree".
    fn output_pages_tree(&mut self, mut pages_tail: i32) -> Result<(), Jump> {
        let a = self.pdf.objs.sys_obj_ptr() + 1;
        // (virtual numbers: the nodes made here, which `l < a` tells)
        let mut made = alloc::collections::BTreeSet::new();
        let mut l = self.pdf.objs.head(OBJ_TYPE_PAGES);
        let mut k = self.pdf.objs.head(OBJ_TYPE_PAGE);
        let mut b = 0;
        loop {
            let mut i = 0;
            let mut c = 0;
            let is_root = self.pdf.objs.get(l).link == 0;
            loop {
                if !is_root {
                    let info_l = self.pdf.objs.get(l).info.num();
                    if i % PAGES_TREE_KIDS_MAX == 0 {
                        let lp = self.pdf_new_objnum()?;
                        made.insert(lp);
                        self.pdf.ship.last_pages = lp;
                        if c == 0 {
                            c = lp;
                        }
                        self.pdf.objs.get_mut(pages_tail).link = lp;
                        pages_tail = lp;
                        self.pdf.objs.get_mut(lp).link = 0;
                        self.pdf.objs.get_mut(lp).info = Id::Num(info_l);
                    } else {
                        let lp = self.pdf.ship.last_pages;
                        let v = self.pdf.objs.get(lp).info.num() + info_l;
                        self.pdf.objs.get_mut(lp).info = Id::Num(v);
                    }
                }
                // output the current Pages object in this level
                self.pdf_begin_dict(l, 1)?;
                self.pdf.out.print_ln(b"/Type /Pages");
                let count = self.pdf.objs.get(l).info.num();
                self.pdf.out.int_entry_ln(b"Count", i64::from(count));
                if !is_root {
                    let lp = self.pdf.ship.last_pages;
                    self.pdf.out.indirect_ln(b"Parent", lp);
                }
                self.pdf.out.print(b"/Kids [");
                let mut j = 0;
                loop {
                    self.pdf.out.objnum(k);
                    self.pdf.out.print(b" 0 R ");
                    k = self.pdf.objs.get(k).link;
                    j += 1;
                    let old = if self.pdf.objs.virt {
                        !made.contains(&l)
                    } else {
                        l < a
                    };
                    if (old && j == count)
                        || k == 0
                        || (k == b && b != 0)
                        || j == PAGES_TREE_KIDS_MAX
                    {
                        break;
                    }
                }
                self.pdf.out.remove_last_space();
                self.pdf.out.print_ln(b"]");
                if k == 0 {
                    k = self.pdf.objs.head(OBJ_TYPE_PAGES);
                    self.pdf.objs.set_head(OBJ_TYPE_PAGES, 0);
                }
                if is_root
                    && let Some(s) = self.toks_loc_string(PDF_PAGES_ATTR_LOC)
                    && !s.is_empty()
                {
                    self.pdf.out.print_ln(&s);
                }
                self.pdf_end_dict();
                i += 1;
                l = self.pdf.objs.get(l).link;
                if l == c {
                    break;
                }
            }
            b = c;
            if l == 0 {
                return Ok(());
            }
        }
    }

    /// "Output name tree": the `Names` dictionary's object, or 0.
    fn output_name_tree(&mut self) -> Result<i32, Jump> {
        let mut names = self.pdf.objs.dest_names_read();
        let n = names.len();
        let mut dests = 0;
        if n > 0 {
            let r = isize::try_from(n).unwrap_or(0) - 1;
            sort_dest_names(&mut names, 0, r);
            // the nodes in order of creation: (object, first, last)
            let mut nodes: Vec<(i32, Vec<u8>, Vec<u8>)> = Vec::new();
            let mut k = 0usize;
            let mut is_names = true;
            let mut b: Option<usize> = None;
            'levels: loop {
                loop {
                    let obj = self.pdf_create_obj(OBJ_TYPE_OTHERS, Id::Num(0))?;
                    let l = nodes.len();
                    nodes.push((obj, Vec::new(), Vec::new()));
                    if b.is_none() {
                        b = Some(l);
                    }
                    self.pdf_begin_dict(obj, 1)?;
                    let mut j = 0;
                    if is_names {
                        nodes[l].1 = names[k].0.to_vec();
                        self.pdf.out.print(b"/Names [");
                        loop {
                            self.pdf.out.print_str(&names[k].0);
                            self.pdf.out.out(b' ');
                            self.pdf.out.objnum(names[k].1);
                            self.pdf.out.print(b" 0 R ");
                            j += 1;
                            k += 1;
                            if j == NAME_TREE_KIDS_MAX || k == n {
                                break;
                            }
                        }
                        self.pdf.out.remove_last_space();
                        self.pdf.out.print_ln(b"]");
                        nodes[l].2 = names[k - 1].0.to_vec();
                        if k == n {
                            is_names = false;
                            k = 0;
                            b = None;
                        }
                    } else {
                        let first = nodes[k].1.clone();
                        nodes[l].1 = first;
                        self.pdf.out.print(b"/Kids [");
                        loop {
                            self.pdf.out.objnum(nodes[k].0);
                            self.pdf.out.print(b" 0 R ");
                            j += 1;
                            let last = nodes[k].2.clone();
                            nodes[l].2 = last;
                            k += 1; // `obj_link`: the next node created
                            if j == NAME_TREE_KIDS_MAX || Some(k) == b || k + 1 == nodes.len() {
                                break;
                            }
                        }
                        self.pdf.out.remove_last_space();
                        self.pdf.out.print_ln(b"]");
                        if Some(k) == b {
                            b = None;
                        }
                    }
                    self.pdf.out.print(b"/Limits [");
                    let (first, last) = (nodes[l].1.clone(), nodes[l].2.clone());
                    self.pdf.out.print_str(&first);
                    self.pdf.out.out(b' ');
                    self.pdf.out.print_str(&last);
                    self.pdf.out.print_ln(b"]");
                    self.pdf_end_dict();
                    if b.is_none() {
                        break;
                    }
                }
                if k == nodes.len() - 1 {
                    dests = nodes[k].0;
                    break 'levels;
                }
            }
        }
        let names_toks = self.pdf.names_toks.take();
        if dests == 0 && names_toks.is_none() {
            return Ok(0);
        }
        let t = self.pdf_new_dict(OBJ_TYPE_OTHERS, 0, 1)?;
        if dests != 0 {
            self.pdf.out.indirect_ln(b"Dests", dests);
        }
        if let Some(t) = names_toks {
            self.pdf_print_text_ln(&t);
        }
        self.pdf_end_dict();
        Ok(t)
    }

    /// `pdf_print_info`.
    fn pdf_print_info(&mut self) -> Result<(), Jump> {
        self.pdf_new_dict(OBJ_TYPE_OTHERS, 0, 3)?;
        let info = self.pdf.info_toks.take().map(|t| {
            let t: partex_engine::node::Tokens = partex_engine::node::TokenList::shared(&t);
            self.tokens_string(&t)
        });
        let given = |k: &[u8]| {
            info.as_ref()
                .is_some_and(|s| super::ship::substr_of_str(k, s))
        };
        let (creator, producer, creation, moddate, trapped) = (
            given(b"/Creator"),
            given(b"/Producer"),
            given(b"/CreationDate"),
            given(b"/ModDate"),
            given(b"/Trapped"),
        );
        if !producer {
            let o = &mut *self.pdf.out;
            o.print(b"/Producer (pdfTeX-");
            o.print_int(i64::from(PDFTEX_VERSION / 100));
            o.out(b'.');
            o.print_int(i64::from(PDFTEX_VERSION % 100));
            o.out(b'.');
            o.print(PDFTEX_REVISION);
            o.print_ln(b")");
        }
        if let Some(s) = &info
            && !s.is_empty()
        {
            self.pdf.out.print_ln(s);
        }
        if !creator {
            self.pdf.out.str_entry_ln(b"Creator", b"TeX");
        }
        if self.int_par(PDF_INFO_OMIT_DATE_CODE) == 0 {
            let date = self.host.creation_date();
            self.clock_read(crate::track::Query::Now, &date);
            if !creation {
                self.pdf.out.print(b"/CreationDate (");
                self.pdf.out.print(&date);
                self.pdf.out.print_ln(b")");
            }
            if !moddate {
                self.pdf.out.print(b"/ModDate (");
                self.pdf.out.print(&date);
                self.pdf.out.print_ln(b")");
            }
        }
        if !trapped {
            self.pdf.out.print_ln(b"/Trapped /False");
        }
        let suppress = self.int_par(PDF_SUPPRESS_PTEX_INFO_CODE);
        if suppress % 2 == 0 {
            let banner = self.pdftex_banner();
            let key: &[u8] = if self.int_par(PDF_PTEX_USE_UNDERSCORE_CODE) > 0
                || self.int_par(PDF_MAJOR_VERSION_CODE) >= 2
            {
                b"PTEX_Fullbanner"
            } else {
                b"PTEX.Fullbanner"
            };
            self.pdf.out.str_entry_ln(key, &banner);
        }
        self.pdf_end_dict();
        Ok(())
    }

    /// With effects on: the rest of the file as an [`Effect::PdfXref`]
    /// (its offsets are the link's), after the last object stream. The
    /// engine renders it too, with the offsets it assumes, to know how
    /// long the file is.
    ///
    /// [`Effect::PdfXref`]: crate::effects::Effect::PdfXref
    fn xref_effect(&mut self, root: i32, omit_info: bool) -> Result<(), Jump> {
        use super::xref::{XEntry, Xref, XrefStream};
        if self.pdf.out.os_enable {
            self.pdf_os_switch(true);
            self.pdf_os_write_objstream();
            self.pdf.out.flush();
            self.pdf_os_switch(false);
        }
        self.pdf.out.flush();
        self.pdf_write_pending();
        let at = self.pdf.out.offset();
        if self.pdf.objs.virt {
            let xref = self.xref_virt(root, omit_info, at)?;
            // (the engine's own count of the bytes: offsets are the link's)
            let host = &mut self.host;
            let bytes = xref.render(at, &mut |_| 0, &mut |_| (0, 0), &mut |level, data| {
                host.deflate(level, data)
            });
            let o = &mut *self.pdf.out;
            o.gone += i64::try_from(bytes.len()).unwrap_or(0);
            o.last_byte = bytes.last().copied().unwrap_or(0);
            if let Some(file) = o.file {
                self.out_mark(crate::effects::Effect::PdfXref {
                    file,
                    xref: alloc::boxed::Box::new(xref),
                });
            }
            return Ok(());
        }
        let symbolic = self.pdf.out.symbolic;
        let entry = |e: &super::objtab::Entry, k: i32| {
            if e.offset <= -1 {
                XEntry::Free(e.link)
            } else if e.os_idx == -1 {
                XEntry::Byte(k)
            } else if symbolic {
                XEntry::Placed(k)
            } else {
                XEntry::InStream(
                    i32::try_from(e.offset).unwrap_or(0),
                    u8::try_from(e.os_idx).unwrap_or(0),
                )
            }
        };
        let xref = if self.pdf.out.os_enable {
            // `pdf_new_dict(obj_type_others, 0, 0)`, at a byte offset
            let num = self.pdf_create_obj(OBJ_TYPE_OTHERS, Id::Num(0))?;
            let e = self.pdf.objs.get_mut(num);
            (e.offset, e.os_idx) = (at, -1);
            self.link_free_objects();
            let obj_ptr = self.pdf.objs.obj_ptr;
            let o = &mut *self.pdf.out;
            o.indirect_ln(b"Root", root);
            if !omit_info {
                o.indirect_ln(b"Info", obj_ptr - 1);
            }
            if let Some(t) = self.pdf.trailer_toks.take() {
                self.pdf_print_text_ln(&t);
            }
            self.print_trailer_id();
            self.pdf.out.out(b'\n');
            let tail = self.pdf.out.take_buf();
            let sys = self.pdf.objs.sys_obj_ptr();
            Xref {
                stream: Some(XrefStream {
                    num,
                    obj_ptr,
                    level: self.int_par(PDF_COMPRESS_LEVEL_CODE),
                }),
                entries: (0..=sys).map(|k| entry(self.pdf.objs.get(k), k)).collect(),
                tail,
            }
        } else {
            let obj_ptr = self.pdf.objs.obj_ptr;
            self.link_free_objects();
            let sys = self.pdf.objs.sys_obj_ptr();
            let o = &mut *self.pdf.out;
            o.print_ln(b"trailer");
            o.print(b"<< ");
            o.int_entry_ln(b"Size", i64::from(sys + 1));
            o.indirect_ln(b"Root", root);
            if !omit_info {
                o.indirect_ln(b"Info", sys);
            }
            if let Some(t) = self.pdf.trailer_toks.take() {
                self.pdf_print_text_ln(&t);
            }
            self.print_trailer_id();
            self.pdf.out.print_ln(b" >>");
            let tail = self.pdf.out.take_buf();
            let mut entries: Vec<XEntry> = (0..=obj_ptr)
                .map(|k| entry(self.pdf.objs.get(k), k))
                .collect();
            entries[0] = XEntry::Free(self.pdf.objs.get(0).link);
            Xref {
                stream: None,
                entries,
                tail,
            }
        };
        let objs = &self.pdf.objs;
        let host = &mut self.host;
        let bytes = xref.render(
            at,
            &mut |k| objs.get(k).offset,
            &mut |k| {
                let e = objs.get(k);
                (
                    i32::try_from(e.offset).unwrap_or(0),
                    u8::try_from(e.os_idx).unwrap_or(0),
                )
            },
            &mut |level, data| host.deflate(level, data),
        );
        let o = &mut *self.pdf.out;
        o.gone += i64::try_from(bytes.len()).unwrap_or(0);
        o.last_byte = bytes.last().copied().unwrap_or(0);
        if let Some(file) = o.file {
            self.out_mark(crate::effects::Effect::PdfXref {
                file,
                xref: alloc::boxed::Box::new(xref),
            });
        }
        Ok(())
    }

    /// Print `bytes`, the length of output `file`. With effects on, the
    /// printed digits become [`Effect::Length`]s (one per stream the
    /// selector prints to), for the link to write the real length.
    ///
    /// [`Effect::Length`]: crate::effects::Effect::Length
    pub(crate) fn print_length(&mut self, file: Option<crate::host::WriteId>, bytes: i32) {
        use crate::effects::{Effect, Stream};
        let Some(file) = file.filter(|_| self.effects.is_some()) else {
            self.print_int(bytes);
            return;
        };
        if self.flow.on {
            // (the digits rendered with the rest of the text, `effects/flow.rs`)
            let mut op = alloc::vec![crate::effects::flow::LEN, 0];
            op.extend_from_slice(&file.0.to_le_bytes());
            op.extend_from_slice(&i64::from(bytes).to_le_bytes());
            self.flow_op(&op);
            self.print_int(bytes);
            self.flow_op(&[crate::effects::flow::LEN_END, 0]);
            return;
        }
        // (what is buffered before the digits goes out first)
        if self.log_file.id.is_some() {
            self.flush_log();
        }
        self.update_terminal();
        self.print_int(bytes);
        let assumed = i64::from(bytes);
        if let Some(log) = self.log_file.id
            && !self.log_file.buf.is_empty()
        {
            let text = core::mem::take(&mut self.log_file.buf);
            self.out_mark(Effect::Length {
                stream: Stream::File(log),
                file,
                assumed,
                text,
            });
        }
        if !self.term_buf.is_empty() {
            let text = core::mem::take(&mut self.term_buf);
            self.out_mark(Effect::Length {
                stream: Stream::Term,
                file,
                assumed,
                text,
            });
        }
    }

    /// [`Tex::xref_effect`]'s cross-reference section with virtual
    /// numbers: laid out by pdfTeX's numbers, which the end of the job
    /// observes (and so re-runs when they change); the free list over
    /// them, as "Build a linked list of free objects" builds it.
    fn xref_virt(
        &mut self,
        root: i32,
        omit_info: bool,
        at: i64,
    ) -> Result<super::xref::Xref, Jump> {
        use super::xref::{XEntry, Xref, XrefStream};
        let os = self.pdf.out.os_enable;
        let num = if os {
            // `pdf_new_dict(obj_type_others, 0, 0)`, at a byte offset
            let num = self.pdf_create_obj(OBJ_TYPE_OTHERS, Id::Num(0))?;
            let e = self.pdf.objs.get_mut(num);
            (e.offset, e.os_idx) = (at, -1);
            num
        } else {
            0
        };
        self.obj_observe();
        let n = self.pdf.objs.numbering();
        // (each number: written, and whether at a byte offset; an object
        // stream's number names no object and is written)
        let mut kind = Vec::with_capacity(usize::try_from(n.sys).unwrap_or(0) + 1);
        for k in 0..=n.sys {
            kind.push(match n.vid.get(&k) {
                Some(&v) => self.pdf.objs.entry(v).map_or((false, false), |e| {
                    (k != 0 && e.offset > -1, e.os_idx == -1)
                }),
                None => (k != 0, true),
            });
        }
        let mut next = alloc::vec![0; kind.len()];
        let mut l = 0usize;
        for (k, &(written, _)) in kind.iter().enumerate().skip(1) {
            if !written {
                next[l] = i32::try_from(k).unwrap_or(0);
                l = k;
            }
        }
        next[l] = 0;
        let entry = |k: usize| {
            let kk = i32::try_from(k).unwrap_or(0);
            match kind[k] {
                (false, _) => XEntry::Free(next[k]),
                (true, true) => XEntry::Byte(kk),
                (true, false) => XEntry::Placed(kk),
            }
        };
        self.pdf.out.forced = super::vnum::Forced(Some(n.clone()));
        let xref = if os {
            let o = &mut *self.pdf.out;
            o.indirect_ln(b"Root", root);
            if !omit_info {
                o.indirect_final_ln(b"Info", n.obj_ptr - 1);
            }
            if let Some(t) = self.pdf.trailer_toks.take() {
                self.pdf_print_text_ln(&t);
            }
            self.print_trailer_id();
            self.pdf.out.out(b'\n');
            let tail = self.pdf.out.take_buf();
            Xref {
                stream: Some(XrefStream {
                    num: n.of(num),
                    obj_ptr: n.obj_ptr,
                    level: self.int_par(PDF_COMPRESS_LEVEL_CODE),
                }),
                entries: (0..kind.len()).map(entry).collect(),
                tail,
            }
        } else {
            let o = &mut *self.pdf.out;
            o.print_ln(b"trailer");
            o.print(b"<< ");
            o.int_entry_ln(b"Size", i64::from(n.sys + 1));
            o.indirect_ln(b"Root", root);
            if !omit_info {
                o.indirect_final_ln(b"Info", n.sys);
            }
            if let Some(t) = self.pdf.trailer_toks.take() {
                self.pdf_print_text_ln(&t);
            }
            self.print_trailer_id();
            self.pdf.out.print_ln(b" >>");
            let tail = self.pdf.out.take_buf();
            let to = usize::try_from(n.obj_ptr).unwrap_or(0) + 1;
            Xref {
                stream: None,
                entries: (0..to.min(kind.len())).map(entry).collect(),
                tail,
            }
        };
        self.pdf.out.forced = super::vnum::Forced(None);
        Ok(xref)
    }

    /// "Build a linked list of free objects".
    fn link_free_objects(&mut self) {
        let sys = self.pdf.objs.sys_obj_ptr();
        let mut l = 0;
        self.pdf.objs.get_mut(0).offset = -2;
        for k in 1..=sys {
            if !self.pdf.objs.is_written(k) {
                self.pdf.objs.get_mut(l).link = k;
                l = k;
            }
        }
        self.pdf.objs.get_mut(l).link = 0;
    }

    /// "Output the cross-reference stream dictionary".
    fn output_xref_stream(&mut self, root: i32, omit_info: bool) -> Result<(), Jump> {
        self.pdf_new_dict(OBJ_TYPE_OTHERS, 0, 0)?;
        let sys = self.pdf.objs.sys_obj_ptr();
        let off = self.pdf.objs.get(sys).offset;
        let width: usize = if off / 256 > 16_777_215 {
            5
        } else if off > 16_777_215 {
            4
        } else if off > 65535 {
            3
        } else {
            2
        };
        self.link_free_objects();
        let obj_ptr = self.pdf.objs.obj_ptr;
        let o = &mut *self.pdf.out;
        o.print_ln(b"/Type /XRef");
        o.print(b"/Index [0 ");
        o.print_int(i64::from(obj_ptr + 1));
        o.print_ln(b"]");
        o.int_entry_ln(b"Size", i64::from(obj_ptr + 1));
        o.print(b"/W [1 ");
        o.print_int(i64::try_from(width).unwrap_or(0));
        o.print_ln(b" 1]");
        o.indirect_ln(b"Root", root);
        if !omit_info {
            o.indirect_ln(b"Info", obj_ptr - 1);
        }
        if let Some(t) = self.pdf.trailer_toks.take() {
            self.pdf_print_text_ln(&t);
        }
        self.print_trailer_id();
        self.pdf.out.out(b'\n');
        self.pdf_begin_stream();
        for k in 0..=sys {
            let e = self.pdf.objs.get(k);
            let (written, link, offset, idx) = (e.offset > -1, e.link, e.offset, e.os_idx);
            let o = &mut *self.pdf.out;
            if !written {
                o.out(0);
                o.out_bytes(i64::from(link), width);
                o.out(255);
            } else if idx == -1 {
                o.out(1);
                o.out_bytes(offset, width);
                o.out(0);
            } else {
                o.out(2);
                o.out_bytes(offset, width);
                o.out(u8::try_from(idx).unwrap_or(0));
            }
        }
        self.pdf_end_stream();
        Ok(())
    }

    /// "Output the `obj_tab`" (with "Build a linked list of free
    /// objects").
    fn output_obj_tab(&mut self) {
        let obj_ptr = self.pdf.objs.obj_ptr;
        self.link_free_objects();
        let o = &mut *self.pdf.out;
        o.save_offset = o.offset();
        o.print_ln(b"xref");
        o.print(b"0 ");
        o.print_int_ln(i64::from(obj_ptr + 1));
        let first = self.pdf.objs.get(0).link;
        self.pdf.out.print_fw_int(i64::from(first), 10);
        self.pdf.out.print_ln(b" 65535 f ");
        for k in 1..=obj_ptr {
            let e = self.pdf.objs.get(k);
            let (written, link, offset) = (e.offset > -1, e.link, e.offset);
            if written {
                self.pdf.out.print_fw_int(offset, 10);
                self.pdf.out.print_ln(b" 00000 n ");
            } else {
                self.pdf.out.print_fw_int(i64::from(link), 10);
                self.pdf.out.print_ln(b" 00000 f ");
            }
        }
    }

    /// utils.c's `printID` (or `printIDalt` for `\pdftrailerid`).
    fn print_trailer_id(&mut self) {
        let data = if let Some(t) = (*self.pdf.trailer_id_toks).clone() {
            let t: partex_engine::node::Tokens = partex_engine::node::TokenList::shared(&t);
            let s = self.tokens_string(&t);
            if s.is_empty() {
                return;
            }
            s
        } else {
            let mut d = self.host.creation_date();
            self.clock_read(crate::track::Query::Now, &d);
            let s = crate::input::ux(self.output_file_name());
            d.extend_from_slice(&self.str_pool[self.str_start[s]..self.str_start[s + 1]]);
            d
        };
        let digest = partex_engine::md5::md5(&data);
        let mut id = Vec::with_capacity(32);
        for b in digest {
            id.extend_from_slice(alloc::format!("{b:02X}").as_bytes());
        }
        let o = &mut *self.pdf.out;
        o.print(b"/ID [<");
        o.print(&id);
        o.print(b"> <");
        o.print(&id);
        o.print(b">]");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pdf_strings_compare_unescaped() {
        assert!(!str_less_str(b"\\(", b"("));
        assert!(!str_less_str(b"(", b"\\("));
        assert!(str_less_str(b"a", b"b"));
        assert!(str_less_str(b"a", b"ab"));
        assert_eq!(pdf_string_chars(b"\\101\\n"), b"A\n");
    }
}
