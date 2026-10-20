//! A page written apart and placed later (exact mode): `pdf_ship_out`
//! (pdfTeX §750–§771) split in two. [`PdfState::ship_recorded`] ships
//! the page on any state (the page's own writer, in parallel with the
//! other pages') and keeps what the walk of its list made: the content
//! stream object's bytes after its header (compressed, with no object
//! number in it), the fonts it set, the characters it used, the writer's
//! page-local variables before and after. [`PdfState::replay_page`]
//! then does on the document's state, in page order, everything of
//! `pdf_ship_out` but the walk: the objects numbered as pdfTeX numbers
//! them (resources, page, contents, the fonts first used, the pages
//! node), the recorded content bytes copied, the page and resources
//! dictionaries written. A replay is taken only where it gives the bytes
//! the walk would give: the variables the walk reads equal, the fonts
//! shared as the walk shared them, and nothing but text and rules on
//! the page (no objects, forms, images, annotations, links,
//! destinations or beads, no error); otherwise the caller ships the
//! page (the same file either way).

use std::collections::BTreeSet;

use phitex_typeset::fonts::FontId;
use phitex_typeset::node::BoxNode;

use crate::{FontSource, FontType, OBJ_TYPE_OTHERS, OBJ_TYPE_PAGE, PdfError, PdfState};

/// An event of the walk that touches the document's tables, in order.
#[derive(Clone, Debug)]
pub(crate) enum FontEv {
    /// `load_font` made the font's state.
    Load(FontId),
    /// `do_vf` on a new font.
    DoVf(FontId),
    /// `init_font` on an unused font.
    Init(FontId),
    /// `pdf_create_obj(t, i)`: the number it gave.
    New(usize, i64, usize),
    /// `get_obj(t, id)`: the number it gave.
    Get(usize, crate::Ident, usize),
    /// An annotation's or link's object (a number TeX gave) found
    /// scheduled or not.
    Sched(usize, bool),
    /// The object an annotation or link got, scheduled.
    Scheduled(usize),
}

/// The writer's variables a page's walk reads or leaves: everything but
/// the object table, the bytes and the fonts.
#[derive(Clone, Debug)]
#[allow(clippy::struct_excessive_bools)]
struct Scratch {
    p: crate::PdfParams,
    dd: i32,
    min_bp_val: i32,
    initialized: bool,
    version_written: bool,
    fixed: [i32; 6],
    os_enable: bool,
    text: [i32; 14],
    flags: [bool; 6],
    cur_form: usize,
    xform: (i32, i32),
    page_group_val: usize,
    link_stack: Vec<crate::marks::LinkLevel>,
    thread: crate::marks::ThreadState,
    dummy_font: FontId,
    stacks: crate::color::Stacks,
    vf_cur_s: i32,
    last_byte: u8,
    cur_s: i32,
}

impl PdfState {
    fn scratch(&self) -> Scratch {
        Scratch {
            p: self.p.clone(),
            dd: self.dd,
            min_bp_val: self.min_bp_val,
            initialized: self.initialized,
            version_written: self.version_written,
            fixed: [
                self.fixed_major,
                self.fixed_minor,
                self.fixed_ocl,
                self.fixed_draftmode,
                self.fixed_image_hicolor,
                self.fixed_inclusion_copy_font,
            ],
            os_enable: self.os.enable,
            text: [
                self.cur_h,
                self.cur_v,
                i32::try_from(self.pdf_f).unwrap_or(-1),
                self.pdf_h,
                self.pdf_v,
                self.tj_start_h,
                self.delta_h,
                self.origin_h,
                self.origin_v,
                i32::try_from(self.last_f).unwrap_or(-1),
                self.last_fs,
                self.tm_a,
                self.cur_page_width,
                self.cur_page_height,
            ],
            flags: [
                self.doing_string,
                self.doing_text,
                self.doing_leaders,
                self.shipping_page,
                self.faked_space,
                self.running_link,
            ],
            cur_form: self.cur_form,
            xform: (self.xform_height, self.xform_depth),
            page_group_val: self.page_group_val,
            link_stack: self.link_stack.clone(),
            thread: self.thread.clone(),
            dummy_font: self.dummy_font,
            stacks: self.stacks.clone(),
            vf_cur_s: self.vf_cur_s,
            last_byte: self.last_byte,
            cur_s: self.cur_s,
        }
    }

