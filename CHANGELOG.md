# Changelog

PhiTeX's versions converge to φ, a digit a milestone: 1.0 is the first
release (the alpha's milestones, the user's decision of 2026-10-31), then
1.6, 1.61, 1.618; a milestone's updates are 1.0.1, 1.0.2, … (`AGENTS.md`). A release's entry is a `## <version> (<date>)` heading
whose first paragraph is its summary: `phitex` says it once on the first
run of that version, `phitex news` prints the whole entry, and the
GitHub release's notes are the entry. The entries before 1.0 name
milestones, not versions.

## 1.0.2 (2026-11-14)

A big one :) Lots of speed, and a first taste of writing papers together
with an AI agent!

- **Your agent can drive PhiTeX now!** Claude (or any agent) can hop onto
  the watch you already have running with `phitex session`: wait for a
  build, read the errors, and write the PDF when you ask. No more
  copy-pasting logs around.
- **Notes and review (experimental).** Turn it on in Settings, then
  Experimental. Select text on the page to leave a note for your agent,
  and review its edits like a pull request: accept or reject each one,
  or see the typeset diff. It's off by default while we polish it, but
  give it a spin!
- **Edits feel instant.** Pressing `r` with nothing changed takes a
  couple of milliseconds, big plain TeX files edit in a few ms instead
  of fifty, and pages with figures aren't redrawn from scratch.
- **TikZ-heavy papers build about a third faster** from clean. Each
  picture is drawn just once now.
- **Pages stream in as they're typeset,** after every edit too, not only
  on the first build.
- **Colour lines!** A reading aid that shades lines in alternating
  colours, so your eyes don't lose their place. It has its own Settings
  tab.
- **Two-column papers behave.** Selecting text no longer grabs half the
  page, and Colour lines keep the columns apart.
- **Zoom into link previews** with Ctrl+scroll, a pinch, or the buttons.
- **Quitting a watch always leaves the right PDF,** whether you press q,
  hit Ctrl-C, or close the terminal. You can cancel the question now, or
  switch it off with `phitex config watch-ask-write off`.
- **`phitex upgrade` has a progress bar.** Small thing, very satisfying.
- **Fonts are found the way xelatex finds them.**

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

