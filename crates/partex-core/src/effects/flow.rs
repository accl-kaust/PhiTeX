//! The log's and the terminal's text relative to their columns (DESIGN
//! 3.8, "Columns are the link's").
//!
//! `term_offset` and `file_offset` (§54) are where the terminal's and the
//! log's current lines stand. TeX reads them only to decide what to print:
//! where a line wraps at `max_print_line` (§58), whether `print_nl` starts
//! a line (§62), and the space-or-new-line rule before `[` (§638), a file
//! name's `(` (§537) and a `\message` (§1280). As state, every step that
//! prints reads and writes them, so one message a character longer would
//! make every later step that prints run again. They are output position
//! instead, like a PDF object's offset: with effects in an SSA build
//! (`Tex::flow`), the engine records what it prints as [`Effect::Flow`]
//! ops that say what to do at each of those decisions, and the link
//! renders them ([`render`]) from the columns the step before left, into
//! the log's bytes and the terminal's text that pdfTeX writes.
//!
//! An op is a byte, then its operands; `mask` says which outputs it goes
//! to: 1 the terminal, 2 the log.

use alloc::vec::Vec;

use super::{Effect, Stream};
use crate::host::WriteId;

/// `TEXT mask n c₁ … cₙ`: characters printed (§58), each column counted
/// and wrapped at `max_print_line`.
pub(crate) const TEXT: u8 = 1;
/// `NL mask`: `print_ln` (§57).
pub(crate) const NL: u8 = 2;
/// `NLC mask`: `print_nl`'s new line, if a column of the outputs is not
/// at its start (§62).
pub(crate) const NLC: u8 = 3;
/// `SEP mask t₀ t₁ t₂ t₃ nl`: if the terminal's column is past `t` (an
/// `i32`), a new line; else if either column is not at its start, a
/// space, which is a new line if `nl` (the space is `\newlinechar`).
pub(crate) const SEP: u8 = 4;
/// `RAW mask n₀ n₁ n₂ n₃ bytes`: bytes that move no column (`wlog`,
/// `wterm` of a Pascal string).
pub(crate) const RAW: u8 = 5;
/// `COL0 mask`: the columns of the outputs set to their start (§55, a
/// line typed on the terminal, §71).
pub(crate) const COL0: u8 = 6;
/// `LEN file₀..₃ assumed₀..₇` … `LEN_END`: the digits between are the
/// length of `file`, taken to be `assumed` ([`Effect::Length`]).
pub(crate) const LEN: u8 = 7;
pub(crate) const LEN_END: u8 = 8;

/// The columns of the terminal and the log.
pub type Cols = (i32, i32);

fn u32_at(ops: &[u8], i: usize) -> u32 {
    let mut b = [0u8; 4];
    b.copy_from_slice(&ops[i..i + 4]);
    u32::from_le_bytes(b)
}

/// The text being rendered for each output, and what it is cut into.
struct Out {
    log: Option<WriteId>,
    term: Vec<u8>,
    file: Vec<u8>,
    cols: Cols,
    max: i32,
}

impl Out {
    fn ch(&mut self, mask: u8, c: u8) {
        if mask & 1 != 0 {
            self.term.push(c);
            self.cols.0 += 1;
            if self.cols.0 == self.max {
                self.term.push(b'\n');
                self.cols.0 = 0;
            }
        }
        if mask & 2 != 0 {
            self.file.push(c);
            self.cols.1 += 1;
            if self.cols.1 == self.max {
                self.file.push(b'\n');
                self.cols.1 = 0;
            }
        }
    }

    fn nl(&mut self, mask: u8) {
        if mask & 1 != 0 {
            self.term.push(b'\n');
            self.cols.0 = 0;
        }
        if mask & 2 != 0 {
            self.file.push(b'\n');
            self.cols.1 = 0;
        }
    }

