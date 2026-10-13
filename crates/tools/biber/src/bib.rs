//! Biber::Input::file::bibtex: reading `.bib` data sources (two btparse
//! passes with LaTeX decoding between them), source maps, and turning
//! Text::BibTeX entries into Biber entries by data model type.

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::biber::{Biber, Fatal};
use crate::btparse::{self, BtEntry, Meta};
use crate::config::split_comma;
use crate::model::{shared, truthy, Entry, Name, NamePart, Names, Val, XRef};
use crate::regex::{self, Regex};
use crate::unicode::{self, fc, lc, nfc, nfd};
use crate::xml::Element;

/// The cache of parsed data sources (Perl's `state $cache`).
#[derive(Default)]
pub struct BibCache {
    pub data: BTreeMap<String, BTreeMap<String, BtEntry>>,
    pub globalds: BTreeMap<String, BtEntry>,
    pub aliases: BTreeMap<String, String>,
    pub orig_key_order: BTreeMap<String, Vec<String>>,
    pub preamble: BTreeMap<String, Vec<String>>,
    pub counts: BTreeMap<String, usize>,
}

pub enum Created {
    Ok,
    NotCreated,
}

const MONTHS: [(&str, &str); 12] = [
    ("jan", "1"),
    ("feb", "2"),
    ("mar", "3"),
    ("apr", "4"),
    ("may", "5"),
    ("jun", "6"),
    ("jul", "7"),
    ("aug", "8"),
    ("sep", "9"),
    ("oct", "10"),
    ("nov", "11"),
    ("dec", "12"),
];

pub fn add_month_macros(m: &mut btparse::Macros) {
    for (k, v) in MONTHS {
        m.0.insert(k.into(), v.into());
    }
}

/// Text::Balanced-style check that the whole string is one `{...}` group;
/// Utils `remove_outer`.
pub fn remove_outer(s: &str) -> (bool, String) {
    let c: Vec<char> = s.chars().collect();
    if c.first() != Some(&'{') {
        return (false, s.into());
    }
    let mut depth = 0i64;
    let mut i = 0;
    let mut end = None;
    while i < c.len() {
        match c[i] {
            '\\' => {
                i += 2;
                continue;
            }
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = Some(i);
                    break;
                }
            }
            _ => {}
        }
        i += 1;
    }
    if end != Some(c.len() - 1) {
        return (false, s.into());
    }
    if c.len() < 3 {
        return (false, s.into());
    }
    (true, c[1..c.len() - 1].iter().collect())
}

pub fn has_outer(s: &str) -> bool {
    // return 0 if m/}\s*{/; return m/^{\X+}$/
    let c: Vec<char> = s.chars().collect();
    for i in 0..c.len() {
        if c[i] == '}' {
            let mut j = i + 1;
            while j < c.len() && unicode::is_space(c[j]) {
                j += 1;
            }
            if j < c.len() && c[j] == '{' {
                return false;
            }
        }
    }
    c.len() >= 3 && c[0] == '{' && c[c.len() - 1] == '}'
}

/// Utils `gen_initials`.
pub fn gen_initials(strings: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for s in strings {
        let braced = s.len() >= 3 && s.starts_with('{') && s.ends_with('}');
        let c: Vec<char> = s.chars().collect();
        let dash_inside = (1..c.len().saturating_sub(1)).any(|i| unicode::is_dash(c[i]) && c[i - 1] != '{' && c[i + 1] != '}');
        if !braced && dash_inside {
            let parts: Vec<String> = s.split(unicode::is_dash).map(Into::into).collect();
            out.push(gen_initials(&parts).join("-"));
        } else {
            let t = s.trim_start_matches('{');
            let first = unicode::gc_substr(t, 0, Some(1));
            if first.chars().next().is_some_and(unicode::is_dia) {
                out.push(unicode::gc_substr(t, 0, Some(2)));
            } else {
                out.push(first);
            }
        }
    }
    out
}

/// Utils `join_name_parts`.
fn join_name_parts(parts: &[String]) -> String {
    match parts.len() {
        0 => String::new(),
        1 => parts[0].clone(),
        2 => alloc::format!("{}~{}", parts[0], parts[1]),
        n => {
            let mut s = parts[0].clone();
            s.push(if unicode::gc_len(&parts[0]) < 3 { '~' } else { ' ' });
            s.push_str(&parts[1..n - 1].join(" "));
            s.push('~');
            s.push_str(&parts[n - 1]);
            s
        }
    }
}

/// Perl `split(/RE/, s)` (trailing empty fields dropped).
pub fn re_split(re: &Regex, s: &str) -> Vec<String> {
    let t: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut last = 0;
    for m in re.find_all(&t) {
        if m.end == m.start && (m.start == 0 || m.start == t.len()) {
            continue;
        }
        if m.start == 0 && m.end > 0 && last == 0 {
            // a leading separator gives a leading empty field
            out.push(String::new());
            last = m.end;
            continue;
        }
        out.push(t[last..m.start].iter().collect());
        last = m.end;
    }
    out.push(t[last..].iter().collect());
    while out.last().is_some_and(String::is_empty) {
        out.pop();
    }
    out
}

/// `_hack_month`.
fn hack_month(v: &str) -> String {
    let t = v.trim_start();
    let l = lc(t);
    for (k, n) in MONTHS {
        if l.starts_with(k) {
            return n.into();
        }
    }
    v.into()
}

/// `_split_initials`.
fn split_initials(v: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut ci = false;
    let mut acc = String::new();
    for g in unicode::graphemes(v) {
        if g == "{" {
            ci = true;
        } else if g == "}" {
            ci = false;
            out.push(core::mem::take(&mut acc));
        } else if ci {
            acc.push_str(g);
        } else {
            out.push(g.into());
        }
    }
    out
}

