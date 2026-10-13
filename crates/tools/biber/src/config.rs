//! Biber::Config and Biber::DataModel: biber's own options, the biblatex
//! options at their scopes (global, per entry type, per entry), and the
//! data model the `.bcf` declares.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::model::{looks_like_number, truthy};
use crate::regex::Regex;
use crate::unicode::fc;
use crate::xml::Element;

/// A biblatex option value.
#[derive(Clone, Debug)]
pub enum BV {
    S(String),
    /// A multivalued option: its `value` elements in order.
    M(Rc<Vec<Element>>),
}

impl BV {
    pub fn s(&self) -> String {
        match self {
            BV::S(s) => s.clone(),
            BV::M(_) => String::from("ARRAY"),
        }
    }
    pub fn truthy(&self) -> bool {
        match self {
            BV::S(s) => truthy(s),
            BV::M(_) => true,
        }
    }
    pub fn items(&self) -> Vec<Element> {
        match self {
            BV::M(v) => v.as_ref().clone(),
            BV::S(_) => Vec::new(),
        }
    }
}

/// `backendin` of an option scope declaration.
#[derive(Clone, Debug)]
pub enum BackendIn {
    List(Vec<String>),
    Map(Vec<(String, String)>),
}

#[derive(Clone, Debug)]
pub struct LantPart {
    pub namepart: String,
    pub use_: bool,
    pub pre: bool,
    pub substring_compound: bool,
    pub substring_side: Option<String>,
    pub substring_width: Option<String>,
}

#[derive(Clone, Debug)]
pub struct UntPart {
    pub namepart: String,
    pub use_: bool,
    pub disambiguation: Option<String>,
    pub base: bool,
}

#[derive(Clone, Debug)]
pub enum SnkPart {
    NamePart {
        value: String,
        use_: Option<String>,
        inits: bool,
    },
    Literal(String),
}

#[derive(Clone, Debug)]
pub struct Snk {
    pub visibility: String,
    pub template: Vec<Vec<SnkPart>>,
}

#[derive(Clone, Debug, Default)]
pub struct SortAttrs {
    pub substring_side: Option<String>,
    pub substring_width: Option<String>,
    pub pad_width: Option<String>,
    pub pad_char: Option<String>,
    pub pad_side: Option<String>,
    pub literal: bool,
}

#[derive(Clone, Debug, Default)]
pub struct SortSet {
    pub final_: bool,
    pub descending: bool,
    pub sortcase: Option<String>,
    pub sortupper: Option<String>,
    pub locale: Option<String>,
    pub items: Vec<(String, SortAttrs)>,
}

#[derive(Clone, Debug, Default)]
pub struct SortTemplate {
    pub locale: String,
    pub spec: Vec<SortSet>,
}

/// One field of the data model.
#[derive(Clone, Debug)]
pub struct FieldDef {
    pub fieldtype: String,
    pub datatype: String,
    pub format: String,
    pub nullok: bool,
    pub skipout: bool,
}

