//! btparse (Text::BibTeX 0.91) as biber uses it: the lexer and grammar of
//! `bibtex.g` / `lex_auxiliary.c`, string post-processing
//! (`postprocess.c`, BTO_FULL for entries and BTO_MACRO for `@string`),
//! the macro table (`macros.c`), `bt_split_list` and `bt_split_name`
//! (`names.c`) and single-part name formatting (`format_name.c`).

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use crate::unicode;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Meta {
    Regular,
    Comment,
    Preamble,
    MacroDef,
}

/// A parsed entry (Text::BibTeX::Entry).
#[derive(Clone, Debug)]
pub struct BtEntry {
    pub meta: Meta,
    pub typ: String,
    pub key: Option<String>,
    /// Field names in order (duplicates kept, as Text::BibTeX does).
    pub fieldlist: Vec<String>,
    pub values: BTreeMap<String, String>,
    /// Value of a preamble or comment.
    pub value: String,
    pub line: usize,
}

impl BtEntry {
    pub fn get(&self, f: &str) -> Option<&str> {
        self.values.get(f).map(String::as_str)
    }
    pub fn exists(&self, f: &str) -> bool {
        self.values.contains_key(f)
    }
    pub fn set(&mut self, f: &str, v: &str) {
        if !self.values.contains_key(f) {
            self.fieldlist.push(f.into());
        }
        self.values.insert(f.into(), v.into());
    }
    pub fn delete(&mut self, f: &str) {
        self.values.remove(f);
        self.fieldlist.retain(|x| x != f);
    }
}

/// The macro table (case-sensitive names; `@string` names are lowercased
/// when defined).
#[derive(Clone, Debug, Default)]
pub struct Macros(pub BTreeMap<String, String>);

#[derive(Clone, Debug)]
enum Simple {
    Str(String),
    Num(String),
    Macro(String),
}

fn is_name_char(c: char) -> bool {
    let u = c as u32;
    if u >= 0x80 {
        return true;
    }
    if u < 0x20 || u == 0x7f {
        return false;
    }
    !matches!(c, ' ' | '"' | '#' | '%' | '\'' | '(' | ')' | ',' | '=' | '\\' | '{' | '}')
}

struct Lexer<'a> {
    t: &'a [char],
    i: usize,
    line: usize,
}

/// A parse failure: btparse prints an `error:` line, which biber turns
/// into a fatal error.
pub type Error = String;

impl Lexer<'_> {
    fn peek(&self) -> Option<char> {
        self.t.get(self.i).copied()
    }
    /// Skip whitespace and `%` comments inside an entry.
    fn skip_ws(&mut self) {
        loop {
            match self.peek() {
                Some('\n') => {
                    self.line += 1;
                    self.i += 1;
                }
                Some(' ' | '\r' | '\t') => self.i += 1,
                Some('%') => {
                    // COMMENT "%~[\n]*\n" (only with a newline)
                    let mut j = self.i;
                    while j < self.t.len() && self.t[j] != '\n' {
                        j += 1;
                    }
                    if j < self.t.len() {
                        self.i = j + 1;
                        self.line += 1;
                    } else {
                        return;
                    }
                }
                _ => return,
            }
        }
    }
    fn name(&mut self) -> Option<String> {
        let s = self.i;
        while self.peek().is_some_and(is_name_char) {
            self.i += 1;
        }
        if self.i == s {
            None
        } else {
            Some(self.t[s..self.i].iter().collect())
        }
    }
    /// A delimited string starting at `{`, `"` or `(`; returns its
    /// content with whitespace characters converted to spaces.
    fn string(&mut self) -> Result<String, Error> {
        let open = self.t[self.i];
        self.i += 1;
        let mut depth: i64 = if open == '{' { 1 } else { 0 };
        let mut pdepth: i64 = if open == '(' { 1 } else { 0 };
        let mut o = String::new();
        loop {
            let Some(c) = self.peek() else {
                return Err(alloc::format!("line {}: unterminated string", self.line));
            };
            self.i += 1;
            match c {
                '{' => {
                    depth += 1;
                    o.push(c);
                }
                '}' => {
                    depth -= 1;
                    if open == '{' && depth == 0 {
                        return Ok(o);
                    }
                    if depth < 0 {
                        return Err(alloc::format!("line {}: unbalanced braces: too many }}'s", self.line));
                    }
                    o.push(c);
                }
                '(' => {
                    pdepth += 1;
                    o.push(c);
                }
                ')' => {
                    pdepth -= 1;
                    if open == '(' && pdepth == 0 {
                        return Ok(o);
                    }
                    o.push(c);
                }
                '"' => {
                    if open == '"' && depth == 0 {
                        return Ok(o);
                    }
                    o.push(c);
                }
                '\n' => {
                    self.line += 1;
                    o.push(' ');
                }
                '\t' | '\x0b' | '\x0c' | '\r' => o.push(' '),
                c => o.push(c),
            }
        }
    }
    fn value(&mut self) -> Result<Vec<Simple>, Error> {
        let mut v = Vec::new();
        loop {
            self.skip_ws();
            match self.peek() {
                Some('{' | '"') => v.push(Simple::Str(self.string()?)),
                Some(c) if is_name_char(c) || c.is_ascii_digit() => {
                    let n = self.name().unwrap_or_default();
                    if n.chars().all(|c| c.is_ascii_digit()) {
                        v.push(Simple::Num(n));
                    } else {
                        v.push(Simple::Macro(n));
                    }
                }
                _ => return Err(alloc::format!("line {}: syntax error: expected a value", self.line)),
            }
            self.skip_ws();
            if self.peek() == Some('#') {
                self.i += 1;
                continue;
            }
            return Ok(v);
        }
    }
}

