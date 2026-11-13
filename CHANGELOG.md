# Changelog

PhiTeX's versions converge to φ, a digit a milestone: 1.0 is the first
release (the alpha's milestones, the user's decision of 2026-10-31), then
1.6, 1.61, 1.618; a milestone's updates are 1.0.1, 1.0.2, … (`AGENTS.md`). A release's entry is a `## <version> (<date>)` heading
whose first paragraph is its summary: `phitex` says it once on the first
run of that version, `phitex news` prints the whole entry, and the
GitHub release's notes are the entry. The entries before 1.0 name
milestones, not versions.

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

