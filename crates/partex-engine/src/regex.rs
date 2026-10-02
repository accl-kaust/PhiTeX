//! POSIX extended regular expressions, as `\pdfmatch` sees them through
//! glibc's `regcomp(REG_EXTENDED)` and `regexec`: leftmost-longest
//! matches over bytes, GNU's escapes (`\w`, `\b`, `\<`, back
//! references …) and glibc's error messages.
//!
//! The matcher backtracks over a small program, exploring every way to
//! match at the leftmost starting point and keeping the longest; the
//! submatches are those of the first path (in greedy order) that reaches
//! that length. That is POSIX's rule for the whole match; for the
//! submatches of ambiguous patterns glibc's own rules may differ.

use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;

/// A byte set (a bracket expression, `.`, a class escape).
#[derive(Clone, Debug, PartialEq, Eq)]
struct Set([u64; 4]);

impl Set {
    const fn empty() -> Self {
        Self([0; 4])
    }

    fn add(&mut self, b: u8) {
        self.0[usize::from(b >> 6)] |= 1 << (b & 63);
    }

    fn has(&self, b: u8) -> bool {
        self.0[usize::from(b >> 6)] & (1 << (b & 63)) != 0
    }

    fn negate(&mut self) {
        for w in &mut self.0 {
            *w = !*w;
        }
    }

    fn add_class(&mut self, class: &[u8]) -> bool {
        let f: fn(u8) -> bool = match class {
            b"alpha" => |c| c.is_ascii_alphabetic(),
            b"digit" => |c| c.is_ascii_digit(),
            b"alnum" => |c| c.is_ascii_alphanumeric(),
            b"upper" => |c| c.is_ascii_uppercase(),
            b"lower" => |c| c.is_ascii_lowercase(),
            b"space" => |c| matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 11 | 12),
            b"blank" => |c| c == b' ' || c == b'\t',
            b"punct" => |c| c.is_ascii_punctuation(),
            b"print" => |c| (32..127).contains(&c),
            b"graph" => |c| c.is_ascii_graphic(),
            b"cntrl" => |c| c < 32 || c == 127,
            b"xdigit" => |c| c.is_ascii_hexdigit(),
            _ => return false,
        };
        for c in 0..=255u8 {
            if f(c) {
                self.add(c);
            }
        }
        true
    }

    /// Close the set under ASCII case (`REG_ICASE`).
    fn fold(&mut self) {
        for c in b'a'..=b'z' {
            let u = c.to_ascii_uppercase();
            if self.has(c) || self.has(u) {
                self.add(c);
                self.add(u);
            }
        }
    }
}

/// A word byte for `\w`, `\b`, `\<` and `\>`.
fn is_word(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

/// A compile error: glibc's `regerror` text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    BadRepeat,
    Paren,
    Brack,
    Brace,
    BadBrace,
    Range,
    CType,
    Escape,
    SubReg,
    Collate,
    Size,
}

