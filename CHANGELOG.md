# Changelog

PhiTeX's versions converge to φ, a digit a milestone: 1.0 is the first
release (the alpha's milestones, the user's decision of 2026-10-31), then
1.6, 1.61, 1.618; a milestone's updates are 1.0.1, 1.0.2, … (`AGENTS.md`). A release's entry is a `## <version> (<date>)` heading
whose first paragraph is its summary: `phitex` says it once on the first
run of that version, `phitex news` prints the whole entry, and the
GitHub release's notes are the entry. The entries before 1.0 name
milestones, not versions.

## 1.0.2 (2026-11-14)

An agent you can work with from the terminal, faster edits and clean
builds, and a calmer, sharper live preview.

- `phitex session` lets an agent such as Claude work with a running
  watch: wait for a build, read its problems, and write the PDF
  (`phitex session pdf`; `--synctex` adds the .synctex.gz).
- Notes and review, an experimental feature (off until you turn it on
  in Settings, Experimental): select text on the pages to highlight it
  or leave a note for your agent, and review its edits as a git-style
  diff, with Accept all, proposals and Show diff.
- Colour lines, a reading aid: lines shaded in alternating colours, in
  its own Settings tab.
- Edits are faster: a rebuild that changes nothing takes a couple of milliseconds, a
  word in a large plain TeX file takes milliseconds, and a page with
  pictures or images is no longer drawn again from scratch.
- Clean builds run each TikZ picture once; documents with many pictures
  build in a third less time.
- Every page streams to the preview as it is typeset, edits included.
- Two-column papers: a selection stays in its column, and Colour lines
  keep the columns apart.
- A link's preview window zooms (Ctrl+wheel, a pinch, its buttons) and
  pans.
- Quitting a watch leaves the exact PDF on disk however it ends (q,
  Ctrl-C, a signal); its question can be cancelled, and
  `watch-ask-write` turns it off.
- `phitex upgrade` shows its download's progress.
- Fonts are found as xelatex finds them.

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

