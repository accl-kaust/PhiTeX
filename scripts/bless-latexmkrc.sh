#!/bin/bash
# Bless the .latexmkrc fixtures (oracle/fixtures/latexmkrc, see
# crates/cli/tests/cli.rs `latexmkrc_*`): each project built from nothing
# by TeX Live's `latexmk -pdf`, with only the project's rc files (an empty
# HOME, no system rc file), under /tmp/tex with SOURCE_DATE_EPOCH=0:
#   course-root.pdf  latexmk -pdf lab1-setup/lab1.tex   in course/
#   course-lab.pdf   latexmk -pdf lab1.tex              in course/lab1-setup/
#   shared.pdf       latexmk -pdf paper.tex             in shared/paper/
# Usage: scripts/bless-latexmkrc.sh   (run under nice -n 19 ionice -c 3)
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
fx=$root/oracle/fixtures/latexmkrc
export SOURCE_DATE_EPOCH=0 FORCE_SOURCE_DATE=1
w=/tmp/tex/latexmkrc
rm -rf "$w"; mkdir -p "$w/home"
export HOME=$w/home LATEXMKRCSYS=$w/home/no-system-latexmkrc
unset XDG_CONFIG_HOME TEXINPUTS
run() { # name dir file
  (cd "$2" && latexmk -pdf -interaction=batchmode "$3" > "$w/$1.out" 2>&1) || { echo "$1: latexmk failed"; cat "$w/$1.out"; exit 1; }
}
cp -r "$fx/course" "$w/course-root"
run course-root "$w/course-root" lab1-setup/lab1.tex
cp "$w/course-root/lab1.pdf" "$fx/course-root.pdf"
cp -r "$fx/course" "$w/course-lab"
run course-lab "$w/course-lab/lab1-setup" lab1.tex
cp "$w/course-lab/lab1-setup/lab1.pdf" "$fx/course-lab.pdf"
cp -r "$fx/shared" "$w/shared"
run shared "$w/shared/paper" paper.tex
cp "$w/shared/paper/paper.pdf" "$fx/shared.pdf"
for f in course-root course-lab shared; do
  echo "$f: $(md5sum < "$fx/$f.pdf" | cut -c1-32), rc files: $(grep -c "Rc files read" "$w/$f.out" || true)"
done
