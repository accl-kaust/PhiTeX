# Changelog

PhiTeX's versions converge to φ, a digit a milestone: 1.0 is the first
release (the alpha's milestones, the user's decision of 2026-10-31), then
1.6, 1.61, 1.618; a milestone's updates are 1.0.1, 1.0.2, … (`AGENTS.md`). A release's entry is a `## <version> (<date>)` heading
whose first paragraph is its summary: `phitex` says it once on the first
run of that version, `phitex news` prints the whole entry, and the
GitHub release's notes are the entry. The entries before 1.0 name
milestones, not versions.

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

