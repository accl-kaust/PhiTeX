#!/usr/bin/env bash
# Merge tex.web with the change files web2c applies when building `tex`
# (same list and order as texk/web2c/am/texmf.am) and tangle the result.
#
#   scripts/sandbox scripts/merge-web.sh
#
# Output in target/web/:
#   tex-merged.web  the source the oracle binary is built from. Sections
#                   §1–§1378 keep tex.web's numbers; web2c's additions are
#                   §1379 onwards (cite them as "merged §N").
#   tex.pool        the initial string pool (its size is observable in logs)
#   tex.p           tangled Pascal, for reference
set -euo pipefail
cd "$(dirname "$0")/.."
w2c=upstream/texlive-source/texk/web2c
out=target/web
mkdir -p "$out"

# am/texmf.am also lists $(tex_ch_synctex), but only when configured with
# --enable-tex-synctex, which TeX Live does not use (SyncTeX is on by default
# for pdfTeX/XeTeX, not for tex): the installed `tex` has no \synctex and
# its box nodes are 7 words, as in tex.web.
changes=(tex.ch enctexdir/enctex1.ch enctexdir/enctex-tex.ch enctexdir/enctex2.ch)
paths=()
for c in "${changes[@]}"; do paths+=("$w2c/$c"); done

tie -m "$out/tex-merged.web" "$w2c/tex.web" "${paths[@]}" "$w2c/tex-binpool.ch" \
  >"$out/tie.log" 2>&1 || { tail "$out/tie.log"; exit 1; }
# The pool file is only written without tex-binpool.ch (which embeds it in C).
tie -c "$out/tex-pool.ch" "$w2c/tex.web" "${paths[@]}" >>"$out/tie.log" 2>&1
(cd "$out" && tangle "../../$w2c/tex.web" tex-pool.ch >tangle.log 2>&1) ||
  { tail "$out/tangle.log"; exit 1; }

sections=$(grep -cE '^@( |\*|$|	)' "$out/tex-merged.web")
strings=$(grep -cv '^\*' "$out/tex.pool")
echo "tex-merged.web: $sections sections; tex.pool: $strings strings"

# The core embeds the pool; keep the committed copy in sync with the pins.
cp "$out/tex.pool" crates/partex-core/src/tex.pool

# pdfTeX (which includes e-TeX): the change files of pdftexdir/am/pdftex.am,
# in its order. TeX Live builds pdfTeX with SyncTeX (`pdftex_ch_synctex` in
# synctexdir/am/synctex.am), which enlarges some nodes.
pchanges=(pdftexdir/tex.ch0 tex.ch tracingstacklevels.ch partoken-102.ch partoken.ch
  locnull-optimize.ch showstream.ch zlib-fmt.ch enctexdir/enctex1.ch
  enctexdir/enctex-pdftex.ch enctexdir/enctex2.ch unbalanced-braces.ch
  synctexdir/synctex-def.ch0 synctexdir/synctex-mem.ch0 synctexdir/synctex-e-mem.ch0
  synctexdir/synctex-e-mem.ch1 synctexdir/synctex-rec.ch0 synctexdir/synctex-rec.ch1
  synctexdir/synctex-e-rec.ch0 synctexdir/synctex-pdf-rec.ch2
  pdftexdir/pdftex.ch pdftexdir/char-warning-pdftex.ch)
ppaths=()
for c in "${pchanges[@]}"; do ppaths+=("$w2c/$c"); done
tie -m "$out/pdftex-merged.web" "$w2c/pdftexdir/pdftex.web" "${ppaths[@]}" "$w2c/tex-binpool.ch" \
  >"$out/pdftex-tie.log" 2>&1 || { tail "$out/pdftex-tie.log"; exit 1; }
tie -c "$out/pdftex-pool.ch" "$w2c/pdftexdir/pdftex.web" "${ppaths[@]}" >>"$out/pdftex-tie.log" 2>&1
mkdir -p "$out/pdftex"
(cd "$out/pdftex" && tangle "../../../$w2c/pdftexdir/pdftex.web" ../pdftex-pool.ch >tangle.log 2>&1) ||
  { tail "$out/pdftex/tangle.log"; exit 1; }
psections=$(grep -cE '^@( |\*|$|	)' "$out/pdftex-merged.web")
pstrings=$(grep -cv '^\*' "$out/pdftex/pdftex.pool")
echo "pdftex-merged.web: $psections sections; pdftex.pool: $pstrings strings"
cp "$out/pdftex/pdftex.pool" crates/partex-core/src/pdftex.pool
