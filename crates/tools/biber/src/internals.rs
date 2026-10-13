//! Biber::Internals and the string helpers of Biber::Utils: name hashes,
//! alphabetic labels, sort keys, and the normalisations they use.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::biber::Biber;
use crate::config::{Config, SnkPart, SortAttrs};
use crate::model::{looks_like_number, numify, truthy, Name, Names, Shared, Val};
use crate::regex::{cap, Regex};
use crate::section::{DataList, LabelCacheV};
use crate::unicode::{fc, gc_len, gc_substr, lc, nfc, nfd, nfkd, uc};
use crate::xml::Element;

fn re(p: &str) -> Regex {
    Regex::new(p, "").unwrap_or_else(|e| panic!("pattern {p:?}: {e}"))
}

pub fn md5_hex(b: &[u8]) -> String {
    let d = phitex_codecs::md5::md5(b);
    let mut s = String::with_capacity(32);
    for x in d {
        s.push_str(&alloc::format!("{x:02x}"));
    }
    s
}

/// Utils `normalise_string_hash`.
pub fn normalise_string_hash(s: &str) -> String {
    if !truthy(s) {
        return String::new();
    }
    let s = re(r"\\(\p{L}+)\s*").replace(s, true, &mut |t, m| alloc::format!("{}:", cap(t, m, 1).unwrap_or_default()));
    let s = re(r"\\([^\p{L}])\s*").replace(&s, true, &mut |t, m| {
        let c = cap(t, m, 1).unwrap_or_default();
        alloc::format!("{}:", c.chars().next().map_or(0, |c| c as u32))
    });
    re(r"[\{\}~\.\s]+").delete_all(&s)
}

/// md5_hex(encode_utf8(NFC(normalise_string_hash(s)))).
pub fn hash_str(s: &str) -> String {
    md5_hex(nfc(&normalise_string_hash(s)).as_bytes())
}

fn trim_collapse(s: &str) -> String {
    let s = re(r"^\s+|\s+$").delete_all(s);
    re(r"\s+").replace(&s, true, &mut |_, _| " ".into())
}

fn tie_to_space(s: &str) -> String {
    re(r"([^\\])~").replace(s, true, &mut |t, m| alloc::format!("{} ", cap(t, m, 1).unwrap_or_default()))
}

/// Utils `normalise_string_common`.
pub fn normalise_string_common(s: &str) -> String {
    let s = re(r"\\[A-Za-z]+").delete_all(s);
    let s = re(r"[\p{P}\p{S}\p{C}]+").delete_all(&s);
    trim_collapse(&s)
}

/// Utils `normalise_string`.
pub fn normalise_string(s: &str) -> String {
    if !truthy(s) {
        return String::new();
    }
    normalise_string_common(&tie_to_space(s))
}

/// Utils `normalise_string_label`.
pub fn normalise_string_label(cfg: &Config, s: &str) -> String {
    if !truthy(s) {
        return String::new();
    }
    let mut s = re(r"\\[A-Za-z]+").delete_all(s);
    s = tie_to_space(&s);
    for r in &cfg.nolabel {
        s = r.delete_all(&s);
    }
    trim_collapse(&s)
}

fn field_res<'a>(cfg: &'a Config, list: &'a [crate::config::FieldRe], field: &str) -> Vec<&'a Regex> {
    let mut v = Vec::new();
    for o in list {
        if fc(&o.name) == fc(field) {
            v.push(&o.re);
        } else if let Some(set) = cfg.datafield_sets.get(&lc(&o.name)) {
            if set.iter().any(|x| fc(x) == fc(field)) {
                v.push(&o.re);
            }
        }
    }
    v
}

/// Utils `strip_nosort`.
pub fn strip_nosort(cfg: &Config, s: &str, field: &str) -> String {
    if !truthy(s) {
        return String::new();
    }
    let mut s = s.to_string();
    for r in field_res(cfg, &cfg.nosort, field) {
        s = r.delete_all(&s);
    }
    s
}

/// Utils `strip_nonamestring`.
pub fn strip_nonamestring(cfg: &Config, s: &str, field: &str) -> String {
    if !truthy(s) {
        return String::new();
    }
    let mut s = s.to_string();
    for r in field_res(cfg, &cfg.nonamestring, field) {
        s = r.delete_all(&s);
    }
    s
}

/// Utils `normalise_string_sort`.
pub fn normalise_string_sort(cfg: &Config, s: &str, field: &str) -> String {
    if !truthy(s) {
        return String::new();
    }
    let s = strip_nosort(cfg, s, field);
    let s = tie_to_space(&s);
    let s = re(r"\\[A-Za-z]+").delete_all(&s);
    let s = re(r"[{}]+").delete_all(&s);
    trim_collapse(&s)
}

/// Utils `escape_label`.
pub fn escape_label(s: &str) -> String {
    if !truthy(s) {
        return String::new();
    }
    let s = re(r"([_\^\$\#\%\&])").replace(s, true, &mut |t, m| alloc::format!("\\{}", cap(t, m, 1).unwrap_or_default()));
    s.replace('~', "{\\textasciitilde}").replace('>', "{\\textgreater}").replace('<', "{\\textless}")
}

/// Utils `unescape_label`.
pub fn unescape_label(s: &str) -> String {
    if !truthy(s) {
        return String::new();
    }
    let s = re(r"\\([_\^\$\~\#\%\&])").replace(s, true, &mut |t, m| cap(t, m, 1).unwrap_or_default());
    s.replace("{\\textasciitilde}", "~").replace("{\\textgreater}", ">").replace("{\\textless}", "<")
}

/// Utils `join_name`.
pub fn join_name(s: &str) -> String {
    let s = re(r"(?<!\\\S)\s+").replace(s, true, &mut |_, _| "\\bibnamedelimb ".into());
    let s = re(r"(?<!\\)~").replace(&s, true, &mut |_, _| "\\bibnamedelima ".into());
    re(r"(?<=\.)\\bibnamedelim[ab]").replace(&s, true, &mut |_, _| "\\bibnamedelimi".into())
}

/// Utils `parse_range` (start defaults to 1); the end is None when absent.
pub fn parse_range(s: &str) -> (Option<String>, Option<String>) {
    let r = Regex::new(r"\A\s*(\P{Pd}+)?\s*(\p{Pd})*\s*(\P{Pd}+)?\s*\z", "ms").expect("static");
    let t: Vec<char> = s.chars().collect();
    let Some(m) = r.find_at(&t, 0) else { return (Some("1".into()), None) };
    let (a, d, b) = (cap(&t, &m, 1), cap(&t, &m, 2), cap(&t, &m, 3));
    if d.as_deref().is_some_and(truthy) {
        (Some(a.unwrap_or_else(|| "1".into())), b)
    } else {
        (Some("1".into()), a)
    }
}

