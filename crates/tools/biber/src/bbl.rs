//! Biber::Output::bbl and Output::base: the `.bbl` text of each entry
//! with its list-dependent placeholders, and DataList
//! `instantiate_entry` which fills them per list.

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::biber::Biber;
use crate::collate::Collator;
use crate::internals::{join_name, md5_hex, rangelen};
use crate::model::{looks_like_number, numify, truthy, Entry, Val};
use crate::regex::Regex;
use crate::section::{DataList, FieldV};
use crate::unicode::{lc, nfc};

#[derive(Default, Debug)]
pub struct Output {
    pub entries: BTreeMap<String, BTreeMap<String, String>>,
    pub alias: BTreeMap<String, BTreeMap<String, String>>,
    pub missing: BTreeMap<String, BTreeMap<String, String>>,
    pub tail: String,
}

const HEAD: &str = "% $ biblatex auxiliary file $
% $ biblatex bbl format version 3.3 $
% Do not modify the above lines!
%
% This is an auxiliary file used by the 'biblatex' package.
% This file may safely be deleted. It will be recreated by
% biber as required.
%
\\begingroup
\\makeatletter
\\@ifundefined{ver@biblatex.sty}
  {\\@latex@error
     {Missing 'biblatex' package}
     {The bibliography requires the 'biblatex' package.}
      \\aftergroup\\endinput}
  {}
\\endgroup

";

fn re(p: &str) -> Regex {
    Regex::new(p, "").expect("static pattern")
}

fn rs(s: &str, pat: &str, with: &str) -> String {
    Regex::new(pat, "ms").expect("bbl pattern").replace(s, true, &mut |_, _| with.into())
}

fn esc(s: &str) -> String {
    let mut o = String::new();
    for c in s.chars() {
        if !c.is_alphanumeric() && c != '_' {
            o.push('\\');
        }
        o.push(c);
    }
    o
}

