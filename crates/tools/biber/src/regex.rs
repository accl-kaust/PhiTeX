//! A backtracking matcher for the Perl regular expressions biber takes
//! from its configuration (`\DeclareNosort`, `\DeclareNoinit`,
//! `\DeclareNolabel`, source map `match` steps, ...) and for the default
//! patterns of Biber::Constants. Covers Perl's syntax as those patterns use
//! it: classes with `\p{..}`, groups (capturing, `(?:..)`, look-ahead and
//! fixed look-behind), greedy/lazy quantifiers, alternation, anchors,
//! `\b`, back-references and the `i`, `m`, `s`, `x` flags.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::unicode;

#[derive(Clone, Debug)]
enum ClassItem {
    Ch(char),
    Range(char, char),
    Prop(String, bool),
    Digit(bool),
    Word(bool),
    Space(bool),
    HSpace(bool),
}

#[derive(Clone, Debug)]
enum Node {
    Char(char),
    Any,
    Class(Vec<ClassItem>, bool),
    Start,
    End,
    StrStart,
    StrEnd,
    StrEndNl,
    WordB(bool),
    Group(Box<Node>, Option<usize>),
    Look(Box<Node>, bool, bool), // node, ahead, negate
    Repeat(Box<Node>, usize, usize, bool), // min, max, greedy
    Alt(Vec<Node>),
    Seq(Vec<Node>),
    Backref(usize),
}

#[derive(Clone, Debug)]
pub struct Regex {
    node: Node,
    ncaps: usize,
    icase: bool,
    multiline: bool,
    dotall: bool,
}

/// A match: the whole span and the capture groups (char indices).
#[derive(Clone, Debug)]
pub struct Match {
    pub start: usize,
    pub end: usize,
    pub caps: Vec<Option<(usize, usize)>>,
}

struct Parser<'a> {
    p: &'a [char],
    i: usize,
    ncaps: usize,
    extended: bool,
}

