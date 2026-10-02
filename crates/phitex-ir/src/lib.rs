//! The SSA: values, each defined once.
//!
//! Everything is a value: primitives, catcode tables, macros, integers,
//! token lists, paragraphs. A value is either a constant or the result
//! of applying a value to operands:
//!
//! ```text
//! %0 = prim def
//! %2 = %0(%1, \name, "World")        ; macro "World"
//! %5 = %3(%2:"World")
//! ```
//!
//! An operand names a value directly (`%2`), or is literal source text.
//! There are no names in the program: a control sequence in an operand is
//! only the text the builder read.
//!
//! A conditional's arms meet in φ nodes: after `\fi`, a name the taken arm
//! changed means `φ(test, then, else)`. One side is what the taken arm
//! made; the other is the arm not taken (its unread text, a value of its
//! own) or, if that arm was empty, the value from before the `\if`:
//!
//! ```text
//! %7 = %6(%0, %5, "<", "10")         ; true
//! %9 = %2(%0, %5, "1")               ; \count1 = 3
//! %10 = %7(%0, "\def\x{a}")          ; not taken
//! %11 = φ(%7, %9, %5)                ; \count1 (then)
//! ```
//!
//! A program may be partial: where the builder stopped (its steps ran out),
//! the rest is a *pending* node, the text not read yet and the environment
//! to read it in. An environment is a value too: what each name the
//! document bound means there (a name not listed means what it means
//! before the document), one per group open, each inner one on the one
//! around it:
//!
//! ```text
//! %4 = env(\count1=%2, \loop=%3)
//! %5 = env(%4; catcodes=%1)           ; env, group 1
//! %6 = pending(%5; "ab"; %3:"\loop", "␤␤more")   ; pending, 1 group open
//! ```
//!
//! Reading the pending text in its environment goes on where the builder
//! stopped (the text before the second `;` is the paragraph it had begun).
//! The source in it is as it is, but for its line ends, written `␤`.
//!
//! A value can also be an operation of its own, named, that defines
//! names: `op(operands; names)`. A build's window (partex's view of a
//! build, `partex_core::ssa::view`) is one: its operands are what it
//! imports, each a name and the value that defined it (`name=%n`, a
//! constant if no value before it did), and its names are what it
//! exports, the names it defines:
//!
//! ```text
//! %0 = format plain
//! %3 = window(\count1=%2, \loop=%0; \count1, \body) ; step 3
//! ```
//!
//! A name is written as it is unless it is empty or has a character that
//! would end it (a space, `,`, `;`, `(`, `)`, `=`, `%` or `"`, or a
//! leading `'`): then it is quoted, as a literal is.

#![no_std]

extern crate alloc;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use core::fmt::Write as _;

/// A value's name, `%n`: its index in [`Program::values`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ValueId(pub u32);

impl core::fmt::Display for ValueId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "%{}", self.0)
    }
}

/// An operand.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Operand {
    /// A value.
    Value(ValueId),
    /// Tokens that came from value `.0`, with their text: a use of the
    /// value (the text is for the reader).
    From(ValueId, String),
    /// Source text, read by the node itself.
    Source(String),
    /// What name `.0` means as value `.1` left it: an import by name
    /// (`\count1=%5`). If `.1` is an operation ([`Def::Op`]), the name
    /// is one it defines.
    Named(String, ValueId),
}

