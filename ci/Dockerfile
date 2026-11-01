# The CI image (docs/ci.md): scripts/accl/phitex.def as a Dockerfile, for
# GitHub's hosted runners. Arch Linux pinned to an Arch Linux Archive
# snapshot, so pdfTeX, TeX Live, zlib and the fonts are the versions the
# oracle outputs come from, and the repository's Rust toolchain. Built and
# pushed by .github/workflows/ci-image.yml; the target `doc` adds TeX
# Live's documentation tree and the language collections the manual
# corpus needs (scripts/accl/phitex-ci.def).
FROM archlinux:base AS base
ARG SNAPSHOT=2026/09/25
RUN echo "Server = https://archive.archlinux.org/repos/$SNAPSHOT/\$repo/os/\$arch" > /etc/pacman.d/mirrorlist \
 && sed -i 's/^#\?CheckSpace/#CheckSpace/; s/^#DisableSandbox/DisableSandbox/' /etc/pacman.conf \
 && (grep -q '^DisableSandbox' /etc/pacman.conf || sed -i '/^\[options\]/a DisableSandbox' /etc/pacman.conf) \
 && pacman -Syyuu --noconfirm --disable-download-timeout \
 && pacman -S --noconfirm --needed --disable-download-timeout base-devel git time nodejs npm rustup clang mold biber ghostscript valgrind qpdf \
      texlive-basic texlive-bibtexextra texlive-bin texlive-binextra texlive-context \
      texlive-fontsextra texlive-fontsrecommended texlive-fontutils texlive-formatsextra \
      texlive-latex texlive-latexextra texlive-latexrecommended texlive-luatex \
      texlive-mathscience texlive-metapost texlive-pictures texlive-plaingeneric \
      texlive-pstricks texlive-publishers texlive-xetex texlive-humanities texlive-langarabic \
 && pacman -Scc --noconfirm
ENV RUSTUP_HOME=/opt/rust/rustup CARGO_HOME=/opt/rust/cargo PATH=/opt/rust/cargo/bin:/usr/bin/vendor_perl:$PATH LANG=C.UTF-8
# (the toolchain rust-toolchain.toml pins)
ARG TOOLCHAIN=nightly-2026-08-08
RUN rustup toolchain install $TOOLCHAIN --profile minimal \
      --component rustfmt,clippy --target wasm32-unknown-unknown,wasm32-wasip1,wasm32-wasip1-threads \
 && rustup default $TOOLCHAIN \
 && node --version | grep -q '^v26\.' \
 && kpsewhich cmr10.pfb

FROM base AS doc
RUN sed -i '/^NoExtract/d' /etc/pacman.conf \
 && pacman -Syy --noconfirm \
 && pacman -S --noconfirm --needed --disable-download-timeout texlive-doc \
      texlive-langcjk texlive-langchinese texlive-langjapanese texlive-langkorean \
      texlive-langcyrillic texlive-langgreek texlive-langeuropean texlive-langother \
      texlive-langfrench texlive-langgerman texlive-langitalian texlive-langspanish \
      texlive-langpolish texlive-langportuguese texlive-langczechslovak texlive-music \
      texlive-games \
 && test -f /usr/share/doc/texlive/generic/pgf/pgfmanual.tex \
 && pacman -Scc --noconfirm
