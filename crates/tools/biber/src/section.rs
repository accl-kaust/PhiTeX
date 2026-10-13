//! Biber::Section (a refsection's keys, entries and caches) and
//! Biber::DataList (a sorted list with its per-list disambiguation state).

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::String;
use alloc::vec::Vec;

use crate::config::SortTemplate;
use crate::model::EntryRef;

#[derive(Clone, Debug)]
pub struct DataSource {
    pub typ: String,
    pub name: String,
    pub datatype: String,
    pub encoding: String,
    pub glob: String,
}

/// A `\DeclareLabelalphaTemplate` `v` width cache.
#[derive(Clone, Debug, Default)]
pub struct LabelCacheV {
    pub data: BTreeMap<String, Vec<String>>,
    pub index: BTreeMap<String, usize>,
    pub nameindex: BTreeMap<String, usize>,
    pub globalindices: BTreeMap<String, usize>,
}

#[derive(Clone, Debug, Default)]
pub struct Section {
    pub entries: BTreeMap<String, EntryRef>,
    pub np_lengths: BTreeMap<String, usize>,
    pub keytorelclone: BTreeMap<String, String>,
    pub relkeys: BTreeSet<String>,
    pub allkeys: bool,
    pub allkeys_nocite: bool,
    pub citekeys: Vec<String>,
    pub citekeys_h: BTreeSet<String>,
    pub labelcache_l: BTreeMap<String, Vec<Vec<String>>>,
    pub labelcache_v: BTreeMap<String, LabelCacheV>,
    pub everykey: BTreeSet<String>,
    pub everykey_lc: BTreeMap<String, String>,
    pub keytods: BTreeMap<String, String>,
    pub orig_order: Vec<String>,
    pub undef_citekeys: Vec<String>,
    pub cite: BTreeSet<String>,
    pub nocite: BTreeSet<String>,
    pub alias: BTreeMap<String, String>,
    pub set_pc: BTreeMap<String, Vec<String>>,
    pub set_cp: BTreeMap<String, Vec<String>>,
    pub seenkeys: BTreeMap<String, usize>,
    pub citecount: BTreeMap<String, String>,
    pub dkeys: Vec<(String, Vec<String>)>,
    pub datasources: Option<Vec<DataSource>>,
}

