//! `phitex-trace`: what dependency structure real documents have when
//! pdfTeX runs them, measured from pdfTeX's e-TeX tracing log without
//! implementing TeX.
//!
//! `record` runs the engine with every tracing parameter on and turns
//! the log into steps (one JSON object per line): what each traced action
//! read and wrote. `analyze` builds the def-use graph over those steps (a
//! read binds to the latest preceding write of its field; a group's end
//! writes the outer values back, as `\tracingrestores` reports) and
//! reports its shape.
//!
//! # What the log shows
//!
//! - every primitive executed and every character acted on
//!   (`\tracingcommands=3`, inside expansions too), with the mode;
//! - every assignment, its old and new value and whether it was global
//!   (`\tracingassigns`), and every value a group end restores or retains
//!   (`\tracingrestores`);
//! - every macro call with its parameter text and arguments, and every
//!   token-list parameter inserted (`\tracingmacros=2`);
//! - every conditional, its outcome, `\else` and `\fi`, with the input
//!   line (`\tracingifs`);
//! - every group entered and left, its kind, level and input line
//!   (`\tracinggroups`);
//! - `\scantokens` pseudo-files (`\tracingscantokens`); files opened and
//!   closed (as parentheses, best effort).
//!
//! # What the log does not show
//!
//! - **Reads by primitives.** Only the command is logged, not its
//!   operands: `\advance\count1 by \count2` shows the write to `\count1`
//!   (and this tool adds the read of `\count1`), not the read of
//!   `\count2`; `\hskip\parindent` doesn't show `\parindent`; `\ifnum
//!   \count1>3` shows the test and its outcome, not the register. Reads
//!   that a primitive makes by its nature (a paragraph break reading
//!   `\hsize`, `\tolerance`...) come from a static table in `prims`;
//!   reads through operands are marked `"?"`.
//! - **Expansion reads.** `\the`, `\number`, `\csname`, `\ifx`,
//!   `\expandafter`'s target, `\edef`'s body: the tokens read are not
//!   logged (`"?"`, `consumes: "pending"`).
//! - **Tokenization.** Catcode reads while scanning the file are never
//!   logged; neither are `\endlinechar`, `\escapechar`, `\newlinechar`
//!   reads.
//! - **Per-step input lines.** The log gives the line at group and
//!   conditional boundaries (`at line N`, `entered on line N`) and in
//!   error contexts (`l.N`); every other step gets the last line reported
//!   before it. The file comes from the log's parentheses.
//! - **A macro call's extent.** No end-of-expansion marker exists: the
//!   analyzer takes a call's extent to run until the next macro call.
//! - **Reads of the current font by characters** are not logged; the tool
//!   marks every character as reading `font:current` (and the space
//!   parameters), from `tex.web` §1034ff.
//! - **Hyphenation**: the patterns and exceptions consulted, `\lccode`
//!   reads during hyphenation.
//! - **Page builder and output**: `\vsize`, `\maxdepth`, `\topskip`, the
//!   inserts consulted when contributions move to the page; this tool
//!   marks a `\par` at group level 0 as reading them.
//! - **Box register reads** (`\copy`, `\unhbox`, `\wd`, `\ifvoid`):
//!   unlogged (`"?"`).
//!
//! `--conservative` adds by rule: every step after any catcode write
//! reads `catcode:*`, and every step with an unknown read (`"?"`) reads
//! the step before it (through `seq:prev`, which every step writes), so
//! such steps form a sequential chain.

pub mod analyze;
pub mod fields;
pub mod hash;
pub mod parse;
pub mod prims;
pub mod record;
pub mod step;
