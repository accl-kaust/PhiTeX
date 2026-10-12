//! Trace replay: a workload given as JSONL steps (what each node reads,
//! writes, opens, closes, claims and unfolds, with the hints a client can
//! derive from text) instead of toy-language text. The trace operation runs
//! every step the same way: it reads the named fields, hashes what it saw
//! into its output, writes the named fields, and applies the structure the
//! step describes. Results therefore test binding, claims, scopes and
//! unfolding, not a language's semantics.
//!
//! # Format (version 1)
//!
//! The first line is `{"meta": {...}}`, every other line one step:
//!
//! ```text
//! {"meta": {"version": 1, "document": "name", "steps": N,
//!           "sequences": ["items"], "env": {"ts": 1}}}
//! {"id": "s1", "class": "macro", "pos": 1, "depth": 0,
//!  "reads": ["a"], "writes": [{"field": "x", "locality": "local"}],
//!  "may_define": "any" | "none" | {"names": [{"name": "x", "locality": "local"}]}
//!                | {"prefixes": [{"prefix": "e", "locality": "global"}]},
//!  "scope": "none" | "open" | "close" | "pending"
//!           | {"closes": 0, "opens": 1, "before": false},
//!  "scope_kind": "A", "starts_implicit": "B", "closes_implicit": "B",
//!  "contributes": "items", "reads_prev_contribution": "items",
//!  "fold": {"sequence": "items", "step_class": "add"},
//!  "claim": {"known": 2} | {"count_from": "n"}
//!           | {"until": {"class": "mark", "inert": true, "confirm": ["go"]}}
//!           | {"predicted": {"n": 3, "until": {...}}},
//!  "limit": 5, "env_reads": ["ts"], "unfold": "defines_nothing" | "unknown",
//!  "children": [steps...], "value_dep": true,
//!  "prior": {"children": 2}, "cause": "tag"}
//! ```
//!
//! Required: `id` (unique, nested steps included), `class`, `pos` (strictly
//! increasing at the top level), `reads`, and `writes` or `may_define`
//! (`writes: null` is a definition set pending until the step runs; the
//! hint then bounds it). Everything else defaults to absent. Unknown keys,
//! wrong types and inconsistent depths are errors naming the line.
//!
//! `edits.jsonl` holds one edit per line, applied in order:
//!
//! ```text
//! {"op": "change_writes", "step": "s1", "writes": [...], "expect": {"reached_bound": 3}}
//! {"op": "change_reads", "step": "s1", "reads": [...]}
//! {"op": "insert_after", "step": "s1", "new": {step}}
//! {"op": "delete", "step": "s1"}
//! ```

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use defuse::{
    Brackets, Claim, Ctx, Effect, Engine, Field, FxHashMap, FxHashSet, Locality, MayDefine, NodeId, OpId, Operation,
    Stop, UnitId, Until, ValueId, Values, hash_words,
};

use super::json::{self, Json};