    /// The text so far, as the effects the engine's direct path makes.
    fn flush(&mut self, out: &mut Vec<Effect>) {
        if !self.file.is_empty() {
            let bytes = core::mem::take(&mut self.file);
            if let Some(file) = self.log {
                push_write(out, file, bytes);
            }
        }
        if !self.term.is_empty() {
            let bytes = core::mem::take(&mut self.term);
            if let Some(Effect::Term(t)) = out.last_mut() {
                t.extend_from_slice(&bytes);
            } else {
                out.push(Effect::Term(bytes));
            }
        }
    }
}

fn push_write(out: &mut Vec<Effect>, file: WriteId, bytes: Vec<u8>) {
    if let Some(Effect::Write { file: f, bytes: b }) = out.last_mut()
        && *f == file
    {
        b.extend_from_slice(&bytes);
    } else {
        out.push(Effect::Write { file, bytes });
    }
}

/// Whether `fx` has text to render.
#[must_use]
pub fn has_flow(fx: &[Effect]) -> bool {
    fx.iter().any(|e| matches!(e, Effect::Flow { .. }))
}

/// `fx` with its [`Effect::Flow`]s rendered from the columns `cols`,
/// which are left where the text ends; lines wrap at `max`
/// (`max_print_line`).
#[must_use]
pub fn render(fx: &[Effect], cols: &mut Cols, max: i32) -> Vec<Effect> {
    let mut out = Vec::with_capacity(fx.len());
    for e in fx {
        let Effect::Flow { log, ops } = e else {
            out.push(e.clone());
            continue;
        };
        let mut o = Out {
            log: *log,
            term: Vec::new(),
            file: Vec::new(),
            cols: *cols,
            max,
        };
        // (a byte count being printed: its file and guess)
        let mut len: Option<(WriteId, i64)> = None;
        let mut i = 0;
        while i < ops.len() {
            let (op, mask) = (ops[i], ops[i + 1]);
            i += 2;
            match op {
                TEXT => {
                    let n = usize::from(ops[i]);
                    for &c in &ops[i + 1..i + 1 + n] {
                        o.ch(mask, c);
                    }
                    i += 1 + n;
                }
                NL => o.nl(mask),
                NLC => {
                    if (mask & 1 != 0 && o.cols.0 > 0) || (mask & 2 != 0 && o.cols.1 > 0) {
                        o.nl(mask);
                    }
                }
                SEP => {
                    let t = u32_at(ops, i).cast_signed();
                    let nl = ops[i + 4] != 0;
                    i += 5;
                    if o.cols.0 > t {
                        o.nl(mask);
                    } else if o.cols.0 > 0 || o.cols.1 > 0 {
                        if nl {
                            o.nl(mask);
                        } else {
                            o.ch(mask, b' ');
                        }
                    }
                }
                RAW => {
                    let n = u32_at(ops, i) as usize;
                    let b = &ops[i + 4..i + 4 + n];
                    if mask & 1 != 0 {
                        o.term.extend_from_slice(b);
                    }
                    if mask & 2 != 0 {
                        o.file.extend_from_slice(b);
                    }
                    i += 4 + n;
                }
                COL0 => {
                    if mask & 1 != 0 {
                        o.cols.0 = 0;
                    }
                    if mask & 2 != 0 {
                        o.cols.1 = 0;
                    }
                }
                LEN => {
                    let file = WriteId(u32_at(ops, i));
                    let mut a = [0u8; 8];
                    a.copy_from_slice(&ops[i + 4..i + 12]);
                    i += 12;
                    o.flush(&mut out);
                    len = Some((file, i64::from_le_bytes(a)));
                }
                LEN_END => {
                    if let Some((file, assumed)) = len.take() {
                        if let Some(log) = o.log
                            && !o.file.is_empty()
                        {
                            out.push(Effect::Length {
                                stream: Stream::File(log),
                                file,
                                assumed,
                                text: core::mem::take(&mut o.file),
                            });
                        }
                        if !o.term.is_empty() {
                            out.push(Effect::Length {
                                stream: Stream::Term,
                                file,
                                assumed,
                                text: core::mem::take(&mut o.term),
                            });
                        }
                    }
                }
                _ => unreachable!("a flow op"),
            }
        }
        o.flush(&mut out);
        *cols = o.cols;
    }
    out
}

