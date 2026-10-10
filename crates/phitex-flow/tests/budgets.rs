//! The edit budgets (`AGENTS_BRIEF.md`, "Budgets"), measured over the plain
//! documents, big and bigout at 1 and 8 workers, lean or not, and a LaTeX
//! document on the saved format at 8 workers, lean (`phitex_flow::budgets`;
//! `--budgets REPS latex` for all of them). Informational: the table is printed (`cargo test --release
//! --test budgets -- --nocapture`), nothing is asserted but that every row
//! was measured. `PHITEX_BUDGET_REPS` (5) repetitions of each edit (the
//! LaTeX document's: 1); `scripts/flow-budgets.sh` for the cluster.

use phitex_flow::budgets;

#[test]
fn edit_budgets_table() {
    let reps = std::env::var("PHITEX_BUDGET_REPS").ok().and_then(|r| r.parse().ok()).unwrap_or(5);
    let rows = budgets::run_all(reps, None);
    println!("{}", budgets::table(&rows));
    let docs = budgets::docs().len();
    assert_eq!(rows.len(), docs * 4 * budgets::EditKind::ALL.len(), "a row for each document, schedule and edit");
}

#[test]
fn latex_budgets_table() {
    if phitex_flow::fuzz::latex_dirs().is_none() {
        return;
    }
    let rows = budgets::run_all(1, Some("latex-tiny"));
    println!("{}", budgets::table(&rows));
    assert_eq!(rows.len(), budgets::EditKind::ALL.len(), "a row for each edit");
}
