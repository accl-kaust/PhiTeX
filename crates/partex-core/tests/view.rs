//! The build printed as a program (`partex_core::ssa::view`, DESIGN 4.3
//! item 7): a small document built in SSA mode, its view, and the view
//! again after a word is edited and the build rebuilt.
//!
//! In a process of its own: an SSA build sets switches the whole process
//! shares (the boxes' versions), which the unit tests of machine mode,
//! run in parallel, must not see change under them.

use std::collections::BTreeMap;
use std::sync::Arc;

use partex_core::diag::Diagnostic;
use partex_core::host::{DateTime, FileKind, Host, OpenedFile, WriteId};
use partex_core::ssa::{Recorder, SsaTracker, rebuild, run, step_trace, view};
use partex_core::{Params, Tex};
use partex_ssa::Trace;
use phitex_ir::{Def, Operand, Program};

/// Files in memory, the outputs kept.
#[derive(Default)]
struct Disk {
    files: BTreeMap<Vec<u8>, Vec<u8>>,
    written: BTreeMap<u32, Vec<u8>>,
    next: u32,
}

impl Host for Disk {
    fn read_file(&mut self, name: &[u8], kind: FileKind) -> Option<OpenedFile> {
        let mut with = name.to_vec();
        with.extend_from_slice(match kind {
            FileKind::Tex => b".tex",
            FileKind::Tfm => b".tfm",
            _ => b"",
        });
        [with, name.to_vec()].into_iter().find_map(|n| {
            let c = self.files.get(&n)?;
            Some(OpenedFile {
                name: [b"./", &n[..]].concat(),
                contents: Arc::from(&c[..]),
            })
        })
    }
    fn open_write(&mut self, name: &[u8], _: FileKind) -> Option<(WriteId, Vec<u8>)> {
        self.next += 1;
        self.written.insert(self.next, Vec::new());
        Some((WriteId(self.next), name.to_vec()))
    }
    fn write(&mut self, file: WriteId, bytes: &[u8]) {
        self.written
            .entry(file.0)
            .or_default()
            .extend_from_slice(bytes);
    }
    fn close(&mut self, _: WriteId) {}
    fn term_write(&mut self, _: &[u8]) {}
    fn term_read_line(&mut self) -> Option<Vec<u8>> {
        None
    }
    fn diagnostic(&mut self, _: &Diagnostic) {}
    fn now(&self) -> DateTime {
        DateTime {
            year: 1776,
            month: 7,
            day: 4,
            minutes: 720,
        }
    }
}

/// A small document for INITEX with one font: definitions, a register
/// in a group and out of it, two paragraphs, a page.
const DOC: &str = r"\catcode`\{=1 \catcode`\}=2 \catcode`\#=6
\font\rm=cmr10 \rm \hsize=100pt \vsize=100pt
\def\greet#1{Hello #1.}
\count1=5

\greet{World} One line.

{\count1=6 Second paragraph.}

\end
";

/// `DOC` built in SSA mode, in batch mode.
fn build() -> Tex<Disk, SsaTracker> {
    let mut host = Disk::default();
    host.files.insert(
        b"cmr10.tfm".to_vec(),
        include_bytes!("../testdata/cmr10.tfm").to_vec(),
    );
    host.files
        .insert(b"doc.tex".to_vec(), DOC.as_bytes().to_vec());
    let params = Params {
        ini: true,
        interaction: Some(0), // (batch mode, §73)
        ..Params::default()
    };
    // (the full recorder: the step trace prints a step's own reads, which
    // lean records do not keep, TODO 8)
    let mut tracker = SsaTracker::new(Recorder::new());
    tracker.set_lean(false);
    let mut tex = Tex::new(host, tracker, params);
    let rep = run(&mut tex, b"doc", false, 0);
    assert!(rep.history <= 1, "the build failed: {}", rep.history);
    tex
}

/// A view checked, and round-tripped through its text.
fn checked(p: &Program) {
    p.check().unwrap();
    let q = Program::parse(&p.to_text()).unwrap();
    assert_eq!(q.values, p.values);
}

/// The pool's strings without their numbers (the build hands them out,
/// DESIGN 3.2: a rebuild takes others).
fn unnumbered(n: &str) -> String {
    if n.starts_with("string:") {
        String::from("string:N")
    } else {
        n.to_string()
    }
}

