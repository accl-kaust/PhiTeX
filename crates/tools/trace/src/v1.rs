//! Format v1's rules, as the replay client's strict loader applies them
//! (`crates/vendor/defuse/examples/common/trace.rs`, read, not linked):
//! the meta line, every step's keys and types (nested steps included),
//! unique ids, strictly increasing `pos` and consistent `depth` at the top
//! level, and edits that name a top-level step present when they apply.
//! `phitex-trace validate` runs these; a rejection names the line.

use std::collections::HashSet;
use std::io::BufRead;

use serde_json::{Map, Value};

/// A line as JSON (nested `children` run deeper than `serde_json`'s default
/// recursion limit).
fn parse(text: &str) -> Result<Value, serde_json::Error> {
    use serde::Deserialize;
    let mut d = serde_json::Deserializer::from_str(text);
    d.disable_recursion_limit();
    Value::deserialize(&mut d)
}

fn fail(line: usize, what: impl std::fmt::Display) -> String {
    format!("line {line}: {what}")
}

fn names(line: usize, key: &str, v: &Value) -> Result<(), String> {
    let a = v
        .as_array()
        .ok_or_else(|| fail(line, format!("`{key}` must be an array of strings")))?;
    if a.iter().all(|x| x.as_str().is_some_and(|s| !s.is_empty())) {
        Ok(())
    } else {
        Err(fail(line, format!("`{key}` must hold non-empty strings")))
    }
}

fn writes(line: usize, key: &str, v: &Value, name_key: &str) -> Result<(), String> {
    let a = v
        .as_array()
        .ok_or_else(|| fail(line, format!("`{key}` must be an array")))?;
    for w in a {
        let o = w
            .as_object()
            .ok_or_else(|| fail(line, format!("`{key}` entries must be objects")))?;
        if let Some(k) = o.keys().find(|k| *k != name_key && *k != "locality") {
            return Err(fail(line, format!("`{key}` entry has unknown key {k:?}")));
        }
        if o.get(name_key)
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
        {
            return Err(fail(
                line,
                format!("`{key}` entry needs a non-empty {name_key:?}"),
            ));
        }
        if !matches!(
            o.get("locality").and_then(Value::as_str),
            Some("local" | "global")
        ) {
            return Err(fail(line, format!("`{key}` entry needs \"locality\"")));
        }
    }
    Ok(())
}

fn until(line: usize, v: &Value) -> Result<bool, String> {
    let o = v
        .as_object()
        .ok_or_else(|| fail(line, "`until` must be an object"))?;
    let mut inert = false;
    for (k, x) in o {
        match k.as_str() {
            "class" => {
                if x.as_str().is_none_or(str::is_empty) {
                    return Err(fail(line, "`until.class` must be a non-empty string"));
                }
            }
            "inert" => {
                inert = x
                    .as_bool()
                    .ok_or_else(|| fail(line, "`until.inert` must be a bool"))?;
            }
            "confirm" => {
                if !x.is_string() {
                    names(line, "until.confirm", x)?;
                }
            }
            _ => return Err(fail(line, format!("`until` has unknown key {k:?}"))),
        }
    }
    if !o.contains_key("class") {
        return Err(fail(line, "`until` needs a class"));
    }
    Ok(inert)
}

fn uint(line: usize, key: &str, v: &Value) -> Result<u64, String> {
    v.as_u64()
        .ok_or_else(|| fail(line, format!("`{key}` must be a non-negative integer")))
}

/// A step's scope effect, for the depth check: (closes, opens), or `None`
/// for a pending one.
type Effect = Option<(i64, i64)>;

/// What the top-level checks need of a step.
#[derive(Debug)]
pub struct Checked {
    pub id: String,
    pub pos: i64,
    pub depth: Option<i64>,
    effect: Effect,
}

fn opt_str(line: usize, key: &str, x: &Value) -> Result<(), String> {
    match x {
        Value::Null => Ok(()),
        Value::String(t) if !t.is_empty() => Ok(()),
        _ => Err(fail(
            line,
            format!("`{key}` must be a non-empty string or null"),
        )),
    }
}

