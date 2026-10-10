//! Input and output (tex.web parts 23–24, 29, 48–49, 53): files are
//! values, and nothing opens or writes a file while commands run.
//!
//! - Reading a file is a read of `Key::File(name)`: its bytes, answered by
//!   the build's `Env` (a pure function of the name during a build). A
//!   changed file re-runs exactly its readers.
//! - A read stream is a `Key`: `\openin n=name` defines
//!   `Key::ReadStream(n)` = `Val::Stream` (the file's bytes and where its
//!   next line begins); `\read n to\x` reads it, defines `\x`, and defines
//!   the stream anew, advanced by the lines it read; `\closein` defines it
//!   closed. Streams are global (TeX's `read_open` has no save stack).
//! - `\input` reads `Key::File`; the reader goes into the file's text
//!   (an entry of `RState::texts`, `TextKind::File`, as `\scantokens`'s
//!   text is), above the input pending at the call, as TeX's input stack
//!   does. At the top `\input` is a node of its own whose result is the
//!   file's text in its out state: the core may cut that text into pieces
//!   instead of reading it in this piece (CORE.md, "`\input` (decided)").
//! - Writing is a node's result. `\immediate\openout`, `\write`,
//!   `\closeout` make an event in the node's log ([`crate::logs::io_event`]);
//!   without `\immediate` they are whatsit items, and shipping a page makes
//!   their events: an explicit `\shipout` in its node, a page the page
//!   builder ships at the join ([`page_io`]: the `\write`s expanded with the
//!   definitions reaching the place the page was shipped). The log's
//!   gatherer replays the events in document order ([`crate::logs::replay`]):
//!   which streams are open there decides whether a line goes to a file or
//!   to the log; the files' contents are `Output::writes`.
//! - `\end` and `\dump`: what `final_cleanup` writes after the job's last
//!   page (tex.web 1335).

use std::fmt::Write as _;
use std::sync::Arc;

use super::{Chain, EOL_UNREAD, Env, Exec, Mode, Outside, PendingText, Scratch, Stack, TState, TextKind};
use crate::item::{BoxNode, Item, Whatsit};
use crate::logs::{SEP, io_event};
use crate::print::{ENDJOB, MAX_PRINT_LINE};
use crate::show::{LN, NL, RAW, SP};
use crate::val::{IntPar, Key, Macro, Meaning, Prim, Stream, Tok, Val};

/// The name `\read` defines when it was not given one (tex.web 1215:
/// `frozen_protection`, which no document can spell).
const INACCESSIBLE: &str = "\u{0}inaccessible";

/// A file's text as the reader reads it (tex.web 31, `input_ln`, as
/// web2c reads lines): its lines — ending at `\n`, `\r` or `\r\n` — without
/// their trailing spaces, each ending in `\r`; each byte a character (TeX's
/// 8-bit input). An empty file is one empty line (tex.web 538 reads a first
/// line whatever).
pub fn file_text(data: &[u8]) -> Arc<[char]> {
    let mut out = Vec::with_capacity(data.len() + 1);
    let mut i = 0;
    while i < data.len() {
        let (line, next) = next_line(data, i);
        out.extend(line.iter().map(|&b| char::from(b)));
        out.push('\r');
        i = next;
    }
    if out.is_empty() {
        out.push('\r');
    }
    Arc::from(out)
}

/// The line of `data` beginning at `i`, without its trailing spaces, and
/// where the next line begins.
fn next_line(data: &[u8], i: usize) -> (&[u8], usize) {
    let end = data[i..].iter().position(|&b| b == b'\n' || b == b'\r').map_or(data.len(), |k| i + k);
    let next = if end >= data.len() {
        end
    } else if data[end] == b'\r' && data.get(end + 1) == Some(&b'\n') {
        end + 2
    } else {
        end + 1
    };
    let mut e = end;
    while e > i && data[e - 1] == b' ' {
        e -= 1;
    }
    (&data[i..e], next)
}

