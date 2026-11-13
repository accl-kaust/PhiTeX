#!/bin/bash
# (temporary, agent editpdf) phitex-cli's unit tests and its cli tests, the new watch test REPS more times.
set -u; cd "$(dirname "$0")/.."; S=$r/summary.txt; REPS=${1:-5}
T="cargo test -q --profile fastdev -p phitex-cli"
$T --bin phitex > $r/unit.txt 2>&1; echo "unit: exit $? $(grep -E '^test result' $r/unit.txt)" | tee -a $S; grep -E "^test .* FAILED|panicked" $r/unit.txt | head | tee -a $S
$T --test tests cli:: > $r/cli.txt 2>&1; echo "cli: exit $? $(grep -E '^test result' $r/cli.txt)" | tee -a $S; grep -E "^test .* FAILED|panicked" $r/cli.txt | head | tee -a $S
for i in $(seq $REPS); do $T --test tests cli::watch_quit_leaves_the_exact_pdf -- --exact > $r/q-$i.txt 2>&1; echo "watch_quit #$i: exit $? $(grep -E '^test result' $r/q-$i.txt)" | tee -a $S; done
