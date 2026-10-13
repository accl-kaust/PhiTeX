//! Field handles interned once per build (the `Graph` owns its one
//! engine for the build's whole life, so a handle stays valid): the
//! state cells with fixed names, the parameters by name, and the
//! category codes by character. A run reads a field through these
//! instead of formatting and interning its name each time; names not
//! here are interned as before.

use phitex_defuse::FxHashMap;
use phitex_defuse::{Field, Values};

/// The state cells with fixed names.
const STATIC: &[&str] = &[
    "aftertoken",
    "align:open",
    "align:state",
    "badness",
    "deadcycles",
    "diags",
    "dvicomment",
    "font",
    "fontcount",
    "ifstack",
    "inspen",
    "interaction",
    "items",
    "jobname",
    "lines",
    "log",
    "mark:bot",
    "mark:top",
    "markclasses",
    "needs",
    "page:state",
    "pagehash",
    "pages",
    "parshape",
    "partoken",
    "patterns",
    "pdf:carry",
    "pdf:colorstacks",
    "pdf:exact",
    "pdf:fixed",
    "pdf:fontcount",
    "pdf:lastannot",
    "pdf:lastlink",
    "pdf:lastobj",
    "pdf:lastph",
    "pdf:lastxform",
    "pdf:lastximage",
    "pdf:lastximagecolordepth",
    "pdf:lastximagepages",
    "pdf:lastxpos",
    "pdf:lastypos",
    "pdf:maps",
    "pdf:match",
    "pdf:objnum",
    "pdf:openaction",
    "pdf:pieces",
    "pdf:random",
    "pdf:relaxed",
    "pdf:retval",
    "pdf:spacefont",
    "pdf:state",
    "pdf:trailerid",
    "pdf:xformcount",
    "pdfdate",
    "streams",
    "tails",
    "writes",
];

/// The parameters (`param:<name>`) known by name.
const PARAMS: &[&str] = &[
    "adjdemerits",
    "baselineskip",
    "binoppenalty",
    "boxmaxdepth",
    "brokenpenalty",
    "clubpenalty",
    "defaulthyphenchar",
    "defaultskewchar",
    "delimiterfactor",
    "delimitershortfall",
    "displayindent",
    "displaywidowpenalty",
    "displaywidth",
    "doublehyphendemerits",
    "emergencystretch",
    "endlinechar",
    "escapechar",
    "everycr",
    "everydisplay",
    "everyeof",
    "everymath",
    "everypar",
    "everyvbox",
    "exhyphenpenalty",
    "fam",
    "finalhyphendemerits",
    "floatingpenalty",
    "globaldefs",
    "hangafter",
    "hangindent",
    "hbadness",
    "hfuzz",
    "hoffset",
    "holdinginserts",
    "hsize",
    "hyphenpenalty",
    "interlinepenalty",
    "language",
    "lastlinefit",
    "lefthyphenmin",
    "leftskip",
    "linepenalty",
    "lineskip",
    "lineskiplimit",
    "looseness",
    "mag",
    "mathsurround",
    "maxdepth",
    "medmuskip",
    "newlinechar",
    "nulldelimiterspace",
    "output",
    "overfullrule",
    "parfillskip",
    "parindent",
    "parskip",
    "pdfcompresslevel",
    "pdfdecimaldigits",
    "pdfdestmargin",
    "pdfdraftmode",
    "pdfgamma",
    "pdfgentounicode",
    "pdfhorigin",
    "pdfimageapplygamma",
    "pdfimagegamma",
    "pdfimagehicolor",
    "pdfimageresolution",
    "pdfinclusioncopyfonts",
    "pdfinclusionerrorlevel",
    "pdfinfoomitdate",
    "pdflinkmargin",
    "pdfmajorversion",
    "pdfminorversion",
    "pdfobjcompresslevel",
    "pdfoutput",
    "pdfpagebox",
    "pdfpageheight",
    "pdfpagewidth",
    "pdfsuppressptexinfo",
    "pdfthreadmargin",
    "pdfvorigin",
    "postdisplaypenalty",
    "predisplaypenalty",
    "predisplaysize",
    "pretolerance",
    "relpenalty",
    "righthyphenmin",
    "rightskip",
    "scriptspace",
    "spaceskip",
    "splitmaxdepth",
    "splittopskip",
    "tabskip",
    "thickmuskip",
    "thinmuskip",
    "tolerance",
    "topskip",
    "tracinglostchars",
    "uchyph",
    "vbadness",
    "vfuzz",
    "voffset",
    "vsize",
    "widowpenalty",
    "xspaceskip",
];

/// The handles of one build's fields.
pub(crate) struct Fields {
    names: FxHashMap<&'static str, Field>,
    params: FxHashMap<&'static str, Field>,
    cat: Vec<Field>,
}

impl Fields {
    pub(crate) fn new(v: &Values) -> Fields {
        let names = STATIC.iter().map(|&n| (n, v.field(n))).collect();
        let params = PARAMS
            .iter()
            .map(|&n| (n, v.field(&format!("param:{n}"))))
            .collect();
        let cat = (0..256).map(|c| v.field(&format!("cat:{c}"))).collect();
        Fields { names, params, cat }
    }

    /// The field `name`.
    pub(crate) fn get(&self, v: &Values, name: &str) -> Field {
        match self.names.get(name) {
            Some(&f) => f,
            None => v.field(name),
        }
    }

    /// The parameter `name` (`param:<name>`).
    pub(crate) fn param(&self, v: &Values, name: &str) -> Field {
        match self.params.get(name) {
            Some(&f) => f,
            None => v.field(&format!("param:{name}")),
        }
    }

    /// The category code of character `c` (`cat:<c>`).
    pub(crate) fn cat(&self, v: &Values, c: u32) -> Field {
        match usize::try_from(c).ok().and_then(|i| self.cat.get(i)) {
            Some(&f) => f,
            None => v.field(&format!("cat:{c}")),
        }
    }
}
