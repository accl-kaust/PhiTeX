#!/bin/bash
# The formats a release publishes (release.py --formats), each taken by a
# build on a fresh cache: for each format in release.json's `formats`, a
# document of its engine that scripts/check.sh built (`% engine: X`) built by
# PhiTeX (BIN, the engine identity the formats were made by) with --shelf
# from the release served on the loopback, its cache empty. The build
# must take the published format, not make it: the format kept in the
# cache, its pack fetched, no hyphen-* pack fetched (making a format reads
# every language's patterns), and its PDF the one check.sh's build made
# (which made the format). Exits non-zero on any failure. Run after
# scripts/check.sh, in the same environment.
#
#   ROOT=<PhiTeX checkout> REL=<releases dir> LABEL=tl2026 NEW=tl2026.N \
#   BIN=<phitex> W=<check.sh's scratch> OUT=<results> shelf/scripts/check-formats.sh
set -u
root=$ROOT rel=$REL label=$LABEL new=$NEW bin=$BIN w=$W out=$OUT
port=8125
python3 "$root/shelf/serve.py" --root "$rel" --port $port > "$w/serve-formats.log" 2>&1 &
serve=$!
trap 'kill $serve 2>/dev/null' EXIT
sleep 1
export SOURCE_DATE_EPOCH=0 FORCE_SOURCE_DATE=1
fails=0 n=0
names=$(python3 -c "import json,sys;print('\n'.join(json.load(open(sys.argv[1])).get('formats') or {}))" "$rel/$label/release.json")
for name in $names; do
  engine=${name%%-*}
  # (a document check.sh built, with a PDF: the one the format made made)
  doc=$(cd "$w/docs" && for d in *.tex; do
    e=$(sed -n 's/^% engine: \([a-z]*\).*/\1/p' "$d" | head -1)
    [ "${e:-xelatex}" = "$engine" ] && [ -s "$w/runs/s-${d%.tex}/${d%.tex}.pdf" ] && { echo "$d"; break; }
  done)
  if [ -z "$doc" ]; then
    echo "format $name: no document of $engine" | tee -a "$out/summary.txt"
    fails=$((fails + 1))
    continue
  fi
  d=${doc%.tex} dir="$w/runs/f-$name" cache="$w/c-fmt-$name"
  rm -rf "$dir" "$cache"
  mkdir -p "$dir"
  cp -a "$w/docs/." "$dir/"
  (cd "$dir" && PHITEX_CACHE_DIR=$cache PHITEX_SHELF_RELEASE=$new PHITEX_SHELF_URL=http://127.0.0.1:$port/$label/ \
    timeout 900 "$bin" build --shelf --engine "$engine" -j 8 "$doc" > run.log 2>&1)
  a=$?
  packs=$(ls "$cache/shelf/$new/h" 2>/dev/null)
  hyph=$(echo "$packs" | grep -c '^hyphen-')
  n=$((n + 1))
  if [ "$a" != 0 ]; then r="exit $a"
  elif [ ! -f "$cache/formats/$name" ]; then r="the format not kept (another name made?)"
  elif ! echo "$packs" | grep -q "^fmt-$name-"; then r="its pack not fetched (made, not taken)"
  elif [ "$hyph" != 0 ]; then r="$hyph hyphen-* packs fetched (the format made)"
  elif ! cmp -s "$dir/$d.pdf" "$w/runs/s-$d/$d.pdf"; then r="its PDF is not the one built with the format made"
  else
    echo "format $name: taken by $doc on a fresh cache, $(echo "$packs" | grep -c .) packs fetched, none hyphen-*" | tee -a "$out/summary.txt"
    continue
  fi
  echo "format $name ($doc): $r" | tee -a "$out/summary.txt"
  cp "$dir/run.log" "$out/format-$name.log" 2>/dev/null
  echo "$packs" > "$out/format-$name.packs.txt"
  fails=$((fails + 1))
done
[ "$n" -gt 0 ] || { echo "no format published in $new" | tee -a "$out/summary.txt"; exit 1; }
[ "$fails" = 0 ] || { echo "FAILED: $fails of $n formats (summary.txt)" | tee -a "$out/summary.txt"; exit 1; }
