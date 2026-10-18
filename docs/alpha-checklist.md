# Alpha: the dev → main checklist

`main` gets the alpha (plain TeX and LaTeX, `CHANGELOG.md`) only when
every box below is checked for one dev commit, and only after **the user
confirms**. Nobody else pushes `main`. Run each step on the same commit:
the one that will become `main`.

    D=$(git rev-parse origin/dev)    # the candidate; every step below is on $D

## 1. The tree, locally

Run these in a worktree at `$D`, through the sandbox, with the pinned
toolchain (rustfmt differs between versions, so fmt runs only here,
never on the cluster):

    T=~/.rustup/toolchains/nightly-2026-08-08-x86_64-unknown-linux-gnu
    sb() { env PHITEX_NO_SLICE=1 CARGO_BUILD_JOBS=8 nice -n 19 ionice -c3 \
      scripts/sandbox env PATH=$T/bin:/usr/bin RUSTUP_TOOLCHAIN=$T CARGO_BUILD_JOBS=8 "$@"; }
    scripts/sandbox --net scripts/fetch-mupdf.sh      # the viewer's renderer, once (the CLI embeds it)

- [ ] `sb cargo fmt --all --check` prints nothing.
- [ ] `sb cargo clippy --workspace --exclude defuse --all-targets -- -D warnings`
      is clean. The vendored engine keeps its own lints.
- [ ] `sb cargo test --workspace --no-run` compiles every crate's tests.
- [ ] `sb cargo check --target wasm32-unknown-unknown -p phitex-codecs -p phitex-defuse -p phitex-dvi -p phitex-otf -p phitex-pdf -p phitex-source -p phitex-tex -p phitex-tfm -p phitex-typeset -p phitex-xdvipdfmx`
      passes. These are the pure crates, as in `cargo xtask check`.
- [ ] `sb cargo clippy --target wasm32-wasip1-threads -p phitex-wasm -- -D warnings`
      is clean.
- [ ] `sb scripts/wasm.sh check` ends with `WASM_OK`. That run checks a
      plain TeX and a LaTeX document, formats made in wasm, each
      byte-identical to pdfTeX's at 1 and 4 threads.
- [ ] `git grep -nI 'claude\.ai' $D` finds nothing, and neither does
      `git log --format=%B origin/main..$D | grep -i 'claude\.ai'`. No
      session or attribution URLs go in files or commit messages.
- [ ] `git diff origin/main..$D --stat -- oracle/` changes no expected
      output that the commits do not explain. Expected outputs are never
      edited to make a test pass.

## 2. The gate, on accl

    scripts/accl/gate-fast $D          # prints the job ids
    scripts/accl/gate-fast wait <ids>  # the table

All of these must hold in the table:

- [ ] Every shard is green: build, lint, suites, latex 0–2, ams,
      amsedit 8, amsedit 16, para, cg. No red line, and no test waived
      without a named cause in `CHANGELOG.md`'s "Known not to work".
- [ ] **amsldoc** (`ams`) exits 0 at `-j 1`, `-j 8` and `-j 16`. Its md5
      is the oracle's, `98c96e004c1c82db1e3d562cf0181a3e`, at all three:
      pdflatex run by latexmk, 44 pages.
- [ ] **Rounds:** amsldoc's cold rounds are ≤ latexmk's pdflatex runs (3).
      A round-cap warning is a red.
- [ ] **Watch equals cold:** every amsedit history (8 and 16 threads)
      equals its cold build.
- [ ] **para4000:** the md5 is the bar's at `-j 1`, `-j 8` and `-j 16`.
- [ ] **Chains:** the realised critical path and the runs per live node
      are no worse than the last `main` gate's (bars: critical path ≤ 6
      on the exact-PDF document, runs per node ≤ 1.1). Record them below.

Record the gate:

| | value |
|---|---|
| dev commit | |
| gate job ids | |
| amsldoc md5 at -j 1 / 8 / 16 | |
| amsldoc cold rounds (latexmk: 3) | |
| amsldoc cold wall at -j 8 (pdfTeX: 3.0 s a run) | |
| amsldoc edit median at -j 8 | |
| para4000 wall at -j 1 / 8 / 16 | |
| realised critical path, runs per node | |

## 3. The notes

- [ ] `CHANGELOG.md`'s "Known not to work" matches the gate. Every red
      that is waived is listed with its cause. The "Speed" item carries
      the numbers recorded above.
- [ ] The README's "Status: alpha" and "latexmk compatibility" sections
      still describe the build: rounds, the round cap, the tools by reads,
      and shell escape.
- [ ] `phitex --help` and `phitex --compat=pdflatex -help` match the
      README. Exit statuses are pdflatex's: 0 on success, 1 otherwise.

## 4. Main

- [ ] **The user confirms**, naming `$D`.
- [ ] Then, and only then, the user (or someone at the user's word)
      pushes `main` from a worktree. The main checkout is never used for
      git:

      git push origin $D:refs/heads/main
      git push github $D:refs/heads/main

GitHub carries only `main` and `dev`.