/// `bt_postprocess_string`: whitespace collapsing (spaces only).
pub fn collapse(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    let mut prev_space = true;
    for c in s.chars() {
        if c == '\r' {
            continue;
        }
        if c == ' ' {
            if prev_space {
                continue;
            }
            prev_space = true;
        } else {
            prev_space = false;
        }
        o.push(c);
    }
    if o.ends_with(' ') {
        o.pop();
    }
    o
}

impl Macros {
    fn expand(&self, v: &[Simple], collapse_ws: bool, warns: &mut Vec<String>, line: usize) -> String {
        let mut o = String::new();
        for s in v {
            match s {
                Simple::Str(x) | Simple::Num(x) => o.push_str(x),
                Simple::Macro(m) => match self.0.get(m) {
                    Some(t) => o.push_str(t),
                    None => warns.push(alloc::format!("line {line}, warning: undefined macro \"{m}\"")),
                },
            }
        }
        if collapse_ws {
            collapse(&o)
        } else {
            o.replace('\r', "")
        }
    }
}

/// Parse a whole `.bib` text into entries (each value fully processed),
/// defining `@string` macros as they come.
pub fn parse(text: &str, macros: &mut Macros, warns: &mut Vec<String>) -> Result<Vec<BtEntry>, Error> {
    let t: Vec<char> = text.chars().collect();
    let mut lx = Lexer { t: &t, i: 0, line: 1 };
    let mut out = Vec::new();
    loop {
        // toplevel
        loop {
            match lx.peek() {
                None => return Ok(out),
                Some('@') => break,
                Some('\n') => {
                    lx.line += 1;
                    lx.i += 1;
                }
                Some(' ' | '\r' | '\t') => lx.i += 1,
                Some('%') => {
                    let mut j = lx.i;
                    while j < t.len() && t[j] != '\n' {
                        j += 1;
                    }
                    if j < t.len() {
                        lx.i = j + 1;
                        lx.line += 1;
                    } else {
                        lx.i = j;
                    }
                }
                Some(_) => {
                    while lx.peek().is_some_and(|c| !matches!(c, '@' | '\n' | ' ' | '\r' | '\t')) {
                        lx.i += 1;
                    }
                }
            }
        }
        lx.i += 1; // '@'
        let line = lx.line;
        lx.skip_ws();
        let Some(typ) = lx.name() else {
            return Err(alloc::format!("line {}: syntax error: expected an entry type", lx.line));
        };
        let ltyp = typ.to_ascii_lowercase();
        let meta = match ltyp.as_str() {
            "comment" => Meta::Comment,
            "preamble" => Meta::Preamble,
            "string" => Meta::MacroDef,
            "alias" | "modify" => return Err(alloc::format!("line {line}: unsupported entry type @{typ}")),
            _ => Meta::Regular,
        };
        lx.skip_ws();
        if meta == Meta::Comment {
            match lx.peek() {
                Some('{' | '(') => {
                    let v = lx.string()?;
                    out.push(BtEntry {
                        meta,
                        typ: ltyp,
                        key: None,
                        fieldlist: Vec::new(),
                        values: BTreeMap::new(),
                        value: v,
                        line,
                    });
                    continue;
                }
                _ => return Err(alloc::format!("line {}: comment entries must be delimited by either braces or parentheses", lx.line)),
            }
        }
        let close = match lx.peek() {
            Some('{') => '}',
            Some('(') => ')',
            _ => return Err(alloc::format!("line {}: syntax error: expected entry opener", lx.line)),
        };
        lx.i += 1;
        let mut e = BtEntry {
            meta,
            typ: ltyp,
            key: None,
            fieldlist: Vec::new(),
            values: BTreeMap::new(),
            value: String::new(),
            line,
        };
        lx.skip_ws();
        match meta {
            Meta::Regular => {
                let Some(k) = lx.name() else {
                    return Err(alloc::format!("line {}: syntax error: expected entry key", lx.line));
                };
                e.key = Some(k);
                lx.skip_ws();
                if lx.peek() == Some(',') {
                    lx.i += 1;
                } else if lx.peek() != Some(close) {
                    return Err(alloc::format!("line {}: syntax error: expected \",\"", lx.line));
                }
                fields(&mut lx, &mut e, macros, warns, true)?;
            }
            Meta::MacroDef => {
                fields(&mut lx, &mut e, macros, warns, false)?;
            }
            Meta::Preamble => {
                let v = lx.value()?;
                let mut o = String::new();
                for s in &v {
                    match s {
                        Simple::Str(x) | Simple::Num(x) | Simple::Macro(x) => o.push_str(x),
                    }
                }
                e.value = o;
            }
            Meta::Comment => {}
        }
        lx.skip_ws();
        match lx.peek() {
            Some(c) if c == close || c == '}' || c == ')' => lx.i += 1,
            _ => return Err(alloc::format!("line {}: syntax error: expected entry closer", lx.line)),
        }
        out.push(e);
    }
}

