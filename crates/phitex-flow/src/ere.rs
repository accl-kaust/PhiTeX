//! POSIX extended regular expressions, as `\pdfmatch` uses them (pdfTeX's
//! `matchstrings`: `regcomp(REG_EXTENDED)`, `regexec`): whether a pattern
//! matches somewhere in a string, and where (`\pdflastmatch`): the match
//! is POSIX's leftmost-longest; a group's submatch is the one the first
//! path to that match takes (POSIX's own subexpression rules are not
//! copied: CORE.md).

/// A parsed expression.
#[derive(Debug)]
enum Node {
    Char(char),
    Any,
    /// A bracket expression: its ranges, negated.
    Class(Vec<(char, char)>, bool),
    Start,
    End,
    Alt(Vec<Node>),
    Concat(Vec<Node>),
    Repeat(Box<Node>, u32, Option<u32>),
    /// A parenthesized subexpression: its number (from 1).
    Group(Box<Node>, usize),
}

struct Parser<'a> {
    p: &'a [char],
    i: usize,
    groups: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<char> {
        self.p.get(self.i).copied()
    }

    fn alt(&mut self) -> Option<Node> {
        let mut v = vec![self.concat()?];
        while self.peek() == Some('|') {
            self.i += 1;
            v.push(self.concat()?);
        }
        Some(if v.len() == 1 { v.pop()? } else { Node::Alt(v) })
    }

    fn concat(&mut self) -> Option<Node> {
        let mut v = Vec::new();
        while let Some(c) = self.peek() {
            if c == '|' || c == ')' {
                break;
            }
            let mut a = self.atom()?;
            loop {
                match self.peek() {
                    Some('*') => {
                        self.i += 1;
                        a = Node::Repeat(Box::new(a), 0, None);
                    }
                    Some('+') => {
                        self.i += 1;
                        a = Node::Repeat(Box::new(a), 1, None);
                    }
                    Some('?') => {
                        self.i += 1;
                        a = Node::Repeat(Box::new(a), 0, Some(1));
                    }
                    Some('{') if self.p.get(self.i + 1).is_some_and(char::is_ascii_digit) => {
                        self.i += 1;
                        let min = self.num()?;
                        let max = if self.peek() == Some(',') {
                            self.i += 1;
                            if self.peek() == Some('}') { None } else { Some(self.num()?) }
                        } else {
                            Some(min)
                        };
                        if self.peek() != Some('}') || max.is_some_and(|m| m < min) {
                            return None;
                        }
                        self.i += 1;
                        a = Node::Repeat(Box::new(a), min, max);
                    }
                    _ => break,
                }
            }
            v.push(a);
        }
        Some(Node::Concat(v))
    }

    fn num(&mut self) -> Option<u32> {
        let mut n: u32 = 0;
        let mut any = false;
        while let Some(d) = self.peek().and_then(|c| c.to_digit(10)) {
            n = n.checked_mul(10)?.checked_add(d)?;
            self.i += 1;
            any = true;
        }
        any.then_some(n)
    }

    fn atom(&mut self) -> Option<Node> {
        let c = self.peek()?;
        self.i += 1;
        Some(match c {
            '.' => Node::Any,
            '^' => Node::Start,
            '$' => Node::End,
            '(' => {
                self.groups += 1;
                let g = self.groups;
                let n = self.alt()?;
                if self.peek() != Some(')') {
                    return None;
                }
                self.i += 1;
                Node::Group(Box::new(n), g)
            }
            '[' => self.bracket()?,
            '\\' => Node::Char(self.peek().inspect(|_| self.i += 1)?),
            '*' | '+' | '?' => return None,
            c => Node::Char(c),
        })
    }

    fn bracket(&mut self) -> Option<Node> {
        let mut neg = false;
        if self.peek() == Some('^') {
            neg = true;
            self.i += 1;
        }
        let mut ranges = Vec::new();
        let mut first = true;
        loop {
            let c = self.peek()?;
            self.i += 1;
            if c == ']' && !first {
                break;
            }
            first = false;
            if c == '[' && self.peek() == Some(':') {
                let rest: String = self.p[self.i + 1..].iter().collect();
                let end = rest.find(":]")?;
                let name = &rest[..end];
                self.i += 1 + end + 2;
                let r: &[(char, char)] = match name {
                    "digit" => &[('0', '9')],
                    "alpha" => &[('a', 'z'), ('A', 'Z')],
                    "alnum" => &[('a', 'z'), ('A', 'Z'), ('0', '9')],
                    "upper" => &[('A', 'Z')],
                    "lower" => &[('a', 'z')],
                    "space" => &[(' ', ' '), ('\t', '\r')],
                    "blank" => &[(' ', ' '), ('\t', '\t')],
                    "xdigit" => &[('0', '9'), ('a', 'f'), ('A', 'F')],
                    "punct" => &[('!', '/'), (':', '@'), ('[', '`'), ('{', '~')],
                    "print" => &[(' ', '~')],
                    "graph" => &[('!', '~')],
                    "cntrl" => &[('\0', '\u{1f}'), ('\u{7f}', '\u{7f}')],
                    _ => return None,
                };
                ranges.extend_from_slice(r);
                continue;
            }
            if self.peek() == Some('-') && self.p.get(self.i + 1).is_some_and(|&d| d != ']') {
                let hi = self.p[self.i + 1];
                self.i += 2;
                if hi < c {
                    return None;
                }
                ranges.push((c, hi));
            } else {
                ranges.push((c, c));
            }
        }
        Some(Node::Class(ranges, neg))
    }
}

struct Matcher<'a> {
    s: &'a [char],
    icase: bool,
    /// Each group's submatch on the path being tried.
    caps: std::cell::RefCell<Vec<Option<(usize, usize)>>>,
}

