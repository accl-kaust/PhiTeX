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
    rustup target add wasm32-wasip1-threads --toolchain nightly-2026-08-08   # once, for the wasm program

- [ ] `sb cargo fmt --all --check` prints nothing.
- [ ] `sb cargo clippy --workspace --exclude phitex-defuse --no-deps --all-targets -- -D warnings`
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
- [ ] **The engine:** the vendored defuse has only the clean core (the
      round core was removed in defuse 57b36bb; there is no `clean`
      feature) and is gate-cleared by its author.
- [ ] **para4000:** the md5 is the bar's at `-j 1`, `-j 8` and `-j 16`.
- [ ] **Chains:** runs per live node ≤ 1.1 on amsldoc at `-j 8`. The
      critical path's bar is the page chain's (section 3: ≤ 2 × fires +
      10); the old "≤ 6" predates the page-chain design and is dropped
      (2026-10-21). Record both below.

## 3. Scaling (an alpha bar, the user's decision of 2026-10-21)

Threads must pay off. Measure on `$D` with release builds, **on one quiet
accl node** (the whole node, `-c 32`), cold, 3 interleaved repetitions
each, the median counting, the engine's lane as `$D` ships it, under the
node's own CPU frequency governor (`ondemand` on harrier and acclnode01: what
a user's machine does; no `--cpu-freq`, no spinners; the user's decision of
2026-10-21). Record the node and its governor beside every number. The `-j 1`
baseline runs one core at full boost (3.1-3.5 GHz on harrier against
2.2-2.5 GHz for workers left idle between jobs, docs/note-cold-scaling.md), so
the ratio to `-j 1` also counts idle time the governor turns into lower clocks:

- [ ] **para4000:** `-j 8` wall ≤ 1.0 s; `-j 16` and `-j 32` no slower than
      `-j 8`; `-j 8` at least 3× faster than `-j 1`.
- [ ] **amsldoc:** `-j 8` wall faster than `-j 1`, and `-j 16` no slower
      than `-j 8`.
- [ ] **The page chain:** the realised critical path (Report
      `critical_path`) ≤ 2 × fires + 10 links on para4000 and amsldoc
      (docs/spec-page-chain.md §1.2).
