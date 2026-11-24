# Watching a folder: `phitex watch DIR`

`phitex watch .` (or any folder) watches a whole folder from one viewer
address. `phitex watch paper.tex` works exactly as before.

## The file manager

The address the watch prints opens a file manager of the folder. It
shows one folder at a time, starting at the folder you gave:

- every folder and file in it, folders first, by name or by date
  modified (click the column's head; in a narrow window, the A–Z button);
- a breadcrumb; Backspace, Alt+↑ or the ↑ button go up;
- one rule everywhere, as on the web: every folder and document is a
  link. A click opens it here; the middle button, Ctrl+click (⌘ on a
  Mac) or the browser's own "Open in new tab" open it in a new tab, and
  "Copy link" copies its address. The browser's Back goes back;
- the keyboard: ↑ ↓ Home End PgUp PgDn move, Enter opens (Ctrl+Enter in
  a new tab), `/` (or typing a letter) filters the folder, Esc clears the
  filter;
- for each document opened in this run, its build: Not built, Building,
  Waiting (for another document's build), Built with its pages, or its
  errors.

Nothing is read to decide what is listed. Only `.tex` files open; other
files are shown, muted, and a PDF or a picture (PNG, JPEG, GIF, WebP)
opens as it is. Hidden files and folders (`.git`,
`.latexmkrc`, …) and PhiTeX's cache are left out. `.gitignore` is not
read. The listing follows the folder as files are added, removed and
renamed.

It is a lazy file manager: the watch starts at once whatever the folder's
size (your home folder too). Nothing is read at the start but the
folder's own listing; a folder is read when a page opens it, and watched
(alone, not what is below it) only while some page shows it. Nothing is
walked below a folder except by quick open (Ctrl+P), for that search
alone. A document's address carries its path (`d/ID/?path=…`), so a tab
left open from an earlier watch finds its document without a search.

It never goes above the folder you gave. Every path the page names is
checked: no `..`, no absolute path, no hidden name, and a symbolic link
is followed only when it stays inside the folder (a link that leads out
is not listed and is refused). The tests in
`crates/cli/tests/dirwatch.rs` try each way out.

## Opening a document

Click a `.tex` file and it opens in this tab; open it in a new tab (the
middle button, Ctrl+click) to keep the file manager beside it. A file whose first lines say `% !TEX root =
main.tex` opens that root instead, and the page says so. A file that is
not a document opens all the same and shows its errors, as any build
does.

The viewer of an open document has a **Files** column (the folder
button at the bar's left, or `b`): the same file manager, the same
links. A click on a document there switches this tab to it; Ctrl+click
or the middle button open it in a new tab, so two documents sit side by
side.
**Ctrl+P** (⌘P on a Mac) finds any document of the folder by name;
Enter opens it here, Ctrl+Enter in a new tab. In a document's tab,
Ctrl+P pressed again in the finder prints the PDF.

## Sessions

Each document opened is a `phitex watch FILE` of its own, started when
it is first opened: nothing is built before. It runs where that command
would be run on it: the nearest folder, from the file's own up to the
watched one, that holds a `phitex.toml`, `.latexmkrc` or `latexmkrc`
(else the watched folder), so those settings apply as they do there.

Its builds run one at a time, each with all the threads the watch was
given (`-j N`, the `threads` setting, else the default): the cores are
never oversubscribed. A save in one document while another builds
waits for that build, and its viewer and the file manager say so.

## Running documents: stopping them

Above the files, the file manager shows what is running: a **Running**
strip with a row for each document whose session runs, wherever it is
in the folder. Each row has the document's path (a link to it), its
state (Building, Waiting, Built · N pages, N errors, or No viewer ·
stops in N min), how many tabs show it, and **Stop**; **Stop all** stops
every one. The strip follows the sessions live and is hidden while
nothing runs. In a document's Files column, each running document has a
small dot in the same colours.

A stopped document is torn down: its session ends and its memory is
given back, and a tab still showing it waits. Open it again (a click,
or reload its tab) and it starts as any document does, from its saved
graph where it has one.

A document's last tab closed, it stops building (what changes meanwhile
is kept) and keeps running for `--auto-stop DURATION`, 5 minutes by
default (`--auto-stop 30s`, `--auto-stop 1h`; `--auto-stop -1`: never),
then stops by itself. A tab opened again before then cancels it and
builds what changed, as an edit. `--idle MS` keeps its own meaning: the
time without an edit before the exact PDF file is written.

Besides, the documents running together may use the memory `--memory
SIZE` allows (default: a quarter of the machine's memory; measured on
Linux). Past it, the least recently viewed of those no tab shows stop
first.

## The terminal

The terminal prints the folder's address, then a tree: the folder's
line, and a line for each document opened in this run with its state,
the same words as the Running strip (and `stopped`), and its tabs. It is
redrawn in place as states change. ↑ and ↓ move a highlight through
the tree; the keys act on the highlighted document, or on every running
document when the folder's line is highlighted:

| Key | What it does |
| --- | --- |
| ↑ ↓ | highlight a document (the top line: every open document) |
| Enter | open the highlighted document in the browser |
| `r` | build again now |
| Ctrl-R | clean rebuild: start over from scratch |
| `x` | stop it: its session ends, its memory is freed |
| `p`, `w`, `s` | errors, warnings, how the build went (as in `phitex watch FILE`) |
| `o` | open the folder in the browser |
| `c`, `h` | clear the screen, list the keys |
| `q`, Ctrl-C | quit, every session with it; the terminal put back as it was |

On a pipe (no terminal), nothing is drawn in place and no key is read
raw: a line is a key, optionally followed by a document's path, as
`r paper/main.tex` (rebuild it), `x paper/main.tex` (stop it), `quit`.

With `--json`, each document's events come on standard output with a
`"doc"` field naming it (its path from the folder), beside the folder's
own: `viewer` (the address), `watching` (how many folders are watched
now), `session` (connected or not; `"auto_stop"`: seconds until it stops
by itself, null when a tab came back; `"ended"`: `stopped` by hand,
`idle`, or `memory`; `"running": false` once its session is gone),
`queued` (a build waiting for another's) and `turn` (a build holding the
one turn, then letting it go).

`phitex watch --lsp` takes one file, as before. VS Code is not covered.