    /// A digest of what a page written apart on this state depends on
    /// besides the page ([`PdfState::ship_recorded`]): the variables
    /// `pdf_ship_out` reads before it sets them and the fonts in use.
    /// Two states with equal digests record the same page alike, up to
    /// object numbers, which [`PdfState::replay_page`] maps and checks.
    #[must_use]
    pub fn page_base_digest(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.pre_key().hash(&mut h);
        self.fonts_used().hash(&mut h);
        h.finish()
    }

    /// What of the writer's variables `pdf_ship_out` reads before it sets
    /// them (the text state is set at the page's start and by
    /// `pdf_begin_text`), formatted.
    fn pre_key(&self) -> String {
        let mut thread = self.thread.clone();
        thread.last = None;
        // (`pdfshipoutbegin` and `colorstackused` as the page starts)
        let mut stacks = self.stacks.clone();
        stacks.ship_begin(true);
        stacks.used();
        format!(
            "{:?}",
            (
                (self.dd, self.min_bp_val),
                (self.initialized, self.version_written),
                [
                    self.fixed_major,
                    self.fixed_minor,
                    self.fixed_ocl,
                    self.fixed_draftmode,
                    self.fixed_image_hicolor,
                    self.fixed_inclusion_copy_font,
                ],
                (self.os.enable, self.faked_space, self.running_link),
                &self.link_stack,
                &thread,
                self.dummy_font,
                &stacks,
                (self.vf_cur_s, self.cur_s, self.cur_form),
            )
        )
    }

    fn set_scratch(&mut self, s: &Scratch) {
        self.p = s.p.clone();
        self.dd = s.dd;
        self.min_bp_val = s.min_bp_val;
        self.initialized = s.initialized;
        self.version_written = s.version_written;
        [
            self.fixed_major,
            self.fixed_minor,
            self.fixed_ocl,
            self.fixed_draftmode,
            self.fixed_image_hicolor,
            self.fixed_inclusion_copy_font,
        ] = s.fixed;
        self.os.enable = s.os_enable;
        let t = s.text;
        self.cur_h = t[0];
        self.cur_v = t[1];
        self.pdf_f = FontId::try_from(t[2]).unwrap_or(0);
        self.pdf_h = t[3];
        self.pdf_v = t[4];
        self.tj_start_h = t[5];
        self.delta_h = t[6];
        self.origin_h = t[7];
        self.origin_v = t[8];
        self.last_f = FontId::try_from(t[9]).unwrap_or(0);
        self.last_fs = t[10];
        self.tm_a = t[11];
        self.cur_page_width = t[12];
        self.cur_page_height = t[13];
        [
            self.doing_string,
            self.doing_text,
            self.doing_leaders,
            self.shipping_page,
            self.faked_space,
            self.running_link,
        ] = s.flags;
        self.cur_form = s.cur_form;
        (self.xform_height, self.xform_depth) = s.xform;
        self.page_group_val = s.page_group_val;
        self.link_stack.clone_from(&s.link_stack);
        self.thread = s.thread.clone();
        self.dummy_font = s.dummy_font;
        self.stacks = s.stacks.clone();
        self.vf_cur_s = s.vf_cur_s;
        self.last_byte = s.last_byte;
        self.cur_s = s.cur_s;
    }
}