/// How a value is defined.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Def {
    /// A constant: what exists before the document (a primitive, the
    /// starting catcodes, a register's starting zero).
    Const(String),
    /// `callee(operands)`.
    Apply {
        callee: ValueId,
        operands: Vec<Operand>,
    },
    /// `φ(cond, then, else)`: `then` if the test `cond` is true, else
    /// `else`.
    Phi {
        cond: ValueId,
        then: ValueId,
        els: ValueId,
    },
    /// `μ(init, next)`: a loop-carried value (a label from the `.aux`, a
    /// page from the page builder, the `.bbl`). In the first pass it is
    /// `init` (nothing, or the last build's value); in each pass after, it
    /// is what the pass before made, `next`: a value later in the program
    /// (the one use that may point forward). Converged, `next` is itself.
    Mu { init: ValueId, next: ValueId },
    /// An environment: the one around it (`base`, for a group), and what
    /// names mean.
    Env {
        base: Option<ValueId>,
        binds: Vec<(String, ValueId)>,
    },
    /// Text not read yet, to read in `env`: the paragraph begun (`par`),
    /// and the rest.
    Pending {
        env: ValueId,
        par: Vec<Operand>,
        text: Vec<Operand>,
    },
    /// `op(operands; defines)`: an operation of its own (`op`, a word of
    /// lowercase letters and `_`) over its operands, defining the names
    /// `defines` (a build's window: its imports, then its exports).
    Op {
        op: String,
        operands: Vec<Operand>,
        defines: Vec<String>,
    },
}

fn operand_uses(o: &Operand) -> Option<ValueId> {
    match o {
        Operand::Value(u) | Operand::From(u, _) | Operand::Named(_, u) => Some(*u),
        Operand::Source(_) => None,
    }
}

impl Def {
    /// Its operands (an application's, an operation's, the text a pending
    /// node holds).
    pub fn operands(&self) -> impl Iterator<Item = &Operand> {
        let (a, b): (&[Operand], &[Operand]) = match self {
            Def::Apply { operands, .. } | Def::Op { operands, .. } => (operands, &[]),
            Def::Pending { par, text, .. } => (par, text),
            _ => (&[], &[]),
        };
        a.iter().chain(b)
    }

    /// The values this one uses.
    #[must_use]
    pub fn uses(&self) -> Vec<ValueId> {
        match self {
            Def::Const(_) => Vec::new(),
            Def::Apply { callee, operands } => core::iter::once(*callee)
                .chain(operands.iter().filter_map(operand_uses))
                .collect(),
            Def::Op { operands, .. } => operands.iter().filter_map(operand_uses).collect(),
            Def::Phi { cond, then, els } => vec![*cond, *then, *els],
            Def::Mu { init, next } => vec![*init, *next],
            Def::Env { base, binds } => base
                .iter()
                .copied()
                .chain(binds.iter().map(|b| b.1))
                .collect(),
            Def::Pending { env, par, text } => core::iter::once(*env)
                .chain(par.iter().chain(text).filter_map(operand_uses))
                .collect(),
        }
    }
}

/// One value: its definition, what it evaluated to (for the reader) and
/// the source line it was defined on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Value {
    pub def: Def,
    pub shows: String,
    pub line: usize,
}

/// A program: its values in definition order, and the source (for the
/// line headings of the text form).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Program {
    pub values: Vec<Value>,
    pub source: String,
}

impl Program {
    /// A new value.
    ///
    /// # Panics
    ///
    /// With 2^32 values.
    pub fn define(&mut self, def: Def, shows: String, line: usize) -> ValueId {
        let id = ValueId(u32::try_from(self.values.len()).expect("fewer than 2^32 values"));
        self.values.push(Value { def, shows, line });
        id
    }