- [ ] **Beside pdflatex:** pdflatex's time for the same document on the
      same node, recorded next to the walls (cold faster than pdflatex is
      the project's goal; record it, the bars above gate).

Record the table (commit, node, jobs, walls at `-j 1/8/16/32`, critical
paths) below with the gate.

## 4. Edits

The edit bar is not in `gate-fast`. Measure it on `$D` with
`ACCL_COMMIT=$D scripts/accl/accl submit cmd bash scripts/accl/edit-classes`
(amsldoc's edit classes through `phitex watch --json --exact-pdf` at
`-j 8`, the release build):

- [ ] **One letter** and **a line re-break**: each ≤ 16 ms. This is a hard
      bar.
- [ ] **A heading**, **a label** and **a preamble definition**: each
      ≤ 50 ms. The label edited is one outside a float (the user's
      decision of 2026-10-21; a label in a table float is recorded, not
      gated).
- [ ] **A page added** is measured and recorded, not gated.

(The user dropped "watch equals cold" as a bar on 2026-10-18. The
amsedit shards still compare each edit history with its cold build, and
a difference there is still a red.)

Record the gate (dev af7eca5d, the clean core, gate jobs 15413–15421,
15424, 15425 on acclnode01, 2026-10-21; the gate builds with profile and
observe):

| bar | value | job | commit | met |
|---|---|---|---|---|
| every shard green | all green: build, lint, suites, latex 0–2, ams, amsedit 8, amsedit 16, para, cg | 15413–15421, 15424, 15425 | af7eca5d | yes |
| amsldoc md5 at -j 1 / 8 / 16 = `98c96e00…` | 98c96e00 / 98c96e00 / 98c96e00, 44 pages, 0 stale reads | 15419 | af7eca5d | yes |
| amsldoc cold rounds ≤ 3 (latexmk) | 3 | 15419 | af7eca5d | yes |
| amsedit histories equal cold (8, 16 threads) | same, same (10 edits each) | 15420, 15421 | af7eca5d | yes |
| para4000 md5 at -j 1 / 8 / 16 = `41467932…` | same at all three, and at -j 32; edit histories (50 edits) same at -j 8 and 16 | 15424 | af7eca5d | yes |
| the clean core | `crates/vendor/defuse` built with `clean` (dev since 333f6c67) | | af7eca5d | yes |
| one letter ≤ 16 ms (-j 8) | ~17 ms (edit-reps, pinned, node01, r4-speed's latest) | | | **no** |
| line re-break ≤ 16 ms (-j 8) | ~19–22 ms (edit-reps, pinned, node01, r4-speed's latest) | | | **no** |
| heading ≤ 50 ms | 136 ms (r5-edits' last check) | | | **no** |
| label ≤ 50 ms | 226 ms (r5-edits' last check); amsldoc's label sits in a table float, whose floor is ~57 ms: the user's decision on how to measure it is pending | | | **no** |
| preamble definition ≤ 50 ms | 43 ms (r5-edits' last check) | | | yes |
| realised critical path ≤ 6 (amsldoc, -j 8) | 698 nodes | 15419 | af7eca5d | **no** |
| runs per node ≤ 1.1 (amsldoc, -j 8) | 1.37 | 15419 | af7eca5d | **no** |

Recorded beside the bars, not gated:

| | value | job | commit |
|---|---|---|---|
| amsldoc cold wall at -j 1 / 8 / 16 (pdfTeX: 3.0 s a run) | 4.35 / 6.20 / 6.42 s (profile and observe build) | 15419 | af7eca5d |
| amsldoc cold wall at -j 1 / 8 / 16, release build, one node | 2.78 / 3.34–3.57 / 3.47 s | 15404 | 3e0cabca |
| amsldoc cold at -j 8: idle, commit, op bodies | 27.7 s summed, 1.50 s, 4.98 s | 15419 | af7eca5d |
| amsldoc one-word edit at -j 8 / 16 (amsedit, 10 edits) | median 24 / 24 ms, max 34 / 34 ms | 15419 | af7eca5d |
| para4000 wall at -j 1 / 8 / 16 / 32 (profile build) | 3.09 / 3.22 / 3.28 / 3.53 s | 15424 | af7eca5d |
| para4000 wall at -j 1 / 8, release | 2.93 / 2.76 s (pdfTeX: 0.734 s) | 15424 | af7eca5d |
| para4000 realised critical path (-j 1 / 8) | 6093 / 4321 nodes | 15424 | af7eca5d |
| para4000 -j 1 under callgrind | 20.3 G Ir; node bodies 156 k Ir a command (pdfTeX ~2.9 k) | 15425 | af7eca5d |

Why cold amsldoc does not scale with threads, and what was ruled out:
[note-cold-scaling.md](note-cold-scaling.md); the scan's chain and the
parked block variant: [note-scan-blocks.md](note-scan-blocks.md).

The tree's local checks (section 1), 2026-10-20, through bubblewrap
with the pinned toolchain:

| check | result | commit |
|---|---|---|
| `cargo fmt --all --check` | clean after 400c72ef (the vendored engine's own test file is left as it is) | 400c72ef |
| `cargo clippy --workspace --exclude phitex-defuse --no-deps --all-targets -D warnings` | clean | 400c72ef |
| `cargo test --workspace --no-run` | compiles | 400c72ef |
| `cargo check --target wasm32-unknown-unknown` (the pure crates) | passes | 400c72ef |
| `cargo clippy --target wasm32-wasip1-threads -p phitex-wasm --no-deps` | clean | 400c72ef |
| `scripts/wasm.sh check` | not run: under bubblewrap Node's WASI refuses a preopened root directory (`UVWASI_EACCES`), and the cluster's image has no Node | |
| no `claude.ai` in files | none | 400c72ef |
| no `claude.ai` in commit messages since `main` | **five** (`Claude-Session:` trailers on 43dd8b7a, 3dbb4614, b9e0a985, 49ba2d93, 4f6e007d, 2026-10-11) | |

## 5. SyncTeX (in the alpha, the user's decision of 2026-10-21)

docs/spec-synctex.md, on `$D`:

- [ ] `phitex build --synctex` writes `JOB.synctex.gz`; the `synctex`
      tool's answers (a point's file and line, a line's boxes) agree with
      pdflatex's file on amsldoc and the latex suite's documents; every
      miss listed.
- [ ] `phitex watch`: every glyph's source place reaches the browser
      (`origins`); a double-click opens the word's place in the editor;
      the editor's cursor and selection highlight their glyphs.
- [ ] Off costs nothing (amsldoc callgrind Ir and peak RSS within noise);
      on, the PDF bytes are unchanged and an edit's cost is recorded.

## 6. The first build

A first build makes the LaTeX format: it is the engine's largest node, so
it shows the engine's weaknesses (2026-10-21: 19.9 s and 990 MB against
`pdftex -ini`'s 6.06 s and 63 MB; docs/note-sequential-region.md). The
user: optimize it, don't hide it.

- [ ] A one-page LaTeX document's first build (empty cache): wall and
      peak memory recorded beside `pdftex -ini` plus pdflatex on the same
      node, with the engine's final-values pass (E1) and the format as a
      sequential region (E2) landed or their state stated.

## 7. The notes

- [ ] `CHANGELOG.md`'s "Known not to work" matches the gate. Every red
      that is waived is listed with its cause. The "Speed" item carries
      the numbers recorded above.
- [ ] The README's "Alpha status" and "latexmk compatibility" sections
      still describe the build: rounds, the round cap, the tools by reads,
      and shell escape.
- [ ] `phitex --help` and `phitex --compat=pdflatex -help` match the
      README. Exit statuses are pdflatex's: 0 on success, 1 otherwise.

## 8. Main

- [ ] **The user confirms**, naming `$D`.
- [ ] Then, and only then, the user (or someone at the user's word)
      pushes `main` from a worktree. The main checkout is never used for
      git:

      git push origin $D:refs/heads/main
      git push github $D:refs/heads/main

GitHub carries only `main` and `dev`.
