#!/usr/bin/env bash
# Submits a gate on REV to accl, its job ids on stdout:
#
#   scripts/flow-gate-submit.sh REV quick|fuzz|cli|full
#
# quick: scripts/flow-gate-quick.sh, one job (-c 12, 48G, 30 min).
# fuzz:  scripts/flow-gate-fuzz.sh in four shards (plain-a, plain-b,
#        plain-j8, latex; -c 12, 48G, 1 h each).
# cli:   scripts/flow-gate-cli.sh in two parts (tikz, rest; -c 3, 16G,
#        50 min each).
# full:  fuzz and cli.
# The gate scripts are this checkout's, laid over REV's tree (a commit
# "gate scripts over REV" on top of REV), so a branch head without them
# runs the same gate. scripts/flow-gate-tally.sh JOB... joins the results.
set -euo pipefail
cd "$(dirname "$0")/.."
rev=$(git rev-parse --short=8 "${1:?REV}^{commit}")
what=${2:?quick, fuzz, cli or full}
# (REV's tree with the gate scripts of this checkout)
idx=$(mktemp)
trap 'rm -f "$idx"' EXIT
GIT_INDEX_FILE=$idx git read-tree "$rev"
for f in scripts/flow-fuzz.sh scripts/flow-fuzz-min.py scripts/flow-fuzz-tex.py scripts/flow-gate-*.sh; do
	m=100644
	[ -x "$f" ] && m=100755
	GIT_INDEX_FILE=$idx git update-index --add --cacheinfo "$m,$(git hash-object -w "$f"),$f"
done
tree=$(GIT_INDEX_FILE=$idx git write-tree)
if [ "$tree" = "$(git rev-parse "$rev^{tree}")" ]; then
	c=$rev
else
	c=$(git commit-tree "$tree" -p "$rev" -m "gate scripts over $rev (of $(git rev-parse --short=8 HEAD))")
fi
sub() { # OPTS, then the command
	local o=$1
	shift
	ACCL_COMMIT=$c ACCL_OPTS="$o --exclude=harrier" scripts/accl/accl submit cmd "$@" 2>/dev/null
}
F="-c 12 --mem=48G -t 01:00:00"
C="-c 3 --mem=16G -t 00:50:00"
case "$what" in
quick) echo "quick $(sub "-c 12 --mem=48G -t 00:30:00" bash scripts/flow-gate-quick.sh)" ;;
fuzz | cli | full)
	if [ "$what" != cli ]; then
		for s in plain-a plain-b plain-j8 latex; do echo "fuzz-$s $(sub "$F" bash scripts/flow-gate-fuzz.sh $s)"; done
	fi
	if [ "$what" != fuzz ]; then
		for p in tikz rest; do echo "cli-$p $(sub "$C" bash scripts/flow-gate-cli.sh $p)"; done
	fi
	;;
*) echo "quick, fuzz, cli or full, not $what" >&2; exit 2 ;;
esac