#[derive(Clone, Debug, PartialEq)]
pub struct Write {
    pub field: String,
    pub local: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Bound {
    None,
    Names(Vec<Write>),
    Prefixes(Vec<Write>),
    Any,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Scope {
    None,
    Open,
    Close,
    Pending,
    Brackets { closes: u8, opens: u8, before: bool },
}

#[derive(Clone, Debug, PartialEq)]
pub struct UntilSpec {
    pub class: String,
    pub inert: bool,
    pub confirm: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ClaimSpec {
    None,
    Known(usize),
    CountFrom(String),
    Until(UntilSpec),
    Predicted(usize, UntilSpec),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    pub id: String,
    pub class: String,
    pub pos: i64,
    pub depth: Option<i64>,
    pub reads: Vec<String>,
    pub writes: Option<Vec<Write>>,
    pub may_define: Bound,
    pub scope: Scope,
    pub scope_kind: Option<String>,
    pub starts_implicit: Option<String>,
    pub closes_implicit: Option<String>,
    pub contributes: Option<String>,
    pub reads_prev: Option<String>,
    pub fold: Option<(String, String)>,
    pub claim: ClaimSpec,
    pub limit: Option<usize>,
    pub env_reads: Vec<String>,
    pub unfold: Option<String>,
    pub children: Vec<Step>,
    pub value_dep: bool,
    pub prior_children: Option<usize>,
    pub cause: Option<String>,
    /// Bumped by every edit of the step: the engine sees new text.
    pub rev: u32,
}

#[derive(Clone, Debug, Default)]
pub struct Meta {
    pub document: String,
    pub steps: usize,
    pub sequences: Vec<String>,
    pub env: Vec<(String, Json)>,
}

#[derive(Clone, Debug)]
pub enum Edit {
    ChangeWrites(String, Option<Vec<Write>>),
    ChangeReads(String, Vec<String>),
    InsertAfter(String, Box<Step>),
    Delete(String),
}

#[derive(Clone, Debug)]
pub struct EditLine {
    pub edit: Edit,
    pub expect_reached: Option<u64>,
}

// ------------------------------------------------------------- validation

fn fail(line: usize, what: impl std::fmt::Display) -> String {
    format!("line {line}: {what}")
}

fn field_names(line: usize, key: &str, v: &Json) -> Result<Vec<String>, String> {
    let a = v
        .as_arr()
        .ok_or_else(|| fail(line, format!("`{key}` must be an array of strings")))?;
    a.iter()
        .map(|x| match x.as_str() {
            Some(s) if !s.is_empty() => Ok(s.to_string()),
            _ => Err(fail(line, format!("`{key}` must hold non-empty strings"))),
        })
        .collect()
}

fn writes(line: usize, key: &str, v: &Json, name_key: &str) -> Result<Vec<Write>, String> {
    let a = v
        .as_arr()
        .ok_or_else(|| fail(line, format!("`{key}` must be an array")))?;
    a.iter()
        .map(|w| {
            let o = w
                .as_obj()
                .ok_or_else(|| fail(line, format!("`{key}` entries must be objects")))?;
            for k in o.keys() {
                if k != name_key && k != "locality" {
                    return Err(fail(line, format!("`{key}` entry has unknown key {k:?}")));
                }
            }
            let field = o
                .get(name_key)
                .and_then(Json::as_str)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| fail(line, format!("`{key}` entry needs a non-empty {name_key:?}")))?;
            let local = match o.get("locality").and_then(Json::as_str) {
                Some("local") => true,
                Some("global") => false,
                _ => {
                    return Err(fail(
                        line,
                        format!("`{key}` entry needs \"locality\": \"local\" | \"global\""),
                    ));
                }
            };
            Ok(Write {
                field: field.to_string(),
                local,
            })
        })
        .collect()
}

fn until_spec(line: usize, v: &Json) -> Result<UntilSpec, String> {
    let o = v.as_obj().ok_or_else(|| fail(line, "`until` must be an object"))?;
    let mut spec = UntilSpec {
        class: String::new(),
        inert: false,
        confirm: Vec::new(),
    };
    for (k, x) in o {
        match k.as_str() {
            "class" => {
                spec.class = x
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| fail(line, "`until.class` must be a non-empty string"))?
                    .to_string();
            }
            "inert" => spec.inert = x.as_bool().ok_or_else(|| fail(line, "`until.inert` must be a bool"))?,
            "confirm" => {
                spec.confirm = match x {
                    Json::Str(s) => vec![s.clone()],
                    _ => field_names(line, "until.confirm", x)?,
                };
            }
            _ => return Err(fail(line, format!("`until` has unknown key {k:?}"))),
        }
    }
    if spec.class.is_empty() {
        return Err(fail(line, "`until` needs a class"));
    }
    Ok(spec)
}

fn usize_of(line: usize, key: &str, v: &Json) -> Result<usize, String> {
    v.as_int()
        .and_then(|i| usize::try_from(i).ok())
        .ok_or_else(|| fail(line, format!("`{key}` must be a non-negative integer")))
}

/// Parses one step (nested ones recursively).
#[allow(clippy::too_many_lines)]
pub fn step_of(line: usize, v: &Json) -> Result<Step, String> {
    let o = v.as_obj().ok_or_else(|| fail(line, "a step must be an object"))?;
    let mut s = Step {
        id: String::new(),
        class: String::new(),
        pos: 0,
        depth: None,
        reads: Vec::new(),
        writes: None,
        may_define: Bound::Any,
        scope: Scope::None,
        scope_kind: None,
        starts_implicit: None,
        closes_implicit: None,
        contributes: None,
        reads_prev: None,
        fold: None,
        claim: ClaimSpec::None,
        limit: None,
        env_reads: Vec::new(),
        unfold: None,
        children: Vec::new(),
        value_dep: true,
        prior_children: None,
        cause: None,
        rev: 0,
    };
    let (mut has_pos, mut has_reads, mut has_writes, mut has_bound) = (false, false, false, false);
    let opt_str = |key: &str, x: &Json| -> Result<Option<String>, String> {
        match x {
            Json::Null => Ok(None),
            Json::Str(t) if !t.is_empty() => Ok(Some(t.clone())),
            _ => Err(fail(line, format!("`{key}` must be a non-empty string or null"))),
        }
    };
    for (k, x) in o {
        match k.as_str() {
            "id" => s.id = opt_str(k, x)?.ok_or_else(|| fail(line, "`id` is required"))?,
            "class" => s.class = opt_str(k, x)?.ok_or_else(|| fail(line, "`class` is required"))?,
            "pos" => {
                s.pos = x.as_int().ok_or_else(|| fail(line, "`pos` must be an integer"))?;
                has_pos = true;
            }
            "depth" => s.depth = Some(x.as_int().ok_or_else(|| fail(line, "`depth` must be an integer"))?),
            "reads" => {
                s.reads = field_names(line, k, x)?;
                has_reads = true;
            }
            "writes" => {
                s.writes = match x {
                    Json::Null => None,
                    _ => Some(writes(line, k, x, "field")?),
                };
                has_writes = true;
            }
            "may_define" => {
                has_bound = true;
                s.may_define = match x {
                    Json::Str(t) if t == "none" => Bound::None,
                    Json::Str(t) if t == "any" => Bound::Any,
                    Json::Obj(m) if m.len() == 1 && m.contains_key("names") => {
                        Bound::Names(writes(line, "may_define.names", &m["names"], "name")?)
                    }
                    Json::Obj(m) if m.len() == 1 && m.contains_key("prefixes") => {
                        Bound::Prefixes(writes(line, "may_define.prefixes", &m["prefixes"], "prefix")?)
                    }
                    _ => {
                        return Err(fail(
                            line,
                            "`may_define` must be \"none\", \"any\", {\"names\": [..]} or {\"prefixes\": [..]}",
                        ));
                    }
                };
            }
            "scope" => {
                s.scope = match x {
                    Json::Str(t) if t == "none" => Scope::None,
                    Json::Str(t) if t == "open" => Scope::Open,
                    Json::Str(t) if t == "close" => Scope::Close,
                    Json::Str(t) if t == "pending" => Scope::Pending,
                    Json::Obj(m) => {
                        let mut b = (0u8, 0u8, false);
                        for (bk, bv) in m {
                            match bk.as_str() {
                                "closes" => {
                                    b.0 = u8::try_from(usize_of(line, "scope.closes", bv)?)
                                        .ok()
                                        .filter(|&n| n <= 3)
                                        .ok_or_else(|| fail(line, "`scope.closes` is at most 3"))?;
                                }
                                "opens" => {
                                    b.1 = u8::try_from(usize_of(line, "scope.opens", bv)?)
                                        .ok()
                                        .filter(|&n| n <= 3)
                                        .ok_or_else(|| fail(line, "`scope.opens` is at most 3"))?;
                                }
                                "before" => {
                                    b.2 = bv
                                        .as_bool()
                                        .ok_or_else(|| fail(line, "`scope.before` must be a bool"))?;
                                }
                                _ => return Err(fail(line, format!("`scope` has unknown key {bk:?}"))),
                            }
                        }
                        Scope::Brackets {
                            closes: b.0,
                            opens: b.1,
                            before: b.2,
                        }
                    }
                    _ => {
                        return Err(fail(
                            line,
                            "`scope` must be \"none\", \"open\", \"close\", \"pending\" or {closes, opens, before}",
                        ));
                    }
                };
            }
            "scope_kind" => s.scope_kind = opt_str(k, x)?,
            "starts_implicit" => s.starts_implicit = opt_str(k, x)?,
            "closes_implicit" => s.closes_implicit = opt_str(k, x)?,
            "contributes" => s.contributes = opt_str(k, x)?,
            "reads_prev_contribution" => s.reads_prev = opt_str(k, x)?,
            "fold" => {
                s.fold = match x {
                    Json::Null => None,
                    Json::Obj(m) => {
                        for fk in m.keys() {
                            if fk != "sequence" && fk != "step_class" {
                                return Err(fail(line, format!("`fold` has unknown key {fk:?}")));
                            }
                        }
                        let seq = m.get("sequence").and_then(Json::as_str).filter(|t| !t.is_empty());
                        let cls = m.get("step_class").and_then(Json::as_str).filter(|t| !t.is_empty());
                        match (seq, cls) {
                            (Some(a), Some(b)) => Some((a.to_string(), b.to_string())),
                            _ => return Err(fail(line, "`fold` needs \"sequence\" and \"step_class\"")),
                        }
                    }
                    _ => return Err(fail(line, "`fold` must be an object or null")),
                };
            }
            "claim" => {
                s.claim = match x {
                    Json::Null => ClaimSpec::None,
                    Json::Obj(m) if m.len() == 1 => {
                        let (ck, cv) = m.iter().next().expect("one");
                        match ck.as_str() {
                            "known" => ClaimSpec::Known(usize_of(line, "claim.known", cv)?),
                            "count_from" => ClaimSpec::CountFrom(
                                cv.as_str()
                                    .filter(|t| !t.is_empty())
                                    .ok_or_else(|| fail(line, "`claim.count_from` must be a field"))?
                                    .to_string(),
                            ),
                            "until" => ClaimSpec::Until(until_spec(line, cv)?),
                            "predicted" => {
                                let p = cv
                                    .as_obj()
                                    .ok_or_else(|| fail(line, "`claim.predicted` must be an object"))?;
                                for pk in p.keys() {
                                    if pk != "n" && pk != "until" {
                                        return Err(fail(line, format!("`claim.predicted` has unknown key {pk:?}")));
                                    }
                                }
                                let n = usize_of(line, "claim.predicted.n", p.get("n").unwrap_or(&Json::Null))?;
                                let u = until_spec(line, p.get("until").unwrap_or(&Json::Null))?;
                                ClaimSpec::Predicted(n, u)
                            }
                            _ => return Err(fail(line, format!("`claim` has unknown form {ck:?}"))),
                        }
                    }
                    _ => {
                        return Err(fail(
                            line,
                            "`claim` must be null or one of {known}, {count_from}, {until}, {predicted}",
                        ));
                    }
                };
            }
            "limit" => s.limit = Some(usize_of(line, k, x)?),
            "env_reads" => s.env_reads = field_names(line, k, x)?,
            "unfold" => {
                s.unfold = match x {
                    Json::Null => None,
                    Json::Str(t) if t == "defines_nothing" || t == "unknown" => Some(t.clone()),
                    _ => return Err(fail(line, "`unfold` must be \"defines_nothing\", \"unknown\" or null")),
                };
            }
            "children" => {
                s.children = match x {
                    Json::Null => Vec::new(),
                    Json::Arr(a) => a.iter().map(|c| step_of(line, c)).collect::<Result<_, _>>()?,
                    _ => return Err(fail(line, "`children` must be an array of steps or null")),
                };
            }
            "value_dep" => s.value_dep = x.as_bool().ok_or_else(|| fail(line, "`value_dep` must be a bool"))?,
            "prior" => {
                s.prior_children = match x {
                    Json::Null => None,
                    Json::Obj(m) if m.len() == 1 && m.contains_key("children") => {
                        Some(usize_of(line, "prior.children", &m["children"])?)
                    }
                    _ => return Err(fail(line, "`prior` must be {\"children\": n} or null")),
                };
            }
            "cause" => s.cause = opt_str(k, x)?,
            _ => return Err(fail(line, format!("unknown step key {k:?}"))),
        }
    }
    if s.id.is_empty() {
        return Err(fail(line, "`id` is required"));
    }
    if s.class.is_empty() {
        return Err(fail(line, "`class` is required"));
    }
    if !has_pos {
        return Err(fail(line, "`pos` is required"));
    }
    if !has_reads {
        return Err(fail(line, "`reads` is required"));
    }
    if !has_writes && !has_bound {
        return Err(fail(line, "`writes` (or `may_define` for a pending set) is required"));
    }
    if s.writes.is_some() && has_bound {
        return Err(fail(line, "`may_define` only goes with `writes: null`"));
    }
    if s.scope_kind.is_some()
        && !matches!(
            s.scope,
            Scope::Open | Scope::Pending | Scope::Brackets { opens: 1..=3, .. }
        )
    {
        return Err(fail(line, "`scope_kind` needs a scope that opens"));
    }
    if (s.starts_implicit.is_some() || s.closes_implicit.is_some()) && s.scope != Scope::Pending {
        return Err(fail(line, "an implicit starter or closer has `scope: \"pending\"`"));
    }
    if let ClaimSpec::Predicted(_, u) = &s.claim
        && u.inert
    {
        return Err(fail(line, "a predicted claim is a stream claim (`inert: false`)"));
    }
    Ok(s)
}

fn meta_of(line: usize, v: &Json) -> Result<Meta, String> {
    let o = v
        .as_obj()
        .and_then(|o| o.get("meta"))
        .and_then(Json::as_obj)
        .ok_or_else(|| fail(line, "the first line must be {\"meta\": {...}}"))?;
    let mut m = Meta::default();
    let mut version = None;
    for (k, x) in o {
        match k.as_str() {
            "version" => version = x.as_int(),
            "document" => {
                m.document = x
                    .as_str()
                    .ok_or_else(|| fail(line, "`meta.document` must be a string"))?
                    .to_string();
            }
            "steps" => m.steps = usize_of(line, "meta.steps", x)?,
            "sequences" => m.sequences = field_names(line, "meta.sequences", x)?,
            "env" => {
                let e = x.as_obj().ok_or_else(|| fail(line, "`meta.env` must be an object"))?;
                for (f, val) in e {
                    if !matches!(val, Json::Int(_) | Json::Str(_) | Json::Bool(_)) {
                        return Err(fail(line, format!("`meta.env.{f}` must be an integer, string or bool")));
                    }
                    m.env.push((f.clone(), val.clone()));
                }
            }
            _ => return Err(fail(line, format!("unknown meta key {k:?}"))),
        }
    }
    if version != Some(1) {
        return Err(fail(line, "`meta.version` must be 1"));
    }
    Ok(m)
}

/// Every id in `steps` (nested included), checking uniqueness.
fn collect_ids(steps: &[Step], seen: &mut FxHashSet<String>) -> Result<(), String> {
    for s in steps {
        if !seen.insert(s.id.clone()) {
            return Err(format!("duplicate step id {:?}", s.id));
        }
        collect_ids(&s.children, seen)?;
    }
    Ok(())
}

fn sequences_in(steps: &[Step], out: &mut Vec<String>) {
    for s in steps {
        for q in [&s.contributes, &s.reads_prev].into_iter().flatten() {
            if !out.contains(q) {
                out.push(q.clone());
            }
        }
        if let Some((q, _)) = &s.fold
            && !out.contains(q)
        {
            out.push(q.clone());
        }
        sequences_in(&s.children, out);
    }
}

/// A parsed trace.
pub struct Trace {
    pub meta: Meta,
    pub steps: Vec<Step>,
}

/// Reads and validates a trace file.
pub fn load_trace(text: &str) -> Result<Trace, String> {
    let mut lines = text.lines().enumerate().filter(|(_, l)| !l.trim().is_empty());
    let Some((i, first)) = lines.next() else {
        return Err("empty trace".into());
    };
    let meta = meta_of(i + 1, &json::parse(first).map_err(|e| fail(i + 1, e))?)?;
    let mut steps = Vec::new();
    let mut depth: Option<i64> = Some(0);
    let mut last_pos: Option<i64> = None;
    for (i, l) in lines {
        let line = i + 1;
        let v = json::parse(l).map_err(|e| fail(line, e))?;
        if v.as_obj().is_some_and(|o| o.contains_key("meta")) {
            return Err(fail(line, "a second meta line"));
        }
        let s = step_of(line, &v)?;
        if last_pos.is_some_and(|p| s.pos <= p) {
            return Err(fail(line, format!("`pos` {} is not after the previous step's", s.pos)));
        }
        last_pos = Some(s.pos);
        if let (Some(d), Some(want)) = (depth, s.depth)
            && d != want
        {
            return Err(fail(line, format!("`depth` {want} but the scopes before give {d}")));
        }
        depth = match (&s.scope, depth) {
            (Scope::None, d) => d,
            (Scope::Open, Some(d)) => Some(d + 1),
            (Scope::Close, Some(d)) if d > 0 => Some(d - 1),
            (Scope::Close, Some(_)) => return Err(fail(line, "closes a scope at depth 0")),
            (Scope::Brackets { closes, opens, .. }, Some(d)) => {
                let d2 = d - i64::from(*closes);
                if d2 < 0 {
                    return Err(fail(line, "closes more scopes than are open"));
                }
                Some(d2 + i64::from(*opens))
            }
            _ => None,
        };
        steps.push(s);
    }
    if meta.steps != steps.len() {
        return Err(format!(
            "meta.steps is {} but the file has {} steps",
            meta.steps,
            steps.len()
        ));
    }
    collect_ids(&steps, &mut FxHashSet::default())?;
    Ok(Trace { meta, steps })
}

/// Reads and validates an edits file.
pub fn load_edits(text: &str) -> Result<Vec<EditLine>, String> {
    let mut out = Vec::new();
    for (i, l) in text.lines().enumerate() {
        if l.trim().is_empty() {
            continue;
        }
        let line = i + 1;
        let v = json::parse(l).map_err(|e| fail(line, e))?;
        let o = v.as_obj().ok_or_else(|| fail(line, "an edit must be an object"))?;
        let op = o
            .get("op")
            .and_then(Json::as_str)
            .ok_or_else(|| fail(line, "`op` is required"))?;
        let step = o
            .get("step")
            .and_then(Json::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| fail(line, "`step` is required"))?
            .to_string();
        let mut expect_reached = None;
        if let Some(e) = o.get("expect") {
            let eo = e.as_obj().ok_or_else(|| fail(line, "`expect` must be an object"))?;
            for (k, x) in eo {
                match k.as_str() {
                    // An upper bound: every step reachable from the edited
                    // step's writes over the recorded def-use edges, with
                    // no cutoff (`reached_bound` is its newer name).
                    "steps_reached" | "reached_bound" => {
                        expect_reached = Some(
                            x.as_int()
                                .and_then(|n| u64::try_from(n).ok())
                                .ok_or_else(|| fail(line, "`expect.steps_reached` must be a non-negative integer"))?,
                        );
                    }
                    _ => return Err(fail(line, format!("`expect` has unknown key {k:?}"))),
                }
            }
        }
        let allowed: &[&str] = match op {
            "change_writes" => &["op", "step", "writes", "expect"],
            "change_reads" => &["op", "step", "reads", "expect"],
            "insert_after" => &["op", "step", "new", "expect"],
            "delete" => &["op", "step", "expect"],
            _ => return Err(fail(line, format!("unknown op {op:?}"))),
        };
        for k in o.keys() {
            if !allowed.contains(&k.as_str()) {
                return Err(fail(line, format!("`{op}` has unknown key {k:?}")));
            }
        }
        let edit = match op {
            "change_writes" => match o.get("writes") {
                None => return Err(fail(line, "`writes` is required")),
                Some(Json::Null) => Edit::ChangeWrites(step, None),
                Some(w) => Edit::ChangeWrites(step, Some(writes(line, "writes", w, "field")?)),
            },
            "change_reads" => Edit::ChangeReads(
                step,
                field_names(
                    line,
                    "reads",
                    o.get("reads").ok_or_else(|| fail(line, "`reads` is required"))?,
                )?,
            ),
            "insert_after" => Edit::InsertAfter(
                step,
                Box::new(step_of(
                    line,
                    o.get("new").ok_or_else(|| fail(line, "`new` is required"))?,
                )?),
            ),
            _ => Edit::Delete(step),
        };
        out.push(EditLine { edit, expect_reached });
    }
    Ok(out)
}

// ------------------------------------------------------------- the operation

/// Steps by key (the value the node's single argument holds).
pub type Table = Arc<RwLock<FxHashMap<ValueId, Arc<Step>>>>;

/// The one operation of a trace: every node is a step.
pub struct TraceOp {
    pub table: Table,
    pub me: Arc<std::sync::OnceLock<OpId>>,
}

/// The key of a step: its id and revision, so an edited step is new text.
pub fn key_of(v: &Values, s: &Step) -> ValueId {
    v.str(&format!("{}#{}", s.id, s.rev))
}

fn state_field(v: &Values, seq: &str) -> Field {
    v.field(&format!("{seq}.state"))
}

impl TraceOp {
    fn step(&self, args: &[ValueId]) -> Arc<Step> {
        self.table
            .read()
            .expect("steps")
            .get(&args[0])
            .cloned()
            .expect("a registered step")
    }

