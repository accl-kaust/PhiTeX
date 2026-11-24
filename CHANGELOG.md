# Changelog

PhiTeX's versions converge to φ, a digit a milestone: 1.0 is the first
release (the alpha's milestones, the user's decision of 2026-10-31), then
1.6, 1.61, 1.618; a milestone's updates are 1.0.1, 1.0.2, … (`AGENTS.md`). A release's entry is a `## <version> (<date>)` heading
whose first paragraph is its summary: `phitex` says it once on the first
run of that version, `phitex news` prints the whole entry, and the
GitHub release's notes are the entry. The entries before 1.0 name
milestones, not versions.

## 1.0.5 (unreleased)

Compare and Blame are now one thing: **± Changes** :)

- **One button, one popover.** Click ± Changes to open it; click it
  again, ✕, Esc or anywhere outside it to close it. Nothing is read until
  you pick a version or switch to Blame. Diff and Blame sit side by side
  in it, each with its look at the top of its list (and in Settings), kept
  for next time.
- **Read the diff with nothing in the way.** Close the popover and the
  diff stays on the pages, the ± Changes button lit, until you stop
  comparing; open it again and you are right where you were. Hover a
  deletion's caret to read the words that were deleted, and right-click
  any change to find it in the list or jump to its source.
- **Diff from anywhere to anywhere.** Pick From and To in a commit graph
  coloured by branch: click one end and then the other, drag a range,
  double-click a commit to see just its changes, or start from your last
  visit, the last tag or before a merge. Changes are listed by section with
  filters, and `]` / `[` step through them. Choose the look: colour-blind
  safe or print colours, wavy, underlined or highlighted additions, struck
  deletions or just a caret, changed figures side by side, overlaid or new
  only.
- **Blame down to the word.** Every line is coloured while Blame is on:
  a soft highlight behind the words (the default), the words themselves, or a
  bar in the margin, by author or by age. Hover a line for its card, and
  everything its author (or its commit) wrote lights up; click a person or
  a commit in the list to keep it that way. It works from the sources and
  the history, so it is ready before the first page is, and new pages get
  their colours as they arrive. Words are counted as a reader counts them:
  data tables, drawings and commands don't add to anyone's share. Blame by
  line, word or character, skip a reformatting commit right from its row,
  keep moved text with its writer, dim everything older than a tag. A
  merged commit's card says which branch it came from, and "What this
  commit changed" opens its diff.
- **Overleaf, exact.** On Overleaf every character is credited to the
  person and edit session that typed it, read straight from the project's
  history, with no cap on its length. The history fills in as it loads,
  with progress you can see, and reopening a project is instant.
- **Stop comparing, in one click.** While a diff is shown, ✕ Stop comparing
  sits right under From → To and puts the pages back to the current
  version; the diff's PDF and diff.tex download from beside it.
- **Your own page colours.** Settings, Page colours: Sepia, Dark, High
  contrast or your own page and text colours, and the marker's colour too.
  Only the screen changes: downloads stay the real PDF, and Dark PDF still
  wins while it is on.
- **A whole folder, one watch.** `phitex watch .` opens a little file
  manager of the folder in your browser: click any .tex file and it builds
  in a tab of its own, switch documents from the Files panel or with
  Ctrl+P, and keep two side by side.
- **Warnings where they are.** `phitex build` lists each warning as
  `file:line: message`, the first 20 (`-v` all), and `--json` gives them
  as a `problems` list. A missing citation says which key is not in which
  .bib file, and a warning about a float names the file the float is in.
- **A page as a picture.** `phitex png paper.tex --page 3` draws a page as
  a PNG, and `--label fig:results` crops just that figure, table or
  equation: a quick look without opening the PDF.
- **Ctrl+R in the terminal** starts a clean rebuild from scratch, as
  Clean rebuild does in the viewer (`h` lists it with the other keys).
- **Fixed:** on Overleaf, a jump between the PDF and the source now opens
  the file and scrolls to a line that was off screen, in the separate PDF
  tab too; text selected on the pages is plain blue.
- **Fixed:** comparing text that ends a change with a forced space
  (`e.g.\ `) builds the diff's pages.