/// Checks one step (and its children), collecting every id into `ids`.
///
/// # Errors
///
/// The first rule broken, with `line`.
#[allow(clippy::too_many_lines, clippy::many_single_char_names)]
pub fn step<S: std::hash::BuildHasher>(
    line: usize,
    v: &Value,
    ids: &mut HashSet<String, S>,
) -> Result<Checked, String> {
    let o: &Map<String, Value> = v
        .as_object()
        .ok_or_else(|| fail(line, "a step must be an object"))?;
    let mut c = Checked {
        id: String::new(),
        pos: 0,
        depth: None,
        effect: Some((0, 0)),
    };
    let (mut has_pos, mut has_reads, mut has_writes, mut has_bound) = (false, false, false, false);
    let mut writes_null = false;
    let mut opens = false;
    let mut pending = false;
    let (mut kind, mut implicit) = (false, false);
    for (k, x) in o {
        match k.as_str() {
            "id" => {
                x.as_str()
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| fail(line, "`id` is required"))?
                    .clone_into(&mut c.id);
            }
            "class" => {
                if x.as_str().is_none_or(str::is_empty) {
                    return Err(fail(line, "`class` is required"));
                }
            }
            "pos" => {
                c.pos = x
                    .as_i64()
                    .ok_or_else(|| fail(line, "`pos` must be an integer"))?;
                has_pos = true;
            }
            "depth" => {
                c.depth = Some(
                    x.as_i64()
                        .ok_or_else(|| fail(line, "`depth` must be an integer"))?,
                );
            }
            "reads" => {
                names(line, k, x)?;
                has_reads = true;
            }
            "writes" => {
                if x.is_null() {
                    writes_null = true;
                } else {
                    writes(line, k, x, "field")?;
                }
                has_writes = true;
            }
            "may_define" => {
                has_bound = true;
                match x {
                    Value::String(t) if t == "none" || t == "any" => {}
                    Value::Object(m) if m.len() == 1 && m.contains_key("names") => {
                        writes(line, "may_define.names", &m["names"], "name")?;
                    }
                    Value::Object(m) if m.len() == 1 && m.contains_key("prefixes") => {
                        writes(line, "may_define.prefixes", &m["prefixes"], "prefix")?;
                    }
                    _ => {
                        return Err(fail(
                            line,
                            "`may_define` must be \"none\", \"any\", {names} or {prefixes}",
                        ));
                    }
                }
            }
            "scope" => match x {
                Value::String(t) if t == "none" => {}
                Value::String(t) if t == "open" => {
                    c.effect = Some((0, 1));
                    opens = true;
                }
                Value::String(t) if t == "close" => c.effect = Some((1, 0)),
                Value::String(t) if t == "pending" => {
                    c.effect = None;
                    pending = true;
                }
                Value::Object(m) => {
                    let (mut cl, mut op) = (0, 0);
                    for (bk, bv) in m {
                        match bk.as_str() {
                            "closes" | "opens" => {
                                let n = uint(line, "scope.closes", bv)?;
                                if n > 3 {
                                    return Err(fail(line, format!("`scope.{bk}` is at most 3")));
                                }
                                let n = i64::try_from(n).unwrap_or(0);
                                if bk == "closes" {
                                    cl = n;
                                } else {
                                    op = n;
                                }
                            }
                            "before" => {
                                bv.as_bool()
                                    .ok_or_else(|| fail(line, "`scope.before` must be a bool"))?;
                            }
                            _ => return Err(fail(line, format!("`scope` has unknown key {bk:?}"))),
                        }
                    }
                    c.effect = Some((cl, op));
                    opens = op > 0;
                }
                _ => {
                    return Err(fail(
                        line,
                        "`scope` must be \"none\", \"open\", \"close\", \"pending\" or {closes, opens, before}",
                    ));
                }
            },
            "scope_kind" => {
                opt_str(line, k, x)?;
                kind = !x.is_null();
            }
            "starts_implicit" | "closes_implicit" => {
                opt_str(line, k, x)?;
                implicit |= !x.is_null();
            }
            "contributes" | "reads_prev_contribution" | "cause" => opt_str(line, k, x)?,
            "fold" => match x {
                Value::Null => {}
                Value::Object(m) => {
                    if let Some(fk) = m.keys().find(|k| *k != "sequence" && *k != "step_class") {
                        return Err(fail(line, format!("`fold` has unknown key {fk:?}")));
                    }
                    let ok = |k: &str| {
                        m.get(k)
                            .and_then(Value::as_str)
                            .is_some_and(|t| !t.is_empty())
                    };
                    if !ok("sequence") || !ok("step_class") {
                        return Err(fail(line, "`fold` needs \"sequence\" and \"step_class\""));
                    }
                }
                _ => return Err(fail(line, "`fold` must be an object or null")),
            },
            "claim" => match x {
                Value::Null => {}
                Value::Object(m) if m.len() == 1 => {
                    let Some((ck, cv)) = m.iter().next() else {
                        unreachable!("one entry")
                    };
                    match ck.as_str() {
                        "known" => {
                            uint(line, "claim.known", cv)?;
                        }
                        "count_from" => {
                            if cv.as_str().is_none_or(str::is_empty) {
                                return Err(fail(line, "`claim.count_from` must be a field"));
                            }
                        }
                        "until" => {
                            until(line, cv)?;
                        }
                        "predicted" => {
                            let p = cv
                                .as_object()
                                .ok_or_else(|| fail(line, "`claim.predicted` must be an object"))?;
                            if let Some(pk) = p.keys().find(|k| *k != "n" && *k != "until") {
                                return Err(fail(
                                    line,
                                    format!("`claim.predicted` has unknown key {pk:?}"),
                                ));
                            }
                            uint(
                                line,
                                "claim.predicted.n",
                                p.get("n").unwrap_or(&Value::Null),
                            )?;
                            if until(line, p.get("until").unwrap_or(&Value::Null))? {
                                return Err(fail(
                                    line,
                                    "a predicted claim is a stream claim (`inert: false`)",
                                ));
                            }
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
            },
            "limit" => {
                uint(line, k, x)?;
            }
            "env_reads" => names(line, k, x)?,
            "unfold" => {
                if !matches!(x, Value::Null)
                    && !x
                        .as_str()
                        .is_some_and(|t| t == "defines_nothing" || t == "unknown")
                {
                    return Err(fail(
                        line,
                        "`unfold` must be \"defines_nothing\", \"unknown\" or null",
                    ));
                }
            }
            "children" => match x {
                Value::Null => {}
                Value::Array(a) => {
                    for ch in a {
                        step(line, ch, ids)?;
                    }
                }
                _ => return Err(fail(line, "`children` must be an array of steps or null")),
            },
            "value_dep" => {
                x.as_bool()
                    .ok_or_else(|| fail(line, "`value_dep` must be a bool"))?;
            }
            "prior" => match x {
                Value::Null => {}
                Value::Object(m) if m.len() == 1 && m.contains_key("children") => {
                    uint(line, "prior.children", &m["children"])?;
                }
                _ => return Err(fail(line, "`prior` must be {\"children\": n} or null")),
            },
            _ => return Err(fail(line, format!("unknown step key {k:?}"))),
        }
    }
    if c.id.is_empty() {
        return Err(fail(line, "`id` is required"));
    }
    if !o.contains_key("class") {
        return Err(fail(line, "`class` is required"));
    }
    if !has_pos {
        return Err(fail(line, "`pos` is required"));
    }
    if !has_reads {
        return Err(fail(line, "`reads` is required"));
    }
    if !has_writes && !has_bound {
        return Err(fail(
            line,
            "`writes` (or `may_define` for a pending set) is required",
        ));
    }
    if has_bound && has_writes && !writes_null {
        return Err(fail(line, "`may_define` only goes with `writes: null`"));
    }
    if kind && !(opens || pending) {
        return Err(fail(line, "`scope_kind` needs a scope that opens"));
    }
    if implicit && !pending {
        return Err(fail(
            line,
            "an implicit starter or closer has `scope: \"pending\"`",
        ));
    }
    if !ids.insert(c.id.clone()) {
        return Err(fail(line, format!("duplicate step id {:?}", c.id)));
    }
    Ok(c)
}

/// Checks a trace file. Returns the top-level ids, in order.
///
/// # Errors
///
/// The first rule broken, with its line.
#[allow(clippy::many_single_char_names)]
pub fn trace<R: BufRead>(reader: R) -> Result<Vec<String>, String> {
    let mut lines = reader
        .lines()
        .enumerate()
        .filter(|(_, l)| l.as_ref().map_or(true, |l| !l.trim().is_empty()));
    let (i, first) = lines.next().ok_or("empty trace")?;
    let first = first.map_err(|e| e.to_string())?;
    let v: Value = parse(&first).map_err(|e| fail(i + 1, e))?;
    let m = v
        .get("meta")
        .and_then(Value::as_object)
        .ok_or_else(|| fail(i + 1, "the first line must be {\"meta\": {...}}"))?;
    let mut declared = 0;
    for (k, x) in m {
        match k.as_str() {
            "version" => {
                if x.as_i64() != Some(1) {
                    return Err(fail(i + 1, "`meta.version` must be 1"));
                }
            }
            "document" => {
                x.as_str()
                    .ok_or_else(|| fail(i + 1, "`meta.document` must be a string"))?;
            }
            "steps" => declared = uint(i + 1, "meta.steps", x)?,
            "sequences" => names(i + 1, "meta.sequences", x)?,
            "env" => {
                let e = x
                    .as_object()
                    .ok_or_else(|| fail(i + 1, "`meta.env` must be an object"))?;
                if let Some((f, _)) = e
                    .iter()
                    .find(|(_, v)| !(v.is_i64() || v.is_string() || v.is_boolean()))
                {
                    return Err(fail(
                        i + 1,
                        format!("`meta.env.{f}` must be an integer, string or bool"),
                    ));
                }
            }
            _ => return Err(fail(i + 1, format!("unknown meta key {k:?}"))),
        }
    }
    if !m.contains_key("version") {
        return Err(fail(i + 1, "`meta.version` must be 1"));
    }
    let mut ids = HashSet::new();
    let mut tops = Vec::new();
    let mut depth = Some(0i64);
    let mut last_pos: Option<i64> = None;
    for (i, l) in lines {
        let line = i + 1;
        let l = l.map_err(|e| e.to_string())?;
        let v: Value = parse(&l).map_err(|e| fail(line, e))?;
        if v.get("meta").is_some() {
            return Err(fail(line, "a second meta line"));
        }
        let s = step(line, &v, &mut ids)?;
        if last_pos.is_some_and(|p| s.pos <= p) {
            return Err(fail(
                line,
                format!("`pos` {} is not after the previous step's", s.pos),
            ));
        }
        last_pos = Some(s.pos);
        if let (Some(d), Some(want)) = (depth, s.depth)
            && d != want
        {
            return Err(fail(
                line,
                format!("`depth` {want} but the scopes before give {d}"),
            ));
        }
        depth = match (s.effect, depth) {
            (Some((cl, op)), Some(d)) => {
                if d - cl < 0 {
                    return Err(fail(line, "closes more scopes than are open"));
                }
                Some(d - cl + op)
            }
            _ => None,
        };
        tops.push(s.id);
    }
    if declared != tops.len() as u64 {
        return Err(format!(
            "meta.steps is {declared} but the file has {} steps",
            tops.len()
        ));
    }
    Ok(tops)
}

/// Checks an edits file against the trace's top-level ids, applying the
/// edits in order (an edit must name a top-level step present then).
///
/// # Errors
///
/// The first rule broken, with its line.
#[allow(clippy::many_single_char_names)]
pub fn edits<R: BufRead, S: std::hash::BuildHasher>(
    reader: R,
    mut tops: Vec<String>,
    all: &mut HashSet<String, S>,
) -> Result<u64, String> {
    let mut n = 0;
    for (i, l) in reader.lines().enumerate() {
        let line = i + 1;
        let l = l.map_err(|e| e.to_string())?;
        if l.trim().is_empty() {
            continue;
        }
        let v: Value = parse(&l).map_err(|e| fail(line, e))?;
        let o = v
            .as_object()
            .ok_or_else(|| fail(line, "an edit must be an object"))?;
        let op = o
            .get("op")
            .and_then(Value::as_str)
            .ok_or_else(|| fail(line, "`op` is required"))?;
        let target = o
            .get("step")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| fail(line, "`step` is required"))?;
        if let Some(e) = o.get("expect") {
            let eo = e
                .as_object()
                .ok_or_else(|| fail(line, "`expect` must be an object"))?;
            for (k, x) in eo {
                if k != "steps_reached" {
                    return Err(fail(line, format!("`expect` has unknown key {k:?}")));
                }
                uint(line, "expect.steps_reached", x)?;
            }
        }
        let allowed: &[&str] = match op {
            "change_writes" => &["op", "step", "writes", "expect"],
            "change_reads" => &["op", "step", "reads", "expect"],
            "insert_after" => &["op", "step", "new", "expect"],
            "delete" => &["op", "step", "expect"],
            _ => return Err(fail(line, format!("unknown op {op:?}"))),
        };
        if let Some(k) = o.keys().find(|k| !allowed.contains(&k.as_str())) {
            return Err(fail(line, format!("`{op}` has unknown key {k:?}")));
        }
        let at = tops
            .iter()
            .position(|t| t == target)
            .ok_or_else(|| fail(line, format!("no top-level step {target:?}")))?;
        match op {
            "change_writes" => match o.get("writes") {
                None => return Err(fail(line, "`writes` is required")),
                Some(Value::Null) => {}
                Some(w) => writes(line, "writes", w, "field")?,
            },
            "change_reads" => names(
                line,
                "reads",
                o.get("reads")
                    .ok_or_else(|| fail(line, "`reads` is required"))?,
            )?,
            "insert_after" => {
                let s = step(
                    line,
                    o.get("new")
                        .ok_or_else(|| fail(line, "`new` is required"))?,
                    all,
                )?;
                tops.insert(at + 1, s.id);
            }
            _ => {
                tops.remove(at);
            }
        }
        n += 1;
    }
    Ok(n)
}

/// Every id of a trace file (nested included).
///
/// # Errors
///
/// A malformed line.
pub fn all_ids<R: BufRead>(reader: R) -> Result<HashSet<String>, String> {
    let mut ids = HashSet::new();
    for (i, l) in reader.lines().enumerate().skip(1) {
        let l = l.map_err(|e| e.to_string())?;
        if l.trim().is_empty() {
            continue;
        }
        let v: Value = parse(&l).map_err(|e| fail(i + 1, e))?;
        step(i + 1, &v, &mut ids)?;
    }
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::{edits, trace};
    use std::collections::HashSet;

    const META: &str =
        r#"{"meta":{"version":1,"document":"d","steps":2,"sequences":["vlist"],"env":{"time":1}}}"#;

    fn file(steps: &[&str]) -> String {
        let mut s = String::from(META);
        for x in steps {
            s.push('\n');
            s.push_str(x);
        }
        s
    }

    const A: &str = r#"{"id":"s0","class":"\\def","pos":1,"reads":[],"writes":[{"field":"cs:\\x","locality":"local"}],"scope":"open"}"#;
    const B: &str = r#"{"id":"s1","class":"\\foo","pos":2,"reads":["cs:\\x"],"writes":null,"may_define":{"names":[{"name":"cs:\\y","locality":"global"}]},"claim":{"predicted":{"n":0,"until":{"class":"char","inert":false,"confirm":["cs:\\foo"]}}},"children":[{"id":"s2","class":"}","pos":0,"reads":[],"writes":[],"scope":"close"}]}"#;

    #[test]
    fn the_rules_hold() {
        assert_eq!(
            trace(file(&[A, B]).as_bytes()),
            Ok(vec!["s0".into(), "s1".into()])
        );
        // pos not increasing
        assert!(trace(file(&[A, &B.replace("\"pos\":2", "\"pos\":1")]).as_bytes()).is_err());
        // duplicate id, nested
        assert!(trace(file(&[A, &B.replace("\"s2\"", "\"s0\"")]).as_bytes()).is_err());
        // may_define with writes
        assert!(
            trace(
                file(&[
                    &A.replace("}],\"scope", "}],\"may_define\":\"any\",\"scope"),
                    B
                ])
                .as_bytes()
            )
            .is_err()
        );
        // unknown key
        assert!(trace(file(&[&A.replace("\"scope\"", "\"scopes\""), B]).as_bytes()).is_err());
        // a close at depth 0
        assert!(trace(file(&[&A.replace("\"open\"", "\"close\""), B]).as_bytes()).is_err());
        // inert predicted claim
        assert!(
            trace(file(&[A, &B.replace("\"inert\":false", "\"inert\":true")]).as_bytes()).is_err()
        );
        // an implicit starter without a pending scope
        assert!(
            trace(
                file(&[
                    &A.replace("\"scope\":\"open\"", "\"starts_implicit\":\"H\""),
                    B
                ])
                .as_bytes()
            )
            .is_err()
        );
        // meta.steps
        assert!(trace(file(&[A]).as_bytes()).is_err());
        let tops = vec!["s0".to_owned(), "s1".to_owned()];
        let e = "{\"op\":\"delete\",\"step\":\"s1\",\"expect\":{\"steps_reached\":3}}\n{\"op\":\"delete\",\"step\":\"s1\"}\n";
        assert!(edits(e.as_bytes(), tops.clone(), &mut HashSet::new()).is_err());
        let e = "{\"op\":\"insert_after\",\"step\":\"s0\",\"new\":{\"id\":\"e0\",\"class\":\"inserted\",\"pos\":0,\"reads\":[],\"writes\":[]}}\n{\"op\":\"delete\",\"step\":\"e0\"}\n";
        assert_eq!(edits(e.as_bytes(), tops, &mut HashSet::new()), Ok(2));
    }
}