    /// Registers `s` and its children, returning `s`'s key.
    pub fn register(&self, v: &Values, s: &Step) -> ValueId {
        for c in &s.children {
            self.register(v, c);
        }
        let k = key_of(v, s);
        self.table.write().expect("steps").insert(k, Arc::new(s.clone()));
        k
    }
}

fn bound_of(v: &Values, b: &Bound) -> MayDefine {
    match b {
        Bound::None => MayDefine::Nothing,
        Bound::Any => MayDefine::Any,
        Bound::Names(ws) => MayDefine::Names(ws.iter().map(|w| v.field(&w.field)).collect()),
        Bound::Prefixes(ws) => MayDefine::Prefixes(ws.iter().map(|w| w.field.clone()).collect()),
    }
}

impl Operation for TraceOp {
    fn name(&self) -> &'static str {
        "step"
    }

    fn defs(&self, args: &[ValueId], v: &Values) -> Option<Vec<(Field, Locality)>> {
        let s = self.step(args);
        let dynamic_unfold = !s.children.is_empty() && s.unfold.as_deref() != Some("defines_nothing");
        let ws = s.writes.as_ref()?;
        if dynamic_unfold {
            return None;
        }
        let mut out: Vec<(Field, Locality)> = ws
            .iter()
            .map(|w| {
                (
                    v.field(&w.field),
                    if w.local { Locality::Local } else { Locality::Global },
                )
            })
            .collect();
        if let Some(q) = &s.contributes {
            out.push((v.field(q), Locality::Local));
            if let Some((fs, _)) = &s.fold {
                out.push((state_field(v, fs), Locality::Local));
            }
        }
        if let Some((q, _)) = &s.fold {
            out.push((state_field(v, q), Locality::Local));
        }
        out.sort_by_key(|d| d.0.0.raw());
        out.dedup();
        Some(out)
    }

