//! phitex-diff for the browser: `ph_diff` reads a JSON request from the
//! buffer the host filled (`ph_alloc`), and leaves a JSON reply for
//! `ph_out`. Request: `{"old": {path: text}, "new": {path: text}, "main":
//! "main.tex", "markup": latexdiff's --type ("underline", "cfont", …),
//! "subtype": its --subtype ("safe", "color", "marker", …), "add_color",
//! "del_color": an xcolor name or "#RRGGBB"}`.
//! Reply: `{"tex": "…", "changes": [{kind, old: {file, start, end}, new:
//! {…}, old_text, new_text, section, out: [start, end]}], "files": [path],
//! "map": [[out, len, file, src]]}` (`map`: phitex-diff's `MapRun`, the runs
//! of the diff's text that are the user's new text as it is, `file` an
//! index into `files`), or `{"error": "…"}`. Offsets are UTF-8 bytes, as
//! phitex-diff gives them.

use std::collections::BTreeMap;

use phitex_diff::{ChangeKind, Color, Markup, Options, Subtype, diff};
use serde_json::{Value, json};

// (the exports: raw pointers from the host, its only unsafe code)
#[allow(
    unsafe_code,
    reason = "the host's buffers: each block's SAFETY says why it is sound"
)]
mod abi;

fn files(v: &Value) -> BTreeMap<String, String> {
    v.as_object()
        .map(|o| {
            o.iter()
                .filter_map(|(k, t)| Some((k.clone(), t.as_str()?.to_owned())))
                .collect()
        })
        .unwrap_or_default()
}

/// The reply to a request (its JSON bytes).
fn answer(req: &[u8]) -> Value {
    let Ok(r) = serde_json::from_slice::<Value>(req) else {
        return json!({ "error": "bad request" });
    };
    // (latexdiff's names, any case; a color an xcolor name or #RRGGBB, checked: nothing else reaches the preamble)
    let color = |k: &str| match r[k].as_str() {
        None | Some("") => Ok(None),
        Some(c) => Color::parse(c).map(Some),
    };
    let (add_color, del_color) = match (color("add_color"), color("del_color")) {
        (Ok(a), Ok(d)) => (a, d),
        (Err(e), _) | (_, Err(e)) => return json!({ "error": format!("color: {e}") }),
    };
    let opts = Options {
        markup: r["markup"]
            .as_str()
            .and_then(Markup::parse)
            .unwrap_or_default(),
        subtype: r["subtype"]
            .as_str()
            .and_then(Subtype::parse)
            .unwrap_or_default(),
        add_color,
        del_color,
        ..Options::default()
    };
    let main = r["main"].as_str().unwrap_or("main.tex");
    match diff(&files(&r["old"]), &files(&r["new"]), main, &opts) {
        Err(e) => json!({ "error": e.to_string() }),
        Ok(d) => json!({
            "tex": d.tex,
            "changes": d.changes.iter().map(|c| json!({
                "kind": match c.kind { ChangeKind::Add => "add", ChangeKind::Del => "del", ChangeKind::Change => "change" },
                "old": { "file": c.old.file, "start": c.old.start, "end": c.old.end },
                "new": { "file": c.new.file, "start": c.new.start, "end": c.new.end },
                "old_text": c.old_text,
                "new_text": c.new_text,
                "section": c.section,
                "out": [c.out.start, c.out.end],
            })).collect::<Vec<_>>(),
            // (the diff's text that is the user's new text as it is: back to their files)
            "files": d.files,
            "map": d.map.iter().map(|r| [r.out, r.len, r.file, r.src]).collect::<Vec<_>>(),
        }),
    }
}