impl Parser<'_> {
    fn peek(&self) -> Option<char> {
        self.p.get(self.i).copied()
    }
    fn skip_x(&mut self) {
        if !self.extended {
            return;
        }
        loop {
            match self.peek() {
                Some(c) if c.is_whitespace() => self.i += 1,
                Some('#') => {
                    while let Some(c) = self.peek() {
                        self.i += 1;
                        if c == '\n' {
                            break;
                        }
                    }
                }
                _ => break,
            }
        }
    }
    fn alt(&mut self) -> Result<Node, String> {
        let mut alts = vec![self.seq()?];
        while self.peek() == Some('|') {
            self.i += 1;
            alts.push(self.seq()?);
        }
        Ok(if alts.len() == 1 { alts.pop().unwrap() } else { Node::Alt(alts) })
    }
    fn seq(&mut self) -> Result<Node, String> {
        let mut items = Vec::new();
        loop {
            self.skip_x();
            match self.peek() {
                None | Some('|' | ')') => break,
                _ => {}
            }
            let atom = self.atom()?;
            self.skip_x();
            let atom = self.quant(atom)?;
            items.push(atom);
        }
        Ok(Node::Seq(items))
    }
    fn number(&mut self) -> Option<usize> {
        let s = self.i;
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.i += 1;
        }
        if s == self.i {
            return None;
        }
        self.p[s..self.i].iter().collect::<String>().parse().ok()
    }
    fn quant(&mut self, atom: Node) -> Result<Node, String> {
        let (min, max) = match self.peek() {
            Some('*') => {
                self.i += 1;
                (0, usize::MAX)
            }
            Some('+') => {
                self.i += 1;
                (1, usize::MAX)
            }
            Some('?') => {
                self.i += 1;
                (0, 1)
            }
            Some('{') => {
                let save = self.i;
                self.i += 1;
                let Some(a) = self.number() else {
                    self.i = save;
                    return Ok(atom);
                };
                let b = if self.peek() == Some(',') {
                    self.i += 1;
                    self.number().unwrap_or(usize::MAX)
                } else {
                    a
                };
                if self.peek() != Some('}') {
                    self.i = save;
                    return Ok(atom);
                }
                self.i += 1;
                (a, b)
            }
            _ => return Ok(atom),
        };
        let mut greedy = true;
        if self.peek() == Some('?') {
            self.i += 1;
            greedy = false;
        } else if self.peek() == Some('+') {
            self.i += 1; // possessive: treated as greedy
        }
        Ok(Node::Repeat(Box::new(atom), min, max, greedy))
    }
    fn hex_escape(&mut self) -> Result<char, String> {
        if self.peek() == Some('{') {
            self.i += 1;
            let s = self.i;
            while self.peek().is_some_and(|c| c != '}') {
                self.i += 1;
            }
            let h: String = self.p[s..self.i].iter().collect();
            self.i += 1;
            let v = u32::from_str_radix(h.trim(), 16).map_err(|_| "bad \\x{}")?;
            char::from_u32(v).ok_or_else(|| "bad char".into())
        } else {
            let s = self.i;
            while self.i < s + 2 && self.peek().is_some_and(|c| c.is_ascii_hexdigit()) {
                self.i += 1;
            }
            let h: String = self.p[s..self.i].iter().collect();
            let v = if h.is_empty() { 0 } else { u32::from_str_radix(&h, 16).unwrap_or(0) };
            char::from_u32(v).ok_or_else(|| "bad char".into())
        }
    }
    fn prop_name(&mut self) -> Result<(String, bool), String> {
        let mut neg = false;
        let name: String = if self.peek() == Some('{') {
            self.i += 1;
            if self.peek() == Some('^') {
                neg = true;
                self.i += 1;
            }
            let s = self.i;
            while self.peek().is_some_and(|c| c != '}') {
                self.i += 1;
            }
            let n = self.p[s..self.i].iter().collect();
            self.i += 1;
            n
        } else {
            let c = self.peek().ok_or("bad \\p")?;
            self.i += 1;
            c.into()
        };
        if unicode::has_property('a', &name).is_none() {
            return Err(alloc::format!("unsupported property \\p{{{name}}}"));
        }
        Ok((name, neg))
    }
    /// An escape after `\`, inside or outside a class.
    fn escape(&mut self, in_class: bool) -> Result<Result<ClassItem, Node>, String> {
        let c = self.peek().ok_or("trailing backslash")?;
        self.i += 1;
        Ok(Ok(match c {
            'd' => ClassItem::Digit(false),
            'D' => ClassItem::Digit(true),
            'w' => ClassItem::Word(false),
            'W' => ClassItem::Word(true),
            's' => ClassItem::Space(false),
            'S' => ClassItem::Space(true),
            'h' => ClassItem::HSpace(false),
            'H' => ClassItem::HSpace(true),
            'p' | 'P' => {
                let (n, neg) = self.prop_name()?;
                ClassItem::Prop(n, neg ^ (c == 'P'))
            }
            'n' => ClassItem::Ch('\n'),
            't' => ClassItem::Ch('\t'),
            'r' => ClassItem::Ch('\r'),
            'f' => ClassItem::Ch('\x0c'),
            'e' => ClassItem::Ch('\x1b'),
            'a' => ClassItem::Ch('\x07'),
            'x' => ClassItem::Ch(self.hex_escape()?),
            '0' => ClassItem::Ch('\0'),
            'b' if in_class => ClassItem::Ch('\x08'),
            'b' => return Ok(Err(Node::WordB(false))),
            'B' if !in_class => return Ok(Err(Node::WordB(true))),
            'A' if !in_class => return Ok(Err(Node::StrStart)),
            'z' if !in_class => return Ok(Err(Node::StrEnd)),
            'Z' if !in_class => return Ok(Err(Node::StrEndNl)),
            '1'..='9' if !in_class => {
                self.i -= 1;
                let n = self.number().unwrap_or(0);
                return Ok(Err(Node::Backref(n)));
            }
            c if c.is_ascii_alphanumeric() => {
                return Err(alloc::format!("unsupported escape \\{c}"));
            }
            c => ClassItem::Ch(c),
        }))
    }
    fn class(&mut self) -> Result<Node, String> {
        let mut neg = false;
        if self.peek() == Some('^') {
            neg = true;
            self.i += 1;
        }
        let mut items = Vec::new();
        let mut first = true;
        loop {
            let c = self.peek().ok_or("unterminated class")?;
            if c == ']' && !first {
                self.i += 1;
                break;
            }
            first = false;
            self.i += 1;
            let item = if c == '\\' {
                match self.escape(true)? {
                    Ok(it) => it,
                    Err(_) => return Err("bad escape in class".into()),
                }
            } else if c == '[' && self.peek() == Some(':') {
                let s = self.i + 1;
                let mut j = s;
                while j < self.p.len() && self.p[j] != ':' {
                    j += 1;
                }
                let name: String = self.p[s..j].iter().collect();
                self.i = j + 2;
                let (n, ng) = match name.strip_prefix('^') {
                    Some(n) => (n.into(), true),
                    None => (name, false),
                };
                let pn = match n.as_str() {
                    "alpha" => "XPosixAlpha",
                    "digit" => "XPosixDigit",
                    "alnum" => "XPosixAlnum",
                    "space" => "XPosixSpace",
                    "upper" => "XPosixUpper",
                    "lower" => "XPosixLower",
                    "punct" => "P",
                    "word" => "Word",
                    _ => return Err(alloc::format!("unsupported [:{n}:]")),
                };
                ClassItem::Prop(pn.into(), ng)
            } else {
                ClassItem::Ch(c)
            };
            if let ClassItem::Ch(a) = item
                && self.peek() == Some('-')
                && self.p.get(self.i + 1).is_some_and(|&x| x != ']')
            {
                self.i += 1;
                let b = self.peek().ok_or("bad range")?;
                self.i += 1;
                let b = if b == '\\' {
                    match self.escape(true)? {
                        Ok(ClassItem::Ch(b)) => b,
                        _ => return Err("bad range".into()),
                    }
                } else {
                    b
                };
                items.push(ClassItem::Range(a, b));
                continue;
            }
            items.push(item);
        }
        Ok(Node::Class(items, neg))
    }
    fn atom(&mut self) -> Result<Node, String> {
        let c = self.peek().ok_or("unexpected end")?;
        self.i += 1;
        Ok(match c {
            '.' => Node::Any,
            '^' => Node::Start,
            '$' => Node::End,
            '[' => self.class()?,
            '(' => {
                if self.peek() == Some('?') {
                    self.i += 1;
                    let k = self.peek().ok_or("bad group")?;
                    self.i += 1;
                    match k {
                        ':' => {
                            let n = self.alt()?;
                            self.close()?;
                            Node::Group(Box::new(n), None)
                        }
                        '=' | '!' => {
                            let n = self.alt()?;
                            self.close()?;
                            Node::Look(Box::new(n), true, k == '!')
                        }
                        '<' if matches!(self.peek(), Some('=' | '!')) => {
                            let neg = self.peek() == Some('!');
                            self.i += 1;
                            let n = self.alt()?;
                            self.close()?;
                            Node::Look(Box::new(n), false, neg)
                        }
                        '<' | '\'' | 'P' => {
                            // named capture
                            if k == 'P' {
                                self.i += 1;
                            }
                            while self.peek().is_some_and(|c| c != '>' && c != '\'') {
                                self.i += 1;
                            }
                            self.i += 1;
                            self.ncaps += 1;
                            let idx = self.ncaps;
                            let n = self.alt()?;
                            self.close()?;
                            Node::Group(Box::new(n), Some(idx))
                        }
                        _ => return Err("unsupported group".into()),
                    }
                } else {
                    self.ncaps += 1;
                    let idx = self.ncaps;
                    let n = self.alt()?;
                    self.close()?;
                    Node::Group(Box::new(n), Some(idx))
                }
            }
            '\\' => match self.escape(false)? {
                Ok(ClassItem::Ch(c)) => Node::Char(c),
                Ok(item) => Node::Class(vec![item], false),
                Err(n) => n,
            },
            c => Node::Char(c),
        })
    }
    fn close(&mut self) -> Result<(), String> {
        if self.peek() == Some(')') {
            self.i += 1;
            Ok(())
        } else {
            Err("missing )".into())
        }
    }
}

