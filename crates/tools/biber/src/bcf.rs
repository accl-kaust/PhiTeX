//! Biber `parse_ctrlfile` and `process_setup`: the `.bcf` control file
//! into options, templates, the data model, sections and data lists.

use alloc::collections::BTreeSet;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::biber::{Biber, Fatal};
use crate::config::{
    split_comma, BackendIn, DataModel, FieldRe, LantPart, Snk, SnkPart, SortAttrs, SortSet, SortTemplate, UntPart, BV,
};
use crate::model::truthy;
use crate::regex::Regex;
use crate::section::{DataList, DataSource, Filter, Section};
use crate::unicode::{lc, nfd};
use crate::xml::Element;

fn sorted_by_order(v: Vec<&Element>) -> Vec<&Element> {
    let mut v = v;
    v.sort_by_key(|e| e.order());
    v
}

fn attr_t(e: &Element, a: &str) -> bool {
    e.attr(a).is_some_and(truthy)
}

/// `locale2bcp47`.
pub fn locale2bcp47(s: &str) -> String {
    if s.is_empty() {
        return s.into();
    }
    crate::locales::locale_map(s).map_or_else(|| s.into(), Into::into)
}

impl Biber<'_> {
    pub fn parse_ctrlfile(&mut self, bcf: &str) -> Result<(), Fatal> {
        // NFD every line except datasource names
        let buf: Vec<String> = bcf
            .split('\n')
            .map(|l| {
                let is_ds = l.find("<bcf:datasource").is_some_and(|i| {
                    let r = &l[i..];
                    r.find('>').is_some_and(|j| r.len() > j + 1 && !r[j + 1..].starts_with('<'))
                });
                if is_ds {
                    l.into()
                } else {
                    nfd(l)
                }
            })
            .collect();
        let buf = buf.join("\n");
        let root = crate::xml::parse(&buf).map_err(|e| self.fatal(alloc::format!("control file is malformed, last biblatex run probably failed ({e})")))?;
        let version = root.attr("version").unwrap_or("");
        if version != "3.11" {
            return Err(self.fatal(alloc::format!(
                "Error: Found biblatex control file version {version}, expected version 3.11.\nThis means that your biber (2.22) and biblatex ({}) versions are incompatible.\nSee compat matrix in biblatex or biber PDF documentation.",
                root.attr("bltxversion").unwrap_or("")
            )));
        }
        let cfg = &mut self.cfg;
        cfg.optscope.entry("controlversion".into()).or_default().insert("GLOBAL".into());
        // option scopes
        for os in root.kids("optionscope") {
            let scope = os.attr("type").unwrap_or("").to_string();
            for o in os.kids("option") {
                let opt = o.text.clone();
                cfg.optout.insert((scope.clone(), opt.clone()), o.attr("backendout").is_some_and(truthy));
                if let Some(bin) = o.attr("backendin").filter(|s| truthy(s)) {
                    let opts = split_comma(bin);
                    let v = if opts.iter().any(|x| x.contains('=')) {
                        BackendIn::Map(
                            opts.iter()
                                .filter_map(|x| {
                                    let t = x.trim();
                                    let i = t.find('=')?;
                                    Some((t[..i].into(), t[i + 1..].into()))
                                })
                                .collect(),
                        )
                    } else {
                        BackendIn::List(opts)
                    };
                    cfg.optin.insert((scope.clone(), opt.clone()), v);
                }
                cfg.optscope.entry(opt.clone()).or_default().insert(scope.clone());
                let so = cfg.scope_opts.entry(scope.clone()).or_default();
                if !so.contains(&opt) {
                    so.push(opt.clone());
                }
                let dt = lc(o.attr("datatype").unwrap_or(""));
                if let Some(old) = cfg.opttype.get(&opt) {
                    if lc(old) != dt {
                        self.warnings.push(alloc::format!(
                            "Warning: Datatype for biblatex option '{opt}' has conflicting values, probably at different scopes. This is not supported."
                        ));
                    }
                } else {
                    cfg.opttype.insert(opt, dt);
                }
            }
        }
        let cfg = &mut self.cfg;
        cfg.set_blx("", "controlversion", BV::S(version.into()), None);
        // options
        for os in root.kids("options") {
            let comp = os.attr("component").unwrap_or("");
            let typ = os.attr("type").unwrap_or("");
            for o in os.kids("option") {
                let key = o.kid("key").map(|k| k.text.clone()).unwrap_or_default();
                let single = o.attr("type") == Some("singlevalued");
                let multi = o.attr("type") == Some("multivalued");
                let vals: Vec<&Element> = o.kids("value").collect();
                if comp == "biber" && typ == "global" {
                    if single {
                        cfg.opts.insert(key, vals.first().map(|v| v.text.clone()).unwrap_or_default());
                    } else if multi {
                        let v: Vec<String> = sorted_by_order(vals).iter().map(|v| v.text.clone()).collect();
                        cfg.opts.insert(key, v.join(","));
                    }
                } else if comp == "biblatex" {
                    let val = if single {
                        BV::S(vals.first().map(|v| v.text.clone()).unwrap_or_default())
                    } else if multi {
                        BV::M(Rc::new(sorted_by_order(vals).into_iter().cloned().collect()))
                    } else {
                        continue;
                    };
                    if typ == "global" {
                        cfg.set_blx("", &key, val, None);
                    } else {
                        cfg.set_blx("", &key, val, Some(("ENTRYTYPE", typ)));
                    }
                }
            }
        }
        // datafield sets
        let mut dfs_raw: Vec<(String, Vec<(Option<String>, Option<String>, Option<String>)>)> = Vec::new();
        for s in root.kids("datafieldset") {
            let name = lc(s.attr("name").unwrap_or(""));
            let mut mem = Vec::new();
            for m in s.kids("member") {
                mem.push((
                    m.attr("field").map(Into::into),
                    m.attr("fieldtype").map(Into::into),
                    m.attr("datatype").map(Into::into),
                ));
            }
            dfs_raw.push((name, mem));
        }
        // source maps
        if let Some(sm) = root.kid("sourcemap") {
            cfg.sourcemap = sm.kids("maps").cloned().collect();
        }
        // label alpha name templates
        for t in root.kids("labelalphanametemplate") {
            let mut v = Vec::new();
            for np in sorted_by_order(t.kids("namepart").collect()) {
                v.push(LantPart {
                    namepart: np.text.clone(),
                    use_: attr_t(np, "use"),
                    pre: attr_t(np, "pre"),
                    substring_compound: attr_t(np, "substring_compound"),
                    substring_side: np.attr("substring_side").map(Into::into),
                    substring_width: np.attr("substring_width").map(Into::into),
                });
            }
            cfg.labelalphanametemplate.insert(t.attr("name").unwrap_or("").into(), v);
        }
        for t in root.kids("labelalphatemplate") {
            let ty = t.attr("type").unwrap_or("global");
            if ty == "global" {
                if cfg.in_scope("labelalphatemplate", "GLOBAL") {
                    cfg.labelalphatemplate.insert(String::new(), t.clone());
                }
            } else if cfg.in_scope("labelalphatemplate", "ENTRYTYPE") {
                cfg.labelalphatemplate.insert(ty.into(), t.clone());
            }
        }
        if let Some(ed) = root.kid("extradatespec") {
            for sc in ed.kids("scope") {
                cfg.extradatespec.push(sorted_by_order(sc.kids("field").collect()).iter().map(|f| f.text.clone()).collect());
            }
        }
        if let Some(inh) = root.kid("inheritance") {
            cfg.inheritance = inh.clone();
        }
        let rx = |s: &str| Regex::new(s, "");
        if let Some(n) = root.kid("noinits") {
            let mut v = Vec::new();
            for x in n.kids("noinit") {
                let p = x.kid("value").map(|v| v.text.clone()).unwrap_or_default();
                v.push(rx(&p).map_err(|e| Fatal(alloc::format!("unsupported noinit pattern: {e}")))?);
            }
            if !v.is_empty() {
                cfg.noinit = v;
            }
        }
        if let Some(n) = root.kid("nolabels") {
            let mut v = Vec::new();
            for x in n.kids("nolabel") {
                let p = x.kid("value").map(|v| v.text.clone()).unwrap_or_default();
                v.push(rx(&p).map_err(|e| Fatal(alloc::format!("unsupported nolabel pattern: {e}")))?);
            }
            if !v.is_empty() {
                cfg.nolabel = v;
            }
        }
        if let Some(n) = root.kid("nolabelwidthcounts") {
            let mut v = Vec::new();
            for x in n.kids("nolabelwidthcount") {
                let p = x.kid("value").map(|v| v.text.clone()).unwrap_or_default();
                v.push(rx(&p).map_err(|e| Fatal(alloc::format!("unsupported nolabelwidthcount pattern: {e}")))?);
            }
            if !v.is_empty() {
                cfg.nolabelwidthcount = v;
            }
        }
        for (outer, inner, slot) in [("nosorts", "nosort", 0), ("nonamestrings", "nonamestring", 1)] {
            if let Some(n) = root.kid(outer) {
                let mut v = Vec::new();
                for x in n.kids(inner) {
                    let f = x.kid("field").map(|v| v.text.clone()).unwrap_or_default();
                    let p = x.kid("value").map(|v| v.text.clone()).unwrap_or_default();
                    v.push(FieldRe { name: f, re: rx(&p).map_err(|e| Fatal(alloc::format!("unsupported {inner} pattern: {e}")))? });
                }
                if !v.is_empty() {
                    if slot == 0 {
                        cfg.nosort = v;
                    } else {
                        cfg.nonamestring = v;
                    }
                }
            }
        }
        let mut checkbase = false;
        for unt in root.kids("uniquenametemplate") {
            let mut v = Vec::new();
            for np in sorted_by_order(unt.kids("namepart").collect()) {
                if attr_t(np, "base") {
                    checkbase = true;
                }
                v.push(UntPart {
                    namepart: np.text.clone(),
                    use_: attr_t(np, "use"),
                    disambiguation: np.attr("disambiguation").map(Into::into),
                    base: attr_t(np, "base"),
                });
            }
            cfg.uniquenametemplate.insert(unt.attr("name").unwrap_or("").into(), v);
        }
        if !checkbase {
            return Err(self.fatal("The uniquenametemplate must contain at least one 'base' part otherwise name disambiguation is impossible".into()));
        }
        let cfg = &mut self.cfg;
        for nht in root.kids("namehashtemplate") {
            let v = sorted_by_order(nht.kids("namepart").collect())
                .iter()
                .map(|np| (np.text.clone(), np.attr("hashscope").unwrap_or("").to_string()))
                .collect();
            cfg.namehashtemplate.insert(nht.attr("name").unwrap_or("").into(), v);
        }
        for sns in root.kids("sortingnamekeytemplate") {
            let mut tmpl = Vec::new();
            for kp in sorted_by_order(sns.kids("keypart").collect()) {
                let mut parts = Vec::new();
                for p in sorted_by_order(kp.kids("part").collect()) {
                    match p.attr("type") {
                        Some("namepart") => parts.push(SnkPart::NamePart {
                            value: p.text.clone(),
                            use_: p.attr("use").map(Into::into),
                            inits: attr_t(p, "inits"),
                        }),
                        Some("literal") => parts.push(SnkPart::Literal(p.text.clone())),
                        _ => {}
                    }
                }
                tmpl.push(parts);
            }
            cfg.sortingnamekeytemplate.insert(
                sns.attr("name").unwrap_or("").into(),
                Snk { visibility: sns.attr("visibility").unwrap_or("").into(), template: tmpl },
            );
        }
        if root.kids("transliteration").next().is_some() {
            return Err(self.unsupported("transliteration (\\DeclareSortTransliteration)"));
        }
        let cfg = &mut self.cfg;
        for sex in root.kids("sortexclusion") {
            let set: BTreeSet<String> = sex.kids("exclusion").map(|e| e.text.clone()).collect();
            if cfg.in_scope("sortexclusion", "ENTRYTYPE") {
                cfg.sortexclusion.insert(sex.attr("type").unwrap_or("").into(), set);
            }
        }
        for sin in root.kids("sortinclusion") {
            let set: BTreeSet<String> = sin.kids("inclusion").map(|e| e.text.clone()).collect();
            if cfg.in_scope("sortinclusion", "ENTRYTYPE") {
                cfg.sortinclusion.insert(sin.attr("type").unwrap_or("").into(), set);
            }
        }
        for p in root.kids("presort") {
            match p.attr("type") {
                None => cfg.set_blx("", "presort", BV::S(p.text.clone()), None),
                Some(t) => cfg.set_blx("", "presort", BV::S(p.text.clone()), Some(("ENTRYTYPE", t))),
            }
        }
        let global_sortlocale = cfg.blx_s(None, "sortlocale", None, None);
        for st in root.kids("sortingtemplate") {
            let mut spec = Vec::new();
            for s in sorted_by_order(st.kids("sort").collect()) {
                let mut items = Vec::new();
                for it in sorted_by_order(s.kids("sortitem").collect()) {
                    items.push((
                        it.text.clone(),
                        SortAttrs {
                            substring_side: it.attr("substring_side").map(Into::into),
                            substring_width: it.attr("substring_width").map(Into::into),
                            pad_width: it.attr("pad_width").map(Into::into),
                            pad_char: it.attr("pad_char").map(Into::into),
                            pad_side: it.attr("pad_side").map(Into::into),
                            literal: attr_t(it, "literal"),
                        },
                    ));
                }
                if items.is_empty() {
                    continue;
                }
                spec.push(SortSet {
                    final_: attr_t(s, "final"),
                    descending: s.attr("sort_direction") == Some("descending"),
                    sortcase: s.attr("sortcase").map(Into::into),
                    sortupper: s.attr("sortupper").map(Into::into),
                    locale: s.attr("locale").map(Into::into),
                    items,
                });
            }
            let loc = st.attr("locale").filter(|s| truthy(s)).map_or_else(|| global_sortlocale.clone(), Into::into);
            cfg.sortingtemplate.insert(st.attr("name").unwrap_or("").into(), SortTemplate { locale: locale2bcp47(&loc), spec });
        }
        let dm = root.kid("datamodel").cloned().unwrap_or_default();
        self.dm = DataModel::new(&dm);
        // datafield sets resolved against the data model
        for (name, mem) in dfs_raw {
            let mut fs: Vec<String> = Vec::new();
            for (f, ft, dt) in mem {
                if let Some(f) = f {
                    fs.push(f);
                } else {
                    match (ft, dt) {
                        (Some(ft), Some(dt)) => fs.extend(self.dm.fields_of_type(&ft, &[dt.as_str()])),
                        (Some(ft), None) => {
                            let mut v = self.dm.fields_of_fieldtype(&ft);
                            v.sort();
                            fs.extend(v);
                        }
                        (None, Some(dt)) => {
                            let mut v = self.dm.fields_of_datatype(&dt);
                            v.sort();
                            fs.extend(v);
                        }
                        _ => {}
                    }
                }
            }
            self.cfg.datafield_sets.insert(name, fs);
        }
        // data sources
        let mut bibds: Vec<(String, Vec<DataSource>)> = Vec::new();
        for bd in root.kids("bibdata") {
            let sec = bd.attr("section").unwrap_or("").to_string();
            for ds in bd.kids("datasource") {
                let d = DataSource {
                    typ: ds.attr("type").unwrap_or("").into(),
                    name: ds.text.clone(),
                    datatype: ds.attr("datatype").unwrap_or("").into(),
                    encoding: ds.attr("encoding").map_or_else(|| self.cfg.opt("input_encoding"), Into::into),
                    glob: ds.attr("glob").map_or_else(|| "0".into(), Into::into),
                };
                let e = match bibds.iter_mut().find(|(s, _)| *s == sec) {
                    Some(e) => e,
                    None => {
                        bibds.push((sec.clone(), Vec::new()));
                        bibds.last_mut().expect("pushed")
                    }
                };
                if !e.1.iter().any(|x| x.typ == d.typ && x.datatype == d.datatype && x.name == d.name) {
                    e.1.push(d);
                }
            }
        }
        if bibds.is_empty() {
            self.warnings.push("No data sources defined!".into());
            self.no_data = true;
            return Ok(());
        }
        let mut key_flag = false;
        for sec in root.kids("section") {
            let secnum = sec.attr("number").unwrap_or("").to_string();
            let mut s = self.sections.remove(&secnum).unwrap_or_else(|| Section::new(&secnum));
            if s.datasources.is_none() {
                s.datasources = bibds.iter().find(|(x, _)| *x == secnum).map(|(_, d)| d.clone());
            }
            // \cite beats \nocite
            let mut prekeys: Vec<(String, bool, &Element)> = Vec::new();
            for kc in sec.kids("citekey") {
                let key = nfd(&kc.text);
                let nocite = kc.attr("nocite").is_some_and(truthy);
                if nocite {
                    if !prekeys.iter().any(|(k, _, _)| *k == key) {
                        prekeys.push((key, true, kc));
                    }
                } else if prekeys.iter().any(|(k, n, _)| *k == key && *n) {
                    for p in &mut prekeys {
                        if p.0 == key {
                            p.1 = false;
                        }
                    }
                } else {
                    prekeys.push((key, false, kc));
                }
            }
            let mut keys: Vec<String> = Vec::new();
            for (key, nocite, kc) in prekeys {
                if key == "*" {
                    s.allkeys = true;
                    self.cfg.keyorder.entry(secnum.clone()).or_default().insert(key.clone(), kc.attr("order").unwrap_or("").into());
                    if nocite {
                        s.allkeys_nocite = true;
                    }
                    key_flag = true;
                    *s.seenkeys.entry(key).or_insert(0) += 1;
                } else if !s.seenkeys.get(&key).is_some_and(|&c| c > 0) {
                    if kc.attr("type") == Some("set") {
                        let mem = split_comma(kc.attr("members").unwrap_or("")).into_iter().map(|m| m.trim().into()).collect();
                        s.set_dynamic_set(&key, mem);
                        keys.push(key);
                        key_flag = true;
                    } else {
                        if nocite {
                            s.nocite.insert(key.clone());
                        } else {
                            s.cite.insert(key.clone());
                        }
                        self.cfg.keyorder.entry(secnum.clone()).or_default().insert(key.clone(), kc.attr("order").unwrap_or("").into());
                        if let Some(io) = kc.attr("intorder").filter(|s| truthy(s)) {
                            self.cfg.internal_keyorder.entry(secnum.clone()).or_default().insert(key.clone(), io.into());
                        }
                        keys.push(key.clone());
                        key_flag = true;
                        *s.seenkeys.entry(key).or_insert(0) += 1;
                    }
                }
            }
            for kc in sec.kids("citekeycount") {
                s.citecount.insert(nfd(&kc.text), kc.attr("count").unwrap_or("").into());
            }
            if s.allkeys {
                s.del_citekeys();
                self.log.push(alloc::format!("Using all citekeys in bib section {secnum}"));
            } else {
                self.log.push(alloc::format!("Found {} citekeys in bib section {secnum}", keys.len()));
                s.add_citekeys(&keys);
            }
            self.sections.insert(secnum, s);
        }
        // data lists
        for l in root.kids("datalist") {
            let get = |a: &str| l.attr(a).unwrap_or("").to_string();
            let mut d = DataList {
                section: l.kid("section").map(|e| e.text.clone()).or_else(|| l.attr("section").map(Into::into)).unwrap_or_default(),
                sortingtemplatename: get("sortingtemplatename"),
                sortingnamekeytemplatename: get("sortingnamekeytemplatename"),
                uniquenametemplatename: lc(&get("uniquenametemplatename")),
                labelalphanametemplatename: lc(&get("labelalphanametemplatename")),
                namehashtemplatename: lc(&get("namehashtemplatename")),
                labelprefix: get("labelprefix"),
                ..DataList::default()
            };
            d.typ = l.attr("type").filter(|s| truthy(s)).unwrap_or("entry").into();
            d.name = match l.attr("name").filter(|s| truthy(s)) {
                Some(n) => n.into(),
                None => alloc::format!(
                    "{}/{}/{}/{}/{}/{}",
                    d.sortingtemplatename,
                    d.sortingnamekeytemplatename,
                    d.labelprefix,
                    d.uniquenametemplatename,
                    d.labelalphanametemplatename,
                    d.namehashtemplatename
                ),
            };
            for f in l.kids("filter") {
                d.filters.push(Filter::One(f.attr("type").unwrap_or("").into(), f.text.clone()));
            }
            for fo in l.kids("filteror") {
                let v: Vec<(String, String)> = fo.kids("filter").map(|f| (f.attr("type").unwrap_or("").into(), f.text.clone())).collect();
                d.filters.push(Filter::Or(v));
            }
            self.lists.push(d);
        }
        let globalss = self.cfg.blx_s(None, "sortingtemplatename", None, None);
        let secnums: Vec<String> = root.kids("section").map(|s| s.attr("number").unwrap_or("").to_string()).collect();
        for secnum in secnums {
            let name = alloc::format!("{globalss}/global//global/global/global");
            let exists = self.lists.iter().any(|l| {
                l.section == secnum
                    && l.typ == "entry"
                    && l.sortingtemplatename == globalss
                    && l.sortingnamekeytemplatename == "global"
                    && l.uniquenametemplatename == "global"
                    && l.labelalphanametemplatename == "global"
                    && l.namehashtemplatename == "global"
                    && l.labelprefix.is_empty()
                    && l.name == name
            });
            if !exists {
                self.lists.push(DataList {
                    section: secnum.clone(),
                    typ: "entry".into(),
                    sortingtemplatename: globalss.clone(),
                    sortingnamekeytemplatename: "global".into(),
                    uniquenametemplatename: "global".into(),
                    labelalphanametemplatename: "global".into(),
                    namehashtemplatename: "global".into(),
                    labelprefix: String::new(),
                    name,
                    ..DataList::default()
                });
            }
        }
        if !key_flag {
            self.warnings.push("The file does not contain any citations!".into());
        }
        // normalise_utf8
        for k in ["input_encoding", "output_encoding"] {
            let v = self.cfg.opt(k);
            let l = lc(&v);
            if l == "utf8" || l == "utf-8" {
                self.cfg.opts.insert(k.into(), "UTF-8".into());
            }
        }
        for k in ["input_encoding", "output_encoding"] {
            if self.cfg.opt(k) != "UTF-8" {
                return Err(self.unsupported(&alloc::format!("{k} '{}' (only UTF-8)", self.cfg.opt(k))));
            }
        }
        for (k, v) in [("sortcase", "sortcase"), ("sortupper", "sortupper")] {
            let x = self.cfg.opt(k);
            let n = match x.as_str() {
                "true" => "1".into(),
                "false" => "0".into(),
                o => String::from(o),
            };
            if n != "1" && n != "0" {
                return Err(self.fatal(alloc::format!("Invalid value for option '{v}'")));
            }
            self.cfg.opts.insert(k.into(), n);
        }
        if self.cfg.opt_true("output_safechars") {
            return Err(self.unsupported("output_safechars"));
        }
        if self.cfg.opt_true("wraplines") {
            return Err(self.unsupported("wraplines"));
        }
        Ok(())
    }

    /// `process_setup`.
    pub fn process_setup(&mut self) {
        let globalss = self.cfg.blx_s(None, "sortingtemplatename", None, None);
        let secnums: Vec<String> = self.sections.keys().cloned().collect();
        for secnum in secnums {
            if !self.lists.iter().any(|l| l.typ == "entry" && l.section == secnum) {
                self.lists.push(DataList {
                    section: secnum.clone(),
                    typ: "entry".into(),
                    sortingtemplatename: globalss.clone(),
                    sortingnamekeytemplatename: "global".into(),
                    uniquenametemplatename: "global".into(),
                    labelalphanametemplatename: "global".into(),
                    namehashtemplatename: "global".into(),
                    labelprefix: String::new(),
                    name: alloc::format!("{globalss}/global//global/global/global"),
                    ..DataList::default()
                });
            }
        }
    }
}