/// Utils `parse_range_alt`.
pub fn parse_range_alt(s: &str) -> Option<(Option<String>, Option<String>)> {
    let r = Regex::new(r"\A\s*(\P{Pd}+)?\s*(\p{Pd})*\s*(\P{Pd}+)?\s*\z", "ms").expect("static");
    let t: Vec<char> = s.chars().collect();
    let m = r.find_at(&t, 0)?;
    if cap(&t, &m, 2).as_deref().is_some_and(truthy) {
        Some((cap(&t, &m, 1), cap(&t, &m, 3)))
    } else {
        None
    }
}

/// Text::Roman `isroman`.
pub fn isroman(s: &str) -> bool {
    !s.is_empty() && Regex::new(r"\A(?:M{0,3})(?:CM|CD|D?C{0,3})(?:XC|XL|L?X{0,3})(?:IX|IV|V?I{0,3})\z", "i").expect("static").is_match(s)
}

/// Text::Roman `roman2int`.
pub fn roman2int(s: &str) -> i64 {
    let v = |c: char| match c.to_ascii_uppercase() {
        'I' => 1,
        'V' => 5,
        'X' => 10,
        'L' => 50,
        'C' => 100,
        'D' => 500,
        'M' => 1000,
        _ => 0,
    };
    let cs: Vec<i64> = s.chars().map(v).collect();
    let mut t = 0;
    for i in 0..cs.len() {
        if i + 1 < cs.len() && cs[i] < cs[i + 1] {
            t -= cs[i];
        } else {
            t += cs[i];
        }
    }
    t
}

/// Utils `rangelen`.
pub fn rangelen(rf: &[(String, Option<String>)]) -> i64 {
    let ok = re(r"^[\p{Nd}\p{Nl}iIvVxXlLcCdDmM]+$");
    let mut rl = 0i64;
    for (m, n) in rf {
        if truthy(m) && ok.is_match(m) {
            let mut m = nfkd(m);
            match n {
                Some(n) if truthy(n) && ok.is_match(n) => {
                    let mut n = nfkd(n);
                    if isroman(&m) {
                        m = roman2int(&m).to_string();
                    }
                    if isroman(&n) {
                        n = roman2int(&n).to_string();
                    }
                    if !(looks_like_number(&n) && looks_like_number(&m)) {
                        return -1;
                    }
                    if numify(&n) < numify(&m) {
                        let mc: Vec<char> = m.chars().rev().collect();
                        let mut nc: Vec<char> = n.chars().rev().collect();
                        for i in 0..mc.len() {
                            if i < nc.len() && nc[i] != '0' {
                                continue;
                            }
                            if i < nc.len() {
                                nc[i] = mc[i];
                            } else {
                                nc.push(mc[i]);
                            }
                        }
                        n = nc.into_iter().rev().collect();
                    }
                    rl += (numify(&n) - numify(&m)) as i64 + 1;
                }
                Some(_) => return -1,
                None => rl += 1,
            }
        } else {
            return -1;
        }
    }
    rl
}

/// `%UNIQUENAME_CONTEXTS`.
fn un_context(un: &str) -> Option<&'static str> {
    Some(match un {
        "false" => "none",
        "init" | "allinit" | "mininit" | "minyearinit" => "init",
        "full" | "allfull" | "minfull" | "minyearfull" => "initorfull",
        _ => return None,
    })
}

/// One computed name disambiguation (`process_namedis`).
pub struct NameDis {
    pub nlid: String,
    pub nid: String,
    pub nameun: String,
    pub namelistul: Option<String>,
    pub namestring: String,
    pub namestrings: Vec<String>,
    pub schema: Vec<(String, Vec<String>)>,
}

fn attr<'a>(e: &'a Element, n: &str) -> Option<&'a str> {
    e.attr(n)
}

