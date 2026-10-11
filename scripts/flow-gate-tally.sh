#!/usr/bin/env bash
# Joins a sharded gate's results (scripts/flow-gate-submit.sh): the jobs'
# headers, the fuzz shards' seeds and failures summed by thread count and
# document, their classes summed, and the CLI parts' tests.
#
#   scripts/flow-gate-tally.sh JOB...
#
# Reads ~/code/flinner/partex-phitex-runs/cmd-JOB/ (where
# `scripts/accl/accl wait JOB` fetches a job's results).
set -uo pipefail
runs=${RUNS:-$HOME/code/flinner/partex-phitex-runs}
dirs=()
for j in "$@"; do
	d="$runs/cmd-$j"
	[ -d "$d" ] || { echo "job $j: no results in $d (accl wait $j first)"; continue; }
	echo "job $j: $(head -1 "$d/summary.txt" 2>/dev/null || echo '(no summary)')"
	dirs+=("$d")
done
[ ${#dirs[@]} -gt 0 ] || exit 1
sums=$(find "${dirs[@]}" -mindepth 2 -maxdepth 2 -path '*/fuzz-*/summary.txt' | sort)
if [ -n "$sums" ]; then
	echo "== fuzz"
	# shellcheck disable=SC2086
	awk '
		/^== / { t = $2; next }
		/ seeds: / { seeds[t] += $1; pass[t] += $3; ts[t] = 1; next }
		/^  [^ ]+ +failed / { k = t SUBSEP $1; fail[k] += $3 + 0; over[k] += $5 + 0; ds[k] = 1 }
		END {
			n = split("1 8 latex", order, " ")
			for (i = 1; i <= n; i++) {
				t = order[i]
				if (!(t in ts)) continue
				printf "%s: %d seeds: %d passed, %d failed or over-invalidated\n", t == "latex" ? "latex" : t " thread" (t == 1 ? "" : "s"), seeds[t], pass[t], seeds[t] - pass[t]
				for (k in ds) { split(k, a, SUBSEP); if (a[1] == t) printf "  %-14s failed %d, over-invalidated %d\n", a[2], fail[k], over[k] }
			}
		}' $sums
	echo "-- classes"
	find "${dirs[@]}" -mindepth 2 -maxdepth 2 -path '*/fuzz-*/classes-count.txt' -exec cat {} + |
		awk '{ n = $1; $1 = ""; c[substr($0, 2)] += n } END { for (k in c) printf "%7d %s\n", c[k], k }' | sort -rn
fi
for d in "${dirs[@]}"; do
	[ -f "$d/cli.txt" ] || continue
	echo "== $(head -1 "$d/summary.txt")"
	grep -hE '^test result' "$d/cli.txt" "$d/flow-pdf.txt" "$d/synctex.txt" 2>/dev/null
	grep -hE '^test .* FAILED$' "$d/cli.txt" "$d/flow-pdf.txt" "$d/synctex.txt" 2>/dev/null || true
done
exit 0
