//! The edit budgets (`AGENTS_BRIEF.md`, "Budgets"), measured over the plain
//! documents, big and bigout at 1 and 8 workers, lean or not
//! (`phitex_flow::budgets`). Informational: the table is printed
//! (`cargo test --release --test budgets -- --nocapture`), nothing is
//! asserted but that every row was measured. `PHITEX_BUDGET_REPS` (5)
//! repetitions of each edit; `scripts/flow-budgets.sh` for the cluster.

use phitex_flow::budgets;

#[test]
fn edit_budgets_table() {
    let reps = std::env::var("PHITEX_BUDGET_REPS").ok().and_then(|r| r.parse().ok()).unwrap_or(5);
    let rows = budgets::run_all(reps, None);
    println!("{}", budgets::table(&rows));
    let docs = budgets::docs().len();
    assert_eq!(rows.len(), docs * 4 * budgets::EditKind::ALL.len(), "a row for each document, schedule and edit");
}