impl Biber<'_> {
    fn sm_warn(&mut self, s: String) {
        self.warnings.push(s);
    }

    /// `extract_entries`: returns the keys still not found.
    pub fn extract_entries(&mut self, filename: &str, bytes: &[u8], keys: &[String]) -> Result<Vec<String>, Fatal> {
        let mut rkeys: Vec<String> = keys.to_vec();
        if bytes.is_empty() {
            self.warnings.push(alloc::format!("Data source '{filename}' is empty, ignoring"));
            return Ok(rkeys);
        }
        let Ok(text) = core::str::from_utf8(bytes) else {
            return Err(self.fatal(alloc::format!("Data file '{filename}' cannot be read in encoding 'UTF-8'")));
        };
        if !text.contains('@') {
            self.warnings.push(alloc::format!("Data source '{filename}' contains no BibTeX entries/macros, ignoring"));
            return Ok(rkeys);
        }
        let mut smaps: Vec<Element> = Vec::new();
        for lvl in ["user", "style"] {
            for m in &self.cfg.sourcemap {
                if m.attr("datatype") == Some("bibtex") && m.attr("level") == Some(lvl) {
                    smaps.push(m.clone());
                }
            }
        }
        if let Some(m) = self.cfg.sourcemap.iter().find(|m| m.attr("datatype") == Some("bibtex") && m.attr("level") == Some("driver")) {
            smaps.push(m.clone());
        }
        *self.bibcache.counts.entry(filename.into()).or_insert(0) += 1;
        if !self.bibcache.data.contains_key(filename) {
            self.cache_data(filename, text)?;
        }
        let secnum = self.cur.clone();
        if self.sec().allkeys {
            let order = self.bibcache.orig_key_order.get(filename).cloned().unwrap_or_default();
            for key in &order {
                self.sec().keytods.insert(key.clone(), filename.into());
                let e = self.bibcache.data[filename][key].clone();
                if let Created::NotCreated = self.create_entry(key, e, filename, &smaps, &mut rkeys)? {
                    if let Some(o) = self.bibcache.orig_key_order.get_mut(filename) {
                        o.retain(|k| k != key);
                    }
                }
            }
            let aliases: Vec<(String, String)> = self.bibcache.aliases.iter().map(|(a, b)| (a.clone(), b.clone())).collect();
            for (a, k) in aliases {
                self.sec().alias.insert(a, k);
            }
            let order = self.bibcache.orig_key_order.get(filename).cloned().unwrap_or_default();
            self.sec().add_citekeys(&order);
            if !self.sec().dkeys.is_empty() {
                let d = self.sec().dynamic_set_keys();
                self.sec().add_citekeys(&d);
            }
        } else {
            for wanted in keys {
                self.sec().keytods.insert(wanted.clone(), filename.into());
                if let Some(e) = self.bibcache.data.get(filename).and_then(|m| m.get(wanted)).cloned() {
                    if !self.sec().entries.contains_key(wanted) {
                        if let Created::NotCreated = self.create_entry(wanted, e, filename, &smaps, &mut rkeys)? {
                            self.sec().del_citekey(wanted);
                            if let Some(o) = self.bibcache.orig_key_order.get_mut(filename) {
                                o.retain(|k| k != wanted);
                            }
                            self.warnings.push(alloc::format!(
                                "Entry with key '{wanted}' in section '{secnum}' is cited and found but not created (likely due to sourcemap)"
                            ));
                        }
                    }
                    rkeys.retain(|k| k != wanted);
                } else if let Some(rk) = self.bibcache.aliases.get(wanted).cloned() {
                    self.sec().alias.insert(wanted.clone(), rk.clone());
                    if !self.sec().entries.contains_key(&rk) {
                        if let Some(e) = self.bibcache.globalds.get(&rk).cloned() {
                            if let Created::NotCreated = self.create_entry(&rk, e, filename, &smaps, &mut rkeys)? {
                                self.sec().del_citekey(wanted);
                                if let Some(o) = self.bibcache.orig_key_order.get_mut(filename) {
                                    o.retain(|k| *k != rk);
                                }
                                self.warnings.push(alloc::format!(
                                    "Entry with key '{rk}' in section '{secnum}' is cited and found but not created (likely due to sourcemap)"
                                ));
                            }
                            if self.sec().has_cited(wanted) {
                                self.sec().add_citekeys(&[rk.clone()]);
                            }
                        }
                    }
                    rkeys.retain(|k| k != wanted);
                } else if let Some(okey) = self.sec().badcasekey(wanted) {
                    self.warnings.push(alloc::format!(
                        "Possible typo (case mismatch) between citation and datasource keys: '{wanted}' and '{okey}' in file '{filename}'"
                    ));
                }
            }
        }
        for w in core::mem::take(&mut self.bt_warnings) {
            self.warnings.push(alloc::format!("BibTeX subsystem: {filename}, {w}"));
        }
        if self.bibcache.counts[filename] < 2 {
            if let Some(p) = self.bibcache.preamble.get(filename) {
                self.preamble.extend(p.iter().cloned());
            }
        }
        Ok(rkeys)
    }

    fn cache_data(&mut self, filename: &str, text: &str) -> Result<(), Fatal> {
        self.bibcache.preamble.insert(filename.into(), Vec::new());
        let mut buf = nfd(text);
        if let Some(b) = buf.strip_prefix('\u{feff}') {
            buf = b.into();
        }
        buf = buf.replace("\r\n", "\n").replace(['\r', '\u{0b}', '\u{0c}', '\u{85}', '\u{2028}', '\u{2029}'], "\n");
        // first pass: parse and LaTeX-decode
        let mut warns = Vec::new();
        let first = btparse::parse(&buf, &mut self.macros, &mut warns).map_err(|e| self.fatal(alloc::format!("BibTeX subsystem: {filename}, {e}")))?;
        self.bt_warnings.extend(warns);
        let mut lbuf = String::new();
        for e in &first {
            match e.meta {
                Meta::Regular => {
                    lbuf.push('@');
                    lbuf.push_str(&e.typ);
                    lbuf.push('{');
                    lbuf.push_str(e.key.as_deref().unwrap_or(""));
                    lbuf.push_str(",\n");
                    for f in &e.fieldlist {
                        let mut fv = e.get(f).unwrap_or("").to_string();
                        if !self.dm.helpers.verbs.iter().any(|v| fc(v) == fc(f)) {
                            fv = self.recode.decode(&fv);
                        }
                        lbuf.push_str(&alloc::format!("  {f} = {{{fv}}},\n"));
                    }
                    lbuf.push_str("\n}\n\n");
                }
                Meta::Preamble => {
                    lbuf.push_str("@PREAMBLE{\"");
                    lbuf.push_str(&e.value);
                    lbuf.push_str("\"}\n");
                }
                Meta::Comment => {
                    lbuf.push_str("@COMMENT{");
                    lbuf.push_str(&e.value);
                    lbuf.push_str("}\n");
                }
                Meta::MacroDef => {
                    lbuf.push_str("@STRING{");
                    for f in &e.fieldlist {
                        lbuf.push_str(&alloc::format!("{f} = {{{}}}", e.get(f).unwrap_or("")));
                    }
                    lbuf.push_str("}\n");
                }
            }
        }
        if !self.cfg.opt_true("nostdmacros") {
            add_month_macros(&mut self.macros);
        }
        let lbuf = nfd(&nfc(&lbuf));
        let mut warns = Vec::new();
        let entries = btparse::parse(&lbuf, &mut self.macros, &mut warns).map_err(|e| self.fatal(alloc::format!("BibTeX subsystem: {filename}, {e}")))?;
        let _ = warns;
        let mut data: BTreeMap<String, BtEntry> = BTreeMap::new();
        let mut order: Vec<String> = Vec::new();
        let xsv = Regex::new(&self.cfg.opt("xsvsep"), "").map_err(|e| self.fatal(e))?;
        for e in entries {
            match e.meta {
                Meta::Preamble => {
                    self.bibcache.preamble.get_mut(filename).expect("inserted").push(e.value.clone());
                    continue;
                }
                Meta::Comment | Meta::MacroDef => continue,
                Meta::Regular => {}
            }
            let Some(key) = e.key.clone().filter(|k| !k.is_empty()) else {
                self.warnings.push(alloc::format!("Invalid or undefined BibTeX entry key in file '{filename}', skipping ..."));
                continue;
            };
            if self.bibcache.aliases.contains_key(&key) {
                self.warnings.push(alloc::format!("Citekey alias '{key}' is also a real entry key, skipping ..."));
                self.bibcache.aliases.remove(&key);
            }
            if let Some(ids) = e.get("ids").filter(|s| truthy(s)) {
                for id in re_split(&xsv, ids) {
                    if id == key {
                        self.warnings.push(alloc::format!("BAD RECURSION! Entry alias '{id}' is identical to the entry key, skipping ..."));
                        continue;
                    }
                    if self.sec().everykey.contains(&id) {
                        self.warnings.push(alloc::format!("Entry alias '{id}' is also a real entry key, skipping ..."));
                        continue;
                    }
                    if let Some(other) = self.bibcache.aliases.get(&id) {
                        if *other != key {
                            let o = other.clone();
                            self.warnings.push(alloc::format!("Entry alias '{id}' already has an alias '{o}', skipping ..."));
                        }
                    } else {
                        self.bibcache.aliases.insert(id, key.clone());
                    }
                }
            }
            if let Some(okey) = self.sec().badcasekey(&key) {
                self.warnings.push(alloc::format!("Possible typo (case mismatch) between datasource keys: '{key}' and '{okey}' in file '{filename}'"));
            }
            if self.sec().everykey.contains(&key) && !self.cfg.opt_true("noskipduplicates") {
                self.warnings.push(alloc::format!("Duplicate entry key: '{key}' in file '{filename}', skipping ..."));
                continue;
            }
            self.sec().add_everykey(&key);
            self.bibcache.globalds.insert(key.clone(), e.clone());
            data.insert(key.clone(), e);
            order.push(key);
        }
        self.bibcache.data.insert(filename.into(), data);
        self.bibcache.orig_key_order.insert(filename.into(), order);
        Ok(())
    }

    fn step_skip(step: &Element) -> u8 {
        // 1 = next step, 2 = next MAP (final)
        if step.attr("map_final").is_some_and(truthy) {
            2
        } else {
            1
        }
    }

    /// `create_entry` with source mapping.
    fn create_entry(&mut self, key: &str, entry0: BtEntry, datasource: &str, smaps: &[Element], rkeys: &mut Vec<String>) -> Result<Created, Fatal> {
        let mut entry = entry0;
        let mut newentries: Vec<(String, BtEntry)> = Vec::new();
        let secnum = self.cur.clone();
        for smap in smaps {
            let level = smap.attr("level").unwrap_or("").to_string();
            let smap_over = smap.attr("map_overwrite").unwrap_or("0").to_string();
            for map in smap.kids("map") {
                if let Some(rs) = map.attr("refsection") {
                    if crate::model::numify(&secnum) != crate::model::numify(rs) {
                        continue;
                    }
                }
                let pt: Vec<&Element> = map.kids("per_type").collect();
                if !pt.is_empty() && !pt.iter().any(|t| fc(&t.text) == fc(&entry.typ)) {
                    continue;
                }
                if map.kids("per_nottype").any(|t| fc(&t.text) == fc(&entry.typ)) {
                    continue;
                }
                let test_path = if datasource.starts_with('/') { datasource.rsplit('/').next().unwrap_or("") } else { datasource };
                let pd: Vec<&Element> = map.kids("per_datasource").collect();
                if !pd.is_empty() && !pd.iter().any(|t| t.text == test_path) {
                    continue;
                }
                let over = map.attr("map_overwrite").map_or_else(|| smap_over.clone(), Into::into);
                let mut last_type = entry.typ.clone();
                let mut last_field: Option<String> = None;
                let mut last_fieldval: Option<String> = None;
                let mut imatches: Vec<String> = Vec::new();
                let mut maploop: Vec<String> = alloc::vec![String::new()];
                if let Some(fe) = map.attr("map_foreach") {
                    if let Some(ds) = self.cfg.datafield_sets.get(&fc(fe)) {
                        maploop = ds.clone();
                    } else if let Some(fl) = entry.get(&fc(fe)).filter(|s| truthy(s)) {
                        maploop = split_comma(fl);
                    } else {
                        maploop = split_comma(fe);
                    }
                }
                'map: for ml in &maploop {
                    for step in map.kids("map_step") {
                        let mr = |s: Option<&str>| -> Option<String> {
                            let s = s?;
                            if s.contains("$MAPUNIQ") {
                                return None;
                            }
                            if ml.is_empty() {
                                Some(s.into())
                            } else {
                                Some(s.replace("$MAPLOOP", ml))
                            }
                        };
                        let a = |n: &str| step.attr(n);
                        if a("map_entry_null").is_some_and(truthy) {
                            return Ok(Created::NotCreated);
                        }
                        for n in ["map_entry_new", "map_entry_clone", "map_entrytarget", "map_type_source", "map_field_source", "map_field_set", "map_field_value", "map_match", "map_replace"] {
                            if a(n).is_some_and(|s| s.contains("$MAPUNIQ")) {
                                return Err(self.unsupported("$MAPUNIQ in source maps"));
                            }
                        }
                        let subst_imatch = |s: &str, im: &[String]| -> String {
                            let re = Regex::new(r"(?<!\\)\$(\d)", "").expect("static");
                            re.replace(s, true, &mut |t, m| {
                                let n: usize = regex::cap(t, m, 1).and_then(|x| x.parse().ok()).unwrap_or(0);
                                im.get(n.wrapping_sub(1)).cloned().unwrap_or_default()
                            })
                        };
                        if let Some(nk) = mr(a("map_entry_new")).filter(|s| truthy(s)) {
                            let nk = subst_imatch(&nk, &imatches);
                            let Some(nt) = mr(a("map_entry_newtype")).filter(|s| truthy(s)) else {
                                self.sm_warn(alloc::format!("Source mapping (type={level}, key={key}): Missing type for new entry '{nk}', skipping step ..."));
                                continue;
                            };
                            rkeys.retain(|k| *k != nk);
                            if a("map_entry_nocite").is_some_and(truthy) {
                                self.sec().nocite.insert(nk.clone());
                                self.sec().add_citekeys(&[nk.clone()]);
                            }
                            if self.sec().allkeys {
                                self.sec().add_citekeys(&[nk.clone()]);
                            }
                            let ne = BtEntry {
                                meta: Meta::Regular,
                                typ: nfd(&nt),
                                key: Some(nfd(&nk)),
                                fieldlist: Vec::new(),
                                values: BTreeMap::new(),
                                value: String::new(),
                                line: 0,
                            };
                            newentries.retain(|(k, _)| *k != nk);
                            newentries.push((nk, ne));
                        }
                        if let Some(ck) = mr(a("map_entry_clone")).filter(|s| truthy(s)) {
                            let ck = subst_imatch(&ck, &imatches);
                            rkeys.retain(|k| *k != ck);
                            if a("map_entry_nocite").is_some_and(truthy) {
                                self.sec().nocite.insert(ck.clone());
                                self.sec().add_citekeys(&[ck.clone()]);
                            }
                            if self.sec().allkeys {
                                self.sec().add_citekeys(&[ck.clone()]);
                            }
                            let c = entry.clone();
                            newentries.retain(|(k, _)| *k != ck);
                            newentries.push((ck, c));
                        }
                        // target entry
                        let target_new: Option<String> = match mr(a("map_entrytarget")).filter(|s| truthy(s)) {
                            Some(t) => {
                                let t = subst_imatch(&t, &imatches);
                                if !newentries.iter().any(|(k, _)| *k == t) {
                                    self.sm_warn(alloc::format!(
                                        "Source mapping (type={level}, key={key}): Dynamically created entry target '{t}' does not exist skipping step ..."
                                    ));
                                    continue;
                                }
                                Some(t)
                            }
                            None => None,
                        };
                        macro_rules! et {
                            () => {
                                match &target_new {
                                    Some(t) => &mut newentries.iter_mut().find(|(k, _)| k == t).expect("exists").1,
                                    None => &mut entry,
                                }
                            };
                        }
                        let skip = Self::step_skip(step);
                        if let Some(ts) = mr(a("map_type_source")).filter(|s| truthy(s)) {
                            let ts = fc(&ts);
                            if et!().typ != ts {
                                if skip == 2 {
                                    continue 'map;
                                }
                                continue;
                            }
                            last_type = et!().typ.clone();
                            let t = fc(&mr(a("map_type_target")).unwrap_or_default());
                            et!().typ = nfd(&t);
                        }
                        let mut fieldcontinue = false;
                        if let Some(nf) = mr(a("map_notfield")).filter(|s| truthy(s)) {
                            if et!().exists(&fc(&nf)) {
                                if skip == 2 {
                                    continue 'map;
                                }
                                continue;
                            }
                            fieldcontinue = true;
                        }
                        if a("map_entrykey_citedornocited").is_some_and(truthy) && !self.sec().is_specific(key) {
                            if skip == 2 {
                                continue 'map;
                            }
                            continue;
                        }
                        if a("map_entrykey_cited").is_some_and(truthy) && !self.sec().cite.contains(key) {
                            if skip == 2 {
                                continue 'map;
                            }
                            continue;
                        }
                        if a("map_entrykey_nocited").is_some_and(truthy) {
                            let s = self.sec();
                            if s.cite.contains(key) || (!s.nocite.contains(key) && !s.allkeys_nocite) {
                                if skip == 2 {
                                    continue 'map;
                                }
                                continue;
                            }
                        }
                        if a("map_entrykey_allnocited").is_some_and(truthy) && !self.sec().allkeys_nocite {
                            if skip == 2 {
                                continue 'map;
                            }
                            continue;
                        }
                        if a("map_entrykey_starnocited").is_some_and(truthy) {
                            let s = self.sec();
                            if s.allkeys_nocite && (s.cite.contains(key) || s.nocite.contains(key)) {
                                if skip == 2 {
                                    continue 'map;
                                }
                                continue;
                            }
                        }
                        let mut fieldsource: Option<String> = None;
                        if let Some(fs) = mr(a("map_field_source")).filter(|s| truthy(s)) {
                            let fs = fc(&fs);
                            if !(fs == "entrykey" || et!().exists(&fs)) {
                                if skip == 2 {
                                    continue 'map;
                                }
                                continue;
                            }
                            fieldsource = Some(fs);
                            fieldcontinue = true;
                        }
                        if fieldcontinue {
                            let fsrc = fieldsource.clone().unwrap_or_default();
                            last_field = fieldsource.clone();
                            last_fieldval = if fsrc == "entrykey" { et!().key.clone() } else { et!().get(&fsrc).map(Into::into) };
                            let lfv = last_fieldval.clone().unwrap_or_default();
                            let mut negmatch = false;
                            let mut ci = false;
                            let mut m_pat = a("map_match").map(String::from);
                            if let Some(nm) = a("map_notmatch").or_else(|| a("map_notmatchi")).filter(|s| truthy(s)) {
                                m_pat = Some(nm.into());
                                negmatch = true;
                            }
                            if let Some(mi) = a("map_matchi").or_else(|| a("map_notmatchi")).filter(|s| truthy(s)) {
                                m_pat = Some(mi.into());
                                ci = true;
                            }
                            let mut matches = a("map_matches").map(String::from);
                            let mut cis = false;
                            if let Some(mis) = a("map_matchesi").filter(|s| truthy(s)) {
                                matches = Some(mis.into());
                                cis = true;
                            }
                            if let Some(ms) = matches.filter(|s| truthy(s)) {
                                let ms = split_comma(&ms);
                                let rs = split_comma(a("map_replace").unwrap_or(""));
                                if ms.len() != rs.len() {
                                    continue;
                                }
                                for (i, m) in ms.iter().enumerate() {
                                    if (cis && fc(&lfv) == fc(m)) || lfv == *m {
                                        let r = nfd(&rs[i]);
                                        et!().set(&fsrc, &r);
                                    }
                                }
                            }
                            if let Some(m) = m_pat.as_deref().and_then(|x| mr(Some(x))).filter(|s| truthy(s)) {
                                if let Some(r) = a("map_replace") {
                                    if fsrc == "entrykey" {
                                        continue;
                                    }
                                    let r = mr(Some(r)).unwrap_or_default();
                                    let re = Regex::new(&m, if ci { "i" } else { "" }).map_err(|e| self.unsupported(&alloc::format!("source map pattern ({e})")))?;
                                    let v = re.replace(&lfv, true, &mut |t, mm| interpolate(&r, t, mm));
                                    et!().set(&fsrc, &nfd(&nfc(&v)));
                                } else {
                                    let m = subst_imatch(&m, &imatches);
                                    let re = Regex::new(&m, if ci { "i" } else { "" }).map_err(|e| self.unsupported(&alloc::format!("source map pattern ({e})")))?;
                                    let t: Vec<char> = lfv.chars().collect();
                                    let ms = re.find_all(&t);
                                    let res: Vec<String> = if negmatch {
                                        if ms.is_empty() {
                                            alloc::vec!["1".into()]
                                        } else {
                                            Vec::new()
                                        }
                                    } else {
                                        let mut v = Vec::new();
                                        for mm in &ms {
                                            if mm.caps.len() > 1 {
                                                for i in 1..mm.caps.len() {
                                                    v.push(regex::cap(&t, mm, i).unwrap_or_default());
                                                }
                                            } else {
                                                v.push(t[mm.start..mm.end].iter().collect());
                                            }
                                        }
                                        v
                                    };
                                    imatches = res;
                                    if imatches.is_empty() {
                                        if skip == 2 {
                                            continue 'map;
                                        }
                                        continue;
                                    }
                                }
                            }
                            if let Some(tg) = mr(a("map_field_target")).filter(|s| truthy(s)) {
                                let tg = fc(&tg);
                                if fsrc == "entrykey" {
                                    continue;
                                }
                                if et!().exists(&tg) && !truthy(&over) {
                                    continue;
                                }
                                let v = entry.get(&fsrc).unwrap_or("").to_string();
                                et!().set(&tg, &v);
                                et!().delete(&fsrc);
                            }
                        }
                        if let Some(field) = mr(a("map_field_set")).filter(|s| truthy(s)) {
                            let field = fc(&field);
                            if a("map_null").is_some_and(truthy) {
                                et!().delete(&field);
                            } else {
                                if et!().exists(&field) && !truthy(&over) {
                                    if skip == 2 {
                                        continue 'map;
                                    }
                                    continue;
                                }
                                let append = a("map_append").is_some_and(truthy) || a("map_appendstrict").is_some_and(truthy);
                                let orig = if append { et!().get(&field).filter(|s| truthy(s)).unwrap_or("").to_string() } else { String::new() };
                                let strict = a("map_appendstrict").is_some_and(truthy);
                                let app = |o: &str, v: &str| -> String {
                                    if strict {
                                        if truthy(o) {
                                            alloc::format!("{o}{v}")
                                        } else {
                                            String::new()
                                        }
                                    } else {
                                        alloc::format!("{o}{v}")
                                    }
                                };
                                if a("map_origentrytype").is_some_and(truthy) {
                                    if !truthy(&last_type) {
                                        continue;
                                    }
                                    let v = app(&orig, &last_type);
                                    et!().set(&field, &nfd(&v));
                                } else if a("map_origfieldval").is_some_and(truthy) {
                                    let Some(lv) = last_fieldval.clone().filter(|s| truthy(s)) else { continue };
                                    let v = app(&orig, &lv);
                                    et!().set(&field, &nfd(&v));
                                } else if a("map_origfield").is_some_and(truthy) {
                                    let Some(lf) = last_field.clone().filter(|s| truthy(s)) else { continue };
                                    let v = app(&orig, &lf);
                                    et!().set(&field, &nfd(&v));
                                } else {
                                    let fv = mr(a("map_field_value")).unwrap_or_default();
                                    let fv = subst_imatch(&fv, &imatches);
                                    let v = app(&orig, &fv);
                                    et!().set(&field, &nfd(&v));
                                }
                            }
                        }
                    }
                }
            }
        }
        let r = self.create_entry_inner(key, &entry)?;
        for (k, e) in newentries {
            self.create_entry_inner(&k, &e)?;
        }
        Ok(r)
    }

    fn create_entry_inner(&mut self, k: &str, e: &BtEntry) -> Result<Created, Fatal> {
        let mut be = Entry::default();
        be.set_field("citekey", Val::s(k));
        be.set_field("entrytype", Val::s(fc(&e.typ)));
        let secnum = self.cur.clone();
        let fields = e.fieldlist.clone();
        for f in &fields {
            let f_c = fc(f);
            let value = e.get(f).unwrap_or("").to_string();
            if f_c == "options" {
                let xsv = Regex::new(&self.cfg.opt("xsvsep"), "").map_err(|x| self.fatal(x))?;
                let opts = re_split(&xsv, &value);
                self.process_entry_options(k, &opts, &secnum);
            }
            if self.dm.is_field(&f_c) {
                if !value.is_empty() {
                    match self.handle_field(&mut be, e, f, &f_c, k, &value)? {
                        Handled::Skip => return Ok(Created::NotCreated),
                        Handled::Val(v) => be.set_data(&f_c, v),
                        Handled::None => {}
                    }
                }
            }
        }
        be.set_field("datatype", Val::s("bibtex"));
        self.sec().entries.insert(k.into(), shared(be));
        Ok(Created::Ok)
    }

    /// Add an XDATA reference if the value is one.
    fn add_xdata_ref(&mut self, be: &mut Entry, reffield: &str, value: XdVal<'_>, pos: Option<usize>) -> bool {
        if reffield == "xdata" {
            let XdVal::L(v) = value else { return false };
            be.xrefs.get_or_insert_with(Vec::new).push(XRef {
                reffield: "xdata".into(),
                refposition: 0,
                entries: v.to_vec(),
                field: None,
                position: "0".into(),
                resolved: None,
            });
            return true;
        }
        let XdVal::S(v) = value else { return false };
        let marker = alloc::format!("{}{}", self.cfg.opt("xdatamarker"), self.cfg.opt("xnamesep"));
        let lv = lc(v);
        if !lv.starts_with(&lc(&marker)) {
            return false;
        }
        let rest = &v[marker.len()..];
        if rest.is_empty() || rest.chars().any(char::is_whitespace) {
            return false;
        }
        let sep = self.cfg.opt("xdatasep");
        let parts: Vec<&str> = rest.split(sep.as_str()).collect();
        let ok = (parts.len() == 2 || parts.len() == 3)
            && parts.iter().all(|p| !p.is_empty())
            && (parts.len() == 2 || parts[2].bytes().all(|b| b.is_ascii_digit()));
        if !ok {
            let key = be.key();
            let bee = be.etype();
            let w = alloc::format!(
                "{bee} entry '{key}' has XDATA reference from field '{reffield}' that contains no source field (section {})",
                self.cur
            );
            be.add_warning(w.clone());
            self.warnings.push(w);
            return false;
        }
        be.xrefs.get_or_insert_with(Vec::new).push(XRef {
            reffield: reffield.into(),
            refposition: pos.map_or(1, |p| p + 1),
            entries: alloc::vec![parts[0].into()],
            field: Some(parts[1].into()),
            position: parts.get(2).map_or_else(|| "*".into(), |s| (*s).into()),
            resolved: None,
        });
        true
    }

    fn handle_field(&mut self, be: &mut Entry, e: &BtEntry, f: &str, f_c: &str, key: &str, value: &str) -> Result<Handled, Fatal> {
        if f_c.contains("+an") && self.dm.is_field(f_c) && !self.dm.fields.contains_key(f_c) {
            return Err(self.unsupported("data annotations (+an fields)"));
        }
        let ft = self.dm.fieldtype(f_c);
        let fmt = self.dm.format(f_c);
        let dt = self.dm.datatype(f_c);
        let kind = match (ft.as_str(), fmt.as_str(), dt.as_str()) {
            ("field", "xsv", "entrykey" | "literal" | "keyword" | "option") => "xsv",
            ("field", _, "code" | "datepart" | "entrykey" | "integer" | "key" | "literal") if fmt != "xsv" => "literal",
            ("field", _, "date") if fmt != "xsv" => "date",
            ("field", _, "range") if fmt != "xsv" => "range",
            ("field", _, "verbatim" | "uri") if fmt != "xsv" => "verbatim",
            ("list", _, "key" | "literal" | "verbatim") if fmt != "xsv" => "list",
            ("list", _, "name") if fmt != "xsv" => "name",
            ("list", _, "uri") if fmt != "xsv" => "urilist",
            _ => return Err(self.unsupported(&alloc::format!("field '{f_c}' of type {ft}/{fmt}/{dt}"))),
        };
        let _ = f;
        Ok(match kind {
            "literal" => {
                if self.add_xdata_ref(be, f_c, XdVal::S(value), None) {
                    return Ok(Handled::Val(Val::s(value)));
                }
                if f_c == "year" {
                    if be.data.get("year").is_some_and(Val::truthy) {
                        return Ok(Handled::None);
                    }
                    if truthy(value) && unicode::num(value).is_none() {
                        self.warnings.push(alloc::format!(
                            "legacy year field '{value}' in entry '{key}' is not an integer - this will probably not sort properly."
                        ));
                    }
                }
                if f_c == "month" {
                    if be.data.get("month").is_some_and(Val::truthy) {
                        return Ok(Handled::None);
                    }
                    if truthy(value) && unicode::num(value).is_none() {
                        self.warnings.push(alloc::format!(
                            "legacy month field '{value}' in entry '{key}' is not an integer - this will probably not sort properly."
                        ));
                    }
                    return Ok(Handled::Val(Val::s(hack_month(value))));
                }
                if f_c == "langid" {
                    if let Some(m) = crate::locales::locale_map_r(value) {
                        return Ok(Handled::Val(Val::s(m)));
                    }
                }
                Handled::Val(Val::s(value))
            }
            "verbatim" => {
                self.add_xdata_ref(be, f_c, XdVal::S(value), None);
                Handled::Val(Val::s(value))
            }
            "xsv" => {
                let xsv = Regex::new(&self.cfg.opt("xsvsep"), "").map_err(|x| self.fatal(x))?;
                let v = re_split(&xsv, value);
                self.add_xdata_ref(be, f_c, XdVal::L(&v), None);
                Handled::Val(Val::list(v))
            }
            "range" => {
                if self.add_xdata_ref(be, f_c, XdVal::S(value), None) {
                    return Ok(Handled::Val(Val::s(value)));
                }
                Handled::Val(Val::R(shared(self.parse_range_field(be, f_c, key, value))))
            }
            "list" | "urilist" => {
                let items = btparse::split_list(&nfc(value), &self.cfg.opt("listsep"));
                let mut out = Vec::new();
                for (i, it) in items.into_iter().enumerate() {
                    let it = nfd(&it.unwrap_or_default());
                    let it = if kind == "list" { remove_outer(&it).1 } else { it };
                    self.add_xdata_ref(be, f_c, XdVal::S(&it), Some(i));
                    out.push(it);
                }
                Handled::Val(Val::list(out))
            }
            "name" => self.handle_name(be, f_c, key, value)?,
            "date" => {
                self.handle_date(be, e, f_c, key, value)?;
                Handled::None
            }
            _ => Handled::None,
        })
    }

    fn parse_range_field(&mut self, be: &mut Entry, field: &str, key: &str, value: &str) -> Vec<(String, Option<String>)> {
        let mut out = Vec::new();
        let sep = Regex::new(r"\s*[;,]\s*", "").expect("static");
        for v0 in re_split(&sep, value) {
            let v = v0.replace('~', " ");
            let r1 = Regex::new(r"\A\s*(\P{Pd}+)\s*\z", "s").expect("static");
            let r2 = Regex::new(r"\A\s*(\{[^\}]+\}|[^\p{Pd} ]+)\s*(\p{Pd}+)\s*(\{[^\}]+\}|\P{Pd}*)\s*\z", "s").expect("static");
            let r3 = Regex::new(r"\A\s*(.+)(\p{Pd}{2,})(.+)\s*\z", "s").expect("static");
            let r4 = Regex::new(r"\A\s*(.+)(\p{Pd}+)(.+)\s*\z", "s").expect("static");
            let t: Vec<char> = v.chars().collect();
            let mut found = None;
            for re in [&r1, &r2, &r3, &r4] {
                if let Some(m) = re.find_at(&t, 0).filter(|m| m.start == 0) {
                    found = Some(m);
                    break;
                }
            }
            let (start, end) = match &found {
                Some(m) => {
                    let s = regex::cap(&t, m, 1).unwrap_or_default();
                    let dash = regex::cap(&t, m, 2).filter(|x| truthy(x));
                    let e = if dash.is_some() { regex::cap(&t, m, 3) } else { None };
                    (s, e)
                }
                None => (String::new(), None),
            };
            let unbrace = |s: &str| -> String {
                if s.len() >= 2 && s.starts_with('{') && s.ends_with('}') && !s[1..s.len() - 1].contains('}') {
                    s[1..s.len() - 1].into()
                } else {
                    s.into()
                }
            };
            let start = unbrace(&start);
            let end = end.map(|e| if truthy(&e) { unbrace(&e) } else { e });
            if truthy(&start) {
                out.push((start, end));
            } else {
                let w = alloc::format!("Range field '{field}' in entry '{key}' is malformed, falling back to literal");
                be.add_warning(w.clone());
                self.warnings.push(w);
                out.push((v0.clone(), None));
            }
        }
        out
    }

    fn handle_name(&mut self, be: &mut Entry, f_c: &str, key: &str, value: &str) -> Result<Handled, Fatal> {
        let mut names = Names { typ: f_c.into(), id: self.new_id(), ..Names::default() };
        let items = btparse::split_list(&nfc(value), &self.cfg.opt("namesep"));
        let xns = self.cfg.opt("xnamesep");
        let nps = self.dm.nameparts();
        for (i, it) in items.into_iter().enumerate() {
            let name = nfd(&it.clone().unwrap_or_default());
            if it.is_some() && self.add_xdata_ref(be, f_c, XdVal::S(&name), Some(i)) {
                let n = Name { id: self.new_id(), xdata: Some(name.clone()), ..Name::default() };
                names.add(shared(n));
                continue;
            }
            // namelist option: ^(\S+)\s*=\s*(\S+)?$
            if let Some(eq) = name.find(xns.as_str()) {
                let o = name[..eq].trim_end();
                let v = name[eq + xns.len()..].trim_start();
                if !o.is_empty() && !o.chars().any(char::is_whitespace) && !v.chars().any(char::is_whitespace) {
                    let nlo = lc(o);
                    if self.cfg.scope_opts.get("NAMELIST").is_some_and(|s| s.contains(&nlo)) {
                        let val = if v.is_empty() { "1" } else { v };
                        for (k, vv) in self.expand_option_input("NAMELIST", &nlo, val) {
                            names.set_opt(&k, vv);
                        }
                        continue;
                    }
                }
            }
            if !truthy(&name) {
                let w = alloc::format!("Name in key '{key}' is empty (probably consecutive 'and'): skipping entry '{key}'");
                be.add_warning(w.clone());
                self.warnings.push(w);
                self.sec().del_citekey(key);
                return Ok(Handled::Skip);
            }
            let is_x = nps.iter().any(|np| {
                let mut s = 0;
                while let Some(p) = name[s..].find(np.as_str()) {
                    let after = &name[s + p + np.len()..];
                    if after.trim_start().starts_with(xns.as_str()) {
                        return true;
                    }
                    s += p + 1;
                }
                false
            });
            let no = if is_x {
                self.parsename_x(&name, f_c, key)
            } else {
                let fully_braced = name.starts_with('{') && name.ends_with('}') && name.len() >= 3;
                if !fully_braced {
                    let commas = name.matches(',').count();
                    if commas > 2 {
                        self.errors.push(alloc::format!("Name \"{name}\" has too many commas, skipping entry '{key}'"));
                        self.sec().del_citekey(key);
                        return Ok(Handled::Skip);
                    }
                    if name.contains(",,") {
                        self.errors.push(alloc::format!("Name \"{name}\" is malformed (consecutive commas): skipping entry '{key}'"));
                        self.sec().del_citekey(key);
                        return Ok(Handled::Skip);
                    }
                }
                self.parsename(&name)
            };
            let Some(no) = no else { continue };
            let raw: String = no.nameparts.values().filter_map(|p| p.string.clone()).collect();
            if lc(&raw) == self.cfg.opt("others_string") {
                names.morenames = true;
            } else {
                names.add(shared(no));
            }
        }
        Ok(if names.count() > 0 { Handled::Val(Val::N(shared(names))) } else { Handled::None })
    }

    pub fn new_id(&mut self) -> String {
        self.id_ctr += 1;
        alloc::format!("N{}X", self.id_ctr)
    }

    /// Utils `strip_noinit`.
    pub fn strip_noinit(&self, s: &str) -> String {
        if !truthy(s) {
            return String::new();
        }
        let mut s = s.to_string();
        for re in &self.cfg.noinit {
            s = re.delete_all(&s);
        }
        let mac = Regex::new(r"\\[A-Za-z]+\s*(\{([^\}]*)?\})?", "").expect("static");
        s = mac.replace(&s, true, &mut |t, m| regex::cap(t, m, 2).unwrap_or_default());
        if s == "{}" {
            s.clear();
        }
        s
    }

    fn parsename(&mut self, namestr: &str) -> Option<Name> {
        let mut s = namestr.trim().to_string();
        // s/\s+|\\\s/ /g
        let ws = Regex::new(r"\s+|\\\s", "").expect("static");
        s = ws.replace(&s, true, &mut |_, _| " ".into());
        // ^\{\{+([^\{\}]+)\}+\}$ -> {$1}
        let env = Regex::new(r"\A\{\{+([^\{\}]+)\}+\}\z", "s").expect("static");
        s = env.replace(&s, false, &mut |t, m| alloc::format!("{{{}}}", regex::cap(t, m, 1).unwrap_or_default()));
        let sn = btparse::split_name(&nfc(&s));
        let mut namec: BTreeMap<&str, String> = BTreeMap::new();
        namec.insert("family", nfd(&btparse::format_part(&sn.last)));
        namec.insert("given", nfd(&btparse::format_part(&sn.first)));
        namec.insert("prefix", nfd(&btparse::format_part(&sn.von)));
        namec.insert("suffix", nfd(&btparse::format_part(&sn.jr)));
        let mut name = Name { id: self.new_id(), ..Name::default() };
        for np in ["prefix", "family", "given", "suffix"] {
            let v = namec[np].clone();
            if truthy(&v) {
                let (flag, stripped) = remove_outer(&v);
                let mut part = v.clone();
                if flag {
                    let w = Regex::new(r"\s+", "").expect("static");
                    part = w.replace(&v, true, &mut |_, _| "_".into());
                }
                let part = self.strip_noinit(&part);
                let sp = Regex::new(r"[\h~]+(?![^{]*\})", "").expect("static");
                let toks = re_split(&sp, &part);
                let inits = gen_initials(&toks);
                self.sec().set_np_length(np, stripped.chars().count());
                self.sec().set_np_length(&alloc::format!("{np}-i"), inits.concat().chars().count());
                name.nameparts.insert(np.into(), NamePart { string: Some(stripped), initial: Some(inits) });
                name.strip.insert(np.into(), flag);
            } else {
                name.nameparts.insert(np.into(), NamePart { string: None, initial: None });
                name.strip.insert(np.into(), false);
            }
        }
        Some(name)
    }

    fn parsename_x(&mut self, namestr: &str, fieldname: &str, key: &str) -> Option<Name> {
        let xns = self.cfg.opt("xnamesep");
        let nps = self.dm.nameparts();
        let mut namec: BTreeMap<String, String> = BTreeMap::new();
        let mut inits: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut hashid = None;
        let mut opts: Vec<(String, String)> = Vec::new();
        for np in split_xsv(namestr) {
            let Some(i) = np.find(xns.as_str()) else { continue };
            let npn = lc(np[..i].trim());
            let npv = np[i + xns.len()..].trim().to_string();
            if self.cfg.scope_opts.get("NAME").is_some_and(|s| s.contains(&npn)) {
                for (k, v) in self.expand_option_input("NAME", &npn, &npv) {
                    opts.push((k, v));
                }
                continue;
            }
            let base = npn.strip_suffix("-i").unwrap_or(&npn).to_string();
            if !(nps.contains(&base) || npn == "id") {
                self.warnings.push(alloc::format!(
                    "Invalid namepart option '{npn}' found in extended name format name '{fieldname}' in entry '{key}', ignoring"
                ));
                continue;
            }
            if npn.ends_with("-i") {
                inits.insert(base, split_initials(&npv));
            } else if npn == "id" {
                hashid = Some(npv);
            } else if has_outer(&npv) {
                namec.insert(npn, npv);
            } else {
                let parts: Vec<String> = npv.split_whitespace().map(Into::into).collect();
                namec.insert(npn, join_name_parts(&parts));
            }
        }
        let mut name = Name { id: self.new_id(), hashid, opts, ..Name::default() };
        for np in &nps {
            if let Some(v) = namec.get(np).cloned() {
                let (s, stripped) = remove_outer(&v);
                let mut part = stripped.clone();
                if s {
                    let w = Regex::new(r"\s+", "").expect("static");
                    part = w.replace(&stripped, true, &mut |_, _| "_".into());
                }
                let part = self.strip_noinit(&part);
                let ini = inits.get(np).cloned().unwrap_or_else(|| {
                    let sp = Regex::new(r"[\s~]+", "").expect("static");
                    gen_initials(&re_split(&sp, &part))
                });
                if truthy(&stripped) {
                    self.sec().set_np_length(np, stripped.chars().count());
                }
                self.sec().set_np_length(&alloc::format!("{np}-i"), ini.concat().chars().count());
                name.nameparts.insert(np.clone(), NamePart { string: Some(stripped), initial: Some(ini) });
                name.strip.insert(np.clone(), false);
            } else {
                name.nameparts.insert(np.clone(), NamePart { string: None, initial: None });
            }
        }
        Some(name)
    }

    fn handle_date(&mut self, be: &mut Entry, e: &BtEntry, field: &str, key: &str, date: &str) -> Result<(), Fatal> {
        let dt = field.strip_suffix("date").unwrap_or(field).to_string();
        let ds = self.sec().keytods.get(key).cloned().unwrap_or_default();
        let bee = be.etype();
        let r = crate::date::parse_date_range(date);
        if r.dateunknown {
            be.set_field(&alloc::format!("{dt}dateunknown"), Val::s("1"));
        }
        if r.enddateunknown {
            be.set_field(&alloc::format!("{dt}enddateunknown"), Val::s("1"));
        }
        if r.nothing {
            let w = alloc::format!("{bee} entry '{key}' ({ds}): Invalid format '{date}' of date field '{field}' - ignoring");
            be.add_warning(w.clone());
            self.warnings.push(w);
            return Ok(());
        }
        if let Some(u) = r.unspec {
            be.set_field(&alloc::format!("{dt}dateunspecified"), Val::s(u));
        }
        let julian = self.cfg.blx_b(None, "julian", None, None);
        match r.start {
            Some(sd) => {
                if let Some(sd) = sd {
                    if julian && !sd.missing_month && !sd.missing_day {
                        return Err(self.unsupported("julian dates (option julian)"));
                    }
                    be.set_field(&alloc::format!("{dt}datesplit"), Val::s("1"));
                    if dt.is_empty() {
                        if let Some(y) = e.get("year").filter(|s| truthy(s)) {
                            if sd.year != 0 && crate::model::numify(y) != sd.year as f64 {
                                let w = alloc::format!("Overwriting field 'year' with year value from field 'date' for entry '{key}'");
                                be.add_warning(w.clone());
                                self.warnings.push(w);
                            }
                        }
                        if !sd.missing_month {
                            if let Some(m) = e.get("month").filter(|s| truthy(s)) {
                                if crate::model::numify(m) != f64::from(sd.month) {
                                    let w = alloc::format!("Overwriting field 'month' with month value from field 'date' for entry '{key}'");
                                    be.add_warning(w.clone());
                                    self.warnings.push(w);
                                }
                            }
                        }
                    }
                    if sd.approximate {
                        be.set_field(&alloc::format!("{dt}dateapproximate"), Val::s("1"));
                    }
                    if let Some(Some(ed)) = &r.end {
                        if ed.approximate {
                            be.set_field(&alloc::format!("{dt}enddateapproximate"), Val::s("1"));
                        }
                    }
                    if sd.uncertain {
                        be.set_field(&alloc::format!("{dt}dateuncertain"), Val::s("1"));
                    }
                    if let Some(Some(ed)) = &r.end {
                        if ed.uncertain {
                            be.set_field(&alloc::format!("{dt}enddateuncertain"), Val::s("1"));
                        }
                    }
                    if let Some(yd) = sd.yeardivision {
                        be.set_field(&alloc::format!("{dt}yeardivision"), Val::s(yd));
                    }
                    be.set_data(&alloc::format!("{dt}year"), Val::s(sd.resolve(sd.year)));
                    be.set_field(&alloc::format!("{dt}era"), Val::s(sd.era()));
                    if !sd.missing_month {
                        be.set_data(&alloc::format!("{dt}month"), Val::s(sd.resolve(i64::from(sd.month))));
                    }
                    if !sd.missing_day {
                        be.set_data(&alloc::format!("{dt}day"), Val::s(sd.resolve(i64::from(sd.day))));
                    }
                    if !sd.missing_time {
                        be.set_data(&alloc::format!("{dt}hour"), Val::s(sd.resolve(i64::from(sd.hour))));
                        be.set_data(&alloc::format!("{dt}minute"), Val::s(sd.resolve(i64::from(sd.minute))));
                        be.set_data(&alloc::format!("{dt}second"), Val::s(sd.resolve(i64::from(sd.second))));
                        if let Some(tz) = &sd.tz {
                            be.set_data(&alloc::format!("{dt}timezone"), Val::s(crate::date::tzformat(tz)));
                        }
                    }
                } else {
                    be.set_data(&alloc::format!("{dt}year"), Val::s(""));
                }
                if r.sep {
                    match r.end {
                        Some(Some(ed)) => {
                            if julian && !ed.missing_month && !ed.missing_day {
                                return Err(self.unsupported("julian dates (option julian)"));
                            }
                            be.set_field(&alloc::format!("{dt}datesplit"), Val::s("1"));
                            be.set_data(&alloc::format!("{dt}endyear"), Val::s(ed.resolve(ed.year)));
                            be.set_field(&alloc::format!("{dt}endera"), Val::s(ed.era()));
                            if !ed.missing_month {
                                be.set_data(&alloc::format!("{dt}endmonth"), Val::s(ed.resolve(i64::from(ed.month))));
                            }
                            if !ed.missing_day {
                                be.set_data(&alloc::format!("{dt}endday"), Val::s(ed.resolve(i64::from(ed.day))));
                            }
                            if let Some(yd) = ed.yeardivision {
                                be.set_field(&alloc::format!("{dt}endyeardivision"), Val::s(yd));
                                be.set_field(&alloc::format!("{dt}endseaason"), Val::s(yd));
                            }
                            if !ed.missing_time {
                                be.set_data(&alloc::format!("{dt}endhour"), Val::s(ed.resolve(i64::from(ed.hour))));
                                be.set_data(&alloc::format!("{dt}endminute"), Val::s(ed.resolve(i64::from(ed.minute))));
                                be.set_data(&alloc::format!("{dt}endsecond"), Val::s(ed.resolve(i64::from(ed.second))));
                                if let Some(tz) = &ed.tz {
                                    be.set_data(&alloc::format!("{dt}endtimezone"), Val::s(crate::date::tzformat(tz)));
                                }
                            }
                        }
                        Some(None) => {
                            be.set_data(&alloc::format!("{dt}endyear"), Val::s(""));
                        }
                        None => {
                            let w = alloc::format!("{bee} entry '{key}' ({ds}): Invalid format '{date}' of end date field '{field}' - ignoring");
                            be.add_warning(w.clone());
                            self.warnings.push(w);
                        }
                    }
                }
            }
            None => {
                let w = alloc::format!("{bee} entry '{key}' ({ds}): Invalid format '{date}' of date field '{field}' - ignoring");
                be.add_warning(w.clone());
                self.warnings.push(w);
            }
        }
        Ok(())
    }
}

