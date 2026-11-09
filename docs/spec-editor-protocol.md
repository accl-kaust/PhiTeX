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
    settings of §3.5, read before the first build; `main` as before), and
    `client: {name, version}`, the extension's own (informational: the
    server needs nothing from it), and `phitexCursor: true` from a client
    that sends `phitex/cursor` (§3.1: its `documentHighlight` requests
    then move nothing).
- Answered with:

```json
{"capabilities": {
   "positionEncoding": "utf-16",
   "textDocumentSync": {"openClose": true, "change": 2, "save": {"includeText": false}},
   "executeCommandProvider": {"commands": ["phitex.openViewer", "phitex.rebuild", "phitex.showOnPage",
       "phitex.stop", "phitex.savePdf", "phitex.clearCache", "phitex.cacheStats"]},
   "documentSymbolProvider": true,
   "documentHighlightProvider": true,
   "experimental": {"phitex": {"protocol": 1}}},
 "serverInfo": {"name": "phitex", "version": "0.0.x"}}
```

  `experimental.phitex.protocol` is this protocol's version (`lsp::PROTOCOL`),
  raised when a message changes its meaning incompatibly; `serverInfo.version`
  is PhiTeX's. An extension needing a newer one says so ("PhiTeX 0.1.4 is
  older than this extension needs: update phitex"), as
  docs/spec-distribution.md §7 has it.

  Natively the answer waits for the main file (as today: the watch's
  argument, `phitex.toml`, `initializationOptions.main`, the one `.tex`
  with a `\documentclass` at the root, else the first `.tex` document
  opened); a watch that cannot start answers `initialize` with an error
  (`-32603`, why).
- `initialized`: the server asks `workspace/configuration`
  (`{items: [{section: "phitex"}]}`) where the client can answer it, and
  sends `phitex/status {state: "starting"}`.