impl Biber<'_> {

    /// Biber `process_namedis`.
    pub fn process_namedis(&mut self, key: &str, dl: &DataList) -> Result<Vec<NameDis>, crate::biber::Fatal> {
        let secnum = self.cur.clone();
        let be = self.be(key);
        let bee = be.borrow().etype();
        let mut untname = dl.uniquenametemplatename.clone();
        let mut un = self.cfg.blx(Some(&secnum), "uniquename", Some(&bee), Some(key)).map(|v| v.s());
        let mut ul = self.cfg.blx(Some(&secnum), "uniquelist", Some(&bee), Some(key)).map(|v| v.s());
        if let Some(v) = self.cfg.blx(Some(&secnum), "uniquenametemplatename", None, Some(key)) {
            untname = v.s();
        }
        let mut out = Vec::new();
        for pn in self.dm.helpers.namelistsall.clone() {
            let Some(nl) = be.borrow().get(&pn).and_then(|v| v.as_names()) else { continue };
            let nl = nl.borrow();
            if let Some(x) = nl.opt("uniquenametemplatename") {
                untname = x;
            }
            if let Some(x) = nl.opt("uniquelist") {
                ul = Some(x);
            }
            if let Some(x) = nl.opt("uniquename") {
                un = Some(x);
            }
            for n in &nl.names {
                let n = n.borrow();
                if let Some(x) = n.opt("uniquenametemplatename") {
                    untname = x;
                }
                let Some(tmpl) = self.cfg.uniquenametemplate.get(&untname).cloned() else {
                    return Err(self.fatal(alloc::format!("No uniquenametemplate called '{untname}' found, cannot continue.")));
                };
                if let Some(x) = n.opt("uniquename") {
                    un = Some(x);
                }
                let nameun = un.clone().unwrap_or_default();
                let useok = |me: &Self, npn: &str| -> bool {
                    let m = alloc::format!("use{npn}");
                    let mut ok = me.cfg.blx_b(Some(&secnum), &m, Some(&bee), Some(key));
                    if let Some(v) = nl.opt(&m) {
                        ok = truthy(&v);
                    }
                    if let Some(v) = n.opt(&m) {
                        ok = truthy(&v);
                    }
                    ok
                };
                let mut namestring = String::new();
                let mut namestrings = Vec::new();
                let mut schema = Vec::new();
                let mut base = String::new();
                let mut baseparts: Option<Vec<String>> = None;
                for np in tmpl.iter().filter(|p| p.base) {
                    let p = n.namepart(&np.namepart);
                    if truthy(&p) {
                        if np.use_ && !useok(self, &np.namepart) {
                            continue;
                        }
                        base.push_str(&p);
                        baseparts.get_or_insert_with(Vec::new).push(np.namepart.clone());
                    }
                }
                namestring.push_str(&base);
                namestrings.push(base);
                if let Some(bp) = baseparts {
                    schema.push(("base".to_string(), bp));
                }
                for np in tmpl.iter().filter(|p| !p.base) {
                    if np.disambiguation.as_deref() == Some("none") {
                        continue;
                    }
                    let npn = &np.namepart;
                    let level = np
                        .disambiguation
                        .clone()
                        .or_else(|| un_context(un.as_deref().unwrap_or("false")).map(Into::into))
                        .unwrap_or_default();
                    let lastns = namestrings.last().cloned().unwrap_or_default();
                    let p = n.namepart(npn);
                    if truthy(&p) {
                        let pi = n.initial(npn).concat();
                        if np.use_ && !useok(self, npn) {
                            continue;
                        }
                        namestring.push_str(&p);
                        let l = fc(&level);
                        if l == "full" {
                            namestrings.push(alloc::format!("{lastns}{p}"));
                            schema.push((npn.clone(), alloc::vec!["fullonly".into()]));
                        }
                        if l == "initorfull" {
                            namestrings.push(alloc::format!("{lastns}{pi}"));
                            schema.push((npn.clone(), alloc::vec!["init".into()]));
                            namestrings.push(alloc::format!("{lastns}{p}"));
                            schema.push((npn.clone(), alloc::vec!["full".into()]));
                        } else if l == "init" {
                            namestrings.push(alloc::format!("{lastns}{pi}"));
                            schema.push((npn.clone(), alloc::vec!["init".into()]));
                        }
                    }
                }
                out.push(NameDis {
                    nlid: nl.id.clone(),
                    nid: n.id.clone(),
                    nameun,
                    namelistul: ul.clone(),
                    namestring: strip_nonamestring(&self.cfg, &namestring, &nl.typ),
                    namestrings: namestrings.iter().map(|s| strip_nonamestring(&self.cfg, s, &nl.typ)).collect(),
                    schema,
                });
            }
        }
        Ok(out)
    }

    fn nht_name(&self, key: &str, names: &Names, dl: &DataList) -> String {
        let mut n = self
            .cfg
            .blx(Some(&self.cur), "namehashtemplatename", None, Some(key))
            .map_or_else(|| dl.namehashtemplatename.clone(), |v| v.s());
        if let Some(x) = names.opt("namehashtemplatename") {
            n = x;
        }
        n
    }

    fn nht(&mut self, name: &str) -> Result<Vec<(String, String)>, crate::biber::Fatal> {
        match self.cfg.namehashtemplate.get(name) {
            Some(t) => Ok(t.clone()),
            None => Err(self.fatal(alloc::format!("No namehash template called '{name}'"))),
        }
    }

    fn nho(&self, key: &str, names: &Names) -> bool {
        let bee = self.be(key).borrow().etype();
        let mut v = self.cfg.blx_b(Some(&self.cur), "nohashothers", Some(&bee), Some(key));
        if let Some(x) = names.opt("nohashothers") {
            v = truthy(&x);
        }
        v
    }

    /// Internals `_getnamehash`.
    pub fn getnamehash(&mut self, key: &str, names: &Names, dl: &DataList, bib: bool) -> Result<String, crate::biber::Fatal> {
        let (vc, vb, _, _) = dl.vis(&names.id);
        let visible = if bib { vb } else { vc };
        let nps = self.dm.nameparts();
        let mut nhtname = self.nht_name(key, names, dl);
        let mut hk = String::new();
        for n in names.first_n(visible) {
            let n = n.borrow();
            if let Some(h) = n.hashid.as_ref().filter(|s| truthy(s)) {
                hk.push_str(h);
                continue;
            }
            if let Some(x) = n.opt("namehashtemplatename") {
                nhtname = x;
            }
            let nht = self.nht(&nhtname)?;
            for nt in &nps {
                hk.push_str(&n.hash_namepart(nt, &nht));
            }
        }
        if !self.nho(key, names) && (visible < names.count() || names.morenames) {
            hk.push('+');
        }
        Ok(hash_str(&hk))
    }

    /// Internals `_getfullhash`.
    pub fn getfullhash(&mut self, key: &str, names: &Names, dl: &DataList) -> Result<String, crate::biber::Fatal> {
        let nps = self.dm.nameparts();
        let mut nhtname = self.nht_name(key, names, dl);
        let mut hk = String::new();
        for n in &names.names {
            let n = n.borrow();
            if let Some(h) = n.hashid.as_ref().filter(|s| truthy(s)) {
                hk.push_str(h);
                continue;
            }
            if let Some(x) = n.opt("namehashtemplatename") {
                nhtname = x;
            }
            let nht = self.nht(&nhtname)?;
            for nt in &nps {
                hk.push_str(&strip_nonamestring(&self.cfg, &n.hash_namepart(nt, &nht), &names.typ));
            }
        }
        if names.morenames {
            hk.push('+');
        }
        Ok(hash_str(&hk))
    }

    /// Internals `_getfullhashraw`.
    pub fn getfullhashraw(&self, names: &Names) -> String {
        let nps = self.dm.nameparts();
        let mut hk = String::new();
        for n in &names.names {
            let n = n.borrow();
            for nt in &nps {
                let p = n.namepart(nt);
                if truthy(&p) {
                    hk.push_str(&strip_nonamestring(&self.cfg, &p, &names.typ));
                }
            }
        }
        if names.morenames {
            hk.push('+');
        }
        hash_str(&hk)
    }

    /// Internals `_getnamehash_u`.
    pub fn getnamehash_u(&mut self, key: &str, names: &Names, dl: &DataList) -> Result<String, crate::biber::Fatal> {
        let (visible, _, _, _) = dl.vis(&names.id);
        let mut untname = self
            .cfg
            .blx(Some(&self.cur), "uniquenametemplatename", None, Some(key))
            .map_or_else(|| dl.uniquenametemplatename.clone(), |v| v.s());
        if let Some(x) = names.opt("uniquenametemplatename") {
            untname = x;
        }
        let mut hk = String::new();
        for n in names.first_n(visible) {
            let n = n.borrow();
            if let Some(x) = n.opt("uniquenametemplatename") {
                untname = x;
            }
            let tmpl = self.cfg.uniquenametemplate.get(&untname).cloned().unwrap_or_default();
            for nps in &tmpl {
                if nps.disambiguation.as_deref() == Some("none") {
                    continue;
                }
                let np = n.namepart(&nps.namepart);
                if !truthy(&np) {
                    continue;
                }
                if nps.base {
                    hk.push_str(&np);
                } else if let Some(u) = dl.get_nd(&names.id, &n.id).and_then(|d| d.un.clone()) {
                    if u.0 != "base" {
                        let l = u.1.first().cloned().unwrap_or_default();
                        if l == "full" || l == "fullonly" {
                            hk.push_str(&np);
                        } else if l == "init" {
                            hk.push_str(&n.initial(&nps.namepart).concat());
                        }
                    }
                }
            }
        }
        if !self.nho(key, names) && (visible < names.count() || names.morenames) {
            hk.push('+');
        }
        Ok(hash_str(&hk))
    }

    /// Internals `_genpnhash`.
    pub fn genpnhash(&mut self, key: &str, names: &Names, n: &Name, dl: &DataList) -> Result<String, crate::biber::Fatal> {
        if let Some(h) = n.hashid.as_ref().filter(|s| truthy(s)) {
            return Ok(hash_str(h));
        }
        let mut nhtname = self.nht_name(key, names, dl);
        if let Some(x) = n.opt("namehashtemplatename") {
            nhtname = x;
        }
        let nht = self.nht(&nhtname)?;
        let mut hk = String::new();
        for nt in self.dm.nameparts() {
            hk.push_str(&n.hash_namepart(&nt, &nht));
        }
        Ok(hash_str(&hk))
    }

    // ---------------------------------------------------------------- labels

    /// Internals `_genlabel`.
    pub fn genlabel(&mut self, key: &str, dl: &DataList) -> (String, String) {
        let be = self.be(key);
        let bee = be.borrow().etype();
        let tmpl = self
            .cfg
            .labelalphatemplate
            .get(&lc(&bee))
            .or_else(|| self.cfg.labelalphatemplate.get(""))
            .cloned()
            .unwrap_or_default();
        let mut els: Vec<&Element> = tmpl.kids("labelelement").collect();
        els.sort_by_key(|e| e.order());
        let mut label = String::new();
        let mut slabel = String::new();
        let mut fin = false;
        for el in els {
            let parts: Vec<Element> = el.kids("labelpart").cloned().collect();
            let (a, b) = self.labelpart(&parts, key, dl, &mut fin);
            label.push_str(&a);
            slabel.push_str(&b);
            if fin {
                break;
            }
        }
        (label, slabel)
    }

    fn labelpart(&mut self, parts: &[Element], key: &str, dl: &DataList, fin: &mut bool) -> (String, String) {
        let be = self.be(key);
        let bee = be.borrow().etype();
        let secnum = self.cur.clone();
        let maxan = self.cfg.blx_n(Some(&secnum), "maxalphanames", Some(&bee), Some(key));
        let minan = self.cfg.blx_n(Some(&secnum), "minalphanames", Some(&bee), Some(key));
        let mut lp = String::new();
        let mut slp = String::new();
        for part in parts {
            if let Some(inc) = attr(part, "ifnames").filter(|s| truthy(s)) {
                let mut f = part.text.clone();
                if f == "labelname" {
                    f = be.borrow().labelname.clone().unwrap_or_default();
                }
                if self.dm.fields_of_type("list", &["name"]).contains(&f) {
                    let Some(nm) = be.borrow().get(&f).and_then(|v| v.as_names()) else { continue };
                    let total = nm.borrow().count() as i64;
                    let vis = if total > maxan { minan } else { total };
                    if re(r"^\d+$").is_match(inc) {
                        if vis != numify(inc) as i64 {
                            continue;
                        }
                    } else {
                        let r = parse_range_alt(inc).unwrap_or((None, None));
                        let n = |s: &Option<String>| s.as_deref().map(|x| numify(x) as i64);
                        match (n(&r.0), n(&r.1)) {
                            (None, Some(b)) => {
                                if vis > b {
                                    continue;
                                }
                            }
                            (Some(a), None) => {
                                if vis < a {
                                    continue;
                                }
                            }
                            (Some(a), Some(b)) => {
                                if !(vis >= a && vis <= b) {
                                    continue;
                                }
                            }
                            (None, None) => {
                                if vis > 0 {
                                    continue;
                                }
                            }
                        }
                    }
                }
            }
            let (a, b) = self.dispatch_label(part, key, dl);
            lp.push_str(&a);
            slp.push_str(&b);
            if truthy(&a) {
                if attr(part, "final").is_some_and(truthy) {
                    *fin = true;
                }
                break;
            }
        }
        (lp, slp)
    }

    fn dispatch_label(&mut self, part: &Element, key: &str, dl: &DataList) -> (String, String) {
        let c = part.text.as_str();
        let basic = |me: &mut Self, f: &str, nostrip: bool| -> (String, String) {
            let be = me.be(key);
            let v = be.borrow().get(f).map(|v| v.str()).unwrap_or_default();
            let f2 = if nostrip { v } else { normalise_string_label(&me.cfg, &v) };
            if truthy(&f2) {
                let b = me.process_label_attributes(key, dl, &[(f2, None)], part, Some(f), None, 0);
                let u = unescape_label(&b);
                (b, u)
            } else {
                (String::new(), String::new())
            }
        };
        match c {
            "label" | "shorthand" | "sortkey" => basic(self, c, true),
            "citekey" | "entrykey" => {
                let k = self.process_label_attributes(key, dl, &[(key.to_string(), None)], part, None, None, 0);
                let u = unescape_label(&k);
                (k, u)
            }
            "labelname" => self.label_name(key, "labelname", part, dl),
            "labeltitle" | "labelmonth" | "labelday" | "labelyear" => basic(self, c, false),
            _ => {
                if !self.dm.is_field(c) {
                    return (escape_label(&unescape_label(c)), unescape_label(c));
                }
                if self.dm.fieldtype(c) == "list" && self.dm.datatype(c) == "name" {
                    self.label_name(key, c, part, dl)
                } else {
                    basic(self, c, false)
                }
            }
        }
    }

    fn label_name(&mut self, key: &str, namename: &str, attrs: &Element, dl: &DataList) -> (String, String) {
        let secnum = self.cur.clone();
        let be = self.be(key);
        let bee = be.borrow().etype();
        let mut useprefix = self.cfg.blx_s(Some(&secnum), "useprefix", Some(&bee), Some(key));
        let alphaothers = self.cfg.blx(None, "alphaothers", Some(&bee), None).map(|v| v.s()).unwrap_or_default();
        let sortalphaothers = self.cfg.blx(None, "sortalphaothers", Some(&bee), None).map(|v| v.s()).unwrap_or_default();
        let mut lantname = dl.labelalphanametemplatename.clone();
        if let Some(v) = self.cfg.blx(Some(&secnum), "labelalphanametemplatename", None, Some(key)) {
            lantname = v.s();
        }
        let Some(lni) = be.borrow().labelname.clone() else { return (String::new(), String::new()) };
        let realname = if namename == "labelname" { lni } else { namename.to_string() };
        let names = be.borrow().get(&realname).and_then(|v| v.as_names());
        let lnameopt = realname.strip_prefix("short").filter(|s| !s.is_empty()).unwrap_or(&realname).to_string();
        let Some(names) = names.filter(|_| self.cfg.blx_b(Some(&secnum), &alloc::format!("use{lnameopt}"), Some(&bee), Some(key))) else {
            return (String::new(), String::new());
        };
        let names = names.borrow();
        if let Some(x) = names.opt("labelalphanametemplatename") {
            lantname = x;
        }
        if let Some(x) = names.opt("useprefix") {
            useprefix = x;
        }
        let numnames = names.count();
        let (_, _, _, visibility) = dl.vis(&names.id);
        let (nr_start, nr_end) = if let Some(nr) = attr(attrs, "names") {
            let (a, b) = parse_range(nr);
            let s = a.map_or(1, |x| numify(&x) as usize);
            let e = match b.as_deref() {
                Some("+") => visibility,
                None => numnames,
                Some(x) => {
                    let v = numify(x) as usize;
                    if v > numnames {
                        numnames
                    } else {
                        v
                    }
                }
            };
            (s, e)
        } else {
            (1, visibility)
        };
        type Acc = Vec<(String, Option<(Option<String>, Option<String>, bool)>)>;
        let mut pre: Vec<Acc> = Vec::new();
        let mut main: Vec<Acc> = Vec::new();
        let mut mpns: Vec<Vec<String>> = Vec::new();
        for name in &names.names {
            let name = name.borrow();
            if let Some(x) = name.opt("labelalphanametemplatename") {
                lantname = x;
            }
            if let Some(x) = name.opt("useprefix") {
                useprefix = x;
            }
            let lnat = self.cfg.labelalphanametemplate.get(&lantname).cloned().unwrap_or_default();
            let (mut pa, mut ma, mut mp) = (Vec::new(), Vec::new(), Vec::new());
            for lnp in &lnat {
                let np = name.namepart(&lnp.namepart);
                if !truthy(&np) {
                    continue;
                }
                if lnp.use_ && !(lnp.namepart == "prefix" && truthy(&useprefix)) {
                    continue;
                }
                let o = Some((lnp.substring_width.clone(), lnp.substring_side.clone(), lnp.substring_compound));
                if lnp.pre {
                    pa.push((normalise_string_label(&self.cfg, &np), o));
                } else {
                    mp.push(lnp.namepart.clone());
                    ma.push((normalise_string_label(&self.cfg, &np), o));
                }
            }
            pre.push(pa);
            main.push(ma);
            mpns.push(mp);
        }
        let mut acc = String::new();
        let mut i = nr_start.saturating_sub(1);
        while i < nr_end {
            for (np, o) in pre.get(i).cloned().unwrap_or_default() {
                let (w, _, comp) = o.unwrap_or((None, None, false));
                let w = w.as_deref().map_or(1, |x| numify(x) as i64);
                if comp {
                    let mut t = String::new();
                    for p in crate::bib::re_split(&re(r"[\s\p{Dash}~]+"), &np) {
                        t.push_str(&gc_substr(&p, 0, Some(w)));
                    }
                    acc.push_str(&t);
                } else {
                    acc.push_str(&gc_substr(&np, 0, Some(w)));
                }
            }
            let fs = main.get(i).cloned().unwrap_or_default();
            let pn = mpns.get(i).cloned();
            acc.push_str(&self.process_label_attributes(key, dl, &fs, attrs, Some(&realname), pn.as_deref(), i));
            if let Some(ns) = attr(attrs, "namessep").filter(|s| truthy(s)) {
                if i + 1 != nr_end {
                    acc.push_str(ns);
                }
            }
            i += 1;
        }
        let mut sortacc = acc.clone();
        if !attr(attrs, "noalphaothers").is_some_and(truthy) && (numnames > nr_end || names.morenames) {
            acc.push_str(&alphaothers);
            sortacc.push_str(&sortalphaothers);
        }
        (acc, unescape_label(&sortacc))
    }

    #[allow(clippy::too_many_arguments)]
    fn process_label_attributes(
        &mut self,
        key: &str,
        dl: &DataList,
        fieldstrings: &[(String, Option<(Option<String>, Option<String>, bool)>)],
        la: &Element,
        field: Option<&str>,
        nameparts: Option<&[String]>,
        index: usize,
    ) -> String {
        let citekeys = self.secr().citekeys.clone();
        let nindex = citekeys.iter().position(|k| k == key);
        let mut out = String::new();
        let sw = attr(la, "substring_width").map(String::from);
        for (fs0, npo) in fieldstrings {
            let mut fs = fs0.clone();
            if let Some(sw) = &sw {
                if sw.contains('v') && field.is_some() {
                    let field = field.unwrap_or_default();
                    if !self.secr().labelcache_v.contains_key(field) {
                        let c = self.build_labelcache_v(&citekeys, field, nameparts, dl, la, sw);
                        self.sec().labelcache_v.insert(field.into(), c);
                    }
                    let c = &self.secr().labelcache_v[field];
                    let idx = c.globalindices.get(&fs).copied().filter(|&x| x != 0).or_else(|| c.index.get(&fs).copied());
                    fs = idx.and_then(|i| c.data.get(&fs).and_then(|d| d.get(i)).cloned()).unwrap_or_default();
                } else if sw.contains('l') && field.is_some() {
                    let field = field.unwrap_or_default();
                    if !self.secr().labelcache_l.contains_key(field) {
                        let strings: Vec<Vec<String>> = citekeys
                            .iter()
                            .map(|k| {
                                let v = self.entry(k).and_then(|e| e.borrow().get(field));
                                match v {
                                    Some(v) if v.truthy() => match (nameparts, v.as_names()) {
                                        (Some(nps), Some(nl)) => {
                                            let nl = nl.borrow();
                                            let (_, _, _, va) = dl.vis(&nl.id);
                                            nl.first_n(va).iter().map(|n| nps.iter().map(|p| n.borrow().namepart(p)).collect::<String>()).collect()
                                        }
                                        _ => alloc::vec![v.str()],
                                    },
                                    _ => alloc::vec![String::new()],
                                }
                            })
                            .collect();
                        let c = label_listdisambiguation(&strings);
                        self.sec().labelcache_l.insert(field.into(), c);
                    }
                    fs = nindex
                        .and_then(|n| self.secr().labelcache_l[field].get(n).and_then(|r| r.get(index)).cloned())
                        .unwrap_or_default();
                } else {
                    let padchar = attr(la, "pad_char").map(String::from);
                    let mut side = attr(la, "substring_side").filter(|s| truthy(s)).unwrap_or("left").to_string();
                    let mut width: i64 = if truthy(sw) { numify(sw) as i64 } else { 1 };
                    if nameparts.is_some() {
                        if let Some((w, s, _)) = npo {
                            if let Some(w) = w.as_deref().filter(|s| truthy(s)) {
                                width = numify(w) as i64;
                            }
                            if let Some(s) = s.as_deref().filter(|s| truthy(s)) {
                                side = s.into();
                            }
                        }
                    }
                    let off = if side == "right" { -width } else { 0 };
                    let mut nlwcis: Option<Vec<(String, usize)>> = None;
                    if !self.cfg.nolabelwidthcount.is_empty() {
                        nlwcis = match_indices(&self.cfg.nolabelwidthcount, &fs);
                        for r in &self.cfg.nolabelwidthcount {
                            fs = r.delete_all(&fs);
                        }
                    }
                    if nameparts.is_some() && npo.as_ref().is_some_and(|o| o.2) {
                        let mut t = String::new();
                        for p in crate::bib::re_split(&re(r"[\s\p{Dash}]+"), &fs) {
                            t.push_str(&gc_substr(&p, off, Some(width)));
                        }
                        fs = t;
                    } else {
                        fs = gc_substr(&fs, off, Some(width));
                    }
                    if let Some(pc) = padchar.filter(|s| truthy(s)) {
                        let pc = unescape_label(&pc);
                        let ps = attr(la, "pad_side").filter(|s| truthy(s)).unwrap_or("right");
                        let diff = width - gc_len(&fs) as i64;
                        if diff > 0 {
                            let pad = pc.repeat(diff as usize);
                            if ps == "right" {
                                fs.push_str(&pad);
                            } else if ps == "left" {
                                fs = alloc::format!("{pad}{fs}");
                            }
                        }
                        fs = escape_label(&fs);
                    }
                    if let Some(ix) = nlwcis {
                        let mut g: Vec<String> = crate::unicode::graphemes(&fs).into_iter().map(String::from).collect();
                        for (s, pos) in ix {
                            if pos < g.len() {
                                g.insert(pos, s);
                            }
                        }
                        fs = g.concat();
                    }
                }
            }
            out.push_str(&fs);
        }
        let up = attr(la, "uppercase").is_some_and(truthy);
        let low = attr(la, "lowercase").is_some_and(truthy);
        if up && !low {
            out = uc(&out);
        } else if low && !up {
            out = lc(&out);
        }
        out
    }

    #[allow(clippy::too_many_arguments)]
    fn build_labelcache_v(&self, citekeys: &[String], field: &str, nameparts: Option<&[String]>, dl: &DataList, la: &Element, sw: &str) -> LabelCacheV {
        let mut indices: BTreeMap<String, usize> = BTreeMap::new();
        for k in citekeys {
            let Some(f) = self.entry(k).and_then(|e| e.borrow().get(field)).filter(Val::truthy) else { continue };
            if let Some(nps) = nameparts {
                if let Some(nl) = f.as_names() {
                    let nl = nl.borrow();
                    let (_, _, _, va) = dl.vis(&nl.id);
                    for n in nl.first_n(va) {
                        let n = n.borrow();
                        let s: String = nps.iter().map(|p| n.namepart(p)).collect();
                        indices.insert(normalise_string_label(&self.cfg, &s), n.index);
                    }
                }
            } else {
                indices.insert(f.str(), 0);
            }
        }
        let strings: Vec<String> = indices.keys().cloned().collect();
        let maxlen = attr(la, "substring_width_max")
            .filter(|s| truthy(s))
            .map_or_else(|| strings.iter().map(|s| gc_len(s)).max().unwrap_or(0), |s| numify(s) as usize);
        let mut c = LabelCacheV::default();
        let mut subs: BTreeMap<String, usize> = BTreeMap::new();
        for i in 1..=maxlen {
            let maps: Vec<(String, String)> = strings
                .iter()
                .map(|s| {
                    let x = gc_substr(s, 0, Some(i as i64));
                    *subs.entry(x.clone()).or_insert(0) += 1;
                    (s.clone(), x)
                })
                .collect();
            for (s, x) in maps {
                c.data.entry(s.clone()).or_default().push(x.clone());
                c.nameindex.insert(s.clone(), indices[&s]);
                if !c.index.contains_key(&s) && (subs[&x] == 1 || i == maxlen) {
                    c.index.insert(s, gc_len(&x).saturating_sub(1));
                }
            }
        }
        if sw.contains('f') {
            let thr = attr(la, "substring_fixed_threshold").map_or(1, |s| numify(s) as usize);
            let mut is: BTreeMap<usize, BTreeMap<usize, usize>> = BTreeMap::new();
            for s in c.data.keys() {
                let ni = c.nameindex.get(s).copied().unwrap_or(0);
                let ix = c.index.get(s).copied().unwrap_or(0);
                *is.entry(ni).or_default().entry(ix).or_insert(0) += 1;
            }
            for s in c.data.keys().cloned().collect::<Vec<_>>() {
                for (ind, m) in &is {
                    if indices.get(&s) != Some(ind) {
                        continue;
                    }
                    if let Some(mx) = m.iter().filter(|(_, n)| **n >= thr).map(|(k, _)| *k).max() {
                        c.globalindices.insert(s.clone(), mx);
                    }
                }
            }
        }
        c
    }

    // --------------------------------------------------------------- sorting

    /// Internals `_generatesortinfo`.
    pub fn generatesortinfo(&mut self, key: &str, dl: &mut DataList) {
        let tmpl = dl.sortingtemplate.clone();
        let be = self.be(key);
        let mut sortobj: Vec<String> = Vec::new();
        let mut szero = false;
        let mut sort_final = String::new();
        let mut suppress_final = true;
        for set in &tmpl.spec {
            let mut s = String::new();
            for (name, attrs) in &set.items {
                let out = self.dispatch_sorting(name, key, dl, attrs);
                if truthy(&out) {
                    if set.final_ {
                        sort_final = out;
                        break;
                    }
                    s = out;
                    break;
                }
            }
            if s == "BIBERZERO" {
                szero = true;
                s = "0".into();
            }
            if truthy(&sort_final) {
                sortobj.push(if suppress_final { String::new() } else { sort_final.clone() });
                suppress_final = false;
            } else {
                sortobj.push(s);
            }
        }
        let ss = sortobj.join(",");
        dl.sortdata.insert(key.into(), (ss.clone(), sortobj));
        if truthy(&ss) || szero {
            let bee = be.borrow().etype();
            let pre = self.cfg.blx_s(Some(&self.cur), "presort", Some(&bee), Some(key));
            let ss2 = match Regex::new(&alloc::format!(r"\A{pre},+"), "") {
                Ok(r) => r.replace(&ss, false, &mut |_, _| String::new()),
                Err(_) => ss.clone(),
            };
            let init = gc_substr(&normalise_string(&ss2), 0, Some(1));
            dl.sortinit.insert(key.into(), init);
        }
    }

    fn dispatch_sorting(&mut self, f: &str, key: &str, dl: &DataList, attrs: &SortAttrs) -> String {
        let be = self.be(key);
        let bee = be.borrow().etype();
        if self.cfg.sortexclusion.get(&bee).is_some_and(|s| s.contains(f)) {
            return String::new();
        }
        if self.cfg.sortexclusion.get("*").is_some_and(|s| s.contains(f)) {
            match self.cfg.sortinclusion.get(&bee) {
                Some(si) => {
                    if !si.contains(f) {
                        return String::new();
                    }
                }
                None => return String::new(),
            }
        }
        if attrs.literal {
            return process_sort_attributes(f, attrs);
        }
        let secnum = self.cur.clone();
        let get = |f: &str| be.borrow().get(f).filter(Val::truthy);
        match f {
            "editoratype" | "editorbtype" | "editorctype" => {
                if self.cfg.blx_b(Some(&secnum), "useeditor", Some(&bee), Some(key)) {
                    if let Some(v) = get(f) {
                        return process_sort_attributes(&v.str(), attrs);
                    }
                }
                String::new()
            }
            "citeorder" => {
                let ko = self.cfg.keyorder(&secnum, key);
                if self.secr().allkeys {
                    let max = self.cfg.keyorder.get(&secnum).map_or(0.0, |m| m.values().map(|v| numify(v)).fold(0.0, f64::max));
                    let pos = self.secr().orig_order.iter().position(|k| k == key).map_or(-1, |p| p as i64);
                    let biborder = crate::model::num_str(max + (pos + 1) as f64);
                    let ako = self.cfg.keyorder(&secnum, "*");
                    if let (Some(k), Some(a)) = (&ko, &ako) {
                        if numify(a) < numify(k) {
                            return biborder;
                        }
                    }
                    ko.filter(|s| truthy(s)).unwrap_or(biborder)
                } else {
                    ko.filter(|s| truthy(s)).unwrap_or_default()
                }
            }
            "citecount" => self.secr().citecount(key),
            "intciteorder" => self.cfg.internal_keyorder.get(&secnum).and_then(|m| m.get(key)).cloned().unwrap_or_default(),
            "labelalpha" => process_sort_attributes(&dl.entryfield_s(key, "sortlabelalpha").unwrap_or_default(), attrs),
            "labelname" => match be.borrow().labelname.clone() {
                Some(l) => self.dispatch_sorting(&l, key, dl, attrs),
                None => String::new(),
            },
            "labeltitle" => match be.borrow().labeltitle.clone() {
                Some(l) => self.dispatch_sorting(&l, key, dl, attrs),
                None => String::new(),
            },
            "labelyear" | "labelmonth" | "labelday" => {
                let ldi = be.borrow().labeldate.clone();
                if let Some(crate::model::LabelDate::Field { year, month, day, .. }) = ldi {
                    let c = match f {
                        "labelyear" => Some(year),
                        "labelmonth" => month,
                        _ => day,
                    };
                    if let Some(c) = c.filter(|s| truthy(s)) {
                        return self.dispatch_sorting(&c, key, dl, attrs);
                    }
                }
                String::new()
            }
            "presort" => process_sort_attributes(&self.cfg.blx_s(Some(&secnum), "presort", Some(&bee), Some(key)), attrs),
            "sortname" => {
                let any = self.dm.fields_of_type("list", &["name"]).iter().any(|n| self.cfg.blx_b(Some(&secnum), &alloc::format!("use{n}"), Some(&bee), Some(key)));
                if get("sortname").is_some() && any {
                    let s = self.namestring(key, "sortname", dl);
                    process_sort_attributes(&s, attrs)
                } else {
                    String::new()
                }
            }
            "entrytype" => process_sort_attributes(&bee, attrs),
            "entrykey" => process_sort_attributes(key, attrs),
            _ => {
                if !self.dm.is_field(f) {
                    self.warnings.push(alloc::format!("Field '{f}' in sorting template is not a sortable field"));
                    return String::new();
                }
                let (ft, dt) = (self.dm.fieldtype(f), self.dm.datatype(f));
                let Some(v) = get(f) else { return String::new() };
                if ft == "list" && dt == "name" {
                    let uo = alloc::format!("use{f}");
                    if self.cfg.optscope.contains_key(&uo) && !self.cfg.blx_b(Some(&secnum), &uo, Some(&bee), Some(key)) {
                        return String::new();
                    }
                    let s = self.namestring(key, f, dl);
                    process_sort_attributes(&s, attrs)
                } else if dt == "verbatim" || dt == "uri" {
                    if ft == "list" {
                        process_sort_attributes(&self.liststring(key, f, true), attrs)
                    } else {
                        process_sort_attributes(&normalise_string_sort(&self.cfg, &v.str(), f), attrs)
                    }
                } else if ft == "field" && (dt == "literal" || dt == "key") {
                    process_sort_attributes(&normalise_string_sort(&self.cfg, &v.str(), f), attrs)
                } else if ft == "field" && (dt == "integer" || dt == "datepart") {
                    let mut s = v.str();
                    if isroman(&nfkd(&s)) && !self.cfg.blx_b(Some(&secnum), "noroman", Some(&bee), Some(key)) {
                        s = roman2int(&nfkd(&s)).to_string();
                    }
                    if let Some(n) = crate::unicode::num(&s) {
                        s = n.to_string();
                    }
                    process_sort_attributes(&s, attrs)
                } else if ft == "list" && (dt == "literal" || dt == "key") {
                    process_sort_attributes(&self.liststring(key, f, false), attrs)
                } else {
                    String::new()
                }
            }
        }
    }

    /// Internals `_namestring`.
    fn namestring(&mut self, key: &str, field: &str, dl: &DataList) -> String {
        let secnum = self.cur.clone();
        let be = self.be(key);
        let bee = be.borrow().etype();
        let Some(names) = be.borrow().get(field).and_then(|v| v.as_names()) else { return String::new() };
        let names = names.borrow();
        let mut useprefix = self.cfg.blx_s(Some(&secnum), "useprefix", Some(&bee), Some(key));
        let mut snk = dl.sortingnamekeytemplatename.clone();
        if let Some(v) = self.cfg.blx(Some(&secnum), "sortingnamekeytemplatename", None, Some(key)) {
            snk = v.s();
        }
        if let Some(v) = names.opt("sortingnamekeytemplatename") {
            snk = v;
        }
        let (vc, _, vs, _) = dl.vis(&names.id);
        let mut visible = vs;
        if self.cfg.sortingnamekeytemplate.get(&snk).is_some_and(|t| t.visibility == "cite") {
            visible = vc;
        }
        if let Some(v) = names.opt("useprefix") {
            useprefix = v;
        }
        let mut s = String::new();
        for n in names.first_n(visible) {
            let n = n.borrow();
            if let Some(v) = n.opt("useprefix") {
                useprefix = v;
            }
            if let Some(v) = n.opt("sortingnamekeytemplatename") {
                snk = v;
            }
            let Some(t) = self.cfg.sortingnamekeytemplate.get(&snk).cloned() else { continue };
            for kp in &t.template {
                let mut kps = String::new();
                for np in kp {
                    match np {
                        SnkPart::NamePart { value, use_, inits } => {
                            let useopt = use_.as_ref().map(|_| alloc::format!("use{value}"));
                            let mut useval = useopt.as_ref().map_or(0.0, |o| numify(&self.cfg.blx_s(Some(&secnum), o, Some(&bee), Some(key))));
                            if useopt.as_deref() == Some("useprefix") {
                                useval = numify(&self.cfg.map_boolean("useprefix", &useprefix, true).unwrap_or_default());
                            }
                            let nps = n.namepart(value);
                            if !truthy(&nps) {
                                continue;
                            }
                            if useopt.is_none() || use_.as_deref().is_some_and(|u| numify(u) == useval) {
                                let x = if *inits {
                                    let i = normalise_string_sort(&self.cfg, &n.initial(value).concat(), field);
                                    pad(&i, self.secr().np_length(&alloc::format!("{value}-i")))
                                } else {
                                    let i = normalise_string_sort(&self.cfg, &nps, field);
                                    pad(&i, self.secr().np_length(value))
                                };
                                kps.push_str(&x);
                            }
                        }
                        SnkPart::Literal(v) => kps.push_str(v),
                    }
                }
                s.push_str(&kps);
            }
        }
        let mut nso = self.cfg.blx_b(Some(&secnum), "nosortothers", Some(&bee), Some(key));
        if let Some(v) = names.opt("nosortothers") {
            nso = truthy(&v);
        }
        if !nso && visible < names.count() {
            s.push('\u{10FFFD}');
        }
        s
    }

    /// Internals `_liststring`.
    fn liststring(&self, key: &str, field: &str, verbatim: bool) -> String {
        let be = self.be(key);
        let bee = be.borrow().etype();
        let Some(l) = be.borrow().get(field).and_then(|v| v.as_list()) else { return String::new() };
        let mut items = l.borrow().clone();
        let mut trunc = false;
        let maxi = self.cfg.blx_n(Some(&self.cur), "maxitems", Some(&bee), Some(key));
        if items.len() as i64 > maxi {
            trunc = true;
            let mini = self.cfg.blx_n(Some(&self.cur), "minitems", Some(&bee), Some(key)).max(0) as usize;
            items.truncate(mini);
        }
        let mut s = items
            .iter()
            .map(|i| if verbatim { strip_nosort(&self.cfg, i, field) } else { normalise_string_sort(&self.cfg, i, field) })
            .collect::<Vec<_>>()
            .join("!");
        s = re(r"\s+\z").delete_all(&s);
        if trunc {
            s.push('\u{10FFFD}');
        }
        s
    }
}