fn fields(lx: &mut Lexer<'_>, e: &mut BtEntry, macros: &mut Macros, warns: &mut Vec<String>, full: bool) -> Result<(), Error> {
    loop {
        lx.skip_ws();
        match lx.peek() {
            Some('}' | ')') | None => return Ok(()),
            _ => {}
        }
        let Some(f) = lx.name() else {
            return Err(alloc::format!("line {}: syntax error: expected a field name", lx.line));
        };
        let f = f.to_ascii_lowercase();
        lx.skip_ws();
        if lx.peek() != Some('=') {
            return Err(alloc::format!("line {}: syntax error: expected \"=\"", lx.line));
        }
        lx.i += 1;
        let line = lx.line;
        let v = lx.value()?;
        let s = macros.expand(&v, full, warns, line);
        if !full {
            macros.0.insert(f.clone(), s.clone());
        }
        e.fieldlist.push(f.clone());
        e.values.insert(f, s);
        lx.skip_ws();
        match lx.peek() {
            Some(',') => lx.i += 1,
            _ => return Ok(()),
        }
    }
}

/// `bt_split_list`: split at a delimiter word surrounded by spaces at
/// brace depth 0 (case-insensitive). Empty items are `None`.
pub fn split_list(s: &str, delim: &str) -> Vec<Option<String>> {
    let b: Vec<char> = s.chars().collect();
    let d: Vec<char> = delim.chars().collect();
    let n = b.len();
    let mut depth = 0i64;
    let mut inword = true;
    let mut i = 0;
    let mut j = 0;
    let mut start = alloc::vec![0usize];
    let mut stop: Vec<i64> = Vec::new();
    while i < n {
        if depth == 0 && !inword && j < d.len() && b[i].to_lowercase().eq(core::iter::once(d[j])) {
            j += 1;
            i += 1;
            if j == d.len() && i < n && b[i] == ' ' {
                stop.push(i as i64 - d.len() as i64 - 1);
                i += 1;
                start.push(i);
                j = 0;
            }
        } else {
            match b[i] {
                '{' => depth += 1,
                '}' => {
                    if depth > 0 {
                        depth -= 1;
                    }
                }
                _ => {}
            }
            inword = b[i] != ' ';
            i += 1;
            j = 0;
        }
    }
    stop.push(n as i64);
    let mut out = Vec::new();
    for (k, &st) in start.iter().enumerate() {
        let sp = stop[k];
        if sp > st as i64 {
            out.push(Some(b[st..sp as usize].iter().collect()));
        } else {
            out.push(None);
        }
    }
    out
}

/// A name split into its four parts, each a list of tokens.
#[derive(Clone, Debug, Default)]
pub struct SplitName {
    pub first: Vec<Option<String>>,
    pub von: Vec<Option<String>>,
    pub last: Vec<Option<String>>,
    pub jr: Vec<Option<String>>,
}