- `shutdown` answered `null`; `exit` (or the transport's end) ends the
  server: natively the watch exits 0, as `q` does. A native server still
  waiting for the first `.tex` document to name the main file ends at
  once: status 0 after `shutdown`, 1 after an `exit` without it (the
  LSP's rule).
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

### 2.1c Pages streamed: `page`

A clean build sends each page as it ships, before the build settles
(an edit's build never streams; its pages come in at once):

```json
{"event": "page", "k": 4, "hash": "5354…", "pages": 12, "exact": true}
```

- `k`: the page, from 0. `pages`: the pages shipped so far (at least
  `k + 1`). `hash`: the page's identity while the build runs. It is
  never a settled page's hash, so a streamed page is always drawn again
  from the build's file once the build settles.
- Until `settled`, the viewer's `frame` and `strips` for page `k` draw
  the streamed page from a PDF file of that page alone.
- `exact`: `true` when that file draws the page as the build's file
  will. Every object the page draws with existed and was complete when
  the page shipped, and is in the file under the number the build's file
  gives it (images from `\pdfximage`, with their soft masks and an
  included page's resources; `\pdfxform` forms; `\pdfobj` objects, and
  what each of them names in turn; of the objects its
  `\pdfpageresources` names, those of the kinds its content draws with:
  a page drawing with no graphics state does not need pgf's). The pages
  before it also left the
  colour stacks as they found them. `false` when the page draws with
  something not complete yet (a `\pdfobj reserveobjnum` filled later,
  objects written at `\AtEndDocument`, such as pgf's shared ExtGState,
  Pattern and ColorSpace dictionaries named through
  `\pdfpageresources`), or a colour may carry into it from an earlier
  page. The page is still drawn, without what it lacks:
  - a missing graphics state is drawn opaque;
  - a missing shading, or a fill with a missing pattern, draws nothing
    (a flat fill would hide what lies under a shaded background);
  - a missing colour space falls back to gray;
  - a missing external object draws nothing.

  A viewer marks such a page quietly, "Preview: final colours and
  shading when the build ends", until the settled page replaces it.
- A page sent with `exact: false` only because the pages before it were
  not known yet (pages ship out of order at more than one thread) is
  sent again with the same `hash` and `exact: true` once they are.
  Treat it as an update of that page's mark only.
- The `status` and `pages` answers list the pages streamed so far
  (`hashes`/`pages`) and, as `"inexact": [k, …]`, those not exact. The
  list is empty once the build is in.

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
 "ms": 67.2, "pages": 3, "errors": 0, "warnings": 2, "engine": "native" | "wasm",
 "root": "/home/me/paper", "main": "paper.tex"}
```

- `root`, `main`: the folder the watch runs in and its main file, named
  from there. The watch may run above the editor's root (a chapter opened
  first, its main file found one folder up, `\input`ing it): a client
  sends the files under `root` to this watch rather than start another.

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

The cursor: notification `phitex/cursor {textDocument: {uri}, position,
range?}` (the client sends it as `follow` says, §3.5); a selection is the
cursor with its `range` (either way round), and a `range` with nothing in
it clears the selection shown. No round trip and no build: the server
takes the place in bytes of the file's text (the buffer it holds, else
the file's), the space the pages' glyph origins are in, and the viewer
shows it from the origins it has, every host the same way (Overleaf's own
cursor goes the same path, from bytes it makes itself):

```json
{"event": "follow", "file": "ch/intro.tex", "at": 812, "word": [808, 815], "line": [790, 842]}
{"event": "select", "file": "ch/intro.tex", "from": 790, "to": 815}
```

- `file`: the project's name of the file, as the origins name it.
- `at`: the cursor's byte; `word`: the word it is in (letters, digits,
  `'`, `-`, any character past ASCII; `null`: none), `line`: its line
  without its end. The viewer marks the word's glyphs, else the line's
  nearest `at`, on the page in view first, and scrolls only when the line
  changed.
- `select` (with a `range` only): the glyphs from `from..to` marked on
  every page; `from == to` clears them.

A client that cannot send `phitex/cursor` (Helix, Zed, Sublime, any LSP
client without PhiTeX's plugin) gets forward search from a standard
request: the server advertises `documentHighlightProvider`, clients ask
`textDocument/documentHighlight {textDocument, position}` as the cursor
moves or rests (VS Code, Zed, Helix; Neovim when configured), and the
server answers `[]` (nothing highlighted) and takes the position as
`phitex/cursor`'s (a `follow`, no selection). Two forward searches per
move would disagree, so a client that sends `phitex/cursor` turns this
off: from the start with `initializationOptions.phitexCursor: true`
(PhiTeX's clients set it), else at the first `phitex/cursor` the
session sees. `phitex.showOnPage` is unaffected.

Page to source stays `window/showDocument`: a double-click on the page
(or an error's place) opens the place in the editor, selected, the focus
taken (`Server::show`); it is the inverse search every LSP editor has.
Text selected on the pages is selected in the editor the same way, the
focus left on the preview: the viewer's `select` op `{file, start, end}`
(bytes of the file, from the selected glyphs' origins) gives the host the
start's and end's lines and columns (`Viewer::on_select`), and the
server sends `window/showDocument` with that `selection` range and
`takeFocus: false` (`Server::select`). An editor's showDocument handler
reveals the range without moving the focus.

### 3.2 Diagnostics and messages

- `textDocument/publishDiagnostics` after each build, by file (as today:
  the main file always, files that had some cleared). A diagnostic's
  `message` is the error's line, then, for TeX's (`code: "tex"`), TeX's
  context lines (`l.4 Hello \hobx` and the rest of the line below it).
  TeX's help ("type `I' and the correct spelling ...") is the interactive
  terminal's advice and is not sent: editors show `message` inline. The
  viewer's problems keep it (`help: string[]`).
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
| `phitex.showOnPage` | `[{textDocument: {uri}, position, range?}]` | `null` (as `phitex/cursor`: `follow`, and `select` with a range) |
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
{threads: n | null,                         // the engine's threads (null: one)
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
  key, nothing fetched: the file's own content sum, the index's
  `file_sums` column, so a format holds across releases that leave its
  files as they were), `.published_format(name)` (the release's own
  format of that store name, its pack fetched; a host gives it as
  `phitex_live::host::Host::published_format`, taken when the store's
  format is none or stale), `.on_fetch(hook)` (`Fetch::Asked`, `Arrived`,
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

## 6. The `--json` event stream

`phitex build --json` and `phitex watch --json` print one JSON object a
line on standard output and nothing else there (the terminal's lines
are not printed; `--json --lsp` is refused). Each object has `event`,
one of those below; a reader ignores events and fields it does not know.
Times are milliseconds (floats), sizes bytes. Read from the code
(`crates/cli/src/watch.rs`, `modern.rs`, `ui.rs`); the build is counted
from 1 in a watch.

| event | when | fields |
|---|---|---|
| `note`, `warning`, `error` | what the terminal says as `note:`, `warning:`, `error:` | `message` |
| `viewer` | a watch's viewer serves | `url` (with its token) |
| `started` | a watch's build begins | `builds` (this build's number), `why` |
| `build` | a watch's build is in | below |
| `build` | `phitex build`'s build is in | below |
| `failed` | a watch's build failed inside the engine (a panic); the watch goes on | `build`, `why`, `message` (its first line) |
| `diagnostics` | a watch's warnings, made once the editing pauses (a `build` event with `diagnostics: null` comes first) | `build`, `diagnostics` (the engine's own checks: a count), `warnings`, `errors` (counts) |
| `written` | the exact PDF file written (a watch writes it when idle) | `exact: true`, `build`, `output` (path) |
| `screen` | an edit painted in the viewer without a build | `build`, `ms` |
| `diff` | a watch's comparison with a past commit shown | `label`, `changes` (count), `errors` (count); or `error` (why it failed) |
| `cache` | after a build, and when a watch ends | `dir`, `bytes`, `limit` (0: none) |
| `clean` | `phitex clean` | `removed` (files), `cache_bytes` (freed); `clean --all`: `cache_bytes` only |

**A watch's `build`:**

```
{event: "build", build: n, why: string,
 notes: [string],                 // the session's notes of this build
 edit: "file:line" | null,        // the line of the main file the edit was on
 ok: bool,                        // no TeX error
 output: path | null,             // the PDF written, if any
 pages: n, pages_changed: [n] ,   // the pages, those that changed
 nodes_run: n, runs: n | null, rounds: n | null, absorbed: n | null,
 parallel: {threads, busy_ms, engine_ms, critical_path, peak_running,
            batches, batches_inline, max_batch,
            hints: {bounded, any, violations, undeclared, stops, pending_waits},
            max_ready,
            speculation: {run, confirmed, mispredicted, fuel_outs}} | null,
 wall_ms: ms,
 stats: {build, why, version, engine, files: "texlive" | "shelf",
         files_release: string | null, wall_ms, phases_ms: {phase: ms},
         hints: {...} | null, nodes: {ran, kept, live}, runs, rounds,
         pages, pages_changed, threads, cores_busy: f | null,
         busy_ms, engine_ms, wasted_ms, resident_kb, values_bytes, ok},
 errors: [string],                // TeX's errors, as the log has them
 diagnostics: n | null}           // the engine's checks; null: the `diagnostics` event says them
```

**`phitex build`'s `build`:**

```
{event: "build", why: string, ok: bool, output: path | null, pages: n,
 wall_ms: ms, nodes_run: n, rounds: n, errors: [string],
 warnings: n,                     // the log's warnings
 diagnostics: n,                  // the engine's checks
 engine_warnings: [string],       // the engine's own warnings (the page layout not settled)
 log: path}
```

The `parallel` and `stats` objects are measurements: their fields may
grow; a client that shows a build needs only `build`, `ok`, `pages`,
`wall_ms`, `output` and `errors`.

## 7. Editor setup

The command is `phitex watch --lsp` (there is no `phitex lsp`), run with
the project's folder as the editor's root. PhiTeX's own clients are the
full setup: the page follows the cursor, a double-click on the page
reaches the source, and they show the caches and status. They live in
[PhiTeX-frontends](https://github.com/accl-kaust/PhiTeX-frontends):

- **VS Code**: the extension *PhiTeX ⚡ Instant* (`accl-kaust.phitex-instant`,
  `apps/vscode/`; not yet on the Marketplace: a `.vsix` built from that
  repository).
- **Neovim 0.11+**: `apps/editors/nvim/phitex.lua` (lazy.nvim:
  `{ "accl-kaust/PhiTeX-frontends", config = function(p) dofile(p.dir .. "/apps/editors/nvim/phitex.lua") end }`).
- **Emacs 29+**: `apps/editors/emacs/phitex.el` (`phitex-mode`, by
  use-package `:vc` or by loading the file).

Any other LSP client runs the server bare: problems, the outline, the
commands, and, with no plugin, forward search through
`textDocument/documentHighlight` (§3.1). Inverse search is
`window/showDocument`, where the client supports it (else the viewer
opens `--editor` / `$EDITOR`). The server answers no hover, completion
or definition: run texlab beside it.

**Neovim 0.11** (built-in client, no plugin):

```lua
vim.lsp.config('phitex', {
  cmd = { 'phitex', 'watch', '--lsp' },
  filetypes = { 'tex', 'plaintex', 'bib' },
  root_markers = { 'phitex.toml', '.latexmkrc', '.git' },
  settings = { phitex = { threads = 4 } },
})
vim.lsp.enable('phitex')
-- forward search: Neovim asks documentHighlight only when told to
vim.api.nvim_create_autocmd({ 'CursorHold', 'CursorHoldI' }, {
  pattern = '*.tex',
  callback = function() vim.lsp.buf.document_highlight() end,
})
```

**Emacs (eglot, no plugin):**

```elisp
(add-to-list 'eglot-server-programs
             '((latex-mode LaTeX-mode tex-mode) . ("phitex" "watch" "--lsp")))
```

eglot runs one server a mode, so this replaces texlab there; `phitex.el`
runs both (or use lsp-mode, which takes add-on servers). eglot asks
`documentHighlight` as the cursor rests (`eglot-ignored-server-capabilities`
must not list `:documentHighlightProvider`).

**Helix** (`languages.toml`):

```toml
[language-server.phitex]
command = "phitex"
args = ["watch", "--lsp"]

[[language]]
name = "latex"
language-servers = ["texlab", "phitex"]
```

**Zed** (`settings.json`; Zed may want an extension to name a new server):

```json
"lsp": { "phitex": { "binary": { "path": "phitex", "arguments": ["watch", "--lsp"] } } },
"languages": { "LaTeX": { "language_servers": ["texlab", "phitex"] } }
```

**Sublime Text** (the LSP package, `LSP.sublime-settings`):

```json
"clients": {
  "phitex": {
    "enabled": true,
    "command": ["phitex", "watch", "--lsp"],
    "selector": "text.tex.latex"
  }
}
```

**VS Code without the extension**: a generic LSP client extension, its
server command `["phitex", "watch", "--lsp"]`.

## 8. Attaching to a running watch

Every `phitex watch`, with `--lsp` or not, also serves the same language
server to editors that attach to it: a watch started in a terminal can
be used from Neovim or Emacs without a second session
(`crates/cli/src/attach.rs`).

**Transport.** One on every system: a TCP listener on `127.0.0.1` at a
port the system chooses, the same `Content-Length` framing as standard
input and output. Each connection is a client of its own.

**Discovery.** Each watch writes one file, `<pid>.json`, in the sessions
folder:

| system | folder |
|---|---|
| Linux, macOS | `$XDG_RUNTIME_DIR/phitex/`, else `~/.cache/phitex/sessions/` (`$PHITEX_CACHE_DIR/sessions/` where that is set) |
| Windows | `%LOCALAPPDATA%\phitex\sessions\` |

```json
{"pid": 4242,
 "main": "/home/u/paper/paper.tex",
 "root": "/home/u/paper",
 "viewer": "http://127.0.0.1:24817/3f0c.../",
 "lsp": {"host": "127.0.0.1", "port": 40125, "token": "9d1e5c0a7b3f48e2a6c4d8b0f1e2a3c4"},
 "version": "1.0.0",
 "started": "2026-11-09T10:21:07.512Z"}
```

- `main` is absolute, `root` the folder the watch runs in (file names
  are relative to it), `viewer` null without a viewer (`--no-view`).
- The folder is made 0700 and must be the user's alone (one another user
  owns, or a link, is refused: the watch then serves no socket); the file
  is 0600, as it holds the token and the viewer's own. It is written
  whole (beside, then renamed), again when the viewer or the main file
  changes, and removed when the watch ends.
- A file whose watch does not run (its process gone, or its port not
  answering) is ignored and removed by its readers.
- `phitex sessions` lists the running watches; `phitex sessions --json`
  prints their files' objects as one JSON array on one line.
- An editor picks the watch whose `root` holds the buffer's file,
  preferring the one whose `main` is that file.

**Connecting.** The first message must be `initialize` with
`initializationOptions.token` (with `phitexCursor` and the settings as
on standard input); it must come within 3 seconds and 64 KiB. Otherwise
the connection is closed with no answer (the token is compared in
constant time; at most 16 connections wait for their first message at
once). The answer is as §1's, with
`capabilities.experimental.phitex.attached: true`; the client is then
told the viewer (`window/logMessage`) and the last diagnostics.

**Many clients on one session.**

- `didOpen`/`didChange` apply as from standard input; the last edit
  wins: a client's ranged change of a file another client edited last is
  sent as its whole text. A file one client closes that another has open
  is that client's text; closed by all, the disk's again.
- `phitex/cursor` and `documentHighlight` (§3.1) come from whichever
  client moved; the client whose cursor or edit came last is in focus.
- Inverse search (`window/showDocument`) goes to the client in focus,
  else to every client.
- Builds, progress, diagnostics, `phitex/status` and the log go to every
  client.
- An attached client's `exit`, its connection's end and `phitex.stop`
  only detach it; its settings (`threads`, `main`, `files`) change
  nothing. The watch belongs to its terminal (or to the editor on its
  standard input, `--lsp`) and keeps running.
