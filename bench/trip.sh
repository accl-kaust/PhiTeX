#!/usr/bin/env bash
# Knuth's TRIP test, on output only: the DVI trip.tex makes, as DVItype
# prints it (trip.typ), against Knuth's. The logs (trip.log, tripin.log,
# trip.fot) are not compared, nor the tiny memory TRIP is meant to run in:
# where trip.tex overflows on purpose, the output may differ (`baseline`
# shows where, with Knuth's own TeX at ordinary limits).
#
#   bench/trip.sh fetch             download the files (once) and check them
#   bench/trip.sh baseline          the system's tex, at its own limits
#   bench/trip.sh run ENGINE...     ENGINE DIR must write DIR/trip.dvi from
#                                   DIR/trip.tex (and trip.tfm there)
#   bench/trip.sh flow [THREADS]     phitex-flow --trip (the release build, in
#                                   scripts/sandbox): the gate is trip.typ;
#                                   the logs are diffed for navigation only
#
# The checked inputs are kept in bench/trip/ (tests/trip.rs reads them);
# each run goes to bench/trip/<name>/ (ignored by git),
# with trip.typ, trip.diff and a line of results.
set -euo pipefail
cd "$(dirname "$0")"
dir=trip
dist=https://mirrors.ctan.org/systems/knuth/dist/tex
triptrap=https://raw.githubusercontent.com/TeX-Live/texlive-source/trunk/texk/web2c/triptrap

sums() {
	cat <<'EOF'
15f15c2ca1470085299056ec89dea5f51e9fe9303ef25581b2f2eaf7809ae97b  trip.tex
93b38cc794f0c4a462667e25ef34a83552cbcdd62a42b10f739a431166525a79  trip.pl
64efc62b962c592c2973f8c45a78e9e5d473f8b9da53ee53bc275a98041675cc  trip.typ
61a653523bdccab9fd3f9aa61d170d0198c322c951938327b7daef9b70f26d8b  trip.log
ba01328756a8901d7c38162c9012014e9540322bf0963e105286f2a6ccb494cc  tripin.log
89e275ac12d025c06022e8dd6eb556b765954af2654b39ac2fbd451cf631b370  trip.fot
a3e47254ad87fc3fdba210d61764c93b021740f56465971f5a41103405add48b  tripman.tex
13c44147bc0c9aaf09c3a502a87d0262f4bd00071e68d31d98e681dea1caaffc  trip1.in
0826e9c727df598af8385d4c68ac19ddde289564e75c17f76ae1ef1be08cb30d  trip2.in
EOF
}

fetch() {
	mkdir -p "$dir"
	for f in trip.tex trip.pl trip.typ trip.log tripin.log trip.fot tripman.tex; do
		[ -s "$dir/$f" ] || curl -sfL -o "$dir/$f" "$dist/$f"
	done
	for f in trip1.in trip2.in; do
		[ -s "$dir/$f" ] || curl -sfL -o "$dir/$f" "$triptrap/$f"
	done
	(cd "$dir" && sums | sha256sum -c --quiet)
	[ -s "$dir/trip.tfm" ] || pltotf "$dir/trip.pl" "$dir/trip.tfm" >/dev/null
	echo "trip: files in bench/$dir, checked"
}

# A run directory with the inputs.
stage() {
	local out=$dir/$1
	rm -rf "$out"
	mkdir -p "$out"
	cp "$dir"/{trip.tex,trip.tfm,trip1.in,trip2.in} "$out"/
	echo "$out"
}

# DVItype as tripman says (output level 2, every page, one point a pixel),
# then the comparison, from the first page (the lines before it name the
# DVI's own preamble comment, a date).
compare() {
	local out=$1
	if [ ! -s "$out/trip.dvi" ]; then
		echo "trip: $out: no trip.dvi" >&2
		return 1
	fi
	(cd "$out" && TFMFONTS=. dvitype -output-level=2 -dpi=72.27 \
		-page-start='*.*.*.*.*.*.*.*.*.*' trip.dvi >trip.typ)
	local body='/beginning of page/,$p'
	diff <(sed -n "$body" "$dir/trip.typ") <(sed -n "$body" "$out/trip.typ") >"$out/trip.diff" || true
	local total same
	total=$(sed -n "$body" "$dir/trip.typ" | wc -l)
	same=$(diff --unchanged-line-format='x
' --old-line-format='' --new-line-format='' \
		<(sed -n "$body" "$dir/trip.typ") <(sed -n "$body" "$out/trip.typ") | wc -l || true)
	printf 'trip: %s: %d/%d lines of trip.typ match (%d.%d%%), diff in %s\n' \
		"$(basename "$out")" "$same" "$total" $((same * 100 / total)) $((same * 1000 / total % 10)) \
		"bench/$out/trip.diff" | tee "$out/result.txt"
}

case "${1:-}" in
fetch) fetch ;;
baseline)
	fetch >/dev/null
	out=$(stage baseline)
	(
		cd "$out"
		export TFMFONTS=. TEXFORMATS=. TEXINPUTS=.
		tex -ini -interaction=batchmode <trip1.in >/dev/null 2>&1 || true
		tex -interaction=batchmode <trip2.in >/dev/null 2>&1 || true
	)
	compare "$out"
	;;
run)
	shift
	[ $# -gt 0 ] || { echo "usage: bench/trip.sh run ENGINE..." >&2; exit 2; }
	fetch >/dev/null
	out=$(stage engine)
	"$@" "$out" || echo "trip: the engine failed ($?)" >&2
	compare "$out"
	;;
flow)
	fetch >/dev/null
	out=$(stage flow)
	../scripts/sandbox ../target/release/phitex-flow --trip "$out" "${2:-1}" || echo "trip: phitex-flow failed ($?)" >&2
	for f in tripin.log trip.log trip.fot; do
		diff "$dir/$f" "$out/$f" >"$out/$f.diff" && echo "trip: $f identical" || echo "trip: $f differs (diagnostics only): $(grep -c '^[<>]' "$out/$f.diff") lines, bench/$out/$f.diff"
	done
	compare "$out"
	;;
*)
	sed -n '2,18p' "$0"
	exit 2
	;;
esac
