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
  server: natively the watch ends as `q` ends it, with status 0 after
  `shutdown` and 1 without it (an `exit` alone, or the input's end: the
  LSP's rule); `phitex.stop` ends it with 0. A native server still
  waiting for the first `.tex` document to name the main file ends at
  once, with the same statuses.
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

Every build sends its pages as they ship, before the build settles: a
clean build every page, an edit the pages it changes, each as it is
made (a page made again as it was sent last is not sent again; the
pages an edit leaves alone are never sent):

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
  before it are known, and its
  file begins from the colours they leave it (a colour pushed on one
  page and popped on a later one is carried into the pages between, as
  the build's file carries it). `false` when the page draws with
  something not complete yet (a `\pdfobj reserveobjnum` filled later,
  objects written at `\AtEndDocument`, such as pgf's shared ExtGState,
  Pattern and ColorSpace dictionaries named through
  `\pdfpageresources`), or the colours carried into it are not known
  yet (pages shipped out of order). The page is still drawn, without what it lacks:
  - a missing graphics state is drawn opaque;
  - a missing shading, or a fill with a missing pattern, draws nothing
    (a flat fill would hide what lies under a shaded background);
  - a missing colour space falls back to gray;
  - a missing external object draws nothing.

  A viewer marks such a page quietly, "Preview: final colours and
  shading when the build ends", until the settled page replaces it.
- A XeTeX build streams its pages too: each page's file is
  xdvipdfmx's of that page alone. It is exact where no colour is carried
  into it and it draws with nothing another page makes (no `pdf:` special
  but annotations, links, destinations, outlines and the document's
  information, and literal code naming no resource).
- A page whose file cannot be written with the objects it names is
  written without them (its text, rules and colours), `exact: false`.
- A page sent with `exact: false` only because the pages before it were
  not known yet (pages ship out of order at more than one thread) is
  sent again with the same `hash` and `exact: true` once they are, where
  they carry no colour into it.
  Treat it as an update of that page's mark only.
- The `status` and `pages` answers list the pages streamed so far
  (`hashes`/`pages`) and, as `"inexact": [k, …]`, those not exact. The
  list is empty once the build is in.

### 2.1d Glyph origins: made per page, when asked

A build makes no glyph origins. The `origins` op (`{"op":"origins","page":K}`)
answers page K's from those made for the current build; a page not made yet
answers `{"pending": true}` and is made then, for that page alone, by the
session's owner (watch loop, or the core's next step): the viewer asks
again shortly, as for `log`. A page's answer is kept while its nodes are,
and shifted, not made again, where an edit only moved them. After every
build the viewer gets `{"event":"origins","pages":[],"edits":[]}`: the
answers it keeps by page are asked again when next needed (a page the build
did not draw again may have moved in its file). `Viewer::set_origins` still
sets every page's at once; nothing calls it after a build any more.
`JOB.synctex.gz` (`--synctex`, or a watch writing its PDF) is written at
idle or on quit, from the same per-page answers.

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
 "root": "/home/me/paper", "main": "paper.tex",
 "update": null | {"latest": "1.1" | null, "advisory": false,
                   "lines": ["phitex 1.1 is out (you have 1.0):", "  1.1     Faster.", "update: phitex upgrade"]}}