impl Error {
    /// glibc's message (`__re_error_msgid`).
    #[must_use]
    pub fn message(self) -> &'static [u8] {
        match self {
            Self::BadRepeat => b"Invalid preceding regular expression",
            Self::Paren => b"Unmatched ( or \\(",
            Self::Brack => b"Unmatched [, [^, [:, [., or [=",
            Self::Brace => b"Unmatched \\{",
            Self::BadBrace => b"Invalid content of \\{\\}",
            Self::Range => b"Invalid range end",
            Self::CType => b"Invalid character class name",
            Self::Escape => b"Trailing backslash",
            Self::SubReg => b"Invalid back reference",
            Self::Collate => b"Invalid collation character",
            Self::Size => b"Regular expression too big",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Assert {
    Start,
    End,
    WordBoundary,
    NotWordBoundary,
    WordStart,
    WordEnd,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Node {
    Set(Set),
    Assert(Assert),
    BackRef(usize),
    Group(usize, Box<Node>),
    Concat(Vec<Node>),
    Alt(Vec<Node>),
    Repeat {
        node: Box<Node>,
        min: u32,
        max: Option<u32>,
    },
}

/// `RE_DUP_MAX`.
const DUP_MAX: u32 = 0x7fff;

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
    groups: usize,
    /// Groups closed so far (a back reference must name one).
    closed: Vec<bool>,
    icase: bool,
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }

    fn alt(&mut self, depth: usize) -> Result<Node, Error> {
        let mut branches = vec![self.concat(depth)?];
        while self.peek() == Some(b'|') {
            self.i += 1;
            branches.push(self.concat(depth)?);
        }
        Ok(if branches.len() == 1 {
            branches.pop().expect("a branch")
        } else {
            Node::Alt(branches)
        })
    }

    fn concat(&mut self, depth: usize) -> Result<Node, Error> {
        let mut items: Vec<Node> = Vec::new();
        // whether a repetition operator may follow (`RE_CONTEXT_INVALID_OPS`)
        let mut can_repeat = false;
        while let Some(c) = self.peek() {
            match c {
                b'|' => break,
                b')' if depth > 0 => break,
                b'*' | b'+' | b'?' | b'{' => {
                    if !can_repeat {
                        return Err(Error::BadRepeat);
                    }
                    self.i += 1;
                    let (min, max) = match c {
                        b'*' => (0, None),
                        b'+' => (1, None),
                        b'?' => (0, Some(1)),
                        _ => self.interval()?,
                    };
                    let node = items.pop().expect("an item");
                    items.push(Node::Repeat {
                        node: Box::new(node),
                        min,
                        max,
                    });
                }
                _ => {
                    let atom = self.atom(depth)?;
                    can_repeat = !matches!(atom, Node::Assert(Assert::Start | Assert::End));
                    items.push(atom);
                }
            }
        }
        Ok(if items.len() == 1 {
            items.pop().expect("an item")
        } else {
            Node::Concat(items)
        })
    }

    fn number(&mut self) -> Option<u32> {
        let start = self.i;
        let mut n: u32 = 0;
        while let Some(d @ b'0'..=b'9') = self.peek() {
            n = n.saturating_mul(10).saturating_add(u32::from(d - b'0'));
            self.i += 1;
        }
        (self.i > start).then_some(n)
    }

    /// `{m}`, `{m,}`, `{m,n}` or `{,n}` (after the brace).
    fn interval(&mut self) -> Result<(u32, Option<u32>), Error> {
        let min = self.number();
        let max = if self.peek() == Some(b',') {
            self.i += 1;
            self.number()
        } else {
            match min {
                Some(m) => Some(m),
                None => {
                    return Err(if self.peek().is_none() {
                        Error::Brace
                    } else {
                        Error::BadBrace
                    });
                }
            }
        };
        match self.peek() {
            Some(b'}') => self.i += 1,
            None => return Err(Error::Brace),
            Some(_) => return Err(Error::BadBrace),
        }
        let min = min.unwrap_or(0);
        if max.is_some_and(|m| m < min) {
            return Err(Error::BadBrace);
        }
        if min > DUP_MAX || max.is_some_and(|m| m > DUP_MAX) {
            return Err(Error::Size);
        }
        Ok((min, max))
    }

    fn byte(&self, c: u8) -> Node {
        let mut s = Set::empty();
        s.add(c);
        if self.icase {
            s.fold();
        }
        Node::Set(s)
    }

    fn atom(&mut self, depth: usize) -> Result<Node, Error> {
        let c = self.peek().expect("a byte");
        self.i += 1;
        Ok(match c {
            b'(' => {
                self.groups += 1;
                let n = self.groups;
                self.closed.push(false);
                let inner = self.alt(depth + 1)?;
                if self.peek() != Some(b')') {
                    return Err(Error::Paren);
                }
                self.i += 1;
                self.closed[n - 1] = true;
                Node::Group(n, Box::new(inner))
            }
            b'^' => Node::Assert(Assert::Start),
            b'$' => Node::Assert(Assert::End),
            b'.' => {
                let mut s = Set::empty();
                s.negate();
                Node::Set(s)
            }
            b'[' => Node::Set(self.bracket()?),
            b'\\' => {
                let Some(e) = self.peek() else {
                    return Err(Error::Escape);
                };
                self.i += 1;
                match e {
                    b'1'..=b'9' => {
                        let n = usize::from(e - b'0');
                        if !self.closed.get(n - 1).copied().unwrap_or(false) {
                            return Err(Error::SubReg);
                        }
                        Node::BackRef(n)
                    }
                    b'w' | b'W' | b's' | b'S' => {
                        let mut s = Set::empty();
                        for b in 0..=255u8 {
                            let word = if e.eq_ignore_ascii_case(&b'w') {
                                is_word(b)
                            } else {
                                b.is_ascii_whitespace() || b == 11
                            };
                            if word {
                                s.add(b);
                            }
                        }
                        if e.is_ascii_uppercase() {
                            s.negate();
                        }
                        Node::Set(s)
                    }
                    b'b' => Node::Assert(Assert::WordBoundary),
                    b'B' => Node::Assert(Assert::NotWordBoundary),
                    b'<' => Node::Assert(Assert::WordStart),
                    b'>' => Node::Assert(Assert::WordEnd),
                    b'`' => Node::Assert(Assert::Start),
                    b'\'' => Node::Assert(Assert::End),
                    _ => self.byte(e),
                }
            }
            _ => self.byte(c),
        })
    }

    /// A bracket expression (after the `[`).
    fn bracket(&mut self) -> Result<Set, Error> {
        let mut set = Set::empty();
        let negated = self.peek() == Some(b'^');
        if negated {
            self.i += 1;
        }
        let mut first = true;
        loop {
            let Some(c) = self.peek() else {
                return Err(Error::Brack);
            };
            if c == b']' && !first {
                self.i += 1;
                break;
            }
            first = false;
            let lo = self.bracket_item(&mut set)?;
            let Some(lo) = lo else { continue };
            // a range?
            if self.peek() == Some(b'-') && self.s.get(self.i + 1).is_some_and(|&c| c != b']') {
                self.i += 1;
                let Some(hi) = self.bracket_item(&mut set)? else {
                    return Err(Error::Range);
                };
                if hi < lo {
                    return Err(Error::Range);
                }
                for b in lo..=hi {
                    set.add(b);
                }
            } else {
                set.add(lo);
            }
        }
        if self.icase {
            set.fold();
        }
        if negated {
            set.negate();
        }
        Ok(set)
    }

    /// One element of a bracket expression: a byte (returned, so it can
    /// start a range) or a class (added to `set`).
    fn bracket_item(&mut self, set: &mut Set) -> Result<Option<u8>, Error> {
        let c = self.peek().ok_or(Error::Brack)?;
        self.i += 1;
        if c != b'[' {
            return Ok(Some(c));
        }
        let Some(kind @ (b':' | b'.' | b'=')) = self.peek() else {
            return Ok(Some(b'['));
        };
        self.i += 1;
        let start = self.i;
        loop {
            match (self.s.get(self.i), self.s.get(self.i + 1)) {
                (Some(&a), Some(b']')) if a == kind => break,
                (None, _) => return Err(Error::Brack),
                _ => self.i += 1,
            }
        }
        let name = &self.s[start..self.i];
        self.i += 2;
        match kind {
            b':' => {
                if !set.add_class(name) {
                    return Err(Error::CType);
                }
                Ok(None)
            }
            _ => match name {
                [b] => Ok(Some(*b)),
                _ => Err(Error::Collate),
            },
        }
    }
}

#[derive(Clone, Debug)]
enum Inst {
    Set(Set),
    Assert(Assert),
    BackRef(usize),
    /// Try `.0` first, then `.1`.
    Split(usize, usize),
    Jmp(usize),
    Save(usize),
    /// Remember the position at a loop's entry, in register `.0`.
    Mark(usize),
    /// Leave the loop for `.1` if the position did not move since
    /// register `.0` was marked (an empty iteration ends it).
    Progress(usize, usize),
    Match,
}

/// A compiled pattern (`regex_t`).
#[derive(Clone, Debug)]
pub struct Regex {
    prog: Vec<Inst>,
    groups: usize,
    marks: usize,
}

/// A bound on the program's size (`REG_ESIZE`).
const MAX_PROG: usize = 1 << 20;

struct Compiler {
    prog: Vec<Inst>,
    marks: usize,
}

impl Compiler {
    fn emit(&mut self, i: Inst) -> Result<usize, Error> {
        if self.prog.len() >= MAX_PROG {
            return Err(Error::Size);
        }
        self.prog.push(i);
        Ok(self.prog.len() - 1)
    }

