//! A JSON reader for the live viewer's requests (DESIGN 4.8): small
//! objects from the browser, read into [`Value`]s. Writing JSON is
//! `format!` and [`crate::origins::json_str`] elsewhere.

/// A JSON value (an object keeps its keys in order).
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Value>),
    Obj(Vec<(String, Value)>),
}

impl Value {
    /// Member `key` of an object.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Obj(m) => m.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    #[must_use]
    pub fn str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    #[must_use]
    pub fn bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    #[must_use]
    pub fn num(&self) -> Option<f64> {
        match self {
            Value::Num(n) => Some(*n),
            _ => None,
        }
    }

    /// A number as an index or count (none if negative or not whole).
    #[must_use]
    pub fn index(&self) -> Option<usize> {
        let n = self.num()?;
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "checked whole and in range"
        )]
        (n >= 0.0 && n.fract() == 0.0 && n < 1e15).then_some(n as usize)
    }
}

/// Parse `s` (one value, nothing but white space after it).
#[must_use]
pub fn parse(s: &str) -> Option<Value> {
    let mut p = Parser {
        b: s.as_bytes(),
        i: 0,
    };
    let v = p.value(0)?;
    p.ws();
    (p.i == p.b.len()).then_some(v)
}

struct Parser<'a> {
    b: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while self.b.get(self.i).is_some_and(u8::is_ascii_whitespace) {
            self.i += 1;
        }
    }

    fn eat(&mut self, c: u8) -> bool {
        self.ws();
        if self.b.get(self.i) == Some(&c) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    fn lit(&mut self, word: &[u8], v: Value) -> Option<Value> {
        self.b[self.i..].starts_with(word).then(|| {
            self.i += word.len();
            v
        })
    }

    fn value(&mut self, depth: u32) -> Option<Value> {
        if depth > 64 {
            return None;
        }
        self.ws();
        match *self.b.get(self.i)? {
            b'{' => {
                self.i += 1;
                let mut m = Vec::new();
                if self.eat(b'}') {
                    return Some(Value::Obj(m));
                }
                loop {
                    self.ws();
                    let k = self.string()?;
                    if !self.eat(b':') {
                        return None;
                    }
                    m.push((k, self.value(depth + 1)?));
                    if self.eat(b'}') {
                        return Some(Value::Obj(m));
                    }
                    if !self.eat(b',') {
                        return None;
                    }
                }
            }
            b'[' => {
                self.i += 1;
                let mut a = Vec::new();
                if self.eat(b']') {
                    return Some(Value::Arr(a));
                }
                loop {
                    a.push(self.value(depth + 1)?);
                    if self.eat(b']') {
                        return Some(Value::Arr(a));
                    }
                    if !self.eat(b',') {
                        return None;
                    }
                }
            }
            b'"' => self.string().map(Value::Str),
            b't' => self.lit(b"true", Value::Bool(true)),
            b'f' => self.lit(b"false", Value::Bool(false)),
            b'n' => self.lit(b"null", Value::Null),
            _ => {
                let s = self.i;
                while self
                    .b
                    .get(self.i)
                    .is_some_and(|c| c.is_ascii_digit() || b"+-.eE".contains(c))
                {
                    self.i += 1;
                }
                std::str::from_utf8(&self.b[s..self.i])
                    .ok()?
                    .parse()
                    .ok()
                    .map(Value::Num)
            }
        }
    }

    fn hex4(&mut self) -> Option<u32> {
        let h = std::str::from_utf8(self.b.get(self.i..self.i + 4)?).ok()?;
        self.i += 4;
        u32::from_str_radix(h, 16).ok()
    }

    fn string(&mut self) -> Option<String> {
        if self.b.get(self.i) != Some(&b'"') {
            return None;
        }
        self.i += 1;
        let mut out = Vec::new();
        loop {
            let c = *self.b.get(self.i)?;
            self.i += 1;
            match c {
                b'"' => return String::from_utf8(out).ok(),
                b'\\' => {
                    let e = *self.b.get(self.i)?;
                    self.i += 1;
                    let ch = match e {
                        b'n' => '\n',
                        b't' => '\t',
                        b'r' => '\r',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'u' => {
                            let mut u = self.hex4()?;
                            // (a surrogate pair)
                            if (0xD800..0xDC00).contains(&u)
                                && self.b.get(self.i..self.i + 2) == Some(b"\\u")
                            {
                                self.i += 2;
                                let lo = self.hex4()?;
                                u = 0x10000
                                    + ((u - 0xD800) << 10)
                                    + (lo.wrapping_sub(0xDC00) & 0x3ff);
                            }
                            char::from_u32(u).unwrap_or('\u{FFFD}')
                        }
                        e => char::from(e),
                    };
                    let mut b = [0; 4];
                    out.extend_from_slice(ch.encode_utf8(&mut b).as_bytes());
                }
                c => out.push(c),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_requests() {
        let v = parse(r#"{"id":3,"op":"edit","file":"a.tex","start":0,"text":"x\"é😀","ok":true,"n":null,"a":[1,-2.5e1]}"#).unwrap();
        assert_eq!(v.get("id").and_then(Value::index), Some(3));
        assert_eq!(v.get("op").and_then(Value::str), Some("edit"));
        assert_eq!(v.get("text").and_then(Value::str), Some("x\"é😀"));
        assert_eq!(v.get("ok"), Some(&Value::Bool(true)));
        assert_eq!(
            v.get("a"),
            Some(&Value::Arr(vec![Value::Num(1.0), Value::Num(-25.0)]))
        );
        assert_eq!(parse("{\"a\":1} x"), None);
        assert_eq!(parse("[1,"), None);
    }
}
