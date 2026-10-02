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

use std::fmt::Write as _;

/// A value's name, `%n`: its index in [`Program::values`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ValueId(pub u32);

impl std::fmt::Display for ValueId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
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
}

fn operand_uses(o: &Operand) -> Option<ValueId> {
    match o {
        Operand::Value(u) | Operand::From(u, _) => Some(*u),
        Operand::Source(_) => None,
    }
}

impl Def {
    /// The values this one uses.
    #[must_use]
    pub fn uses(&self) -> Vec<ValueId> {
        match self {
            Def::Const(_) => Vec::new(),
            Def::Apply { callee, operands } => std::iter::once(*callee)
                .chain(operands.iter().filter_map(operand_uses))
                .collect(),
            Def::Phi { cond, then, els } => vec![*cond, *then, *els],
            Def::Mu { init, next } => vec![*init, *next],
            Def::Env { base, binds } => base
                .iter()
                .copied()
                .chain(binds.iter().map(|b| b.1))
                .collect(),
            Def::Pending { env, par, text } => std::iter::once(*env)
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
    /// defined before its use.
    pub fn check(&self) -> Result<(), String> {
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
    }
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

/// One operand at the start of `s` (up to `, ` or the closing `)`).
fn operand(s: &str) -> Option<(Operand, &str)> {
    if let Some((v, rest)) = value_id(s) {
        return Some(match rest.strip_prefix(':') {
            Some(r) => {
                let (text, rest) = unquote(r)?;
                (Operand::From(v, text), rest)
            }
            None => (Operand::Value(v), rest),
        });
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
    let end = [s.find(", "), s.rfind(')')]
        .into_iter()
        .flatten()
        .min()
        .unwrap_or(s.len());
    Some((Operand::Source(s[..end].to_string()), &s[end..]))
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
        let (o, next) = operand(r)?;
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
                    let (o, next) = operand(r).ok_or_else(|| bad("an operand"))?;
                    operands.push(o);
                    r = next.strip_prefix(", ").unwrap_or(next);
                }
                (Def::Apply { callee, operands }, &r[1..])
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