/// The helper field lists of Biber::DataModel, each sorted.
#[derive(Clone, Debug, Default)]
pub struct Helpers {
    pub namelistsall: Vec<String>,
    pub namelists: Vec<String>,
    pub lists: Vec<String>,
    pub fields: Vec<String>,
    pub datefields: Vec<String>,
    pub dateparts: Vec<String>,
    pub xsv: Vec<String>,
    pub ranges: Vec<String>,
    pub verbs: Vec<String>,
    pub vfields: Vec<String>,
    pub vlists: Vec<String>,
    pub integers: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct DataModel {
    pub fields: BTreeMap<String, FieldDef>,
    pub casemap: BTreeMap<String, String>,
    pub entrytypes: BTreeMap<String, bool>,
    pub constants: BTreeMap<String, (String, String)>,
    pub helpers: Helpers,
}

impl DataModel {
    pub fn new(dm: &Element) -> DataModel {
        let mut m = DataModel::default();
        let mut order: Vec<(String, FieldDef)> = Vec::new();
        if let Some(c) = dm.kid("constants") {
            for k in c.kids("constant") {
                m.constants.insert(
                    k.attr("name").unwrap_or("").into(),
                    (k.attr("type").unwrap_or("").into(), k.text.clone()),
                );
            }
        }
        if let Some(ets) = dm.kid("entrytypes") {
            for et in ets.kids("entrytype") {
                let f = fc(&et.text);
                m.casemap.insert(f.clone(), et.text.clone());
                let skip = et.attr("skip_output").is_some_and(truthy);
                m.entrytypes.insert(f, skip);
            }
        }
        if let Some(fs) = dm.kid("fields") {
            for f in fs.kids("field") {
                let name = fc(&f.text);
                m.casemap.insert(name.clone(), f.text.clone());
                let def = FieldDef {
                    fieldtype: f.attr("fieldtype").unwrap_or("").into(),
                    datatype: f.attr("datatype").unwrap_or("").into(),
                    format: f
                        .attr("format")
                        .filter(|s| truthy(s))
                        .unwrap_or("default")
                        .into(),
                    nullok: f.attr("nullok").is_some_and(truthy),
                    skipout: f.attr("skip_output").is_some_and(truthy),
                };
                order.retain(|(n, _)| n != &name);
                order.push((name, def));
            }
        }
        for (n, d) in order {
            m.fields.insert(n, d);
        }
        let all: Vec<String> = m.fields.keys().cloned().collect();
        let sel = |p: &dyn Fn(&FieldDef) -> bool| -> Vec<String> {
            let mut v: Vec<String> = all.iter().filter(|n| p(&m.fields[*n])).cloned().collect();
            v.sort();
            v
        };
        let h = Helpers {
            namelistsall: sel(&|d| d.fieldtype == "list" && d.datatype == "name"),
            namelists: sel(&|d| d.fieldtype == "list" && d.datatype == "name" && !d.skipout),
            lists: sel(&|d| {
                d.fieldtype == "list"
                    && d.datatype != "name"
                    && !d.skipout
                    && d.datatype != "verbatim"
                    && d.datatype != "uri"
            }),
            fields: sel(&|d| {
                d.fieldtype == "field"
                    && ["entrykey", "key", "integer", "datepart", "literal", "code"]
                        .contains(&d.datatype.as_str())
                    && !d.skipout
                    && d.format != "xsv"
            }),
            datefields: sel(&|d| d.fieldtype == "field" && d.datatype == "date"),
            dateparts: sel(&|d| d.fieldtype == "field" && d.datatype == "datepart"),
            xsv: sel(&|d| d.format == "xsv" && !d.skipout),
            ranges: sel(&|d| d.datatype == "range" && !d.skipout),
            verbs: sel(&|d| (d.datatype == "verbatim" || d.datatype == "uri") && !d.skipout),
            vfields: sel(&|d| {
                d.fieldtype == "field"
                    && (d.datatype == "verbatim" || d.datatype == "uri")
                    && !d.skipout
            }),
            vlists: sel(&|d| {
                d.fieldtype == "list"
                    && (d.datatype == "verbatim" || d.datatype == "uri")
                    && !d.skipout
            }),
            integers: sel(&|d| d.datatype == "datepart" || d.datatype == "integer"),
        };
        m.helpers = h;
        m
    }
    pub fn outcase(&self, s: &str) -> Option<String> {
        self.casemap.get(s).cloned()
    }
    pub fn constant_list(&self, name: &str) -> Vec<String> {
        match self.constants.get(name) {
            Some((t, v)) if t == "list" => split_comma(v),
            Some((t, v)) if t == "string" => alloc::vec![v.clone()],
            _ => Vec::new(),
        }
    }
    pub fn nameparts(&self) -> Vec<String> {
        self.constant_list("nameparts")
    }
    pub fn is_field(&self, f: &str) -> bool {
        // annotation fields: <field>+an[:name]
        if let Some(i) = f.find("+an") {
            let (base, rest) = (&f[..i], &f[i + 3..]);
            if !base.is_empty() && (rest.is_empty() || (rest.starts_with(':') && rest.len() > 1)) {
                return self.fields.contains_key(base);
            }
        }
        self.fields.contains_key(f)
    }
    pub fn entrytype_skipout(&self, t: &str) -> bool {
        self.entrytypes.get(t).copied().unwrap_or(false)
    }
    pub fn fieldtype(&self, f: &str) -> String {
        self.fields
            .get(f)
            .map(|d| d.fieldtype.clone())
            .unwrap_or_default()
    }
    pub fn datatype(&self, f: &str) -> String {
        self.fields
            .get(f)
            .map(|d| d.datatype.clone())
            .unwrap_or_default()
    }
    pub fn format(&self, f: &str) -> String {
        self.fields
            .get(f)
            .map(|d| d.format.clone())
            .unwrap_or_default()
    }
    pub fn is_type(&self, ft: &str, dt: &str, f: &str) -> bool {
        self.fields
            .get(f)
            .is_some_and(|d| d.fieldtype == ft && d.datatype == dt)
    }
    pub fn nullok(&self, f: &str) -> bool {
        self.fields.get(f).is_some_and(|d| d.nullok)
    }
    pub fn fields_of_type(&self, ft: &str, dts: &[&str]) -> Vec<String> {
        let mut v: Vec<String> = self
            .fields
            .iter()
            .filter(|(_, d)| d.fieldtype == ft && dts.contains(&d.datatype.as_str()))
            .map(|(n, _)| n.clone())
            .collect();
        v.sort();
        v
    }
    pub fn fields_of_fieldtype(&self, ft: &str) -> Vec<String> {
        self.fields
            .iter()
            .filter(|(_, d)| d.fieldtype == ft)
            .map(|(n, _)| n.clone())
            .collect()
    }
    pub fn fields_of_datatype(&self, dt: &str) -> Vec<String> {
        self.fields
            .iter()
            .filter(|(_, d)| d.datatype == dt)
            .map(|(n, _)| n.clone())
            .collect()
    }
    /// `sortdataschema`: integer sort fields compare as numbers.
    pub fn sort_is_int(&self, f: &str) -> bool {
        ["intciteorder", "citeorder", "citecount"].contains(&f)
            || self.helpers.integers.iter().any(|x| x == f)
    }
}

pub fn split_comma(s: &str) -> Vec<String> {
    // Perl's split(/\s*,\s*/): whitespace around each comma goes; trailing
    // empty fields are dropped
    let parts: Vec<&str> = s.split(',').collect();
    let n = parts.len();
    let mut v: Vec<String> = parts
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let mut x: &str = p;
            if i > 0 {
                x = x.trim_start();
            }
            if i + 1 < n {
                x = x.trim_end();
            }
            x.to_string()
        })
        .collect();
    while v.last().is_some_and(String::is_empty) {
        v.pop();
    }
    v
}

