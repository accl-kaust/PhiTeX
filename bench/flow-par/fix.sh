#!/bin/bash
# fix.sh: this machine's TeX Live paths (Arch) as the running system's
TD=$(kpsewhich -var-value TEXMFDIST)
HC=$(dirname "$(kpsewhich hyphen.cfg)")
TI=$(dirname "$(kpsewhich pdftexconfig.tex)")
sed -e "s#/usr/share/texmf-dist#$TD#g" -e "s#/etc/texmf/tex/generic/config#$HC#g" -e "s#/etc/texmf/tex/generic/tex-ini-files#$TI#g"