/// What printing `s` writes (tex.web 58–59): the `\newlinechar` begins a
/// new line (`\n`), characters below 32 and 127 are `^^` codes (tex.web 49).
fn printable(s: &str, newline: i32) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        let k = u32::from(c);
        if i64::from(k) == i64::from(newline) {
            out.push('\n');
        } else if k < 32 || k == 127 {
            out.push_str("^^");
            out.push(char::from_u32(if k < 64 { k + 64 } else { k - 64 }).unwrap_or('?'));
        } else {
            out.push(c);
        }
    }
    out
}

/// Whether a run's piece holds `\end`: the document ends there (CORE.md:
/// the pieces after it are not the document's).
pub fn ends_document(run: &super::Run) -> bool {
    run.paras.iter().any(|p| p.before.iter().chain(&p.after).any(|i| matches!(i, Item::End(_))))
}

/// The `\openout`, `\write` and `\closeout` whatsits of a box in the order
/// shipping meets them (tex.web 619–637: depth first; none inside leaders,
/// tex.web 1374's `doing_leaders`).
pub fn whatsits(b: &BoxNode) -> Vec<Arc<Whatsit>> {
    fn walk(list: &[Item], out: &mut Vec<Arc<Whatsit>>) {
        for it in list {
            match it {
                Item::Box(b) => walk(&b.list, out),
                Item::Whatsit(w) if !matches!(**w, Whatsit::Special(_)) => out.push(w.clone()),
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    walk(&b.list, &mut out);
    out
}

/// The events of shipping box `b` where node `at` of `chain` stands (the
/// place the page builder shipped it: TeX writes the page's whatsits then,
/// tex.web 1374): each `\write` expanded with the definitions reaching
/// there — the chain's, then the `Env`'s. A pure function of the box, the
/// place and those definitions; its errors go to `errors`.
pub fn page_io(chain: &Chain, at: usize, env: &mut dyn Env, b: &BoxNode, errors: &mut Vec<String>) -> String {
    if whatsits(b).is_empty() {
        return String::new();
    }
    let at = at.min(chain.nodes.len());
    let pre = Chain::prefix(chain, at);
    let input = pre.input(at);
    let mut outside = Outside::default();
    let mut scratch = Scratch::default();
    let mut x = Exec::new(&[], &pre, &input, env, &mut outside, &mut scratch);
    let s = x.ship_io(b);
    errors.append(&mut x.errors);
    s
}

/// A conditional's name from its code (`IfKind::code`; 32 more after
/// `\unless`, which is printed before it: etex.web's `print_cmd_chr`).
fn if_name(code: u8) -> &'static str {
    const NAMES: [&str; 20] = [
        "if", "ifcat", "ifnum", "ifdim", "ifodd", "ifvmode", "ifhmode", "ifmmode", "ifinner", "ifvoid", "ifhbox",
        "ifvbox", "ifx", "ifeof", "iftrue", "iffalse", "ifcase", "ifdefined", "ifcsname", "iffontchar",
    ];
    NAMES.get(usize::from(code % 32)).copied().unwrap_or("if")
}

/// The line `pos` of `text` is on (as the log counts: a line's end
/// character read is still that line's).
fn line_in(text: &[char], pos: usize) -> i32 {
    let p = pos.saturating_sub(1).min(text.len());
    1 + text[..p].iter().filter(|&&c| c == '\r').count() as i32
}

impl Exec<'_> {
    /// Appends an entry to the node's log (entries apart by
    /// [`crate::logs::SEP`]).
    pub(super) fn log_push(&mut self, e: &str) {
        match &mut self.log {
            Some(l) => {
                let mut s = l.to_string();
                s.push(SEP);
                s.push_str(e);
                *l = s.into_boxed_str();
            }
            None => self.log = Some(Box::from(e)),
        }
    }

    // ------------------------------------------------------------ files

    /// tex.web 526: a file name (characters up to a space, which goes, or
    /// anything else, which stays; web2c's `"` quotes spaces).
    pub(super) fn scan_name(&mut self) -> String {
        let mut name = String::new();
        let mut quoted = false;
        let mut first = true;
        while let Some((t, m)) = self.next_x_m(false) {
            let c = match m {
                Meaning::Char(cat, c) if (1..=12).contains(&cat) && u32::from(c) <= 255 => c,
                _ => {
                    self.back(t);
                    break;
                }
            };
            if c == ' ' && first {
                continue;
            }
            first = false;
            if c == '"' {
                quoted = !quoted;
                continue;
            }
            if c == ' ' && !quoted {
                break;
            }
            name.push(c);
        }
        name
    }

    /// The file a name finds, as TeX Live's kpathsea finds TeX's inputs
    /// (`name.tex` first unless the name ends so, then the name as it is):
    /// the name it was found by and its bytes. Each try is a read of
    /// `Key::File`.
    pub(super) fn find_file(&mut self, name: &str) -> Option<(String, Arc<[u8]>)> {
        let mut tries = Vec::with_capacity(2);
        if !std::path::Path::new(name).extension().is_some_and(|e| e.eq_ignore_ascii_case("tex")) {
            tries.push(format!("{name}.tex"));
        }
        tries.push(name.to_string());
        for t in tries {
            if let Val::Bytes(b) = self.get(&Key::File(Arc::from(t.as_str()))) {
                return Some((t, b));
            }
        }
        None
    }

    /// The reader goes into `text` — an `\input` file or a `\read` line —
    /// above the input pending now, its first line in state `new_line`
    /// (TeX's `begin_file_reading`, tex.web 328, 537). Its end is the
    /// reader's (`Exec::text_token`).
    fn push_source(&mut self, text: Arc<[char]>, kind: TextKind) {
        let at = self.st.pending.len();
        let x = PendingText { text, at, pos: 0, tstate: TState::N, eol: EOL_UNREAD, eof: false, kind, endinput: false };
        self.st.texts.push(x);
    }

    /// `\input name` (tex.web 537): the file is read now (`(name` in the
    /// log). At the top a node of its own, whose result is the file's text
    /// (its out state's new `RState::texts` entry, at its start); met while
    /// another command scans, a step of that command's node.
    pub(super) fn start_input(&mut self) {
        let name = self.scan_name();
        let Some((found, data)) = self.find_file(&name) else {
            self.err(format!("I can't find file `{name}'"));
            return;
        };
        let shown = self.env.file_path(&found);
        // (`term_offset` is 0: in batch mode nothing reaches the terminal)
        let lead = if shown.chars().count() > MAX_PRINT_LINE - 2 { LN } else { SP };
        self.log_raw(&format!("{RAW}{lead}({shown}"));
        self.push_source(file_text(&data), TextKind::File(shown));
    }

    /// `\endinput` (tex.web 378, `force_eof`): the file being read (or
    /// `\scantokens`'s text) ends with its current line.
    pub(super) fn end_input(&mut self) {
        match self.st.texts.last_mut() {
            Some(x) if x.kind != TextKind::Read => x.endinput = true,
            // (a file piece's, or the main file's: the piece ends with the
            // line)
            _ => self.st.endinput = true,
        }
    }

    /// `\inputlineno` (tex.web 424: `line`): in a file read in the piece,
    /// its line there; in the piece's own text, the lines of its file
    /// before the piece (`Key::LineBase`: a read the `Env` answers, so a
    /// changed line count above re-runs only this node) plus its line in
    /// the piece.
    pub(super) fn input_line_no(&mut self) -> i32 {
        if let Some(x) = self.st.texts.last().filter(|x| x.kind != TextKind::Read) {
            return line_in(&x.text, x.pos);
        }
        let base = self.int(&Key::LineBase);
        base + line_in(self.text, self.st.pos)
    }

    // ------------------------------------------------------------ reading

    /// `\openin n=name`, `\closein n` (tex.web 1275): the stream is
    /// closed, then (`\openin`) opened on the file if there is one.
    /// Definitions of `Key::ReadStream(n)`, always global.
    pub(super) fn open_in(&mut self, open: bool) {
        let n = self.scan_four_bit_int() as u8;
        let key = Key::ReadStream(n);
        if open {
            self.optional_equals();
            let name = self.scan_name();
            let v = match self.find_file(&name) {
                Some((_, data)) => Val::Stream(Arc::new(Stream { data, next: 0, normal: false })),
                None => Val::Void,
            };
            self.define(key, v, true);
        } else if matches!(self.get(&key), Val::Stream(_)) {
            self.define(key, Val::Void, true);
        }
    }

    /// pdfTeX's `\pdffilesize`: the size of the file a name finds (as
    /// `\input` finds it), if there is one.
    pub(super) fn file_size(&mut self, name: &str) -> Option<usize> {
        self.find_file(name).map(|(_, b)| b.len())
    }

    /// `\ifeof n` (tex.web 498): is the stream closed?
    pub(super) fn if_eof(&mut self) -> bool {
        let n = self.scan_four_bit_int() as u8;
        !matches!(self.get(&Key::ReadStream(n)), Val::Stream(_))
    }

    /// `\read n to \x` (tex.web 1225): `\x` is defined as the tokens of the
    /// stream's next line (and of the lines after it while braces are
    /// open), a macro without parameters.
    pub(super) fn read_to_cs(&mut self, global: bool) {
        let n = self.scan_int();
        if !self.keyword("to") {
            self.err("Missing `to' inserted".into());
        }
        // (tex.web 1215: `get_r_token`)
        let key = loop {
            let Some(t) = self.next_raw() else { return };
            if t == Tok::Char(10, ' ') {
                continue;
            }
            if let Some(k) = t.key() {
                break k;
            }
            self.err("Missing control sequence inserted".into());
            self.back(t);
            break Key::Cs(self.env.cs().id(INACCESSIBLE));
        };
        let m = if (0..=15).contains(&n) { n as u8 } else { 16 };
        let body = self.read_toks(m);
        let mac = Macro { params: Vec::new(), body: body.into(), long: false, outer: false, protected: false };
        self.define(key, Val::Meaning(Meaning::Macro(Arc::new(mac))), global);
    }

    /// e-TeX's `\readline n to \x` (etex.web §1445): like `\read`, one
    /// line, its characters (and the `\endlinechar`, if in range) made
    /// tokens of category 12, a space of category 10; braces are nothing.
    pub(super) fn read_line(&mut self, global: bool) {
        let n = self.scan_int();
        if !self.keyword("to") {
            self.err("Missing `to' inserted".into());
        }
        let key = self.r_token().unwrap_or_else(|| Key::Cs(self.env.cs().id(INACCESSIBLE)));
        let m = if (0..=15).contains(&n) { n as u8 } else { 16 };
        let stream = Key::ReadStream(m);
        let line: String = match self.get(&stream) {
            Val::Stream(s) if s.next < s.data.len() => {
                let (l, next) = next_line(&s.data, s.next);
                let l = l.iter().map(|&b| char::from(b)).collect();
                let v = Stream { data: s.data.clone(), next, normal: true };
                self.define(stream, Val::Stream(Arc::new(v)), true);
                l
            }
            Val::Stream(_) => {
                self.define(stream, Val::Void, true);
                String::new()
            }
            _ => {
                self.err("*** (cannot \\read from terminal in nonstop modes)".into());
                return;
            }
        };
        let eol = u8::try_from(self.int_par(IntPar::Endlinechar)).ok().map(char::from);
        let body = super::etex::readline_toks(&line, eol);
        let mac = Macro { params: Vec::new(), body: body.into(), long: false, outer: false, protected: false };
        self.define(key, Val::Meaning(Meaning::Macro(Arc::new(mac))), global);
    }

    /// tex.web 482–486: the tokens of stream `m`'s next line, tokenized with
    /// the category codes and `\endlinechar` in force, and of the lines
    /// after it while braces are open; the stream is defined advanced, and
    /// closed at the file's end (where the line read is empty: its end of
    /// line is a `\par`).
    fn read_toks(&mut self, m: u8) -> Vec<Tok> {
        let key = Key::ReadStream(m);
        let mut out = Vec::new();
        // (`align_state` less 1000000: the braces open)
        let mut depth: i32 = 0;
        loop {
            let line: Vec<char> = match self.get(&key) {
                Val::Stream(s) if s.next < s.data.len() => {
                    let (l, next) = next_line(&s.data, s.next);
                    let l = l.iter().map(|&b| char::from(b)).collect();
                    let v = Stream { data: s.data.clone(), next, normal: true };
                    self.define(key.clone(), Val::Stream(Arc::new(v)), true);
                    l
                }
                Val::Stream(s) => {
                    self.define(key.clone(), Val::Void, true);
                    if s.normal && depth != 0 {
                        self.err("File ended within \\read".into());
                        depth = 0;
                    }
                    Vec::new()
                }
                _ => {
                    // (§484: the terminal, which batch mode cannot read)
                    self.err("*** (cannot \\read from terminal in nonstop modes)".into());
                    return out;
                }
            };
            let mut text = line;
            text.push('\r');
            self.push_source(Arc::from(text), TextKind::Read);
            while let Some(t) = self.next_raw() {
                match t.cat() {
                    Some(1) => depth += 1,
                    Some(2) => depth -= 1,
                    _ => {}
                }
                if depth < 0 {
                    // (an unmatched `}`: the rest of the line goes)
                    while self.next_raw().is_some() {}
                    depth = 0;
                    break;
                }
                out.push(t);
            }
            self.st.texts.pop();
            if depth == 0 {
                return out;
            }
        }
    }

    // ------------------------------------------------------------ writing

    /// tex.web 1350: a `\write`'s stream: a negative number is 17 (the log
    /// only), one above 15 is 16 (the terminal and the log).
    fn write_stream(&mut self) -> u8 {
        let n = self.scan_int();
        if n < 0 {
            17
        } else if n > 15 {
            16
        } else {
            n as u8
        }
    }

    /// `\write n{…}` (tex.web 1352): a whatsit holding the tokens,
    /// unexpanded.
    pub(super) fn write_cmd(&mut self) {
        let stream = self.write_stream();
        let toks = self.scan_toks_raw();
        self.emit_here(Item::Whatsit(Arc::new(Whatsit::Write { stream, toks: Arc::new(toks) })));
    }

    /// `\openout n=name`, `\closeout n`, `\special{…}` (tex.web 1351,
    /// 1353, 1354): whatsits. A `\special`'s text is expanded now and kept
    /// as shipping prints it.
    pub(super) fn extension(&mut self, p: Prim) {
        let w = match p {
            Prim::OpenOut => {
                let stream = self.scan_four_bit_int() as u8;
                self.optional_equals();
                let name = self.scan_name();
                Whatsit::Open { stream, name: Arc::from(name) }
            }
            Prim::CloseOut => Whatsit::Close { stream: self.scan_four_bit_int() as u8 },
            _ => {
                let toks = self.scan_toks_x();
                let mut s = String::new();
                self.show_toks(&mut s, &toks);
                Whatsit::Special(Arc::from(s))
            }
        };
        self.emit_here(Item::Whatsit(Arc::new(w)));
    }

    /// `\immediate` (tex.web 1375): `\openout`, `\write`, `\closeout` are
    /// done now — an event in the node's log, which the log's gatherer
    /// replays; anything else goes back.
    pub(super) fn immediate(&mut self) {
        let Some((t, m)) = self.next_x_m(false) else { return };
        match m {
            Meaning::Prim(Prim::Write) => {
                let stream = self.write_stream();
                let toks = self.scan_toks_raw();
                let text = self.expand_write(&toks);
                self.log_raw(&format!("{RAW}{}", io_event('w', stream, &text)));
            }
            Meaning::Prim(Prim::OpenOut) => {
                let stream = self.scan_four_bit_int() as u8;
                self.optional_equals();
                let name = self.scan_name();
                self.log_raw(&format!("{RAW}{}", io_event('o', stream, &name)));
            }
            Meaning::Prim(Prim::CloseOut) => {
                let stream = self.scan_four_bit_int() as u8;
                self.log_raw(&format!("{RAW}{}", io_event('c', stream, "")));
            }
            _ => self.back(t),
        }
    }

    /// A `\write`'s text (tex.web 1370): its tokens expanded now as
    /// `\edef` expands them, in no mode, then shown as printing writes them.
    pub(super) fn expand_write(&mut self, toks: &[Tok]) -> String {
        let mut l = Vec::with_capacity(toks.len() + 1);
        l.extend_from_slice(toks);
        l.push(Tok::Char(2, '}'));
        self.rewrite(l);
        let was = self.in_write;
        self.in_write = true;
        let out = self.expand_text(None);
        self.in_write = was;
        let mut s = String::new();
        self.show_toks(&mut s, &out);
        let nl = self.int_par(IntPar::Newlinechar);
        printable(&s, nl)
    }

    /// The events of shipping box `b` here (tex.web 1374: `out_what` for
    /// each whatsit met), in order.
    pub(super) fn ship_io(&mut self, b: &BoxNode) -> String {
        let mut s = String::new();
        for w in whatsits(b) {
            match &*w {
                Whatsit::Open { stream, name } => s.push_str(&io_event('o', *stream, name)),
                Whatsit::Close { stream } => s.push_str(&io_event('c', *stream, "")),
                Whatsit::Write { stream, toks } => {
                    let text = self.expand_write(toks);
                    s.push_str(&io_event('w', *stream, &text));
                }
                Whatsit::Special(_) => {}
            }
        }
        s
    }

    // ------------------------------------------------------------ the end

    /// `\end`, `\dump` (tex.web 1054, 1335): on the main vertical list, the
    /// lines `final_cleanup` writes after the job's last page (a ` )` for
    /// each file still open, the group and each conditional still open,
    /// `\dump`'s format file) are an entry the log puts after the main
    /// file's ` )` ([`ENDJOB`]); the reading stops.
    pub(super) fn stop_job(&mut self, dump: bool) {
        if self.st.mode == Mode::V && self.st.level == 0 {
            let mut s = String::new();
            s.push(ENDJOB);
            s.push(RAW);
            if let Some(v) = &self.st.texts.0 {
                for _ in v.iter().filter(|x| matches!(x.kind, TextKind::File(_))) {
                    s.push_str(" )");
                }
            }
            let level = self.st.groups.len();
            if level > 0 {
                s.push(NL);
                s.push('(');
                self.print_esc(&mut s, "end occurred ");
                let _ = write!(s, "inside a group at level {level})");
            }
            let conds: Vec<super::Cond> = self.st.conds.0.as_ref().map(|v| v.to_vec()).unwrap_or_default();
            for c in conds.iter().rev() {
                s.push(NL);
                s.push('(');
                self.print_esc(&mut s, "end occurred ");
                s.push_str("when ");
                if c.code >= 32 {
                    self.print_esc(&mut s, "unless");
                }
                self.print_esc(&mut s, if_name(c.code));
                let line = self.cond_line(c.open);
                if line != 0 {
                    let _ = write!(s, " on line {line}");
                }
                s.push_str(" was incomplete)");
            }
            if dump {
                if level > 0 {
                    self.err("You can't dump inside a group".into());
                } else {
                    // (tex.web 1328: the format's identification)
                    let job = match self.get(&Key::Job) {
                        Val::Name(n) => n,
                        _ => Arc::from(super::expand::JOBNAME),
                    };
                    let (y, mo, d) =
                        (self.int_par(IntPar::Year), self.int_par(IntPar::Month), self.int_par(IntPar::Day));
                    s.push(NL);
                    let _ = write!(s, "Beginning to dump on file {job}.fmt");
                    s.push(NL);
                    let _ = write!(s, " (preloaded format={job} {y}.{mo}.{d})");
                }
            }
            if s.chars().count() > 2 {
                self.log_push(&s);
            }
            self.st.groups = Stack::new();
            self.st.conds = Stack::new();
            self.st.texts = Stack::new();
        }
        self.stop();
    }

    /// The line of the conditional node `open` began (its line in the
    /// piece's file: `Key::LineBase` and the line in the piece).
    fn cond_line(&mut self, open: u32) -> i32 {
        let i = (open as usize).min(self.chain.nodes.len());
        let t = self.text;
        let mut q = self.chain.input_pos(i).min(t.len());
        while q < t.len() && (t[q] == ' ' || t[q] == '\r') {
            q += 1;
        }
        let base = self.int(&Key::LineBase);
        base + 1 + t[..q].iter().filter(|&&c| c == '\r').count() as i32
    }
}