```

- `update`: what the update check says to this version, natively, when
  the user turned the check on (`update-check = true`; never under CI or
  `PHITEX_NO_UPDATE_CHECK`): the releases after it, the advisories for it
  (`advisory: true` when there is one) and how to update, the lines the
  terminal prints (docs/spec-distribution.md §4). `null`: the check is
  off or unset, or there is nothing to say. Read from the kept
  `news.json` as the watch starts and at each idle point; fetched at most
  once a day in the background. The wasm core sends `null`.

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
| `clean` | `phitex clean` | `removed` (files), `cache_bytes` (freed); `clean --all`: `cache_bytes` only; `--dry-run`: `dry_run` true, `files` and `cache_bytes` what would go, nothing removed (a bare `phitex clean` tells `cache` instead) |

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

- **VS Code**: the extension *PhiTeX ⚡ Instant LaTeX Preview* (`accl-kaust.phitex`,
  `apps/vscode/`; on the VS Code Marketplace and Open VSX).
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
- `phitex session list` lists the running watches; `phitex session list --json`
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

## 9. Reviewing with an agent: `phitex session`, notes, changes

Status: designed 2026-11-14 (built in three parts: the session's CLI,
notes, changes). The user reads the pages in a watch's viewer, marks
text and leaves notes; their own agent (any program in a terminal)
reads the notes through `phitex session`, edits the `.tex` files on
disk, and answers; the user reviews the agent's edits in the viewer as
hunks and accepts or rejects each, commenting on them as on a pull
request. PhiTeX never calls an AI and sends nothing anywhere: "Ask AI"
stays copy-only. Notes never touch the PDF: it stays byte-identical to
pdfTeX's.

### 9.1 `phitex session`: one command for a running watch

```
phitex session start [FILE] [-- WATCH-ARGS…]  # a watch in the background, returns once its first build is in
phitex session wait [--warnings] [--pdf] [--notes] [--timeout 60s]
phitex session status
phitex session problems [--errors] [--warnings] [--file F] [--context N]
phitex session rebuild [--clean]
phitex session log [--tail N] [--build N]
phitex session stop                            # as q: the exact PDF written, then the watch ends
phitex session list                            # the running watches
phitex session notes [--open] [--all] | reply ID TEXT | resolve ID [TEXT] | changes
```

- **Which watch.** Every verb finds its watch from the current
  directory as an editor does (§8): the running watches whose `root`
  holds it, preferring one whose `main` is `FILE` (or the directory's
  `phitex.toml` main), else the newest. `--session PID` names one.
  `start` reuses a running watch of the same main file.
- **`start`** runs `phitex watch --write FILE` detached (its own process
  group, standard input closed, its output in `.phitex/session.log`),
  waits for its discovery file (the sessions folder watched by the
  system, no polling) and its first build (`wait`), then prints `pid`,
  the viewer's URL and the log's path. `--json` prints them as one
  object.
- **Exit statuses** (stable): 0 done; 1 the build has TeX errors (`wait`,
  `status`, `problems`, `rebuild`); 2 timed out (`wait`, `start`); 3 no
  running watch (or it ended while waited on); 4 refused (a bad note id,
  a usage error).
- **Output.** One line per problem, `file:line: message`, and with
  `--context N` the source lines around it, `>` marking the line.
  `--json`: one JSON object, the method's result. Times are wall times.
- **Transport.** The watch's attach socket (§8): the CLI connects with
  the token from the discovery file, sends `initialize` with
  `initializationOptions.session: true` (no buffers, no cursor: never an
  editor), then one request, ignoring the notifications every client
  gets. The methods below are requests on that socket, so any editor
  attached can use them too. Errors: `-32602` bad arguments, `-32603`
  failed, `-32601` unknown; `-32800` with `data: {status: N}` where the
  CLI exits N.

| method | params | result |
|---|---|---|
| `phitex/session/status` | none | `Status` |
| `phitex/session/wait` | `{warnings?, pdf?, notes?, timeout_ms?}` | `Status`, with `timed_out: true` past the timeout; with `notes`, `{notes: [Note], timed_out?}` |
| `phitex/session/problems` | `{errors?, warnings?, file?}` | `{build, problems: [Problem]}` |
| `phitex/session/rebuild` | `{clean?}` | `Status` of the build it made |
| `phitex/session/log` | `{build?}` | `{build, text}` (the transcript of the last build: older ones are not kept, `build` other than the last fails) |
| `phitex/session/stop` | none | `null`, then the watch ends as `q` ends it |
| `phitex/notes/list` | `{open?, all?}` | `{notes: [Note]}`; marks them read (§9.3) |
| `phitex/notes/reply` | `{id, text}` | `Note` |
| `phitex/notes/resolve` | `{id, text?}` | `Note` |
| `phitex/changes/list` | none | `Changes` (§9.4) |

`Status = {build, ok, errors, warnings, pages, ms, pdf, viewer, main,
root, pid, problems: [Problem], diagnostics_pending, written}`:
`problems` the errors (warnings with `--warnings`), each `{severity,
file, line, message, excerpt?}` as the viewer's diagnostics have them.

Left for later, the names kept free: `phitex/session/where` (a page's
place to source), `…/source` (a source place to pages), `…/page` (a
page as an image), `…/events` (the `--json` stream over the socket),
and `phitex session mcp` (the same methods as an MCP server on standard
input and output).

### 9.2 `wait`: exact

"Done" means a finished build read the project's files exactly as they
are when `wait` is called. Never a build from before the agent's save:

- **What a build read.** The engine is given every file through the
  session's finder; the CLI's host is told each file it gives
  (`Host::fed`: path and bytes, a buffer's or the disk's), and keeps a
  64-bit hash of each under the watch's root. When a build ends, its
  record is those hashes (the files kept from earlier builds keep the
  hash they were read with) and the main text's (the text the build
  built, `Session::source`), for the files the build reads
  (`Session::watched`), with the names it looked for and did not find
  as absent.
- **The call.** `wait` hashes each of those files as it is now (the
  disk's; a file an attached editor holds open: its buffer, which is
  what a build reads), and answers with the newest build whose record
  equals that, at once if the last one does. Else it waits for the
  build that does. A file an editor holds whose buffer differs from the
  disk is named in the answer (`held: [file]`): the agent's write to it
  is not built until the editor takes it.
- **Then.** `--warnings` waits further for that build's diagnostics
  (made when the editing pauses; at once if they are); `--pdf` for the
  exact PDF written at the idle point after it (`written`; a watch that
  writes no PDF refuses `--pdf`, exit 4). A later build that read the
  same files counts as that build.
- **`--notes`** waits for the user instead: a note added, or a reply,
  review or verdict made in the viewer after the call (§9.3); it prints
  them. The timeout (default 60 s; `--timeout 10m`, `0` for none) ends
  any wait with status 2.

### 9.3 Notes

**In the viewer.** Text selected on a page offers **Highlight** (a
mark, no words), **Note** (a remark kept with the text, for the user and
the agent) and **Comment** (a request to the agent: the agent's to-do
list). A click on the page with nothing selected (Alt+click, or the
toolbar's note tool) notes a spot: a figure, a table. A note has a
thread (replies, the agent's and the user's), can be resolved and opened
again; the Notes panel lists them with an Open/Resolved/All filter and
"Clear resolved". A note's highlight is drawn over its glyphs on every
page they are on (across a page break too), never into the PDF.

**Anchors are in the source**, not on the page: `{file, start, end,
quote, before, after, line}`, bytes of the file as the glyph origins
give them (§2.1d): the selected glyphs' origins (the file most of them
came from, from the first one's start to the last one's end), the quoted
source text, and up to 64 bytes of context each side. A spot without
glyphs anchors at the nearest glyph's place, its quote empty, with the
page and point kept for drawing it. The viewer draws a note from its
range through the pages' origins (`boxesOf`, as a problem's underline).

**Following edits.** The watch moves every anchor when a file changes:

- an attached editor's ranged change moves it exactly (a change before
  it shifts it, one inside it grows or shrinks it);
- a change on disk (or a whole buffer) maps the old text to the new by a
  diff of their words (patience over tokens, the unchanged prefix and
  suffix first): a range keeps the tokens of it that survive;
- a range none of whose tokens survive is looked for by its quote and
  context (moved text: the nearest exact match, context breaking ties);
  found nowhere, the note is **detached**: kept with its last place and
  quote, listed under "Detached" and never dropped silently.

**Storage.** `.phitex/notes.json` in the project, plain JSON, written
whole (beside, then renamed) by the running watch only; the CLI reads it
directly when no watch runs (`session notes` works on a stopped
project; `reply` and `resolve` need the watch, status 3).

```
{"version": 1, "next": 12,
 "notes": [Note], "reviews": [Review], "verdicts": [Verdict]}
