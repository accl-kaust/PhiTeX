//! Biber's data objects: Biber::Entry (data and derived fields),
//! Biber::Entry::Names / Biber::Entry::Name, and the field values they
//! hold. Lists, ranges and name lists are shared references as in Perl,
//! so inheritance shares them between entries the way biber does.

use alloc::collections::BTreeMap;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;

pub type Shared<T> = Rc<RefCell<T>>;

pub fn shared<T>(v: T) -> Shared<T> {
    Rc::new(RefCell::new(v))
}

/// A range item: start and (when there was a dash) end.
pub type RangeItem = (String, Option<String>);

/// A field value.
#[derive(Clone, Debug)]
pub enum Val {
    S(String),
    L(Shared<Vec<String>>),
    N(Shared<Names>),
    R(Shared<Vec<RangeItem>>),
}

impl Val {
    pub fn s(v: impl Into<String>) -> Val {
        Val::S(v.into())
    }
    pub fn list(v: Vec<String>) -> Val {
        Val::L(shared(v))
    }
    /// Perl truthiness of the value as `if (my $x = get_field(..))` sees it.
    pub fn truthy(&self) -> bool {
        match self {
            Val::S(s) => truthy(s),
            _ => true,
        }
    }
    pub fn str(&self) -> String {
        match self {
            Val::S(s) => s.clone(),
            Val::L(l) => {
                // Perl stringifies an array ref; never meaningful
                let _ = l;
                String::from("ARRAY")
            }
            _ => String::from("REF"),
        }
    }
    pub fn as_list(&self) -> Option<Shared<Vec<String>>> {
        match self {
            Val::L(l) => Some(l.clone()),
            _ => None,
        }
    }
    pub fn as_names(&self) -> Option<Shared<Names>> {
        match self {
            Val::N(n) => Some(n.clone()),
            _ => None,
        }
    }
    pub fn as_range(&self) -> Option<Shared<Vec<RangeItem>>> {
        match self {
            Val::R(r) => Some(r.clone()),
            _ => None,
        }
    }
}

/// Perl's boolean value of a string.
pub fn truthy(s: &str) -> bool {
    !(s.is_empty() || s == "0")
}

/// Scalar::Util `looks_like_number` (decimal forms Perl accepts).
pub fn looks_like_number(s: &str) -> bool {
    let t = s.trim_start();
    let t = t.strip_suffix('\n').unwrap_or(t);
    let b = t.as_bytes();
    if b.is_empty() {
        return false;
    }
    let mut i = 0;
    if b[0] == b'+' || b[0] == b'-' {
        i += 1;
    }
    let rest = &t[i..];
    let lr = rest.to_ascii_lowercase();
    if lr == "inf" || lr == "infinity" || lr == "nan" {
        return true;
    }
    let mut digits = 0;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
        digits += 1;
    }
    if i < b.len() && b[i] == b'.' {
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
            digits += 1;
        }
    }
    if digits == 0 {
        return false;
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        let s = j;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        if j == s {
            return false;
        }
        i = j;
    }
    i == b.len()
}

/// Perl's numeric value of a string (leading number, else 0).
pub fn numify(s: &str) -> f64 {
    let t = s.trim_start();
    let b = t.as_bytes();
    let mut i = 0;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    if i < b.len() && b[i] == b'.' {
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        let s0 = j;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        if j > s0 {
            i = j;
        }
    }
    t[..i].parse::<f64>().unwrap_or(0.0)
}

/// Perl's stringification of a number.
pub fn num_str(v: f64) -> String {
    if v.abs() < 1e15 && v == (v as i64) as f64 {
        alloc::format!("{}", v as i64)
    } else {
        alloc::format!("{v}")
    }
}

/// One name part: its string and its initials.
#[derive(Clone, Debug, Default)]
pub struct NamePart {
    pub string: Option<String>,
    pub initial: Option<Vec<String>>,
}

/// Biber::Entry::Name.
#[derive(Clone, Debug, Default)]
pub struct Name {
    pub id: String,
    pub index: usize,
    pub nameparts: BTreeMap<String, NamePart>,
    /// Which parts had their outer braces removed.
    pub strip: BTreeMap<String, bool>,
    /// Per-name options (`NAME` scope), in declaration order.
    pub opts: Vec<(String, String)>,
    pub hashid: Option<String>,
    pub xdata: Option<String>,
    pub hash: Option<String>,
    pub gender: Option<String>,
}

impl Name {
    pub fn namepart(&self, np: &str) -> String {
        self.nameparts
            .get(np)
            .and_then(|p| p.string.clone())
            .filter(|s| truthy(s))
            .unwrap_or_default()
    }
    pub fn initial(&self, np: &str) -> Vec<String> {
        self.nameparts.get(np).and_then(|p| p.initial.clone()).unwrap_or_default()
    }
    pub fn was_stripped(&self, np: &str) -> bool {
        self.strip.get(np).copied().unwrap_or(false)
    }
    pub fn opt(&self, o: &str) -> Option<String> {
        self.opts.iter().find(|(k, _)| k == o).map(|(_, v)| v.clone())
    }
    /// The parts present (Perl's `keys` of the name parts hash; the order
    /// only matters where every part is treated alike).
    pub fn parts(&self) -> Vec<String> {
        self.nameparts.keys().cloned().collect()
    }
    pub fn hash_namepart(&self, np: &str, template: &[(String, String)]) -> String {
        for (n, scope) in template {
            if crate::unicode::fc(n) == crate::unicode::fc(np) {
                let sc = crate::unicode::fc(scope);
                if sc == "init" {
                    return self.nameparts.get(np).and_then(|p| p.initial.clone()).map(|i| i.concat()).unwrap_or_default();
                } else if sc == "full" {
                    return self.namepart(np);
                }
            }
        }
        String::new()
    }
}