impl Section {
    pub fn new(_number: &str) -> Section {
        Section {
            ..Section::default()
        }
    }
    pub fn citecount(&self, k: &str) -> String {
        match self.citecount.get(k) {
            Some(c) if crate::model::truthy(c) => c.clone(),
            _ => String::from("-1"),
        }
    }
    pub fn set_np_length(&mut self, np: &str, len: usize) {
        let e = self.np_lengths.entry(np.into()).or_insert(0);
        if len > *e {
            *e = len;
        }
    }
    pub fn np_length(&self, np: &str) -> usize {
        self.np_lengths.get(np).copied().unwrap_or(0)
    }
    pub fn add_set_pc(&mut self, p: &str, c: &str) {
        let v = self.set_pc.entry(p.into()).or_default();
        if !v.iter().any(|x| x == c) {
            v.push(c.into());
        }
    }
    pub fn add_set_cp(&mut self, c: &str, p: &str) {
        let v = self.set_cp.entry(c.into()).or_default();
        if !v.iter().any(|x| x == p) {
            v.push(p.into());
        }
    }
    pub fn badcasekey(&self, k: &str) -> Option<String> {
        let c = self.everykey_lc.get(&crate::unicode::lc(k))?;
        if c == k { None } else { Some(c.clone()) }
    }
    pub fn is_specific(&self, k: &str) -> bool {
        self.cite.contains(k) || self.nocite.contains(k)
    }
    pub fn add_everykey(&mut self, k: &str) {
        self.everykey.insert(k.into());
        self.everykey_lc.insert(crate::unicode::lc(k), k.into());
    }
    pub fn alias_of(&self, k: &str) -> Option<String> {
        self.alias.get(k).cloned()
    }
    pub fn entry(&self, k: &str) -> Option<EntryRef> {
        match self.alias.get(k) {
            Some(r) => self.entries.get(r).cloned(),
            None => self.entries.get(k).cloned(),
        }
    }
    pub fn has_cited(&self, k: &str) -> bool {
        self.citekeys_h.contains(k)
    }
    pub fn has_citekey(&self, k: &str) -> bool {
        let k2 = self
            .alias
            .get(k)
            .filter(|s| crate::model::truthy(s))
            .map_or(k, String::as_str);
        self.citekeys_h.contains(k2)
    }
    pub fn del_citekey(&mut self, k: &str) {
        if !self.has_citekey(k) {
            return;
        }
        self.citekeys.retain(|x| x != k);
        self.orig_order.retain(|x| x != k);
        self.citekeys_h.remove(k);
    }
    pub fn del_citekeys(&mut self) {
        self.citekeys.clear();
        self.citekeys_h.clear();
        self.orig_order.clear();
    }
    pub fn add_citekeys(&mut self, keys: &[String]) {
        for k in keys {
            if self.has_citekey(k) {
                continue;
            }
            self.citekeys_h.insert(k.clone());
            self.citekeys.push(k.clone());
            self.orig_order.push(k.clone());
        }
    }
    pub fn dynamic_set(&self, k: &str) -> Vec<String> {
        self.dkeys
            .iter()
            .find(|(x, _)| x == k)
            .map(|(_, m)| m.clone())
            .unwrap_or_default()
    }
    pub fn set_dynamic_set(&mut self, k: &str, m: Vec<String>) {
        if let Some(e) = self.dkeys.iter_mut().find(|(x, _)| x == k) {
            e.1 = m;
        } else {
            self.dkeys.push((k.into(), m));
        }
    }
    pub fn dynamic_set_keys(&self) -> Vec<String> {
        self.dkeys.iter().map(|(k, _)| k.clone()).collect()
    }
    pub fn static_citekeys(&self) -> Vec<String> {
        let d = self.dynamic_set_keys();
        self.citekeys
            .iter()
            .filter(|k| !d.contains(k))
            .cloned()
            .collect()
    }
}

/// A name's disambiguation schema step: (part or "base", level or parts).
pub type DisStep = (String, Vec<String>);

#[derive(Clone, Debug, Default)]
pub struct NameData {
    pub namestring: Option<String>,
    pub namestrings: Vec<String>,
    pub basenamestring: Option<String>,
    pub basenamestringparts: Vec<String>,
    pub namedisschema: Vec<DisStep>,
    pub hash: Option<String>,
    pub unmininfo: Option<String>,
    pub un: Option<DisStep>,
    pub unall: Option<DisStep>,
    pub unparts: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Default)]
pub struct ListData {
    pub ul: Option<usize>,
    pub viscite: usize,
    pub visbib: usize,
    pub vissort: usize,
    pub visalpha: usize,
    pub names: BTreeMap<String, NameData>,
}

/// The disambiguation and labelling state of a list (`reset_state`).
#[derive(Clone, Debug, Default)]
pub struct ListState {
    pub seenpa: BTreeMap<String, BTreeSet<String>>,
    pub fields: BTreeMap<String, BTreeMap<String, FieldV>>,
    pub uniquenamecount: BTreeMap<String, BTreeMap<String, BTreeSet<String>>>,
    pub uniquenamecount_all: BTreeMap<String, BTreeMap<String, BTreeSet<String>>>,
    pub ulcount: BTreeMap<String, usize>,
    pub ulcount_final: BTreeMap<String, usize>,
    pub ulcount_final_year: BTreeMap<String, BTreeMap<String, usize>>,
    pub ulcount_minyear: BTreeMap<String, BTreeMap<String, BTreeSet<String>>>,
    pub namelistdata: BTreeMap<String, ListData>,
    pub seen_extradate: BTreeMap<String, usize>,
    pub seen_extraname: BTreeMap<String, usize>,
    pub seen_labelname: BTreeMap<String, usize>,
    pub seen_extratitle: BTreeMap<String, usize>,
    pub seen_extratitleyear: BTreeMap<String, usize>,
    pub seen_extraalpha: BTreeMap<String, usize>,
    pub seen_nametitledateparts: BTreeMap<String, usize>,
    pub seen_nametitle: BTreeMap<String, usize>,
    pub seen_titleyear: BTreeMap<String, usize>,
    pub seenname: BTreeMap<String, usize>,
    pub seentitle: BTreeMap<String, usize>,
    pub seenbaretitle: BTreeMap<String, usize>,
    pub seenwork: BTreeMap<String, usize>,
    pub ladisambiguation: BTreeMap<String, usize>,
    pub labelalphadata: BTreeMap<String, String>,
    pub extradatedata: BTreeMap<String, usize>,
    pub extranamedata: BTreeMap<String, usize>,
    pub extratitledata: BTreeMap<String, usize>,
    pub extratitleyeardata: BTreeMap<String, usize>,
    pub extraalphadata: BTreeMap<String, usize>,
}