    fn node(&mut self, n: &Node) -> Result<(), Error> {
        match n {
            Node::Set(s) => {
                self.emit(Inst::Set(s.clone()))?;
            }
            Node::Assert(a) => {
                self.emit(Inst::Assert(*a))?;
            }
            Node::BackRef(k) => {
                self.emit(Inst::BackRef(*k))?;
            }
            Node::Group(k, inner) => {
                self.emit(Inst::Save(2 * k))?;
                self.node(inner)?;
                self.emit(Inst::Save(2 * k + 1))?;
            }
            Node::Concat(items) => {
                for i in items {
                    self.node(i)?;
                }
            }
            Node::Alt(branches) => {
                let mut jumps = Vec::new();
                for (k, b) in branches.iter().enumerate() {
                    if k + 1 < branches.len() {
                        let split = self.emit(Inst::Split(0, 0))?;
                        self.node(b)?;
                        jumps.push(self.emit(Inst::Jmp(0))?);
                        let next = self.prog.len();
                        self.prog[split] = Inst::Split(split + 1, next);
                    } else {
                        self.node(b)?;
                    }
                }
                let end = self.prog.len();
                for j in jumps {
                    self.prog[j] = Inst::Jmp(end);
                }
            }
            Node::Repeat { node, min, max } => {
                for _ in 0..*min {
                    self.node(node)?;
                }
                match max {
                    None => self.star(node)?,
                    Some(max) => {
                        // optional copies, each skipping the rest
                        let mut splits = Vec::new();
                        for _ in *min..*max {
                            splits.push(self.emit(Inst::Split(0, 0))?);
                            self.node(node)?;
                        }
                        let end = self.prog.len();
                        for s in splits {
                            self.prog[s] = Inst::Split(s + 1, end);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// `node*`, refusing empty iterations.
    fn star(&mut self, node: &Node) -> Result<(), Error> {
        let reg = self.marks;
        self.marks += 1;
        let split = self.emit(Inst::Split(0, 0))?;
        self.emit(Inst::Mark(reg))?;
        self.node(node)?;
        let progress = self.emit(Inst::Progress(reg, 0))?;
        self.emit(Inst::Jmp(split))?;
        let end = self.prog.len();
        self.prog[split] = Inst::Split(split + 1, end);
        self.prog[progress] = Inst::Progress(reg, end);
        Ok(())
    }
}

/// How many steps a search may take before settling for the longest
/// match found so far.
const STEP_LIMIT: usize = 4_000_000;

/// A match: the whole match and each group, as byte ranges.
pub type Captures = Vec<Option<(usize, usize)>>;

impl Regex {
    /// `regcomp` with `REG_EXTENDED` (and `REG_ICASE` if `icase`).
    ///
    /// # Errors
    ///
    /// When the pattern is malformed, as glibc reports it.
    pub fn new(pattern: &[u8], icase: bool) -> Result<Self, Error> {
        let mut p = Parser {
            s: pattern,
            i: 0,
            groups: 0,
            closed: Vec::new(),
            icase,
        };
        let mut ast = p.alt(0)?;
        // an unmatched `)` is an ordinary byte (`RE_UNMATCHED_RIGHT_PAREN_ORD`)
        while p.i < pattern.len() {
            p.i += 1;
            let rest = p.alt(0)?;
            ast = Node::Concat(vec![ast, p.byte(b')'), rest]);
        }
        let mut c = Compiler {
            prog: Vec::new(),
            marks: 0,
        };
        c.node(&ast)?;
        c.emit(Inst::Match)?;
        Ok(Self {
            prog: c.prog,
            groups: p.groups,
            marks: c.marks,
        })
    }

    /// `regexec`: the leftmost-longest match in `text`, with its groups.
    #[must_use]
    pub fn exec(&self, text: &[u8]) -> Option<Captures> {
        for start in 0..=text.len() {
            if let Some(caps) = self.at(text, start) {
                return Some(caps);
            }
        }
        None
    }

    fn assert(a: Assert, text: &[u8], pos: usize) -> bool {
        let before = pos > 0 && is_word(text[pos - 1]);
        let after = pos < text.len() && is_word(text[pos]);
        match a {
            Assert::Start => pos == 0,
            Assert::End => pos == text.len(),
            Assert::WordBoundary => before != after,
            Assert::NotWordBoundary => before == after,
            Assert::WordStart => !before && after,
            Assert::WordEnd => before && !after,
        }
    }

    /// The longest match starting at `start`.
    fn at(&self, text: &[u8], start: usize) -> Option<Captures> {
        let nslots = 2 * (self.groups + 1);
        let mut slots: Vec<Option<usize>> = vec![None; nslots];
        let mut marks: Vec<usize> = vec![usize::MAX; self.marks];
        // undo log: (is_mark, index, old value)
        let mut undo: Vec<(bool, usize, Option<usize>)> = Vec::new();
        // backtrack points: (pc, pos, undo length)
        let mut stack: Vec<(usize, usize, usize)> = vec![(0, start, 0)];
        let mut best: Option<(usize, Vec<Option<usize>>)> = None;
        let mut steps = 0usize;
        'threads: while let Some((mut pc, mut pos, u)) = stack.pop() {
            while undo.len() > u {
                let (is_mark, i, old) = undo.pop().expect("undo entry");
                if is_mark {
                    marks[i] = old.unwrap_or(usize::MAX);
                } else {
                    slots[i] = old;
                }
            }
            loop {
                steps += 1;
                if steps > STEP_LIMIT && best.is_some() {
                    break 'threads;
                }
                match &self.prog[pc] {
                    Inst::Set(s) => {
                        if pos < text.len() && s.has(text[pos]) {
                            pos += 1;
                            pc += 1;
                        } else {
                            continue 'threads;
                        }
                    }
                    Inst::Assert(a) => {
                        if Self::assert(*a, text, pos) {
                            pc += 1;
                        } else {
                            continue 'threads;
                        }
                    }
                    Inst::BackRef(k) => {
                        let (Some(a), Some(b)) = (slots[2 * k], slots[2 * k + 1]) else {
                            continue 'threads;
                        };
                        let n = b - a;
                        if pos + n <= text.len() && text[a..b] == text[pos..pos + n] {
                            pos += n;
                            pc += 1;
                        } else {
                            continue 'threads;
                        }
                    }
                    Inst::Split(x, y) => {
                        stack.push((*y, pos, undo.len()));
                        pc = *x;
                    }
                    Inst::Jmp(x) => pc = *x,
                    Inst::Save(i) => {
                        undo.push((false, *i, slots[*i]));
                        slots[*i] = Some(pos);
                        pc += 1;
                    }
                    Inst::Mark(r) => {
                        let old = (marks[*r] != usize::MAX).then_some(marks[*r]);
                        undo.push((true, *r, old));
                        marks[*r] = pos;
                        pc += 1;
                    }
                    Inst::Progress(r, exit) => {
                        pc = if marks[*r] == pos { *exit } else { pc + 1 };
                    }
                    Inst::Match => {
                        if best.as_ref().is_none_or(|(end, _)| pos > *end) {
                            best = Some((pos, slots.clone()));
                        }
                        if pos == text.len() {
                            break 'threads; // nothing can be longer
                        }
                        continue 'threads;
                    }
                }
            }
        }
        let (end, slots) = best?;
        let mut caps = vec![Some((start, end))];
        for k in 1..=self.groups {
            caps.push(match (slots[2 * k], slots[2 * k + 1]) {
                (Some(a), Some(b)) if b >= a => Some((a, b)),
                _ => None,
            });
        }
        Some(caps)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(p: &str, t: &str) -> Option<Captures> {
        Regex::new(p.as_bytes(), false).unwrap().exec(t.as_bytes())
    }

    #[test]
    fn leftmost_longest() {
        assert_eq!(m("a|ab", "xab").unwrap()[0], Some((1, 3)));
        assert_eq!(m("(a*)*", "b").unwrap()[1], Some((0, 0)));
        assert_eq!(m("x(a|b)+y", "zxabay").unwrap()[1], Some((4, 5)));
        assert_eq!(m("^[[:digit:]]{2,3}$", "123").unwrap()[0], Some((0, 3)));
        assert!(m("^[[:digit:]]{2,3}$", "1234").is_none());
        assert_eq!(m("(.)\\1", "abcc").unwrap()[0], Some((2, 4)));
        assert_eq!(m("[]a]+", "x]a]").unwrap()[0], Some((1, 4)));
        assert_eq!(m("a)", "a)").unwrap()[0], Some((0, 2)));
    }

    #[test]
    fn errors() {
        assert_eq!(Regex::new(b"*a", false).unwrap_err(), Error::BadRepeat);
        assert_eq!(Regex::new(b"(a", false).unwrap_err(), Error::Paren);
        assert_eq!(Regex::new(b"[a", false).unwrap_err(), Error::Brack);
        assert_eq!(Regex::new(b"a\\", false).unwrap_err(), Error::Escape);
        assert_eq!(Regex::new(b"[[:foo:]]", false).unwrap_err(), Error::CType);
        assert!(Regex::new(b"A", true).unwrap().exec(b"a").is_some());
    }
}