    fn may_define(&self, args: &[ValueId], v: &Values) -> MayDefine {
        let s = self.step(args);
        if s.writes.is_some() && !s.children.is_empty() {
            // The unfolded text's definitions are what the hint bounds.
            return match s.unfold.as_deref() {
                Some("unknown") | None => bound_of(v, &s.may_define),
                _ => MayDefine::Nothing,
            };
        }
        bound_of(v, &s.may_define)
    }

    fn effect(&self, args: &[ValueId], v: &Values) -> Effect {
        let s = self.step(args);
        let kind = s.scope_kind.as_ref().map(|k| v.str(k));
        match &s.scope {
            Scope::None => Effect::None,
            Scope::Open if kind.is_none() => Effect::Open,
            Scope::Open => Effect::Scope(Brackets {
                opens: 1,
                kind,
                ..Brackets::default()
            }),
            Scope::Close => Effect::Close,
            Scope::Pending => Effect::Pending,
            Scope::Brackets { closes, opens, before } => Effect::Scope(Brackets {
                closes: *closes,
                opens: *opens,
                before: *before,
                kind,
            }),
        }
    }

    fn claim_limit(
        &self,
        args: &[ValueId],
        _v: &Values,
        _following: &mut dyn Iterator<Item = (OpId, Vec<ValueId>)>,
    ) -> Option<usize> {
        self.step(args).limit
    }