/// NFD(sprintf("%-*s", len, NFC(s))).
fn pad(s: &str, len: usize) -> String {
    let c = nfc(s);
    let n = c.chars().count();
    let mut o = c;
    for _ in n..len {
        o.push(' ');
    }
    nfd(&o)
}

/// Internals `_process_sort_attributes`.
pub fn process_sort_attributes(s: &str, a: &SortAttrs) -> String {
    if s == "0" {
        return "BIBERZERO".into();
    }
    if !truthy(s) {
        return s.into();
    }
    let mut s = s.to_string();
    let t = |o: &Option<String>| o.as_deref().filter(|x| truthy(x)).map(String::from);
    if t(&a.substring_width).is_some() || t(&a.substring_side).is_some() {
        let w = t(&a.substring_width).map_or(4, |x| numify(&x) as i64);
        let side = t(&a.substring_side).unwrap_or_else(|| "left".into());
        let off = if side == "right" { -w } else { 0 };
        s = gc_substr(&s, off, Some(w));
    }
    if t(&a.pad_side).is_some() || t(&a.pad_width).is_some() || t(&a.pad_char).is_some() {
        let w = t(&a.pad_width).map_or(4, |x| numify(&x) as i64);
        let side = t(&a.pad_side).unwrap_or_else(|| "left".into());
        let ch = t(&a.pad_char).unwrap_or_else(|| "0".into());
        let n = w - gc_len(&s) as i64;
        if n > 0 {
            let p = ch.repeat(n as usize);
            if side == "left" {
                s = alloc::format!("{p}{s}");
            } else if side == "right" {
                s.push_str(&p);
            }
        }
    }
    s
}

