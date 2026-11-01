#!/usr/bin/env bash
# fetchdocs-tmp.sh DIR...: (accl cmd, CI image) copy TeX Live doc source dirs to $r/docs
for d in "$@"; do mkdir -p "$r/docs/$d"; cp -r /usr/share/doc/texlive/$d/. "$r/docs/$d/" 2>&1 | head -2; done
ls -R "$r/docs" | head -50
