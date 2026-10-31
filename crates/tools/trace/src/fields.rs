//! Field names: the slot of TeX's state an assignment line of the log
//! names, as a string `kind:key`.
//!
//! - registers: `count:12`, `dimen:3`, `skip:7`, `muskip:0`, `toks:5`,
//!   `box:0`, `catcode:124`, `lccode:65`, `uccode:65`, `sfcode:46`,
//!   `mathcode:43`, `delcode:40`, `textfont:3`, `scriptfont:3`,
//!   `scriptscriptfont:3`;
//! - parameters: `param:\tolerance` (every integer, dimen, glue and
//!   token-list parameter of TeX, e-TeX and pdfTeX);
//! - control sequences: `cs:\foo` (macros, `\let` meanings, fonts loaded
//!   with `\font\foo`, `\chardef`s...), active characters `cs:~`;
//! - the current font: `font:current`;
//! - anything else the log names, verbatim: `misc:...`.

/// The register families the log prints as `\kindNUMBER`.
const REGISTERS: &[&str] = &[
    "count",
    "dimen",
    "skip",
    "muskip",
    "toks",
    "box",
    "catcode",
    "lccode",
    "uccode",
    "sfcode",
    "mathcode",
    "delcode",
    "textfont",
    "scriptfont",
    "scriptscriptfont",
    "insert",
    "marks",
];

/// TeX's, e-TeX's and pdfTeX's parameters (tex.web §236, §247, §224,
/// §230; etex.ch; pdftex.web).
pub const PARAMS: &[&str] = &[
    // integer parameters (§236)
    "pretolerance",
    "tolerance",
    "linepenalty",
    "hyphenpenalty",
    "exhyphenpenalty",
    "clubpenalty",
    "widowpenalty",
    "displaywidowpenalty",
    "brokenpenalty",
    "binoppenalty",
    "relpenalty",
    "predisplaypenalty",
    "postdisplaypenalty",
    "interlinepenalty",
    "doublehyphendemerits",
    "finalhyphendemerits",
    "adjdemerits",
    "mag",
    "delimiterfactor",
    "looseness",
    "time",
    "day",
    "month",
    "year",
    "showboxbreadth",
    "showboxdepth",
    "hbadness",
    "vbadness",
    "pausing",
    "tracingonline",
    "tracingmacros",
    "tracingstats",
    "tracingparagraphs",
    "tracingpages",
    "tracingoutput",
    "tracinglostchars",
    "tracingcommands",
    "tracingrestores",
    "uchyph",
    "outputpenalty",
    "maxdeadcycles",
    "hangafter",
    "floatingpenalty",
    "globaldefs",
    "fam",
    "escapechar",
    "defaulthyphenchar",
    "defaultskewchar",
    "endlinechar",
    "newlinechar",
    "language",
    "lefthyphenmin",
    "righthyphenmin",
    "holdinginserts",
    "errorcontextlines",
    // e-TeX
    "tracingassigns",
    "tracinggroups",
    "tracingifs",
    "tracingscantokens",
    "tracingnesting",
    "predisplaydirection",
    "lastlinefit",
    "savingvdiscards",
    "savinghyphcodes",
    "interactionmode",
    // pdfTeX
    "pdfoutput",
    "pdfcompresslevel",
    "pdfdecimaldigits",
    "pdfimageresolution",
    "pdfpkresolution",
    "pdftracingfonts",
    "pdfuniqueresname",
    "pdfadjustspacing",
    "pdfprotrudechars",
    "pdfminorversion",
    "pdfmajorversion",
    "pdfforcepagebox",
    "pdfoptionalwaysusepdfpagebox",
    "pdfoptionpdfinclusionerrorlevel",
    "pdfinclusionerrorlevel",
    "pdfimagehicolor",
    "pdfimageapplygamma",
    "pdfgamma",
    "pdfimagegamma",
    "pdfdraftmode",
    "pdfinclusioncopyfonts",
    "pdfsuppressptexinfo",
    "pdfobjcompresslevel",
    "pdfgentounicode",
    "pdfomitcharset",
    "pdfomitinfodict",
    "pdfomitprocset",
    "pdfinfoomitdate",
    "pdfsuppresswarningdupdest",
    "pdfsuppresswarningdupmap",
    "pdfsuppresswarningpagegroup",
    "pdfinterwordspaceon",
    "pdfinterwordspaceoff",
    "pdfpagebox",
    "pdfcompresslevel",
    "tracingstacklevels",
    "partokencontext",
    "showstream",
    // dimen parameters (§247)
    "parindent",
    "mathsurround",
    "lineskiplimit",
    "hsize",
    "vsize",
    "maxdepth",
    "splitmaxdepth",
    "boxmaxdepth",
    "hfuzz",
    "vfuzz",
    "delimitershortfall",
    "nulldelimiterspace",
    "scriptspace",
    "predisplaysize",
    "displaywidth",
    "displayindent",
    "overfullrule",
    "hangindent",
    "hoffset",
    "voffset",
    "emergencystretch",
    "pdfhorigin",
    "pdfvorigin",
    "pdfpagewidth",
    "pdfpageheight",
    "pdflinkmargin",
    "pdfdestmargin",
    "pdfthreadmargin",
    "pdfpxdimen",
    // glue parameters (§224)
    "lineskip",
    "baselineskip",
    "parskip",
    "abovedisplayskip",
    "belowdisplayskip",
    "abovedisplayshortskip",
    "belowdisplayshortskip",
    "leftskip",
    "rightskip",
    "topskip",
    "splittopskip",
    "tabskip",
    "spaceskip",
    "xspaceskip",
    "parfillskip",
    "thinmuskip",
    "medmuskip",
    "thickmuskip",
    // token-list parameters (§230)
    "output",
    "everypar",
    "everymath",
    "everydisplay",
    "everyhbox",
    "everyvbox",
    "everyjob",
    "everycr",
    "errhelp",
    "everyeof",
    "pdfpagesattr",
    "pdfpageattr",
    "pdfpageresources",
    "pdfpkmode",
    // other assignable internal quantities the log names
    "parshape",
    "interlinepenalties",
    "clubpenalties",
    "widowpenalties",
    "displaywidowpenalties",
    "prevdepth",
    "spacefactor",
    "prevgraf",
    "deadcycles",
    "insertpenalties",
    "pagegoal",
    "pagetotal",
    "pagestretch",
    "pagefilstretch",
    "pagefillstretch",
    "pagefilllstretch",
    "pageshrink",
    "pagedepth",
    "hyphenchar",
    "skewchar",
    "fontdimen",
    "lastpenalty",
    "lastkern",
    "lastskip",
];