pub enum Handled {
    Val(Val),
    None,
    Skip,
}

pub enum XdVal<'a> {
    S(&'a str),
    L(&'a [String]),
}

/// Perl string interpolation of `$1`..`$9` in a replacement.
fn interpolate(r: &str, t: &[char], m: &regex::Match) -> String {
    let c: Vec<char> = r.chars().collect();
    let mut o = String::new();
    let mut i = 0;
    while i < c.len() {
        if c[i] == '\\' && i + 1 < c.len() {
            o.push(c[i + 1]);
            i += 2;
            continue;
        }
        if c[i] == '$' && i + 1 < c.len() && c[i + 1].is_ascii_digit() {
            let mut j = i + 1;
            let mut n = 0usize;
            while j < c.len() && c[j].is_ascii_digit() {
                n = n * 10 + (c[j] as usize - '0' as usize);
                j += 1;
            }
            o.push_str(&regex::cap(t, m, n).unwrap_or_default());
            i = j;
            continue;
        }
        o.push(c[i]);
        i += 1;
    }
    o
}

/// Text::CSV split (allow_whitespace, double quotes).
fn split_xsv(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut inq = false;
    let c: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < c.len() {
        let ch = c[i];
        if inq {
            if ch == '"' {
                if c.get(i + 1) == Some(&'"') {
                    cur.push('"');
                    i += 2;
                    continue;
                }
                inq = false;
            } else {
                cur.push(ch);
            }
        } else if ch == '"' {
            inq = true;
        } else if ch == ',' {
            out.push(cur.trim().to_string());
            cur.clear();
        } else {
            cur.push(ch);
        }
        i += 1;
    }
    out.push(cur.trim().to_string());
    out
}