    /// Whether the program is SSA: every operand and callee is a value
    /// defined before its use; and whether its names are names: none
    /// empty or with a line end, and each one imported from an operation
    /// one that the operation defines.
    pub fn check(&self) -> Result<(), String> {
        // (each operation's names, sorted: what an import from it finds)
        let mut defined: Vec<Vec<&str>> = Vec::new();
        for (i, v) in self.values.iter().enumerate() {
            let uses = match &v.def {
                // (μ's `next` is the back edge: anywhere in the program)
                Def::Mu { init, next } => {
                    if next.0 as usize >= self.values.len() {
                        return Err(format!("%{i}: μ's next {next} is not a value"));
                    }
                    vec![*init]
                }
                d => d.uses(),
            };
            for u in uses {
                if u.0 as usize >= i {
                    return Err(format!("%{i} uses {u}, defined at or after it"));
                }
            }
            for o in v.def.operands() {
                let Operand::Named(n, u) = o else { continue };
                if let Some(e) = bad_name(n) {
                    return Err(format!("%{i}: the name {n:?} {e}"));
                }
                let op = matches!(self.values[u.0 as usize].def, Def::Op { .. });
                if op && defined[u.0 as usize].binary_search(&n.as_str()).is_err() {
                    return Err(format!(
                        "%{i} imports {n} from {u}, which does not define it"
                    ));
                }
            }
            let mut names: Vec<&str> = Vec::new();
            if let Def::Op { op, defines, .. } = &v.def {
                if !is_op(op) {
                    return Err(format!("%{i}: `{op}` is not an operation's name"));
                }
                if let Some((n, e)) = defines.iter().find_map(|n| Some((n, bad_name(n)?))) {
                    return Err(format!("%{i}: the name {n:?} {e}"));
                }
                names = defines.iter().map(String::as_str).collect();
                names.sort_unstable();
            }
            defined.push(names);
        }
        Ok(())
    }

    /// The text form: each value on a line, `%n = …`, under a heading
    /// with the source line it was defined on.
    #[must_use]
    pub fn to_text(&self) -> String {
        let lines: Vec<&str> = self.source.lines().collect();
        let mut out = String::new();
        let mut at = 0;
        for (i, v) in self.values.iter().enumerate() {
            if v.line != at {
                at = v.line;
                let text = lines.get(at.wrapping_sub(1)).copied().unwrap_or("");
                let _ = writeln!(out, "\n; l.{at}  {text}");
            }
            let rhs = match &v.def {
                Def::Const(c) => c.clone(),
                Def::Apply { callee, operands } => {
                    let ops: Vec<String> = operands.iter().map(operand_text).collect();
                    format!("{callee}({})", ops.join(", "))
                }
                Def::Phi { cond, then, els } => format!("φ({cond}, {then}, {els})"),
                Def::Mu { init, next } => format!("μ({init}, {next})"),
                Def::Env { base, binds } => {
                    let binds: Vec<String> =
                        binds.iter().map(|(n, v)| format!("{n}={v}")).collect();
                    match base {
                        Some(b) => format!("env({b}; {})", binds.join(", ")),
                        None => format!("env({})", binds.join(", ")),
                    }
                }
                Def::Pending { env, par, text } => {
                    let ops = |os: &[Operand]| -> String {
                        os.iter().map(operand_text).collect::<Vec<_>>().join(", ")
                    };
                    format!("pending({env}; {}; {})", ops(par), ops(text))
                }
                Def::Op {
                    op,
                    operands,
                    defines,
                } => {
                    let ops: Vec<String> = operands.iter().map(operand_text).collect();
                    let names: Vec<String> = defines.iter().map(|n| name_text(n)).collect();
                    format!("{op}({}; {})", ops.join(", "), names.join(", "))
                }
            };
            let lhs = format!("%{i} = {rhs}");
            if v.shows.is_empty() {
                let _ = writeln!(out, "{lhs}");
            } else {
                let _ = writeln!(out, "{lhs:<44} ; {}", v.shows);
            }
        }
        out
    }
}

fn operand_text(o: &Operand) -> String {
    match o {
        Operand::Value(v) => v.to_string(),
        Operand::From(v, text) => format!("{v}:{}", quote(text)),
        Operand::Source(text) => text.clone(),
        Operand::Named(name, v) => format!("{}={v}", name_text(name)),
    }
}

/// Whether `op` names an operation ([`Def::Op`]): a word of lowercase
/// letters and `_`, not `env` or `pending`.
fn is_op(op: &str) -> bool {
    !op.is_empty()
        && op.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
        && op != "env"
        && op != "pending"
}

