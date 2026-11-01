//! The read sets of primitives, as far as they are fixed by the primitive
//! itself and not by its operands.
//!
//! The log shows which primitive ran, not what it looked at. For the
//! common primitives below the fields read are known from `tex.web`
//! (parameters a command consults); where a primitive reads operands the
//! log doesn't show (a register in `\advance\count1 by \count2`, the
//! meanings `\ifx` compares, the tokens `\the` reads), the read is marked
//! `"?"`. Primitives not in the table read `"?"` too.

use crate::step::Consumes;

/// The parameters a paragraph break reads (tex.web §848ff, §1070,
/// §1091: `line_break`, `\par` in horizontal mode).
const PAR: &[&str] = &[
    "param:\\hsize",
    "param:\\parshape",
    "param:\\hangindent",
    "param:\\hangafter",
    "param:\\tolerance",
    "param:\\pretolerance",
    "param:\\emergencystretch",
    "param:\\looseness",
    "param:\\leftskip",
    "param:\\rightskip",
    "param:\\parfillskip",
    "param:\\parskip",
    "param:\\baselineskip",
    "param:\\lineskip",
    "param:\\lineskiplimit",
    "param:\\adjdemerits",
    "param:\\linepenalty",
    "param:\\hyphenpenalty",
    "param:\\exhyphenpenalty",
    "param:\\doublehyphendemerits",
    "param:\\finalhyphendemerits",
    "param:\\interlinepenalty",
    "param:\\interlinepenalties",
    "param:\\clubpenalty",
    "param:\\clubpenalties",
    "param:\\widowpenalty",
    "param:\\widowpenalties",
    "param:\\brokenpenalty",
    "param:\\uchyph",
    "param:\\language",
    "param:\\lefthyphenmin",
    "param:\\righthyphenmin",
    "param:\\lastlinefit",
    "param:\\hbadness",
    "param:\\hfuzz",
    "param:\\overfullrule",
    "param:\\pdfadjustspacing",
    "param:\\pdfprotrudechars",
];

/// The parameters an inter-line glue computation reads (§679).
const BASELINE: &[&str] = &[
    "param:\\baselineskip",
    "param:\\lineskip",
    "param:\\lineskiplimit",
    "param:\\prevdepth",
];

/// The parameters the page builder reads (§980ff).
const PAGE: &[&str] = &[
    "param:\\vsize",
    "param:\\maxdepth",
    "param:\\topskip",
    "param:\\holdinginserts",
    "param:\\vbadness",
    "param:\\vfuzz",
    "param:\\output",
    "param:\\outputpenalty",
    "param:\\maxdeadcycles",
];

/// The parameters a math formula reads (§1194ff).
const MATH: &[&str] = &[
    "param:\\mathsurround",
    "param:\\abovedisplayskip",
    "param:\\belowdisplayskip",
    "param:\\abovedisplayshortskip",
    "param:\\belowdisplayshortskip",
    "param:\\predisplaypenalty",
    "param:\\postdisplaypenalty",
    "param:\\displaywidowpenalty",
    "param:\\binoppenalty",
    "param:\\relpenalty",
    "param:\\delimiterfactor",
    "param:\\delimitershortfall",
    "param:\\nulldelimiterspace",
    "param:\\scriptspace",
    "param:\\thinmuskip",
    "param:\\medmuskip",
    "param:\\thickmuskip",
    "param:\\fam",
    "param:\\everymath",
    "param:\\everydisplay",
    "textfont:2",
    "textfont:3",
];

/// What the log doesn't show the primitive reading.
const UNKNOWN: &[&str] = &["?"];
/// Reads nothing beyond its operands, which the log shows.
const NONE: &[&str] = &[];
const CURRENT_FONT: &[&str] = &[
    "font:current",
    "param:\\spacefactor",
    "param:\\spaceskip",
    "param:\\xspaceskip",
];
const EVERYHBOX: &[&str] = &["param:\\everyhbox"];
const EVERYVBOX: &[&str] = &["param:\\everyvbox"];
const EVERYCR: &[&str] = &["param:\\everycr", "param:\\tabskip"];
const INPUT: &[&str] = &["param:\\everyeof", "param:\\endlinechar"];
const CSNAME: &[&str] = &["param:\\escapechar", "?"];