/// `bt_split_name`.
pub fn split_name(name: &str) -> SplitName {
    if name.is_empty() {
        return SplitName::default();
    }
    // find_commas: collapse whitespace around commas, drop excess and
    // trailing commas
    let mut buf: Vec<char> = Vec::new();
    let mut depth = 0i64;
    let mut num_commas = 0;
    let mut last_ws = false;
    let mut warned = false;
    for c0 in name.chars() {
        let mut c = c0;
        match c {
            '{' => depth += 1,
            '}' => {
                if depth > 0 {
                    depth -= 1;
                }
            }
            _ => {}
        }
        if depth == 0 && c == ',' {
            num_commas += 1;
            if num_commas > 2 {
                warned = true;
                c = ' ';
            }
            if last_ws {
                buf.pop();
            } else {
                last_ws = true;
            }
        } else if c.is_ascii_whitespace() {
            if last_ws {
                continue;
            }
            last_ws = true;
        } else {
            last_ws = false;
        }
        buf.push(c);
    }
    if warned {
        let s: String = buf.iter().collect();
        buf = collapse(&s).chars().collect();
    }
    if num_commas > 0 {
        while buf.last() == Some(&',') {
            buf.pop();
            num_commas -= 1;
        }
    }
    let num_commas = num_commas.min(2);
    // find_tokens
    let mut tokens: Vec<Option<String>> = Vec::new();
    let mut comma_token: Vec<i64> = Vec::new();
    let mut in_boundary = true;
    let mut depth = 0i64;
    let mut cur = String::new();
    for &c in &buf {
        if depth == 0 && (c == ' ' || c == ',') {
            if c == ',' {
                comma_token.push(tokens.len() as i64);
            }
            if in_boundary {
                tokens.push(None);
            } else {
                tokens.push(Some(core::mem::take(&mut cur)));
            }
            in_boundary = true;
        } else {
            in_boundary = false;
            cur.push(c);
        }
        match c {
            '{' => depth += 1,
            '}' => {
                if depth > 0 {
                    depth -= 1;
                }
            }
            _ => {}
        }
    }
    if !in_boundary {
        tokens.push(Some(cur));
    }
    if buf.is_empty() {
        return SplitName::default();
    }
    // find_lc_tokens: the first run of lowercase tokens
    let (mut first_lc, mut last_lc) = (-1i64, -1i64);
    let mut i = 0usize;
    while i < tokens.len() {
        let lower = tokens[i].as_deref().is_some_and(|t| unicode::bt_isulower(&unicode::nfc(t)));
        if first_lc == -1 && lower {
            first_lc = i as i64;
            i += 1;
            while i < tokens.len() && tokens[i].as_deref().is_some_and(|t| unicode::bt_isulower(&unicode::nfc(t))) {
                i += 1;
            }
            last_lc = i as i64 - 1;
        } else {
            i += 1;
        }
    }
    let end = tokens.len() as i64 - 1;
    let range = |a: i64, b: i64| -> Vec<Option<String>> {
        if b - a + 1 <= 0 {
            Vec::new()
        } else {
            tokens[a as usize..=b as usize].to_vec()
        }
    };
    let mut sn = SplitName::default();
    if num_commas == 0 {
        if first_lc > -1 {
            if last_lc == end {
                last_lc -= 1;
            }
            sn.first = range(0, first_lc - 1);
            sn.von = range(first_lc, last_lc);
            sn.last = range(last_lc + 1, end);
        } else {
            sn.first = range(0, end - 1);
            sn.last = range(end, end);
        }
    } else {
        let (v0, v1);
        if first_lc == 0 {
            if last_lc == comma_token[0] {
                last_lc -= 1;
            }
            v0 = first_lc;
            v1 = last_lc;
        } else {
            v0 = 0;
            v1 = -1;
        }
        sn.von = range(v0, v1);
        sn.last = range(v1 + 1, comma_token[0]);
        if num_commas == 1 {
            sn.first = range(comma_token[0] + 1, end);
        } else {
            sn.jr = range(comma_token[0] + 1, comma_token[1]);
            sn.first = range(comma_token[1] + 1, end);
        }
    }
    sn
}

/// The "virtual length" of a token: a `{\...}` special counts as one.
fn vlength(s: &str) -> usize {
    let c: Vec<char> = s.chars().collect();
    let mut len = 0;
    let mut depth = 0i64;
    let mut special = false;
    for (i, &ch) in c.iter().enumerate() {
        match ch {
            '{' => {
                if depth == 0 && c.get(i + 1) == Some(&'\\') {
                    special = true;
                }
                depth += 1;
            }
            '}' => {
                if depth == 1 && special {
                    special = false;
                    len += 1;
                }
                depth -= 1;
            }
            _ => {
                if !special {
                    len += 1;
                }
            }
        }
    }
    len
}

/// Format one name part with `BTJ_MAYTIE` between tokens.
pub fn format_part(tokens: &[Option<String>]) -> String {
    let n = tokens.len();
    let mut o = String::new();
    for (j, t) in tokens.iter().enumerate() {
        let Some(t) = t else { continue };
        o.push_str(t);
        let vlen = vlength(t);
        if j + 1 < n {
            let tie = n > 1 && ((j == 0 && vlen < 3) || j == n - 2);
            o.push(if tie { '~' } else { ' ' });
        }
    }
    o
}
