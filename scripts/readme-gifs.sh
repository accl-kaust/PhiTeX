#!/usr/bin/env bash
# Renders the README's animations (assets/banner.html, cli.html, everywhere.html) to GIFs, light and dark, in
# assets/readme/: captured by each page's own clock (scripts/readme-capture.mjs), then 64 colours, unchanged frames
# merged into longer ones, and only the changed rectangle stored per frame.
# Usage: scripts/readme-gifs.sh [banner cli everywhere]
set -euo pipefail
cd "$(dirname "$0")/.."
fps=15 scale=0.75                     # 1600 px stages → 1200 px GIFs, sharp at GitHub's README width
names=("$@"); [ $# -gt 0 ] || names=(banner cli everywhere)
work=$(mktemp -d); trap 'rm -rf "$work"' EXIT
for name in "${names[@]}"; do
  for theme in light dark; do
    suffix=; [ $theme = light ] || suffix=-$theme
    node scripts/readme-capture.mjs "assets/$name.html" $theme $fps $scale "$work/$name-$theme" >/dev/null
    frames="$work/$name-$theme/f%04d.png" pal="$work/$name-$theme.png" gif="assets/readme/$name$suffix.gif"
    ffmpeg -loglevel error -y -framerate $fps -i "$frames" -vf "palettegen=max_colors=64:stats_mode=full:reserve_transparent=0" "$pal"
    # GitHub's page colour exactly in the palette: the palette's nearest colour to it becomes it, so the GIF has no edge
    bg=$([ $theme = light ] && echo ffffff || echo 0d1117)
    xy=$(magick "$pal" txt:- | awk -v bg=$bg 'NR > 1 { split($1, p, /[,:]/); h = tolower(substr($3, 2, 6));
      d = 0; for (i = 0; i < 3; i++) { a = strtonum("0x" substr(h, 2*i+1, 2)); b = strtonum("0x" substr(bg, 2*i+1, 2)); d += (a-b)^2 }
      if (best == "" || d < best) { best = d; at = p[1] "," p[2] } } END { print at }')
    magick "$pal" -fill "#$bg" -draw "point $xy" "$pal"
    ffmpeg -loglevel error -y -framerate $fps -i "$frames" -i "$pal" -lavfi \
      "mpdecimate=hi=64:lo=64:frac=0[v];[v][1:v]paletteuse=dither=none:diff_mode=rectangle" -fps_mode vfr -loop 0 "$gif"
    echo "$gif $(( $(stat -c%s "$gif") / 1024 )) KiB, edge #$(magick "$gif[0]" -format '%[hex:p{0,0}]' info: | tr A-F a-f)"
  done
done