/// A primitive's known reads and what it consumes.
// (The arms with the same reads are kept apart: each lists the primitives
// that read that way for a reason of its own, given above it.)
#[must_use]
#[allow(clippy::too_many_lines, clippy::match_same_arms)]
pub fn reads(name: &str) -> (&'static [&'static str], Option<Consumes>) {
    // Register-assignment primitives print with a number (`\dimen0` for a
    // `\dimendef`ed name): classify by the letters.
    let word = name
        .trim_start_matches('\\')
        .trim_end_matches(|c: char| c.is_ascii_digit() || c == '"');
    match word {
        // prefixes, no-ops
        "global" | "long" | "outer" | "protected" | "relax" | "ignorespaces" | "noindent"
        | "indent" | "noexpand" | "begingroup" | "endgroup" | "nonscript" | "left" | "right"
        | "middle" | "displaylimits" | "limits" | "nolimits" | "end" | "dump" | "unskip"
        | "unkern" | "unpenalty" | "pdfsavepos" | "pdfliteral" | "special" | "shipout"
        | "endcsname" => (NONE, None),
        // definitions whose operands the log shows in the assignment
        "def" | "gdef" | "let" | "futurelet" | "chardef" | "mathchardef" | "countdef"
        | "dimendef" | "skipdef" | "muskipdef" | "toksdef" | "font" | "catcode" | "lccode"
        | "uccode" | "sfcode" | "mathcode" | "delcode" | "read" | "readline" | "openin"
        | "closein" | "openout" | "closeout" | "write" | "immediate" | "message" | "errmessage"
        | "newlinechar" | "escapechar" | "endlinechar" | "setbox" | "box" | "copy" | "unhbox"
        | "unvbox" | "unhcopy" | "unvcopy" | "lastbox" | "vsplit" | "mark" | "marks" | "insert"
        | "vadjust" | "pdfobj" | "pdfrefobj" | "pdfdest" | "pdfstartlink" | "pdfendlink"
        | "pdfannot" | "pdfoutline" | "pdfcatalog" | "pdfinfo" | "pdfnames" | "pdffontattr"
        | "pdfmapfile" | "pdfmapline" | "pdfximage" | "pdfrefximage" | "pdfxform"
        | "pdfrefxform" | "pdfglyphtounicode" => (NONE, Some(Consumes::Known)),
        // expanding definitions and assignments whose right-hand side may
        // read registers the log doesn't show
        "edef"
        | "xdef"
        | "count"
        | "dimen"
        | "skip"
        | "muskip"
        | "toks"
        | "advance"
        | "multiply"
        | "divide"
        | "hskip"
        | "vskip"
        | "kern"
        | "mkern"
        | "mskip"
        | "hfil"
        | "vfil"
        | "hfill"
        | "vfill"
        | "hss"
        | "vss"
        | "hfilneg"
        | "vfilneg"
        | "penalty"
        | "vrule"
        | "hrule"
        | "lower"
        | "raise"
        | "moveleft"
        | "moveright"
        | "accent"
        | "char"
        | "mathchar"
        | "delimiter"
        | "radical"
        | "mathaccent"
        | "discretionary"
        | "hyphenation"
        | "patterns"
        | "pdfpagewidth"
        | "pdfpageheight"
        | "pdfhorigin"
        | "pdfvorigin"
        | "textfont"
        | "scriptfont"
        | "scriptscriptfont"
        | "fontdimen"
        | "hyphenchar"
        | "skewchar"
        | "parshape"
        | "interlinepenalties"
        | "clubpenalties"
        | "widowpenalties"
        | "displaywidowpenalties"
        | "prevdepth"
        | "spacefactor"
        | "prevgraf"
        | "deadcycles"
        | "insertpenalties"
        | "pagegoal"
        | "pagetotal"
        | "pagestretch"
        | "pagefilstretch"
        | "pagefillstretch"
        | "pagefilllstretch"
        | "pageshrink"
        | "pagedepth" => (UNKNOWN, None),
        // expansion primitives reading tokens the log doesn't show
        "the" | "number" | "romannumeral" | "string" | "meaning" | "fontname" | "jobname"
        | "expandafter" | "csname" | "uppercase" | "lowercase" | "unexpanded" | "detokenize"
        | "scantokens" | "numexpr" | "dimexpr" | "glueexpr" | "muexpr" | "eTeXrevision"
        | "eTeXversion" | "pdfstrcmp" | "pdfescapestring" | "pdfescapename" | "pdfescapehex"
        | "pdfunescapehex" | "pdffilesize" | "pdfmdfivesum" | "pdfelapsedtime"
        | "pdfnormaldeviate" | "pdfuniformdeviate" | "pdfrandomseed" | "pdffilemoddate"
        | "pdffiledump" | "pdfcreationdate" | "pdfshellescape" | "pdfmatch" | "pdflastmatch"
        | "pdfximagebbox" | "pdfxformname" | "pdfpageref" | "pdflastobj" | "pdflastxform"
        | "pdflastximage" | "pdflastximagepages" | "pdflastannot" | "pdflastlink" | "pdfretval"
        | "pdftexrevision" | "pdftexbanner" | "pdfcolorstackinit" | "showthe" | "show"
        | "showbox" | "showlists" | "showgroups" | "showtokens" | "showifs"
        | "currentgrouplevel" | "currentgrouptype" | "currentiflevel" | "currentiftype"
        | "currentifbranch" | "lastnodetype" | "lastpenalty" | "lastkern" | "lastskip"
        | "inputlineno" | "badness" | "fontcharwd" | "fontcharht" | "fontchardp" | "fontcharic"
        | "parshapelength" | "parshapeindent" | "parshapedimen" | "topmarks" | "firstmarks"
        | "botmarks" | "splitfirstmarks" | "splitbotmarks" | "topmark" | "firstmark"
        | "botmark" | "splitfirstmark" | "splitbotmark" | "wd" | "ht" | "dp"
        | "afterassignment" | "aftergroup" | "pdfprimitive" => (
            if word == "csname" { CSNAME } else { UNKNOWN },
            Some(Consumes::Pending),
        ),
        "input" | "endinput" => (INPUT, Some(Consumes::Pending)),
        // typesetting
        "par" => (PAR, None),
        "hbox" => (EVERYHBOX, None),
        "vbox" | "vtop" | "vcenter" => (EVERYVBOX, None),
        "halign" | "valign" | "cr" | "crcr" | "noalign" | "omit" | "span" => (EVERYCR, None),
        "eqno" | "leqno" | "mathchoice" | "displaystyle" | "textstyle" | "scriptstyle"
        | "scriptscriptstyle" | "over" | "atop" | "above" | "overwithdelims" | "atopwithdelims"
        | "abovewithdelims" | "underline" | "overline" | "mathord" | "mathop" | "mathbin"
        | "mathrel" | "mathopen" | "mathclose" | "mathpunct" | "mathinner" | "fam" => (MATH, None),
        "leaders" | "cleaders" | "xleaders" | "nointerlineskip" | "offinterlineskip" => {
            (BASELINE, None)
        }
        "pdfoutput"
        | "pdfcompresslevel"
        | "pdfminorversion"
        | "pdfmajorversion"
        | "pdfdecimaldigits"
        | "pdfpkresolution"
        | "pdfobjcompresslevel" => (NONE, None),
        "pagediscards" | "splitdiscards" => (PAGE, None),
        _ => (UNKNOWN, None),
    }
}

/// What a character token reads when typeset.
#[must_use]
pub fn char_reads(class: &str) -> &'static [&'static str] {
    match class {
        "the letter" | "the character" | "blank space" => CURRENT_FONT,
        "math shift character" => MATH,
        "alignment tab character" | "end of alignment template" => EVERYCR,
        _ => NONE,
    }
}

/// The fields a `\par` step in vertical mode is treated as reading: the
/// page builder's, since contributions move to the page then (§1070).
#[must_use]
pub fn page_reads() -> &'static [&'static str] {
    PAGE
}

/// Whether a primitive reads the field it writes (`\advance\count1 by`).
#[must_use]
pub fn reads_its_target(name: &str) -> bool {
    matches!(
        name.trim_start_matches('\\'),
        "advance" | "multiply" | "divide"
    )
}