fn class_matches(items: &[ClassItem], neg: bool, c: char, icase: bool) -> bool {
    let test = |c: char| {
        items.iter().any(|it| match it {
            ClassItem::Ch(x) => *x == c,
            ClassItem::Range(a, b) => *a <= c && c <= *b,
            ClassItem::Prop(n, ng) => unicode::has_property(c, n).unwrap_or(false) ^ ng,
            ClassItem::Digit(ng) => unicode::is_digit(c) ^ ng,
            ClassItem::Word(ng) => unicode::is_word(c) ^ ng,
            ClassItem::Space(ng) => unicode::is_space(c) ^ ng,
            ClassItem::HSpace(ng) => unicode::is_hspace(c) ^ ng,
        })
    };
    let mut r = test(c);
    if !r && icase {
        let l = c.to_lowercase().next().unwrap_or(c);
        let u = c.to_uppercase().next().unwrap_or(c);
        r = test(l) || test(u);
    }
    r ^ neg
}

fn ceq(a: char, b: char, icase: bool) -> bool {
    a == b
        || (icase && a.to_lowercase().next() == b.to_lowercase().next())
}

struct Ctx<'a> {
    t: &'a [char],
    re: &'a Regex,
    steps: usize,
}

type Caps = Vec<Option<(usize, usize)>>;

