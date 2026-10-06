# AGENTS.md

Rules for coding agents (and people) working in this repository. Read
`DESIGN.md` first: it is the whole design and is self-contained. Keep it
current. When a design changes, or a measurement changes what the design
should be, update its section in the same branch. Don't add other design
documents.

## Workflow

- Work on your own branch, in a git worktree under `~/code/tmp/` (never
  `/tmp`). Make small commits, each with its tests passing. Commit
  messages say what changed and why, with the measured numbers. They
  carry no tool or session links.
- Before reporting a change done, run:
  - `scripts/sandbox cargo fmt` and `scripts/sandbox cargo clippy --all-targets` (clean);
  - the gates, on the pushed commit:

        ACCL_COMMIT=<rev> scripts/accl/accl submit gate
        ACCL_COMMIT=<rev> scripts/accl/accl submit edits --brief --fixpoint
        scripts/accl/accl wait <job>

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
  will converge to φ, the golden ratio (1.6, 1.61, 1.618, 1.6180, …).
  Until the first such release, every crate stays at 0.0.x: don't bump
  versions, tag releases or adopt another scheme.
- `scripts/sandbox` shares one sccache cache among all worktrees (the
  server runs inside the sandbox). `PHITEX_NO_SCCACHE=1` turns it off.
- The draw list's renderer lives here, in `viewer/src` (viewer.ts,
  page2.ts, sync.ts, css.ts): `phitex watch` bundles it
  (`scripts/viewer-bundle.sh`, its bundle committed and checked in CI),
  and the Overleaf extension copies it from the PhiTeX commit it pins.
  Change the draw list (phitex-draw) and its rendering together, here;
  never copy the renderer back from the extension.

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
  exact capacity limits). They are masked in comparisons
  (`xtask/src/mask.rs`).

## Architecture

- Core crates build for `wasm32-unknown-unknown`. `partex-core` is
  `no_std` + `alloc`, and every effect goes through the `Host` trait.
- Engine state is reached only through accessors that call the
  `Tracker` (DESIGN 3.2, 3.15).
- Dynamic SSA (DESIGN 3.1): "a step runs again if and only if a version
  it read changed" is the invariant. Any change to the SSA runtime, the
  tracker or the records reports a one-word edit's rebuild on a real
  document: its ms, steps run and reads checked. A rebuild whose work
  grows with the document instead of with the edit is a bug in the
  dependency graph.
- Every acceleration (memos, speculation, parallelism) can be switched
  off and gives byte-identical output.
- Every IR has a canonical, round-tripping text form (DESIGN 5.3).

## Quality

- `cargo fmt` and `cargo clippy` (pedantic, workspace lints) are clean;
  `unsafe_code` is denied.
- Performance claims need a benchmark. Keep the `tracing` spans and the
  TeX-level profiler working.
- `upstream/` is fetched from pinned commits by
  `scripts/fetch-upstream.sh` and isn't committed. To change a pin,
  update the script and run `scripts/sandbox cargo xtask corpus`.
