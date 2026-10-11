//! D2: `\openin` `\read` `\readline` `\closein` `\ifeof` data, `\openout`
//! `\write` `\closeout` `\immediate`, `\message` `\errmessage`, `\special`.
//!
//! Streams are values: `Key::ReadStream(n)` holds `Val::Stream` (open) or
//! `Val::Void` (closed); reading a line defines the stream again with its
//! position moved (global: TeX's read files are not subject to grouping).
//! Write streams: `Key::WriteOpen(n)` is `Val::Name(file)` while open (the
//! loop-back driver collects the text written to it), `Val::Void` closed.
//!
//! `Emit::Write { stream, text }` streams: 0..=15 a file (no trailing newline:
//! the collector adds one), `TERM_AND_LOG` (16), `LOG_ONLY` (17), and
//! `MESSAGE` (18: `\message` / page numbers, spaced like `print`, no newline).
use std::sync::Arc;

use crate::ctx::Ctx;
use crate::item::{Item, Whatsit};
use crate::prim::{ip, tp, Kind, Registry};
use crate::term::Emit;
use crate::tok::{Tok, Toks};
use crate::val::{Key, Macro, Meaning, Val};

pub const TERM_AND_LOG: u8 = 16;
pub const LOG_ONLY: u8 = 17;
pub const MESSAGE: u8 = 18;

/// `long_help_seen` of `\errmessage` (global).
const LONG_HELP_SEEN: Key = Key::Other(crate::prim::tag::IO, 0);