- **Fixed:** SyncTeX and the viewer's follow take you to the right copy
  of text that repeats: clicking equation (2) no longer lands on (1).
- **Fixed:** `files = texlive` finds TeX Live installed with Nix
  (home-manager's `texliveFull`, `texlive.withPackages`).

## 1.0.4 (2026-11-17)

Compare got a lot smarter, and Blame is here :)

- **Compare reads like your paper.** Comparing with a past version
  (latexdiff) now shows tables row by row, figures and tables that moved
  or changed as whole blocks, and a changed picture old and new side by
  side. Citations are compared one key at a time, so adding a reference to
  `\cite{a,b}` marks just the new one, and long marked lines break
  properly instead of running off the page. Small edits that change
  nothing you can see stay unmarked.
- **Blame, beside Compare.** Margin bars show who changed each line and
  when, coloured by age, by author, or since a version you pick. Hover a
  bar to see the change and jump to it.
- **Fixed:** Overleaf projects open in Firefox again, with Compare and
  replaced figures.

Thanks for the quick reports, they made this release!

## 1.0.3 (2026-11-17)

Opening a paper again is now nearly instant, everywhere PhiTeX runs :)

- **Reopening is fast.** PhiTeX remembers what it built, so the next
  time you open a document it starts from there instead of from scratch:
  often well under a second, where it used to take several. This works in
  `phitex build` and `phitex watch`, and in the built-in engine of VS
  Code, the web demo and Overleaf. Don't want it? `phitex config graph
  off`.
- **Compare starts from your build.** Comparing with a past version
  (latexdiff) no longer typesets everything from scratch or loads the
  packages again; it starts from the document you already have.
- **VS Code follows your files on disk.** Replace a figure or edit your
  `.bib` outside the editor and the preview catches up on its own.
- **Citation cards link the DOI.** Click it to open the paper, even when
  the line broke in the middle of it.
- **The contents panel shows printed page numbers,** like iv in a preface
  or A-3 in an appendix.
- **Double-click and selection land on the right text** in floats on
  later pages.
- **Overleaf: your recent projects come first** in the folder view.
- **Fixed:** with several threads (`-j8`), an edit to the `.bib` could
  now and then put the bibliography's page in the wrong place, and a few
  rare races could misplace a float or leave a build hanging; a page an
  edit took away could stay on screen; Compare in Firefox; a figure saved
  over that kept showing its old picture; and smaller things in the
  preview.

Thanks for writing in, it all helps!

## 1.0.2 (2026-11-15)

A big one :) Lots of speed, and a nicer preview to read your paper in!

- **Edits feel instant.** Big plain TeX files edit in a few ms instead
  of fifty, and pages with figures aren't redrawn from scratch.
- **TikZ-heavy papers build about a third faster** from clean. Each
  picture is drawn just once now.
- **Pages stream in as they're typeset,** after every edit too, not only
  on the first build.
- **Colour lines!** A reading aid that shades lines in alternating
  colours, so your eyes don't lose their place. It has its own Settings
  tab.
- **Zoom into link previews** with Ctrl+scroll, a pinch, or the buttons.
- **Quitting a watch asks nicely.** You can cancel the question now, or
  switch it off with `phitex config watch-ask-write off`.
- **`phitex upgrade` has a progress bar.** Small thing, very satisfying.
- **Ironed out bugs.**

Thanks for all the feedback, keep it coming!

## 1.0.1 (2026-11-12)

Fixes for the live preview, Compare that streams its pages, and
multi-threaded builds that come out the same every time.

- The warnings filters no longer typeset the document again, and each
  document keeps its own (they were one setting for every document).
- The loading animation you picked is the one that plays, Bad Apple
  included, from its first frame.
- Compare shows the marked-up pages as they are typeset, with its
  progress; a Compare after an edit is an edit too (tens of milliseconds,
  not a clean build).
- Builds with several threads (`-j`) give the same PDF every run: a few
  manuals could lose floats or differ in their links now and then.
- Edits with several threads typeset each changed paragraph once.
- A release missing its signing password says so.

## 1.0 (2026-11-09)

The first release of PhiTeX. Enjoy!