/// One `nosort`/`nonamestring` rule.
#[derive(Clone, Debug)]
pub struct FieldRe {
    pub name: String,
    pub re: Regex,
}

#[derive(Clone, Debug, Default)]
pub struct Config {
    pub opts: BTreeMap<String, String>,
    pub noinit: Vec<Regex>,
    pub nolabel: Vec<Regex>,
    pub nolabelwidthcount: Vec<Regex>,
    pub nosort: Vec<FieldRe>,
    pub nonamestring: Vec<FieldRe>,
    pub sourcemap: Vec<Element>,
    pub optscope: BTreeMap<String, BTreeSet<String>>,
    pub scope_opts: BTreeMap<String, Vec<String>>,
    pub opttype: BTreeMap<String, String>,
    pub optout: BTreeMap<(String, String), bool>,
    pub optin: BTreeMap<(String, String), BackendIn>,
    pub blx_global: BTreeMap<String, BV>,
    pub blx_type: BTreeMap<String, BTreeMap<String, BV>>,
    pub blx_entry: BTreeMap<(String, String), BTreeMap<String, BV>>,
    pub labelalphanametemplate: BTreeMap<String, Vec<LantPart>>,
    pub labelalphatemplate: BTreeMap<String, Element>,
    pub extradatespec: Vec<Vec<String>>,
    pub inheritance: Element,
    pub uniquenametemplate: BTreeMap<String, Vec<UntPart>>,
    pub namehashtemplate: BTreeMap<String, Vec<(String, String)>>,
    pub sortingnamekeytemplate: BTreeMap<String, Snk>,
    pub sortexclusion: BTreeMap<String, BTreeSet<String>>,
    pub sortinclusion: BTreeMap<String, BTreeSet<String>>,
    pub sortingtemplate: BTreeMap<String, SortTemplate>,
    pub datafield_sets: BTreeMap<String, Vec<String>>,
    pub keyorder: BTreeMap<String, BTreeMap<String, String>>,
    pub internal_keyorder: BTreeMap<String, BTreeMap<String, String>>,
    /// Per-section state reset by `_init`.
    pub crossrefkeys: Vec<(String, usize)>,
    pub xrefkeys: Vec<(String, usize)>,
    pub inherit_crossref: Vec<(String, String)>,
    pub inherit_xdata: Vec<(String, String)>,
    pub uniqignore: BTreeMap<String, BTreeMap<String, Vec<String>>>,
}

