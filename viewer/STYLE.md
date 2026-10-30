# The viewer's style

The panel looks native to the host it is in: in Overleaf it is part of
Overleaf's PDF pane, in VS Code part of VS Code, and in `phitex watch`,
which has no host, it has a look of its own that sits well beside an
editor. The sheet is `src/print.ts`, the only place colours are chosen.

## Tokens

Every rule reads `--ph-*` tokens; each host maps them once, and every
token has a fallback (a variable a host renames leaves a look, not a hole).

| Token | Overleaf | VS Code | The watch |
|---|---|---|---|
| `--ph-bar` | `--toolbar-alt-bg-color` | `--vscode-editorGroupHeader-tabsBackground` | white / `#161b23` |
| `--ph-stage` | the pane's background, `--pdf-bg` | `--vscode-editor-background` | `#eceef2` / `#0f1319` |
| `--ph-fg`, `--ph-fg2` | `--content-primary(-dark)`, `--content-secondary` | `--vscode-foreground`, `--vscode-descriptionForeground` | ink, slate |
| `--ph-border`, `--ph-hover`, `--ph-menu` | dividers, its toolbar's hover, its menus | `--vscode-panel-border`, `--vscode-toolbar-hoverBackground`, `--vscode-menu-background` | |
| `--ph-accent` | its green (`--bg-accent-01`) | `--vscode-button-background` | the logo's navy `#1d3e72` |
| `--ph-progress` | its green | `--vscode-progressBar-background` | the logo's orange `#f58f2d` |
| `--ph-danger`, `--ph-warning`, `--ph-info`, `--ph-link` | its content colours | VS Code's | |
| `--ph-font`, `--ph-radius`, `--ph-pill` | Noto Sans, its radii | `--vscode-font-family`, 2 px | Noto Sans, 6 px, round |

Overleaf's light theme is `.win.light` (dock() reads `body[data-theme]`);
VS Code's variables change with its theme; the watch follows the system.

## The bar

One bar (VS Code, the watch; in Overleaf the extension puts its controls
in Overleaf's own toolbar): **Rebuild** and its menu (Clean rebuild,
Compare with an earlier version, Clear cache), the build's status (its
times a click away), then find, compare, download, the page, zoom, the
problems' count (only when there are some) and a menu for the rest (the
log, the contents, dark PDF, the debug check, what's new, the keys,
about). Narrow (560 px): Rebuild, the page and the menu.

Builds run by themselves as the writer types; the words never suggest
otherwise ("Rebuild", not "Recompile").

## Loading

Before the first page, the first page's sheet (A4, whole in the view)
plays the opening (opening.ts: the logo, assets/logo-concepts.html's
"Opening a document"), and under it the step: what, a thin bar (its
length when the host knows it), the detail (the file, the package, `n of
m`, as the host counts them; never invented). When the first page comes,
the zoom stays whole-page and only the sheet's width eases to the page's
shape; then the logo dives into the page. A page there at once (a project
built before) is shown at once. A package fetched later, a build that
runs on: a small note in the corner, never over the page.

## Shapes and words

- The host's font and size; sentence case; numbers in tabular figures;
  monospace only for source lines, logs and keys.
- The host's radii; menus and popovers are the host's menu colour with a
  soft shadow; the pages sit on the stage with a faint shadow.
- Problems are Overleaf's log entries: a coloured left edge (error,
  warning, info), the message, its place as a link, the source line.
- Feature tips point at a control with an arrow, once each; "What's new"
  shows them again.
- Dark PDF is the user's choice, Off by default (the filter inverts
  figures too): a grey page (`#2c2e33`) and soft ink, never black.
- Terse, plain words: "Built · 42 ms", "Fetching amsmath.sty · 3 of 9".

## Motion

The progress line, the opening, the sheet's ease, a note sliding in.
`prefers-reduced-motion` stops all of it (the steps are still said).

## Not here

Small caps, crop marks, badges, step trackers, glows, gradients, emoji in
the chrome, a second sheet overriding the first.
