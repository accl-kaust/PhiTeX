//! Field handles interned once per build (the `Graph` owns its one
//! engine for the build's whole life, so a handle stays valid): the
//! state cells with fixed names, the parameters by name, and the
//! category codes by character. A run reads a field through these
//! instead of formatting and interning its name each time; names not
//! here are interned as before.

use std::cell::RefCell;

use phitex_defuse::FxHashMap;
use phitex_defuse::{Field, Values};

use crate::val::Tok;

/// The state cells with fixed names.
const STATIC: &[&str] = &[
    // (the tags of a node's input frames, interned once: token list,
    // source line, file rest)
    "t",
    "s",
    "f",
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

    /// Every handle held here: the engine must keep them alive across
    /// its value sweeps (they are held outside the graph).
    pub(crate) fn handles(&self) -> impl Iterator<Item = Field> + '_ {
        self.names
            .values()
            .chain(self.params.values())
            .chain(self.cat.iter())
            .copied()
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

    /// The field `name`, in a run at values epoch `epoch`
    /// ([`phitex_defuse::Ctx::values_epoch`]): names not in the table are
    /// kept per thread while the epoch lasts (an id made at an epoch is
    /// valid while it lasts), so a name is interned once an epoch, not
    /// once a read.
    pub(crate) fn get_at(&self, v: &Values, epoch: u64, name: &str) -> Field {
        if let Some(&f) = self.names.get(name) {
            return f;
        }
        let hit = with_names(v, epoch, |c| c.names.get(name).copied());
        hit.unwrap_or_else(|| {
            let f = v.field(name);
            with_names(v, epoch, |c| {
                bounded(&mut c.names);
                c.names.insert(name.into(), f);
            });
            f
        })
    }

    /// The field of token `t`'s meaning (`cs:<name>`, `act:<c>`), if it
    /// has one, kept per thread as [`Self::get_at`] keeps names.
    #[allow(clippy::unused_self)]
    pub(crate) fn meaning_at(&self, v: &Values, epoch: u64, t: &Tok) -> Option<Field> {
        if let Some(f) = with_names(v, epoch, |c| c.toks.get(t).copied()) {
            return Some(f);
        }
        let f = v.field(&t.field_name()?);
        with_names(v, epoch, |c| {
            bounded(&mut c.toks);
            c.toks.insert(t.clone(), f);
        });
        Some(f)
    }
}

/// A thread's fields named at one values epoch of one table.
#[derive(Default)]
struct Named {
    instance: u64,
    epoch: u64,
    names: FxHashMap<Box<str>, Field>,
    toks: FxHashMap<Tok, Field>,
}

thread_local! {
    static NAMED: RefCell<Named> = RefCell::default();
}

/// Entries kept per map before it is emptied (a bound, not a policy).
const NAMED_MAX: usize = 1 << 16;

fn bounded<K, V>(m: &mut FxHashMap<K, V>) {
    if m.len() >= crate::val::cache_cap(NAMED_MAX) {
        m.clear();
    }
}

/// `f` on this thread's names for `v` at `epoch` (emptied when either
/// changed).
fn with_names<T>(v: &Values, epoch: u64, f: impl FnOnce(&mut Named) -> T) -> T {
    NAMED.with(|c| {
        let mut c = c.borrow_mut();
        if c.instance != v.instance() || c.epoch != epoch {
            *c = Named {
                instance: v.instance(),
                epoch,
                ..Named::default()
            };
        }
        f(&mut c)
    })
}