    fn claim(
        &self,
        args: &[ValueId],
        _v: &Values,
        _following: &mut dyn Iterator<Item = (OpId, Vec<ValueId>)>,
    ) -> Claim {
        match &self.step(args).claim {
            ClaimSpec::Known(n) => Claim::Count(*n),
            ClaimSpec::CountFrom(_) => Claim::Pending,
            ClaimSpec::Until(u) if u.inert => Claim::Pending,
            // Stream claims read outputs; they take nothing.
            ClaimSpec::None | ClaimSpec::Until(_) | ClaimSpec::Predicted(..) => Claim::None,
        }
    }

    fn until(&self, args: &[ValueId], v: &Values, cand: OpId, cand_args: &[ValueId]) -> Until {
        let s = self.step(args);
        let (ClaimSpec::Until(u) | ClaimSpec::Predicted(_, u)) = &s.claim else {
            return Until::No;
        };
        if u.inert || Some(&cand) != self.me.get() {
            return Until::No;
        }
        let c = self.step(cand_args);
        if c.class != u.class {
            return Until::No;
        }
        match u.confirm.len() {
            0 => Until::Yes,
            1 => Until::Read(v.field(&u.confirm[0])),
            _ => Until::ReadEach(u.confirm.iter().map(|f| v.field(f)).collect()),
        }
    }

    fn until_value(&self, _: &[ValueId], _: &Values, _: OpId, _: &[ValueId], value: Option<ValueId>) -> bool {
        value.is_some()
    }

    fn until_values(&self, _: &[ValueId], _: &Values, _: OpId, _: &[ValueId], fields: &[Option<ValueId>]) -> bool {
        fields.iter().all(Option::is_some)
    }

    fn until_predict(
        &self,
        args: &[ValueId],
        _v: &Values,
        _following: &mut dyn Iterator<Item = (OpId, Vec<ValueId>)>,
    ) -> Option<usize> {
        match &self.step(args).claim {
            ClaimSpec::Predicted(n, _) => Some(*n),
            _ => None,
        }
    }

    fn reads(&self, args: &[ValueId], v: &Values) -> Option<Vec<Field>> {
        let s = self.step(args);
        let mut out: Vec<Field> = s.reads.iter().chain(&s.env_reads).map(|f| v.field(f)).collect();
        if let ClaimSpec::CountFrom(f) = &s.claim {
            out.push(v.field(f));
        }
        if let Some((q, cls)) = &s.fold
            && s.class == *cls
        {
            out.push(state_field(v, q));
        }
        Some(out)
    }

    #[allow(clippy::too_many_lines)]
    fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
        let v = cx.values();
        let s = self.step(&cx.args());
        let me = *self.me.get().expect("registered");
        let mut seen: Vec<u64> = vec![hash_words(31, &[cx.args()[0].raw()])];
        // The kind of the enclosing scope decides an implicit starter or
        // closer; it is a binding to the opener.
        if s.starts_implicit.is_some() || s.closes_implicit.is_some() {
            let k = cx.scope_kind()?.map(|k| super::text(v, k));
            let mut b = Brackets {
                before: true,
                ..Brackets::default()
            };
            if let Some(c) = &s.closes_implicit
                && k.as_deref() == Some(c)
            {
                b.closes = 1;
            }
            if let Some(o) = &s.starts_implicit
                && k.as_deref() != Some(o)
            {
                b.opens = 1;
                b.kind = Some(v.str(o));
            }
            cx.set_effect(if b.closes == 0 && b.opens == 0 {
                Effect::None
            } else {
                Effect::Scope(b)
            });
        } else if s.scope == Scope::Pending {
            cx.set_effect(Effect::None);
        }
        for f in s.reads.iter().chain(&s.env_reads) {
            let x = cx.use_field(v.field(f))?;
            seen.push(x.map_or(0, ValueId::raw));
        }
        if let Some(q) = &s.reads_prev {
            let p = cx.prev_contribution(v.field(q))?;
            seen.push(p.map_or(0, ValueId::raw));
        }
        // A fold step chains the state through the sequence.
        let fold_state = match &s.fold {
            Some((q, cls)) if s.class == *cls => {
                let prev = cx.use_field(state_field(v, q))?;
                Some((q.clone(), prev.map_or(0, ValueId::raw)))
            }
            _ => None,
        };
        match &s.claim {
            ClaimSpec::CountFrom(f) => {
                let n = cx.use_field(v.field(f))?.and_then(|x| v.as_int(x)).unwrap_or(0);
                cx.claim(usize::try_from(n).unwrap_or(0));
                for (op, a) in cx.claimed() {
                    if op == me {
                        seen.push(a[0].raw());
                    }
                }
            }
            ClaimSpec::Until(u) if u.inert => {
                let mut n = 0;
                while let Some((op, a)) = cx.following(n) {
                    n += 1;
                    if op == me && self.step(&a).class == u.class {
                        break;
                    }
                    if s.limit.is_some_and(|l| n >= l) {
                        break;
                    }
                }
                cx.claim(n);
                for (op, a) in cx.claimed() {
                    if op == me {
                        seen.push(a[0].raw());
                    }
                }
            }
            ClaimSpec::Until(_) | ClaimSpec::Predicted(..) => {
                for x in cx.claim_stream()? {
                    seen.push(x.raw());
                }
            }
            ClaimSpec::None | ClaimSpec::Known(_) => {}
        }
        let out = if s.value_dep {
            v.int(i64::from_ne_bytes(hash_words(32, &seen).to_ne_bytes()))
        } else {
            v.int(i64::from_ne_bytes(hash_words(33, &seen[..1]).to_ne_bytes()))
        };
        if let Some(q) = &s.contributes {
            cx.push_contribution(v.field(q), out);
        }
        if let Some((q, prev)) = fold_state {
            let st = v.int(i64::from_ne_bytes(hash_words(34, &[prev, out.raw()]).to_ne_bytes()));
            cx.define_with(state_field(v, &q), st, Locality::Local);
        } else if let Some((q, _)) = &s.fold {
            // The fold itself: the initial state.
            cx.define_with(state_field(v, q), out, Locality::Local);
        }
        for c in &s.children {
            cx.child(me, vec![key_of(v, c)]);
        }
        if let Some(ws) = &s.writes {
            for w in ws {
                let val = v.int(i64::from_ne_bytes(
                    hash_words(35, &[out.raw(), v.field(&w.field).0.raw()]).to_ne_bytes(),
                ));
                cx.define_with(
                    v.field(&w.field),
                    val,
                    if w.local { Locality::Local } else { Locality::Global },
                );
            }
        }
        Ok(out)
    }
}