/// `p` with only the imports from windows (the edges between them), the
/// strings unnumbered.
fn edges(p: &Program) -> String {
    let mut q = p.clone();
    for v in &mut q.values {
        if let Def::Op {
            operands, defines, ..
        } = &mut v.def
        {
            operands.retain(|o| {
                !matches!(o, Operand::Named(_, from)
                    if matches!(p.values[from.0 as usize].def, Def::Const(_)))
            });
            for d in defines.iter_mut() {
                *d = unnumbered(d);
            }
        }
    }
    q.to_text()
}

/// What changed from `a` to `b`, value by value: the comment, then the
/// imports and exports gone (`-`) and come (`+`).
fn changes(a: &Program, b: &Program) -> String {
    use std::fmt::Write as _;
    let named = |o: &Operand| match o {
        Operand::Named(n, v) => format!("{}={v}", unnumbered(n)),
        _ => String::new(),
    };
    let mut out = String::new();
    for (i, (x, y)) in a.values.iter().zip(&b.values).enumerate() {
        if x == y {
            continue;
        }
        let _ = writeln!(out, "%{i} {}", y.shows);
        let (
            Def::Op {
                operands: oa,
                defines: da,
                ..
            },
            Def::Op {
                operands: ob,
                defines: db,
                ..
            },
        ) = (&x.def, &y.def)
        else {
            continue;
        };
        let (oa, ob): (Vec<String>, Vec<String>) = (
            oa.iter().map(named).collect(),
            ob.iter().map(named).collect(),
        );
        let (da, db): (Vec<String>, Vec<String>) = (
            da.iter().map(|d| unnumbered(d)).collect(),
            db.iter().map(|d| unnumbered(d)).collect(),
        );
        for o in oa.iter().filter(|o| !ob.contains(o)) {
            let _ = writeln!(out, "  -import {o}");
        }
        for o in ob.iter().filter(|o| !oa.contains(o)) {
            let _ = writeln!(out, "  +import {o}");
        }
        for d in da.iter().filter(|d| !db.contains(d)) {
            let _ = writeln!(out, "  -export {d}");
        }
        for d in db.iter().filter(|d| !da.contains(d)) {
            let _ = writeln!(out, "  +export {d}");
        }
    }
    out
}

/// The view of a small document, and of it rebuilt after a word
/// changed: a value per window, its imports from the windows that
/// defined them, its exports, its span and the text it set; one step's
/// calls with the addresses named as in the view; after the edit, what
/// changed ([`WARM`]).
#[test]
fn a_small_document() {
    let mut tex = build();
    let cold = view(&tex);
    checked(&cold);
    let a = edges(&cold);
    assert_eq!(a, COLD, "the view's edges are now:\n{a}");
    // (the paragraph's calls, the addresses named as in the view: its
    // start, to the first word after the macro, reads it, and the step of
    // its lines breaks them)
    let t = step_trace(&tex, 11).unwrap();
    assert!(t.contains(r"read @\greet = "), "{t}");
    assert_eq!(Trace::parse(&t).unwrap().to_text(), t);
    let t = step_trace(&tex, 12).unwrap();
    assert!(t.contains("call @line_break("), "{t}");
    assert_eq!(Trace::parse(&t).unwrap().to_text(), t);
    let edited = DOC.replace("World", "Moon");
    tex.host_mut()
        .files
        .insert(b"doc.tex".to_vec(), edited.into_bytes());
    let r = rebuild(&mut tex, false, true);
    assert!(r.unsupported.is_none(), "{:?}", r.unsupported);
    let warm = view(&tex);
    checked(&warm);
    let d = changes(&cold, &warm);
    assert_eq!(d, WARM, "the changes are now:\n{d}");
}