/// Why `n` is not a name, if it is not: empty, or with a line end.
fn bad_name(n: &str) -> Option<&'static str> {
    if n.is_empty() {
        Some("is empty")
    } else if n.contains('\n') {
        Some("has a line end")
    } else {
        None
    }
}

/// Whether `c` ends a name written as it is.
fn ends_name(c: char) -> bool {
    c.is_whitespace() || matches!(c, ',' | ';' | '(' | ')' | '=' | '%' | '"' | '␤')
}

/// Whether name `n` is written as it is: nothing in it ends a name, and it
/// does not begin as a character literal does.
fn bare(n: &str) -> bool {
    !n.is_empty() && !n.starts_with('\'') && !n.chars().any(ends_name)
}

/// A name in the text form: as it is, or quoted ([`bare`]).
#[must_use]
pub fn name_text(n: &str) -> String {
    if bare(n) { n.to_string() } else { quote(n) }
}

/// A name at the start of `s` ([`name_text`]'s inverse): its text and
/// the rest of `s`. A bare name ends where a character that cannot be in
/// one is.
fn name(s: &str) -> Option<(String, &str)> {
    if s.starts_with('"') {
        return unquote(s);
    }
    let end = s.find(ends_name).unwrap_or(s.len());
    bare(&s[..end]).then(|| (s[..end].to_string(), &s[end..]))
}

/// Text as a literal: in double quotes, a `"` inside written `""`, a line
/// end `␤` (backslashes as they are, so TeX reads as TeX).
#[must_use]
pub fn quote(t: &str) -> String {
    format!("\"{}\"", t.replace('"', "\"\"").replace('\n', "␤"))
}

/// A literal at the start of `s`: its text and the rest of `s`.
fn unquote(s: &str) -> Option<(String, &str)> {
    let mut rest = s.strip_prefix('"')?;
    let mut out = String::new();
    loop {
        let i = rest.find('"')?;
        out.push_str(&rest[..i]);
        rest = &rest[i + 1..];
        match rest.strip_prefix('"') {
            Some(r) => {
                out.push('"');
                rest = r;
            }
            None => return Some((out, rest)),
        }
    }
}

/// `%n` at the start of `s`: its id and the rest.
fn value_id(s: &str) -> Option<(ValueId, &str)> {
    let s = s.strip_prefix('%')?;
    let end = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    let n = s[..end].parse().ok()?;
    Some((ValueId(n), &s[end..]))
}

/// One operand at the start of `s`, in a list that `end` ends (`)`, or
/// `;` for the first of two).
fn operand(s: &str, end: char) -> Option<(Operand, &str)> {
    if let Some((v, rest)) = value_id(s) {
        return Some(match rest.strip_prefix(':') {
            Some(r) => {
                let (text, rest) = unquote(r)?;
                (Operand::From(v, text), rest)
            }
            None => (Operand::Value(v), rest),
        });
    }
    // (a name, then the value it is imported from)
    if let Some((n, rest)) = name(s)
        && let Some((v, rest)) = rest.strip_prefix('=').and_then(value_id)
        && (rest.is_empty() || rest.starts_with(", ") || rest.starts_with(end))
    {
        return Some((Operand::Named(n, v), rest));
    }
    if s.starts_with('"') {
        let (text, rest) = unquote(s)?;
        return Some((Operand::Source(quote(&text)), rest));
    }
    if let Some(r) = s.strip_prefix('\'')
        && let Some(i) = r.get(1..).and_then(|t| t.find('\''))
    {
        let end = i + 2;
        return Some((Operand::Source(s[..=end].to_string()), &s[end + 1..]));
    }
    let end = [
        s.find(", "),
        s.rfind(')'),
        (end == ';').then(|| s.find("; ")).flatten(),
    ]
    .into_iter()
    .flatten()
    .min()
    .unwrap_or(s.len());
    Some((Operand::Source(s[..end].to_string()), &s[end..]))
}

