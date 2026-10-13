//! Biber.pm: the run (`prepare`) and the per-section passes before the
//! lists: fetching data and dependents, aliases, dynamic sets, related
//! entries, XDATA, sets, crossref inheritance and the static per-entry
//! fields (labelname, labeldate, labeltitle, presort).

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::bib::BibCache;
use crate::btparse::Macros;
use crate::config::{BackendIn, Config, DataModel, BV};
use crate::model::{shared, truthy, Entry, EntryRef, LabelDate, Val};
use crate::section::{DataList, Section};
use crate::unicode::{fc, lc};
use crate::Files;

/// A fatal error (biber_error): the message.
#[derive(Debug)]
pub struct Fatal(pub String);

pub struct Biber<'f> {
    pub files: &'f mut dyn Files,
    pub cfg: Config,
    pub dm: DataModel,
    pub sections: BTreeMap<String, Section>,
    pub cur: String,
    pub lists: Vec<DataList>,
    pub warnings: Vec<String>,
    pub errors: Vec<String>,
    pub log: Vec<String>,
    pub unsupported: Vec<String>,
    pub recode: crate::recode::Recode,
    pub table: crate::collate::Table,
    pub macros: Macros,
    pub bibcache: BibCache,
    pub bt_warnings: Vec<String>,
    pub preamble: Vec<String>,
    pub id_ctr: usize,
    pub no_data: bool,
    pub bbl: crate::bbl::Output,
}

/// Section numbers in numeric order.
pub fn numeric_order(keys: &[String]) -> Vec<String> {
    let mut v = keys.to_vec();
    v.sort_by(|a, b| crate::model::numify(a).partial_cmp(&crate::model::numify(b)).unwrap_or(core::cmp::Ordering::Equal));
    v
}

