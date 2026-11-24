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

## Closing, memory

There are no timers. A document's last tab closed, it stops building
and stays warm: opened again, it builds what changed meanwhile, as an
edit. The documents kept warm may use the memory `--memory SIZE` allows
together (default: a quarter of the machine's memory; measured on
Linux). Past it, the least recently viewed of those no tab shows end and
give their memory back. Opened again, an ended document starts from its
saved graph, or with a clean build.

## The terminal

The terminal prints the folder's address, then one status line for each
open document. The keys act on every open document, or on the one
chosen with `1`–`9` (`a` or `0`: all again):

| Key | What it does |
| --- | --- |
| `1`–`9`, `a` | choose a document, or all |
| `r`, Enter | build again now |
| Ctrl-R | clean rebuild: start over from scratch |
| `p`, `w`, `s` | errors, warnings, how the build went (as in `phitex watch FILE`) |
| `o` | open the folder (or the chosen document) in the browser |
| `c`, `h` | clear the screen, list the keys |
| `q`, Ctrl-C | quit, every session with it |

On a pipe, a line is a key, optionally followed by a document's path:
`r paper/main.tex` rebuilds that document alone.

With `--json`, each document's events come on standard output with a
`"doc"` field naming it (its path from the folder), beside the folder's
own: `viewer` (the address), `session` (connected, disconnected, ended,
`"ended": "memory"`), `queued` (a build waiting for another's) and `turn` (a build holding the one turn, then letting it go).

`phitex watch --lsp` takes one file, as before. VS Code is not covered.