/// Whether `name` (without its backslash) is a parameter.
#[must_use]
pub fn is_param(name: &str) -> bool {
    PARAMS.contains(&name)
}

/// The field an assignment line names (the text before its `=`).
#[must_use]
pub fn field_of(name: &str) -> String {
    let name = name.trim();
    if name == "current font" {
        return "font:current".to_owned();
    }
    if let Some(rest) = name.strip_prefix('\\') {
        if rest.contains('\\') {
            // `\hyphenchar\tenrm`, `\fontdimen2\tenrm`: a font's slot.
            return format!("misc:\\{rest}");
        }
        let word = rest.trim_end_matches(|c: char| c.is_ascii_digit());
        let tail = &rest[word.len()..];
        if !tail.is_empty() && REGISTERS.contains(&word) {
            return format!("{word}:{tail}");
        }
        return if is_param(rest) {
            format!("param:\\{rest}")
        } else {
            format!("cs:\\{rest}")
        };
    }
    if name.chars().count() == 1 {
        // An active character.
        return format!("cs:{name}");
    }
    format!("misc:{name}")
}

#[cfg(test)]
mod tests {
    use super::field_of;

    #[test]
    fn classes() {
        assert_eq!(field_of("\\count1"), "count:1");
        assert_eq!(field_of("\\catcode64"), "catcode:64");
        assert_eq!(field_of("\\tracingassigns"), "param:\\tracingassigns");
        assert_eq!(field_of("\\hsize"), "param:\\hsize");
        assert_eq!(field_of("\\foo"), "cs:\\foo");
        assert_eq!(field_of("\\@"), "cs:\\@");
        assert_eq!(field_of("current font"), "font:current");
        assert_eq!(field_of("~"), "cs:~");
        assert_eq!(field_of("\\hyphenchar\\tenrm"), "misc:\\hyphenchar\\tenrm");
        assert_eq!(field_of("\\textfont3"), "textfont:3");
        assert_eq!(field_of("\\MT@feat"), "cs:\\MT@feat");
        assert_eq!(field_of("\\c@page"), "cs:\\c@page");
    }
}
