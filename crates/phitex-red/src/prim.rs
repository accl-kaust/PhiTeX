//! The primitive registry and parameter numbers.
use crate::ctx::Ctx;
use crate::val::{Key, Val};

/// A primitive's id: its index in the registry (deterministic: registration order).
pub type PrimId = u16;

/// Value levels (tex.web 410).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub enum Level {
    Int,
    Dimen,
    Glue,
    Mu,
    Ident,
    Tok,
}

#[derive(Clone, Copy)]
pub enum Kind {
    /// Expandable: rewrites the input (`\expandafter`, `\csname`, `\the`, `\input` …).
    Expand(fn(&mut dyn Ctx, u16)),
    /// A conditional test (`\ifx` …): returns the branch; `cond.rs` does the select.
    If(fn(&mut dyn Ctx, u16) -> bool),
    /// A command executed in the current list.
    Command(fn(&mut dyn Ctx, u16)),
    /// A quantity that is a name in env: `key` scans what follows the token (a
    /// register number) and gives the key. Read by `\the`/scanning; assigned by `assign.rs`.
    Quantity { level: Level, key: fn(&mut dyn Ctx, u16) -> Key },
    /// A read-only (or specially assigned) computed quantity (`\lastskip`, `\badness`, `\wd`…).
    Computed { level: Level, get: fn(&mut dyn Ctx, u16) -> Val, set: Option<fn(&mut dyn Ctx, u16, Val, bool)> },
    /// `\global` `\long` `\outer` `\protected` (code 1 2 4 8).
    Prefix,
}

#[derive(Clone, Copy)]
pub struct Def {
    pub name: &'static str,
    pub code: u16,
    pub kind: Kind,
    /// 0 TeX, 1 e-TeX (extended mode), 2 pdfTeX.
    pub engine: u8,
}

#[derive(Default)]
pub struct Registry {
    pub defs: Vec<Def>,
}

impl Registry {
    pub fn add(&mut self, name: &'static str, code: u16, kind: Kind, engine: u8) {
        self.defs.push(Def { name, code, kind, engine });
    }
    pub fn get(&self, id: PrimId) -> &Def {
        &self.defs[id as usize]
    }
    pub fn by_name(&self, name: &str) -> Option<PrimId> {
        self.defs.iter().position(|d| d.name == name).map(|i| i as PrimId)
    }
    /// Every module's `register`, in this fixed order.
    pub fn all() -> Registry {
        let mut r = Registry::default();
        crate::expand::register(&mut r);
        crate::scan::register(&mut r);
        crate::macros::register(&mut r);
        crate::cond::register(&mut r);
        crate::assign::register(&mut r);
        crate::groups::register(&mut r);
        crate::input::register(&mut r);
        crate::list::register(&mut r);
        crate::boxes::register(&mut r);
        crate::linebreak::register(&mut r);
        crate::hyph::register(&mut r);
        crate::math::register(&mut r);
        crate::align::register(&mut r);
        crate::font::register(&mut r);
        crate::page::register(&mut r);
        crate::output::register(&mut r);
        crate::io::register(&mut r);
        crate::pdfprim::register(&mut r);
        crate::pdffont::register(&mut r);
        crate::diag::register(&mut r);
        r
    }
}

/// The session's registry: no name registered twice.
#[cfg(test)]
mod tests {
    #[test]
    fn no_duplicate_names() {
        let r = super::registry();
        let mut seen = std::collections::HashMap::new();
        let mut dups = Vec::new();
        for d in &r.defs {
            if seen.insert(d.name, ()).is_some() {
                dups.push(d.name);
            }
        }
        assert!(dups.is_empty(), "registered twice: {dups:?}");
    }

    #[test]
    fn relax_registered() {
        assert!(super::registry().by_name("relax").is_some());
    }
}

/// The registry of the session (built once).
pub fn registry() -> &'static Registry {
    static R: std::sync::OnceLock<Registry> = std::sync::OnceLock::new();
    R.get_or_init(Registry::all)
}

/// Module tags for `Key::Other`.
pub mod tag {
    pub const ALIGN: u8 = 1;
    pub const MATH: u8 = 2;
    pub const PAGE: u8 = 3;
    pub const OUTPUT: u8 = 4;
    pub const IO: u8 = 5;
    pub const PDF: u8 = 6;
    pub const FONT: u8 = 7;
    pub const LIST: u8 = 8;
    pub const DIAG: u8 = 9;
    pub const EXPAND: u8 = 10;
    pub const BOXES: u8 = 11;
}

