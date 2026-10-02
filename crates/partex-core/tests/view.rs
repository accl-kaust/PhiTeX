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
    // (the paragraph's calls, the addresses named as in the view)
    let t = step_trace(&tex, 11).unwrap();
    assert!(t.contains(r"read @\greet = "), "{t}");
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
const COLD: &str = r#"%0 = format                                  ; step 0: the job's start, (INITEX), 31944 definitions: doc:1
%1 = file doc
%2 = window(; \catcode123)                   ; step 1: doc:1 "\catcode`\{=1 \catcode`\}=2 \catcode`\#=…"
%3 = window(\catcode123=%2; \catcode125)     ; step 2: doc:1 "\catcode`\}=2 \catcode`\#=6"
%4 = window(\catcode123=%2, \catcode125=%3; \catcode35) ; step 3: doc:1 "\catcode`\#=6"
%5 = window(\catcode123=%2, \catcode125=%3, \catcode35=%4; string:N, str_ptr, text:\rm, string:N, font:cmr10.metrics, font:cmr10.fontdimen, font:cmr10.hyphenchar, font:cmr10.skewchar, font:cmr10.expand, font:cmr10.glue, font:cmr10.code0, font:cmr10.code1, font:cmr10.code2, font:cmr10.code3, font:cmr10.code4, font:cmr10.code5, font:cmr10.code6, font:cmr10.code7, fonts, \rm, fontid:1:\rm, text:fontid:1:\rm, current_font) ; step 4: doc:2 "\font\rm=cmr10 \rm \hsize=100pt \vsize=1…"
%6 = window(; hsize)                         ; step 5: doc:2 "\hsize=100pt \vsize=100pt"
%7 = window(; vsize)                         ; step 6: doc:2 "\vsize=100pt"
%8 = window(\catcode35=%4, str_ptr=%5, \catcode123=%2, \catcode125=%3; string:N, str_ptr, text:\greet, align_state, \greet) ; step 7: doc:3 "\def\greet#1{Hello #1.}"
%9 = window(\catcode35=%4, \catcode123=%2, \catcode125=%3; ) ; step 8: doc:3
%10 = window(\catcode35=%4, \catcode123=%2, \catcode125=%3; \count1) ; step 9: doc:4 "\count1=5"
%11 = window(; )                             ; step 10: doc:5
%12 = window(\catcode123=%2, text:\greet=%8, \greet=%8, align_state=%8, \catcode125=%3, current_font=%5, font:cmr10.metrics=%5, font:cmr10.hyphenchar=%5, font:cmr10.glue=%5, font:cmr10.fontdimen=%5, hsize=%6, font:cmr10.expand=%5, text:fontid:1:\rm=%5, vsize=%7; align_state, font:cmr10.glue, list.mlist, list.mode, list.ml, list.space_factor, list.clang, list.incompleat, list.middle, list.lr_box, nest, hyph.patterns, last_badness, file_offset, selector, hpack_result, list.prev_depth, list.pg, line_break_result, list.lr_save, error_count, page.contents, page.goal, page.fil, page.fill, page.filll, page.max_depth, page.least_cost, page.stretch, page.shrink, page[0], page.total, page.depth, page.last_glue, page.last_penalty, page.last_kern, page.last_node_type, page_step_result, page[1], page.len, page.tail, list.list) ; step 11: doc:6-7 "Hello World. One line."
%13 = window(\catcode123=%2, align_state=%12, list.mode=%12, nest=%12, list.list=%12; align_state, save[0], save.boundary, save.group, save.level, save.ptr) ; step 12: doc:8 "{\count1=6 Second paragraph.}"
%14 = window(\catcode123=%2, \catcode125=%3, list.mode=%12, save.level=%13, \count1=%10, save.ptr=%13, nest=%12, list.list=%12; save[1], save.ptr, save[2], \count1) ; step 13: doc:8 "\count1=6 Second paragraph.}"
%15 = window(\catcode123=%2, \catcode125=%3, list.mode=%12, list.list=%12, nest=%12, list.mlist=%12, list.ml=%12, list.prev_depth=%12, list.space_factor=%12, list.clang=%12, list.incompleat=%12, list.middle=%12, list.lr_save=%12, list.lr_box=%12, page.contents=%12, page.goal=%12, page.total=%12, page.stretch=%12, page.fil=%12, page.fill=%12, page.filll=%12, page.shrink=%12, page.depth=%12, page.max_depth=%12, page.least_cost=%12, page.len=%12, page.tail=%12, current_font=%5, font:cmr10.metrics=%5, font:cmr10.hyphenchar=%5, font:cmr10.glue=%12, align_state=%13, save.group=%13, save.level=%13, save.ptr=%14, save[2]=%14, save[1]=%14, \count1=%14, save[0]=%13, save.boundary=%13, hsize=%6, hyph.patterns=%12, selector=%12, font:cmr10.expand=%5, text:fontid:1:\rm=%5; page.least_cost, page.best_break, page.best_size, page[2], align_state, save.level, save.ptr, save[1], \count1, save.group, save.boundary, list.mlist, list.mode, list.ml, list.space_factor, list.clang, list.incompleat, list.middle, list.lr_box, nest, last_badness, file_offset, selector, hpack_result, list.prev_depth, list.pg, line_break_result, list.lr_save, error_count, page.stretch, page.shrink, page[3], page.total, page.depth, page.last_glue, page.last_penalty, page.last_kern, page.last_node_type, page_step_result, page[4], page.len, page.tail, list.list) ; step 14: doc:8-9 "Second paragraph."
%16 = window(list.mode=%15, page.len=%15, hsize=%6, list.list=%15, nest=%15, list.mlist=%15, list.pg=%15, list.ml=%15, list.prev_depth=%15, list.space_factor=%15, list.clang=%15, list.incompleat=%15, list.middle=%15, list.lr_save=%15, list.lr_box=%15, page.contents=%12, page.total=%15, page.depth=%15, page.max_depth=%12, page.goal=%12, page.stretch=%15, page.fil=%12, page.fill=%12, page.filll=%12, page.shrink=%15, page.least_cost=%15; page[5], page.total, page.fill, page.shrink, page.depth, page[6], page.len, page.tail, page.least_cost, page.best_break, page.best_size, page.last_glue, page.last_penalty, page.last_kern, page.last_node_type, page_step_result, list.list) ; step 15: doc:10 "\end"
%17 = window(nest=%15, list.list=%16, page.contents=%12, page.goal=%12, page.total=%16, page.stretch=%15, page.fil=%12, page.fill=%16, page.filll=%12, page.shrink=%16, page.depth=%16, page.max_depth=%12, page.least_cost=%16, page.best_break=%16, page.best_size=%16, page.last_glue=%16, page.last_penalty=%16, page.last_kern=%16, page.last_node_type=%16, page.len=%16, page[0]=%12, page[1]=%12, page[2]=%15, page[3]=%15, page[4]=%15, page[5]=%16, page[6]=%16, file_offset=%15, selector=%15, \count1=%15, str_ptr=%8, list.mlist=%15, list.mode=%15, list.pg=%15, list.ml=%15, list.prev_depth=%15, list.space_factor=%15, list.clang=%15, list.incompleat=%15, list.middle=%15, list.lr_save=%15, list.lr_box=%15, save.level=%15; page.contents, page.goal, page.total, page.stretch, page.fil, page.fill, page.filll, page.shrink, page.depth, page.max_depth, page.least_cost, page.best_break, page.best_size, page.ins, page.insert_penalties, last_badness, vpack_result, outputpenalty, page.len, page.tail, page.discards, string:N, str_ptr, string:N, string:N, output_file_name, dead_cycles, \box255, page.last_glue, page.last_penalty, page.last_kern, page.last_node_type, page_step_result, list.list, file_offset, open_parens, newlinechar, mag_set, dvi.file, dvi.fonts, dvi.totals, dvi.writer, write:log, selector) ; step 16: ships [0.5] "Hello World. One line. Second paragraph."
"#;

/// What changed after the edit: the paragraph ran again, and the fire;
/// the second paragraph's imports of the font's glue and of the patterns
/// are now the ones before the first paragraph (its run read the glue and
/// the patterns the cold build had made, which are not placed).
const WARM: &str = r#"%12 step 11 (run 2): doc:6-7 "Hello Moon. One line."
  -import \catcode87=%0
  -import \catcode100=%0
  -import font:cmr10.fontdimen=%5
  -import \sfcode87=%0
  -import \sfcode114=%0
  -import \sfcode100=%0
  -import \lccode87=%0
  +import \catcode77=%0
  +import \sfcode77=%0
  +import \lccode77=%0
  -export font:cmr10.glue
  -export hyph.patterns
%15 step 14: doc:8-9 "Second paragraph."
  -import font:cmr10.glue=%12
  -import hyph.patterns=%12
  +import font:cmr10.glue=%5
  +import hyph.patterns=%0
%17 step 16 (run 2): ships [0.5] "Hello Moon. One line. Second paragraph."
"#;
