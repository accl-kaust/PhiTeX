# amsldoc's serial preamble: measurements and two designs (2026-10-21)

Status: a design report ([spec-page-chain.md](spec-page-chain.md) P6). Nothing
is built. Caching across builds (package records from an earlier build) is
left aside; it is the user's open question.

Sources:

- the clean-core recording of amsldoc -j 8 (`amsldoc-j8-48b8e846-clean.rec.gz`);
- the profile runs of job 15453 (`PHITEX_TEX_TIME`: `nodetime` lines, `acct`);
- `bench/r6/preamble.py` (`python3 -I bench/r6/preamble.py REC.gz 3,51,53,56,170`).

## What the preamble is

amsldoc's preamble is five load nodes plus the node that begins the
document body. Each load is one node, which runs the whole file a load
command names, including every file that file loads in turn.

| node | time (-j 1) | reads values only these earlier loads wrote |
|---|---|---|
| load 3 (the class) | 14.8 ms | none |
| load 51 | 39.3 ms | 3: 20 |
| load 53 | 3.7 ms | 3: 4, 51: 30 |
| load 56 | 30.8 ms | 3: 10, 51: 14, 53: 4 |
| load 170 | **820 ms** | 3: 37, 51: 39, 56: 75 |
| node 180 (the body's beginning) | 24-27 ms × 4 runs | 170: 304, 51: 32, 3: 27, 56: 23 |

- The loads form a chain. Each reads tens of values that only an earlier
  load wrote, so in the order they run they are not independent.
- Load 170 is 88% of the load time.
- The serial part before the body:
  - prelude 94 ms, loads 909 ms and node 180's runs ~100 ms: about 1.1 s;
  - that is 27% of the -j 1 wall (4.0 s);
  - at -j 8 the loads take 962 ms (the same serial work, slower per
    operation), about 25% of a 4.5-4.75 s wall.
  - Nothing else runs meanwhile: no body piece can start (next section).

## What the body waits for

Of the 1259 pieces after the preamble:

- **1060 read a value only node 180 wrote.**
  - The fields: the category-code table (935 pieces), the hook macros the
    body's beginning finalizes (the paragraph hooks, 295-431 pieces each),
    the font defaults (`f@series`/`f@shape`/… 127, the size and
    line-spread macros ~300).
  - These are set where the body begins. A body piece cannot start before
    node 180 has run, and node 180 reads 304 values of load 170.
- 364 pieces read a value only load 170 wrote: active characters'
  meanings, the index-writing and item commands it redefines, and its
  string-conversion switches.
- Only 59 pieces read nothing later than loads 3/51/56. They are still
  behind node 180 through the category-code table.
- So the body depends on the whole preamble through two ordinary paths:
  - the body's beginning (node 180), which finalizes hooks and fonts;
  - load 170's redefinitions.

## Design (a): the body starts on guessed definitions

- **Body pieces run before the preamble ends, on guessed values.** A
  wrong guess is a refutation and a rerun, as for any guess.
- **Guesses from inside one cold build:** the only state there is the
  format's.
  - Compared with the format's environment, **21 of the 1084 body pieces**
    read only format-equal values of the fields the preamble defines.
  - The other 1063 would run twice. The differing fields: the
    category-code table 936, the hooks 295-431, the font defaults ~300,
    hyphenation patterns 321, and load 170's redefinitions ~180-270.
  - Gain on a cold build: none. It adds ~1000 wasted runs, and the body's
    second runs still wait for node 180.
- **Guesses from an earlier build** (the previous build's values, or the
  package records' predicted outputs, `Options::pkg_predict`):
  - Every guess is right while the preamble is unchanged.
  - The body overlaps the preamble. At -j 8, about 1.0-1.1 s comes off
    the critical path: amsldoc -j 8 ≈ 4.5 → ~3.5 s, other limits aside.
  - That is the caching question, so it stays the user's call.

## Design (b): independent loads in parallel

- **Top-level loads in parallel: no gain.** They form a dependency chain
  (table above), and a load reading another's 20-75 values would be
  refuted and run again.
- **The gain is inside load 170.**
  - It runs 58 file opens in one run: about 20 generic utility files
    (key-value parsing, string escaping, integer arithmetic, encoding
    tables), then its own configuration and driver.
  - The recording shows only what each file read first from the engine.
    Definitions are published at the run's end, so the dependencies
    between these files inside the run cannot be seen.
  - Engine reads before the run's definitions, 10821 in all, by file: a
    Unicode encoding table 3472 (32%), big-integer arithmetic 1435 (13%),
    a string-encoding file 571, integer arithmetic 576, the title-string
    configuration 579, the toolbox 451, the main file ~590, the rest
    under 200 each.
- **The rule:**
  - A file a run loads is a slice of its own: a node like any other,
    placed where the load command was.
  - It runs on the definitions reaching it (guessed while earlier files
    are in work), and the engine refutes wrong guesses.
  - This is the cuts rule applied to file boundaries. The load command's
    rest (what follows the file in the file that loads it) is the next
    piece.
  - A file that only defines macros from the kernel's primitives reads
    nothing of its siblings and runs in parallel with them.
- **Expected gain, to be measured first:**
  - If the utility files are mostly independent (each reads the kernel
    and one or two base utilities), the encoding table and the
    arithmetic files run beside the main file.
  - Load 170's 820 ms could drop to roughly its longest dependency chain.
    By the read counts above, ~40-50% of it, i.e. 0.35-0.45 s off amsldoc
    at -j 8.
  - Also unknown: guess quality inside a cold build. A file run early on
    the kernel's definitions is refuted wherever it uses a sibling's
    macro.
- **Needed before building:** a profile-only per-file timer inside a run
  (time from a file's open to its close, nested), and an observe-only
  trace of which file's definitions a later file reads in the same run.
  Both are small. They turn the read-count proxy into times and a real
  dependency graph.

## Per-command speed (outside both designs)

Load 170 takes 820 ms in one run. pdfTeX's time for the same load was
not measured here; it is worth measuring on the same node. The per-command cost of a run on package code is the
largest single lever on this preamble (r4-speed's line of work). Every
design above only overlaps or splits that time; none removes it.