#[derive(Clone, Debug)]
pub enum FieldV {
    S(String),
    L(Vec<String>),
}

impl FieldV {
    pub fn s(&self) -> String {
        match self {
            FieldV::S(s) => s.clone(),
            FieldV::L(_) => String::from("ARRAY"),
        }
    }
}

#[derive(Clone, Debug)]
pub enum Filter {
    One(String, String),
    Or(Vec<(String, String)>),
}

#[derive(Clone, Debug, Default)]
pub struct DataList {
    pub section: String,
    pub name: String,
    pub typ: String,
    pub sortingtemplatename: String,
    pub sortingnamekeytemplatename: String,
    pub uniquenametemplatename: String,
    pub labelalphanametemplatename: String,
    pub namehashtemplatename: String,
    pub labelprefix: String,
    pub filters: Vec<Filter>,
    pub keys: Vec<String>,
    pub state: ListState,
    pub unul_changed: bool,
    pub sortingtemplate: SortTemplate,
    /// Per sort set: is it an integer sort.
    pub sortdataschema: Vec<(bool, bool)>,
    pub sortdata: BTreeMap<String, (String, Vec<String>)>,
    pub sortinit: BTreeMap<String, String>,
}

impl DataList {
    pub fn ld(&mut self, nlid: &str) -> &mut ListData {
        self.state.namelistdata.entry(nlid.into()).or_default()
    }
    pub fn nd(&mut self, nlid: &str, nid: &str) -> &mut NameData {
        self.ld(nlid).names.entry(nid.into()).or_default()
    }
    pub fn get_nd(&self, nlid: &str, nid: &str) -> Option<&NameData> {
        self.state
            .namelistdata
            .get(nlid)
            .and_then(|l| l.names.get(nid))
    }
    pub fn uniquelist(&self, nlid: &str) -> Option<usize> {
        self.state.namelistdata.get(nlid).and_then(|l| l.ul)
    }
    pub fn vis(&self, nlid: &str) -> (usize, usize, usize, usize) {
        self.state
            .namelistdata
            .get(nlid)
            .map_or((0, 0, 0, 0), |l| (l.viscite, l.visbib, l.vissort, l.visalpha))
    }
    pub fn entryfield(&self, key: &str, f: &str) -> Option<FieldV> {
        self.state.fields.get(key).and_then(|m| m.get(f)).cloned()
    }
    pub fn entryfield_s(&self, key: &str, f: &str) -> Option<String> {
        self.entryfield(key, f).map(|v| v.s())
    }
    pub fn set_entryfield(&mut self, key: &str, f: &str, v: FieldV) {
        self.state
            .fields
            .entry(key.into())
            .or_default()
            .insert(f.into(), v);
    }
    pub fn set_uniquename(&mut self, nlid: &str, nid: &str, s: DisStep) {
        let nd = self.nd(nlid, nid);
        let changed = nd.un.as_ref() != Some(&s);
        nd.un = Some(s);
        if changed {
            self.unul_changed = true;
        }
    }
}