/// The edges of `DOC`'s view.
const COLD: &str = r#"%0 = format                                  ; step 0: the job's start, (INITEX), 32708 definitions: doc:1
%1 = file doc
%2 = file cmr10
%3 = window(; \catcode123)                   ; step 1: doc:1 "\catcode`\{=1 \catcode`\}=2 \catcode`\#=…"
%4 = window(\catcode123=%3; \catcode125)     ; step 2: doc:1 "\catcode`\}=2 \catcode`\#=6"
%5 = window(\catcode123=%3, \catcode125=%4; \catcode35) ; step 3: doc:1 "\catcode`\#=6"
%6 = window(\catcode123=%3, \catcode125=%4, \catcode35=%5; string:N, str_ptr, text:\rm, string:N, font_count, font:cmr10.number, font:cmr10.metrics, font:cmr10.fontdimen, font:cmr10.hyphenchar, font:cmr10.skewchar, font:cmr10.expand, font:cmr10.glue, font:cmr10.code0, font:cmr10.code1, font:cmr10.code2, font:cmr10.code3, font:cmr10.code4, font:cmr10.code5, font:cmr10.code6, font:cmr10.code7, fonts, \rm, fontid:1:\rm, text:fontid:1:\rm, current_font) ; step 4: doc:2 "\font\rm=cmr10 \rm \hsize=100pt \vsize=1…"
%7 = window(; hsize)                         ; step 5: doc:2 "\hsize=100pt \vsize=100pt"
%8 = window(; vsize)                         ; step 6: doc:2 "\vsize=100pt"
%9 = window(\catcode35=%5, str_ptr=%6, \catcode123=%3, \catcode125=%4; string:N, str_ptr, text:\greet, align_state, \greet) ; step 7: doc:3 "\def\greet#1{Hello #1.}"
%10 = window(\catcode35=%5, \catcode123=%3, \catcode125=%4; ) ; step 8: doc:3
%11 = window(\catcode35=%5, \catcode123=%3, \catcode125=%4; \count1) ; step 9: doc:4 "\count1=5"
%12 = window(; )                             ; step 10: doc:5
%13 = window(\catcode123=%3, text:\greet=%9, \greet=%9, align_state=%9, \catcode125=%4, current_font=%6, font:cmr10.metrics=%6, font:cmr10.hyphenchar=%6, font:cmr10.fontdimen=%6; align_state, list.pg, nest, list1.mlist, list1.mode, list1.pg, list1.ml, list1.prev_depth, list1.clang, list1.incompleat, list1.middle, list1.lr_save, list1.lr_box, page.last_glue, page.last_penalty, page.last_kern, page.last_node_type, page_step_result, list.list, list1.space_factor, list1.list, font:cmr10.glue) ; step 11: doc:6 "Hello World."
%14 = window(\catcode123=%3, \catcode125=%4, nest=%13, list1.mode=%13, current_font=%6, font:cmr10.metrics=%6, list1.clang=%13, list1.list=%13, font:cmr10.hyphenchar=%6, font:cmr10.fontdimen=%6, align_state=%13, list1.pg=%13, hsize=%7, list.pg=%13, font:cmr10.expand=%6, text:fontid:1:\rm=%6, list.list=%13; list1.ml, list1.space_factor, list1.list, nest, hyph.patterns, last_badness, selector, hpack_result, sealed:37c7f75150622283, list.list, list.prev_depth, list.pg, line_break_result, list.lr_save, error_count) ; step 12: doc:6-7 "Hello World. One line."
%15 = window(nest=%14, list.list=%14, vsize=%8; page.contents, page.goal, page.fil, page.fill, page.filll, page.max_depth, page.least_cost, page.stretch, page.shrink, page[0], page.total, page.depth, page.last_glue, page.last_penalty, page.last_kern, page.last_node_type, page_step_result, page[1], page.len, page.tail, list.list) ; step 13: "Hello World. One line."
%16 = window(\catcode123=%3, align_state=%13, nest=%14, list.list=%15; align_state, save[0], save.boundary, save.group, save.level, save.ptr) ; step 14: doc:8 "{\count1=6 Second paragraph.}"
%17 = window(\catcode123=%3, \catcode125=%4, nest=%14, save.level=%16, \count1=%11, save.ptr=%16, list.list=%15; save[1], save.ptr, save[2], \count1) ; step 15: doc:8 "\count1=6 Second paragraph.}"
%18 = window(\catcode123=%3, \catcode125=%4, nest=%14, list.list=%15, list.prev_depth=%14, page.contents=%15, page.goal=%15, page.total=%15, page.stretch=%15, page.fil=%15, page.fill=%15, page.filll=%15, page.shrink=%15, page.depth=%15, page.max_depth=%15, page.least_cost=%15, page.len=%15, page.tail=%15, current_font=%6, font:cmr10.metrics=%6, font:cmr10.hyphenchar=%6, font:cmr10.fontdimen=%6; list.pg, nest, list1.mlist, list1.mode, list1.pg, list1.ml, list1.prev_depth, list1.clang, list1.incompleat, list1.middle, list1.lr_save, list1.lr_box, page.total, page.stretch, page.shrink, page.depth, page.least_cost, page.best_break, page.best_size, page.last_glue, page.last_penalty, page.last_kern, page.last_node_type, page_step_result, page[2], page.len, page.tail, list.list, list1.space_factor, list1.list) ; step 16: doc:8 "Second"
%19 = window(\catcode123=%3, \catcode125=%4, nest=%18, list1.mode=%18, current_font=%6, font:cmr10.metrics=%6, list1.clang=%18, list1.list=%18, font:cmr10.hyphenchar=%6, align_state=%16, save.group=%16, save.level=%16, save.ptr=%17, save[2]=%17, save[1]=%17, \count1=%17, save[0]=%16, save.boundary=%16, font:cmr10.fontdimen=%6, list1.pg=%18, hsize=%7, list.pg=%18, list.lr_save=%14, hyph.patterns=%14, selector=%14, font:cmr10.expand=%6, text:fontid:1:\rm=%6, list.prev_depth=%14, list.list=%18; list1.ml, list1.space_factor, align_state, save.level, save.ptr, save[1], \count1, save.group, save.boundary, list1.list, nest, last_badness, selector, hpack_result, sealed:a1557562e55d2c4f, list.list, list.prev_depth, list.pg, line_break_result, list.lr_save, error_count) ; step 17: doc:8-9 "Second paragraph."
%20 = window(nest=%19, list.list=%19, page.contents=%15, page.total=%18, page.stretch=%18, page.shrink=%18, page.depth=%18, page.max_depth=%15, page.tail=%18, page.len=%18; page.stretch, page.shrink, page[3], page.total, page.depth, page.last_glue, page.last_penalty, page.last_kern, page.last_node_type, page_step_result, page[4], page.len, page.tail, list.list) ; step 18: "Second paragraph."
%21 = window(nest=%19, page.len=%20, hsize=%7, list.list=%20, page.contents=%15, page.total=%20, page.depth=%20, page.max_depth=%15, page.goal=%15, page.stretch=%20, page.fil=%15, page.fill=%15, page.filll=%15, page.shrink=%20, page.least_cost=%18; page[5], page.total, page.fill, page.shrink, page.depth, page[6], page.len, page.tail, page.least_cost, page.best_break, page.best_size, page.last_glue, page.last_penalty, page.last_kern, page.last_node_type, page_step_result, list.list) ; step 19: doc:10 "\end"
%22 = window(nest=%19, list.list=%21, page.contents=%15, page.goal=%15, page.total=%21, page.stretch=%20, page.fil=%15, page.fill=%21, page.filll=%15, page.shrink=%21, page.depth=%21, page.max_depth=%15, page.least_cost=%21, page.best_break=%21, page.best_size=%21, page.last_glue=%21, page.last_penalty=%21, page.last_kern=%21, page.last_node_type=%21, page.len=%21, page[0]=%15, page[1]=%15, page[2]=%18, page[3]=%20, page[4]=%20, page[5]=%21, page[6]=%21, sealed:37c7f75150622283=%14, sealed:a1557562e55d2c4f=%19, selector=%19, \count1=%19, str_ptr=%9, font:cmr10.number=%6, save.level=%19; page.contents, page.goal, page.total, page.stretch, page.fil, page.fill, page.filll, page.shrink, page.depth, page.max_depth, page.least_cost, page.best_break, page.best_size, page.ins, page.insert_penalties, last_badness, vpack_result, outputpenalty, page.len, page.tail, page.discards, string:N, str_ptr, string:N, string:N, output_file_name, dead_cycles, \box255, page.last_glue, page.last_penalty, page.last_kern, page.last_node_type, page_step_result, list.list, open_parens, newlinechar, mag_set, dvi.file, dvi.fonts, dvi.totals, dvi.writer, write:log, selector) ; step 20: ships [0.5] "Hello World. One line. Second paragraph."
"#;

/// What changed after the edit: the paragraph ran again, and the fire;
/// the run found the font's glue the cold build made (the glue is read
/// through the `\fontdimen`s it is made from), and the second paragraph's
/// import of the patterns is now the one before the first paragraph (its
/// run read the patterns the cold build had made, which are not placed).
const WARM: &str = r#"%13 step 11 (run 2): doc:6 "Hello Moon."
  -import \catcode87=%0
  -import \catcode108=%0
  -import \catcode100=%0
  -import \sfcode87=%0
  -import \sfcode114=%0
  -import \sfcode100=%0
  +import \catcode77=%0
  +import \catcode110=%0
  +import \sfcode77=%0
  +import \sfcode110=%0
  -export font:cmr10.glue
%14 step 12 (run 2): doc:6-7 "Hello Moon. One line."
  -import \catcode87=%0
  -import \catcode100=%0
  -import \lccode87=%0
  +import \catcode77=%0
  +import \lccode77=%0
  -export hyph.patterns
%15 step 13: "Hello Moon. One line."
%19 step 17: doc:8-9 "Second paragraph."
  -import hyph.patterns=%14
  +import hyph.patterns=%0
%22 step 20 (run 2): ships [0.5] "Hello Moon. One line. Second paragraph."
"#;