impl Ctx<'_> {
    fn is_word_at(&self, i: usize) -> bool {
        i < self.t.len() && unicode::is_word(self.t[i])
    }
    /// Match `n` at `pos`, then the continuation.
    fn m(&mut self, n: &Node, pos: usize, caps: &mut Caps, k: &mut dyn FnMut(&mut Self, usize, &mut Caps) -> bool) -> bool {
        self.steps += 1;
        if self.steps > 5_000_000 {
            return false;
        }
        let t = self.t;
        match n {
            Node::Char(c) => pos < t.len() && ceq(t[pos], *c, self.re.icase) && k(self, pos + 1, caps),
            Node::Any => pos < t.len() && (self.re.dotall || t[pos] != '\n') && k(self, pos + 1, caps),
            Node::Class(items, neg) => {
                pos < t.len() && class_matches(items, *neg, t[pos], self.re.icase) && k(self, pos + 1, caps)
            }
            Node::Start => {
                (pos == 0 || (self.re.multiline && t[pos - 1] == '\n')) && k(self, pos, caps)
            }
            Node::End => {
                let ok = pos == t.len()
                    || (pos + 1 == t.len() && t[pos] == '\n')
                    || (self.re.multiline && t[pos] == '\n');
                ok && k(self, pos, caps)
            }
            Node::StrStart => pos == 0 && k(self, pos, caps),
            Node::StrEnd => pos == t.len() && k(self, pos, caps),
            Node::StrEndNl => {
                (pos == t.len() || (pos + 1 == t.len() && t[pos] == '\n')) && k(self, pos, caps)
            }
            Node::WordB(neg) => {
                let a = pos > 0 && self.is_word_at(pos - 1);
                let b = self.is_word_at(pos);
                ((a != b) ^ neg) && k(self, pos, caps)
            }
            Node::Backref(i) => {
                let Some(Some((s, e))) = caps.get(*i).copied() else {
                    return k(self, pos, caps);
                };
                let len = e - s;
                if pos + len > t.len() {
                    return false;
                }
                for j in 0..len {
                    if !ceq(t[s + j], t[pos + j], self.re.icase) {
                        return false;
                    }
                }
                k(self, pos + len, caps)
            }
            Node::Group(inner, idx) => {
                if let Some(idx) = *idx {
                    let old = caps[idx];
                    let start = pos;
                    let r = self.m(inner, pos, caps, &mut |s: &mut Self, p, c: &mut Caps| {
                        let o = c[idx];
                        c[idx] = Some((start, p));
                        if k(s, p, c) {
                            return true;
                        }
                        c[idx] = o;
                        false
                    });
                    if !r {
                        caps[idx] = old;
                    }
                    r
                } else {
                    self.m(inner, pos, caps, k)
                }
            }
            Node::Look(inner, ahead, neg) => {
                let mut c2 = caps.clone();
                let found = if *ahead {
                    self.m(inner, pos, &mut c2, &mut |_, _, _| true)
                } else {
                    // fixed or bounded look-behind: try every start
                    let mut f = false;
                    for s in (0..=pos).rev() {
                        let mut c3 = caps.clone();
                        if self.m(inner, s, &mut c3, &mut |_, p, _| p == pos) {
                            c2 = c3;
                            f = true;
                            break;
                        }
                    }
                    f
                };
                if found == *neg {
                    return false;
                }
                if found && !*neg {
                    let mut c2 = c2;
                    k(self, pos, &mut c2) && {
                        *caps = c2;
                        true
                    }
                } else {
                    k(self, pos, caps)
                }
            }
            Node::Alt(alts) => {
                for a in alts {
                    if self.m(a, pos, caps, k) {
                        return true;
                    }
                }
                false
            }
            Node::Seq(items) => self.seq(items, 0, pos, caps, k),
            Node::Repeat(inner, min, max, greedy) => self.rep(inner, *min, *max, *greedy, 0, pos, usize::MAX, caps, k),
        }
    }
    fn seq(&mut self, items: &[Node], i: usize, pos: usize, caps: &mut Caps, k: &mut dyn FnMut(&mut Self, usize, &mut Caps) -> bool) -> bool {
        if i == items.len() {
            return k(self, pos, caps);
        }
        let (first, _) = items.split_at(i + 1);
        let node = &first[i];
        self.m(node, pos, caps, &mut |s: &mut Self, p, c: &mut Caps| s.seq(items, i + 1, p, c, k))
    }
    #[allow(clippy::too_many_arguments)]
    fn rep(&mut self, inner: &Node, min: usize, max: usize, greedy: bool, count: usize, pos: usize, last: usize, caps: &mut Caps, k: &mut dyn FnMut(&mut Self, usize, &mut Caps) -> bool) -> bool {
        let can_more = count < max && !(count >= min && last == pos);
        if count >= min && !greedy && k(self, pos, caps) {
            return true;
        }
        if can_more
            && self.m(inner, pos, caps, &mut |s: &mut Self, p, c: &mut Caps| {
                if p == pos && count >= min {
                    return false; // zero-length iteration
                }
                s.rep(inner, min, max, greedy, count + 1, p, pos, c, k)
            })
        {
            return true;
        }
        count >= min && greedy && k(self, pos, caps)
    }
}

