# AGENTS.md

Rules for coding agents (and people) working in this repository. Each
crate's own docs say what it is; keep them current.

## Workflow

- Fix bugs at their root, never at the symptom: a guard that hides a
  wrong value is not a fix.
- Timings come from the accl cluster (alternating base and new runs on
  the same node). This machine is loaded, so its wall-clock times are not
  measurements. Never run anything heavy on `acclhead1` itself.
- Memory-cap every heavy local run:
  `systemd-run --user --scope -q -p MemoryMax=12G -p MemorySwapMax=0 ...`.
  Stop only processes you started, by PID. Never `pkill` or `killall`
  by name.
- Private documents used for testing (for example a thesis) stay under
  `target/` or outside the repo, and are never committed.
- Versions: like TeX's, whose version numbers converge to π, PhiTeX's
  converge to φ, the golden ratio, a digit a milestone: the alpha is
  1.6, the beta 1.61, the first stable 1.618 (the user's decision of
  2026-10-31: TeX adds its digits rarely, PhiTeX ships fixes often). A
  milestone's updates count in a third part: 1.6.1, 1.6.2, …, then
  1.61, 1.61.1, … Only the CLI crate carries it (Cargo `1.6.0`, `1.6.1`,
  `1.61.0`, …: the minor grows by digits, 6 < 61 < 618, so semver orders
  them); a milestone is named and tagged without its `.0` (`v1.6`), an
  update with it (`v1.6.1`); the other crates stay 0.0.x. Versions are
  bumped and tags made only by a release (docs/ci.md).
- `scripts/sandbox` shares one sccache cache among all worktrees (the
  server runs inside the sandbox). `PHITEX_NO_SCCACHE=1` turns it off.
- The viewer lives here, in `viewer/src`; its pages are drawn as strips
  by the core (`crates/output/render`, hayro; `viewer/src/strips.ts`
  composes them; docs/spec-display-list.md):
  `phitex watch` bundles the viewer (`scripts/viewer-bundle.sh`, the
  bundle committed unminified and checked in CI), and the Overleaf extension
  copies it from the PhiTeX commit it pins. Change the viewer here;
  never copy it back from the extension.

## Sandbox

- Run every `cargo` command, built binary, script and anything from
  `upstream/` through `scripts/sandbox [--net] <cmd>` (bubblewrap: the
  repo is writable, `$HOME` is empty).
- The network is off. Pass `--net` only to fetch
  (`scripts/sandbox --net cargo fetch`, `scripts/sandbox --net scripts/fetch-upstream.sh`).
- Never weaken the sandbox.

## Compatibility

- The oracle is the installed TeX Live binaries (pdfTeX 1.40.29), not
  our own judgement. A difference from the oracle is a bug in PhiTeX.
  Never edit expected outputs or loosen a comparison to make a test pass.
- The PDF is byte-identical to pdfTeX's, cold and after every edit, and
  so are the files the job reads back (`.aux`, `.toc`, `.bbl`, ...). The
  `.log` doesn't have to match.
- Port behaviour, not code: `tex.web`, `etex.ch` and `pdftex.web` are
  specifications. Cite their sections (`// §123`; pdfTeX's as
  "pdfTeX §N").
- Don't emulate internal statistics (memory, pool, hash, stack counts,
  exact capacity limits) beyond what the oracle's outputs show.

## Architecture

- The engines are layers, each depending only on those below:
  `phitex-defuse` (the core, no TeX) and `phitex-typeset` (Knuth's
  algorithms, pure: never depends on defuse or tex) under `phitex-tex`
  (TeX's semantics, the `Engine` trait) under `phitex-pdftex`,
  `phitex-xetex`, `phitex-luatex`. Engines add behaviour through
  `Engine`, never fork `phitex-tex`; only `phitex-luatex` links
  `phitex-lua`.
- TeX state lives only in the engine crates. There is no second source
  of truth, the CLI included: the build is the document's state.
- A cold build is an edit from nothing: there is no separate cold path.
- Pure crates build for `wasm32-unknown-unknown` (`cargo xtask check`).
- The engine runs as WebAssembly with threads (`phitex-wasm`,
  `wasm32-wasip1-threads`): `scripts/wasm.sh check` keeps it byte-identical.
- The scheduler affects speed, never results: every schedule gives
  byte-identical output.

## Quality

- `cargo fmt` and `cargo clippy` (pedantic, workspace lints) are clean;
  `unsafe_code` is denied.
- Performance claims need a benchmark on the accl cluster.
- `upstream/` is fetched from pinned commits by
  `scripts/fetch-upstream.sh` and isn't committed. To change a pin,
  update the script and run `scripts/sandbox cargo xtask corpus`.
