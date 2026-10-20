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

    T=${RUSTUP_HOME:-~/.rustup}/toolchains/nightly-2026-08-08-x86_64-unknown-linux-gnu
    sb() { env PHITEX_NO_SLICE=1 CARGO_BUILD_JOBS=8 nice -n 19 ionice -c3 \
      scripts/sandbox env PATH=$T/bin:/usr/bin RUSTUP_TOOLCHAIN=$T CARGO_BUILD_JOBS=8 "$@"; }
    scripts/sandbox --net scripts/fetch-mupdf.sh      # the viewer's renderer, once (the CLI embeds it)
    rustup target add wasm32-wasip1-threads --toolchain nightly-2026-08-08   # once, for the wasm program

- [ ] `sb cargo fmt --all --check` prints nothing.
- [ ] `sb cargo clippy --workspace --exclude defuse --no-deps --all-targets -- -D warnings`
      is clean. The vendored engine keeps its own lints (`--no-deps`
      keeps it out as a path dependency too).
- [ ] `sb cargo test --workspace --no-run` compiles every crate's tests.
- [ ] `sb cargo check --target wasm32-unknown-unknown -p phitex-codecs -p phitex-defuse -p phitex-dvi -p phitex-otf -p phitex-pdf -p phitex-source -p phitex-tex -p phitex-tfm -p phitex-typeset -p phitex-xdvipdfmx`
      passes. These are the pure crates, as in `cargo xtask check`.
- [ ] `sb cargo clippy --target wasm32-wasip1-threads -p phitex-wasm --no-deps -- -D warnings`
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
- [ ] **The clean core:** the vendored engine is the clean core
      (`crates/vendor/defuse` built with its `clean` feature, as
      `crates/core/defuse/Cargo.toml` asks for it).
- [ ] **para4000:** the md5 is the bar's at `-j 1`, `-j 8` and `-j 16`.
- [ ] **Chains:** the realised critical path and the runs per live node
      are no worse than the last `main` gate's (bars: critical path ≤ 6
      on the exact-PDF document, runs per node ≤ 1.1). Record them below.

The edit bar is not in `gate-fast`. Measure it on `$D` with
`ACCL_COMMIT=$D scripts/accl/accl submit cmd bash scripts/accl/edit-classes`
(amsldoc's edit classes through `phitex watch --json --exact-pdf` at
`-j 8`, the release build):

- [ ] **One letter** and **a line re-break**: each ≤ 16 ms. This is a hard
      bar.
- [ ] **A heading**, **a label** and **a preamble definition**: each
      ≤ 50 ms.
- [ ] **A page added** is measured and recorded, not gated.

(The user dropped "watch equals cold" as a bar on 2026-10-18. The
amsedit shards still compare each edit history with its cold build, and
a difference there is still a red.)

Record the gate:

| bar | value | job | commit | met |
|---|---|---|---|---|
| every shard green | all green but lint: clippy `missing_fields_in_debug` at `crates/output/pdf/src/lib.rs:503`, fixed after it in f8c4f328 (workspace clippy clean locally there) | 15050–15062 (lint 15051) | 1c8ca62a | **no** (fixed after) |
| amsldoc md5 at -j 1 / 8 / 16 = `98c96e00…` | 98c96e00 / 98c96e00 / 98c96e00, 44 pages | 15056 | 1c8ca62a | yes |
| amsldoc cold rounds ≤ 3 (latexmk) | 3 | 15056 | 1c8ca62a | yes |
| amsedit histories equal cold (8, 16 threads) | same, same | 15058, 15059 | 1c8ca62a | yes |
| para4000 md5 at -j 1 / 8 / 16 = `41467932…` | same at all three | 15061 | 1c8ca62a | yes |
| the clean core | the round core (the clean core is on `try-clean6`, not on dev) | | 1c8ca62a | **no** |
| one letter ≤ 16 ms (-j 8) | 22 ms | 15043 | 00765988 | **no** |
| line re-break ≤ 16 ms (-j 8) | 27 ms | 15043 | 00765988 | **no** |
| heading ≤ 50 ms | 127 ms | 15043 | 00765988 | **no** |
| label ≤ 50 ms | 574 ms | 15043 | 00765988 | **no** |
| preamble definition ≤ 50 ms | 82 ms | 15043 | 00765988 | **no** |
| page added (recorded, not gated) | 1702 ms | 15043 | 00765988 | – |
| realised critical path ≤ 6 (amsldoc, -j 8) | 1222 nodes, 3.49 s | 15056 | 1c8ca62a | **no** |
| runs per node ≤ 1.1 (amsldoc, -j 8) | 1.44 | 15056 | 1c8ca62a | **no** |

Recorded beside the bars, not gated:

| | value | job | commit |
|---|---|---|---|
| amsldoc cold wall at -j 1 / 8 / 16 (pdfTeX: 3.0 s a run) | 5.17 / 8.69 / 9.20 s (profile and observe build, shared node) | 15056 | 1c8ca62a |
| amsldoc cold watch build at -j 8, release | 5.4 s | 15043 | 00765988 |
| amsldoc one-word edit at -j 8 / 16 (amsedit, 10 edits) | median 34 / 31 ms, max 52 / 44 ms | 15058, 15059 | 1c8ca62a |
| para4000 wall at -j 1 / 8 / 16 (profile build) | 2.92 / 2.77 / 3.13 s | 15061 | 1c8ca62a |
| para4000 wall at -j 1 / 8, release | 2.43 / 2.34 s (pdfTeX: 0.734 s) | 15061 | 1c8ca62a |
| para4000 one-word edit at -j 8 / 16 | median 18 / 20 ms, max 24 / 24 ms | 15061 | 1c8ca62a |
| para4000 realised critical path, runs per node (-j 8) | 6865 nodes, 1.09 | 15061 | 1c8ca62a |

The tree's local checks (section 1), 2026-10-20, through bubblewrap
with the pinned toolchain:

| check | result | commit |
|---|---|---|
| `cargo fmt --all --check` | clean after f8c4f328 (the vendored engine's own test file is left as it is) | f8c4f328 |
| `cargo clippy --workspace --exclude defuse --no-deps --all-targets -D warnings` | clean | f8c4f328 |
| `cargo test --workspace --no-run` | compiles | f8c4f328 |
| `cargo check --target wasm32-unknown-unknown` (the pure crates) | passes | f8c4f328 |
| `cargo clippy --target wasm32-wasip1-threads -p phitex-wasm --no-deps` | clean | f8c4f328 |
| `scripts/wasm.sh check` | not run: under bubblewrap Node's WASI refuses a preopened root directory (`UVWASI_EACCES`), and the cluster's image has no Node | |
| no `claude.ai` in files | none | f8c4f328 |
| no `claude.ai` in commit messages since `main` | **five** (`Claude-Session:` trailers on 291d695a, 3580c450, 4de2c281, 6953bbf3, 5eaae244, 2026-10-11) | |

## 3. The notes

- [ ] `CHANGELOG.md`'s "Known not to work" matches the gate. Every red
      that is waived is listed with its cause. The "Speed" item carries
      the numbers recorded above.
- [ ] The README's "Alpha status" and "latexmk compatibility" sections
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
