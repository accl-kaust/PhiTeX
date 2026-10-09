# HANDOFF: shared interner (branch `interner`), then the parallel graft

This branch is work in progress and may be red. It branches off `main` (merge 1944023).
It covers queue item 4, the content-hash interner. Item 5, the parallel graft for the
φ core in crates/phi, comes next and is designed below.

## Do not touch / rules

- crates/partex-core/src/pure/* and crates/partex-cli/src/pure.rs belong to the TeX agent.
  Coordinate through the coordinator before you change them. Item 7 (enforcer coverage) needs them.
- Keep fmt and clippy pedantic clean. unsafe is denied. wasm32 must build.
  The interner code is `#[cfg(feature = "std")]`, so the default no-std build and wasm keep working.
- Never force-push. No session links or tool URLs in commits.
- The coordinator lands branches on main. Gate first (the commands are below).

## 1. Interner: state

Goal: worker views of one engine session must agree on a control sequence's location p,
because pure-SSA eqtb slots are keyed by p. No output and no version may depend on p.

Done (uncommitted until this WIP commit):

- crates/partex-core/src/interner.rs: `SharedNames`.
  - It maps spelling to p through 64 `Mutex<HashMap>` shards, picked by the FNV-1a spelling hash.
  - Each slot has an `AtomicU8` taken flag, claimed by compare_exchange.
    So inserting is atomic per spelling (coordinator point a): two workers that make `\foo` get one p.
  - A name is placed at the first free slot of [its TeX hash-code slot] followed by
    `name_probes` (the hash_extra region, as in `Tex::probe_names`).
  - When it places a name, it records the spelling hash in `Spellings`.
  - `enter()` / `leave()` install or clear the thread-local `Spellings` used for versions.
- crates/partex-engine/src/node.rs: `Spellings`, `set_spellings`, `tok_version_spelled`.
  - `tok_version` folds a cs token whose p is in the shared table as `[i32::MIN, lo, hi]`
    of its spelling hash, never p.
  - Needs partex-engine feature `std`. partex-core's `std` feature enables it.
- hash.rs:
  - `id_lookup_chain` goes to `id_lookup_shared` when `shared_names` is set and hash_extra > 0.
  - `id_lookup_shared` checks the format's chain from the start slot first.
    Otherwise it calls find, or inserts into the shared table.
    Then it enters the name in the view's own table with a local string number
    (string numbers may differ per view).
- effects.rs:
  - `Tex::share_names()` builds the table from this view's hash and enters it.
  - `set_shared_names(Option<Arc<_>>)` hands the table to other views (fork.rs clones the Arc).
  - `intern_name(name)` places a decoy, for tests.
- tex.rs, fork.rs, save_state.rs and statehash.rs carry the new field.
  It is not saved and not hashed.

Known limit: p still depends on the insertion order of colliding names, because a name is
never moved once lists hold it. The guarantee is "no output or version depends on p". The
coordinator approved this.

## 2. Decoy-order test: status

crates/partex-core/tests/interner.rs runs an ini pdfTeX doc five ways:
plain, shared, shared with decoys(2), decoys(2) reversed, and decoys(3).
- The doc uses \csname, \let, \toks, \write to doc.aux, \meaning, \string, and `\ifx\csname`.
- Decoys are names with the same TeX hash codes.
- The test asserts that the PDF and .aux bytes equal plain's, and that the names did land elsewhere (p0≠p1, p1≠p3).

Status:
- In an earlier debug run, plain and shared both completed. The test then failed on p1 == p2,
  because reversing the same decoys gives the same taken set.
  It now compares different decoy sets.
- The release run with the new comparison was still running when I handed off, so the result is unknown.
  Run it first.
- If bytes differ, p or a string number leaks somewhere. The candidate places are in §3.

Not done (coordinator point c, if cheap): W=1 against a shuffled insertion order on a LaTeX doc (latexdoc).
- Easiest route: a CLI env var that calls `share_names()` and then interns N seeded decoys before the run.
- Compare the PDF and .aux against a plain run.

## 3. Audit list (coordinator point b): where p or a string number could reach output or behaviour

Checked:
- diag.rs: suggestions are sorted by content. OK.
- track.rs: cell_names is used only in reports. OK.
- BTreeMaps in the pdf code are keyed by object or form numbers, not p. OK.

To do:
- pdfTeX name trees and destination ordering (\pdfdest names, /Dests, /Names): sort by string bytes, never by string number or p.
- \meaning, \show and \string of internal or frozen names. Also the print of an undefined cs.
- Format dump / save_state: the shared table must not be dumped. Check that a dump made with sharing on equals one made with it off.
- Every Ord or Hash on p (or a str number) in a map whose iteration feeds output or versions.
  `grep -n "HashMap<i32\|BTreeMap<i32\|BTreeSet<i32"` in crates/partex-core/src, then follow each to its users.
- statehash.rs: hashes over eqtb or hash contents keyed by p. With sharing on, any state hash used for reuse
  must hash spellings instead.
- Token-list versions made outside `tok_version` (any other fold over raw token values).
- \fontname and font identifiers (`font_id_text`) are string numbers, so check that they print by characters.
- The pure-SSA layer's Eqtb slot keys (in pure/*, not ours): agreement across views holds only if every view
  calls `set_shared_names` before making names. Tell the coordinator.

## 4. Next steps for the interner

1. Get tests/interner.rs green, in release (debug takes 12+ minutes).
2. Finish the §3 audit. Fix leaks and add each one to the doc in the test.
3. Point c (the latexdoc shuffle) if it is cheap.
4. Run fmt, `cargo clippy --workspace --all-targets` (and with `--features std` for partex-core), and the wasm check.
5. Add a two-line note to DESIGN (3.17.10, "shared interner").
6. Commit and push to accl `interner`. Gate it (commands below) and report to the coordinator, who lands it.

## 5. Parallel graft design (approved model; build after the interner)

Target: at least 6× speedup at W=8, cold, on 10^8 nodes (crates/phi, bench `examples/prof.rs` on accl).

Model:
- There is no single-threaded main executor.
  - All W workers execute segments speculatively, each into a private `Graph` (as `Graph::pack` does today).
  - Each segment records its read set: names and slots with the version it read, or `Ver::ABSENT` for an unresolved operand.
- Validation is parallel.
  - A worker validates segment k against the provisional state, which is the entry state plus
    provisionally committed segments 0..k-1.
  - It compares each read's version against the export versions of those segments
    (the last writer < k, per name).
  - On a real mismatch (the version differs, not just "something was written"), segment k is invalidated and re-executed.
  - Graph must support this: `validate(seg, upto: k)`, which looks names up in an index over provisional
    segments' exports (per name, a sorted list of (segment, ver)) without merging contents.
- The commit stays sequential and in order, and it is thin.
  - Committing segment k means: stamp k committed, check the few version compares left over from
    provisional validation (only names whose last writer changed since k validated), and link k's packed body.
  - Bodies are handed over by reference or splice: the segment's packed arena goes into the graph's
    segment list (Vec of Arc/Box chunks). Commit never copies segment contents.
    Node ids are (segment, local) or remapped lazily through a per-segment base offset.
  - Target cost: O(1) amortized per segment plus O(changed names).
- Invalidating segment k also invalidates any provisionally validated segment > k that read a name k exported
  with a different version. Use the per-name reader index, so this costs O(readers) and is not a rescan.
- Cancellation already exists (rounds + cancellation, 3b, e2489cc). Reuse it for re-execution.
- Measure:
  - cold W=1/8 at 1e6 and 1e8, K10 and K20;
  - commit time per segment (it must stay flat with the segment count);
  - graft bytes copied (target 0 on the commit path).
- The current serial graft is why W=8 cold at 1e8 was only 2.48×.

The φ core lives in crates/phi on branch phi-core (head 9a6bb93, landed by the coordinator as 3a02b35 on main).
- graph.rs is the engine, and graph/compact.rs, seal.rs, runs.rs and check.rs are its helpers.
- Tests are tests/core.rs, toy_props.rs and spec.rs. The toy client is tests/toy/mod.rs.

## 6. After the graft

- Item 6: PHITEX_SSA_PURE=1 harness at W=1/8 in xtask e2e.rs and in scripts/ssa-edits.
- Item 7: enforcer coverage for Glyphs, PageNode, PdfObj/Name/Num, Clock, Class and Unknown,
  with the host files tfm/vf/enc/pict as sources. This touches pure/*, so coordinate first.

## 7. Building and testing without accl or local paths

Toolchain: rust-toolchain.toml pins nightly-2026-08-08 with rustfmt, clippy and the wasm32-unknown-unknown target.
rustup installs it on first cargo use.

System packages (Debian/Ubuntu):

    apt-get install -y build-essential pkg-config git curl bubblewrap python3 \
        texlive-base texlive-latex-base texlive-latex-recommended texlive-fonts-recommended \
        texlive-latex-extra texlive-pictures texlive-binaries
    curl --proto '=https' -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain none

Arch: `pacman -S base-devel bubblewrap python texlive-basic texlive-latex texlive-latexrecommended texlive-fontsrecommended texlive-latexextra texlive-pictures`.

- The interner test needs only cmr10.pfb. It tries /usr/share/texmf-dist (Arch),
  then /usr/share/texlive/texmf-dist (Debian), then `kpsewhich cmr10.pfb`.
  The tfm is in crates/partex-core/testdata.
- Wider e2e tests need a real TeX Live with formats (`fmtutil-sys --all`) and `kpsewhich` on PATH.

`scripts/sandbox` (bubblewrap) wraps every cargo and binary run.
- If bubblewrap cannot create namespaces (some containers), run the same commands without the
  `scripts/sandbox` prefix inside the throwaway cloud VM.
- Fetching crates needs `--net` once: `scripts/sandbox --net cargo fetch`.

Commands:

    scripts/sandbox --net cargo fetch
    # the interner test (release: debug takes over 12 min)
    scripts/sandbox cargo test --release -p partex-core --features std --test interner -- --nocapture
    # the engine and core unit tests touched here
    scripts/sandbox cargo test --release -p partex-engine --features std
    scripts/sandbox cargo test --release -p partex-core --features std --lib
    # lint and format
    scripts/sandbox cargo fmt --all -- --check
    scripts/sandbox cargo clippy --workspace --all-targets
    scripts/sandbox cargo clippy -p partex-core --features std --all-targets
    # wasm must build (no std feature)
    scripts/sandbox cargo build -p partex-core --target wasm32-unknown-unknown
    # φ core (for the graft)
    scripts/sandbox cargo test --release -p phi
    PHI_SEED=5 scripts/sandbox cargo test --release -p phi --test toy_props
    scripts/sandbox cargo run --release -p phi --example prof   # env K1/K10/K20, SEAL, EDITS, RUNS

Gating (needs accl; leave this to the coordinator if you cannot reach it):

    ACCL_COMMIT=<rev> scripts/accl/accl submit gate
    ACCL_COMMIT=<rev> scripts/accl/accl submit edits --brief --fixpoint

Heavy local runs on the original machine were capped:
`systemd-run --user --scope -q --slice=partex.slice -p MemoryMax=16G -p MemorySwapMax=0 nice -n 19 ionice -c 3 <cmd>`.
In a cloud VM, at least keep each heavy run under a memory limit.