pub const NOKEY: &str = "\u{10FFFD}";

impl Config {
    pub fn new() -> Config {
        let mut c = Config::default();
        for (k, v) in [
            ("annotation_marker", "+an"),
            ("clrmacros", "0"),
            ("decodecharsset", "base"),
            ("fixinits", "0"),
            ("input_encoding", "UTF-8"),
            ("listsep", "and"),
            ("mincrossrefs", "2"),
            ("minxrefs", "2"),
            ("named_annotation_marker", ":"),
            ("namesep", "and"),
            ("nostdmacros", "0"),
            ("noskipduplicates", "0"),
            ("others_string", "others"),
            ("output_encoding", "UTF-8"),
            ("output_safechars", "0"),
            ("sortcase", "1"),
            ("sortupper", "1"),
            ("wraplines", "0"),
            ("xdatamarker", "xdata"),
            ("xdatasep", "-"),
            ("xnamesep", "="),
            ("xsvsep", "\\s*,\\s*"),
        ] {
            c.opts.insert(k.into(), v.into());
        }
        c.noinit = [r"\b\p{Ll}{2}\p{Pd}(?=\S)", r"[\x{2bf}\x{2018}]"]
            .iter()
            .map(|p| Regex::new(p, "").expect("noinit default"))
            .collect();
        c.nolabel = alloc::vec![
            Regex::new(r"[\p{Pc}\p{Ps}\p{Pe}\p{Pi}\p{Pf}\p{Po}\p{S}\p{C}]+", "").expect("nolabel")
        ];
        c.nosort = [r"\A\p{L}{2}\p{Pd}(?=\S)", r"[\x{2bf}\x{2018}]"]
            .iter()
            .map(|p| FieldRe {
                name: "setnames".into(),
                re: Regex::new(p, "").expect("nosort default"),
            })
            .collect();
        c
    }

    pub fn opt(&self, k: &str) -> String {
        self.opts.get(k).cloned().unwrap_or_default()
    }
    pub fn opt_true(&self, k: &str) -> bool {
        truthy(&self.opt(k))
    }

    pub fn in_scope(&self, opt: &str, scope: &str) -> bool {
        self.optscope.get(opt).is_some_and(|s| s.contains(scope))
    }

    /// Utils `map_boolean`.
    pub fn map_boolean(&self, name: &str, v: &str, tonum: bool) -> Option<String> {
        if self.opttype.get(name).map(String::as_str) != Some("boolean") {
            return Some(v.into());
        }
        let b = crate::unicode::lc(v);
        if tonum {
            if looks_like_number(&b) {
                return Some(b);
            }
            match b.as_str() {
                "true" => Some("1".into()),
                "false" => Some("0".into()),
                _ => None,
            }
        } else {
            if !looks_like_number(&b) {
                return Some(b);
            }
            match b.as_str() {
                "1" => Some("true".into()),
                "0" => Some("false".into()),
                _ => None,
            }
        }
    }

    /// `setblxoption`.
    pub fn set_blx(&mut self, secnum: &str, opt: &str, val: BV, scope: Option<(&str, &str)>) {
        let val = match val {
            BV::S(s) => match self.map_boolean(opt, &s, true) {
                Some(v) => BV::S(v),
                None => return self.unset_blx(secnum, opt, scope),
            },
            m => m,
        };
        match scope {
            None => {
                if self.in_scope(opt, "GLOBAL") {
                    self.blx_global.insert(opt.into(), val);
                }
            }
            Some(("ENTRY", key)) => {
                if self.in_scope(opt, "ENTRY") {
                    self.blx_entry
                        .entry((key.into(), secnum.into()))
                        .or_default()
                        .insert(opt.into(), val);
                }
            }
            Some((sc, t)) => {
                if self.in_scope(opt, sc) {
                    self.blx_type
                        .entry(t.into())
                        .or_default()
                        .insert(opt.into(), val);
                }
            }
        }
    }
    fn unset_blx(&mut self, secnum: &str, opt: &str, scope: Option<(&str, &str)>) {
        // an undef value: stored as undef, which reads as not defined
        match scope {
            None => {
                if self.in_scope(opt, "GLOBAL") {
                    self.blx_global.remove(opt);
                }
            }
            Some(("ENTRY", key)) => {
                if self.in_scope(opt, "ENTRY") {
                    if let Some(m) = self.blx_entry.get_mut(&(key.into(), secnum.into())) {
                        m.remove(opt);
                    }
                }
            }
            Some((sc, t)) => {
                if self.in_scope(opt, sc) {
                    if let Some(m) = self.blx_type.get_mut(t) {
                        m.remove(opt);
                    }
                }
            }
        }
    }

