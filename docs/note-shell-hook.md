# The shell hook: `\write18` through the host (2026-10-28)

Status: the contract a host implements to run TeX's restricted (or, when
the user asks, unrestricted) shell escape. The engine decides which command
runs and how it is quoted, which files it is handed, and what its results
mean to the job; the host only runs the command it is given over the files
it is given. The native CLI runs it as a process; the wasm hosts run it
with `tools/minted-pyodide` (latexminted in Pyodide).

## Who decides what

- **The engine** (`phitex_tex`, `crates/engines/tex/src/shell.rs`):
  - whether `\write18` is on: `Options::shell_escape` (`\pdfshellescape`:
    0 off, 1 unrestricted, 2 restricted, TeX Live's default `p`);
  - in restricted mode, whether the command is allowed and how it is quoted
    (web2c's `runsystem` and `shell_cmd_is_allowed`, texmfmp.c, Unix):
    - its first word must be in `Options::shell_commands` (default TeX Live
      2026's `shell_escape_commands`: `bibtex`, `bibtex8`, `extractbb`,
      `gregorio`, `kpsewhich`, `l3sys-query`, `latexminted`, `makeindex`,
      `memoize-extract.pl`, `memoize-extract.py`, `repstopdf`, `r-mpost`,
      `texosquery-jre8`);
    - every argument is quoted with `'`; a user's `"…"` becomes `'…'`;
    - a `'` in the command, or a `"` not closed or not followed by a space
      or the end, is a quotation error: nothing runs;
    - a `|` in the quoted command is refused;
    - so the host receives, for `\write18{latexminted config --timestamp
      20260101000000 4B2C}`, the string
      `latexminted 'config' '--timestamp' '20260101000000' '4B2C'`;
  - in unrestricted mode the command is handed over as written;
  - `bibtex`, `biber` and `makeindex` run in process (`crate::tools`),
    never through the hook;
  - with no hook, every other command is refused with a diagnostic, as
    before the hook existed;
  - the log line, as pdfTeX writes it: `runsystem(CMD)...executed safely
    (allowed).`, `...executed.`, `...disabled (restricted).`,
    `...quotation error in system command.`
- **The host** runs the command string, as `/bin/sh -c` would, in a working
  directory holding the files it was handed, and reports what changed.

## The Rust contract

```rust
/// What a command did (`ShellHook::run`).
pub struct ShellRan {
    /// The wait status, as C's `system` returns it (`127 << 8`: not found).
    pub status: i32,
    /// The files it made or changed, by their names relative to the
    /// working directory, `/`-separated, with their bytes.
    pub wrote: Vec<(String, Vec<u8>)>,
    /// The names of the given files it removed.
    pub removed: Vec<String>,
    /// The names of the given files it read or looked at, if the host can
    /// tell; `None`: assume all of them.
    pub read: Option<Vec<String>>,
}

/// Runs the commands `\write18` allows (`Options::shell_hook`).
pub trait ShellHook: Send + Sync {
    /// Runs `command` (quoted by the engine, above) in a working directory
    /// holding exactly `files` (name relative to it, bytes).
    fn run(&self, command: &str, files: &[(String, std::sync::Arc<[u8]>)]) -> ShellRan;
}
```

`Options::shell_hook: Option<Arc<dyn ShellHook>>`.

Rules for an implementation:

1. **Any thread, any time, any number of times.** The engine calls it from
   its workers, possibly several at once, and possibly for a run that is
   later thrown away (a run on inputs that turned out stale). So a call
   must not leave anything behind that the job or a later call could see:
   each call gets a fresh working directory, and what the command did
   reaches the job only through the returned `ShellRan`.
2. **Same inputs, same result.** The engine memoises a call by the command
   and the contents of the files it read (`read`, else all of `files`);
   a host whose command depends on something else (the clock, the
   network) gets the first answer again.
3. **Only the given files are the job's.** The working directory starts
   with exactly `files`; a host may let the command find other files read
   only through its own search (the native host lets `kpsewhich` find the
   project's files and TeX Live's), and those reads are not tracked: a
   change of such a file alone runs nothing again. (minted's
   `\inputminted` file is md5-summed by TeX itself, so a change of it
   changes the data file the command reads.)
4. **`wrote` and `removed` are relative to the working directory**; a
   name under a directory the command made (`_minted/…`) is reported by its
   path. Hidden directories (`.…`) may be skipped.
5. **`read`** is an optimisation: with it, a command is not run again when
   only files it did not read changed (minted's `config` does not read the
   cache directory its `clean` reads). A host that cannot tell returns
   `None`. A command that lists a directory depends on what is in it: the
   host returns `None` then.

## What the files are

At the place of the `\write18` in TeX's order, the engine hands the host:

- every file the job wrote and closed before that place, as the job wrote
  it (minted's `_<md5>.data.minted`, written and closed right before its
  command); a file still open is not handed (pdfTeX's command would see
  whatever stdio had flushed);
- every file earlier commands left, with what they removed taken away: the
  build's store (below) changed by the commands before this one in this
  pass.

The command's results are what the job reads after it: a file it wrote is
read, by its name, from what it wrote (minted's `\input` of
`_minted/<md5>.highlight.minted`, `\InputIfFileExists` of
`_<md5>.config.minted` and `.message.minted`); a name it removed is absent.
A read before the command, in TeX's order, does not see them in this pass.

## The store across passes

The files commands make are not loop-backs (a read at the front of the
document of a value written at its back): minted writes a highlight file
only where it is missing, and a read of the final value would find it,
skip the command, and lose the file the next pass. They behave as the
working directory does for `pdflatex` run again and again:

- after an evaluation the engine applies the commands' results, in TeX's
  order, to the build's store of command files (kept across edits);
- if the store changed, the files are what the next evaluation starts
  from (a read before any command sees them), and the build evaluates
  again, as `latexmk` runs `pdflatex` again; capped as latexmk caps its
  runs, with a warning past the cap; the build's stats count these
  evaluations (`Stats::shell_passes`);
- a cold minted build so takes two: the first as pdflatex's first run
  (placeholders, `latexminted batch` at the end), the second as its second
  (the highlighted code read from the cache);
- an edit of one listing runs that listing's `highlight` and the end's
  `clean`, and nothing else.

`Build::shell_files()` gives the store (name, bytes; `None` for a name
removed), which the native CLI writes into the output directory as
pdflatex leaves `_minted/` there.

## The native host (the CLI)

- `/bin/sh -c COMMAND` in a fresh scratch directory per call holding only
  the given files;
- the environment: kpathsea's `SELFAUTOLOC`, `SELFAUTODIR`,
  `SELFAUTOPARENT` (latexminted's `latexrestricted` finds TeX Live by
  them), and `TEXINPUTS` with the document's directory first, so
  `kpsewhich` finds the project's files read only;
- `wrote`: the files new or changed in the directory after the command,
  `removed`: the given ones gone;
- `read`: every given file's access time is set to the epoch before the
  command; a file whose access time moved was read; probed once per
  process (a file read by the host itself), and `None` where access times
  do not move (`noatime`), or when a directory's moved (listed);
- unrestricted (`shell_escape = 1`) only with `-shell-escape` or
  `shell-escape = true` in `phitex.toml`; restricted otherwise.

The runner lives in the CLI and is passed in through `Options`; nothing in
the engine spawns a process or touches the file system.

## The wasm hosts (`tools/minted-pyodide`)

phitex-tex has no wasm imports for this: a wasm host implements
`ShellHook` in Rust, with whatever runs the command behind it
(`tools/minted-pyodide`'s runner):

- its `run` is called on an engine worker; it posts the framed call
  (`u32 n, (u32 len, name, u32 len, bytes) × n`, little-endian, as
  `host-glue.mjs` frames files) to the host's thread that holds Pyodide
  (thread 0) and waits on a `Condvar` for the answer (`i32 status`, the
  files written framed, the names removed framed with no bytes);
- `files` is exactly what `run` is given: the host adds nothing of its own
  (the old glue added the host's project files and its own written files:
  the engine now hands those it means the command to see);
- `runner.system(command, files)` already returns `{status, wrote,
  removed}`; `read` is `None` unless the runner reports it (Emscripten's
  `FS.trackingDelegate.onOpenFile` can: names opened under `/work`, and
  `None` once a directory under `/work` is listed);
- each call must start `/work` as exactly the given files: the runner's
  copy-only-what-differs must also remove the files of an earlier call
  that are not given this time;
- the engine's options: `shell_escape: 2` and the default
  `shell_commands` (latexminted is in it).

## Time, threads, thrown-away calls

- **A call may take seconds** (Pyodide's first load, about 10 MB, then
  latexminted's start). `run` is called inside the command's node run, with
  no engine lock held that another worker needs: the rest of the
  evaluation goes on while a command runs (the nodes before it, and the
  ones after it that do not read what it makes); only its readers wait.
  A test holds this: a fake hook that sleeps 2 s while 300 paragraphs on
  the other workers finish.
- **The engine never assumes two calls run in parallel.** It may make
  several at once from different workers; a host may serialise them (the
  wasm hosts do, on Pyodide's thread). Since a call's result depends only
  on its command and files, the order they run in changes nothing.
- **A thrown-away call is a known cost.** The engine runs a command's node
  as soon as the files at its place are known, as it runs any node; where
  what comes before it is still moving (a cold build's first pass, a run
  on a value that is later refuted), the call may be made on files that
  turn out stale and its result dropped. The engine has no way to hold a
  node until its inputs are final that the text could still wait on
  (`Operation::final_only` nodes run only at quiet points, and nothing the
  text reads may come from one), so such a call is made and its time
  spent; a call with the same command and files as any earlier one is
  answered from the memo without calling the host. In wasm a dropped call
  costs its seconds.
