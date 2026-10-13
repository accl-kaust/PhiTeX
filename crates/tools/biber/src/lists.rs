//! Biber.pm `process_lists` and the per-list passes: name
//! disambiguation (uniquename / uniquelist), visible names, hashes,
//! labels, extra* tracking, filters, sorting and context data
//! (with the Biber::DataList state they use).

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::biber::{Biber, Fatal};
use crate::collate::Collator;
use crate::model::{numify, truthy, LabelDate, Val};
use crate::section::{DataList, DisStep, FieldV, Filter};
use crate::unicode::{fc, lc};

const SEP: &str = "\u{10FFFD}";

fn uv(v: Option<String>) -> String {
    v.unwrap_or_default()
}

impl DataList {
    fn ulc_key(nl: &[String]) -> String {
        nl.join(SEP)
    }
    /// `set_uniquelist`.
    fn set_uniquelist_v(&mut self, nlid: &str, num_names: usize, namelist: &[String], labelyear: &str, ul: &str, maxcn: i64, mincn: i64) {
        let mut u = namelist.len() as i64;
        let cur = self.uniquelist(nlid);
        if cur.map(|c| c as i64) != Some(u) {
            self.unul_changed = true;
        }
        if u <= 1 {
            return;
        }
        if num_names as i64 <= maxcn {
            return;
        }
        if u <= mincn && mincn != num_names as i64 {
            return;
        }
        let k = Self::ulc_key(namelist);
        if self.state.ulcount_final.get(&k).copied().unwrap_or(0) > 1 {
            let Some(index) = self.namelist_differs_index(namelist) else { return };
            if index == 0 {
                return;
            }
            u = index as i64 + 1;
        } else if num_names as i64 > u && !self.namelist_differs_nth(namelist, u as usize, ul, labelyear) {
            u -= 1;
        }
        self.ld(nlid).ul = Some(u as usize);
    }

    fn final_keys(&self) -> Vec<String> {
        let mut v: Vec<String> = self.state.ulcount_final.keys().cloned().collect();
        v.extend(self.state.ulcount_final_year.keys().cloned());
        v
    }

    fn namelist_differs_index(&self, list: &[String]) -> Option<usize> {
        let mut index: Option<usize> = None;
        for ls in self.final_keys() {
            let l: Vec<&str> = ls.split(SEP).collect();
            if l.len() == list.len() && l.iter().zip(list).all(|(a, b)| *a == b) {
                continue;
            }
            for (i, x) in list.iter().enumerate() {
                if l.get(i).is_some_and(|y| y == x) {
                    if index.is_none_or(|ix| i > ix) {
                        index = Some(i);
                    }
                } else {
                    break;
                }
            }
        }
        let ix = index?;
        if ix + 1 == list.len() {
            Some(ix)
        } else {
            Some(ix + 1)
        }
    }

    fn namelist_differs_nth(&self, list: &[String], n: usize, ul: &str, labelyear: &str) -> bool {
        let keys: Vec<String> = if ul == "minyear" {
            self.state.ulcount_final_year.get(labelyear).map(|m| m.keys().cloned().collect()).unwrap_or_default()
        } else {
            self.final_keys()
        };
        for ls in keys {
            let l: Vec<String> = ls.split(SEP).map(String::from).collect();
            if l.len() < list.len() {
                continue;
            }
            if list.get(n - 1) == l.get(n - 1) {
                continue;
            }
            let a = &list[..(n - 1).min(list.len())];
            let b = &l[..(n - 1).min(l.len())];
            if a != b {
                continue;
            }
            return true;
        }
        false
    }
}

fn unsummary(un: &DisStep) -> u8 {
    if un.0 == "base" {
        return 0;
    }
    match un.1.first().map(String::as_str) {
        Some("none") => 0,
        Some("init") => 1,
        Some("full" | "fullonly") => 2,
        _ => 0,
    }
}

pub fn get_unsummary(dl: &DataList, nlid: &str, nid: &str) -> Option<(u8, String)> {
    let un = dl.get_nd(nlid, nid)?.un.clone()?;
    Some((unsummary(&un), un.0))
}

