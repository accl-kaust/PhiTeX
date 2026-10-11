//! What a node sees while it is reduced. Implemented by A1 (`graph.rs`).
//! Every read is recorded; every change goes into the node's `Out`.
use std::sync::Arc;

use crate::env::{GroupKind, Mode};
use crate::item::Item;
use crate::term::{Emit, Span, TokKind};
use crate::tok::{Names, Tok, Toks};
use crate::val::{Key, Meaning, Val};

pub trait Ctx {
    // ---- input
    /// The next token, unexpanded (tokenizes text with the catcodes in env;
    /// reads `Key::Cat` of each character taken, `\endlinechar` at line ends).
    /// `None`: the input is exhausted (the job's end).
    fn next(&mut self) -> Option<Tok>;
    /// The next unexpandable token: expands macros (Apply nodes), expandable
    /// primitives and conditionals as it goes. Honours `Tok::DontExpand`.
    fn next_x(&mut self) -> Option<Tok>;
    /// Puts a token back in front of the input (`back_input`).
    fn back(&mut self, t: Tok);
    /// Pushes a token list in front of the input.
    fn push(&mut self, toks: Toks, kind: TokKind);
    /// Pushes a text in front of the input (`\input`, `\scantokens`, `\read` lines):
    /// returns its id; `name` for errors/`\inputlineno`.
    fn push_text(&mut self, name: Arc<str>, data: Arc<str>) -> crate::term::TextId;
    /// `\endinput`: the current file ends at the end of its current line.
    fn end_input(&mut self);
    /// The kind of the innermost token list being read, if the input head is one.
    fn reading(&self) -> Option<TokKind>;
    /// The current line number of the innermost text (records a `Key::LineNo` read).
    fn line_no(&mut self) -> i32;
    /// Skips raw (no expansion) up to a token satisfying `stop`, nesting by `nest`
    /// (+1/-1/0 per token); returns the skipped span(s). Used to pass an untaken arm.
    fn skip_raw(&mut self, stop: &mut dyn FnMut(&Tok, i32) -> bool, nest: &mut dyn FnMut(&Tok) -> i32) -> Vec<Span>;

    // ---- environment (reads recorded)
    fn get(&mut self, key: &Key) -> Val;
    fn meaning(&mut self, t: &Tok) -> Meaning;
    fn int(&mut self, key: &Key) -> i32 {
        self.get(key).int()
    }
    fn define(&mut self, key: Key, val: Val, global: bool);
    /// `\globaldefs` applied by the caller; this is the raw change.
    fn open_group(&mut self, kind: GroupKind);
    /// Closes the innermost group, pushing its `\aftergroup` tokens.
    fn close_group(&mut self);
    fn after_group(&mut self, t: Tok);
    fn group_kind(&mut self) -> GroupKind;
    fn group_depth(&mut self) -> u32;
    fn mode(&mut self) -> Mode;
    fn open_list(&mut self, mode: Mode);
    /// Closes the innermost list; returns its items (all emits since its open).
    fn close_list(&mut self) -> Vec<Item>;
    /// The current list's last item (a recorded read of `Key::Tail`).
    fn tail(&mut self) -> Option<Item>;

    // ---- output
    fn emit(&mut self, e: Emit);
    fn item(&mut self, it: Item) {
        self.emit(Emit::Item(it));
    }

    // ---- the build
    fn names(&self) -> &Names;
    /// An input file's bytes by the name TeX asked for (kpse lookup inside; a
    /// recorded read of `Key::File`). Written files come from the loop-back.
    fn file(&mut self, name: &str) -> Option<(Arc<str>, Arc<[u8]>)>;
    fn etex(&self) -> bool;
    fn pdf(&self) -> bool;
    /// Speculation abandoned / job stopped: handlers should return soon.
    fn halted(&self) -> bool;
}