/// Utils `match_indices`.
fn match_indices(res: &[Regex], s: &str) -> Option<Vec<(String, usize)>> {
    let t: Vec<char> = s.chars().collect();
    let mut ret = Vec::new();
    let mut relen = 0usize;
    for r in res {
        let mut len = 0;
        for m in r.find_all(&t) {
            let gs: String = t[m.start..m.end].iter().collect();
            ret.push((gs.clone(), m.start.saturating_sub(relen)));
            len = gc_len(&gs);
        }
        relen += len;
    }
    if ret.is_empty() {
        None
    } else {
        ret.reverse();
        Some(ret)
    }
}

/// Internals `_label_listdisambiguation`: per row, the disambiguated
/// prefixes.
fn label_listdisambiguation(strings: &[Vec<String>]) -> Vec<Vec<String>> {
    let mut substr_map: Vec<Vec<i64>> = strings.iter().map(|r| r.iter().map(|_| 1).collect()).collect();
    let mut data: Vec<Option<Vec<String>>> = strings.iter().map(|_| None).collect();
    let mut name_map: BTreeMap<usize, usize> = BTreeMap::new();
    type Keys = BTreeMap<String, (Vec<usize>, Vec<String>)>;
    let do_substr = |data: &Vec<Option<Vec<String>>>, sm: &Vec<Vec<i64>>| -> Keys {
        let mut keys: Keys = BTreeMap::new();
        for (i, row) in strings.iter().enumerate() {
            if data[i].is_some() {
                continue;
            }
            let s: Vec<String> = row.iter().enumerate().map(|(j, x)| gc_substr(x, 0, Some(sm[i][j]))).collect();
            let e = keys.entry(s.concat()).or_default();
            e.0.push(i);
            e.1 = s;
        }
        keys
    };
    let mut keys = do_substr(&data, &substr_map);
    let mut guard = 0;
    while data.iter().any(Option::is_none) && guard < 10_000 {
        guard += 1;
        let mut amb = Vec::new();
        for (idx, s) in keys.values() {
            if idx.len() > 1 {
                amb.push(idx.clone());
            } else {
                data[idx[0]] = Some(s.clone());
            }
        }
        for ai in amb {
            let ast: Vec<Vec<String>> = ai.iter().map(|&i| strings[i].clone()).collect();
            if ast.iter().all(|x| *x == ast[0]) {
                data[ai[0]] = Some(ast[0].iter().map(|x| gc_substr(x, 0, Some(1))).collect());
            } else {
                for i in 0..ast.len() {
                    let check: Vec<&Vec<String>> = ast.iter().enumerate().filter(|(j, x)| *j != i && **x != ast[i]).map(|(_, x)| x).collect();
                    for j in 0..ast[0].len() {
                        let v = ast[i].get(j).cloned().unwrap_or_default();
                        if !check.iter().any(|c| c.get(j).cloned().unwrap_or_default() == v) {
                            name_map.insert(ai[i], j);
                            break;
                        }
                    }
                }
                if let Some(&j) = name_map.get(&ai[0]) {
                    if let Some(x) = substr_map[ai[0]].get_mut(j) {
                        *x += 1;
                    }
                }
            }
            keys = do_substr(&data, &substr_map);
        }
    }
    data.into_iter().map(Option::unwrap_or_default).collect()
}