/// An operation `op`'s operands and names, `s` from after its `(`: the
/// value and the rest after its `)`.
fn op_rest(op: String, s: &str) -> Option<(Def, &str)> {
    let (operands, r) = operands_until(s, ';')?;
    let mut r = r.strip_prefix(' ').unwrap_or(r);
    let mut defines = Vec::new();
    while !r.starts_with(')') {
        let (n, next) = name(r)?;
        defines.push(n);
        r = next.strip_prefix(", ").unwrap_or(next);
    }
    let def = Def::Op {
        op,
        operands,
        defines,
    };
    Some((def, &r[1..]))
}

/// An operation's name and its `(` at the start of `s`: the name and the
/// rest after the `(`.
fn op_head(s: &str) -> Option<(String, &str)> {
    let end = s.find(|c: char| !(c.is_ascii_lowercase() || c == '_'))?;
    let r = s[end..].strip_prefix('(')?;
    is_op(&s[..end]).then(|| (s[..end].to_string(), r))
}

/// `name=%n` at the start of `s`: the name is up to the first `=%`
/// followed by digits and then `, ` or `)`.
fn bind(s: &str) -> Option<((String, ValueId), &str)> {
    let mut from = 0;
    loop {
        let i = from + s[from..].find("=%")?;
        if let Some((v, rest)) = value_id(&s[i + 1..])
            && (rest.starts_with(", ") || rest.starts_with(')'))
            && i > 0
        {
            return Some(((s[..i].to_string(), v), rest));
        }
        from = i + 1;
    }
}

/// Operands up to `end` (`;` or `)`), which is consumed.
fn operands_until(mut r: &str, end: char) -> Option<(Vec<Operand>, &str)> {
    let mut out = Vec::new();
    loop {
        if let Some(rest) = r.strip_prefix(end) {
            return Some((out, rest));
        }
        let (o, next) = operand(r, end)?;
        out.push(o);
        r = next.strip_prefix(", ").unwrap_or(next);
    }
}