impl<'f> Biber<'f> {
    pub fn new(files: &'f mut dyn Files) -> Biber<'f> {
        let cfg = Config::new();
        let set = cfg.opt("decodecharsset");
        Biber {
            files,
            cfg,
            dm: DataModel::default(),
            sections: BTreeMap::new(),
            cur: String::new(),
            lists: Vec::new(),
            warnings: Vec::new(),
            errors: Vec::new(),
            log: Vec::new(),
            unsupported: Vec::new(),
            recode: crate::recode::Recode::new(&set),
            table: crate::collate::Table::load(),
            macros: Macros::default(),
            bibcache: BibCache::default(),
            bt_warnings: Vec::new(),
            preamble: Vec::new(),
            id_ctr: 0,
            no_data: false,
            bbl: crate::bbl::Output::default(),
        }
    }

    pub fn fatal(&mut self, msg: String) -> Fatal {
        self.errors.push(msg.clone());
        Fatal(msg)
    }
    pub fn unsupported(&mut self, what: &str) -> Fatal {
        self.unsupported.push(what.into());
        let m = alloc::format!("unsupported feature in the native biber: {what}");
        self.errors.push(m.clone());
        Fatal(m)
    }
    pub fn sec(&mut self) -> &mut Section {
        let c = self.cur.clone();
        self.sections.entry(c.clone()).or_insert_with(|| Section::new(&c))
    }
    pub fn secr(&self) -> &Section {
        &self.sections[&self.cur]
    }
    pub fn entry(&self, k: &str) -> Option<EntryRef> {
        self.secr().entry(k)
    }
    pub fn be(&self, k: &str) -> EntryRef {
        self.entry(k).unwrap_or_else(|| shared(Entry::default()))
    }

    /// Options of a scope given on input (`expand_option_input`).
    pub fn expand_option_input(&self, scope: &str, opt: &str, val: &str) -> Vec<(String, String)> {
        let v = self.cfg.map_boolean(opt, val, true);
        let mut out = Vec::new();
        match self.cfg.optin.get(&(scope.into(), lc(opt))) {
            None => {
                if let Some(v) = v {
                    out.push((opt.into(), v));
                }
            }
            Some(BackendIn::List(ks)) => {
                if let Some(v) = v {
                    for k in ks {
                        out.push((k.clone(), v.clone()));
                    }
                }
            }
            Some(BackendIn::Map(m)) => {
                for (k, sv) in m {
                    let subval = self.cfg.map_boolean(k, sv, true).unwrap_or_default();
                    if !v.as_deref().is_some_and(truthy) {
                        if self.cfg.opttype.get(k).map(String::as_str) == Some("boolean") {
                            out.push((k.clone(), if truthy(&subval) { "0".into() } else { "1".into() }));
                        }
                    } else {
                        out.push((k.clone(), subval));
                    }
                }
            }
        }
        out
    }

    /// Utils `process_entry_options`.
    pub fn process_entry_options(&mut self, key: &str, options: &[String], secnum: &str) {
        for o in options {
            let o = {
                let re = crate::regex::Regex::new(r"\s+=\s+", "").expect("static");
                re.replace(o, true, &mut |_, _| "=".into())
            };
            let (name, val) = match o.find('=') {
                Some(i) => (o[..i].to_string(), if o.len() > i + 1 { o[i + 1..].to_string() } else { "1".into() }),
                None => (o.clone(), "1".into()),
            };
            if name.is_empty() {
                continue;
            }
            for (k, v) in self.expand_option_input("ENTRY", &name, &val) {
                self.cfg.set_blx(secnum, &k, BV::S(v), Some(("ENTRY", key)));
            }
        }
    }

    /// Biber `prepare`.
    pub fn prepare(&mut self) -> Result<(), Fatal> {
        self.process_setup();
        let secs = numeric_order(&self.sections.keys().cloned().collect::<Vec<_>>());
        for secnum in secs {
            {
                let s = &self.sections[&secnum];
                if s.citekeys.is_empty() && !s.allkeys {
                    continue;
                }
            }
            self.log.push(alloc::format!("Processing section {secnum}"));
            self.cur = secnum.clone();
            {
                let s = self.sec();
                s.labelcache_l.clear();
                s.labelcache_v.clear();
            }
            self.cfg.crossrefkeys.clear();
            self.cfg.xrefkeys.clear();
            self.cfg.inherit_crossref.clear();
            self.cfg.inherit_xdata.clear();
            self.cfg.uniqignore.clear();
            self.fetch_data()?;
            self.process_citekey_aliases();
            self.instantiate_dynamic();
            self.resolve_alias_refs();
            self.resolve_xdata()?;
            self.cite_setmembers();
            self.preprocess_sets();
            self.calculate_interentry();
            self.process_interentry()?;
            self.postprocess_sets();
            self.process_entries_static();
            self.log.push(alloc::format!("section {secnum}: {} citekeys, allkeys {}", self.secr().citekeys.len(), self.secr().allkeys));
            self.process_lists()?;
            self.create_output_section()?;
        }
        self.create_output_misc();
        Ok(())
    }

    fn fetch_data(&mut self) -> Result<(), Fatal> {
        let citekeys = self.sec().static_citekeys();
        if self.cfg.opt_true("clrmacros") {
            self.macros.0.clear();
        }
        if !self.cfg.opt_true("nostdmacros") {
            crate::bib::add_month_macros(&mut self.macros);
        }
        let mut remaining = citekeys;
        let dss = self.sec().datasources.clone().unwrap_or_default();
        let mut ds2 = Vec::new();
        for ds in dss {
            if ds.typ != "file" {
                return Err(self.unsupported(&alloc::format!("data source type '{}'", ds.typ)));
            }
            if truthy(&ds.glob) && ds.glob != "false" {
                return Err(self.unsupported("globbed data sources"));
            }
            ds2.push(ds);
        }
        self.sec().datasources = Some(ds2.clone());
        for ds in &ds2 {
            if remaining.is_empty() && !self.secr().allkeys {
                break;
            }
            if ds.datatype != "bibtex" {
                return Err(self.unsupported(&alloc::format!("data source datatype '{}'", ds.datatype)));
            }
            if ds.name.starts_with("http://") || ds.name.starts_with("https://") || ds.name.starts_with("ftp://") || ds.name.starts_with("ftps://") {
                return Err(self.unsupported("remote data sources"));
            }
            let Some((path, bytes)) = self.files.datasource(&ds.name) else {
                return Err(self.fatal(alloc::format!("Cannot find '{}'!", ds.name)));
            };
            self.log.push(alloc::format!("Looking for bibtex file '{}' for section {}", ds.name, self.cur));
            remaining = self.extract_entries(&path, &bytes, &remaining)?;
        }
        let secnum = self.cur.clone();
        for k in remaining {
            self.warnings.push(alloc::format!("I didn't find a database entry for '{k}' (section {secnum})"));
            self.sec().del_citekey(&k);
            self.sec().undef_citekeys.push(k);
        }
        let keys = self.sec().citekeys.clone();
        let mut kwd = Vec::new();
        self.get_dependents(&keys, &mut kwd)?;
        Ok(())
    }

    fn get_dependents(&mut self, keys: &[String], keyswithdeps: &mut Vec<String>) -> Result<(), Fatal> {
        let mut new_deps: Vec<String> = Vec::new();
        let mut missing: Vec<String> = Vec::new();
        let push_u = |v: &mut Vec<String>, k: &str| {
            if !v.iter().any(|x| x == k) {
                v.push(k.into());
            }
        };
        for ck in keys {
            if let Some(real) = self.secr().alias_of(ck).filter(|s| truthy(s)) {
                new_deps.push(real.clone());
                push_u(keyswithdeps, &real);
            } else if !self.secr().dynamic_set(ck).is_empty() {
                for m in self.secr().dynamic_set(ck) {
                    if self.entry(&m).is_none() {
                        new_deps.push(m);
                        push_u(keyswithdeps, ck);
                    }
                }
            } else {
                let Some(be) = self.entry(ck) else { continue };
                let b = be.borrow();
                if let Some(x) = &b.xrefs {
                    for xd in x {
                        for r in &xd.entries {
                            if self.entry(r).is_none() {
                                new_deps.push(r.clone());
                            }
                            push_u(keyswithdeps, ck);
                        }
                    }
                }
                for f in ["xref", "crossref"] {
                    if b.has(f) {
                        let r = b.gs(f);
                        if self.entry(&r).is_none() {
                            new_deps.push(r);
                        }
                        push_u(keyswithdeps, ck);
                    }
                }
                if b.etype() == "set" {
                    if let Some(l) = b.get("entryset").and_then(|v| v.as_list()) {
                        for sm in l.borrow().iter() {
                            if !self.secr().has_citekey(sm) {
                                new_deps.push(sm.clone());
                                push_u(keyswithdeps, ck);
                            }
                        }
                    }
                }
                if let Some(l) = b.get("related").and_then(|v| v.as_list()) {
                    let rel = l.borrow().clone();
                    drop(b);
                    for rm in rel {
                        if !(self.secr().has_citekey(&rm) || self.secr().relkeys.contains(&rm)) {
                            self.sec().relkeys.insert(rm.clone());
                            new_deps.push(rm);
                        }
                        push_u(keyswithdeps, ck);
                    }
                }
            }
        }
        let mut u = Vec::new();
        for d in new_deps {
            if !u.contains(&d) {
                u.push(d);
            }
        }
        let mut new_deps = u;
        if !new_deps.is_empty() {
            if self.secr().allkeys {
                for dk in &new_deps {
                    if !self.secr().has_citekey(dk) {
                        missing.push(dk.clone());
                    }
                }
            } else {
                missing = new_deps.clone();
                let dss = self.secr().datasources.clone().unwrap_or_default();
                for ds in dss {
                    if missing.is_empty() {
                        break;
                    }
                    let Some((path, bytes)) = self.files.datasource(&ds.name) else {
                        return Err(self.fatal(alloc::format!("Cannot find '{}'!", ds.name)));
                    };
                    missing = self.extract_entries(&path, &bytes, &missing)?;
                }
            }
            new_deps.retain(|d| !missing.contains(d));
        }
        if !new_deps.is_empty() {
            self.get_dependents(&new_deps.clone(), keyswithdeps)?;
        }
        for kwd in keyswithdeps.clone() {
            for mk in &missing {
                self.remove_undef_dependent(&kwd, mk);
            }
        }
        Ok(())
    }

    fn remove_undef_dependent(&mut self, ck: &str, mk: &str) {
        let secnum = self.cur.clone();
        let dm = self.secr().dynamic_set(ck);
        if !dm.is_empty() {
            if dm.iter().any(|x| x == mk) {
                let n: Vec<String> = dm.into_iter().filter(|x| x != mk).collect();
                self.sec().set_dynamic_set(ck, n);
                self.warnings.push(alloc::format!("I didn't find a database entry for dynamic set member '{mk}' - ignoring (section {secnum})"));
            }
            return;
        }
        let Some(be) = self.entry(ck) else { return };
        let mut b = be.borrow_mut();
        for f in ["xref", "crossref"] {
            if b.has(f) && b.gs(f) == mk {
                let w = alloc::format!("I didn't find a database entry for {f} '{mk}' in entry '{ck}' - ignoring (section {secnum})");
                b.add_warning(w.clone());
                self.warnings.push(w);
                b.del_field(f);
            }
        }
        if let Some(x) = b.get("xdata").and_then(|v| v.as_list()) {
            let l = x.borrow().clone();
            if l.iter().any(|k| k == mk) {
                let w = alloc::format!("I didn't find a database entry for xdata entry '{mk}' in entry '{ck}' - ignoring (section {secnum})");
                b.add_warning(w.clone());
                self.warnings.push(w);
            }
            b.set_data("xdata", Val::list(l.into_iter().filter(|k| k != mk).collect()));
        }
        if b.etype() == "set" {
            if let Some(x) = b.get("entryset").and_then(|v| v.as_list()) {
                let l = x.borrow().clone();
                if l.iter().any(|k| k == mk) {
                    b.set_data("entryset", Val::list(l.into_iter().filter(|k| k != mk).collect()));
                    let w = alloc::format!("I didn't find a database entry for static set member '{mk}' in entry '{ck}' - ignoring (section {secnum})");
                    b.add_warning(w.clone());
                    self.warnings.push(w);
                }
            }
        }
        if let Some(x) = b.get("related").and_then(|v| v.as_list()) {
            let l = x.borrow().clone();
            if l.iter().any(|k| k == mk) {
                let n: Vec<String> = l.into_iter().filter(|k| k != mk).collect();
                let empty = n.is_empty();
                b.set_data("related", Val::list(n));
                if empty {
                    b.del_field("relatedtype");
                    b.del_field("relatedstring");
                }
                let w = alloc::format!("I didn't find a database entry for related entry '{mk}' in entry '{ck}' - ignoring (section {secnum})");
                b.add_warning(w.clone());
                self.warnings.push(w);
            }
        }
    }

    fn process_citekey_aliases(&mut self) {
        for ck in self.sec().citekeys.clone() {
            if self.secr().alias_of(&ck).is_some_and(|s| truthy(&s)) {
                self.sec().del_citekey(&ck);
            }
        }
    }

    fn instantiate_dynamic(&mut self) {
        let secnum = self.cur.clone();
        for (dset, members) in self.secr().dkeys.clone() {
            let members: Vec<String> = members.iter().map(|m| self.secr().alias_of(m).unwrap_or_else(|| m.clone())).collect();
            self.sec().set_dynamic_set(&dset, members.clone());
            let mut be = Entry::default();
            be.set_field("entrytype", Val::s("set"));
            be.set_field("entryset", Val::list(members.clone()));
            be.set_field("citekey", Val::s(dset.clone()));
            be.set_field("datatype", Val::s("dynamic"));
            self.sec().entries.insert(dset.clone(), shared(be));
            let minorder = members
                .iter()
                .filter_map(|m| self.cfg.keyorder(&secnum, m))
                .map(|o| crate::model::numify(&o))
                .fold(None, |a: Option<f64>, x| Some(a.map_or(x, |a| a.min(x))));
            for m in &members {
                if let Some(e) = self.entry(m) {
                    self.relclone(&e);
                }
                if let Some(mo) = minorder.filter(|&x| x != 0.0) {
                    self.cfg.keyorder.entry(secnum.clone()).or_default().insert(dset.clone(), crate::model::num_str(mo));
                }
            }
        }
        for ck in self.sec().citekeys.clone() {
            if let Some(e) = self.entry(&ck) {
                self.relclone(&e);
            }
        }
    }

    /// Biber::Entry `relclone`.
    fn relclone(&mut self, be: &EntryRef) {
        let secnum = self.cur.clone();
        let rel = be.borrow().get("related").and_then(|v| v.as_list());
        let Some(rel) = rel else { return };
        let relkeys = rel.borrow().clone();
        let mut clonekeys = Vec::new();
        for relkey in relkeys {
            let relkey = self.secr().alias_of(&relkey).unwrap_or(relkey);
            if let Some(ck) = self.secr().keytorelclone.get(&relkey).cloned() {
                clonekeys.push(ck);
            } else {
                let Some(relentry) = self.entry(&relkey) else { continue };
                let clonekey = crate::internals::md5_hex(relkey.as_bytes());
                clonekeys.push(clonekey.clone());
                let relclone = self.clone_entry(&relentry.borrow(), Some(&clonekey));
                let relopts = be.borrow().get("relatedoptions").and_then(|v| v.as_list()).map(|l| l.borrow().clone());
                let opts = if let Some(ro) = relopts {
                    if self.secr().has_citekey(&relkey) {
                        merge_entry_options(Some(ro), Some(alloc::vec!["skipbib".into(), "skipbiblist".into()]))
                    } else {
                        ro
                    }
                } else {
                    let eo = relentry.borrow().get("options").and_then(|v| v.as_list()).map(|l| l.borrow().clone());
                    merge_entry_options(
                        Some(["skipbib", "skiplab", "skipbiblist", "uniquename=false", "uniquelist=false"].iter().map(|s| (*s).to_string()).collect()),
                        eo,
                    )
                };
                self.process_entry_options(&clonekey, &opts, &secnum);
                relclone.borrow_mut().set_data("options", Val::list(opts));
                self.sec().entries.insert(clonekey.clone(), relclone.clone());
                self.sec().keytorelclone.insert(relkey.clone(), clonekey.clone());
                self.relclone(&relclone);
            }
        }
        self.sec().add_citekeys(&clonekeys);
        be.borrow_mut().set_data("related", Val::list(clonekeys));
    }

    /// Biber::Entry `clone`.
    pub fn clone_entry(&self, e: &Entry, newkey: Option<&str>) -> EntryRef {
        let mut n = Entry { data: e.data.clone(), xrefs: e.xrefs.clone(), ..Entry::default() };
        for df in &self.dm.helpers.datefields {
            let d = df.strip_suffix("date").unwrap_or(df);
            for dsf in ["dateunspecified", "datesplit", "datejulian", "enddatejulian", "dateapproximate", "enddateapproximate", "dateuncertain", "enddateuncertain", "yeardivision", "era", "endera"] {
                let k = alloc::format!("{d}{dsf}");
                if let Some(v) = e.derived.get(&k).filter(|v| v.truthy()) {
                    n.derived.insert(k, v.clone());
                }
            }
        }
        for k in ["entrytype", "datatype"] {
            if let Some(v) = e.derived.get(k) {
                n.derived.insert(k.into(), v.clone());
            }
        }
        if let Some(k) = newkey.filter(|s| truthy(s)) {
            n.derived.insert("citekey".into(), Val::s(k));
        }
        n.derived.insert("clonesourcekey".into(), Val::s(e.key()));
        shared(n)
    }

    fn resolve_alias_refs(&mut self) {
        for ck in self.sec().citekeys.clone() {
            let Some(be) = self.entry(&ck) else { continue };
            let mut b = be.borrow_mut();
            for f in ["xref", "crossref"] {
                if b.has(f) {
                    if let Some(r) = self.secr().alias_of(&b.gs(f)).filter(|s| truthy(s)) {
                        b.set_data(f, Val::s(r));
                    }
                }
            }
            if let Some(x) = b.xrefs.as_mut() {
                for xd in x.iter_mut() {
                    let sec = &self.sections[&self.cur];
                    if xd.field.is_none() {
                        xd.entries = xd.entries.iter().map(|k| sec.alias_of(k).unwrap_or_else(|| k.clone())).collect();
                    } else if let Some(k) = xd.entries.first().cloned() {
                        xd.entries = alloc::vec![sec.alias_of(&k).unwrap_or(k)];
                    }
                }
            }
        }
    }

    fn resolve_xdata(&mut self) -> Result<(), Fatal> {
        let entries: Vec<EntryRef> = self.secr().entries.values().cloned().collect();
        for be in entries {
            if be.borrow().etype() == "xdata" {
                continue;
            }
            if be.borrow().xrefs.is_none() {
                continue;
            }
            self.entry_resolve_xdata(&be)?;
        }
        Ok(())
    }

    fn entry_resolve_xdata(&mut self, be: &EntryRef) -> Result<(), Fatal> {
        let secnum = self.cur.clone();
        let key = be.borrow().key();
        let bee = be.borrow().etype();
        let xrefs = be.borrow().xrefs.clone().unwrap_or_default();
        let mut newx = xrefs.clone();
        for (xi, xd) in xrefs.iter().enumerate() {
            for xdref in &xd.entries {
                let Some(xe) = self.entry(xdref) else {
                    let w = alloc::format!("{bee} entry '{key}' references XDATA entry '{xdref}' which does not exist, not resolving (section {secnum})");
                    be.borrow_mut().add_warning(w.clone());
                    self.warnings.push(w);
                    newx[xi].resolved = Some(false);
                    continue;
                };
                if xe.borrow().etype() != "xdata" {
                    let w = alloc::format!("{bee} entry '{key}' references XDATA entry '{xdref}' which is not an XDATA entry, not resolving (section {secnum})");
                    be.borrow_mut().add_warning(w.clone());
                    self.warnings.push(w);
                    newx[xi].resolved = Some(false);
                    continue;
                }
                self.cfg.inherit_xdata.push((xdref.clone(), key.clone()));
                if Config::is_inheritance_path(&self.cfg.inherit_xdata, &key, xdref) {
                    return Err(self.fatal(alloc::format!("Circular XDATA inheritance between '{xdref}'<->'{key}'")));
                }
                if xe.borrow().xrefs.is_some() {
                    self.entry_resolve_xdata(&xe)?;
                }
                match &xd.field {
                    None => {
                        let fields = xe.borrow().datafields();
                        for f in fields {
                            if f == "ids" {
                                continue;
                            }
                            let v = xe.borrow().get(&f);
                            if let Some(v) = v {
                                be.borrow_mut().set_data(&f, v);
                            }
                        }
                    }
                    Some(xf) => {
                        let rf = &xd.reffield;
                        let (rft, rdt) = (self.dm.fieldtype(rf), self.dm.datatype(rf));
                        let (xft, xdt) = (self.dm.fieldtype(xf), self.dm.datatype(xf));
                        if !(rft == xft && rdt == xdt) {
                            let w = alloc::format!("Field '{rf}' in {bee} entry '{key}' which xdata references field '{xf}' in entry '{xdref}' are not the same types, not resolving (section {secnum})");
                            be.borrow_mut().add_warning(w.clone());
                            self.warnings.push(w);
                            newx[xi].resolved = Some(false);
                            continue;
                        }
                        let xv = xe.borrow().get(xf);
                        let Some(xv) = xv.filter(|v| v.truthy()) else {
                            let w = alloc::format!("Field '{rf}' in {bee} entry '{key}' references XDATA field '{xf}' in entry '{xdref}' and this field does not exist, not resolving (section {secnum})");
                            be.borrow_mut().add_warning(w.clone());
                            self.warnings.push(w);
                            newx[xi].resolved = Some(false);
                            continue;
                        };
                        if self.dm.is_type("list", "name", rf) {
                            return Err(self.unsupported("granular XDATA references in name lists"));
                        } else if self.dm.fieldtype(rf) == "list" {
                            let src = xv.as_list().map(|l| l.borrow().clone()).unwrap_or_default();
                            let tgt = be.borrow().get(rf).and_then(|v| v.as_list());
                            let Some(tgt) = tgt else { continue };
                            let rp = xd.refposition;
                            if xd.position == "*" {
                                let mut t = tgt.borrow_mut();
                                let i = rp.saturating_sub(1).min(t.len());
                                let end = (i + 1).min(t.len());
                                t.splice(i..end, src);
                            } else {
                                let p: usize = xd.position.parse().unwrap_or(0);
                                let Some(v) = src.get(p.wrapping_sub(1)).filter(|s| truthy(s)).cloned() else {
                                    let w = alloc::format!("Field '{rf}' in {bee} entry '{key}' references field '{xf}' position {p} in entry '{xdref}' and this position does not exist, not resolving (section {secnum})");
                                    be.borrow_mut().add_warning(w.clone());
                                    self.warnings.push(w);
                                    newx[xi].resolved = Some(false);
                                    continue;
                                };
                                let mut t = tgt.borrow_mut();
                                if rp >= 1 && rp <= t.len() {
                                    t[rp - 1] = v;
                                }
                            }
                        } else {
                            be.borrow_mut().set_data(rf, xv);
                        }
                    }
                }
                newx[xi].resolved = Some(true);
            }
        }
        be.borrow_mut().xrefs = Some(newx);
        Ok(())
    }

    fn cite_setmembers(&mut self) {
        for ck in self.sec().citekeys.clone() {
            let Some(be) = self.entry(&ck) else { continue };
            if be.borrow().etype() != "set" {
                continue;
            }
            let Some(l) = be.borrow().get("entryset").and_then(|v| v.as_list()) else { continue };
            let keys = l.borrow().clone();
            if keys.is_empty() {
                continue;
            }
            let real: Vec<String> = keys.iter().map(|m| self.secr().alias_of(m).unwrap_or_else(|| m.clone())).collect();
            be.borrow_mut().set_data("entryset", Val::list(real.clone()));
            for k in &real {
                self.sec().add_citekeys(&[k.clone()]);
            }
            if let Some(first) = self.entry(&real[0]) {
                set_inherit_from(&mut be.borrow_mut(), &first.borrow(), &self.dm);
            }
            if be.borrow().has("crossref") {
                let w = alloc::format!("Field 'crossref' is no longer needed in set entries in Biber - ignoring in entry '{ck}'");
                be.borrow_mut().add_warning(w.clone());
                self.warnings.push(w);
                be.borrow_mut().del_field("crossref");
            }
        }
    }

    fn preprocess_sets(&mut self) {
        for ck in self.sec().citekeys.clone() {
            let Some(be) = self.entry(&ck) else { continue };
            if be.borrow().etype() != "set" {
                continue;
            }
            let es = be.borrow().get("entryset").filter(|v| v.truthy()).and_then(|v| v.as_list());
            let Some(es) = es else {
                let w = alloc::format!("Set entry '{ck}' has no entryset field, ignoring");
                be.borrow_mut().add_warning(w.clone());
                self.warnings.push(w);
                continue;
            };
            for m in es.borrow().clone() {
                self.sec().add_set_pc(&ck, &m);
                self.sec().add_set_cp(&m, &ck);
                if let Some(me) = self.entry(&m) {
                    self.relclone(&me);
                }
            }
        }
    }

    fn calculate_interentry(&mut self) {
        for ck in self.sec().citekeys.clone() {
            let Some(be) = self.entry(&ck) else { continue };
            let b = be.borrow();
            if b.has("crossref") {
                let r = b.gs("crossref");
                if self.entry(&r).is_some() {
                    self.cfg.incr_crossrefkey(&r);
                }
            }
            if b.has("xref") {
                let r = b.gs("xref");
                self.cfg.incr_xrefkey(&r);
            }
        }
        let min_c = crate::model::numify(&self.cfg.opt("mincrossrefs")) as usize;
        for (k, n) in self.cfg.crossrefkeys.clone() {
            if n >= min_c {
                if !self.secr().has_citekey(&k) {
                    if let Some(e) = self.entry(&k) {
                        e.borrow_mut().set_field("crossrefsource", Val::s("1"));
                    }
                }
                self.sec().add_citekeys(&[k]);
            }
        }
        let min_x = crate::model::numify(&self.cfg.opt("minxrefs")) as usize;
        for (k, n) in self.cfg.xrefkeys.clone() {
            if n >= min_x {
                if !self.secr().has_citekey(&k) {
                    if let Some(e) = self.entry(&k) {
                        e.borrow_mut().set_field("xrefsource", Val::s("1"));
                    }
                }
                self.sec().add_citekeys(&[k]);
            }
        }
    }

    fn process_interentry(&mut self) -> Result<(), Fatal> {
        for ck in self.sec().citekeys.clone() {
            let Some(be) = self.entry(&ck) else { continue };
            if !be.borrow().has("crossref") {
                continue;
            }
            let cr = be.borrow().gs("crossref");
            let k = be.borrow().key();
            if self.cfg.inherit_crossref.iter().any(|(s, t)| *s == cr && *t == k) {
                continue;
            }
            match self.entry(&cr) {
                None => {
                    let w = alloc::format!("Cannot inherit from crossref key '{cr}' - does it exist?");
                    be.borrow_mut().add_warning(w.clone());
                    self.warnings.push(w);
                }
                Some(p) => self.inherit_from(&be, &p)?,
            }
        }
        Ok(())
    }

    /// Biber::Entry `inherit_from`.
    fn inherit_from(&mut self, child: &EntryRef, parent: &EntryRef) -> Result<(), Fatal> {
        let secnum = self.cur.clone();
        let tk = child.borrow().key();
        let sk = parent.borrow().key();
        self.cfg.inherit_crossref.push((sk.clone(), tk.clone()));
        if Config::is_inheritance_path(&self.cfg.inherit_crossref, &tk, &sk) {
            return Err(self.fatal(alloc::format!("Circular inheritance between '{sk}'<->'{tk}'")));
        }
        if parent.borrow().has("crossref") {
            let pp = parent.borrow().gs("crossref");
            if let Some(ppe) = self.entry(&pp) {
                self.inherit_from(parent, &ppe)?;
            }
        }
        let bee = child.borrow().etype();
        let ptype = parent.borrow().etype();
        let inh = self.cfg.inheritance.clone();
        let defaults = inh.kid("defaults").cloned().unwrap_or_default();
        let mut inherit_all = defaults.attr("inherit_all").unwrap_or("").to_string();
        let mut override_target = defaults.attr("override_target").unwrap_or("").to_string();
        let mut dignore: Option<String> = defaults.attr("ignore").map(Into::into);
        for tp in defaults.kids("type_pair") {
            let s = tp.attr("source").unwrap_or("");
            let t = tp.attr("target").unwrap_or("");
            if (s == "*" || s == ptype) && (t == "*" || t == bee) {
                if let Some(x) = tp.attr("inherit_all").filter(|s| truthy(s)) {
                    inherit_all = x.into();
                }
                if let Some(x) = tp.attr("override_target").filter(|s| truthy(s)) {
                    override_target = x.into();
                }
                if let Some(x) = tp.attr("ignore") {
                    dignore = Some(x.into());
                }
            }
        }
        let noinherit = self.cfg.blx(Some(&secnum), "noinherit", None, Some(&tk)).filter(|v| v.truthy()).map(|v| v.s());
        let mut processed: Vec<String> = Vec::new();
        for inherit in inh.kids("inherit") {
            for tp in inherit.kids("type_pair") {
                let s = tp.attr("source").unwrap_or("");
                let t = tp.attr("target").unwrap_or("");
                if !((s == "*" || s == ptype) && (t == "*" || t == bee)) {
                    continue;
                }
                for field in inherit.kids("field") {
                    let fsrc = field.attr("source").unwrap_or("").to_string();
                    let ftgt = field.attr("target").map(String::from);
                    if let (Some(ni), Some(tg)) = (&noinherit, &ftgt) {
                        if self.cfg.datafield_sets.get(ni).is_some_and(|set| set.contains(tg)) {
                            continue;
                        }
                    }
                    if !parent.borrow().exists(&fsrc) {
                        continue;
                    }
                    processed.push(fsrc.clone());
                    let fot = field.attr("override_target").unwrap_or("false");
                    if field.attr("skip").is_some_and(truthy) {
                        continue;
                    }
                    let tg = ftgt.clone().unwrap_or_default();
                    if !child.borrow().exists(&tg) || fot == "true" {
                        let v = parent.borrow().get(&fsrc);
                        if let Some(v) = v {
                            child.borrow_mut().set_data(&tg, v);
                        } else {
                            child.borrow_mut().data.remove(&tg);
                        }
                        let ign = inherit.attr("ignore").filter(|s| truthy(s)).map(String::from).or_else(|| dignore.clone());
                        self.cfg.add_uniq_ignore(&tk, &tg, ign.as_deref());
                    }
                }
            }
        }
        if inherit_all == "true" {
            let fields = parent.borrow().datafields();
            let mut filtered = Vec::new();
            let mut removed = Vec::new();
            for f in fields {
                if self.dm.helpers.dateparts.contains(&f) && parent.borrow().has("datesplit") && child.borrow().has("datesplit") && child.borrow().date_fields_exist(&f) {
                    if override_target == "true" {
                        child.borrow_mut().delete_date_fields(&f);
                    } else {
                        removed.push(f);
                        continue;
                    }
                }
                filtered.push(f);
            }
            for df in &self.dm.helpers.datefields {
                if removed.contains(df) {
                    continue;
                }
                let d = df.strip_suffix("date").unwrap_or(df);
                for dsf in ["dateunspecified", "datesplit", "datejulian", "enddatejulian", "dateapproximate", "enddateapproximate", "dateuncertain", "enddateuncertain", "yeardivision", "endyeardivision", "era", "endera"] {
                    let k = alloc::format!("{d}{dsf}");
                    let v = parent.borrow().derived.get(&k).filter(|v| v.truthy()).cloned();
                    if let Some(v) = v {
                        child.borrow_mut().derived.insert(k, v);
                    }
                }
            }
            for f in filtered {
                if let Some(ni) = &noinherit {
                    if self.cfg.datafield_sets.get(ni).is_some_and(|set| set.contains(&f)) {
                        continue;
                    }
                }
                if processed.contains(&f) {
                    continue;
                }
                if !child.borrow().exists(&f) || override_target == "true" {
                    let v = parent.borrow().get(&f);
                    if let Some(v) = v {
                        child.borrow_mut().set_data(&f, v);
                    }
                    self.cfg.add_uniq_ignore(&tk, &f, dignore.as_deref());
                }
            }
        }
        for df in &self.dm.helpers.datefields {
            let d = df.strip_suffix("date").unwrap_or(df);
            let k = alloc::format!("{d}datesplit");
            let v = parent.borrow().get(&k).filter(|v| v.truthy());
            if let Some(v) = v {
                child.borrow_mut().set_field(&k, v);
            }
        }
        Ok(())
    }

    fn postprocess_sets(&mut self) {
        let secnum = self.cur.clone();
        let opts: Vec<String> = ["skipbib", "skiplab", "skipbiblist", "uniquename=false", "uniquelist=false"].iter().map(|s| (*s).to_string()).collect();
        for ck in self.sec().citekeys.clone() {
            let children = self.secr().set_pc.get(&ck).cloned().unwrap_or_default();
            if !children.is_empty() {
                for m in children {
                    self.process_entry_options(&m, &opts, &secnum);
                    if let Some(me) = self.entry(&m) {
                        if me.borrow().data.get("entryset").is_some_and(Val::truthy) {
                            let w = alloc::format!("Field 'entryset' is no longer needed in set member entries in Biber - ignoring in entry '{m}'");
                            me.borrow_mut().add_warning(w.clone());
                            self.warnings.push(w);
                            me.borrow_mut().del_field("entryset");
                        }
                        me.borrow_mut().set_field("entryset", Val::list(alloc::vec![ck.clone()]));
                    }
                }
            } else if self.secr().set_cp.get(&ck).is_some_and(|v| !v.is_empty()) {
                self.process_entry_options(&ck, &opts, &secnum);
            }
        }
    }

    fn process_entries_static(&mut self) {
        let secnum = self.cur.clone();
        for ck in self.sec().citekeys.clone() {
            let Some(be) = self.entry(&ck) else { continue };
            // nocite
            {
                let s = self.secr();
                if s.nocite.contains(&ck) || (s.allkeys_nocite && !s.is_specific(&ck)) {
                    be.borrow_mut().set_field("nocite", Val::s("1"));
                }
            }
            let bee = be.borrow().etype();
            // labelname
            let spec = self.cfg.blx(None, "labelnamespec", Some(&bee), None).map(|v| v.items()).unwrap_or_default();
            for h in &spec {
                let ln = h.text.clone();
                let lnameopt = ln.strip_prefix("short").filter(|x| !x.is_empty()).map_or(ln.clone(), String::from);
                if !self.dm.helpers.namelistsall.contains(&ln) {
                    self.warnings.push(alloc::format!("Labelname candidate '{ln}' is not a name field - skipping"));
                    continue;
                }
                let uo = alloc::format!("use{lnameopt}");
                if self.cfg.optscope.contains_key(&uo) && !self.cfg.blx_b(Some(&secnum), &uo, Some(&bee), Some(&ck)) {
                    continue;
                }
                if be.borrow().has(&ln) {
                    be.borrow_mut().labelname = Some(ln);
                    break;
                }
            }
            for h in &spec {
                let ln = h.text.clone();
                if ln.starts_with("short") && ln.len() > 5 {
                    continue;
                }
                if !self.dm.helpers.namelistsall.contains(&ln) {
                    continue;
                }
                let uo = alloc::format!("use{ln}");
                if self.cfg.optscope.contains_key(&uo) && !self.cfg.blx_b(Some(&secnum), &uo, Some(&bee), Some(&ck)) {
                    continue;
                }
                if be.borrow().has(&ln) {
                    be.borrow_mut().labelnamefh = Some(ln);
                    break;
                }
            }
            self.process_labeldate(&be, &bee, &ck);
            // labeltitle
            let lts = self.cfg.blx(None, "labeltitlespec", Some(&bee), None).map(|v| v.items()).unwrap_or_default();
            for h in &lts {
                let lt = be.borrow().get(&h.text).filter(|v| v.truthy());
                if let Some(v) = lt {
                    let mut b = be.borrow_mut();
                    b.labeltitle = Some(h.text.clone());
                    b.set_field("labeltitle", v);
                    break;
                }
            }
            // presort
            if be.borrow().has("presort") {
                let ps = be.borrow().gs("presort");
                self.cfg.set_blx(&secnum, "presort", BV::S(ps), Some(("ENTRY", &ck)));
            }
        }
    }

    fn process_labeldate(&mut self, be: &EntryRef, bee: &str, ck: &str) {
        if !self.cfg.blx_b(None, "labeldateparts", Some(bee), Some(ck)) {
            return;
        }
        let spec = self.cfg.blx(None, "labeldatespec", Some(bee), None).map(|v| v.items()).unwrap_or_default();
        for lds in &spec {
            let ld = lds.text.clone();
            match lds.attr("type") {
                Some("field") => {
                    let (ldy, ldey, rest, pseudo, datetype);
                    if self.dm.datatype(&ld) == "date" {
                        let dt = ld.strip_suffix("date").unwrap_or(&ld).to_string();
                        ldy = alloc::format!("{dt}year");
                        ldey = Some(alloc::format!("{dt}endyear"));
                        rest = Some(dt.clone());
                        pseudo = false;
                        datetype = dt;
                    } else {
                        ldy = ld.clone();
                        ldey = None;
                        rest = None;
                        pseudo = true;
                        datetype = String::new();
                    }
                    let b = be.borrow();
                    if b.get(&ldy).is_some() || ldey.as_deref().is_some_and(|k| b.get(k).is_some()) {
                        drop(b);
                        let f = |s: &str| rest.as_ref().map(|d| alloc::format!("{d}{s}"));
                        be.borrow_mut().labeldate = Some(LabelDate::Field {
                            year: ldy.clone(),
                            month: f("month"),
                            day: f("day"),
                            hour: f("hour"),
                            minute: f("minute"),
                            second: f("second"),
                            timezone: f("timezone"),
                            pseudodate: pseudo,
                            source: if pseudo { ldy } else { datetype },
                        });
                        break;
                    }
                }
                Some("string") => {
                    be.borrow_mut().labeldate = Some(LabelDate::Str(ld));
                    break;
                }
                _ => {}
            }
        }
        let ldi = be.borrow().labeldate.clone();
        match ldi {
            Some(LabelDate::Field { year, month, day, hour, minute, second, timezone, pseudodate, source }) => {
                let mut b = be.borrow_mut();
                let gv = |b: &Entry, k: &str| b.get(k);
                let yv = gv(&b, &year);
                match yv {
                    Some(v) => b.set_field("labelyear", v),
                    None => {
                        b.derived.remove("labelyear");
                    }
                }
                for (lab, src) in [("labelmonth", &month), ("labelday", &day), ("labelhour", &hour), ("labelminute", &minute), ("labelsecond", &second), ("labeltimezone", &timezone)] {
                    if let Some(s) = src {
                        match gv(&b, s) {
                            Some(v) => b.set_field(lab, v),
                            None => {
                                b.derived.remove(lab);
                            }
                        }
                    }
                }
                b.set_field("labeldatesource", Val::s(source));
                let ytype = year.strip_suffix("year").unwrap_or("").to_string();
                let ey = alloc::format!("{ytype}endyear");
                if b.exists(&ey) && b.gs(&year) != b.gs(&ey) {
                    let v = alloc::format!("{}\\bibdatedash {}", b.gs("labelyear"), b.gs(&ey));
                    b.set_field("labelyear", Val::s(v));
                }
                if !pseudodate {
                    for (part, lab, src) in [("month", "labelmonth", &month), ("day", "labelday", &day), ("hour", "labelhour", &hour), ("minute", "labelminute", &minute), ("second", "labelsecond", &second)] {
                        let ek = alloc::format!("{ytype}end{part}");
                        if b.has(&ek) && src.as_deref().map(|s| b.gs(s)).unwrap_or_default() != b.gs(&ek) {
                            let v = alloc::format!("{}\\bibdatedash {}", b.gs(lab), b.gs(&ek));
                            b.set_field(lab, Val::s(v));
                        }
                    }
                }
            }
            Some(LabelDate::Str(s)) => {
                if truthy(&s) {
                    be.borrow_mut().set_field("labeldatesource", Val::s(s));
                }
            }
            None => {}
        }
    }
}

/// Biber::Entry `set_inherit_from` (sets inherit from their first member).
pub fn set_inherit_from(e: &mut Entry, parent: &Entry, dm: &DataModel) {
    for f in parent.datafields() {
        if e.exists(&f) || fc(&f) == "annotation" || fc(&f) == "shorthand" {
            continue;
        }
        if let Some(v) = parent.get(&f) {
            e.set_data(&f, v);
        }
    }
    for df in &dm.helpers.datefields {
        let d = df.strip_suffix("date").unwrap_or(df);
        let k = alloc::format!("{d}datesplit");
        if let Some(v) = parent.get(&k).filter(|v| v.truthy()) {
            e.set_field(&k, v);
        }
    }
}

/// Utils `merge_entry_options`.
pub fn merge_entry_options(opts: Option<Vec<String>>, over: Option<Vec<String>>) -> Vec<String> {
    let Some(over) = over else { return opts.unwrap_or_default() };
    let Some(opts) = opts else { return over };
    let split = |s: &str| -> (String, String, String) {
        match s.find('=') {
            Some(i) => (s[..i].into(), "=".into(), s[i + 1..].into()),
            None => (s.into(), String::new(), String::new()),
        }
    };
    let mut merged = Vec::new();
    let mut used = Vec::new();
    for ov in &opts {
        let (o, e, v) = split(ov);
        let mut or = false;
        for oov in &over {
            let (oo, eo, vo) = split(oov);
            if o == oo {
                or = true;
                let x = alloc::format!("{oo}{eo}{vo}");
                merged.push(x.clone());
                used.push(x);
                break;
            }
        }
        if !or {
            merged.push(alloc::format!("{o}{e}{v}"));
        }
    }
    for oov in over {
        if !used.contains(&oov) {
            merged.push(oov);
        }
    }
    merged
}
