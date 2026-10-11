//! The fold's marks (pdftex.web part 32f's "Write out pending PDF marks",
//! `write_action`, and the end's outlines, name tree and threads).

use super::{Data, F, Id, OBJ_TYPE_DEST, OBJ_TYPE_OTHERS, OBJ_TYPE_OUTLINE, OBJ_TYPE_PAGE, OBJ_TYPE_STRUCT_DEST, OBJ_TYPE_THREAD, Out, PageBody};
use super::{NAME_TREE_KIDS_MAX, Outline};
use crate::pdf::content::Rect;
use crate::pdf::what::{self, Action};

/// `str_less_str`: PDF strings compared as their characters, escapes
/// read.
fn str_less_str(a: &[u8], b: &[u8]) -> bool {
    fn next(s: &[u8], j: &mut usize) -> u8 {
        let e = s.len();
        let mut c = s[*j];
        *j += 1;
        if c == 92 && *j < e {
            c = s[*j];
            *j += 1;
            if (48..=55).contains(&c) {
                c -= 48;
                if *j < e && (48..=55).contains(&s[*j]) {
                    c = 8u8.wrapping_mul(c).wrapping_add(s[*j] - 48);
                    *j += 1;
                    if *j < e && (48..=55).contains(&s[*j]) && c < 32 {
                        c = 8 * c + s[*j] - 48;
                        *j += 1;
                    }
                }
            } else {
                c = match c {
                    98 => 8,
                    102 => 12,
                    110 => 10,
                    114 => 13,
                    116 => 9,
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

/// `sort_dest_names`: pdfTeX's quicksort (its order of equal names).
fn sort_dest_names(v: &mut [(crate::pdf::what::Text, u32)], l: isize, r: isize) {
    let (mut i, mut j) = (l, r);
    let s = v[isize::midpoint(l, r) as usize].0.clone();
    loop {
        while str_less_str(&v[i as usize].0, &s) {
            i += 1;
        }
        while str_less_str(&s, &v[j as usize].0) {
            j -= 1;
        }
        if i <= j {
            v.swap(i as usize, j as usize);
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

impl F<'_> {
    /// `write_action`.
    pub(super) fn write_action(&mut self, a: &Action) {
        let mut o = Out::new();
        if a.kind == what::ACTION_USER {
            if !a.text.is_empty() {
                o.print_ln(&a.text);
            }
            self.print_out(&o);
            return;
        }
        o.print(b"<< ");
        if let Some(f) = &a.file {
            o.print(b"/F ");
            if f.first() == Some(&b'(') && f.last() == Some(&b')') {
                o.print(f);
            } else {
                o.str(f);
            }
            o.print(b" ");
            if a.new_window > 0 {
                o.print(b"/NewWindow ");
                o.print(if a.new_window == 1 { b"true " } else { b"false " });
            }
        }
        let named = matches!(a.id, Id::Name(_));
        match a.kind {
            what::ACTION_PAGE => {
                let Id::Num(n) = a.id else { return };
                if a.file.is_none() {
                    o.print(b"/S /GoTo /D [");
                    let k = self.get_obj(OBJ_TYPE_PAGE, &Id::Num(n));
                    o.int(i64::from(k));
                    o.print(b" 0 R");
                } else {
                    o.print(b"/S /GoToR /D [");
                    o.int(i64::from(n) - 1);
                }
                o.out(b' ');
                o.print(&a.text);
                o.out(b']');
            }
            what::ACTION_GOTO => {
                let mut d = 0;
                if a.file.is_none() {
                    o.print(b"/S /GoTo ");
                    d = self.get_obj(OBJ_TYPE_DEST, &a.id);
                } else {
                    o.print(b"/S /GoToR ");
                }
                match &a.id {
                    Id::Name(s) => {
                        o.print(b"/D ");
                        o.str(s);
                    }
                    Id::Num(_) if a.file.is_none() => o.indirect(b"D", d),
                    Id::Num(_) => {
                        self.errors.push("pdfTeX error (ext4): `goto' option cannot be used with both `file' and `num'".into());
                    }
                }
            }
            _ => {
                o.print(b"/S /Thread ");
                let mut d = 0;
                if a.file.is_none() {
                    d = self.get_obj(OBJ_TYPE_THREAD, &a.id);
                }
                match &a.id {
                    Id::Name(s) => {
                        o.print(b"/D ");
                        o.str(s);
                    }
                    Id::Num(_) if a.file.is_none() => o.indirect(b"D", d),
                    Id::Num(n) => {
                        o.print(b"/D ");
                        o.int(i64::from(*n));
                    }
                }
            }
        }
        if let Some(sd) = &a.struct_id {
            o.print(b" ");
            if a.file.is_none() {
                let k = self.get_obj(OBJ_TYPE_STRUCT_DEST, sd);
                o.indirect(b"SD", k);
            } else {
                o.print(b"/SD ");
                if let Id::Name(s) = sd {
                    o.print(s);
                }
            }
        }
        let _ = named;
        o.print_ln(b" >>");
        self.print_out(&o);
    }

    /// `pdf_rectangle`'s numbers, or `pdf_print_rect_spec`'s (`mag`, the
    /// page's `\mag`): left, bottom, right, top, in the page's PDF
    /// coordinates.
    fn rect_spec(&self, o: &mut Out, r: &Rect, mag: i32, dd: i32) {
        let h = self.cur_page_height;
        o.mag_bp(r[0], dd, mag);
        o.out(b' ');
        o.mag_bp(h - r[3], dd, mag);
        o.out(b' ');
        o.mag_bp(r[2], dd, mag);
        o.out(b' ');
        o.mag_bp(h - r[1], dd, mag);
    }

    /// "Write out pending PDF marks" of the page shipped.
    pub(super) fn write_marks(&mut self, body: &PageBody, mag: i32, dd: i32) {
        let marks = std::mem::take(&mut self.marks);
        for (k, r, data) in &marks.annots {
            self.begin_dict(*k, 1);
            let mut o = Out::new();
            o.print_ln(b"/Type /Annot");
            if !data.is_empty() {
                o.print_ln(data);
            }
            o.print(b"/Rect [");
            self.rect_spec(&mut o, r, mag, dd);
            o.print_ln(b"]");
            self.print_out(&o);
            self.end_dict();
        }
        for &(k, l) in &marks.links {
            let Some(link) = body.links.get(l) else { continue };
            self.begin_dict(k, 1);
            let mut o = Out::new();
            o.print_ln(b"/Type /Annot");
            let user = link.action.kind == what::ACTION_USER;
            if !user {
                o.print_ln(b"/Subtype /Link");
            }
            if let Some(a) = link.attr.as_ref().filter(|a| !a.is_empty()) {
                o.print_ln(a);
            }
            o.print(b"/Rect [");
            self.rect_spec(&mut o, &link.rect, mag, dd);
            o.print_ln(b"]");
            if !user {
                o.print(b"/A ");
            }
            self.print_out(&o);
            self.write_action(&link.action.clone());
            self.end_dict();
        }
        for (k, r) in &marks.dests {
            let k = *k;
            if self.is_written(k) {
                self.errors.push("pdfTeX error (ext5): destination has been already written (this shouldn't happen)".into());
                continue;
            }
            let Some(Data::Dest { kind, zoom, struct_num, named }) = self.data.get(&k).cloned() else { continue };
            let dict = named && struct_num.is_none();
            if dict {
                self.begin_dict(k, 1);
                self.puts(b"/D ");
            } else {
                self.begin_obj(k, 1);
            }
            let mut o = Out::new();
            o.out(b'[');
            o.int(i64::from(struct_num.unwrap_or(self.last_page as i32)));
            o.print(b" 0 R ");
            let h = self.cur_page_height;
            match kind {
                what::DEST_XYZ => {
                    o.print(b"/XYZ ");
                    o.mag_bp(r[0], dd, mag);
                    o.out(b' ');
                    o.mag_bp(h - r[1], dd, mag);
                    o.out(b' ');
                    match zoom {
                        None => o.print(b"null"),
                        Some(z) => {
                            o.int(i64::from(z / 1000));
                            o.out(b'.');
                            o.int(i64::from(z % 1000));
                        }
                    }
                }
                what::DEST_FIT => o.print(b"/Fit"),
                what::DEST_FITH => {
                    o.print(b"/FitH ");
                    o.mag_bp(h - r[1], dd, mag);
                }
                what::DEST_FITV => {
                    o.print(b"/FitV ");
                    o.mag_bp(r[0], dd, mag);
                }
                what::DEST_FITB => o.print(b"/FitB"),
                what::DEST_FITBH => {
                    o.print(b"/FitBH ");
                    o.mag_bp(h - r[1], dd, mag);
                }
                what::DEST_FITBV => {
                    o.print(b"/FitBV ");
                    o.mag_bp(r[0], dd, mag);
                }
                _ => {
                    o.print(b"/FitR ");
                    self.rect_spec(&mut o, r, mag, dd);
                }
            }
            o.print_ln(b"]");
            self.print_out(&o);
            if dict {
                self.end_dict();
            } else {
                self.end_obj();
            }
        }
        for &(b, i) in &marks.beads {
            let Some(r) = body.beads.get(i) else { continue };
            let n = self.new_obj(OBJ_TYPE_OTHERS, 1);
            let mut o = Out::new();
            o.out(b'[');
            self.rect_spec(&mut o, r, mag, dd);
            o.print_ln(b"]");
            self.print_out(&o);
            self.bead_mut(b).rect = n;
            self.end_obj();
        }
    }

    /// "Check for non-existing destinations" (`pdf_fix_dest`), and the
    /// structure destinations' warnings.
    pub(super) fn fix_dests(&mut self) {
        let mut k = self.head_tab[usize::from(OBJ_TYPE_DEST)];
        while k != 0 {
            if self.objs[k as usize].aux == 0 {
                self.warnings.push(format!(
                    "pdfTeX warning (dest): {} has been referenced but does not exist, replaced by a fixed one",
                    self.obj_id(OBJ_TYPE_DEST, k)
                ));
                self.begin_obj(k, 1);
                let mut o = Out::new();
                o.out(b'[');
                o.int(i64::from(self.head_tab[usize::from(OBJ_TYPE_PAGE)]));
                o.print_ln(b" 0 R /Fit]");
                self.print_out(&o);
                self.end_obj();
            }
            k = self.objs[k as usize].link;
        }
        let mut k = self.head_tab[usize::from(OBJ_TYPE_STRUCT_DEST)];
        while k != 0 {
            if self.objs[k as usize].aux == 0 {
                self.warnings.push(format!(
                    "pdfTeX warning (structure dest): {} has been referenced but does not exist",
                    self.obj_id(OBJ_TYPE_STRUCT_DEST, k)
                ));
            }
            k = self.objs[k as usize].link;
        }
    }

    /// An object's identifier as pdfTeX prints it (`name{…}`, `num…`).
    fn obj_id(&self, t: u8, k: u32) -> String {
        match self.ids.iter().find(|(key, v)| key.0 == t && **v == k).map(|(key, _)| &key.1) {
            Some(Id::Name(s)) => format!("name{{{}}}", String::from_utf8_lossy(s)),
            Some(Id::Num(n)) => format!("num{n}"),
            None => format!("num{}", self.objs[k as usize].info),
        }
    }

    /// `open_subentries`.
    fn open_subentries(&mut self, p: u32) -> i32 {
        let mut k = 0;
        let first = self.ol(p).first;
        if first != 0 {
            let mut l = first;
            loop {
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
                if l == 0 {
                    break;
                }
            }
        }
        let o = self.ol_mut(p);
        o.count = if o.count > 0 { k } else { -k };
        k
    }

    /// "Output outlines": the outline root, then each entry; its number.
    pub(super) fn output_outlines(&mut self) -> u32 {
        if self.first_outline == 0 {
            return 0;
        }
        let outlines = self.create_obj(OBJ_TYPE_OTHERS, 0);
        self.begin_dict(outlines, 1);
        let mut l = self.first_outline;
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
        let mut o = Out::new();
        o.print_ln(b"/Type /Outlines");
        o.indirect_ln(b"First", self.first_outline);
        o.indirect_ln(b"Last", self.last_outline);
        o.int_entry_ln(b"Count", i64::from(k));
        self.print_out(&o);
        self.end_dict();
        // ("Output PDF outline entries")
        let mut k = self.head_tab[usize::from(OBJ_TYPE_OUTLINE)];
        while k != 0 {
            let e: Outline = self.ol(k);
            if e.parent == self.parent_outline {
                if e.prev == 0 {
                    self.first_outline = k;
                }
                if e.next == 0 {
                    self.last_outline = k;
                }
            }
            self.begin_dict(k, 1);
            let mut o = Out::new();
            o.indirect_ln(b"Title", e.title);
            o.indirect_ln(b"A", e.action);
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
            if let Some(a) = e.attr.as_ref().filter(|a| !a.is_empty()) {
                o.print_ln(a);
            }
            self.print_out(&o);
            self.end_dict();
            k = self.objs[k as usize].link;
        }
        outlines
    }

    /// "Output name tree"; the `/Names` dictionary's number (0: none).
    pub(super) fn output_name_tree(&mut self) -> u32 {
        let mut dests = 0;
        let n = self.dest_names.len();
        if n > 0 {
            let mut names = std::mem::take(&mut self.dest_names);
            sort_dest_names(&mut names, 0, n as isize - 1);
            // (a node's first and last names, and the leaves' objects)
            let mut first: std::collections::HashMap<u32, crate::pdf::what::Text> = std::collections::HashMap::new();
            let mut last: std::collections::HashMap<u32, crate::pdf::what::Text> = std::collections::HashMap::new();
            let mut names_head = 0u32;
            let mut names_tail = 0u32;
            let mut k_leaf = 0usize;
            let mut k_node = 0u32;
            let mut is_names = true;
            let mut b = 0u32;
            loop {
                let mut l;
                loop {
                    l = self.create_obj(OBJ_TYPE_OTHERS, 0);
                    if b == 0 {
                        b = l;
                    }
                    if names_head == 0 {
                        names_head = l;
                        names_tail = l;
                    } else {
                        self.objs[names_tail as usize].link = l;
                        names_tail = l;
                    }
                    self.objs[names_tail as usize].link = 0;
                    // ("Output the current node in this level")
                    self.begin_dict(l, 1);
                    let mut o = Out::new();
                    let mut j = 0;
                    if is_names {
                        first.insert(l, names[k_leaf].0.clone());
                        o.print(b"/Names [");
                        loop {
                            o.str(&names[k_leaf].0);
                            o.out(b' ');
                            o.int(i64::from(names[k_leaf].1));
                            o.print(b" 0 R ");
                            j += 1;
                            k_leaf += 1;
                            if j == NAME_TREE_KIDS_MAX || k_leaf == n {
                                break;
                            }
                        }
                        o.remove_last_space();
                        o.print_ln(b"]");
                        last.insert(l, names[k_leaf - 1].0.clone());
                        if k_leaf == n {
                            is_names = false;
                            k_node = names_head;
                            b = 0;
                        }
                    } else {
                        let f = first.get(&k_node).cloned().unwrap_or_else(|| std::sync::Arc::from(&b""[..]));
                        first.insert(l, f);
                        o.print(b"/Kids [");
                        loop {
                            o.int(i64::from(k_node));
                            o.print(b" 0 R ");
                            j += 1;
                            let la = last.get(&k_node).cloned().unwrap_or_else(|| std::sync::Arc::from(&b""[..]));
                            last.insert(l, la);
                            k_node = self.objs[k_node as usize].link;
                            if j == NAME_TREE_KIDS_MAX || k_node == b || self.objs[k_node as usize].link == 0 {
                                break;
                            }
                        }
                        o.remove_last_space();
                        o.print_ln(b"]");
                        if k_node == b {
                            b = 0;
                        }
                    }
                    o.print(b"/Limits [");
                    o.str(&first[&l]);
                    o.out(b' ');
                    o.str(&last[&l]);
                    o.print_ln(b"]");
                    self.print_out(&o);
                    self.end_dict();
                    if b == 0 {
                        break;
                    }
                }
                if k_node == l {
                    dests = l;
                    break;
                }
            }
            self.dest_names = names;
        }
        if dests == 0 && self.names.is_none() {
            return 0;
        }
        let t = self.new_dict(OBJ_TYPE_OTHERS, 0, 1);
        let mut o = Out::new();
        if dests != 0 {
            o.indirect_ln(b"Dests", dests);
        }
        if let Some(s) = self.names.take().filter(|s| !s.is_empty()) {
            o.print_ln(&s);
        }
        self.print_out(&o);
        self.end_dict();
        t
    }

    /// "Output article threads"; the threads array's number (0: none).
    pub(super) fn output_threads(&mut self) -> u32 {
        let head = self.head_tab[usize::from(OBJ_TYPE_THREAD)];
        if head == 0 {
            return 0;
        }
        let threads = self.new_obj(OBJ_TYPE_OTHERS, 1);
        let mut o = Out::new();
        o.out(b'[');
        let mut k = head;
        while k != 0 {
            o.int(i64::from(k));
            o.print(b" 0 R ");
            k = self.objs[k as usize].link;
        }
        o.remove_last_space();
        o.print_ln(b"]");
        self.print_out(&o);
        self.end_obj();
        let mut k = head;
        while k != 0 {
            self.out_thread(k);
            k = self.objs[k as usize].link;
        }
        threads
    }

    /// `thread_title`.
    fn thread_title(&self, o: &mut Out, t: u32) {
        o.print(b"/Title (");
        match self.ids.iter().find(|(key, v)| key.0 == OBJ_TYPE_THREAD && **v == t).map(|(key, _)| &key.1) {
            Some(Id::Name(s)) => o.print(s),
            _ => o.int(self.objs[t as usize].info),
        }
        o.print_ln(b")");
    }

    /// `out_thread` (`pdf_fix_thread` for one without beads).
    fn out_thread(&mut self, t: u32) {
        let first = self.objs[t as usize].aux as u32;
        let dd = self.fixed.map_or(3, |f| f.decimal_digits);
        if first == 0 {
            self.warnings.push(format!(
                "pdfTeX warning (thread): destination {} has been referenced but does not exist, replaced by a fixed one",
                self.obj_id(OBJ_TYPE_THREAD, t)
            ));
            let a = self.new_dict(OBJ_TYPE_OTHERS, 0, 0);
            let mut o = Out::new();
            o.indirect_ln(b"T", t);
            o.indirect_ln(b"V", a);
            o.indirect_ln(b"N", a);
            o.indirect_ln(b"P", self.head_tab[usize::from(OBJ_TYPE_PAGE)]);
            o.print(b"/R [0 0 ");
            o.bp(self.end_page_dims.0, dd);
            o.out(b' ');
            o.bp(self.end_page_dims.1, dd);
            o.print_ln(b"]");
            self.print_out(&o);
            self.end_dict();
            self.begin_dict(t, 1);
            let mut o = Out::new();
            o.print_ln(b"/I << ");
            self.thread_title(&mut o, t);
            o.print_ln(b">>");
            o.indirect_ln(b"F", a);
            self.print_out(&o);
            self.end_dict();
            return;
        }
        self.begin_dict(t, 1);
        let mut a = first;
        let b = a;
        let mut last_attr = None;
        loop {
            if let Some(x) = self.bead(a).attr {
                last_attr = Some(x);
            }
            a = self.bead(a).next;
            if a == b {
                break;
            }
        }
        let mut o = Out::new();
        if let Some(x) = last_attr {
            o.print_ln(&x);
        } else {
            o.print_ln(b"/I << ");
            self.thread_title(&mut o, t);
            o.print_ln(b">>");
        }
        o.indirect_ln(b"F", a);
        self.print_out(&o);
        self.end_dict();
        loop {
            self.begin_dict(a, 1);
            let e = self.bead(a);
            let mut o = Out::new();
            if a == b {
                o.indirect_ln(b"T", t);
            }
            o.indirect_ln(b"V", e.prev);
            o.indirect_ln(b"N", e.next);
            o.indirect_ln(b"P", e.page);
            o.indirect_ln(b"R", e.rect);
            self.print_out(&o);
            self.end_dict();
            a = e.next;
            if a == b {
                break;
            }
        }
    }
}