impl Biber<'_> {
    fn printfield(&self, be: &Entry, field: &str, s: &str) -> String {
        let out = self.dm.outcase(field).unwrap_or_else(|| field.to_string());
        if s.is_empty() && !self.dm.nullok(field) {
            return String::new();
        }
        let ft = if lc(field) == "crossref" || lc(field) == "xref" { "strng" } else { "field" };
        let mut s = s.to_string();
        if let Some(p) = field.strip_suffix("year") {
            // m/^(.*)(?!end)year$/ always matches a field ending in "year"
            let y = be.get(&alloc::format!("{p}year")).map(|v| v.str()).unwrap_or_default();
            if truthy(&y) && looks_like_number(&y) {
                s = crate::model::num_str(numify(&y).abs());
            }
        }
        if ft != "strng" && be.gs("datatype") != "bibtex" {
            s = re(r"(?<!\\)(\#|\&|\%)").replace(&s, true, &mut |t, m| alloc::format!("\\{}", crate::regex::cap(t, m, 1).unwrap_or_default()));
        }
        alloc::format!("      \\{ft}{{{out}}}{{{s}}}\n")
    }

    fn name_to_bbl(&self, n: &crate::model::Name, un: &str) -> String {
        let mut pno: Vec<String> = Vec::new();
        let mut nss: Vec<String> = Vec::new();
        let nid = &n.id;
        for np in self.dm.nameparts() {
            let p = n.namepart(&np);
            if truthy(&p) {
                let npc = if n.was_stripped(&np) { alloc::format!("{{{p}}}") } else { join_name(&p) };
                let mut npci = n.initial(&np).join("\\bibinitperiod\\bibinitdelim ") + "\\bibinitperiod";
                npci = re(r"\p{Pd}").replace(&npci, true, &mut |_, _| "\\bibinithyphendelim ".into());
                nss.push(alloc::format!("           {np}={{{npc}}}"));
                nss.push(alloc::format!("           {np}i={{{npci}}}"));
                if un != "false" {
                    nss.push(alloc::format!("           <BDS>UNP-{np}-{nid}</BDS>"));
                }
            }
        }
        if un != "false" {
            pno.push(alloc::format!("<BDS>UNS-{nid}</BDS>"));
            pno.push(alloc::format!("<BDS>UNP-{nid}</BDS>"));
        }
        for (o, v) in &n.opts {
            if self.cfg.optout.get(&("NAME".into(), o.clone())).copied().unwrap_or(false) {
                pno.push(alloc::format!("{o}={}", self.cfg.map_boolean(o, v, false).unwrap_or_default()));
            }
        }
        pno.push(alloc::format!("<BDS>{nid}-PERNAMEHASH</BDS>"));
        alloc::format!("        {{{{{}}}{{%\n{}}}}}%\n", pno.join(","), nss.join(",\n"))
    }

    fn filter_entry_options(&self, key: &str) -> Vec<String> {
        let mut out = Vec::new();
        for opt in self.cfg.entry_options(&self.cur, key) {
            let v = self.cfg.blx(Some(&self.cur), &opt, None, Some(key)).map(|v| v.s()).unwrap_or_default();
            let v = self.cfg.map_boolean(&opt, &v, false).unwrap_or_default();
            if self.cfg.optout.get(&("ENTRY".into(), opt.clone())).copied().unwrap_or(false) {
                out.push(if truthy(&v) { alloc::format!("{opt}={v}") } else { opt.clone() });
            }
        }
        out
    }

    /// Output::bbl `set_output_entry`.
    fn set_output_entry(&mut self, key: &str) {
        let secnum = self.cur.clone();
        let be = self.be(key);
        let b = be.borrow();
        let bee = b.etype();
        if self.dm.entrytype_skipout(&bee) {
            return;
        }
        let outtype = self.dm.outcase(&bee).unwrap_or_else(|| bee.clone());
        let mut un = self.cfg.blx_s(Some(&secnum), "uniquename", Some(&bee), Some(key));
        let mut ul = self.cfg.blx_s(Some(&secnum), "uniquelist", Some(&bee), Some(key));
        let lni = b.labelname.clone();
        let lnl = lni.as_deref().and_then(|l| b.get(l)).and_then(|v| v.as_names());
        if let Some(nl) = &lnl {
            if let Some(x) = nl.borrow().opt("uniquelist").filter(|s| truthy(s)) {
                ul = x;
            }
            if let Some(x) = nl.borrow().opt("uniquename").filter(|s| truthy(s)) {
                un = x;
            }
        }
        let kc = self.secr().citecount(key);
        let blx = |o: &str| self.cfg.blx_b(None, o, Some(&bee), Some(key));
        let mut a = alloc::format!(
            "    \\entry{{{key}}}{{{outtype}}}{{{}}}{{{}}}\n",
            self.filter_entry_options(key).join(","),
            if numify(&kc) == -1.0 { String::new() } else { kc }
        );
        let helpers = self.dm.helpers.clone();
        'body: {
            if bee == "set" {
                a.push_str("      <BDS>ENTRYSET</BDS>\n");
                if blx("labelalpha") {
                    a.push_str("      <BDS>LABELALPHA</BDS>\n      <BDS>EXTRAALPHA</BDS>\n");
                }
                a.push_str("      <BDS>SORTINIT</BDS>\n      <BDS>SORTINITHASH</BDS>\n      <BDS>LABELPREFIX</BDS>\n");
                for f in ["label", "annotation", "shorthand"] {
                    if let Some(v) = b.get(f).filter(Val::truthy) {
                        a.push_str(&alloc::format!("      \\field{{{f}}}{{{}}}\n", v.str()));
                    }
                }
                if let Some(k) = b.get("keywords").and_then(|v| v.as_list()) {
                    a.push_str(&alloc::format!("      \\keyw{{{}}}\n", k.borrow().join(",")));
                }
                break 'body;
            }
            if let Some(es) = b.get("entryset").and_then(|v| v.as_list()) {
                a.push_str(&alloc::format!("      \\inset{{{}}}\n", es.borrow().join(",")));
            }
            for nf in &helpers.namelists {
                let Some(nl) = b.get(nf).and_then(|v| v.as_names()) else { continue };
                let nl = nl.borrow();
                let is_ln = lni.as_deref() == Some(nf.as_str());
                if nl.morenames {
                    a.push_str(&alloc::format!("      \\true{{more{nf}}}\n"));
                    if is_ln {
                        a.push_str("      \\true{morelabelname}\n");
                    }
                }
                if is_ln {
                    if let Some(x) = nl.opt("uniquename") {
                        un = x;
                    }
                }
                let mut nfv = String::new();
                if is_ln {
                    let mut plo = Vec::new();
                    if ul != "false" {
                        plo.push(alloc::format!("<BDS>UL-{}</BDS>", nl.id));
                    }
                    for (o, v) in &nl.opts {
                        if self.cfg.optout.get(&("NAMELIST".into(), o.clone())).copied().unwrap_or(false) {
                            plo.push(alloc::format!("{o}={}", self.cfg.map_boolean(o, v, false).unwrap_or_default()));
                        }
                    }
                    nfv = plo.join(",");
                }
                a.push_str(&alloc::format!("      \\name{{{nf}}}{{{}}}{{{nfv}}}{{%\n", nl.count()));
                for n in &nl.names {
                    a.push_str(&self.name_to_bbl(&n.borrow(), &un));
                }
                a.push_str("      }\n");
            }
            let others = self.cfg.opt("others_string");
            for lf in &helpers.lists {
                let Some(l) = b.get(lf).and_then(|v| v.as_list()) else { continue };
                let mut l = l.borrow_mut();
                if l.last().is_some_and(|x| lc(x) == others) {
                    a.push_str(&alloc::format!("      \\true{{more{lf}}}\n"));
                    l.pop();
                }
                a.push_str(&alloc::format!("      \\list{{{lf}}}{{{}}}{{%\n", l.len()));
                for f in l.iter() {
                    a.push_str(&alloc::format!("        {{{f}}}%\n"));
                }
                a.push_str("      }\n");
            }
            a.push_str("      <BDS>NAMEHASH</BDS>\n      <BDS>FULLHASH</BDS>\n      <BDS>FULLHASHRAW</BDS>\n      <BDS>BIBNAMEHASH</BDS>\n");
            for nf in &helpers.namelists {
                if b.get(nf).is_none() {
                    continue;
                }
                for s in ["BIBNAMEHASH", "NAMEHASH", "FULLHASH", "FULLHASHRAW"] {
                    a.push_str(&alloc::format!("      <BDS>{nf}{s}</BDS>\n"));
                }
            }
            if lni.is_some() {
                a.push_str("      <BDS>EXTRANAME</BDS>\n");
            }
            if blx("labelalpha") {
                a.push_str("      <BDS>LABELALPHA</BDS>\n");
            }
            a.push_str("      <BDS>SORTINIT</BDS>\n      <BDS>SORTINITHASH</BDS>\n");
            if blx("labeldateparts") {
                a.push_str("      <BDS>EXTRADATE</BDS>\n");
                if let Some(s) = b.get("extradatescope").filter(Val::truthy) {
                    a.push_str(&alloc::format!("      \\field{{extradatescope}}{{{}}}\n", s.str()));
                }
                if b.exists("labeldatesource") {
                    a.push_str(&alloc::format!("      \\field{{labeldatesource}}{{{}}}\n", b.gs("labeldatesource")));
                }
            }
            if !b.get("shorthand").is_some_and(|v| v.truthy()) {
                a.push_str("      <BDS>LABELPREFIX</BDS>\n");
            }
            if blx("labeltitle") {
                a.push_str("      <BDS>EXTRATITLE</BDS>\n");
            }
            if blx("labeltitleyear") {
                a.push_str("      <BDS>EXTRATITLEYEAR</BDS>\n");
            }
            if blx("labelalpha") {
                a.push_str("      <BDS>EXTRAALPHA</BDS>\n");
            }
            if b.get("crossrefsource").is_some() {
                a.push_str("      \\true{crossrefsource}\n");
            }
            if b.get("xrefsource").is_some() {
                a.push_str("      \\true{xrefsource}\n");
            }
            a.push_str("      <BDS>SINGLETITLE</BDS>\n      <BDS>UNIQUETITLE</BDS>\n      <BDS>UNIQUEBARETITLE</BDS>\n      <BDS>UNIQUEWORK</BDS>\n      <BDS>UNIQUEPRIMARYAUTHOR</BDS>\n");
            if let Some(l) = &lni {
                a.push_str(&alloc::format!("      \\field{{labelnamesource}}{{{l}}}\n"));
            }
            if let Some(l) = &b.labeltitle {
                a.push_str(&alloc::format!("      \\field{{labeltitlesource}}{{{l}}}\n"));
            }
            if let Some(c) = b.get("clonesourcekey").filter(Val::truthy) {
                a.push_str(&alloc::format!("      \\field{{clonesourcekey}}{{{}}}\n", c.str()));
            }
            for f in &helpers.fields {
                let v = b.get(f).map(|v| v.str());
                let present = v.as_deref().is_some_and(|s| !s.is_empty()) || (self.dm.nullok(f) && b.exists(f));
                if !present {
                    continue;
                }
                if (f == "crossref" || f == "xref") && !self.secr().has_citekey(&b.gs(f)) {
                    continue;
                }
                a.push_str(&self.printfield(&b, f, &v.unwrap_or_default()));
            }
            for df in &helpers.datefields {
                let d = df.strip_suffix("date").unwrap_or(df);
                let g = |s: &str| b.get(&alloc::format!("{d}{s}")).filter(Val::truthy);
                if let Some(u) = g("dateunspecified") {
                    a.push_str(&alloc::format!("      \\field{{{d}dateunspecified}}{{{}}}\n", u.str()));
                }
                for (src, out) in [
                    ("datejulian", "datejulian"),
                    ("enddatejulian", "enddatejulian"),
                    ("dateapproximate", "datecirca"),
                    ("enddateapproximate", "enddatecirca"),
                    ("dateuncertain", "dateuncertain"),
                    ("enddateuncertain", "enddateuncertain"),
                    ("dateunknown", "dateunknown"),
                    ("enddateunknown", "enddateunknown"),
                ] {
                    if g(src).is_some() {
                        a.push_str(&alloc::format!("      \\true{{{d}{out}}}\n"));
                    }
                }
                if b.exists(&alloc::format!("{d}endyear")) {
                    if let Some(e) = g("endera") {
                        a.push_str(&alloc::format!("      \\field{{{d}enddateera}}{{{}}}\n", e.str()));
                    }
                }
                if b.exists(&alloc::format!("{d}year")) {
                    if g("datesplit").is_none() {
                        continue;
                    }
                    if let Some(e) = g("era") {
                        a.push_str(&alloc::format!("      \\field{{{d}dateera}}{{{}}}\n", e.str()));
                    }
                }
            }
            for f in &helpers.xsv {
                if f == "keywords" {
                    continue;
                }
                if let Some(l) = b.get(f).and_then(|v| v.as_list()) {
                    let j = l.borrow().join(",");
                    a.push_str(&self.printfield(&b, f, &j));
                }
            }
            if b.get("nocite").is_some_and(|v| v.truthy()) {
                a.push_str("      \\true{nocite}\n");
            }
            for rf in &helpers.ranges {
                let Some(r) = b.get(rf).and_then(|v| v.as_range()) else { continue };
                let r = r.borrow();
                let pr: Vec<String> = r
                    .iter()
                    .map(|(s, e)| match e {
                        Some(e) => alloc::format!("{s}\\bibrangedash{}", if truthy(e) { alloc::format!(" {e}") } else { String::new() }),
                        None => s.clone(),
                    })
                    .collect();
                a.push_str(&alloc::format!("      \\field{{{rf}}}{{{}}}\n", pr.join("\\bibrangessep ")));
                a.push_str(&alloc::format!("      \\range{{{rf}}}{{{}}}\n", rangelen(&r)));
            }
            for vf in &helpers.vfields {
                let Some(v) = b.get(vf).filter(Val::truthy) else { continue };
                let mut v = v.str();
                if self.dm.datatype(vf) == "uri" {
                    a.push_str(&alloc::format!("      \\verb{{{vf}raw}}\n      \\verb {v}\n      \\endverb\n"));
                    v = uri_as_string(&nfc(&v));
                }
                a.push_str(&alloc::format!("      \\verb{{{vf}}}\n      \\verb {v}\n      \\endverb\n"));
            }
            for vl in &helpers.vlists {
                let Some(l) = b.get(vl).and_then(|v| v.as_list()) else { continue };
                let mut l = l.borrow_mut();
                if l.last().is_some_and(|x| lc(x) == others) {
                    a.push_str(&alloc::format!("      \\true{{more{vl}}}\n"));
                    l.pop();
                }
                let uri = self.dm.datatype(vl) == "uri";
                if uri {
                    a.push_str(&alloc::format!("      \\lverb{{{vl}raw}}{{{}}}\n", l.len()));
                    for f in l.iter() {
                        a.push_str(&alloc::format!("      \\lverb {f}\n"));
                    }
                    a.push_str("      \\endlverb\n");
                }
                a.push_str(&alloc::format!("      \\lverb{{{vl}}}{{{}}}\n", l.len()));
                for f in l.iter_mut() {
                    if uri {
                        *f = uri_as_string(&nfc(f));
                    }
                    a.push_str(&alloc::format!("      \\lverb {f}\n"));
                }
                a.push_str("      \\endlverb\n");
            }
            if let Some(k) = b.get("keywords").and_then(|v| v.as_list()) {
                a.push_str(&alloc::format!("      \\keyw{{{}}}\n", k.borrow().join(",")));
            }
            if let Some(w) = b.get("warnings").and_then(|v| v.as_list()) {
                for x in w.borrow().iter() {
                    a.push_str(&alloc::format!("      \\warn{{\\item {x}}}\n"));
                }
            }
        }
        a.push_str("    \\endentry\n");
        self.bbl.entries.entry(secnum).or_default().insert(key.into(), a);
    }

    /// Output::base `create_output_section`.
    pub fn create_output_section(&mut self) -> Result<(), crate::biber::Fatal> {
        let secnum = self.cur.clone();
        for k in self.secr().citekeys.clone() {
            if self.entry(&k).is_none() {
                return Err(self.fatal(alloc::format!("Cannot find entry with key '{k}' to output")));
            }
            self.set_output_entry(&k);
        }
        for k in self.secr().undef_citekeys.clone() {
            self.bbl.missing.entry(secnum.clone()).or_default().insert(k.clone(), alloc::format!("  \\missing{{{k}}}\n"));
        }
        for (a, r) in self.secr().alias.clone() {
            self.bbl.alias.entry(secnum.clone()).or_default().insert(a.clone(), alloc::format!("  \\keyalias{{{a}}}{{{r}}}\n"));
        }
        Ok(())
    }

    /// Output::bbl `create_output_misc`.
    pub fn create_output_misc(&mut self) {
        self.bbl.tail.push_str("\\endinput\n\n");
    }

    fn instantiate_entry(&self, dl: &DataList, entry: &str, key: &str) -> String {
        let Some(be) = self.entry(key) else { return String::new() };
        let b = be.borrow();
        let mut s = entry.to_string();
        let sub = |s: &mut String, ph: &str, v: &str| {
            *s = s.replace(&alloc::format!("<BDS>{ph}</BDS>"), v);
        };
        let ef = |f: &str| dl.entryfield(key, f);
        let eft = |f: &str| ef(f).is_some_and(|v| truthy(&v.s()));
        if let Some(FieldV::L(es)) = ef("entryset") {
            sub(&mut s, "ENTRYSET", &alloc::format!("\\set{{{}}}", es.join(",")));
        }
        for t in ["uniqueprimaryauthor", "uniquework", "uniquebaretitle", "uniquetitle"] {
            if eft(t) {
                sub(&mut s, &t.to_uppercase(), &alloc::format!("\\true{{{t}}}"));
            }
        }
        if let Some(e) = dl.state.extraalphadata.get(key).filter(|&&v| v != 0) {
            sub(&mut s, "EXTRAALPHA", &alloc::format!("\\field{{extraalpha}}{{{e}}}"));
        }
        if let Some(e) = dl.state.labelalphadata.get(key).filter(|v| truthy(v)) {
            sub(&mut s, "LABELALPHA", &alloc::format!("\\field{{labelalpha}}{{{e}}}"));
        }
        let nls = self.dm.helpers.namelists.clone();
        for nf in &nls {
            let Some(nl) = b.get(nf).and_then(|v| v.as_names()) else { continue };
            let nlid = nl.borrow().id.clone();
            match dl.uniquelist(&nlid) {
                Some(u) => sub(&mut s, &alloc::format!("UL-{nlid}"), &alloc::format!("ul={u}")),
                None => s = rs(&s, &alloc::format!("<BDS>UL-{}</BDS>,?", esc(&nlid)), ""),
            }
        }
        for nf in &nls {
            let Some(nl) = b.get(nf).and_then(|v| v.as_names()) else { continue };
            let nl = nl.borrow();
            for n in &nl.names {
                let n = n.borrow();
                let nid = n.id.clone();
                if let Some((sum, part)) = crate::lists::get_unsummary(dl, &nl.id, &nid) {
                    sub(&mut s, &alloc::format!("UNS-{nid}"), &alloc::format!("un={sum}"));
                    sub(&mut s, &alloc::format!("UNP-{nid}"), &alloc::format!("uniquepart={part}"));
                    let d = dl.get_nd(&nl.id, &nid).cloned().unwrap_or_default();
                    for np in n.parts() {
                        if d.basenamestringparts.contains(&np) {
                            s = rs(&s, &alloc::format!(r"\s+<BDS>UNP-{}-{}</BDS>,?", esc(&np), esc(&nid)), "");
                        } else {
                            let v = d.unparts.get(&np).cloned().unwrap_or_default();
                            sub(&mut s, &alloc::format!("UNP-{np}-{nid}"), &alloc::format!("{np}un={v}"));
                        }
                    }
                } else {
                    s = rs(&s, &alloc::format!("<BDS>UN[SP]-{}</BDS>,?", esc(&nid)), "");
                    for np in n.parts() {
                        s = rs(&s, &alloc::format!(r"\s+<BDS>UNP-{}-{}</BDS>,?", esc(&np), esc(&nid)), "");
                    }
                }
            }
        }
        if let Some(e) = dl.state.extratitleyeardata.get(key).filter(|&&v| v != 0) {
            sub(&mut s, "EXTRATITLEYEAR", &alloc::format!("\\field{{extratitleyear}}{{{e}}}"));
        }
        if let Some(e) = dl.state.extratitledata.get(key).filter(|&&v| v != 0) {
            sub(&mut s, "EXTRATITLE", &alloc::format!("\\field{{extratitle}}{{{e}}}"));
        }
        for nf in &nls {
            for (suf, ph) in [("bibnamehash", "BIBNAMEHASH"), ("namehash", "NAMEHASH"), ("fullhash", "FULLHASH"), ("fullhashraw", "FULLHASHRAW")] {
                if let Some(e) = ef(&alloc::format!("{nf}{suf}")).map(|v| v.s()).filter(|v| truthy(v)) {
                    sub(&mut s, &alloc::format!("{nf}{ph}"), &alloc::format!("\\strng{{{nf}{suf}}}{{{e}}}"));
                }
            }
        }
        for (f, ph) in [("fullhash", "FULLHASH"), ("fullhashraw", "FULLHASHRAW"), ("bibnamehash", "BIBNAMEHASH"), ("namehash", "NAMEHASH")] {
            if let Some(e) = ef(f).map(|v| v.s()).filter(|v| truthy(v)) {
                sub(&mut s, ph, &alloc::format!("\\strng{{{f}}}{{{e}}}"));
            }
        }
        for pn in self.dm.helpers.namelistsall.clone() {
            let Some(nl) = b.get(&pn).and_then(|v| v.as_names()) else { continue };
            let nl = nl.borrow();
            for n in &nl.names {
                let nid = n.borrow().id.clone();
                match dl.get_nd(&nl.id, &nid).and_then(|d| d.hash.clone()).filter(|h| truthy(h)) {
                    Some(h) => sub(&mut s, &alloc::format!("{nid}-PERNAMEHASH"), &alloc::format!("hash={h}")),
                    None => s = rs(&s, &alloc::format!("<BDS>{}-PERNAMEHASH</BDS>,?", esc(&nid)), ""),
                }
            }
        }
        if let Some(e) = dl.state.extranamedata.get(key).filter(|&&v| v != 0) {
            sub(&mut s, "EXTRANAME", &alloc::format!("\\field{{extraname}}{{{e}}}"));
        }
        if let Some(e) = dl.state.extradatedata.get(key).filter(|&&v| v != 0) {
            sub(&mut s, "EXTRADATE", &alloc::format!("\\field{{extradate}}{{{e}}}"));
        }
        if let Some(si) = dl.sortinit.get(key) {
            sub(&mut s, "SORTINIT", &alloc::format!("\\field{{sortinit}}{{{si}}}"));
            let c = Collator { level: 1, upper_before_lower: false, shifted: true, normalize: true };
            let h = md5_hex(c.view_sort_key(&self.table, si).as_bytes());
            sub(&mut s, "SORTINITHASH", &alloc::format!("\\field{{sortinithash}}{{{h}}}"));
        }
        if truthy(&dl.labelprefix) {
            sub(&mut s, "LABELPREFIX", &alloc::format!("\\field{{labelprefix}}{{{}}}", dl.labelprefix));
        }
        if eft("singletitle") {
            sub(&mut s, "SINGLETITLE", "\\true{singletitle}");
        }
        s = rs(&s, r",(?:\n\s+)?\}\}", "}}");
        s = rs(&s, r"^\s+<BDS>[^<]+</BDS>\n", "");
        s
    }

    /// Output::bbl `output`.
    pub fn bbl_text(&self) -> String {
        let mut o = String::from(HEAD);
        if !self.preamble.is_empty() {
            o.push_str(&alloc::format!("\\preamble{{%\n{}%\n}}\n\n", self.preamble.join("%\n")));
        }
        let gss = self.cfg.blx_s(None, "sortingtemplatename", None, None);
        for (secnum, ents) in &self.bbl.entries {
            o.push_str(&alloc::format!("\n\\refsection{{{secnum}}}\n"));
            let mut lists: Vec<&DataList> = self.lists.iter().filter(|l| l.section == *secnum).collect();
            lists.sort_by(|a, b| a.sortingtemplatename.cmp(&b.sortingtemplatename));
            let mut ordered: Vec<&DataList> = lists.iter().copied().filter(|l| !(l.sortingtemplatename == gss && l.typ == "entry")).collect();
            ordered.extend(self.lists.iter().filter(|l| l.section == *secnum && l.typ == "entry" && l.sortingtemplatename == gss));
            for l in ordered {
                if l.keys.is_empty() {
                    continue;
                }
                o.push_str(&alloc::format!("  \\datalist[{}]{{{}}}\n", l.typ, l.name));
                for k in &l.keys {
                    let e = ents.get(k).cloned().unwrap_or_default();
                    if e.is_empty() {
                        continue;
                    }
                    o.push_str(&self.instantiate_entry(l, &e, k));
                }
                o.push_str("  \\enddatalist\n");
            }
            for v in self.bbl.alias.get(secnum).into_iter().flat_map(|m| m.values()) {
                o.push_str(v);
            }
            for v in self.bbl.missing.get(secnum).into_iter().flat_map(|m| m.values()) {
                o.push_str(v);
            }
            o.push_str("\\endrefsection\n");
        }
        o.push_str(&self.bbl.tail);
        nfc(&o)
    }
}

/// URI->new($s)->as_string: characters outside the URI set are
/// percent-encoded as UTF-8; existing escapes are kept.
fn uri_as_string(s: &str) -> String {
    let mut o = String::new();
    for c in s.chars() {
        let keep = c.is_ascii_alphanumeric() || "-_.!~*'();/?:@&=+$,[]#%".contains(c);
        if keep {
            o.push(c);
        } else {
            let mut b = [0u8; 4];
            for x in c.encode_utf8(&mut b).bytes() {
                o.push_str(&alloc::format!("%{x:02X}"));
            }
        }
    }
    o
}