/// What [`PdfState::ship_recorded`] keeps of a page.
#[derive(Clone, Debug, Default)]
pub struct PageRecord {
    /// Whether the page can be replayed at all.
    simple: bool,
    /// The variables before the walk (after `init_output`), formatted,
    /// the parameters apart; and the same after the page.
    pre: String,
    pre_p: Option<crate::PdfParams>,
    post_key: String,
    /// ... and after the page.
    post: Option<Box<Scratch>>,
    /// The content stream object after its header line and `<<`.
    blob: Vec<u8>,
    events: Vec<FontEv>,
    /// The fonts the page's resources list.
    font_list: Vec<FontId>,
    /// The characters the page used per font, and the fonts it
    /// initialized with their sharing key.
    chars: Vec<(FontId, [bool; 256])>,
    /// The font numbers in the content (`/F<n> ... Tf`): a symbolic
    /// handle each, the page's font `n` names (its `ff` here), at a
    /// place and length in the uncompressed content; the link rewrites
    /// them as it numbers the fonts, exactly like object numbers.
    holes: Vec<(usize, usize, FontId)>,
    /// The uncompressed content, width and height (`last_page_shown`).
    shown: (std::sync::Arc<[u8]>, i32, i32),
    /// The page's resources, page and contents objects where recorded.
    pre_nums: [usize; 3],
    /// The objects of the state recorded on (numbers up to it shared).
    shared: usize,
    /// The marks lists and their data, numbered where recorded.
    annots: Vec<usize>,
    links: Vec<usize>,
    dests: Vec<usize>,
    marks: Vec<(usize, crate::marks::Mark)>,
    dest_data: Vec<(usize, crate::marks::Dest)>,
    obj_list: Vec<usize>,
    ximage_list: Vec<usize>,
}

impl PageRecord {
    /// The fonts the page's walk read in (`do_vf` on a virtual font:
    /// `(true, f)`; `load_font`: `(false, f)`), in order.
    #[must_use]
    pub fn font_loads(&self) -> Vec<(bool, FontId)> {
        self.events
            .iter()
            .filter_map(|e| match *e {
                FontEv::DoVf(f) => Some((true, f)),
                FontEv::Load(f) => Some((false, f)),
                _ => None,
            })
            .collect()
    }

    /// The variables' key after the page placed, for the next page's
    /// [`PdfState::replay_page`] (`known`): none while a link is open
    /// (its object numbers are the document's after placing).
    #[must_use]
    pub fn post_key(&self) -> Option<&str> {
        self.post
            .as_deref()
            .filter(|p| p.link_stack.is_empty())
            .map(|_| self.post_key.as_str())
    }

    /// Whether the page can be replayed.
    #[must_use]
    pub fn simple(&self) -> bool {
        self.simple
    }
}

/// The recording of a walk in progress.
#[derive(Clone, Debug, Default)]
pub(crate) struct Recording {
    pub events: Vec<FontEv>,
    /// Whether the walk of the page is being recorded (the object
    /// events count only then), and how deep in a recorded call.
    pub walk: bool,
    pub depth: u32,
    /// The page's resources, page and contents objects.
    pub pre_nums: [usize; 3],
    /// Where the content object's bytes begin and end in `out`.
    pub blob: Option<(usize, usize)>,
    /// The font numbers written in the content ([`PageRecord`]'s).
    pub holes: Vec<(usize, usize, FontId)>,
    pub font_list: Vec<FontId>,
    /// Whether the page made anything but text, rules, annotations,
    /// links and destinations.
    pub other: bool,
    /// The objects and images the page refers to (TeX's numbers).
    pub obj_list: Vec<usize>,
    pub ximage_list: Vec<usize>,
    /// The marks and destinations as the walk left them.
    pub marks: Vec<(usize, crate::marks::Mark)>,
    pub dest_data: Vec<(usize, crate::marks::Dest)>,
}

impl PdfState {
    /// Records a font event of the walk, when recording.
    pub(crate) fn rec_font(&mut self, e: FontEv) {
        if let Some(r) = &mut self.rec {
            let object = matches!(
                e,
                FontEv::New(..) | FontEv::Get(..) | FontEv::Sched(..) | FontEv::Scheduled(_)
            );
            if !object || (r.walk && r.depth == 0) {
                r.events.push(e);
            }
        }
    }

    /// Enters (`true`) or leaves a call whose objects its own replay
    /// makes again (`init_font`, `get_obj`).
    pub(crate) fn rec_depth(&mut self, enter: bool) {
        if let Some(r) = &mut self.rec {
            if enter {
                r.depth += 1;
            } else {
                r.depth -= 1;
            }
        }
    }