    /// `getblxoption`.
    pub fn blx(
        &self,
        secnum: Option<&str>,
        opt: &str,
        etype: Option<&str>,
        key: Option<&str>,
    ) -> Option<BV> {
        let secnum = secnum.unwrap_or(NOKEY);
        let etype = etype.unwrap_or(NOKEY);
        let key = key.unwrap_or(NOKEY);
        if self.in_scope(opt, "ENTRY") {
            if let Some(v) = self
                .blx_entry
                .get(&(key.into(), secnum.into()))
                .and_then(|m| m.get(opt))
            {
                return Some(v.clone());
            }
        }
        if self.in_scope(opt, "ENTRYTYPE") {
            if let Some(v) = self
                .blx_type
                .get(&crate::unicode::lc(etype))
                .and_then(|m| m.get(opt))
            {
                return Some(v.clone());
            }
        }
        if self.in_scope(opt, "GLOBAL") {
            return self.blx_global.get(opt).cloned();
        }
        None
    }
    pub fn blx_s(
        &self,
        secnum: Option<&str>,
        opt: &str,
        etype: Option<&str>,
        key: Option<&str>,
    ) -> String {
        self.blx(secnum, opt, etype, key)
            .map(|v| v.s())
            .unwrap_or_default()
    }
    pub fn blx_b(
        &self,
        secnum: Option<&str>,
        opt: &str,
        etype: Option<&str>,
        key: Option<&str>,
    ) -> bool {
        self.blx(secnum, opt, etype, key)
            .is_some_and(|v| v.truthy())
    }
    pub fn blx_n(
        &self,
        secnum: Option<&str>,
        opt: &str,
        etype: Option<&str>,
        key: Option<&str>,
    ) -> i64 {
        crate::model::numify(&self.blx_s(secnum, opt, etype, key)) as i64
    }
    /// The entry-scope options set for a key (sorted).
    pub fn entry_options(&self, secnum: &str, key: &str) -> Vec<String> {
        self.blx_entry
            .get(&(key.into(), secnum.into()))
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default()
    }

    pub fn incr_crossrefkey(&mut self, k: &str) {
        incr(&mut self.crossrefkeys, k);
    }
    pub fn incr_xrefkey(&mut self, k: &str) {
        incr(&mut self.xrefkeys, k);
    }
    pub fn is_inheritance_path(list: &[(String, String)], e1: &str, e2: &str) -> bool {
        for (s, t) in list.iter().filter(|(s, _)| s == e1) {
            let _ = s;
            if t == e2 {
                return true;
            }
            if Self::is_inheritance_path(list, t, e2) {
                return true;
            }
        }
        false
    }
    pub fn add_uniq_ignore(&mut self, key: &str, field: &str, uniqs: Option<&str>) {
        let Some(u) = uniqs.filter(|s| truthy(s)) else {
            return;
        };
        for x in split_comma(u) {
            self.uniqignore
                .entry(key.into())
                .or_default()
                .entry(x)
                .or_default()
                .push(field.into());
        }
    }
    pub fn keyorder(&self, sec: &str, key: &str) -> Option<String> {
        self.keyorder.get(sec).and_then(|m| m.get(key)).cloned()
    }
}

fn incr(v: &mut Vec<(String, usize)>, k: &str) {
    if let Some(e) = v.iter_mut().find(|(x, _)| x == k) {
        e.1 += 1;
    } else {
        v.push((k.into(), 1));
    }
}