impl Regex {
    /// Compile a Perl pattern with flags (any of `imsx`).
    pub fn new(pat: &str, flags: &str) -> Result<Regex, String> {
        let chars: Vec<char> = pat.chars().collect();
        let mut p = Parser { p: &chars, i: 0, ncaps: 0, extended: flags.contains('x') };
        let node = p.alt()?;
        if p.i != chars.len() {
            return Err(alloc::format!("cannot parse pattern '{pat}'"));
        }
        Ok(Regex {
            node,
            ncaps: p.ncaps,
            icase: flags.contains('i'),
            multiline: flags.contains('m'),
            dotall: flags.contains('s'),
        })
    }

    /// The first match starting at or after char index `from`.
    pub fn find_at(&self, t: &[char], from: usize) -> Option<Match> {
        let mut ctx = Ctx { t, re: self, steps: 0 };
        for start in from..=t.len() {
            let mut caps: Caps = vec![None; self.ncaps + 1];
            let mut end = None;
            if ctx.m(&self.node, start, &mut caps, &mut |_, p, c: &mut Caps| {
                end = Some((p, c.clone()));
                true
            }) {
                let (e, c) = end.unwrap();
                let mut c = c;
                c[0] = Some((start, e));
                return Some(Match { start, end: e, caps: c });
            }
        }
        None
    }

    pub fn is_match(&self, s: &str) -> bool {
        let t: Vec<char> = s.chars().collect();
        self.find_at(&t, 0).is_some()
    }

    /// All non-overlapping matches, as Perl's `m//g` finds them.
    pub fn find_all(&self, t: &[char]) -> Vec<Match> {
        let mut out = Vec::new();
        let mut from = 0;
        let mut last_empty_at = None;
        while from <= t.len() {
            let Some(m) = self.find_at(t, from) else { break };
            if m.start == m.end && last_empty_at == Some(m.start) {
                // Perl does not allow an empty match where the last match ended
                if m.start >= t.len() {
                    break;
                }
                from = m.start + 1;
                last_empty_at = None;
                continue;
            }
            from = m.end;
            last_empty_at = if m.end == m.start { Some(m.end) } else { None };
            out.push(m);
            if from > t.len() {
                break;
            }
        }
        out
    }

    /// `s/re/f(match)/g` (or the first only, `global` false).
    pub fn replace(&self, s: &str, global: bool, f: &mut dyn FnMut(&[char], &Match) -> String) -> String {
        let t: Vec<char> = s.chars().collect();
        let ms = if global {
            self.find_all(&t)
        } else {
            self.find_at(&t, 0).into_iter().collect()
        };
        let mut o = String::new();
        let mut last = 0;
        for m in &ms {
            o.extend(&t[last..m.start]);
            o.push_str(&f(&t, m));
            last = m.end;
        }
        o.extend(&t[last..]);
        o
    }

    /// `s/re//g`.
    pub fn delete_all(&self, s: &str) -> String {
        self.replace(s, true, &mut |_, _| String::new())
    }
}

/// The text of a capture.
pub fn cap(t: &[char], m: &Match, i: usize) -> Option<String> {
    m.caps.get(i).copied().flatten().map(|(s, e)| t[s..e].iter().collect())
}