    /// `pdf_ship_out(page, true)` on this state, recording the page for
    /// [`PdfState::replay_page`] (the state is the page's own: it is
    /// thrown away).
    ///
    /// # Errors
    /// What the writer stops at.
    pub fn ship_recorded(
        &mut self,
        page: &BoxNode,
        fonts: &dyn FontSource,
        wh: &mut crate::WhatsitSource<'_>,
    ) -> (Result<(), PdfError>, PageRecord) {
        self.init_output();
        let pre = self.pre_key();
        let shared = self.sys_obj_ptr();
        // (a link running across the page's start: its objects are the
        // page before's, numbered where that page was placed)
        let link_at_start = !self.link_stack.is_empty();
        // (the record keeps the characters the page uses, not those the
        // state had used: the state it is written on is any with its
        // page-base digest, which counts the fonts in use, not their
        // characters, and a state another page left would add that
        // page's, or hold this page's already; nothing in the walk reads
        // them, so they are cleared for it and given back after)
        let chars_before: Vec<(FontId, [bool; 256])> = self
            .fonts
            .iter()
            .filter(|(_, s)| s.chars.iter().any(|&c| c))
            .map(|(&f, s)| (f, s.chars))
            .collect();
        for &(f, _) in &chars_before {
            self.font_mut(f).chars = [false; 256];
        }
        self.rec = Some(Box::default());
        let r = self.ship(page, true, fonts, wh);
        let page_chars: Vec<(FontId, [bool; 256])> = self
            .fonts
            .iter()
            .filter(|(_, s)| s.chars.iter().any(|&c| c))
            .map(|(&f, s)| (f, s.chars))
            .collect();
        for &(f, b) in &chars_before {
            for (c, was) in self.font_mut(f).chars.iter_mut().zip(b) {
                *c |= was;
            }
        }
        let rec = self.rec.take().unwrap_or_default();
        let pre_p = Some(self.p.clone());
        let mut pr = PageRecord {
            pre,
            pre_p,
            shared,
            pre_nums: rec.pre_nums,
            ..PageRecord::default()
        };
        // (the fonts the walk read in, in order, kept for every page:
        // the link numbers them, [`PageRecord::font_loads`])
        pr.events.clone_from(&rec.events);
        let Some((a, b)) = rec.blob else {
            return (r, pr);
        };
        // (a page inside a link running across a page break is shipped
        // where it is placed, not replayed: the link's objects carry over
        // from page to page)
        pr.simple = r.is_ok()
            && !rec.other
            && a <= b
            && b <= self.out.len()
            && !link_at_start
            && self.link_stack.is_empty();
        if pr.simple {
            pr.blob = self.out[a..b].to_vec();
            pr.post = Some(Box::new(self.scratch()));
            pr.font_list = rec.font_list;
            pr.chars = page_chars;
            pr.holes = rec.holes;
            pr.shown = (
                std::sync::Arc::from(self.page_shown.0.as_slice()),
                self.page_shown.1,
                self.page_shown.2,
            );
            pr.post_key = self.pre_key();
            pr.annots.clone_from(&self.annot_list);
            pr.links.clone_from(&self.link_list);
            pr.dests.clone_from(&self.dest_list);
            // (the marks were written and taken out: the walk's copies)
            pr.obj_list = rec.obj_list;
            pr.ximage_list = rec.ximage_list;
            pr.marks = rec.marks;
            pr.dest_data = rec.dest_data;
        }
        (r, pr)
    }

    /// The characters marked used since the last call, by font, the
    /// marks cleared: a caller that takes them after every step keeps
    /// them out of the state (they are read only by the fonts' embedding
    /// at the end, [`PdfState::add_chars`]), so that a page using one
    /// more character leaves the state after it equal.
    pub fn take_chars(&mut self) -> Vec<(FontId, [bool; 256])> {
        let marked: Vec<FontId> = self
            .fonts
            .iter()
            .filter(|(_, s)| s.chars.iter().any(|&c| c))
            .map(|(&f, _)| f)
            .collect();
        marked
            .into_iter()
            .map(|f| {
                (
                    f,
                    std::mem::replace(&mut self.font_mut(f).chars, [false; 256]),
                )
            })
            .collect()
    }

