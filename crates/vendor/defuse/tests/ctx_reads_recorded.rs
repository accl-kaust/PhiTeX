//! Every public `Ctx` method is classified: a read the run's log records
//! (and revalidates), a write the commit applies, or local to the run.
//! A new method fails this test until it is classified, so a query of
//! engine state cannot be added without deciding how it is recorded.

const READS: &[&str] = &[
    "use_field",
    "use_field_at",
    "use_field_before",
    "use_field_after_own",
    "use_field_proj",
    "use_fields",
    "use_fields_proj",
    "use_env",
    "output_of",
    "placed_output",
    "placed_output_or",
    "apply",
    "acc_range",
    "acc_range_to",
    "acc_range_after_def",
    "acc_fold",
    "acc_next",
    "scope_kind",
    "claimed",
    "following",
    "claim_stream",
    "prev_sibling_where",
    "prev_contribution",
    "contributions_before_me",
    "take_prev_contribution",
    "is_loopback",
    "define_forward",
];
const WRITES: &[&str] = &[
    "define",
    "define_with",
    "define_in_scope",
    "define_in_scope_at",
    "declare_loopback",
    "publish",
    "child",
    "follow",
    "follow_after",
    "follow_after_any",
    "open_scope",
    "close_scope",
    "contribute",
    "set_kind",
    "set_kinds",
    "set_effect",
    "narrow",
    "claim",
    "push_contribution",
    "set_env",
    "park",
    "park_continuing",
    "restart",
    "diagnostic",
];
/// The run's own inputs and state, pure memos, and hints that may not
/// change a result.
const LOCAL: &[&str] = &[
    "values",
    "args",
    "op",
    "node",
    "effect_pending",
    "parked",
    "memo_text",
    "is_speculative",
    "values_epoch",
    "tick",
];

#[test]
fn every_ctx_method_is_classified() {
    let src = include_str!("../src/clean/ctx.rs");
    let mut missing = Vec::new();
    for line in src.lines() {
        let Some(rest) = line.strip_prefix("    pub fn ") else {
            continue;
        };
        let name: String = rest.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
        if ![READS, WRITES, LOCAL].iter().any(|l| l.contains(&name.as_str())) {
            missing.push(name);
        }
    }
    assert!(
        missing.is_empty(),
        "unclassified Ctx methods (record a Pred for a read): {missing:?}"
    );
}

/// The `fn` names inside the block starting at `start` (to its closing
/// brace at column 0).
fn fns_in(src: &str, start: &str) -> Vec<String> {
    let i = src.find(start).unwrap_or_else(|| panic!("{start} not found"));
    let body = &src[i..];
    let end = body.find("\n}\n").expect("block end");
    body[..end]
        .lines()
        .filter_map(|l| l.trim_start().strip_prefix("fn "))
        .map(|r| r.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect())
        .collect()
}

#[test]
fn every_operation_hook_is_recorded_and_replayed() {
    // A hook the recorder does not forward answers its default while
    // recording: the recorded build is not the build (a client's
    // `final_only` or `contribs_complete` lost made a cycle).
    let hooks = fns_in(include_str!("../src/api.rs"), "pub trait Operation");
    let rec = fns_in(include_str!("../src/record.rs"), "impl Operation for RecOp");
    let play = fns_in(include_str!("../examples/play.rs"), "impl Operation for PlayOp");
    let missing: Vec<(&str, &String)> = hooks
        .iter()
        .flat_map(|h| [("recorder", &rec), ("player", &play)].into_iter().filter(move |(_, l)| !l.contains(h)).map(move |(w, _)| (w, h)))
        .collect();
    assert!(missing.is_empty(), "Operation hooks not forwarded: {missing:?}");
}
