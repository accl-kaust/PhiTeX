# The editor protocol: LSP for the editor, the viewer protocol for pages

Status: decided 2026-10-29 with the frontends (Phi Frontends); built in
steps A (progress), B (LSP features), C (the wasm core speaks LSP), D
(phitex-shelf). This file is the contract: every method, field and event
a client may rely on. A change here is agreed with the frontends first.

Every client (VS Code, Neovim, Emacs, the Overleaf extension) is a thin
client over two protocols, both PhiTeX's and the same native and wasm:

1. **LSP** (JSON-RPC 2.0) for the editor: buffers, diagnostics,
   progress, messages, commands, the outline, settings.
2. **The viewer protocol** (`phitex_live::view`: `frame`, `strips`,
   `text`, `outline`, `origins`, `source`, ...) for the pages. Unchanged
   but for the `progress` event (§2.1). Its editor ops (`set_file`,
   `edit`) stay for the viewer's own console; an editor speaks LSP.

There is one LSP server, `phitex_live::lsp::Server`, transport-agnostic:
one JSON-RPC message in (`Server::handle`), the server's messages out
through a callback. Two transports wrap it:

| host | in | out |
|---|---|---|
| native `phitex watch --lsp` | stdin, `Content-Length` framing | stdout, the same framing |
| wasm core (frontends' `core/`) | export `ph_lsp(ptr, len)`: one message, UTF-8 JSON, no framing | import `phitex.lsp(ptr, len)`: one message, UTF-8 JSON, no framing |

What the server asks of its host is `Ask` (the viewer's asks:
`SetFile`, `Edit`, `Closed`, `Cursor`, `Rebuild`, `Cold`) and the
host's commands (§3.3); what it is told is the session's progress
records (§2), a build's start and its `Built`, the console's items, a
source place to show. Native and wasm feed it the same calls from the
same records, so they cannot drift.

## 1. Lifecycle

- `initialize` first. Read from `params`:
  - `rootUri` (else `workspaceFolders[0].uri`, else `rootPath`): the
    project's root; file names (the watch's, the viewer's) are paths
    relative to it. A `file:` root is a directory; any other scheme (the
    Overleaf extension's `overleaf://PROJECT/`) is a prefix: a URI's name
    is what follows `ROOT/`, percent-decoded.
  - `capabilities.window.showDocument.support`,
    `capabilities.window.workDoneProgress`,
    `capabilities.workspace.configuration`: what the server may ask.
  - `initializationOptions`: `{main?, threads?, follow?, files?}` (the
    settings of §3.5, read before the first build; `main` as before).
- Answered with:

```json
{"capabilities": {
   "positionEncoding": "utf-16",
   "textDocumentSync": {"openClose": true, "change": 2, "save": {"includeText": false}},
   "executeCommandProvider": {"commands": ["phitex.openViewer", "phitex.rebuild", "phitex.showOnPage",
       "phitex.stop", "phitex.savePdf", "phitex.clearCache", "phitex.cacheStats"]},
   "documentSymbolProvider": true},
 "serverInfo": {"name": "phitex", "version": "0.0.x"}}
```

  Natively the answer waits for the main file (as today: the watch's
  argument, `phitex.toml`, `initializationOptions.main`, the one `.tex`
  with a `\documentclass` at the root, else the first `.tex` document
  opened); a watch that cannot start answers `initialize` with an error
  (`-32603`, why).
- `initialized`: the server asks `workspace/configuration`
  (`{items: [{section: "phitex"}]}`) where the client can answer it, and
  sends `phitex/status {state: "starting"}`.
- `shutdown` answered `null`; `exit` (or the transport's end) ends the
  server: natively the watch exits 0, as `q` does.
- The server's own requests have string ids `phitex-N`; their answers
  are read (`workspace/configuration`, `window/showMessageRequest`) or
  ignored.

## 2. Progress: one record, three views

The session keeps one progress record (`phitex_live::progress::Progress`,
made by its `Meter`, which `Status` holds): the engine's pages and the
files being fetched. The viewer's `progress` event, LSP `$/progress` and
`phitex/status` are all derived from it.

```
Progress = {phase: "fetching" | "typesetting",
            pass: n,          // this build's engine evaluation, from 1
            pages: n,         // pages shipped so far in this pass (0: none yet)
            name?: string,    // fetching: the file whose pack is fetched ("amsmath.sty")
            pack?: string,    // fetching: Shelf's pack id ("amsmath-3f9a2c41b0de")
            done?: n, total?: n}  // fetching: packs fetched / asked so far in this build
```

- `typesetting` records come as the engine reaches a new page, at most
  one per 100 ms (a new pass is always told). No timer: a record is made
  only when the engine moves.
- `fetching` records come when a pack is asked (`done` the packs
  finished, `total` those asked, this one included) and when it arrives
  (`done` one more). Native: the Shelf source (§5); wasm: the host's
  `phitex.fetch`, through the same `phitex-shelf` files. With
  `files = "texlive"` there are none.

### 2.1 The viewer event

```json
{"event": "progress", "phase": "fetching", "pass": 1, "pages": 0, "name": "amsmath.sty",
 "pack": "amsmath-3f9a2c41b0de", "done": 12, "total": 40}
{"event": "progress", "phase": "typesetting", "pass": 1, "pages": 3}
```

Absent fields are left out (never `null`). Sent between `preparing
{on: true}` and `preparing {on: false}`; `settled` still says a build is
in.

### 2.1b The exact file: `written`

`{"event": "written", "exact": true}` (the viewer's; `phitex watch
--json`'s adds `build` and `output`): the file on disk is the exact
one. Said at every idle point after a build where it is exact, whether
the watch had to write it there (a PDF kept as update sections while
the editor types, compacted when idle) or it was exact already (a DVI
file, `--exact`, a build that wrote it whole): a client waiting for the
exact file never waits for an event that does not come. Not said when a
build came meanwhile (its own idle point says it).

### 2.2 LSP `$/progress`

Only where the client said `window.workDoneProgress`. A build's token is
made with `window/workDoneProgress/create` (`{token: "phitex-build-N"}`)
and used at once (the client's answer is not waited for).

- **When.** The first build always; any later build only once it has run
  400 ms (at its first progress record past 400 ms: typing never
  flickers).
- `begin`: `{kind: "begin", title: "PhiTeX", message, percentage?, cancellable: false}`.
- `report`: `{kind: "report", message, percentage?}`, from each record:
  - fetching: `"Fetching amsmath.sty (12 of 40)"`, `percentage` =
    `done * 100 / total`;
  - typesetting: `"Typesetting page 3"`, and while the last build's page
    count is known and this pass is the first, `percentage` = `pages * 100 /
    last pages` (at most 99); from pass 2 on `"Typesetting page 3 (pass 2)"`.
- `end`: `{kind: "end", message}`: `"Built in 67 ms, 3 pages"` (one page:
  `"1 page"`), or `"2 errors"` (`"1 error"`), or `"The engine failed"`.

### 2.3 `phitex/status` (notification)

```json
{"state": "starting" | "fetching" | "typesetting" | "ready" | "failed" | "stopped",
 "ms": 67.2, "pages": 3, "errors": 0, "warnings": 2, "engine": "native" | "wasm"}
```

- Sent at every change of `state` and after every build.
- `starting`: after `initialized`, before the first build is in.
  `fetching` / `typesetting`: the record's phase (sent when it changes,
  not per record), in the builds whose progress is shown (§2.2: the
  first, and those past 400 ms; a keystroke's build goes from `ready` to
  `ready`). `ready`: a build is in without errors; `failed`: a
  build is in with errors, or the engine failed. `stopped`: the server
  ends on `phitex.stop` (the last message before the exit: a client
  tells a stop from a crash by it).
- `ms`, `pages`, `errors`, `warnings`: the last build's (`ms` its wall
  time; 0 and absent before the first build is in: `ms`/`pages`/`errors`/
  `warnings` are always present, 0 until then).

## 3. Editor features

### 3.1 Buffers (as today)

`didOpen`, `didChange` (ranged, UTF-16, or whole), `didClose` are the
viewer's `set_file`, `edit`, and `closed` (the file as on disk again).
While open the buffer owns the file (natively the disk's changes of it
are not read until it is closed). `didSave` brings nothing.

The cursor: notification `phitex/cursor {textDocument: {uri}, position}`
(the client sends it as `follow` says, §3.5, debounced): the build finds
the place on the pages and the viewer reveals it (`reveal`).

### 3.2 Diagnostics and messages

- `textDocument/publishDiagnostics` after each build, by file (as today:
  the main file always, files that had some cleared).
- `window/logMessage`: the terminal's lines (errors 1, warnings 2, notes
  3, lines 4).
- `window/showMessage` / `window/showMessageRequest` for what needs the
  user, each once until it changes:

| what | type | message | actions → what the server does |
|---|---|---|---|
| the engine failed (a panic, `code: "engine"`) | 1 | `PhiTeX: the engine failed: …` | `Rebuild` → a cold build |
| the format could not be built | 1 | `PhiTeX: the format did not build: …` | `Rebuild` → a cold build |
| a package could not be had (Shelf: unreachable, or refused by the pack table) | 2 | `PhiTeX: a package could not be had: FILE: why` | `Retry` → a build |
| a setting needs a new session (§3.5) and it failed | 1 | `PhiTeX: …` | none |

  `showMessageRequest` where the client has it (it always does in LSP
  3.17); its answer `{title}` runs the action.

### 3.3 Commands (`workspace/executeCommand`)

| command | arguments | result |
|---|---|---|
| `phitex.openViewer` | none | the viewer's URL (native: a browser opened) |
| `phitex.showOnPage` | `[{textDocument: {uri}, position}]` | `null` (as `phitex/cursor`) |
| `phitex.rebuild` | none | `null`; a **cold** build: the session's build dropped, the format kept |
| `phitex.stop` | none | `null`; then `phitex/status {state: "stopped"}` and the server ends cleanly (native: exit 0) |
| `phitex.savePdf` | `[{path}]` | `{path, bytes}`: the last build's PDF written to `path` (absolute, or relative to the root). Wasm (no file system): `[{}]` answers `{bytes, data}`, `data` the PDF in base64, which the client writes |
| `phitex.clearCache` | `[{scope}]`, `scope` `"build"` (default) \| `"packages"` \| `"all"` | `{freed_bytes}` |
| `phitex.cacheStats` | none | `{caches: [Cache...]}` |

Errors: `-32602` (bad arguments, with why), `-32603` (it failed, with
why), `-32601` (no such command).

**Caches.** The user sees and clears them:

```
Cache = {scope: "build" | "packages",
         location: string,   // native: the directory, ~ for $HOME; wasm: the host's name for its store
         bytes: n, entries: n,
         last_used: string | null,   // ISO 8601 UTC ("2026-10-29T14:03:11Z"), the newest entry's use; null: empty
         hits: n, misses: n}         // this session's
```

- `build`: the formats and the session's records (native:
  `PHITEX_CACHE_DIR`, else `~/.cache/phitex`, without its `shelf/`).
  `hits`/`misses`: the format and the records found / not found this
  session.
- `packages`: Shelf's packs and the index (native:
  `~/.cache/phitex/shelf/<release>/`; present only with `files =
  "shelf"`, else an entry with zeros). Fetched packs persist across runs
  and documents; `hits` the packs read from the cache this session,
  `misses` those fetched. Clearing them is safe: they are fetched again.
- `clearCache`'s `build` scope empties the build cache (the running
  session's build is kept in memory; the next cold build makes the
  format again); `packages` empties the pack cache (the index too: it is
  fetched again at the next start); `all` both.
- Clients show them: VS Code's "PhiTeX: Caches…" quick pick (a row per
  cache, clear with a confirm, "Clear all"), Neovim's `:PhiTeXCaches`
  and `:PhiTeXClearCache [build|packages|all]`, Emacs's
  `phitex-caches` and `phitex-clear-cache`, the Overleaf popup.

### 3.4 `textDocument/documentSymbol`

The outline of the file asked for, from its text (the open buffer, else
the file), as `DocumentSymbol[]` (hierarchical): `\part`, `\chapter`,
`\section`, `\subsection`, `\subsubsection`, `\paragraph`,
`\subparagraph` (starred too), each with its title (the braced argument,
the optional short title passed over) as `name`, kind `Namespace` (3)
for parts and chapters, `Module` (2) for sections and below; `range`
from the command to the next heading of the same or a higher level (or
the file's end), `selectionRange` the command. `\label{x}` inside a
heading's range is its child: `name` `x`, kind `Constant` (14). Lines
commented out (`%`) are passed over. (The pages' outline, the PDF's,
stays the viewer's `outline` op: it has pages, not source places.)

### 3.5 Settings (section `phitex`)

```
{threads: n | null,                         // the engine's threads (null: every core)
 main: string | null,                       // the main file, from the root
 follow: "cursor" | "selection" | "dblclick",   // when the client sends phitex/cursor: the client's
 files: "texlive" | "shelf"}                // where TeX's files come from (§5)
```

- Read from `initializationOptions` at the start (a client sends its
  settings there too, so the first build is already the right one), then
  `workspace/configuration` (after `initialized`, and at every
  `workspace/didChangeConfiguration`: its `settings.phitex` used when
  given, else asked again).
- `follow` is the client's (the server keeps it, echoed nowhere).
- `threads` null or 0 and `main` null in an answer mean not given: the
  command line's or `phitex.toml`'s stays.
- `threads`, `main`, `files`: a value that differs from the running
  session's starts a new session with it (natively the watch's session
  is made again in the same process, the viewer and its address kept;
  wasm: the host's `ph_open` again). Command-line flags and
  `phitex.toml` are the defaults the settings override.

## 4. The wasm core

- `ph_lsp(ptr, len) -> i32`: one JSON-RPC message in (0: taken; 1: not
  JSON; 2: no document open). Answers and notifications go out through
  the import `phitex.lsp(ptr, len)`, each one message, from whichever
  thread makes it (the host copies before returning).
- `ph_request` stays for the viewer protocol; `ph_push` and the core's
  own `preparing`/`fetching` events go: `ph_push`'s `set_file`/`edit`/
  `closed`/`cursor` are `didOpen`/`didChange`/`didClose`/`phitex/cursor`,
  its `rebuild` is `phitex.rebuild`. The viewer's events stay on
  `phitex.event` (the viewer's `progress` among them).
- The core makes the server with `Document::lsp(out, commands)` before
  it moves the document to its session thread: `out` calls the import
  (from any thread), `commands` runs what the document cannot
  (`phitex.stop`, `phitex.clearCache`, `phitex.cacheStats`: the core's
  store and its host; none: `-32601`) and is told the settings when they
  change, as `("phitex/configure", {threads, main, follow, files})` (a
  new document where threads, main or files differ). The editor's asks go to the
  document's viewer as `ph_push` sent them; the document tells the server
  each build's start, its progress records and its end; `phitex.savePdf`
  is answered with the viewer's PDF (`{bytes, data}`). The server's
  `Server::show(file, line, col)` is what the core's `goto` becomes
  (`window/showDocument`), its `Server::console(item)` what its console
  items become.
- `engine: "wasm"` in `phitex/status`.
- Files: the core's files are `phitex_shelf::Files` with the host's
  `phitex.fetch` as its `PackSource` (§5).

## 5. Shelf (`phitex-shelf`)

`crates/tools/shelf`: Shelf's index (schema 4, release `tl2026.6`; schema
3 read too), the resolve rule per engine (kpathsea's order, from the
release's `search`), the pack container, the pack table's checks, and
`Files`, which finds a name's file for an engine and asks a `PackSource`
for its pack:

```rust
pub trait PackSource: Send + Sync {
    /// Pack `id`'s bytes as Shelf serves them (gzip of the container).
    fn pack(&self, id: &str) -> Result<Vec<u8>, String>;
}
```

- **API**: `Index::new(tsv, release_json)`, `.with_pack_table(tsv)`,
  `.resolve(name, format, engine)`, `.packs(path, engine)`,
  `.check(id, raw)`; `Files::new(index, source, engine)`, `.find(name)`
  / `.locate(name)` (fetching), `.read(path)`, `.key(name)` (a content
  key, nothing fetched), `.on_fetch(hook)` (`Fetch::Asked`, `Arrived`,
  `Failed`); `PackSource::pack(id)`, `.packs(ids)` (a source that fetches
  at once overrides it); `gunzip`, `unpack`, `engine`, `format_of`.
- **The rule** (the frontends' `resolve.ts` and `core/src/shelf.rs`
  until they use this crate): a name with a `/` is a path (TeX Live's
  `/usr/share/texmf-dist/` dropped); else its basename's paths, the first
  prefix of the engine's list for the name's kpathsea format that holds
  any (at any depth), within one prefix the smallest path; none: not
  found; a format or engine the release has no list for: the smallest
  path. Shelf's files are named `/usr/share/texmf-dist/<texmf path>` to
  the engine (as the wasm core names them).
- **Packs**: `u32 n, (u32 len, path, u32 len, bytes) × n`,
  little-endian, gzip'd; schema 4's pack table (`pack TAB sha256 TAB
  bytes`) checks every pack before use: one that differs is refused,
  never read.
- **One source per run.** `files = "shelf"` (phitex.toml) or `--shelf`:
  the run's TeX files are Shelf's and the project's own (its directory,
  first, as kpathsea's `.`), never TeX Live's; `files = "texlive"` (the
  default): kpathsea's, never Shelf's. No fallback from one to the other.
- **Native source** (`crates/cli/src/shelf.rs`): HTTP(S) (ureq, rustls)
  from `PHITEX_SHELF_URL` (the release's directory, `https://HOST/tl2026/`;
  no host is the default until the user chooses one); cache
  `~/.cache/phitex/shelf/<release>/` (`release-<release>.json`, the index,
  the pack table, `h/<pack>.pack`): the index is fetched once per
  release, then everything is offline; a pack is fetched once, checked,
  and kept (one that differs from the table is never kept). The release
  is the binary's (`tl2026.6`), so two machines with the same PhiTeX build
  a document from the same files. A project's own files come first (its
  directory, as kpathsea's `.`); a format is built from Shelf's files
  only.
- Fetch progress (§2) comes from `Files`, through its hook.
- **XeTeX's fonts by name**: the release's font index,
  `fonts/misc/partex/fontindex.pxfi` (a pack of its own: Shelf's
  `build.py` `FONT_INDEX`), `phitex-otf`'s PXFI made where the release is
  built, with fontconfig over the release's `fonts/opentype` and
  `fonts/truetype` (its paths relative to the tree): the faces in
  fontconfig's `FcFontList` order, which XeTeX's name matching follows,
  so no fontconfig is needed where it is used. One a release, fetched and
  kept with its packs (native: the package cache; wasm: the host's).
  Its paths are named under TeX Live's tree (`/usr/share/texmf-dist/
  fonts/...`, as xelatex's XDV names them), each face read from Shelf's
  packs as it is found. A release without one: an XeTeX job with `files =
  "shelf"` is refused at the start, said so (tl2026.6 has none: the next
  release is to be made with `FONT_INDEX`).