/// The engine's side: where the open text run is, to add to it.
#[derive(Clone, Debug, Default)]
pub(crate) struct Flow {
    /// Printing makes [`Effect::Flow`] ops (an SSA build's effects).
    pub(crate) on: bool,
    /// The decision being made runs `print_ln` and `print_char` for the
    /// engine's own columns only: its op is made already.
    pub(crate) quiet: bool,
    /// The open `TEXT` op: its mask and the index of its count in the
    /// last effect's ops.
    run: Option<(u8, usize)>,
}

impl Flow {
    /// Add op bytes `op` to `fx`'s last effect if it is a flow to the same
    /// log, else to a new one (the bool: whether it is new).
    pub(crate) fn push(&mut self, fx: &mut Vec<Effect>, log: Option<WriteId>, op: &[u8]) -> bool {
        self.run = None;
        if let Some(Effect::Flow { log: l, ops }) = fx.last_mut()
            && *l == log
        {
            ops.extend_from_slice(op);
            return false;
        }
        fx.push(Effect::Flow {
            log,
            ops: op.to_vec(),
        });
        true
    }

    /// Character `c` to the outputs `mask` (the bool: a new flow).
    pub(crate) fn text(
        &mut self,
        fx: &mut Vec<Effect>,
        log: Option<WriteId>,
        mask: u8,
        c: u8,
    ) -> bool {
        if let (Some((m, at)), Some(Effect::Flow { log: l, ops })) = (self.run, fx.last_mut())
            && m == mask
            && *l == log
            && ops
                .get(at)
                .is_some_and(|&n| n < u8::MAX && ops.len() == at + 1 + usize::from(n))
        {
            ops[at] += 1;
            ops.push(c);
            return false;
        }
        let new = self.push(fx, log, &[TEXT, mask, 1, c]);
        if let Some(Effect::Flow { ops, .. }) = fx.last() {
            self.run = Some((mask, ops.len() - 2));
        }
        new
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(fx: &[Effect]) -> (Vec<u8>, Vec<u8>) {
        let (mut t, mut f) = (Vec::new(), Vec::new());
        for e in fx {
            match e {
                Effect::Term(b) => t.extend_from_slice(b),
                Effect::Write { bytes, .. } => f.extend_from_slice(bytes),
                _ => {}
            }
        }
        (t, f)
    }

    /// The same ops render the wraps and separators at the columns they
    /// start from.
    #[test]
    fn renders_from_the_columns() {
        let mut fl = Flow::default();
        let mut fx = Vec::new();
        let log = Some(WriteId(3));
        let mut sep = alloc::vec![SEP, 3];
        sep.extend_from_slice(&(10 - 9i32).to_le_bytes());
        sep.push(0);
        fl.push(&mut fx, log, &sep);
        for &c in b"[12]" {
            fl.text(&mut fx, log, 3, c);
        }
        fl.push(&mut fx, log, &[NLC, 2]);
        fl.text(&mut fx, log, 2, b'x');
        let mut cols = (0, 0);
        let r = render(&fx, &mut cols, 10);
        assert_eq!(text(&r), (b"[12]".to_vec(), b"[12]\nx".to_vec()));
        assert_eq!(cols, (4, 1));
        let mut cols = (3, 8);
        let r = render(&fx, &mut cols, 10);
        assert_eq!(text(&r), (b"\n[12]".to_vec(), b"\n[12]\nx".to_vec()));
        let mut cols = (1, 7);
        let r = render(&fx, &mut cols, 10);
        assert_eq!(text(&r), (b" [12]".to_vec(), b" [1\n2]\nx".to_vec()));
        assert_eq!(cols, (6, 1));
    }
}
