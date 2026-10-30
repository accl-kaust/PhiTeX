#!/usr/bin/env bash
# Renders assets/readme-art.html to the README's PNGs (light and dark, 2×) in assets/readme/.
# The same files are copied to phitex.org. Usage: scripts/readme-art.sh [art ...]
set -euo pipefail
cd "$(dirname "$0")/.."
arts=("$@"); [ $# -gt 0 ] || arts=(edit exact editors offline texlive cores exact-wide how)
for art in "${arts[@]}"; do
  case $art in exact-wide|how) size=1600,520 ;; *) size=600,340 ;; esac
  for theme in light dark; do
    suffix=; [ $theme = light ] || suffix=-$theme
    out=assets/readme/$art$suffix.png
    chromium --headless=new --disable-gpu --hide-scrollbars --force-device-scale-factor=2 \
      --default-background-color=00000000 --window-size=$size --virtual-time-budget=3000 \
      --screenshot="$PWD/$out" "file://$PWD/assets/readme-art.html?art=$art&theme=$theme" 2>/dev/null
    echo "$out"
  done
done