pub fn register(r: &mut Registry) {
    r.add("openin", 0, Kind::Command(open_in), 0);
    r.add("closein", 0, Kind::Command(close_in), 0);
    r.add("read", 0, Kind::Command(read_cmd), 0);
    r.add("readline", 1, Kind::Command(read_cmd), 1);
    r.add("openout", 0, Kind::Command(open_out_cmd), 0);
    r.add("write", 0, Kind::Command(write_cmd), 0);
    r.add("closeout", 0, Kind::Command(close_out_cmd), 0);
    r.add("special", 0, Kind::Command(special_cmd), 0);
    r.add("immediate", 0, Kind::Command(immediate_cmd), 0);
    r.add("message", 0, Kind::Command(message_cmd), 0);
    r.add("errmessage", 1, Kind::Command(message_cmd), 0);
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stream { pub name: Arc<str>, pub data: Arc<[u8]>, pub next: usize }

// ---------------------------------------------------------------- reading

/// `\ifeof n` (for `cond.rs`): true unless stream n is open.
pub fn if_eof(c: &mut dyn Ctx, n: i32) -> bool {
    if !(0..=15).contains(&n) {
        return true;
    }
    !matches!(c.get(&Key::ReadStream(n as u8)), Val::Stream(_))
}

fn open_in(c: &mut dyn Ctx, _code: u16) {
    let n = crate::scan::scan_four_bit_int(c);
    crate::scan::scan_optional_equals(c);
    let name = crate::scan::scan_file_name(c);
    // close first (tex.web 1275)
    c.define(Key::ReadStream(n), Val::Void, true);
    if name.is_empty() {
        return;
    }
    let base = name.rsplit('/').next().unwrap_or(&name).to_string();
    let mut found = None;
    if !base.contains('.') {
        found = c.file(&format!("{name}.tex"));
    }
    if found.is_none() {
        found = c.file(&name);
    }
    if let Some((full, data)) = found {
        c.define(Key::ReadStream(n), Val::Stream(Arc::new(Stream { name: full, data, next: 0 })), true);
    }
}

fn close_in(c: &mut dyn Ctx, _code: u16) {
    let n = crate::scan::scan_four_bit_int(c);
    c.define(Key::ReadStream(n), Val::Void, true);
}

fn read_cmd(c: &mut dyn Ctx, code: u16) {
    read_to_cs(c, code, false);
}

/// `\read n to \cs` (code 0) / `\readline` (code 1), with the `\global`
/// prefix given (for `assign::prefixed_command`).
pub fn read_to_cs(c: &mut dyn Ctx, code: u16, global: bool) {
    let n = crate::scan::scan_int(c);
    if !crate::scan::scan_keyword(c, "to") {
        crate::diag::error(
            c,
            "Missing `to' inserted",
            &["You should have said `\\read<number> to \\cs'.", "I'm going to look for the \\cs now."],
        );
    }
    let r = crate::assign::get_r_token(c);
    let key = match r {
        Tok::Active(ch) => Key::Active(ch),
        Tok::Cs(cs) => Key::Cs(cs),
        _ => return,
    };
    let body = read_toks(c, n, code == 1);
    let m = Macro {
        params: Arc::from(Vec::<Tok>::new()),
        body,
        brace_end: false,
        long: false,
        outer: false,
        protected: false,
    };
    crate::assign::assign(c, key, Val::Meaning(Meaning::Macro(Arc::new(m))), global);
}

/// Takes the next line of stream `n`: `None` at its end (the stream is then closed).
fn next_line(c: &mut dyn Ctx, n: u8) -> Option<Vec<char>> {
    let s = match c.get(&Key::ReadStream(n)) {
        Val::Stream(s) => s,
        _ => return None,
    };
    if s.next >= s.data.len() {
        c.define(Key::ReadStream(n), Val::Void, true);
        return None;
    }
    let rest = &s.data[s.next..];
    let (line, adv) = match rest.iter().position(|&b| b == b'\n') {
        Some(i) => (&rest[..i], i + 1),
        None => (rest, rest.len()),
    };
    let mut end = line.len();
    if end > 0 && line[end - 1] == b'\r' {
        end -= 1;
    }
    while end > 0 && line[end - 1] == b' ' {
        end -= 1;
    }
    let chars: Vec<char> = match std::str::from_utf8(&line[..end]) {
        Ok(t) => t.chars().collect(),
        Err(_) => line[..end].iter().map(|&b| b as char).collect(),
    };
    let next = Stream { name: s.name.clone(), data: s.data.clone(), next: s.next + adv };
    c.define(Key::ReadStream(n), Val::Stream(Arc::new(next)), true);
    Some(chars)
}

/// `read_toks` (tex.web 482–486): lines until braces balance; at the file's
/// end an empty line (so `\par` with the usual `\endlinechar`) and the stream closes.
pub fn read_toks(c: &mut dyn Ctx, n: i32, readline: bool) -> Toks {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let open = (0..=15).contains(&n) && matches!(c.get(&Key::ReadStream(n as u8)), Val::Stream(_));
    if !open {
        crate::diag::error(c, "*** (cannot \\read from terminal in nonstop modes)", &[]);
        c.define(Key::Ended, Val::Int(1), true);
        return Arc::from(out);
    }
    loop {
        let mut line = match next_line(c, n as u8) {
            Some(l) => l,
            None => {
                if depth != 0 {
                    crate::diag::error(c, "File ended within \\read", &["This \\read has unbalanced braces."]);
                    depth = 0;
                }
                Vec::new()
            }
        };
        let elc = c.int(&Key::IntPar(ip::of("endlinechar")));
        if (0..=255).contains(&elc) {
            line.push(char::from_u32(elc as u32).unwrap_or('\r'));
        }
        if readline {
            for ch in line {
                out.push(Tok::Char { cat: if ch == ' ' { 10 } else { 12 }, ch });
            }
            break;
        }
        tokenize_line(c, line, &mut out, &mut depth);
        if depth == 0 {
            break;
        }
        if !matches!(c.get(&Key::ReadStream(n as u8)), Val::Stream(_)) {
            // closed while unbalanced: the next round reports and ends.
            continue;
        }
    }
    Arc::from(out)
}

fn cat(c: &mut dyn Ctx, ch: char) -> u8 {
    c.get(&Key::Cat(ch)).int() as u8
}

/// `^^` reduction at `i` (tex.web 352/355): rewrites the buffer in place.
fn reduce_hat(c: &mut dyn Ctx, line: &mut Vec<char>, i: usize) -> bool {
    if i + 2 >= line.len() {
        return false;
    }
    let ch = line[i];
    if line[i + 1] != ch || cat(c, ch) != 7 {
        return false;
    }
    let c3 = line[i + 2];
    let hex = |x: char| x.is_ascii_digit() || ('a'..='f').contains(&x);
    if i + 3 < line.len() && hex(c3) && hex(line[i + 3]) {
        let v = c3.to_digit(16).unwrap_or(0) * 16 + line[i + 3].to_digit(16).unwrap_or(0);
        line.splice(i..i + 4, [char::from_u32(v).unwrap_or('?')]);
        return true;
    }
    if (c3 as u32) < 128 {
        let v = c3 as u32;
        let r = if v < 64 { v + 64 } else { v - 64 };
        line.splice(i..i + 3, [char::from_u32(r).unwrap_or('?')]);
        return true;
    }
    false
}

#[derive(Clone, Copy, PartialEq)]
enum St {
    N,
    M,
    S,
}

/// Tokenizes one line (its end-of-line char already appended) with the
/// catcodes in force (tex.web 343–357).
fn tokenize_line(c: &mut dyn Ctx, mut line: Vec<char>, out: &mut Vec<Tok>, depth: &mut i32) {
    let mut st = St::N;
    let mut i = 0;
    while i < line.len() {
        let ch = line[i];
        let cc = cat(c, ch);
        if cc == 7 && reduce_hat(c, &mut line, i) {
            continue;
        }
        i += 1;
        match cc {
            0 => {
                if i >= line.len() {
                    out.push(Tok::Cs(c.names().intern("")));
                    st = St::M;
                    continue;
                }
                while cat(c, line[i]) == 7 && reduce_hat(c, &mut line, i) {}
                let ch2 = line[i];
                if cat(c, ch2) == 11 {
                    let mut j = i;
                    while j < line.len() {
                        let k = cat(c, line[j]);
                        if k == 7 && reduce_hat(c, &mut line, j) {
                            continue;
                        }
                        if k != 11 {
                            break;
                        }
                        j += 1;
                    }
                    let name: String = line[i..j].iter().collect();
                    i = j;
                    out.push(Tok::Cs(c.names().intern(&name)));
                    st = St::S;
                } else {
                    i += 1;
                    out.push(Tok::Cs(c.names().intern(&ch2.to_string())));
                    st = if cat(c, ch2) == 10 { St::S } else { St::M };
                }
            }
            5 => {
                match st {
                    St::N => out.push(Tok::Cs(c.names().intern("par"))),
                    St::M => out.push(Tok::Char { cat: 10, ch: ' ' }),
                    St::S => {}
                }
                break;
            }
            9 => {}
            10 => {
                if st == St::M {
                    out.push(Tok::Char { cat: 10, ch: ' ' });
                    st = St::S;
                }
            }
            14 => break,
            15 => {
                crate::diag::error(
                    c,
                    "Text line contains an invalid character",
                    &["A funny symbol that I can't read has just been input.", "Continue, and I'll forget that it ever happened."],
                );
            }
            13 => {
                out.push(Tok::Active(ch));
                st = St::M;
            }
            1 => {
                *depth += 1;
                out.push(Tok::Char { cat: 1, ch });
                st = St::M;
            }
            2 => {
                *depth -= 1;
                out.push(Tok::Char { cat: 2, ch });
                st = St::M;
            }
            _ => {
                out.push(Tok::Char { cat: cc, ch });
                st = St::M;
            }
        }
    }
}

// ---------------------------------------------------------------- writing

/// Splits a file name into (area, name, ext).
pub fn split_name(f: &str) -> (Arc<str>, Arc<str>, Arc<str>) {
    let (area, rest) = match f.rfind('/') {
        Some(i) => (&f[..=i], &f[i + 1..]),
        None => ("", f),
    };
    let (name, ext) = match rest.rfind('.') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    (area.into(), name.into(), ext.into())
}

/// The file an `Open` whatsit opens (`.tex` when no extension, tex.web 1374).
pub fn open_name(area: &str, name: &str, ext: &str) -> Arc<str> {
    let ext = if ext.is_empty() { ".tex" } else { ext };
    Arc::from(format!("{area}{name}{ext}"))
}

/// `\write`/`\closeout`'s stream number (tex.web 1350): <0 → 17, >15 → 16.
fn write_stream(c: &mut dyn Ctx) -> u8 {
    let n = crate::scan::scan_int(c);
    if n < 0 {
        LOG_ONLY
    } else if n > 15 {
        TERM_AND_LOG
    } else {
        n as u8
    }
}

fn open_out_cmd(c: &mut dyn Ctx, _code: u16) {
    open_out(c, false);
}
fn write_cmd(c: &mut dyn Ctx, _code: u16) {
    write(c, false);
}
fn close_out_cmd(c: &mut dyn Ctx, _code: u16) {
    close_out(c, false);
}

pub fn open_out(c: &mut dyn Ctx, immediate: bool) {
    let n = crate::scan::scan_four_bit_int(c);
    crate::scan::scan_optional_equals(c);
    let f = crate::scan::scan_file_name(c);
    let (area, name, ext) = split_name(&f);
    let w = Whatsit::Open { stream: n, name, area, ext };
    if immediate {
        out_what(c, &w, &mut OpenState::default());
    } else {
        c.item(Item::Whatsit(Arc::new(w)));
    }
}

pub fn write(c: &mut dyn Ctx, immediate: bool) {
    let stream = write_stream(c);
    let (_, toks) = crate::scan::scan_toks(c, false, false);
    let w = Whatsit::Write { stream, toks };
    if immediate {
        out_what(c, &w, &mut OpenState::default());
    } else {
        c.item(Item::Whatsit(Arc::new(w)));
    }
}

pub fn close_out(c: &mut dyn Ctx, immediate: bool) {
    let stream = write_stream(c);
    let w = Whatsit::Close { stream };
    if immediate {
        out_what(c, &w, &mut OpenState::default());
    } else {
        c.item(Item::Whatsit(Arc::new(w)));
    }
}

/// Write-stream states changed earlier within the same node (shipout walks
/// several whatsits in one node and does not read its own defines back).
#[derive(Default)]
pub struct OpenState(pub std::collections::BTreeMap<u8, Option<Arc<str>>>);

impl OpenState {
    fn get(&self, c: &mut dyn Ctx, n: u8) -> Option<Arc<str>> {
        if let Some(v) = self.0.get(&n) {
            return v.clone();
        }
        match c.get(&Key::WriteOpen(n)) {
            Val::Name(f) => Some(f),
            _ => None,
        }
    }
}

/// `out_what` / `write_out` (tex.web 1370–1374): performs an open, write or
/// close whatsit now (at `\immediate` or at shipout). `\write` text is
/// expanded in the env of the performing node.
pub fn out_what(c: &mut dyn Ctx, w: &Whatsit, st: &mut OpenState) {
    match w {
        Whatsit::Open { stream, name, area, ext } => {
            let j = *stream;
            if j >= 16 {
                return;
            }
            let full = open_name(area, name, ext);
            c.define(Key::WriteOpen(j), Val::Name(full.clone()), true);
            st.0.insert(j, Some(full.clone()));
            c.emit(Emit::Write { stream: LOG_ONLY, text: Arc::from(format!("\\openout{j} = `{full}'.\n")) });
        }
        Whatsit::Close { stream } => {
            let j = *stream;
            if j < 16 && st.get(c, j).is_some() {
                c.define(Key::WriteOpen(j), Val::Void, true);
                st.0.insert(j, None);
            }
        }
        Whatsit::Write { stream, toks } => {
            let x = crate::scan::expand_toks(c, toks);
            let text = toks_to_string(c, &x);
            let j = *stream;
            let s = if j < 16 && st.get(c, j).is_some() {
                j
            } else if j == LOG_ONLY {
                LOG_ONLY
            } else {
                TERM_AND_LOG
            };
            c.emit(Emit::Write { stream: s, text: Arc::from(text) });
        }
        _ => {}
    }
}

/// `\special{…}`: expanded at once (tex.web 1354); the back end prints it.
fn special_cmd(c: &mut dyn Ctx, _code: u16) {
    let (_, toks) = crate::scan::scan_toks(c, false, true);
    c.item(Item::Whatsit(Arc::new(Whatsit::Special { toks })));
}

/// `\immediate` (tex.web 1375).
fn immediate_cmd(c: &mut dyn Ctx, _code: u16) {
    let Some(t) = c.next_x() else { return };
    if let Meaning::Prim(id) = c.meaning(&t) {
        match crate::prim::registry().get(id).name {
            "openout" => return open_out(c, true),
            "write" => return write(c, true),
            "closeout" => return close_out(c, true),
            _ => {
                if crate::pdfprim::immediate(c, id) {
                    return;
                }
            }
        }
    }
    c.back(t);
}

/// `\message` (0) / `\errmessage` (1) (tex.web 1279–1283).
fn message_cmd(c: &mut dyn Ctx, code: u16) {
    let (_, toks) = crate::scan::scan_toks(c, false, true);
    let s = toks_to_string(c, &toks);
    if code == 0 {
        c.emit(Emit::Write { stream: MESSAGE, text: Arc::from(s) });
        return;
    }
    let errhelp = c.get(&Key::TokPar(tp::of("errhelp"))).toks();
    if !errhelp.is_empty() {
        let h = toks_to_string(c, &errhelp);
        let lines: Vec<&str> = h.split('\n').collect();
        crate::diag::error(c, &s, &lines);
    } else if c.int(&LONG_HELP_SEEN) != 0 {
        crate::diag::error(c, &s, &["(That was another \\errmessage.)"]);
    } else {
        // interaction < error_stop_mode (3)
        if c.int(&Key::Interaction) < 3 {
            c.define(LONG_HELP_SEEN, Val::Int(1), true);
        }
        crate::diag::error(
            c,
            &s,
            &[
                "This error message was generated by an \\errmessage",
                "command, so I can't give any explicit help.",
                "Pretend that you're Hercule Poirot: Examine all clues,",
                "and deduce the truth by order and method.",
            ],
        );
    }
}

// ---------------------------------------------------------------- printing

/// Prints one character as `print` does: `\newlinechar` → newline,
/// unprintables in `^^` form.
pub fn put_char(out: &mut String, ch: char, newline: i32) {
    let v = ch as u32;
    if v as i32 == newline {
        out.push('\n');
    } else if v < 32 {
        out.push_str("^^");
        out.push(char::from_u32(v + 64).unwrap_or('?'));
    } else if v == 127 {
        out.push_str("^^?");
    } else {
        out.push(ch);
    }
}

/// `show_token_list` of a whole list (no limit), as `\write`, `\message`,
/// `\special` print it: escapechar, a space after control words, `##`.
pub fn toks_to_string(c: &mut dyn Ctx, toks: &[Tok]) -> String {
    let esc = c.int(&Key::IntPar(ip::of("escapechar")));
    let nl = c.int(&Key::IntPar(ip::of("newlinechar")));
    let mut out = String::new();
    let put_esc = |out: &mut String| {
        if (0..=255).contains(&esc) {
            put_char(out, char::from_u32(esc as u32).unwrap_or('\\'), nl);
        }
    };
    for t in toks {
        match *t {
            Tok::Char { cat, ch } => {
                put_char(&mut out, ch, nl);
                if cat == 6 {
                    put_char(&mut out, ch, nl);
                }
            }
            Tok::Active(ch) => put_char(&mut out, ch, nl),
            Tok::Param(n) => {
                out.push('#');
                if n > 0 {
                    out.push(char::from(b'0' + n));
                }
            }
            Tok::DontExpand => {}
            Tok::Cs(cs) => {
                let name = c.names().name(cs);
                let mut chs = name.chars();
                match (chs.next(), chs.next()) {
                    (None, _) => {
                        put_esc(&mut out);
                        out.push_str("csname");
                        put_esc(&mut out);
                        out.push_str("endcsname ");
                    }
                    (Some(one), None) => {
                        put_esc(&mut out);
                        put_char(&mut out, one, nl);
                        if cat(c, one) == 11 {
                            out.push(' ');
                        }
                    }
                    _ => {
                        put_esc(&mut out);
                        for ch in name.chars() {
                            put_char(&mut out, ch, nl);
                        }
                        out.push(' ');
                    }
                }
            }
        }
    }
    out
}