// ------------------------------------------------------------- the document

/// A trace loaded into an engine, with the edits applied so far.
pub struct Doc {
    pub op: OpId,
    pub table: Table,
    pub steps: Vec<Step>,
    pub nodes: Vec<NodeId>,
    pub unit: UnitId,
    pub meta: Meta,
    /// Every id in the document (nested steps included).
    ids: FxHashSet<String>,
}

fn env_value(v: &Values, x: &Json) -> ValueId {
    match x {
        Json::Int(i) => v.int(*i),
        Json::Bool(b) => v.bool(*b),
        Json::Str(s) => v.str(s),
        _ => ValueId::UNIT,
    }
}

impl Doc {
    /// Installs the trace operation, declares the sequences, pushes the
    /// steps and supplies the environment.
    pub fn load(eng: &mut Engine, t: &Trace) -> Doc {
        let table: Table = Arc::new(RwLock::new(FxHashMap::default()));
        let me = Arc::new(std::sync::OnceLock::new());
        let op = eng.register(Arc::new(TraceOp {
            table: table.clone(),
            me: me.clone(),
        }));
        me.set(op).expect("fresh");
        let mut seqs = t.meta.sequences.clone();
        sequences_in(&t.steps, &mut seqs);
        for q in &seqs {
            let f = eng.values().field(q);
            eng.declare_sequence(f);
        }
        let unit = eng.new_unit();
        let tr = TraceOp {
            table: table.clone(),
            me: me.clone(),
        };
        let mut nodes = Vec::with_capacity(t.steps.len());
        for s in &t.steps {
            let k = tr.register(eng.values(), s);
            let n = eng.push(unit, op, vec![k]);
            eng.add_root(n);
            nodes.push(n);
        }
        let env: Vec<(Field, ValueId)> = t
            .meta
            .env
            .iter()
            .map(|(f, x)| (eng.values().field(f), env_value(eng.values(), x)))
            .collect();
        eng.set_environment(env);
        let mut ids = FxHashSet::default();
        collect_ids(&t.steps, &mut ids).expect("validated");
        Doc {
            op,
            table,
            steps: t.steps.clone(),
            nodes,
            unit,
            meta: t.meta.clone(),
            ids,
        }
    }

    fn index_of(&self, id: &str) -> Result<usize, String> {
        self.steps
            .iter()
            .position(|s| s.id == id)
            .ok_or_else(|| format!("no top-level step {id:?}"))
    }

    /// Applies one edit to the engine and to the kept steps.
    pub fn apply(&mut self, eng: &mut Engine, e: &Edit) -> Result<(), String> {
        let tr = TraceOp {
            table: self.table.clone(),
            me: Arc::new(std::sync::OnceLock::from(self.op)),
        };
        match e {
            Edit::ChangeWrites(id, ws) => {
                let i = self.index_of(id)?;
                let s = &mut self.steps[i];
                if s.writes.is_none() && ws.is_some() && s.may_define != Bound::Any {
                    s.may_define = Bound::Any;
                }
                s.writes.clone_from(ws);
                s.rev += 1;
                let k = tr.register(eng.values(), s);
                eng.replace(self.nodes[i], self.op, vec![k]);
            }
            Edit::ChangeReads(id, rs) => {
                let i = self.index_of(id)?;
                let s = &mut self.steps[i];
                s.reads.clone_from(rs);
                s.rev += 1;
                let k = tr.register(eng.values(), s);
                eng.replace(self.nodes[i], self.op, vec![k]);
            }
            Edit::InsertAfter(id, new) => {
                let i = self.index_of(id)?;
                collect_ids(std::slice::from_ref(new), &mut self.ids)?;
                let k = tr.register(eng.values(), new);
                let n = eng.insert_after(self.nodes[i], self.op, vec![k]);
                eng.add_root(n);
                self.nodes.insert(i + 1, n);
                self.steps.insert(i + 1, new.as_ref().clone());
            }
            Edit::Delete(id) => {
                let i = self.index_of(id)?;
                eng.remove(self.nodes[i]);
                self.nodes.remove(i);
                let gone = self.steps.remove(i);
                let mut ids = FxHashSet::default();
                collect_ids(std::slice::from_ref(&gone), &mut ids).expect("were unique");
                for g in ids {
                    self.ids.remove(&g);
                }
            }
        }
        Ok(())
    }

    /// The outputs of the top-level steps (the replay's result).
    pub fn result(&self, eng: &Engine) -> Vec<u64> {
        self.nodes
            .iter()
            .map(|&n| eng.output(n).map_or(0, ValueId::raw))
            .collect()
    }

    /// The current steps as a trace (for a cold replay of the edited state).
    pub fn snapshot(&self) -> Trace {
        let mut meta = self.meta.clone();
        meta.steps = self.steps.len();
        Trace {
            meta,
            steps: self.steps.clone(),
        }
    }
}

