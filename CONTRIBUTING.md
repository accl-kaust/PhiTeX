# Contributing to PhiTeX

Thank you for helping. PhiTeX is young: the most useful thing you can
send is a document it gets wrong.

## Reporting a problem

- **A PDF that differs from pdflatex's is a bug.** Open an issue with the
  smallest `.tex` file that shows it, the command you ran, and
  `phitex --version`. Say which TeX Live you compared with.
- **A crash or an engine failure:** add the failure report phitex prints
  (it names its file) or the output of the command with `-v`.
- **Slow builds or edits:** the document (or a public one that behaves
  the same), the thread count, the machine, and the times you saw.
- Private documents: don't attach them. Cut them down to a part you can
  share.
- **Security problems** (a document that escapes the sandbox, runs a
  command, or reads files it shouldn't): don't open an issue. Write to
  flinner &lt;at&gt; nand &lt;dot&gt; sh.

## Building and testing

Everything runs inside `scripts/sandbox` (bubblewrap: only the repository
is writable, no network unless asked):

```sh
scripts/sandbox --net cargo fetch          # once, and after Cargo.lock changes
scripts/sandbox cargo build --release -p phitex-cli
scripts/sandbox cargo test -p phitex-cli    # most tests need TeX Live 2026 installed
scripts/viewer-test.sh                      # the viewer's TypeScript tests
scripts/viewer-typecheck.sh
```

The release binary is `target/release/phitex`. `phitex --help` shows the
common commands; `phitex --help-all` shows every option.

## Changing the code

[AGENTS.md](AGENTS.md) has the rules for people and coding agents alike.
The ones that matter most:

- **Root causes only.** A fix explains why the bug happened and removes
  the cause. No retries, widened tolerances or special cases that hide it.
- **pdfTeX is the oracle.** Expected outputs come from TeX Live's
  binaries. Never edit an expected output or loosen a comparison to make
  a test pass.
- **Same output at any thread count and on every schedule.**
- `cargo fmt` and `cargo clippy` (pedantic, workspace lints) are clean.
- Performance claims come with a measurement: the machine, the thread
  count, the number of runs, before and after.

## Pull requests

- Open them against **`dev`**. Keep one change per pull request, with a
  test that fails without it.
- A commit message says what changed, why, and how you checked it.
- `main` moves only by a pull request from `dev` whose checks and gate
  are green (docs/ci.md). Nobody pushes to `main`.

## Licence

PhiTeX is free software under the GNU Affero General Public License,
version 3 only ([LICENSE](LICENSE)). By contributing you agree that your
contribution is licensed the same way.
