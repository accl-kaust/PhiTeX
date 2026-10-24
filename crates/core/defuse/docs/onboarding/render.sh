#!/bin/sh
# Renders the deck to defuse-onboarding.pdf: one 1920x1080 screenshot per
# slide (index.html?static shows every fragment and the final frame of every
# animation), joined into a PDF. Runs chromium and ImageMagick in bubblewrap.
cd "$(dirname "$0")" || exit 1
N=$(grep -c '<section' index.html)
mkdir -p tmp-shots && rm -f tmp-shots/*.png
exec /home/lambda/.local/bin/bw sh -c '
for i in $(seq 0 $(('"$N"' - 1))); do
  chromium --headless=new --no-sandbox --disable-gpu --user-data-dir="$PWD/tmp-shots/prof" \
    --hide-scrollbars --force-device-scale-factor=1.5 --window-size=1280,720 \
    --virtual-time-budget=4000 --screenshot="$PWD/$(printf tmp-shots/s%02d.png $i)" \
    "file://$PWD/index.html?static#/$i" >/dev/null 2>&1
done
magick tmp-shots/s*.png -quality 92 defuse-onboarding.pdf && rm -rf tmp-shots .cache'