// ------------------------------------------------------------ serialization

fn write_json(w: &[Write], name_key: &str) -> String {
    let items: Vec<String> = w
        .iter()
        .map(|x| {
            format!(
                "{{{}: {}, \"locality\": \"{}\"}}",
                json::quote(name_key),
                json::quote(&x.field),
                if x.local { "local" } else { "global" }
            )
        })
        .collect();
    format!("[{}]", items.join(", "))
}

fn strs(xs: &[String]) -> String {
    let items: Vec<String> = xs.iter().map(|s| json::quote(s)).collect();
    format!("[{}]", items.join(", "))
}

fn until_json(u: &UntilSpec) -> String {
    format!(
        "{{\"class\": {}, \"inert\": {}, \"confirm\": {}}}",
        json::quote(&u.class),
        u.inert,
        strs(&u.confirm)
    )
}

/// One step as a JSON line (children nested).
pub fn step_json(s: &Step) -> String {
    let mut parts = vec![
        format!("\"id\": {}", json::quote(&s.id)),
        format!("\"class\": {}", json::quote(&s.class)),
        format!("\"pos\": {}", s.pos),
    ];
    if let Some(d) = s.depth {
        parts.push(format!("\"depth\": {d}"));
    }
    parts.push(format!("\"reads\": {}", strs(&s.reads)));
    if let Some(w) = &s.writes {
        parts.push(format!("\"writes\": {}", write_json(w, "field")));
    } else {
        parts.push("\"writes\": null".into());
        parts.push(format!(
            "\"may_define\": {}",
            match &s.may_define {
                Bound::None => "\"none\"".to_string(),
                Bound::Any => "\"any\"".to_string(),
                Bound::Names(w) => format!("{{\"names\": {}}}", write_json(w, "name")),
                Bound::Prefixes(w) => format!("{{\"prefixes\": {}}}", write_json(w, "prefix")),
            }
        ));
    }
    let scope = match &s.scope {
        Scope::None => None,
        Scope::Open => Some("\"open\"".to_string()),
        Scope::Close => Some("\"close\"".to_string()),
        Scope::Pending => Some("\"pending\"".to_string()),
        Scope::Brackets { closes, opens, before } => Some(format!(
            "{{\"closes\": {closes}, \"opens\": {opens}, \"before\": {before}}}"
        )),
    };
    if let Some(sc) = scope {
        parts.push(format!("\"scope\": {sc}"));
    }
    for (k, x) in [
        ("scope_kind", &s.scope_kind),
        ("starts_implicit", &s.starts_implicit),
        ("closes_implicit", &s.closes_implicit),
        ("contributes", &s.contributes),
        ("reads_prev_contribution", &s.reads_prev),
        ("unfold", &s.unfold),
        ("cause", &s.cause),
    ] {
        if let Some(x) = x {
            parts.push(format!("{}: {}", json::quote(k), json::quote(x)));
        }
    }
    if let Some((q, c)) = &s.fold {
        parts.push(format!(
            "\"fold\": {{\"sequence\": {}, \"step_class\": {}}}",
            json::quote(q),
            json::quote(c)
        ));
    }
    let claim = match &s.claim {
        ClaimSpec::None => None,
        ClaimSpec::Known(n) => Some(format!("{{\"known\": {n}}}")),
        ClaimSpec::CountFrom(f) => Some(format!("{{\"count_from\": {}}}", json::quote(f))),
        ClaimSpec::Until(u) => Some(format!("{{\"until\": {}}}", until_json(u))),
        ClaimSpec::Predicted(n, u) => Some(format!(
            "{{\"predicted\": {{\"n\": {n}, \"until\": {}}}}}",
            until_json(u)
        )),
    };
    if let Some(c) = claim {
        parts.push(format!("\"claim\": {c}"));
    }
    if let Some(l) = s.limit {
        parts.push(format!("\"limit\": {l}"));
    }
    if !s.env_reads.is_empty() {
        parts.push(format!("\"env_reads\": {}", strs(&s.env_reads)));
    }
    if !s.children.is_empty() {
        let kids: Vec<String> = s.children.iter().map(step_json).collect();
        parts.push(format!("\"children\": [{}]", kids.join(", ")));
    }
    if !s.value_dep {
        parts.push("\"value_dep\": false".into());
    }
    if let Some(p) = s.prior_children {
        parts.push(format!("\"prior\": {{\"children\": {p}}}"));
    }
    format!("{{{}}}", parts.join(", "))
}

/// The whole trace as JSONL.
pub fn trace_jsonl(t: &Trace) -> String {
    let env: Vec<String> = t
        .meta
        .env
        .iter()
        .map(|(f, x)| {
            let val = match x {
                Json::Int(i) => i.to_string(),
                Json::Bool(b) => b.to_string(),
                Json::Str(s) => json::quote(s),
                _ => "null".into(),
            };
            format!("{}: {val}", json::quote(f))
        })
        .collect();
    let mut out = format!(
        "{{\"meta\": {{\"version\": 1, \"document\": {}, \"steps\": {}, \"sequences\": {}, \"env\": {{{}}}}}}}\n",
        json::quote(&t.meta.document),
        t.steps.len(),
        strs(&t.meta.sequences),
        env.join(", ")
    );
    for s in &t.steps {
        out.push_str(&step_json(s));
        out.push('\n');
    }
    out
}

// ------------------------------------------------------- from the toy language

/// The toy language's one declared sequence.
const ITEMS: &str = "items";