impl Biber<'_> {
    pub fn process_lists(&mut self) -> Result<(), Fatal> {
        let secnum = self.cur.clone();
        for li in 0..self.lists.len() {
            if self.lists[li].section != secnum {
                continue;
            }
            let mut dl = core::mem::take(&mut self.lists[li]);
            let r = self.process_list(&mut dl);
            self.lists[li] = dl;
            r?;
        }
        Ok(())
    }

    fn process_list(&mut self, dl: &mut DataList) -> Result<(), Fatal> {
        dl.state = Default::default();
        for f in [
            &mut dl.sortingnamekeytemplatename,
            &mut dl.uniquenametemplatename,
            &mut dl.labelalphanametemplatename,
            &mut dl.namehashtemplatename,
        ] {
            if f.is_empty() {
                *f = "global".into();
            }
        }
        dl.keys = self.secr().citekeys.clone();
        dl.unul_changed = true;
        self.entries_pre(dl)?;
        self.uniqueness(dl);
        self.visible_names(dl);
        self.entries_post(dl)?;
        self.entries_final(dl);
        if !dl.filters.is_empty() {
            let mut fl: Vec<String> = Vec::new();
            'k: for k in dl.keys.clone() {
                let be = self.be(&k);
                for f in &dl.filters {
                    match f {
                        Filter::Or(v) => {
                            if !v.iter().any(|(t, x)| check_list_filter(t, x, &be.borrow())) {
                                continue 'k;
                            }
                        }
                        Filter::One(t, x) => {
                            if !check_list_filter(t, x, &be.borrow()) {
                                continue 'k;
                            }
                        }
                    }
                }
                fl.push(k);
            }
            let any_set = fl.iter().any(|k| self.be(k).borrow().etype() == "set");
            for k in dl.keys.clone() {
                if any_set && self.be(&k).borrow().get("entryset").is_some_and(|v| v.truthy()) {
                    fl.push(k);
                }
            }
            let mut u = Vec::new();
            for k in fl {
                if !u.contains(&k) {
                    u.push(k);
                }
            }
            dl.keys = u;
        }
        // generate_sortdataschema
        let st = self
            .cfg
            .sortingtemplate
            .get(&dl.sortingtemplatename)
            .filter(|t| !t.spec.is_empty())
            .cloned()
            .or_else(|| self.cfg.sortingtemplate.get(&self.cfg.blx_s(None, "sortingtemplatename", None, None)).cloned())
            .unwrap_or_default();
        dl.sortingtemplate = st.clone();
        dl.sortdataschema = st.spec.iter().map(|s| (s.descending, s.items.first().is_some_and(|i| self.dm.sort_is_int(&i.0)))).collect();
        for k in dl.keys.clone() {
            self.generatesortinfo(&k, dl);
        }
        self.sort_list(dl)?;
        self.contextdata(dl);
        Ok(())
    }

    fn entries_pre(&mut self, dl: &mut DataList) -> Result<(), Fatal> {
        for k in self.secr().citekeys.clone() {
            for d in self.process_namedis(&k, dl)? {
                if d.nameun == "false" && d.namelistul.as_deref() == Some("false") {
                    continue;
                }
                let nd = dl.nd(&d.nlid, &d.nid);
                nd.namestring = Some(d.namestring);
                nd.namestrings = d.namestrings.clone();
                for (i, se) in d.schema.iter().enumerate() {
                    if se.0 == "base" {
                        nd.basenamestring = d.namestrings.get(i).cloned();
                        nd.basenamestringparts = se.1.clone();
                        break;
                    }
                }
                nd.namedisschema = d.schema;
            }
        }
        Ok(())
    }

    fn opt_un(&self, k: &str, bee: &str) -> String {
        self.cfg.blx_s(Some(&self.cur), "uniquename", Some(bee), Some(k))
    }

    fn uniqueness(&mut self, dl: &mut DataList) {
        let mut first_ul = true;
        loop {
            if dl.unul_changed {
                dl.unul_changed = false;
                self.create_uniquename_info(dl);
                self.generate_uniquename(dl);
            } else {
                break;
            }
            if first_ul || dl.unul_changed {
                dl.unul_changed = false;
                first_ul = false;
                self.create_uniquelist_info(dl);
                self.generate_uniquelist(dl);
            } else {
                break;
            }
        }
    }

    fn create_uniquename_info(&mut self, dl: &mut DataList) {
        let secnum = self.cur.clone();
        dl.state.uniquenamecount.clear();
        dl.state.uniquenamecount_all.clear();
        'main: for k in self.secr().citekeys.clone() {
            let be = self.be(&k);
            let bee = be.borrow().etype();
            let Some(lni) = be.borrow().labelname.clone() else { continue };
            let Some(nl) = be.borrow().get(&lni).and_then(|v| v.as_names()) else { continue };
            let nl = nl.borrow();
            let nlid = nl.id.clone();
            let mut un = self.opt_un(&k, &bee);
            if let Some(x) = nl.opt("uniquename") {
                un = x;
            }
            let ul = dl.uniquelist(&nlid);
            let maxcn = self.cfg.blx_n(Some(&secnum), "maxcitenames", Some(&bee), Some(&k));
            let mincn = self.cfg.blx_n(Some(&secnum), "mincitenames", Some(&bee), Some(&k));
            let num = nl.count() as i64;
            let more = nl.morenames;
            let mut trunc = BTreeMap::new();
            let mut basenames = Vec::new();
            let mut allnames = Vec::new();
            let minf = |u: &str| matches!(u, "mininit" | "minfull" | "minyearinit" | "minyearfull");
            for n in &nl.names {
                let n = n.borrow();
                if let Some(x) = n.opt("uniquename") {
                    un = x;
                }
                if un == "false" {
                    continue 'main;
                }
                let idx = n.index as i64;
                if matches!(un.as_str(), "allinit" | "allfull" | "minyearinit" | "minyearfull")
                    || ul.is_some_and(|u| u > 0 && idx <= u as i64)
                    || more
                    || num <= maxcn
                    || idx <= mincn
                {
                    trunc.insert(n.id.clone(), true);
                    if minf(&un) {
                        let d = dl.get_nd(&nlid, &n.id);
                        if let Some(b) = d.and_then(|d| d.basenamestring.clone()) {
                            basenames.push(b);
                        } else {
                            basenames.push(String::new());
                        }
                        allnames.push(uv(d.and_then(|d| d.namestring.clone())));
                    }
                }
            }
            let mut min_base = String::new();
            let mut min_ns = String::new();
            if minf(&un) {
                min_base = basenames.join(SEP);
                min_ns = allnames.join(SEP);
                if (basenames.len() as i64) < num || more {
                    min_base.push_str(&alloc::format!("{SEP}et al"));
                    min_ns.push_str(&alloc::format!("{SEP}et al"));
                }
            }
            let lyear = be.borrow().get("labelyear").map(|v| v.str()).unwrap_or_default();
            for n in &nl.names {
                let nid = n.borrow().id.clone();
                let nss = dl.get_nd(&nlid, &nid).map(|d| d.namestrings.clone()).unwrap_or_default();
                let (scope, nskey) = match un.as_str() {
                    "init" | "full" | "allinit" | "allfull" => ("global".to_string(), nss.join(SEP)),
                    "mininit" | "minfull" => {
                        dl.nd(&nlid, &nid).unmininfo = Some(min_base.clone());
                        (min_base.clone(), min_ns.clone())
                    }
                    "minyearinit" | "minyearfull" => {
                        dl.nd(&nlid, &nid).unmininfo = Some(alloc::format!("{min_base}{lyear}"));
                        (alloc::format!("{min_base}{lyear}"), alloc::format!("{min_ns}{lyear}"))
                    }
                    _ => (String::new(), String::new()),
                };
                if trunc.contains_key(&nid) {
                    for ns in &nss {
                        dl.state.uniquenamecount.entry(ns.clone()).or_default().entry(scope.clone()).or_default().insert(nskey.clone());
                    }
                }
                let mut eul = self.cfg.blx_s(Some(&secnum), "uniquelist", Some(&bee), Some(&k));
                if let Some(x) = nl.opt("uniquelist").filter(|s| truthy(s)) {
                    eul = x;
                }
                if eul != "false" {
                    for ns in &nss {
                        dl.state.uniquenamecount_all.entry(ns.clone()).or_default().entry(scope.clone()).or_default().insert(nskey.clone());
                    }
                }
            }
        }
    }

    fn generate_uniquename(&mut self, dl: &mut DataList) {
        let secnum = self.cur.clone();
        'main: for k in self.secr().citekeys.clone() {
            let be = self.be(&k);
            let bee = be.borrow().etype();
            let Some(lni) = be.borrow().labelname.clone() else { continue };
            let Some(nl) = be.borrow().get(&lni).and_then(|v| v.as_names()) else { continue };
            let nl = nl.borrow();
            let nlid = nl.id.clone();
            let mut un = self.opt_un(&k, &bee);
            if let Some(x) = nl.opt("uniquename") {
                un = x;
            }
            let ul = dl.uniquelist(&nlid);
            let maxcn = self.cfg.blx_n(Some(&secnum), "maxcitenames", Some(&bee), Some(&k));
            let mincn = self.cfg.blx_n(Some(&secnum), "mincitenames", Some(&bee), Some(&k));
            let num = nl.count() as i64;
            let more = nl.morenames;
            let mut trunc = BTreeMap::new();
            for n in &nl.names {
                let n = n.borrow();
                if let Some(x) = n.opt("uniquename") {
                    un = x;
                }
                if un == "false" {
                    continue 'main;
                }
                let idx = n.index as i64;
                if matches!(un.as_str(), "allinit" | "allfull") || ul.is_some_and(|u| u > 0 && idx <= u as i64) || more || num <= maxcn || idx <= mincn {
                    trunc.insert(n.id.clone(), true);
                } else {
                    let nd = dl.nd(&nlid, &n.id);
                    nd.un = Some(("base".into(), nd.basenamestringparts.clone()));
                }
            }
            for n in &nl.names {
                let nid = n.borrow().id.clone();
                let d = dl.get_nd(&nlid, &nid).cloned().unwrap_or_default();
                let scope = if matches!(un.as_str(), "mininit" | "minfull" | "minyearinit" | "minyearfull") {
                    uv(d.unmininfo.clone())
                } else {
                    "global".into()
                };
                let count = |m: &BTreeMap<String, BTreeMap<String, alloc::collections::BTreeSet<String>>>, ns: &str| {
                    m.get(ns).and_then(|x| x.get(&scope)).map_or(0, alloc::collections::BTreeSet::len)
                };
                if trunc.contains_key(&nid) {
                    for (i, ns) in d.namestrings.iter().enumerate() {
                        if count(&dl.state.uniquenamecount, ns) == 1 {
                            if let Some(s) = d.namedisschema.get(i) {
                                dl.set_uniquename(&nlid, &nid, s.clone());
                            }
                            break;
                        }
                    }
                    if dl.get_nd(&nlid, &nid).and_then(|x| x.un.clone()).is_none() {
                        if let Some(s) = d.namedisschema.first() {
                            dl.set_uniquename(&nlid, &nid, s.clone());
                        }
                    }
                }
                let mut eul = self.cfg.blx_s(Some(&secnum), "uniquelist", Some(&bee), Some(&k));
                if let Some(x) = nl.opt("uniquelist") {
                    eul = x;
                }
                if eul != "false" {
                    for (i, ns) in d.namestrings.iter().enumerate() {
                        if count(&dl.state.uniquenamecount_all, ns) == 1 {
                            dl.nd(&nlid, &nid).unall = d.namedisschema.get(i).cloned();
                            break;
                        }
                    }
                    if dl.get_nd(&nlid, &nid).and_then(|x| x.unall.clone()).is_none() {
                        dl.nd(&nlid, &nid).unall = d.namedisschema.first().cloned();
                    }
                }
            }
        }
    }

    fn ul_namelist_item(dl: &DataList, nlid: &str, nid: &str) -> Vec<String> {
        let d = dl.get_nd(nlid, nid).cloned().unwrap_or_default();
        let mut out = Vec::new();
        match &d.unall {
            None => out.extend(d.basenamestring.clone()),
            Some(u) if u.0 == "base" => out.extend(d.basenamestring.clone()),
            Some(u) => {
                for (i, s) in d.namedisschema.iter().enumerate() {
                    if s == u {
                        if let Some(x) = d.namestrings.get(i) {
                            out.push(x.clone());
                        }
                    }
                }
            }
        }
        out
    }

    fn create_uniquelist_info(&mut self, dl: &mut DataList) {
        let secnum = self.cur.clone();
        dl.state.ulcount.clear();
        dl.state.ulcount_final.clear();
        dl.state.ulcount_final_year.clear();
        dl.state.ulcount_minyear.clear();
        for k in self.secr().citekeys.clone() {
            let be = self.be(&k);
            let bee = be.borrow().etype();
            let maxcn = self.cfg.blx_n(Some(&secnum), "maxcitenames", Some(&bee), Some(&k));
            let mincn = self.cfg.blx_n(Some(&secnum), "mincitenames", Some(&bee), Some(&k));
            let Some(lni) = be.borrow().labelname.clone() else { continue };
            let Some(nl) = be.borrow().get(&lni).and_then(|v| v.as_names()) else { continue };
            let nl = nl.borrow();
            let labelyear = be.borrow().get("labelyear").map(|v| v.str());
            let mut ul = self.cfg.blx_s(Some(&secnum), "uniquelist", Some(&bee), Some(&k));
            if let Some(x) = nl.opt("uniquelist") {
                ul = x;
            }
            if ul == "false" {
                continue;
            }
            let num = nl.count() as i64;
            let mut namelist: Vec<String> = Vec::new();
            let mut my: Vec<String> = Vec::new();
            for n in &nl.names {
                let n = n.borrow();
                let flag = ul == "minyear" && num > maxcn && (n.index as i64) <= mincn;
                let items = Self::ul_namelist_item(dl, &nl.id, &n.id);
                if flag {
                    let d = dl.get_nd(&nl.id, &n.id).cloned().unwrap_or_default();
                    match &d.unall {
                        None => my.push(uv(d.basenamestring.clone())),
                        Some(u) if u.0 == "base" => my.push(uv(d.basenamestring.clone())),
                        _ => my.extend(items.iter().cloned()),
                    }
                }
                namelist.extend(items);
                *dl.state.ulcount.entry(DataList::ulc_key_pub(&namelist)).or_insert(0) += 1;
            }
            *dl.state.ulcount_final.entry(namelist.join(SEP)).or_insert(0) += 1;
            if ul == "minyear" {
                *dl.state.ulcount_final.entry(namelist.join(SEP)).or_insert(0) += 1;
                if let Some(y) = labelyear.as_ref().filter(|s| truthy(s)) {
                    *dl.state.ulcount_final_year.entry(y.clone()).or_default().entry(namelist.join(SEP)).or_insert(0) += 1;
                }
            }
            if !my.is_empty() {
                dl.state
                    .ulcount_minyear
                    .entry(my.join(SEP))
                    .or_default()
                    .entry(labelyear.clone().unwrap_or_else(|| "0".into()))
                    .or_default()
                    .insert(namelist.join(SEP));
            }
        }
    }

    fn generate_uniquelist(&mut self, dl: &mut DataList) {
        let secnum = self.cur.clone();
        'main: for k in self.secr().citekeys.clone() {
            let be = self.be(&k);
            let bee = be.borrow().etype();
            let labelyear = be.borrow().get("labelyear").map(|v| v.str());
            let maxcn = self.cfg.blx_n(Some(&secnum), "maxcitenames", Some(&bee), Some(&k));
            let mincn = self.cfg.blx_n(Some(&secnum), "mincitenames", Some(&bee), Some(&k));
            let Some(lni) = be.borrow().labelname.clone() else { continue };
            let Some(nl) = be.borrow().get(&lni).and_then(|v| v.as_names()) else { continue };
            let nl = nl.borrow();
            let mut ul = self.cfg.blx_s(Some(&secnum), "uniquelist", Some(&bee), Some(&k));
            if let Some(x) = nl.opt("uniquelist") {
                ul = x;
            }
            if ul == "false" {
                continue;
            }
            let num = nl.count() as i64;
            let mut namelist: Vec<String> = Vec::new();
            for n in &nl.names {
                let n = n.borrow();
                namelist.extend(Self::ul_namelist_item(dl, &nl.id, &n.id));
                if ul == "minyear" && num > maxcn && (n.index as i64) <= mincn {
                    let c = dl
                        .state
                        .ulcount_minyear
                        .get(&namelist.join(SEP))
                        .and_then(|m| m.get(&labelyear.clone().unwrap_or_else(|| "0".into())))
                        .map_or(0, alloc::collections::BTreeSet::len);
                    if c == 1 {
                        continue 'main;
                    }
                }
                if dl.state.ulcount.get(&namelist.join(SEP)).copied() == Some(1) {
                    break;
                }
            }
            dl.set_uniquelist_v(&nl.id, nl.count(), &namelist, &labelyear.unwrap_or_default(), &ul, maxcn, mincn);
        }
    }

    fn visible_names(&mut self, dl: &mut DataList) {
        let secnum = self.cur.clone();
        for k in self.secr().citekeys.clone() {
            let be = self.be(&k);
            let bee = be.borrow().etype();
            let g = |o: &str| self.cfg.blx_n(Some(&secnum), o, Some(&bee), Some(&k));
            let (maxcn, mincn, maxbn, minbn, maxsn, minsn, maxan, minan) = (
                g("maxcitenames"),
                g("mincitenames"),
                g("maxbibnames"),
                g("minbibnames"),
                g("maxsortnames"),
                g("minsortnames"),
                g("maxalphanames"),
                g("minalphanames"),
            );
            let pluralothers = self.cfg.blx_b(None, "pluralothers", None, None);
            for f in self.dm.helpers.namelistsall.clone() {
                let Some(nl) = be.borrow().get(&f).and_then(|v| v.as_names()) else { continue };
                let nl = nl.borrow();
                let c = nl.count() as i64;
                let ulv = dl.uniquelist(&nl.id).map(|x| x as i64);
                let va = if c > maxan { c.min(minan) } else { c };
                let mut vc = if c > maxcn { ulv.unwrap_or(c.min(mincn)) } else { c };
                if pluralothers && c - vc == 1 {
                    vc = c;
                }
                let vb = if c > maxbn { ulv.unwrap_or(c.min(minbn)) } else { c };
                let vs = if c > maxsn { ulv.unwrap_or(c.min(minsn)) } else { c };
                let ld = dl.ld(&nl.id);
                ld.viscite = vc.max(0) as usize;
                ld.visbib = vb.max(0) as usize;
                ld.vissort = vs.max(0) as usize;
                ld.visalpha = va.max(0) as usize;
            }
        }
    }

    fn entries_post(&mut self, dl: &mut DataList) -> Result<(), Fatal> {
        let secnum = self.cur.clone();
        for k in self.secr().citekeys.clone() {
            let be = self.be(&k);
            let bee = be.borrow().etype();
            let skiplab = self.cfg.blx_b(Some(&secnum), "skiplab", Some(&bee), Some(&k));
            let nl_of = |f: &str| be.borrow().get(f).and_then(|v| v.as_names());
            let lni = be.borrow().labelname.clone();
            let lti = be.borrow().labeltitle.clone();
            // fullhash
            if let Some(f) = be.borrow().labelnamefh.clone() {
                if let Some(n) = nl_of(&f) {
                    let h = self.getfullhash(&k, &n.borrow(), dl)?;
                    dl.set_entryfield(&k, "fullhash", FieldV::S(h));
                    dl.set_entryfield(&k, "fullhashraw", FieldV::S(self.getfullhashraw(&n.borrow())));
                }
            }
            for f in self.dm.helpers.namelistsall.clone() {
                if let Some(n) = nl_of(&f) {
                    let h = self.getfullhash(&k, &n.borrow(), dl)?;
                    dl.set_entryfield(&k, &alloc::format!("{f}fullhash"), FieldV::S(h));
                    dl.set_entryfield(&k, &alloc::format!("{f}fullhashraw"), FieldV::S(self.getfullhashraw(&n.borrow())));
                }
            }
            // labelalpha
            if !skiplab && self.cfg.blx_b(None, "labelalpha", Some(&bee), Some(&k)) {
                let (l, s) = self.genlabel(&k, dl);
                dl.set_entryfield(&k, "labelalpha", FieldV::S(l));
                dl.set_entryfield(&k, "sortlabelalpha", FieldV::S(s));
            }
            // extraalpha
            if self.cfg.blx_b(None, "labelalpha", Some(&bee), Some(&k)) {
                if let Some(la) = dl.entryfield_s(&k, "labelalpha").filter(|s| truthy(s)) {
                    *dl.state.ladisambiguation.entry(la).or_insert(0) += 1;
                }
            }
            // extradate
            if self.cfg.blx_b(None, "labeldateparts", Some(&bee), Some(&k)) && !skiplab {
                let mut ch = String::new();
                let edc = self.cfg.blx(None, "extradatecontext", None, None).map(|v| v.items()).unwrap_or_default();
                for f in edc {
                    let mut fc_ = f.text.clone();
                    if fc_.starts_with("label") && fc_.len() > 5 {
                        fc_ = match fc_.as_str() {
                            "labelname" => uv(lni.clone()),
                            "labeltitle" => uv(lti.clone()),
                            "labelnamefh" => uv(be.borrow().labelnamefh.clone()),
                            o => return Err(self.unsupported(&alloc::format!("extradatecontext field '{o}'"))),
                        };
                    }
                    let fv = be.borrow().get(&fc_).filter(Val::truthy);
                    if let Some(fv) = fv {
                        if self.dm.datatype(&fc_) == "name" {
                            if let Some(n) = fv.as_names() {
                                ch = self.getnamehash_u(&k, &n.borrow(), dl)?;
                            }
                        } else if self.dm.fieldtype(&fc_) == "list" {
                            let j = fv.as_list().map(|l| l.borrow().concat()).unwrap_or_default();
                            ch = crate::internals::hash_str(&j);
                        } else {
                            ch = crate::internals::hash_str(&fv.str());
                        }
                        break;
                    }
                }
                let mut ds = String::new();
                let mut edscope: Option<String> = None;
                for scope in self.cfg.extradatespec.clone() {
                    for f in scope {
                        if let Some(v) = be.borrow().get(&f) {
                            ds.push_str(&v.str());
                            edscope = Some(f);
                            break;
                        }
                    }
                }
                match edscope {
                    Some(s) => be.borrow_mut().set_field("extradatescope", Val::s(s)),
                    None => {
                        be.borrow_mut().derived.remove("extradatescope");
                    }
                }
                dl.set_entryfield(&k, "nametitledateparts", FieldV::S(alloc::format!("{ch},{ds}")));
                let t = alloc::format!("{ch},{ds}");
                let e = dl.state.seen_nametitledateparts.get(&t).copied();
                if e.is_none() || truthy(&ch) {
                    *dl.state.seen_nametitledateparts.entry(t).or_insert(0) += 1;
                }
            }
            // extraname
            if !skiplab {
                if let Some(l) = lni.clone() {
                    if let Some(n) = nl_of(&l) {
                        let h = self.getnamehash_u(&k, &n.borrow(), dl)?;
                        dl.set_entryfield(&k, "labelnamehash", FieldV::S(h.clone()));
                        *dl.state.seen_labelname.entry(h).or_insert(0) += 1;
                    }
                }
            }
            // extratitle
            if self.cfg.blx_b(None, "labeltitle", Some(&bee), None) && !skiplab {
                let mut nh = String::new();
                if let Some(l) = lni.clone() {
                    if let Some(n) = nl_of(&l) {
                        nh = self.getnamehash_u(&k, &n.borrow(), dl)?;
                    }
                }
                let ts = lti.as_deref().and_then(|t| be.borrow().get(t)).map(|v| v.str()).unwrap_or_default();
                let t = alloc::format!("{nh},{ts}");
                dl.set_entryfield(&k, "nametitle", FieldV::S(t.clone()));
                if dl.state.seen_nametitle.get(&t).copied().unwrap_or(0) == 0 || truthy(&ts) {
                    *dl.state.seen_nametitle.entry(t).or_insert(0) += 1;
                }
            }
            // extratitleyear
            if self.cfg.blx_b(None, "labeltitleyear", Some(&bee), Some(&k)) && !skiplab {
                let ts = lti.as_deref().and_then(|t| be.borrow().get(t)).map(|v| v.str()).unwrap_or_default();
                let ys = [be.borrow().get("labelyear"), be.borrow().get("year")]
                    .into_iter()
                    .flatten()
                    .map(|v| v.str())
                    .find(|s| truthy(s))
                    .unwrap_or_default();
                let t = alloc::format!("{ts},{ys}");
                dl.set_entryfield(&k, "titleyear", FieldV::S(t.clone()));
                if dl.state.seen_titleyear.get(&t).copied().unwrap_or(0) == 0 || truthy(&ts) {
                    *dl.state.seen_titleyear.entry(t).or_insert(0) += 1;
                }
            }
            // workuniqueness
            self.workuniqueness(&k, dl)?;
            // namehash
            if let Some(l) = lni.clone() {
                if let Some(n) = nl_of(&l) {
                    let a = self.getnamehash(&k, &n.borrow(), dl, false)?;
                    let b = self.getnamehash(&k, &n.borrow(), dl, true)?;
                    dl.set_entryfield(&k, "namehash", FieldV::S(a));
                    dl.set_entryfield(&k, "bibnamehash", FieldV::S(b));
                }
            }
            for f in self.dm.helpers.namelistsall.clone() {
                if let Some(n) = nl_of(&f) {
                    let a = self.getnamehash(&k, &n.borrow(), dl, false)?;
                    let b = self.getnamehash(&k, &n.borrow(), dl, true)?;
                    dl.set_entryfield(&k, &alloc::format!("{f}namehash"), FieldV::S(a));
                    dl.set_entryfield(&k, &alloc::format!("{f}bibnamehash"), FieldV::S(b));
                }
            }
            // pername hashes
            for f in self.dm.helpers.namelistsall.clone() {
                if let Some(nl) = nl_of(&f) {
                    let names = nl.borrow().names.clone();
                    for n in names {
                        let h = self.genpnhash(&k, &nl.borrow(), &n.borrow(), dl)?;
                        n.borrow_mut().hash = Some(h.clone());
                        let id = n.borrow().id.clone();
                        dl.nd(&nl.borrow().id, &id).hash = Some(h);
                    }
                }
            }
            // uniqueprimaryauthor
            if let Some(l) = lni.clone() {
                if self.cfg.blx_b(None, "uniqueprimaryauthor", Some(&bee), Some(&k)) {
                    if let Some(nl) = nl_of(&l) {
                        let nl = nl.borrow();
                        if let Some(n1) = nl.names.first() {
                            let n1 = n1.borrow();
                            let dis = self.process_namedis(&k, dl)?;
                            let mut pabase = None;
                            if let Some(d) = dis.iter().find(|d| d.nlid == nl.id && d.nid == n1.id) {
                                for (i, se) in d.schema.iter().enumerate() {
                                    if se.0 == "base" {
                                        pabase = d.namestrings.get(i).cloned();
                                    }
                                }
                            }
                            let pb = uv(pabase);
                            dl.set_entryfield(&k, "seenprimaryauthor", FieldV::S(pb.clone()));
                            dl.state.seenpa.entry(pb).or_default().insert(uv(n1.hash.clone()));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn workuniqueness(&mut self, k: &str, dl: &mut DataList) -> Result<(), Fatal> {
        let be = self.be(k);
        let bee = be.borrow().etype();
        let lni = be.borrow().labelname.clone();
        let lti = be.borrow().labeltitle.clone();
        let ign = self.cfg.uniqignore.get(k).cloned().unwrap_or_default();
        let inign = |set: &str, f: &Option<String>| f.as_ref().is_some_and(|f| ign.get(set).is_some_and(|v| v.iter().any(|x| fc(x) == fc(f))));
        if let Some(o) = be.borrow().get("options").and_then(|v| v.as_list()) {
            if o.borrow().iter().any(|x| x == "skipbib") {
                return Ok(());
            }
        }
        let names = lni.as_deref().and_then(|l| be.borrow().get(l)).and_then(|v| v.as_names());
        let title = lti.as_deref().and_then(|t| be.borrow().get(t)).map(|v| v.str()).unwrap_or_default();
        if lni.is_some() && self.cfg.blx_b(None, "singletitle", Some(&bee), Some(k)) {
            let id = match &names {
                Some(n) => self.getfullhash(k, &n.borrow(), dl)?,
                None => String::new(),
            };
            if !(inign("singletitle", &lni) && inign("singletitle", &lti)) {
                *dl.state.seenname.entry(id.clone()).or_insert(0) += 1;
            }
            dl.set_entryfield(k, "seenname", FieldV::S(id));
        }
        if lti.is_some() && self.cfg.blx_b(None, "uniquetitle", Some(&bee), Some(k)) {
            if !inign("uniquetitle", &lti) {
                *dl.state.seentitle.entry(title.clone()).or_insert(0) += 1;
            }
            dl.set_entryfield(k, "seentitle", FieldV::S(title.clone()));
        }
        if lti.is_some() && lni.is_none() && self.cfg.blx_b(None, "uniquebaretitle", Some(&bee), Some(k)) {
            if !inign("uniquebaretitle", &lti) {
                *dl.state.seenbaretitle.entry(title.clone()).or_insert(0) += 1;
            }
            dl.set_entryfield(k, "seenbaretitle", FieldV::S(title.clone()));
        }
        if lni.is_some() && lti.is_some() && self.cfg.blx_b(None, "uniquework", Some(&bee), Some(k)) {
            let id = match &names {
                Some(n) => self.getfullhash(k, &n.borrow(), dl)?,
                None => String::new(),
            } + &title;
            if !(inign("uniquework", &lni) && inign("uniquework", &lti)) {
                *dl.state.seenwork.entry(id.clone()).or_insert(0) += 1;
            }
            dl.set_entryfield(k, "seenwork", FieldV::S(id));
        }
        Ok(())
    }

    fn entries_final(&mut self, dl: &mut DataList) {
        for k in self.secr().citekeys.clone() {
            let bee = self.be(&k).borrow().etype();
            let b = |o: &str| self.cfg.blx_b(None, o, Some(&bee), Some(&k));
            let seen = |m: &BTreeMap<String, usize>, f: &str| dl.entryfield_s(&k, f).map(|x| m.get(&x).copied().unwrap_or(0));
            let mut set = Vec::new();
            if b("singletitle") && seen(&dl.state.seenname, "seenname").is_some_and(|c| c < 2) {
                set.push("singletitle");
            }
            if b("uniquetitle") && seen(&dl.state.seentitle, "seentitle").is_some_and(|c| c < 2) {
                set.push("uniquetitle");
            }
            if b("uniquebaretitle") && seen(&dl.state.seenbaretitle, "seenbaretitle").is_some_and(|c| c < 2) {
                set.push("uniquebaretitle");
            }
            if b("uniquework") && dl.entryfield_s(&k, "seenwork").is_some_and(|s| truthy(&s)) && seen(&dl.state.seenwork, "seenwork").is_some_and(|c| c < 2) {
                set.push("uniquework");
            }
            if b("uniqueprimaryauthor") {
                if let Some(pa) = dl.entryfield_s(&k, "seenprimaryauthor").filter(|s| truthy(s)) {
                    if dl.state.seenpa.get(&pa).map_or(0, alloc::collections::BTreeSet::len) < 2 {
                        set.push("uniqueprimaryauthor");
                    }
                }
            }
            for f in set {
                dl.set_entryfield(&k, f, FieldV::S("1".into()));
            }
        }
    }

    fn sort_list(&mut self, dl: &mut DataList) -> Result<(), Fatal> {
        let st = dl.sortingtemplate.clone();
        let loc = if truthy(&st.locale) { st.locale.clone() } else { self.cfg.blx_s(None, "sortlocale", None, None) };
        let thislocale = {
            let o = self.cfg.opt("sortlocale");
            if truthy(&o) {
                o
            } else {
                crate::bcf::locale2bcp47(&loc)
            }
        };
        self.check_locale(&thislocale)?;
        let sortcase = self.cfg.opt("sortcase") == "1";
        let sortupper = self.cfg.opt("sortupper") == "1";
        let base_level: u8 = if sortcase { 4 } else { 2 };
        let mut colls = Vec::new();
        for set in &st.spec {
            if let Some(l) = set.locale.as_deref().filter(|s| truthy(s)) {
                self.check_locale(&crate::bcf::locale2bcp47(l))?;
            }
            let mut level = base_level;
            let mut ubl = sortupper;
            let mut changed = false;
            if let Some(sc) = set.sortcase.as_deref() {
                let v = numify(sc) != 0.0;
                if v != sortcase {
                    level = if v { 4 } else { 2 };
                    changed = true;
                }
            }
            if let Some(su) = set.sortupper.as_deref() {
                let v = numify(su) != 0.0;
                if v != sortupper {
                    ubl = v;
                    changed = true;
                }
            }
            let _ = changed;
            colls.push(Collator { level, upper_before_lower: ubl, shifted: false, normalize: false });
        }
        enum K {
            I(i64),
            S(Vec<u16>),
        }
        let mut ext: Vec<(String, Vec<K>)> = Vec::new();
        let mut cache: BTreeMap<(usize, String), Vec<u16>> = BTreeMap::new();
        for k in &dl.keys {
            let sd = dl.sortdata.get(k).map(|x| x.1.clone()).unwrap_or_default();
            let mut ks = Vec::new();
            for (i, f) in sd.iter().enumerate() {
                let int = dl.sortdataschema.get(i).is_some_and(|x| x.1);
                if int {
                    ks.push(K::I(if crate::model::looks_like_number(f) { numify(f) as i64 } else { 2_000_000_000 }));
                } else {
                    let c = colls.get(i).copied().unwrap_or(Collator { level: base_level, upper_before_lower: sortupper, shifted: false, normalize: false });
                    let key = cache.entry((i, f.clone())).or_insert_with(|| c.sort_key(&self.table, f)).clone();
                    ks.push(K::S(key));
                }
            }
            ext.push((k.clone(), ks));
        }
        let desc: Vec<bool> = dl.sortdataschema.iter().map(|x| x.0).collect();
        ext.sort_by(|a, b| {
            for (i, (x, y)) in a.1.iter().zip(b.1.iter()).enumerate() {
                let o = match (x, y) {
                    (K::I(p), K::I(q)) => p.cmp(q),
                    (K::S(p), K::S(q)) => crate::collate::cmp_keys(p, q),
                    _ => core::cmp::Ordering::Equal,
                };
                let o = if desc.get(i).copied().unwrap_or(false) { o.reverse() } else { o };
                if o != core::cmp::Ordering::Equal {
                    return o;
                }
            }
            core::cmp::Ordering::Equal
        });
        dl.keys = ext.into_iter().map(|x| x.0).collect();
        Ok(())
    }

    fn check_locale(&mut self, l: &str) -> Result<(), Fatal> {
        let lang = lc(l.split(['-', '_']).next().unwrap_or(""));
        if ["", "en", "de", "fr", "it", "nl", "pt", "id", "ms", "ga", "la"].contains(&lang.as_str()) {
            Ok(())
        } else {
            Err(self.unsupported(&alloc::format!("sorting locale '{l}' (only untailored collation)")))
        }
    }

    fn contextdata(&mut self, dl: &mut DataList) {
        let secnum = self.cur.clone();
        for key in dl.keys.clone() {
            let be = self.be(&key);
            let bee = be.borrow().etype();
            let lni = be.borrow().labelname.clone();
            if bee == "set" {
                let es: Vec<String> = be.borrow().get("entryset").and_then(|v| v.as_list()).map(|l| l.borrow().clone()).unwrap_or_default();
                let es = if self.cfg.blx_b(None, "sortsets", None, None) {
                    let mut idx: Vec<usize> = es.iter().filter_map(|e| dl.keys.iter().position(|k| k == e)).collect();
                    idx.sort_unstable();
                    idx.into_iter().map(|i| dl.keys[i].clone()).collect()
                } else {
                    es
                };
                dl.set_entryfield(&key, "entryset", FieldV::L(es));
            }
            if !self.cfg.blx_b(Some(&secnum), "skiplab", Some(&bee), Some(&key)) {
                if let Some(h) = dl.entryfield_s(&key, "labelnamehash").filter(|s| truthy(s)) {
                    if dl.state.seen_labelname.get(&h).copied().unwrap_or(0) > 1 {
                        let v = dl.state.seen_extraname.entry(h).or_insert(0);
                        *v += 1;
                        let v = *v;
                        dl.state.extranamedata.insert(key.clone(), v);
                    }
                }
                let b = |o: &str| self.cfg.blx_b(None, o, Some(&bee), Some(&key));
                if b("labeldateparts") {
                    let t = dl.entryfield_s(&key, "nametitledateparts").unwrap_or_default();
                    if dl.state.seen_nametitledateparts.get(&t).copied().unwrap_or(0) > 1 {
                        let v = dl.state.seen_extradate.entry(t).or_insert(0);
                        *v += 1;
                        let v = *v;
                        dl.state.extradatedata.insert(key.clone(), v);
                    }
                }
                if b("labeltitle") {
                    let t = dl.entryfield_s(&key, "nametitle").unwrap_or_default();
                    if dl.state.seen_nametitle.get(&t).copied().unwrap_or(0) > 1 {
                        let v = dl.state.seen_extratitle.entry(t).or_insert(0);
                        *v += 1;
                        let v = *v;
                        dl.state.extratitledata.insert(key.clone(), v);
                    }
                }
                if b("labeltitleyear") {
                    let t = dl.entryfield_s(&key, "titleyear").unwrap_or_default();
                    if dl.state.seen_titleyear.get(&t).copied().unwrap_or(0) > 1 {
                        let v = dl.state.seen_extratitleyear.entry(t).or_insert(0);
                        *v += 1;
                        let v = *v;
                        dl.state.extratitleyeardata.insert(key.clone(), v);
                    }
                }
                if b("labelalpha") {
                    let la = dl.entryfield_s(&key, "labelalpha");
                    if let Some(l) = la.clone() {
                        dl.state.labelalphadata.insert(key.clone(), l);
                    }
                    let la = la.unwrap_or_default();
                    if dl.state.ladisambiguation.get(&la).copied().unwrap_or(0) > 1 {
                        let v = dl.state.seen_extraalpha.entry(la).or_insert(0);
                        *v += 1;
                        let v = *v;
                        dl.state.extraalphadata.insert(key.clone(), v);
                    }
                }
            }
            for nf in self.dm.helpers.namelists.clone() {
                if lni.as_deref() != Some(nf.as_str()) {
                    continue;
                }
                let Some(nl) = be.borrow().get(&nf).and_then(|v| v.as_names()) else { continue };
                let nl = nl.borrow();
                for n in &nl.names {
                    let n = n.borrow();
                    let Some(d) = dl.get_nd(&nl.id, &n.id).cloned() else { continue };
                    let Some(un) = d.un.clone() else { continue };
                    let mut pnun: BTreeMap<String, String> = BTreeMap::new();
                    for (i, nss) in d.namedisschema.iter().enumerate() {
                        if *nss == un {
                            let mut dis: Vec<DisStep> = Vec::new();
                            if i > 1 {
                                for x in &d.namedisschema[1..i] {
                                    let lv = x.1.first().map(String::as_str);
                                    if x.0 != "base" && lv != Some("full") {
                                        dis.push(x.clone());
                                    }
                                }
                            }
                            dis.push(d.namedisschema[i].clone());
                            pnun = dis
                                .into_iter()
                                .map(|x| {
                                    let l = if x.0 == "base" { String::from("ARRAY") } else { x.1.first().cloned().unwrap_or_default() };
                                    (x.0, if l == "fullonly" { "full".into() } else { l })
                                })
                                .collect();
                            break;
                        }
                    }
                    for np in n.parts() {
                        let v = match pnun.get(&np).map(String::as_str).unwrap_or("none") {
                            "none" => "0",
                            "init" => "1",
                            "full" => "2",
                            _ => "0",
                        };
                        dl.nd(&nl.id, &n.id).unparts.insert(np, v.to_string());
                    }
                }
            }
        }
    }
}

impl DataList {
    pub fn ulc_key_pub(nl: &[String]) -> String {
        Self::ulc_key(nl)
    }
}

fn check_list_filter(t: &str, fs: &str, be: &crate::model::Entry) -> bool {
    let lfs = lc(fs);
    match t {
        "type" => be.etype() == lfs,
        "nottype" => be.etype() != lfs,
        "subtype" => be.exists("entrysubtype") && be.gs("entrysubtype") == lfs,
        "notsubtype" => !(be.exists("entrysubtype") && be.gs("entrysubtype") == lfs),
        "keyword" => be.has_keyword(fs),
        "notkeyword" => !be.has_keyword(fs),
        "field" => be.exists(fs),
        "notfield" => !be.exists(fs),
        _ => true,
    }
}