/// Biber::Entry::Names.
#[derive(Clone, Debug, Default)]
pub struct Names {
    pub id: String,
    pub typ: String,
    pub names: Vec<Shared<Name>>,
    pub morenames: bool,
    /// Name list options (`NAMELIST` scope).
    pub opts: Vec<(String, String)>,
}

impl Names {
    pub fn count(&self) -> usize {
        self.names.len()
    }
    pub fn add(&mut self, n: Shared<Name>) {
        self.names.push(n.clone());
        n.borrow_mut().index = self.names.len();
    }
    pub fn first_n(&self, n: usize) -> Vec<Shared<Name>> {
        let size = self.names.len();
        if size == 0 {
            return Vec::new();
        }
        let end = if n > size { size } else { n };
        if end == 0 {
            // Perl: @list[0 .. -1] is empty
            return Vec::new();
        }
        self.names[..end].to_vec()
    }
    pub fn opt(&self, o: &str) -> Option<String> {
        self.opts.iter().find(|(k, _)| k == o).map(|(_, v)| v.clone())
    }
    pub fn set_opt(&mut self, o: &str, v: String) {
        if let Some(e) = self.opts.iter_mut().find(|(k, _)| k == o) {
            e.1 = v;
        } else {
            self.opts.push((o.into(), v));
        }
    }
}

/// How the label date was found.
#[derive(Clone, Debug)]
pub enum LabelDate {
    Field {
        year: String,
        month: Option<String>,
        day: Option<String>,
        hour: Option<String>,
        minute: Option<String>,
        second: Option<String>,
        timezone: Option<String>,
        pseudodate: bool,
        source: String,
    },
    Str(String),
}

/// An XDATA reference.
#[derive(Clone, Debug)]
pub struct XRef {
    pub reffield: String,
    pub refposition: usize,
    pub entries: Vec<String>,
    pub field: Option<String>,
    pub position: String,
    pub resolved: Option<bool>,
}

/// Biber::Entry.
#[derive(Clone, Debug, Default)]
pub struct Entry {
    pub data: BTreeMap<String, Val>,
    pub derived: BTreeMap<String, Val>,
    pub labelname: Option<String>,
    pub labelnamefh: Option<String>,
    pub labeltitle: Option<String>,
    pub labeldate: Option<LabelDate>,
    pub xrefs: Option<Vec<XRef>>,
}

impl Entry {
    pub fn get(&self, k: &str) -> Option<Val> {
        if k.is_empty() {
            return None;
        }
        self.data.get(k).or_else(|| self.derived.get(k)).cloned()
    }
    /// The field as a string ('' when absent or not a string).
    pub fn gs(&self, k: &str) -> String {
        match self.get(k) {
            Some(Val::S(s)) => s,
            _ => String::new(),
        }
    }
    /// `if (my $x = $be->get_field($k))`.
    pub fn has(&self, k: &str) -> bool {
        self.get(k).is_some_and(|v| v.truthy())
    }
    pub fn exists(&self, k: &str) -> bool {
        self.data.contains_key(k) || self.derived.contains_key(k)
    }
    pub fn set_field(&mut self, k: &str, v: Val) {
        self.derived.insert(k.into(), v);
    }
    pub fn set_data(&mut self, k: &str, v: Val) {
        self.data.insert(k.into(), v);
    }
    pub fn del_field(&mut self, k: &str) {
        self.data.remove(k);
        self.derived.remove(k);
    }
    pub fn key(&self) -> String {
        self.gs("citekey")
    }
    pub fn etype(&self) -> String {
        self.gs("entrytype")
    }
    pub fn datafields(&self) -> Vec<String> {
        self.data.keys().cloned().collect()
    }
    pub fn add_warning(&mut self, w: String) {
        match self.derived.get("warnings") {
            Some(Val::L(l)) => l.borrow_mut().push(w),
            _ => {
                self.derived.insert("warnings".into(), Val::list(alloc::vec![w]));
            }
        }
    }
    pub fn has_keyword(&self, k: &str) -> bool {
        match self.data.get("keywords") {
            Some(Val::L(l)) => l.borrow().iter().any(|x| x == k),
            _ => false,
        }
    }
    pub fn date_fields_exist(&self, field: &str) -> bool {
        let t = strip_datepart(field);
        DATEPARTS.iter().any(|dp| {
            self.data.contains_key(&alloc::format!("{t}{dp}")) || self.data.contains_key(&alloc::format!("{t}end{dp}"))
        })
    }
    pub fn delete_date_fields(&mut self, field: &str) {
        let t = strip_datepart(field);
        for dp in DATEPARTS {
            self.data.remove(&alloc::format!("{t}{dp}"));
            self.data.remove(&alloc::format!("{t}end{dp}"));
        }
    }
}

const DATEPARTS: [&str; 8] = ["year", "month", "day", "hour", "minute", "second", "yeardivision", "timezone"];

fn strip_datepart(field: &str) -> String {
    for dp in DATEPARTS {
        if let Some(t) = field.strip_suffix(dp) {
            return t.strip_suffix("end").unwrap_or(t).into();
        }
    }
    field.into()
}

pub type EntryRef = Shared<Entry>;