Note = {id: "n3", kind: "highlight" | "note" | "comment",
        anchor: {file, start, end, quote, before, after, line, page?, point?},
        detached: bool, status: "open" | "resolved",
        created, updated,              // ISO 8601 UTC
        thread: [{by: "you" | "agent", text, at,
                  edited?: [{file, start, end, from_line, to_line}],
                  diff?: {old, new}}],   // a hunk's diff after the agent changed it ("updated")
        hunk?: {file, old, new, line},   // a comment on a change (§9.4)
        draft?: true,                    // part of a review not submitted yet: the agent does not see it
        review?: "r2"}
```

**The agent's side.** `session notes` prints each note's id, kind,
`file:line`, its quote with the context dimmed, and its thread; with
`--open`, only open ones (the default: open ones; `--all` every one).
Listing marks the notes read: the watch keeps the project's text as it
was then. `reply ID TEXT` adds to the thread; `resolve ID [TEXT]`
resolves it and records the source ranges the agent changed since it
read the note (a diff of the text then and now, by file and line), so
the thread says "edited: paper.tex:118–121", a link to the place.
Those ranges move with later edits as anchors do.

### 9.4 Changes: the agent's edits, reviewed as hunks

**Baseline.** In a git repository, git's index (`git diff`'s left side):
accepting a hunk stages it (the index's text with that hunk applied
written as a blob and staged, as `git apply --cached` would); rejecting
one writes the index's text of it back to the file. Without git, the
watch keeps `.phitex/baseline/` (each project file's text as first seen,
then as accepted), written only once a change of that file waits: a
watch with nothing to review writes nothing into the project, and
`.phitex/notes.json` likewise appears with the first note, review or
verdict.

**Whose edit.**

- An attached editor's changes (over the language server) are the
  user's and are accepted as they are typed: a change outside every
  pending hunk is applied to the baseline at its place (in git, the
  index is written when the editing pauses); one inside a pending hunk
  becomes part of that hunk. A buffer change that only makes the buffer
  the disk's text (the editor taking the agent's write) is not the
  user's.
- A change that appears on disk otherwise is pending: the agent's, or
  the user's own from another editor. The Changes panel accepts them in
  one click (Accept all, or Accept file), and `review-accept-disk =
  true` in `phitex.toml` accepts disk changes as they come.

**Hunks.** The current text (an editor's buffer, else the disk) against
the baseline, by words: the two texts' tokens (words, spaces, single
other characters) diffed (patience), the changed runs closer than three
tokens on one paragraph joined into one hunk. A hunk is
`{id, file, line, base: [start, end], now: [start, end], old, new,
note?, comments}`: byte ranges in the baseline and the current text;
`id` a hash of file, base start, old and new (a hunk changed is a new
one). Renamed or deleted files: a file the baseline has and the disk
lacks is one hunk deleting it whole (Reject writes it back); a new file
the build reads is one hunk adding it whole.

**Grouped by note.** A hunk whose current range meets a range a resolve
(or reply) recorded as edited is listed under that note.

**The Changes panel** (beside the Notes panel): the hunks by note, then
by file, each its source diff (removed words struck in red, added ones
in green, a line of context) with Accept and Reject; Accept all and
Reject all; a hunk's place on the pages marked (its new text's glyphs,
else where its old text was), a click on either selecting the other.
"Show changes" turns on Compare (the latexdiff path, its pages
streamed) against the baseline (`staged` in git, else
`.phitex/baseline/`), so deletions and insertions show typeset.

**Reject** writes the baseline's text of that hunk into the current
text and the file, and rebuilds. A file an attached editor holds is
changed through it (`workspace/applyEdit`) when its buffer is the disk's
text; with unsaved changes there, Reject refuses and says so (save or
undo them first). Reject and Accept name the hunk by `id` and refuse a
hunk that changed since it was shown ("this change was edited again:
look at it again").

**Comments on changes.** A comment on a pending hunk (the whole hunk,
or a line or range in its diff or on its marks on the page) is a Note of
kind `comment` with `hunk` set, anchored to the hunk's new text (its
removed text's place, for a deletion), so it follows the hunk as notes
follow text, and is kept with the hunk's history after Accept or Reject.
Comments thread, are replied to and resolved as notes are. When the
agent's next edit changes a commented hunk, its thread gets the new diff
(`diff`, "updated", as a pull request's thread after a push).

**Reviews.** Comments may be posted at once, or gathered as a review
(drafts) and submitted together with a summary and a verdict on the set
of hunks: **Approve** (each hunk in it accepted), **Request changes**
(nothing applied; the agent is asked to revise), **Reject** (each
rejected) or **Comment** (no verdict). A review wakes `wait --notes`.

**The agent's view.** `session changes` prints what is pending (each
hunk's `file:line`, its old and new words, its note), the user's
verdicts since the agent last looked (rejected hunks with the text that
was rejected, so the agent does not write it again; accepted ones), the
open comments on changes with their diffs, and the reviews.

```
Changes = {pending: [Hunk], verdicts: [Verdict], reviews: [Review], comments: [Note]}
Verdict = {hunk: {file, line, old, new}, verdict: "accepted" | "rejected", at, review?}
Review = {id, verdict: "approve" | "request_changes" | "reject" | "comment", summary, at, notes: [id], hunks: [id]}
```

### 9.5 The viewer's ops (the watch's; another host may serve them)

| op | params | answer |
|---|---|---|
| `review_state` | none | `{notes, hunks, reviews, git, baseline}` |
| `review_note` | `{kind, anchor, text?, seen?}` (anchor without `quote`: the watch fills it from the file; `seen`, the words the viewer showed, is TEMPORARY: while an `\input` file's glyph origins name the `\input` command, the watch finds the note's place by these words in its file; it goes when the origins name the glyph's own file) | `Note` |
| `review_reply`, `review_resolve`, `review_reopen`, `review_delete` | `{id, text?}` | `Note` |
| `review_clear` | none | `{removed}` (the resolved notes) |
| `review_accept`, `review_reject` | `{ids: [hunk id] \| "all", file?}` | `{done, refused?: why}` |
| `review_comment` | `{hunk, text, range?, draft?}` | `Note` |
| `review_submit` | `{verdict, summary}` | `Review` |

The event `{"event": "review", "notes": [...], "hunks": [...],
"reviews": [...]}` is sent to every viewer whenever any of it changes
(a note made or moved, a reply, a build, an accept): the whole state,
small next to a page.
