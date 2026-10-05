#!/usr/bin/env bash
# Fetch pinned upstream sources (test corpora + reference WEB sources) into upstream/.
# Shallow and sparse; re-running updates to the pinned commits.
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p upstream

# name  url  commit  [sparse paths...]
REPOS=(
  "texlive-source https://github.com/TeX-Live/texlive-source.git 94234726a6f894b4c170e296f86c93603ae48534 texk/web2c texk/dvipdfm-x libs/teckit"
  "latex2e https://github.com/latex3/latex2e.git 4e72b3d75e366268e9f4330ed29bafbab2820f78"
  "latex3 https://github.com/latex3/latex3.git e2de762010a413b93a4216e028d0a629d7948dfd"
  "pgf https://github.com/pgf-tikz/pgf.git 839974a3f895bfb86f5a8bc155f0886c918f1bff doc/generic/pgf"
  "fontspec https://github.com/latex3/fontspec.git 43ee7b4b647c4cd4fcf78b4ca19f9edd02e40669 testfiles"
  "unicode-math https://github.com/latex3/unicode-math.git 184a23b0cb259d4dc9848ec3db0aa2cd383cae99 testfiles"
  "polyglossia https://github.com/reutenauer/polyglossia.git 07647fa747ab7002c73209cedbc174239308690b testfiles"
)

for spec in "${REPOS[@]}"; do
  read -r name url commit sparse <<<"$spec"
  dir="upstream/$name"
  if [ ! -d "$dir/.git" ]; then
    git init -q "$dir"
    git -C "$dir" remote add origin "$url"
  fi
  if [ -n "${sparse:-}" ]; then
    git -C "$dir" config remote.origin.partialclonefilter blob:none
    git -C "$dir" sparse-checkout set $sparse
  fi
  if [ "$(git -C "$dir" rev-parse -q --verify HEAD 2>/dev/null)" != "$commit" ]; then
    echo "fetching $name @ ${commit:0:12}"
    git -C "$dir" fetch -q --depth 1 --filter=blob:none origin "$commit"
    git -C "$dir" -c advice.detachedHead=false checkout -q "$commit"
  fi
  echo "$name @ ${commit:0:12} ok"
done

# CTAN documentation sources have no immutable Git tag. Pin every downloaded
# file by SHA-256 so a mirror update cannot silently change the PDF oracle.
fetch_doc() {
  local base="$1" path="$2" hash="$3" dest="upstream/$4/$2"
  mkdir -p "$(dirname "$dest")"
  if [ ! -f "$dest" ] || [ "$(sha256sum "$dest" | cut -d' ' -f1)" != "$hash" ]; then
    curl -fsSL --retry 3 "$base/$path" -o "$dest"
  fi
  [ "$(sha256sum "$dest" | cut -d' ' -f1)" = "$hash" ] || {
    echo "SHA-256 mismatch: $dest" >&2
    exit 1
  }
}

pdftex_doc=https://mirrors.ibiblio.org/CTAN/systems/doc/pdftex/manual
fetch_doc "$pdftex_doc" pdftex.tex 28c85f1689b25ae798f7bd48f2a05159b39462d779c040522c7ac74547d483d7 pdftex-manual
fetch_doc "$pdftex_doc" pdftexmanual.cls 5349359e9db462e669053c3c289c0b7744362122cccb8242baf2ecbbf5827df8 pdftex-manual
fetch_doc "$pdftex_doc" incl/fdl-1.2.tex 53f0b916d5675559f9936306acc4471f76bb80a662ff3b3c5786032b1268a907 pdftex-manual
fetch_doc "$pdftex_doc" incl/ini-etex.txt a6e65150abae75525b4e2fa1bf8cdc28cea4c71c2658c7b48db7e1002e8b84ac pdftex-manual
fetch_doc "$pdftex_doc" incl/ini-pdfetex.txt 4f92bc557b65e528c01b1875c74c3f68b84a3c664541167ec0746a01d7c6330a pdftex-manual
fetch_doc "$pdftex_doc" incl/pdfmin-crop.pdf 35aee6870f8cf54a367a555e822ca152020275db35a67c700eca19faa15ab0f7 pdftex-manual
fetch_doc "$pdftex_doc" incl/pdftex-help.txt e288c5b84871795d68ab1bb7a866ece4978a6412b1786b2b0d55eeb65a03fc00 pdftex-manual
fetch_doc "$pdftex_doc" incl/pdftex-syntax.tex 3085086f59e7503663cc25751f22adcbabd2a651b3329e3c361e8c518b0f1fa4 pdftex-manual
fetch_doc "$pdftex_doc" incl/pdftexconfig.txt 9f86f811f1ab046b7a1408ef7f52420a8f914a5588aa668a6d163568da49439e pdftex-manual

lwarp_doc=https://mirrors.ibiblio.org/CTAN/macros/latex/contrib/lwarp
fetch_doc "$lwarp_doc" lwarp.dtx 9802784805ed6e3b9ffb9e0bb696728e976e2b2e155db2d112ff931b60b78588 lwarp-ctan
fetch_doc "$lwarp_doc" lwarp.ins 5fbe2a3ecf78f7c810bd1e2bc36cfd36ea44a4111d8c1701adc91fc1b930ccf9 lwarp-ctan
fetch_doc "$lwarp_doc" lwarp_baseline_marker.png 6186d5aba9d04ef6688c2107307a836845a32d9d08a94f91970a73a4d87eddb0 lwarp-ctan