/// Derives a trace from toy-language lines using the language's own static
/// hooks (what a real client derives from its text): reads, writes, scope
/// effects, claims and bounds. Unfolded text is not in the trace (the
/// steps' definition sets stay pending, bounded by the hints).
#[allow(clippy::too_many_lines)]
pub fn from_toy(lines: &[String], document: &str) -> Trace {
    let mut eng = Engine::new();
    let lang = super::Lang::install(&mut eng);
    let v = eng.values();
    let asts: Vec<ValueId> = lines.iter().map(|l| super::parse_stmt(v, l)).collect();
    let loops = super::Lang::declare_loops(&mut eng, &asts);
    {
        let mut p = lang.procs.write().expect("procs");
        for &ast in &asts {
            let mut d = Vec::new();
            super::proc_decls(eng.values(), ast, &mut d);
            p.add(d);
            p.items += usize::from(super::has_tag(eng.values(), ast, &["item"]));
        }
        p.tracked = true;
    }
    let v = eng.values();
    let op = &*lang.stmt;
    let mut steps = Vec::with_capacity(lines.len());
    let name = |f: Field| super::text(v, f.0);
    for (i, ast) in asts.into_iter().enumerate() {
        let ast = super::Lang::rewrite(v, &loops, ast);
        let args = [ast];
        let (class, _) = super::tag(v, ast);
        let defs = op.defs(&args, v);
        let reads = op.reads(&args, v).unwrap_or_default();
        let eff = op.effect(&args, v);
        let claim = op.claim(&args, v, &mut std::iter::empty());
        let mut s = Step {
            id: format!("L{i}"),
            class: class.clone(),
            pos: i64::try_from(i).unwrap_or(0),
            depth: None,
            reads: reads.iter().map(|&f| name(f)).collect(),
            writes: defs.as_ref().map(|d| {
                d.iter()
                    .map(|&(f, l)| Write {
                        field: name(f),
                        local: l == Locality::Local,
                    })
                    .collect()
            }),
            may_define: Bound::Any,
            scope: Scope::None,
            scope_kind: None,
            starts_implicit: None,
            closes_implicit: None,
            contributes: None,
            reads_prev: None,
            fold: None,
            claim: ClaimSpec::None,
            limit: op.claim_limit(&args, v, &mut std::iter::empty()),
            env_reads: Vec::new(),
            unfold: None,
            children: Vec::new(),
            value_dep: true,
            prior_children: None,
            cause: if defs.is_none() { Some(class.clone()) } else { None },
            rev: 0,
        };
        if defs.is_none() {
            s.may_define = match op.may_define(&args, v) {
                MayDefine::Nothing => Bound::None,
                MayDefine::Any => Bound::Any,
                MayDefine::Names(fs) => Bound::Names(
                    fs.iter()
                        .map(|&f| Write {
                            field: name(f),
                            local: true,
                        })
                        .collect(),
                ),
                MayDefine::Prefixes(ps) => Bound::Prefixes(
                    ps.iter()
                        .map(|p| Write {
                            field: p.clone(),
                            local: true,
                        })
                        .collect(),
                ),
            };
        }
        s.scope = match eff {
            Effect::None => Scope::None,
            Effect::Open => Scope::Open,
            Effect::Close => Scope::Close,
            Effect::Pending => Scope::Pending,
            Effect::Scope(b) => {
                s.scope_kind = b.kind.map(|k| super::text(v, k));
                Scope::Brackets {
                    closes: b.closes,
                    opens: b.opens,
                    before: b.before,
                }
            }
        };
        s.claim = match claim {
            Claim::None => ClaimSpec::None,
            Claim::Count(n) => ClaimSpec::Known(n),
            Claim::Pending => ClaimSpec::Until(UntilSpec {
                class: "mark".into(),
                inert: true,
                confirm: Vec::new(),
            }),
        };
        if class == "collect" {
            let a = super::tag(v, ast).1;
            let class = super::text(v, a[1]);
            let confirm: Vec<String> = super::text(v, a[2])
                .split(' ')
                .filter(|x| !x.is_empty())
                .map(String::from)
                .collect();
            let u = UntilSpec {
                class,
                inert: false,
                confirm,
            };
            s.claim = match v.as_int(a[3]).and_then(|n| usize::try_from(n).ok()) {
                Some(n) => ClaimSpec::Predicted(n, u),
                None => ClaimSpec::Until(u),
            };
        }
        if matches!(class.as_str(), "add" | "take" | "replace") {
            s.contributes = Some(ITEMS.into());
        }
        if s.reads.iter().any(|r| r == ITEMS) || class == "item" {
            s.reads_prev = Some(ITEMS.into());
        }
        s.reads.retain(|r| r != ITEMS && !r.starts_with('\u{1}'));
        steps.push(s);
    }
    // Depths from the static effects, for the strict loader.
    let mut d: Option<i64> = Some(0);
    for s in &mut steps {
        s.depth = d;
        d = match (&s.scope, d) {
            (Scope::None, x) => x,
            (Scope::Open, Some(x)) => Some(x + 1),
            (Scope::Close, Some(x)) => Some((x - 1).max(0)),
            (Scope::Brackets { closes, opens, .. }, Some(x)) => {
                Some((x - i64::from(*closes)).max(0) + i64::from(*opens))
            }
            _ => None,
        };
    }
    let mut sequences = vec![ITEMS.to_string()];
    sequences_in(&steps, &mut sequences);
    Trace {
        meta: Meta {
            document: document.into(),
            steps: steps.len(),
            sequences,
            env: Vec::new(),
        },
        steps,
    }
}

/// Edits of the three forms over a trace, for a replay without a supplied
/// `edits.jsonl`: each names a top-level step chosen by `seed`.
pub fn generated_edits(t: &Trace, seed: u64, n: usize) -> Vec<EditLine> {
    let mut rng = defuse::Rng::new(seed);
    let mut out = Vec::new();
    let mut ids: Vec<String> = t.steps.iter().map(|s| s.id.clone()).collect();
    let mut next = 0;
    for k in 0..n {
        if ids.is_empty() {
            break;
        }
        let i = rng.below(ids.len());
        let edit = match k % 3 {
            0 => {
                let s = t.steps.iter().find(|s| s.id == ids[i]).cloned();
                let ws = s.and_then(|s| s.writes).unwrap_or_default();
                Edit::ChangeWrites(ids[i].clone(), Some(ws))
            }
            1 => {
                next += 1;
                let new = Step {
                    id: format!("new{next}"),
                    class: "assign".into(),
                    pos: 0,
                    depth: None,
                    reads: Vec::new(),
                    writes: Some(vec![Write {
                        field: format!("n{next}"),
                        local: true,
                    }]),
                    may_define: Bound::Any,
                    scope: Scope::None,
                    scope_kind: None,
                    starts_implicit: None,
                    closes_implicit: None,
                    contributes: None,
                    reads_prev: None,
                    fold: None,
                    claim: ClaimSpec::None,
                    limit: None,
                    env_reads: Vec::new(),
                    unfold: None,
                    children: Vec::new(),
                    value_dep: true,
                    prior_children: None,
                    cause: None,
                    rev: 0,
                };
                ids.push(new.id.clone());
                Edit::InsertAfter(ids[i].clone(), Box::new(new))
            }
            _ => {
                let id = ids.remove(i);
                Edit::Delete(id)
            }
        };
        out.push(EditLine {
            edit,
            expect_reached: None,
        });
    }
    out
}

/// A tally of pending steps by `cause`, and how many resolved to the shape
/// their `prior` predicted.
pub fn causes(eng: &Engine, doc: &Doc) -> BTreeMap<String, (u64, u64)> {
    let mut out: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    for (s, &n) in doc.steps.iter().zip(&doc.nodes) {
        let Some(c) = &s.cause else { continue };
        let e = out.entry(c.clone()).or_default();
        e.0 += 1;
        if s.prior_children.is_some_and(|p| p == eng.children(n).len()) {
            e.1 += 1;
        }
    }
    out
}
