//! Every public `Ctx` method is classified: a read the run's log records
//! (and revalidates), a write the commit applies, or local to the run.
//! A new method fails this test until it is classified, so a query of
//! engine state cannot be added without deciding how it is recorded.

const READS: &[&str] = &[
    "use_field",
    "use_field_at",
    "use_field_before",
    "use_field_proj",
    "use_env",
    "output_of",
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