/// Integer parameters (tex.web 236, then e-TeX, then pdfTeX).
pub mod ip {
    pub const NAMES: &[&str] = &[
        "pretolerance", "tolerance", "linepenalty", "hyphenpenalty", "exhyphenpenalty",
        "clubpenalty", "widowpenalty", "displaywidowpenalty", "brokenpenalty", "binoppenalty",
        "relpenalty", "predisplaypenalty", "postdisplaypenalty", "interlinepenalty",
        "doublehyphendemerits", "finalhyphendemerits", "adjdemerits", "mag", "delimiterfactor",
        "looseness", "time", "day", "month", "year", "showboxbreadth", "showboxdepth", "hbadness",
        "vbadness", "pausing", "tracingonline", "tracingmacros", "tracingstats", "tracingparagraphs",
        "tracingpages", "tracingoutput", "tracinglostchars", "tracingcommands", "tracingrestores",
        "uchyph", "outputpenalty", "maxdeadcycles", "hangafter", "floatingpenalty", "globaldefs",
        "fam", "escapechar", "defaulthyphenchar", "defaultskewchar", "endlinechar", "newlinechar",
        "language", "lefthyphenmin", "righthyphenmin", "holdinginserts", "errorcontextlines",
        // e-TeX
        "tracingassigns", "tracinggroups", "tracingifs", "tracingscantokens", "tracingnesting",
        "predisplaydirection", "lastlinefit", "savingvdiscards", "savinghyphcodes", "TeXXeTstate",
        // pdfTeX
        "pdfoutput", "pdfcompresslevel", "pdfdecimaldigits", "pdfmovechars", "pdfimageresolution",
        "pdfpkresolution", "pdfuniqueresname", "pdfoptionalwaysusepdfpagebox",
        "pdfoptionpdfinclusionerrorlevel", "pdfmajorversion", "pdfminorversion", "pdfforcepagebox",
        "pdfpagebox", "pdfinclusionerrorlevel", "pdfgamma", "pdfimagegamma", "pdfimagehicolor",
        "pdfimageapplygamma", "pdfadjustspacing", "pdfprotrudechars", "pdftracingfonts",
        "pdfobjcompresslevel", "pdfadjustinterwordglue", "pdfprependkern", "pdfappendkern",
        "pdfgentounicode", "pdfdraftmode", "pdfinclusioncopyfonts", "pdfsuppresswarningdupdest",
        "pdfsuppresswarningdupmap", "pdfsuppresswarningpagegroup", "pdfinfoomitdate",
        "pdfsuppressptexinfo", "pdfomitcharset", "pdfomitprocset", "showstream",
    ];
    pub fn of(name: &str) -> u16 {
        NAMES.iter().position(|n| *n == name).expect(name) as u16
    }
}

/// Dimension parameters (tex.web 247, then pdfTeX).
pub mod dp {
    pub const NAMES: &[&str] = &[
        "parindent", "mathsurround", "lineskiplimit", "hsize", "vsize", "maxdepth",
        "splitmaxdepth", "boxmaxdepth", "hfuzz", "vfuzz", "delimitershortfall", "nulldelimiterspace",
        "scriptspace", "predisplaysize", "displaywidth", "displayindent", "overfullrule",
        "hangindent", "hoffset", "voffset", "emergencystretch",
        "pdfhorigin", "pdfvorigin", "pdfpagewidth", "pdfpageheight", "pdflinkmargin",
        "pdfdestmargin", "pdfthreadmargin", "pdffirstlineheight", "pdflastlinedepth",
        "pdfeachlineheight", "pdfeachlinedepth", "pdfignoreddimen", "pdfpxdimen",
    ];
    pub fn of(name: &str) -> u16 {
        NAMES.iter().position(|n| *n == name).expect(name) as u16
    }
}

/// Glue parameters (tex.web 224).
pub mod gp {
    pub const NAMES: &[&str] = &[
        "lineskip", "baselineskip", "parskip", "abovedisplayskip", "belowdisplayskip",
        "abovedisplayshortskip", "belowdisplayshortskip", "leftskip", "rightskip", "topskip",
        "splittopskip", "tabskip", "spaceskip", "xspaceskip", "parfillskip",
    ];
    pub fn of(name: &str) -> u16 {
        NAMES.iter().position(|n| *n == name).expect(name) as u16
    }
}

/// Mu glue parameters.
pub mod mp {
    pub const NAMES: &[&str] = &["thinmuskip", "medmuskip", "thickmuskip"];
}

/// Token parameters (tex.web 230, then e-TeX, pdfTeX).
pub mod tp {
    pub const NAMES: &[&str] = &[
        "output", "everypar", "everymath", "everydisplay", "everyhbox", "everyvbox", "everyjob",
        "everycr", "errhelp", "everyeof", "pdfpagesattr", "pdfpageattr", "pdfpageresources",
        "pdfpkmode",
    ];
    pub fn of(name: &str) -> u16 {
        NAMES.iter().position(|n| *n == name).expect(name) as u16
    }
}
