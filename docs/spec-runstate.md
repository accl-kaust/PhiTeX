# SPEC: run-local native state (branch `runstate`)

Bar (user, 2026-10-17): a node's run must cost per TeX command what pdfTeX
costs (about 20 ns). The format run's first 12M commands take 28.9 s today
(2.4 µs/command); the target is 0.5 s or less at 12M. Node-boundary work
(import on entry, publish at the end) stays under 5% of the run's time.
Nothing short of this counts; report the number reached and the top ten
exclusive costs when it is not reached.

## Why it is 100x today

Inside a run, every token is an enum (`val.rs` `Tok`, 16 bytes), every
meaning lookup goes `Tok -> Field -> own/over hash maps -> engine
use_field -> ValueId -> decode (mcache hash)` (`run.rs` `mean_of`,
`get_f`, `set_cur`), every definition goes `set_f -> own.insert +
defs.push + def_ix hash + ginert hash + slot_note`, every token list is
encoded to engine values and decoded back, and field names are formatted
and hashed (`format!("count:{k}")`, `fields.get_at`). All of that existed
so the engine could track def-use per command. With paragraph-region nodes
the engine only needs, per node, the SET of fields read on entry and the
DELTA of fields defined at the end. Everything between is private to the
run and must be native.

## The design (literal)

### 1. Tokens: one `u32`

`pub struct Tok(u32)`, `#[repr(transparent)]`, `Copy`, `Eq`, `Hash`.
- bit 31 = 0: character token, `cat << 21 | char` (as `Tok::C` today).
- bit 31 = 1: control sequence, low 31 bits = `CsId` index (`val.rs`
  `CsId::index`). `CsId` keeps its interning table; `Tok::cs(name)` interns.
- Frozen control sequences keep their names (`\u{10FFFE}` prefix) and so
  their indices; nothing else changes about naming.
- `Tok::cat()`, `chr()`, `is_cs_or_active()`, `cs()` keep their signatures.
  `Tok::C(x)` / `Tok::S(c)` pattern sites become `tok.as_char()` /
  `tok.as_cs()` returning `Option`. Every match on `Tok` in the crate is
  rewritten (run.rs, expand.rs, scan.rs, etex.rs, control.rs, ops.rs,
  math.rs, align.rs, hints.rs, macros).
- Token lists inside a run are `Rc<[Tok]>` (4 bytes per token). They are
  encoded to engine values ONLY when they leave the run (a definition that
  is published, a contribution) and decoded ONLY when imported.

### 2. One run-local table for all TeX state: `runstate.rs`

`pub struct State { eq: Vec<Entry>, dirty: Vec<Loc>, save: Vec<Saved> }`

- `Loc = u32`, the index of a field in the run's table. `fields.rs` is
  extended from the current `NSLOT` slots (params, catcodes, registers
  0..255) to EVERY family a run touches, in fixed ranges:
  - meanings: `Loc = MEAN_BASE + CsId::index` (one entry per interned
    control sequence; the table grows when a new CsId is interned, as
    `tfields` grows today); active characters: `MEAN_ACTIVE_BASE + char`.
  - `cat:`, `lc:`, `uc:`, `sf:`, `mathcode:`, `delcode:` for chars < 256,
    plus a small map for chars >= 256 (the one hash map allowed, rarely hit).
  - `count:`, `dimen:`, `skip:`, `muskip:`, `toks:`, `box:` 0..255 (eTeX
    registers >= 256 through the same spill map).
  - all `Par` parameters, `font`, `fontdimen`, `deadcycles`,
    `insertpenalties`, `prevgraf`, `spacefactor`, `prevdepth`, marks, the
    current font, the e-TeX state fields, `inspen`, `box:255`.
  - keyed families (`file:`, `wfile:`, `wkeys:`, `pagefx:`, `pgdef:` ...)
    keep their interned per-epoch `Field` and the slow path (they are read
    a few times per run, never per command).
  A field that is in no fixed range has no `Loc`; it is read through
  the existing `Field` path. The point is that NOTHING on the per-command
  path takes that branch.

- `Entry { val: Native, state: EntryState }` where
  `EntryState = Unloaded | Loaded | Dirty(Locality)` and
  ```
  enum Native {
      Absent,                 // engine has no definition (undefined cs)
      Int(i32), Dimen(i32), Glue(GlueRef), MuGlue(GlueRef),
      Mean(Rc<Mean>),         // a decoded meaning (`Mean` as today)
      Toks(Rc<[Tok]>),        // token list register / parameter
      Box(Option<Rc<BoxNode>>),// box register, native node (nodes.rs)
      Font(u32), Raw(ValueId),// anything else: kept as the engine value
  }
  ```
  `Native` is the ONLY representation touched by the run. Decode happens
  once per entry per run on first read (`Unloaded -> Loaded`), encode once
  per dirty entry at publish.

