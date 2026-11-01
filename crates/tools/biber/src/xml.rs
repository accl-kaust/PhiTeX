//! Just enough XML for the `.bcf` control file and `recode_data.xml`:
//! elements (namespace prefixes stripped, as XML::LibXML::Simple's
//! `NsStrip`), attributes, text and CDATA; comments, processing
//! instructions and the doctype are skipped.

use alloc::string::String;
use alloc::vec::Vec;

#[derive(Clone, Debug, Default)]
pub struct Element {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Element>,
    /// The element's own text (its text children, concatenated).
    pub text: String,
}

impl Element {
    pub fn attr(&self, n: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == n)
            .map(|(_, v)| v.as_str())
    }
    pub fn kids<'a>(&'a self, n: &'a str) -> impl Iterator<Item = &'a Element> + 'a {
        self.children.iter().filter(move |c| c.name == n)
    }
    pub fn kid(&self, n: &str) -> Option<&Element> {
        self.children.iter().find(|c| c.name == n)
    }
    /// `order` attribute as a number (0 if absent).
    pub fn order(&self) -> i64 {
        self.attr("order")
            .and_then(|o| o.trim().parse().ok())
            .unwrap_or(0)
    }
}

fn strip_ns(n: &str) -> String {
    match n.rfind(':') {
        Some(i) => n[i + 1..].into(),
        None => n.into(),
    }
}

fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.into();
    }
    let mut o = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        o.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(j) = rest.find(';') else {
            o.push_str(rest);
            return o;
        };
        let ent = &rest[1..j];
        match ent {
            "lt" => o.push('<'),
            "gt" => o.push('>'),
            "amp" => o.push('&'),
            "quot" => o.push('"'),
            "apos" => o.push('\''),
            _ => {
                let v = if let Some(h) = ent.strip_prefix("#x").or_else(|| ent.strip_prefix("#X")) {
                    u32::from_str_radix(h, 16).ok()
                } else if let Some(d) = ent.strip_prefix('#') {
                    d.parse().ok()
                } else {
                    None
                };
                match v.and_then(char::from_u32) {
                    Some(c) => o.push(c),
                    None => o.push_str(&rest[..=j]),
                }
            }
        }
        rest = &rest[j + 1..];
    }
    o.push_str(rest);
    o
}

/// Parse a document; the root element.
pub fn parse(s: &str) -> Result<Element, String> {
    let b = s.as_bytes();
    let mut i = 0;
    let mut stack: Vec<Element> = Vec::new();
    let mut root: Option<Element> = None;
    while i < b.len() {
        if b[i] == b'<' {
            if s[i..].starts_with("<!--") {
                let e = s[i..].find("-->").ok_or("unterminated comment")?;
                i += e + 3;
                continue;
            }
            if s[i..].starts_with("<![CDATA[") {
                let e = s[i..].find("]]>").ok_or("unterminated CDATA")?;
                if let Some(top) = stack.last_mut() {
                    top.text.push_str(&s[i + 9..i + e]);
                }
                i += e + 3;
                continue;
            }
            if s[i..].starts_with("<?") {
                let e = s[i..].find("?>").ok_or("unterminated PI")?;
                i += e + 2;
                continue;
            }
            if s[i..].starts_with("<!") {
                let e = s[i..].find('>').ok_or("unterminated declaration")?;
                i += e + 1;
                continue;
            }
            if s[i..].starts_with("</") {
                let e = s[i..].find('>').ok_or("unterminated end tag")?;
                let name = strip_ns(s[i + 2..i + e].trim());
                i += e + 1;
                let el = stack.pop().ok_or("unbalanced end tag")?;
                if el.name != name {
                    return Err(alloc::format!("mismatched </{name}>"));
                }
                match stack.last_mut() {
                    Some(p) => p.children.push(el),
                    None => root = Some(el),
                }
                continue;
            }
            // start tag
            let mut j = i + 1;
            while j < b.len() && !b[j].is_ascii_whitespace() && b[j] != b'>' && b[j] != b'/' {
                j += 1;
            }
            let mut el = Element {
                name: strip_ns(&s[i + 1..j]),
                ..Element::default()
            };
            loop {
                while j < b.len() && b[j].is_ascii_whitespace() {
                    j += 1;
                }
                if j >= b.len() {
                    return Err("unterminated tag".into());
                }
                if b[j] == b'>' {
                    j += 1;
                    stack.push(el);
                    break;
                }
                if b[j] == b'/' {
                    j += 2;
                    match stack.last_mut() {
                        Some(p) => p.children.push(el),
                        None => root = Some(el),
                    }
                    break;
                }
                let ks = j;
                while j < b.len() && b[j] != b'=' && !b[j].is_ascii_whitespace() {
                    j += 1;
                }
                let key = strip_ns(&s[ks..j]);
                while j < b.len() && (b[j].is_ascii_whitespace() || b[j] == b'=') {
                    j += 1;
                }
                let q = b[j];
                if q != b'"' && q != b'\'' {
                    return Err("bad attribute".into());
                }
                let vs = j + 1;
                let ve = vs + s[vs..].find(q as char).ok_or("unterminated attribute")?;
                el.attrs.push((key, unescape(&s[vs..ve])));
                j = ve + 1;
            }
            i = j;
        } else {
            let e = s[i..].find('<').map_or(b.len(), |e| i + e);
            if let Some(top) = stack.last_mut() {
                top.text.push_str(&unescape(&s[i..e]));
            }
            i = e;
        }
    }
    root.ok_or_else(|| "no root element".into())
}
