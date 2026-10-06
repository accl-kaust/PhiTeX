//! pdfTeX's outlines (bookmarks): `\pdfoutline` builds the tree as entries
//! arrive (pdfTeX §1565), and the finish writes it (§800–§801).

use super::objtab::{Aux, Id, OBJ_TYPE_OTHERS, OBJ_TYPE_OUTLINE, Outline};
use crate::host::Host;
use crate::tex::{Jump, Tex};
use crate::track::Tracker;

impl<H: Host, T: Tracker> Tex<H, T> {
    fn ol(&self, k: i32) -> &Outline {
        match &self.pdf.objs.get(k).aux {
            Aux::Outline(o) => o,
            _ => unreachable!("an outline object"),
        }
    }

    fn ol_mut(&mut self, k: i32) -> &mut Outline {
        match &mut self.pdf.objs.get_mut(k).aux {
            Aux::Outline(o) => o,
            _ => unreachable!("an outline object"),
        }
    }

    /// Before the outlines' first, last and parent are read and set (the
    /// tracker is told: `pdf::word`).
    fn outline_words_access(&self) {
        for k in [
            super::word::FIRST_OUTLINE,
            super::word::LAST_OUTLINE,
            super::word::PARENT_OUTLINE,
        ] {
            self.tracker.pdf_word_access(k, false);
            self.tracker.pdf_word_access(k, true);
        }
    }

    /// `outline_list_count`: the entries at `p`'s level up to `p`.
    fn outline_list_count(&self, mut p: i32) -> i32 {
        let mut k = 1;
        while self.ol(p).prev != 0 {
            k += 1;
            p = self.ol(p).prev;
        }
        k
    }

    /// pdfTeX §1565: `\pdfoutline`.
    pub(crate) fn implement_pdfoutline(&mut self) -> Result<(), Jump> {
        self.check_pdfoutput(b"\\pdfoutline", true)?;
        let attr = if self.scan_keyword(b"attr")? {
            Some(self.scan_pdf_ext_toks()?)
        } else {
            None
        };
        let action = self.scan_action()?;
        let count = if self.scan_keyword(b"count")? {
            self.scan_int()?;
            self.cur_val
        } else {
            0
        };
        let q = self.scan_pdf_ext_toks()?;
        self.outline_words_access();
        let j = self.pdf_new_obj(OBJ_TYPE_OTHERS, 0, 1)?;
        self.write_action(&action)?;
        self.pdf_end_obj();
        let k = self.pdf_create_obj(OBJ_TYPE_OUTLINE, Id::Num(0))?;
        let title = self.pdf_new_obj(OBJ_TYPE_OTHERS, 0, 1)?;
        let s = self.tokens_string(&q);
        self.pdf.out.print_str(&s);
        self.pdf.out.out(b'\n');
        self.pdf_end_obj();
        let parent = self.pdf.parent_outline;
        self.pdf.objs.get_mut(k).aux = Aux::Outline(alloc::boxed::Box::new(Outline {
            title,
            parent,
            action_objnum: j,
            count,
            attr,
            ..Outline::default()
        }));
        if self.pdf.first_outline == 0 {
            self.pdf.first_outline = k;
        }
        let last = self.pdf.last_outline;
        if last == 0 {
            if parent != 0 {
                self.ol_mut(parent).first = k;
            }
        } else {
            self.ol_mut(last).next = k;
            self.ol_mut(k).prev = last;
        }
        self.pdf.last_outline = k;
        if count != 0 {
            self.pdf.parent_outline = k;
            self.pdf.last_outline = 0;
        } else if parent != 0 && self.outline_list_count(k) == self.ol(parent).count.abs() {
            let mut j = self.pdf.last_outline;
            loop {
                let p = self.pdf.parent_outline;
                self.ol_mut(p).last = j;
                j = p;
                self.pdf.parent_outline = self.ol(p).parent;
                let pp = self.pdf.parent_outline;
                if pp == 0 || self.outline_list_count(j) < self.ol(pp).count.abs() {
                    break;
                }
            }
            let pp = self.pdf.parent_outline;
            let mut l = if pp == 0 {
                self.pdf.first_outline
            } else {
                self.ol(pp).first
            };
            while self.ol(l).next != 0 {
                l = self.ol(l).next;
            }
            self.pdf.last_outline = l;
        }
        Ok(())
    }

    /// `open_subentries`.
    fn open_subentries(&mut self, p: i32) -> i32 {
        let mut k = 0;
        let mut l = self.ol(p).first;
        while l != 0 {
            k += 1;
            let c = self.open_subentries(l);
            if self.ol(l).count > 0 {
                k += c;
            }
            self.ol_mut(l).parent = p;
            let r = self.ol(l).next;
            if r == 0 {
                self.ol_mut(p).last = l;
            }
            l = r;
        }
        let o = self.ol_mut(p);
        o.count = if o.count > 0 { k } else { -k };
        k
    }

    /// pdfTeX §800–§801: "Output outlines"; the `/Outlines` object, or 0.
    pub(crate) fn output_outlines(&mut self) -> Result<i32, Jump> {
        self.outline_words_access();
        if self.pdf.first_outline == 0 {
            return Ok(0);
        }
        let outlines = self.pdf_new_dict(OBJ_TYPE_OTHERS, 0, 1)?;
        let mut l = self.pdf.first_outline;
        let mut k = 0;
        loop {
            k += 1;
            let a = self.open_subentries(l);
            if self.ol(l).count > 0 {
                k += a;
            }
            self.ol_mut(l).parent = outlines;
            l = self.ol(l).next;
            if l == 0 {
                break;
            }
        }
        let o = &mut *self.pdf.out;
        o.print_ln(b"/Type /Outlines");
        o.indirect_ln(b"First", self.pdf.first_outline);
        o.indirect_ln(b"Last", self.pdf.last_outline);
        o.int_entry_ln(b"Count", i64::from(k));
        self.pdf_end_dict();
        // the entries
        let mut w = self.pdf.objs.walk(OBJ_TYPE_OUTLINE);
        while let Some(k) = w.next(&self.pdf.objs) {
            let e = self.ol(k).clone();
            if e.parent == self.pdf.parent_outline {
                if e.prev == 0 {
                    self.pdf.first_outline = k;
                }
                if e.next == 0 {
                    self.pdf.last_outline = k;
                }
            }
            self.pdf_begin_dict(k, 1)?;
            let o = &mut *self.pdf.out;
            o.indirect_ln(b"Title", e.title);
            o.indirect_ln(b"A", e.action_objnum);
            for (key, v) in [
                (&b"Parent"[..], e.parent),
                (b"Prev", e.prev),
                (b"Next", e.next),
                (b"First", e.first),
                (b"Last", e.last),
            ] {
                if v != 0 {
                    o.indirect_ln(key, v);
                }
            }
            if e.count != 0 {
                o.int_entry_ln(b"Count", i64::from(e.count));
            }
            if let Some(a) = &e.attr {
                self.pdf_print_toks_ln(a);
                self.ol_mut(k).attr = None;
            }
            self.pdf_end_dict();
        }
        Ok(outlines)
    }
}