impl Program {
    /// The program the text form `text` is ([`Program::to_text`]'s
    /// inverse): `parse(p.to_text())` is `p`, but for source lines no
    /// value was defined on.
    pub fn parse(text: &str) -> Result<Program, String> {
        let mut prog = Program::default();
        let mut lines: Vec<String> = Vec::new();
        let mut at = 0;
        for (k, l) in text.lines().enumerate() {
            let bad = |what: &str| format!("line {}: {what}: {l}", k + 1);
            if l.is_empty() {
                continue;
            }
            if let Some(h) = l.strip_prefix("; l.") {
                let end = h.find("  ").ok_or_else(|| bad("a heading"))?;
                at = h[..end].parse().map_err(|_| bad("a line number"))?;
                // (line 0: values not made at a line, the loop's)
                if at > 0 {
                    if lines.len() < at {
                        lines.resize(at, String::new());
                    }
                    lines[at - 1] = h[end + 2..].to_string();
                }
                continue;
            }
            let (id, rest) = value_id(l).ok_or_else(|| bad("a value"))?;
            if id.0 as usize != prog.values.len() {
                return Err(bad("values out of order"));
            }
            let rest = rest.strip_prefix(" = ").ok_or_else(|| bad("` = `"))?;
            let (def, rest) = if let Some(r) = rest.strip_prefix("env(") {
                let (base, mut r) = match value_id(r) {
                    Some((b, r)) => (Some(b), r.strip_prefix("; ").unwrap_or(r)),
                    None => (None, r),
                };
                let mut binds = Vec::new();
                while !r.starts_with(')') {
                    let (b, next) = bind(r).ok_or_else(|| bad("a name and its value"))?;
                    binds.push(b);
                    r = next.strip_prefix(", ").unwrap_or(next);
                }
                (Def::Env { base, binds }, &r[1..])
            } else if let Some(r) = rest.strip_prefix("pending(") {
                let (env, r) = value_id(r).ok_or_else(|| bad("pending's env"))?;
                let r = r.strip_prefix("; ").ok_or_else(|| bad("`; `"))?;
                let (par, r) = operands_until(r, ';').ok_or_else(|| bad("the paragraph"))?;
                let r = r.strip_prefix(' ').unwrap_or(r);
                let (text, r) = operands_until(r, ')').ok_or_else(|| bad("the text"))?;
                (Def::Pending { env, par, text }, r)
            } else if let Some(r) = rest.strip_prefix("φ(") {
                let (cond, r) = value_id(r).ok_or_else(|| bad("φ's test"))?;
                let r = r.strip_prefix(", ").ok_or_else(|| bad("`, `"))?;
                let (then, r) = value_id(r).ok_or_else(|| bad("φ's then"))?;
                let r = r.strip_prefix(", ").ok_or_else(|| bad("`, `"))?;
                let (els, r) = value_id(r).ok_or_else(|| bad("φ's else"))?;
                let r = r.strip_prefix(')').ok_or_else(|| bad("`)`"))?;
                (Def::Phi { cond, then, els }, r)
            } else if let Some(r) = rest.strip_prefix("μ(") {
                let (init, r) = value_id(r).ok_or_else(|| bad("μ's init"))?;
                let r = r.strip_prefix(", ").ok_or_else(|| bad("`, `"))?;
                let (next, r) = value_id(r).ok_or_else(|| bad("μ's next"))?;
                let r = r.strip_prefix(')').ok_or_else(|| bad("`)`"))?;
                (Def::Mu { init, next }, r)
            } else if let Some((callee, r)) = value_id(rest) {
                let mut r = r.strip_prefix('(').ok_or_else(|| bad("`(`"))?;
                let mut operands = Vec::new();
                while !r.starts_with(')') {
                    let (o, next) = operand(r, ')').ok_or_else(|| bad("an operand"))?;
                    operands.push(o);
                    r = next.strip_prefix(", ").unwrap_or(next);
                }
                (Def::Apply { callee, operands }, &r[1..])
            } else if let Some((op, r)) = op_head(rest) {
                op_rest(op, r).ok_or_else(|| bad("an operation"))?
            } else {
                let end = rest.find(" ; ").unwrap_or(rest.len());
                (Def::Const(rest[..end].trim_end().to_string()), &rest[end..])
            };
            let shows = match rest.trim_start().strip_prefix("; ") {
                Some(s) => s.to_string(),
                None if rest.trim().is_empty() => String::new(),
                None => return Err(bad("text after the value")),
            };
            prog.values.push(Value {
                def,
                shows,
                line: at,
            });
        }
        prog.source = lines.join("\n");
        Ok(prog)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `p`'s text parses back to `p`, and prints again as the same text.
    fn round_trips(p: &Program) {
        let text = p.to_text();
        let q = Program::parse(&text).unwrap_or_else(|e| panic!("{e}\n{text}"));
        assert_eq!(q.to_text(), text);
        assert_eq!(q.values, p.values, "{text}");
    }

    fn value(def: Def, shows: &str) -> Value {
        Value {
            def,
            shows: shows.to_string(),
            line: 0,
        }
    }

    /// A program as `PhiTeX`'s builder writes one, with headings.
    const PHITEX: &str = "
; l.1  \\def\\name{World}
%0 = prim def
%1 = catcodes plain
%2 = %0(%1, \\name, \"World\")                  ; macro \"World\"

; l.2  \\greet{\\name}
%3 = prim par
%4 = %3(%1, %2:\"Hello \", 'x', \"a \"\"b\"\"\")     ; paragraph
%5 = φ(%4, %2, %3)                           ; \\count1 (then)
%6 = μ(%5, %7)
%7 = env(%6; \\count1=%2, catcodes=%1)
%8 = pending(%7; \"ab\"; %2:\"\\loop\", \"␤␤more\")
";

    #[test]
    fn phitex_programs_round_trip() {
        let p = Program::parse(PHITEX).unwrap();
        p.check().unwrap();
        assert_eq!(p.values.len(), 9);
        assert_eq!(p.to_text(), PHITEX);
        round_trips(&p);
    }

    /// Operations with their imports and exports by name, the names that
    /// need quoting quoted.
    #[test]
    fn operations_round_trip() {
        let named = |n: &str, v: u32| Operand::Named(n.to_string(), ValueId(v));
        let strings = |ns: &[&str]| ns.iter().map(|n| (*n).to_string()).collect();
        let odd = [
            "\\=",
            "\\csname a b\\endcsname",
            "a\"b",
            "x, y",
            "p; q",
            "f(x)",
            "'c'",
            "100%",
            "a␤b",
        ];
        let p = Program {
            values: vec![
                value(Def::Const("format plain".to_string()), "the format"),
                value(Def::Const("file edits.tex".to_string()), ""),
                value(
                    Def::Op {
                        op: "window".to_string(),
                        operands: vec![
                            named("\\count1", 0),
                            named("edits.tex:1-3", 1),
                            named(odd[0], 0),
                        ],
                        defines: strings(&["\\count1", "\\x"]),
                    },
                    "step 1: edits.tex:1-3 \"Hello\"",
                ),
                value(
                    Def::Op {
                        op: "window".to_string(),
                        operands: Vec::new(),
                        defines: strings(&odd),
                    },
                    "",
                ),
                value(
                    Def::Op {
                        op: "window".to_string(),
                        operands: odd.iter().map(|n| named(n, 3)).collect(),
                        defines: Vec::new(),
                    },
                    "imports only",
                ),
                value(
                    Def::Op {
                        op: "page_step".to_string(),
                        operands: vec![named("\\count1", 2), Operand::Value(ValueId(4))],
                        defines: strings(&["\\x"]),
                    },
                    "",
                ),
                value(
                    Def::Apply {
                        callee: ValueId(0),
                        operands: vec![named("\\x", 5), Operand::Source("\\y".to_string())],
                    },
                    "an application with a name",
                ),
            ],
            source: String::new(),
        };
        p.check().unwrap();
        let text = p.to_text();
        assert!(
            text.contains("%2 = window(\\count1=%0, edits.tex:1-3=%1, \"\\=\"=%0; \\count1, \\x)"),
            "{text}"
        );
        assert!(text.contains("%4 = window(\"\\=\"=%3, "), "{text}");
        assert!(text.contains("\"a\"\"b\"=%3, \"x, y\"=%3, "), "{text}");
        assert!(text.contains("%3 = window(; \"\\=\", "), "{text}");
        assert!(text.contains("\"100%\"=%3, \"a␤b\"=%3; )"), "{text}");
        round_trips(&p);
    }

    /// What `check` rejects: a name imported from an operation that does
    /// not define it, a use before its definition, a name with a line end.
    #[test]
    fn check_names() {
        let parse = |t: &str| Program::parse(t).unwrap();
        let ok = "%0 = format\n%1 = window(\\a=%0; \\b)\n%2 = window(\\b=%1, \\c=%0; )\n";
        parse(ok).check().unwrap();
        let undefined = "%0 = format\n%1 = window(\\a=%0; \\b)\n%2 = window(\\a=%1; )\n";
        assert_eq!(
            parse(undefined).check(),
            Err("%2 imports \\a from %1, which does not define it".to_string())
        );
        let later = "%0 = window(\\a=%1; )\n%1 = format\n";
        assert!(parse(later).check().is_err());
        let p = Program {
            values: vec![value(
                Def::Op {
                    op: "window".to_string(),
                    operands: Vec::new(),
                    defines: vec!["a\nb".to_string()],
                },
                "",
            )],
            source: String::new(),
        };
        assert_eq!(
            p.check(),
            Err("%0: the name \"a\\nb\" has a line end".to_string())
        );
    }
}