    /// The characters `cs` of font `f` marked used (the union of what
    /// [`PdfState::take_chars`] took, at the end); nothing for a font
    /// not loaded here.
    pub fn add_chars(&mut self, f: FontId, cs: &[bool; 256]) {
        if self.fonts.get(&f).is_some() && cs.iter().any(|&c| c) {
            for (a, &b) in self.font_mut(f).chars.iter_mut().zip(cs) {
                *a |= b;
            }
        }
    }

    /// The fonts used here, by id: what a later replay compares with
    /// ([`PdfState::replay_page`]'s `base`).
    #[must_use]
    pub fn fonts_used(&self) -> BTreeSet<FontId> {
        self.fonts
            .iter()
            .filter(|(_, s)| s.used)
            .map(|(&f, _)| f)
            .collect()
    }

    /// The page `rec` recorded, placed on this state as `pdf_ship_out`
    /// would ship it here: false (the state untouched but for
    /// `init_output`) where the replay would not give the walk's bytes.
    /// `fmap` gives the font here a font of the record is (the record's
    /// fonts are the page's own handles); the fonts' numbers in the
    /// content are rewritten as here, and where any differs the content
    /// goes to `parts` as a [`Part::Content`] after the bytes before it,
    /// for the caller to compress (in parallel with the other pages').
    /// `tex` gives a number the record shares with the state it was
    /// recorded on (one TeX numbered before the page) the number here.
    ///
    /// # Errors
    /// Why the page cannot be placed (the caller ships it).
    #[allow(clippy::too_many_lines)]
    pub fn replay_page(
        &mut self,
        rec: &PageRecord,
        fonts: &dyn FontSource,
        fmap: &dyn Fn(FontId) -> FontId,
        tex: &dyn Fn(usize) -> usize,
        known: Option<&str>,
        parts: &mut Vec<Part>,
    ) -> Result<Option<String>, String> {
        let Some(post) = rec.post.as_deref().filter(|_| rec.simple) else {
            return Err("not only text and rules".to_owned());
        };
        self.init_output();
        if rec.pre_p.as_ref() != Some(&self.p) {
            return Err("the parameters differ".to_owned());
        }
        let pre = match known {
            Some(k) => k.to_owned(),
            None => self.pre_key(),
        };
        if pre != rec.pre {
            let at = pre
                .bytes()
                .zip(rec.pre.bytes())
                .position(|(a, b)| a != b)
                .unwrap_or(0);
            let from = at.saturating_sub(60);
            return Err(format!(
                "the variables differ: here {:?}, recorded {:?}",
                pre.get(from..(at + 60).min(pre.len())).unwrap_or(""),
                rec.pre
                    .get(from..(at + 60).min(rec.pre.len()))
                    .unwrap_or("")
            ));
        }
        // every font the walk set up is loaded here by the time it does
        // (a state other than the recorded one's, e.g. a speculative
        // run's, may lack a font the record found loaded)
        {
            let mut loaded: BTreeSet<FontId> = self.fonts.iter().map(|(&f, _)| f).collect();
            for e in &rec.events {
                match *e {
                    FontEv::Load(f) => {
                        loaded.insert(fmap(f));
                    }
                    FontEv::DoVf(f) | FontEv::Init(f) if !loaded.contains(&fmap(f)) => {
                        return Err(format!("font {f} not loaded here"));
                    }
                    _ => {}
                }
            }
        }
        // the objects TeX numbered are as they were where recorded, and
        // no destination the page defines was defined by a page before
        for e in &rec.events {
            match e {
                FontEv::Sched(n, was) if *n <= rec.shared => {
                    let n = tex(*n);
                    if n > self.sys_obj_ptr() || self.is_scheduled(n) != *was {
                        return Err(format!("object {n} scheduled differently here"));
                    }
                }
                FontEv::Get(t, id, n) if rec.dests.contains(n) => {
                    if let Some(k) = self.index.get(&(*t, id.clone()))
                        && self.aux.dests.contains_key(k)
                    {
                        return Err(format!("destination {id:?} defined before"));
                    }
                }
                _ => {}
            }
        }
        // §752 as `ship` does it
        let last_resources = self.new_objnum_();
        self.page_group_val = 0;
        let saved = self.reset_resource_lists();
        self.cur_page_width = post.text[12];
        self.cur_page_height = post.text[13];
        self.last_page = self.get_obj(
            OBJ_TYPE_PAGE,
            &crate::Ident::Num(i32::try_from(self.total_pages + 1).unwrap_or(i32::MAX)),
        );
        self.aux.pages_made.insert(self.last_page);
        let last_stream = self.new_dict(OBJ_TYPE_OTHERS, 0, 0);
        // the recorded state's numbers here: its own objects mapped as
        // they are made again, the shared ones kept
        let mut map: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
        for (a, b) in rec
            .pre_nums
            .into_iter()
            .zip([last_resources, self.last_page, last_stream])
        {
            map.insert(a, b);
        }
        self.annot_list.clear();
        self.link_list.clear();
        self.dest_list.clear();
        self.bead_list.clear();
        self.thread.last = None;
        // the walk's fonts, in its order
        for e in &rec.events {
            match *e {
                FontEv::Load(f) => self.load_font(fmap(f), fonts),
                FontEv::DoVf(f) => {
                    let f = fmap(f);
                    if self.font(f).kind == FontType::New {
                        self.do_vf(f, fonts);
                    }
                }
                FontEv::Init(f) => {
                    let f = fmap(f);
                    if !self.font(f).used {
                        self.init_font(f, fonts);
                    }
                }
                FontEv::New(t, i, n) => {
                    let i = if t == crate::OBJ_TYPE_FONT {
                        FontId::try_from(i).map_or(i, |f| i64::from(fmap(f)))
                    } else {
                        i
                    };
                    let k = self.create_obj(t, i);
                    map.insert(n, k);
                }
                FontEv::Get(t, ref id, n) => {
                    let k = self.get_obj(t, id);
                    map.insert(n, k);
                }
                FontEv::Sched(..) => {}
                FontEv::Scheduled(n) => {
                    let k = map.get(&n).copied().unwrap_or_else(|| tex(n));
                    self.set_scheduled(k);
                }
            }
        }
        // (a number shared with the state recorded on: TeX's, as the
        // caller numbers it, `tex`)
        let at = |n: usize| map.get(&n).copied().unwrap_or_else(|| tex(n));
        for (k, m) in &rec.marks {
            self.aux.marks.insert(at(*k), m.clone());
        }
        for (k, d) in &rec.dest_data {
            self.aux.dests.insert(at(*k), Some(d.clone()));
        }
        for (f, cs) in &rec.chars {
            let f = fmap(*f);
            if let Some(s) = self.fonts.get(&f)
                && s.chars.iter().zip(cs).any(|(&a, &b)| b && !a)
            {
                let s = self.font_mut(f);
                for (a, &b) in s.chars.iter_mut().zip(cs) {
                    *a |= b;
                }
            }
        }
        // the contents: as recorded, the fonts' numbers in it as here (a
        // number the page wrote is its own handle of the font, `ff`
        // there; here `ff` of the font it is, named as the caller numbers
        // it); where one differs, the content is the caller's to
        // compress, after the bytes so far
        let subs: Vec<(usize, usize, Vec<u8>)> = rec
            .holes
            .iter()
            .filter_map(|&(a, len, k)| {
                let d = fmap(k);
                let d = if self.fonts.contains_key(&d) {
                    self.ff(d)
                } else {
                    d
                };
                let digits = Self::font_tag(d, fonts).to_string().into_bytes();
                (rec.shown.0.get(a..a + len) != Some(&digits[..])).then_some((a, len, digits))
            })
            .collect();
        let report = (!subs.is_empty()).then(|| format!("{} font numbers rewritten", subs.len()));
        if subs.is_empty() {
            self.print(&rec.blob);
        } else {
            if let Some(c) = self.flush_chunk() {
                parts.push(Part::Bytes(c));
            }
            self.flushes += 1;
            parts.push(Part::Content {
                data: rec.shown.0.clone(),
                subs,
                level: self.p.compress_level,
            });
        }
        self.total_pages += 1;
        self.set_scratch(post);
        for l in &mut self.link_stack {
            l.objnum = at(l.objnum);
        }
        self.annot_list = rec.annots.iter().map(|&k| at(k)).collect();
        self.link_list = rec.links.iter().map(|&k| at(k)).collect();
        self.dest_list = rec.dests.iter().map(|&k| at(k)).collect();
        self.page_shown = (Vec::new(), rec.shown.1, rec.shown.2);
        self.write_page_object(last_stream, last_resources);
        // "Write out resource lists": what no page before wrote
        self.obj_list = rec.obj_list.iter().map(|&k| at(k)).collect();
        self.ximage_list = rec.ximage_list.iter().map(|&k| at(k)).collect();
        for k in self.obj_list.clone() {
            if !self.is_written(k) {
                self.pdf_write_obj(k);
            }
        }
        for k in self.ximage_list.clone() {
            if !self.is_written(k)
                && let Err(e) = self.pdf_write_image(k, fonts)
            {
                // (the step's write logged it)
                let _ = e;
            }
        }
        self.font_list.clone_from(&rec.font_list);
        self.write_page_marks();
        self.font_list = self.font_list.iter().map(|&f| fmap(f)).collect();
        self.write_resources(last_resources, true, fonts);
        self.restore_resource_lists(saved);
        let links = std::mem::take(&mut self.link_stack);
        self.set_scratch(post);
        self.link_stack = links;
        (self.pdf_f, self.last_f, self.dummy_font) =
            (fmap(self.pdf_f), fmap(self.last_f), fmap(self.dummy_font));
        Ok(report)
    }
}

