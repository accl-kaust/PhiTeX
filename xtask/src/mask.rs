//! Output that partex deliberately does not reproduce: statistics about
//! TeX's internal tables (memory, string pool, hash, font memory, trie,
//! stacks). partex's data structures are its own; only the typeset output
//! (DVI/PDF) and the rest of the transcript must match the oracle.

use regex::Regex;

/// Regexes for the internal statistics TeX prints, with replacements.
pub fn memory_statistics() -> Vec<(Regex, &'static str)> {
    [
        // bibtex.ch: web2c logs each time BibTeX's arrays grow.
        (r"(?m)^Reallocated .*\n", ""),
        (r"(?m)^Field filled up at .*\n", ""),
        // §1334: the job's statistics block.
        (r"(?m)^ \d+ strings? out of \d+$", " MASKED strings"),
        (
            r"(?m)^ \d+ string characters out of \d+$",
            " MASKED string characters",
        ),
        (
            r"(?m)^ \d+ multiletter control sequences out of \d+\+\d+$",
            " MASKED multiletter control sequences",
        ),
        (
            r"(?m)^ \d+ words of font info for \d+ fonts?, out of \d+ for \d+$",
            " MASKED words of font info",
        ),
        (
            r"(?m)^ \d+ hyphenation exceptions? out of \d+$",
            " MASKED hyphenation exceptions",
        ),
        (
            r"(?m)^ \d+i,\d+n,\d+p,\d+b,\d+s stack positions out of .*$",
            " MASKED stack positions",
        ),
        // §1309–§1324: `\dump`.
        (r"(?m)^\d+ strings of total length \d+$", "MASKED strings"),
        (
            r"(?m)^\d+ multiletter control sequences$",
            "MASKED multiletter control sequences",
        ),
        (
            r"(?m)^\d+ words of font info for \d+ preloaded fonts?$",
            "MASKED font info",
        ),
        (
            r"(?m)^\d+ hyphenation exceptions?$",
            "MASKED hyphenation exceptions",
        ),
        (
            r"(?m)^Hyphenation trie of length \d+ has \d+ ops? out of \d+$",
            "MASKED hyphenation trie",
        ),
        // §639: `\tracingstats` after each page.
        (
            r"Memory usage before: \d+&\d+; after: \d+&\d+; still untouched: \d+",
            "Memory usage MASKED",
        ),
        // §1334: statistics at the end of the job.
        (r"\d+ words of memory out of \d+", "MASKED words of memory"),
        // pdfTeX's `pdf_mem` (its font code tables live there, too).
        (
            r"(?m)^ \d+ words of extra memory for PDF output out of \d+ \(max\. \d+\)$",
            " MASKED words of extra memory for PDF output",
        ),
        // §1311: `\dump`.
        (
            r"\d+ memory locations dumped; current usage is \d+&\d+",
            "MASKED memory locations dumped",
        ),
    ]
    .into_iter()
    .map(|(re, rep)| (Regex::new(re).expect("valid regex"), rep))
    .collect()
}