impl Matcher<'_> {
    fn eq(&self, a: char, b: char) -> bool {
        a == b || (self.icase && a.to_lowercase().eq(b.to_lowercase()))
    }

    fn class(&self, r: &[(char, char)], neg: bool, c: char) -> bool {
        let inr = |c: char| r.iter().any(|&(lo, hi)| lo <= c && c <= hi);
        let hit = inr(c) || (self.icase && (inr(c.to_ascii_lowercase()) || inr(c.to_ascii_uppercase())));
        hit != neg
    }

    /// Whether `n` matches at `i` with the continuation `k` matching after.
    fn m(&self, n: &Node, i: usize, k: &mut dyn FnMut(usize) -> bool) -> bool {
        match n {
            Node::Char(c) => i < self.s.len() && self.eq(self.s[i], *c) && k(i + 1),
            Node::Any => i < self.s.len() && k(i + 1),
            Node::Class(r, neg) => i < self.s.len() && self.class(r, *neg, self.s[i]) && k(i + 1),
            Node::Start => i == 0 && k(i),
            Node::End => i == self.s.len() && k(i),
            Node::Alt(v) => v.iter().any(|x| self.m(x, i, k)),
            Node::Concat(v) => self.seq(v, i, k),
            Node::Repeat(x, min, max) => self.rep(x, *min, *max, 0, i, k),
            Node::Group(x, g) => {
                let was = self.caps.borrow().get(*g).copied().flatten();
                let hit = self.m(x, i, &mut |j| {
                    let old = self.caps.borrow_mut()[*g].replace((i, j));
                    if k(j) {
                        return true;
                    }
                    self.caps.borrow_mut()[*g] = old;
                    false
                });
                if !hit {
                    self.caps.borrow_mut()[*g] = was;
                }
                hit
            }
        }
    }

    fn seq(&self, v: &[Node], i: usize, k: &mut dyn FnMut(usize) -> bool) -> bool {
        match v.split_first() {
            None => k(i),
            Some((h, t)) => self.m(h, i, &mut |j| self.seq(t, j, k)),
        }
    }

    fn rep(&self, x: &Node, min: u32, max: Option<u32>, n: u32, i: usize, k: &mut dyn FnMut(usize) -> bool) -> bool {
        if max.is_none_or(|m| n < m)
            && self.m(x, i, &mut |j| (j > i || n < min) && self.rep(x, min, max, n + 1, j, k))
        {
            return true;
        }
        n >= min && k(i)
    }
}

fn parse(pattern: &str) -> Option<(Node, usize)> {
    let p: Vec<char> = pattern.chars().collect();
    let mut parser = Parser { p: &p, i: 0, groups: 0 };
    let n = parser.alt()?;
    (parser.i == p.len()).then_some((n, parser.groups))
}

/// Whether `pattern` matches somewhere in `s` (`None`: not a valid
/// expression, `regcomp`'s error).
pub fn is_match(pattern: &str, s: &str, icase: bool) -> Option<bool> {
    let (n, groups) = parse(pattern)?;
    let s: Vec<char> = s.chars().collect();
    let m = Matcher { s: &s, icase, caps: std::cell::RefCell::new(vec![None; groups + 1]) };
    Some((0..=s.len()).any(|i| m.m(&n, i, &mut |_| true)))
}

/// Each subexpression's match (start, end), the whole one first.
pub type Caps = Vec<Option<(usize, usize)>>;

/// Where `pattern` matches in `s` (`None`: not a valid expression; then
/// `None`: no match): the whole match (index 0, leftmost-longest) and each
/// group's submatch, in characters.
pub fn find(pattern: &str, s: &str, icase: bool) -> Option<Option<Caps>> {
    let (n, groups) = parse(pattern)?;
    let s: Vec<char> = s.chars().collect();
    let m = Matcher { s: &s, icase, caps: std::cell::RefCell::new(vec![None; groups + 1]) };
    for start in 0..=s.len() {
        let mut best: Option<(usize, Caps)> = None;
        m.m(&n, start, &mut |j| {
            if best.as_ref().is_none_or(|(e, _)| j > *e) {
                best = Some((j, m.caps.borrow().clone()));
            }
            false
        });
        if let Some((end, mut caps)) = best {
            caps[0] = Some((start, end));
            return Some(Some(caps));
        }
    }
    Some(None)
}

#[cfg(test)]
mod tests {
    use super::{find, is_match};

    #[test]
    fn microtypes_patterns() {
        let int = "^-*[0-9]+ *$";
        assert_eq!(is_match(int, "12", false), Some(true));
        assert_eq!(is_match(int, "--3  ", false), Some(true));
        assert_eq!(is_match(int, "1.5", false), Some(false));
        let dim = "^([0-9]+([.,][0-9]+)?|[.,][0-9]+)(em|ex|cm|mm|in|pc|pt|dd|cc|bp|sp|nd|nc|px)? *$";
        assert_eq!(is_match(dim, "1.5pt", false), Some(true));
        assert_eq!(is_match(dim, ",5", false), Some(true));
        assert_eq!(is_match(dim, "1.5 pt", false), Some(false));
        assert_eq!(is_match("a{2,3}", "xaay", false), Some(true));
        assert_eq!(is_match("A[[:digit:]]", "xa1", true), Some(true));
        assert_eq!(is_match("(", "x", false), None);
        let f = find("b(c+)d?", "abccdx", false).unwrap().unwrap();
        assert_eq!(f[0], Some((1, 5)));
        assert_eq!(f[1], Some((2, 4)));
        assert_eq!(find("z", "abc", false), Some(None));
    }
}