/// Bytes the link places after the state's own (a page's content
/// written apart, its font numbers rewritten and compressed by the
/// caller, in parallel with the other pages': [`PdfState::replay_page`]).
#[derive(Clone, Debug)]
pub enum Part {
    /// A chunk as the state flushed it.
    Bytes(crate::Chunk),
    /// A content stream's dictionary from its `/Length` on and its
    /// stream: `data` uncompressed with `subs` (place, length, bytes)
    /// put in, compressed at `level` (0: as it is).
    Content {
        data: std::sync::Arc<[u8]>,
        subs: Vec<(usize, usize, Vec<u8>)>,
        level: i32,
    },
}

impl Part {
    /// The chunk: as it is, or the content's bytes (no object begins in
    /// them: the stream's header is the chunk's before).
    #[must_use]
    pub fn resolve(self) -> crate::Chunk {
        match self {
            Part::Bytes(c) => c,
            Part::Content { data, subs, level } => {
                let mut d = Vec::with_capacity(data.len() + 8 * subs.len());
                let mut i = 0;
                for (a, len, b) in &subs {
                    d.extend_from_slice(&data[i..*a]);
                    d.extend_from_slice(b);
                    i = a + len;
                }
                d.extend_from_slice(&data[i..]);
                let (body, filter) = if level > 0 {
                    (crate::compress_memo(&d, level), true)
                } else {
                    (d, false)
                };
                let len = body.len().to_string();
                let mut out = b"/Length           \n".to_vec();
                out[8..8 + len.len()].copy_from_slice(len.as_bytes());
                if filter {
                    out.extend_from_slice(b"/Filter /FlateDecode\n");
                }
                out.extend_from_slice(b">>\nstream\n");
                out.extend_from_slice(&body);
                out.extend_from_slice(b"\nendstream\nendobj\n");
                crate::Chunk {
                    bytes: out.into(),
                    objs: Vec::new(),
                }
            }
        }
    }
}