- Reads: `state.get(loc) -> &Native`. On `Unloaded`: `cx.use_field(field_of(loc))`
  (this is the read set the engine needs; one call per distinct field per
  run), decode, store, return. On `Loaded`/`Dirty`: return the slot. No
  hashing, no `Option<Option<>>`, no `own`/`over` lookups.
  `over` (a page step's `\count0..9` overlay, `page.rs` `page_step`) becomes
  `state.preload(loc, native)` before the run starts: a `Loaded` entry
  whose read never reaches the engine.

- Writes: `state.set(loc, native, locality)`:
  - local inside a group: push `Saved { loc, prev: Entry }` on `save`
    (the existing `nest.rs` save stack moves here; `pop_level` restores
    by `Loc`); globals follow §283 exactly as `set_f` does today
    (`ginert` logic expressed on the saved entries, not a hash map).
  - mark `Dirty(locality)`, push `loc` on `dirty` once (a bit per entry,
    not a search).
  - `irreversible += 1` stays (the engine's park/resume contract).

- Publish (`run.rs` `finish`): for each `loc` in `dirty` with a definition
  that is still in force at the run's base level: encode `Native` to a
  `ValueId` once and `cx.define_with(field_of(loc), value, locality)`. The
  `defs`/`def_ix` vectors and hash maps are deleted; `contribs` stays.
  A dirty entry whose final value equals its loaded value is NOT published
  (value-compare on the encoded `ValueId`): fewer definitions, same result.

- `mean_of(tok)` becomes `state.get(mean_loc(tok))` and `set_cur` reads
  `cmd/chr` off the `Rc<Mean>` without cloning `Mean` (`Cur` holds the
  `Rc`). `mcache`, `tfields`, `afields`, `own`, `over`, `defs`, `def_ix`,
  `ginert`, `slots`, `slot_get/slot_set/slot_note` are deleted; `ipar`,
  `par`, `qget`, `reg`, `box_reg`, `set_box_reg`, `int(name)`,
  `get(name)`, `set(name, ..)` are rewritten over `Loc`; the `&str` forms
  (`self.int("deadcycles")`, `self.set("box:255", ..)`,
  `format!("count:{k}")`) are replaced by `Loc` constants or
  `Loc::count(k)` at every call site (grep `self.int(&format`,
  `self.get(&format`, `self.set(&format`, `get_at(`, `"box:`, `"count:`).

- Macros: a macro meaning is `Mean::Macro { params, body: Rc<[Tok]> }`
  already decoded natively; `macro_call` reads the `Rc<[Tok]>` directly
  (no decode, no re-encode on `\def` inside the run unless published).

- Alignment, math, box building, page builder: nodes stay in `nodes.rs`'s
  native form inside the run; `encode` only on contribute/publish (the
  decode agent's cache work is subsumed: an entry decodes once).

### 3. What the engine sees (unchanged contract)

- Reads: exactly the fields read through `Unloaded -> Loaded` (plus the
  keyed slow path). Same set as today, so hints, claims, parks and
  speculation behave the same.
- Definitions: the dirty entries at base level, value-compared.
- Scopes, contributions, kids, effects: unchanged.
- Byte-identical output on every gate; the scheduler never changes results.

### 4. Measurement (every stage, report numbers not percentages)

- Format heartbeat (as fmtperf/interning/getnext/expand did): latex.ini
  through `latex_hello`, release, accl `-c 18 --mem=64G`, seconds and
  µs/command at 2M and 12M, before (dev head) and after.
- Callgrind at 2.8M commands: top ten exclusive, before and after.
- para400 / para4000 cold wall at 1/4/8 threads and the profile test's
  per-paragraph Ir (parcost branch harness).
- hello (pdflatex.ini) warm cold start and one-word edit wall.

## Rules for this agent

- Worktree `~/code/tmp/phitex-runstate`, branch `runstate` from the main
  checkout's dev (`git fetch ~/code/flinner/PhiTeX dev`); merge dev again
  right before your first build and before the final gate. Push to
  `github runstate`.
- ONE SHOT: type the whole change (Tok, runstate.rs, fields.rs ranges,
  every call site) before the first `cargo check`. Then at most two
  fix-and-build rounds per stage. Stage 1: Tok as u32 + meanings and
  parameters through `State` (delete mean_of/own/defs/slots). Stage 2: all
  families. Build and gate after each stage; push after each.
- Builds, checks, tests, timing: on the accl cluster through
  `ACCL_OPTS="-c 4 --mem=12G -x harrier" scripts/accl/accl run cmd scripts/sandbox cargo ...`
  (`-c 18 --mem=64G` for the heartbeat and callgrind). Tests use
  `--profile fastdev`; timing uses `--release`. fmt and clippy may run
  locally through `scripts/sandbox`. Do not run tests locally.
- Never touch `crates/vendor/`. Never edit expected outputs. No `git stash`.
  No `pkill`/`pgrep -f`; kill by PID. No session links in commits.
- Gate before every push: plain, ini, etex, core, edits, page_steps, hints,
  diagnostics, pdftex (pdf_state_cutoff is a known red), latex_hello,
  pdflatex_hello; clippy pedantic clean; fmt clean.
- Report ONLY by SendMessage to "main": after stage 1 and stage 2, with
  the hash, the heartbeat table, the callgrind top ten, and what is left.
  Heartbeat message every 45 minutes: done / left / ETA, five lines.
- The vocabulary: this is a run-local state table with slots, not an
  "eqtb"; do not use that name anywhere.
