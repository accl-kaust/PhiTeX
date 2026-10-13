//! Field handles interned once per build (the `Graph` owns its one
//! engine for the build's whole life, so a handle stays valid): the
//! state cells with fixed names, the parameters by name, and the
//! category codes by character. A run reads a field through these
//! instead of formatting and interning its name each time; names not
//! here are interned as before.

use std::cell::RefCell;

use phitex_defuse::FxHashMap;
use phitex_defuse::{Field, Values};

use crate::val::{COUNT_BASE, MU_SKIP_BASE, SCALED_BASE, SKIP_BASE, TOKS_BASE, Tok, cmd};

/// The register kinds by slot block (count, dimen, skip, muskip, toks).
const REGS: [&str; 5] = ["count", "dimen", "skip", "muskip", "toks"];

/// The slot of register `n` of kind `r` (0 count … 4 toks), if 0–255.
pub(crate) fn reg_slot(r: usize, n: i32) -> Option<usize> {
    usize::try_from(n)
        .ok()
        .filter(|&n| n < 256)
        .map(|n| REG0 + r * 256 + n)
}

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
    "writes",
];

/// The parameters (`param:<name>`) known by name.
/// Declares the parameters known by name: their names (`PARAMS`) and
/// their indices ([`Par`]), in the same order.
macro_rules! params {
    ($($n:ident),* $(,)?) => {
        const PARAMS: &[&str] = &[$(stringify!($n)),*];
        /// A parameter's field slot.
        #[allow(non_camel_case_types, dead_code)]
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub(crate) enum Par {
            $($n),*
        }
    };
}

params!(
    adjdemerits,
    baselineskip,
    binoppenalty,
    boxmaxdepth,
    brokenpenalty,
    clubpenalty,
    defaulthyphenchar,
    defaultskewchar,
    delimiterfactor,
    delimitershortfall,
    displayindent,
    displaywidowpenalty,
    displaywidth,
    doublehyphendemerits,
    emergencystretch,
    endlinechar,
    escapechar,
    everycr,
    everydisplay,
    everyeof,
    everymath,
    everypar,
    everyvbox,
    exhyphenpenalty,
    fam,
    finalhyphendemerits,
    floatingpenalty,
    globaldefs,
    hangafter,
    hangindent,
    hbadness,
    hfuzz,
    hoffset,
    holdinginserts,
    hsize,
    hyphenpenalty,
    interlinepenalty,
    language,
    lastlinefit,
    lefthyphenmin,
    leftskip,
    linepenalty,
    lineskip,
    lineskiplimit,
    looseness,
    mag,
    mathsurround,
    maxdepth,
    medmuskip,
    newlinechar,
    nulldelimiterspace,
    output,
    overfullrule,
    parfillskip,
    parindent,
    parskip,
    pdfcompresslevel,
    pdfdecimaldigits,
    pdfdestmargin,
    pdfdraftmode,
    pdfgamma,
    pdfgentounicode,
    pdfhorigin,
    pdfimageapplygamma,
    pdfimagegamma,
    pdfimagehicolor,
    pdfimageresolution,
    pdfinclusioncopyfonts,
    pdfinclusionerrorlevel,
    pdfinfoomitdate,
    pdflinkmargin,
    pdfmajorversion,
    pdfminorversion,
    pdfobjcompresslevel,
    pdfoutput,
    pdfpagebox,
    pdfpageheight,
    pdfpagewidth,
    pdfsuppressptexinfo,
    pdfthreadmargin,
    pdfvorigin,
    postdisplaypenalty,
    predisplaypenalty,
    predisplaysize,
    pretolerance,
    relpenalty,
    righthyphenmin,
    rightskip,
    scriptspace,
    spaceskip,
    splitmaxdepth,
    splittopskip,
    tabskip,
    thickmuskip,
    thinmuskip,
    tolerance,
    topskip,
    tracinglostchars,
    uchyph,
    vbadness,
    vfuzz,
    voffset,
    vsize,
    widowpenalty,
    xspaceskip,
);

/// The field slots of a run (a run-local cache of field values by slot):
/// the parameters known by name ([`Par`]), the category codes of
/// characters 0–255, and registers 0–255 of each kind (count, dimen,
/// skip, muskip, toks). A slot's field is fixed for the build.
pub(crate) const CAT0: usize = PARAMS.len();
pub(crate) const REG0: usize = CAT0 + 256;
/// The number of slots.
pub(crate) const NSLOT: usize = REG0 + 5 * 256;

/// The handles of one build's fields.
pub(crate) struct Fields {
    names: FxHashMap<&'static str, Field>,
    params: FxHashMap<&'static str, Field>,
    /// The field of each slot of the run's field slots.
    slot_field: Vec<Field>,
    /// The slot of a field that has one.
    field_slot: FxHashMap<Field, u32>,
    /// The slot of a parameter by its name.
    par_slot: FxHashMap<&'static str, u32>,
    /// The slot of a parameter by its assignment command and code.
    qpar: FxHashMap<(u8, i32), u32>,
}

impl Fields {
    pub(crate) fn new(v: &Values) -> Fields {
        let names = STATIC.iter().map(|&n| (n, v.field(n))).collect();
        let params: FxHashMap<&'static str, Field> = PARAMS
            .iter()
            .map(|&n| (n, v.field(&format!("param:{n}"))))
            .collect();
        let mut slot_field: Vec<Field> = PARAMS.iter().map(|n| params[n]).collect();
        slot_field.extend((0..256).map(|c| v.field(&format!("cat:{c}"))));
        for reg in REGS {
            slot_field.extend((0..256).map(|n| v.field(&format!("{reg}:{n}"))));
        }
        debug_assert_eq!(slot_field.len(), NSLOT);
        let field_slot = slot_field
            .iter()
            .enumerate()
            .map(|(i, &f)| (f, u32::try_from(i).unwrap_or(u32::MAX)))
            .collect();
        let par_slot: FxHashMap<&'static str, u32> = PARAMS
            .iter()
            .enumerate()
            .map(|(i, &n)| (n, u32::try_from(i).unwrap_or(u32::MAX)))
            .collect();
        let qpar = crate::etex::all_prims()
            .filter(|&&(_, c, _)| (cmd::ASSIGN_TOKS..=cmd::ASSIGN_MU_GLUE).contains(&c))
            .filter_map(|&(n, c, h)| Some(((c, h), *par_slot.get(n)?)))
            .collect();
        Fields {
            names,
            params,
            slot_field,
            field_slot,
            par_slot,
            qpar,
        }
    }

    /// Every handle held here: the engine must keep them alive across
    /// its value sweeps (they are held outside the graph).
    pub(crate) fn handles(&self) -> impl Iterator<Item = Field> + '_ {
        self.names.values().chain(self.slot_field.iter()).copied()
    }

    /// The field of slot `s`.
    pub(crate) fn slot_field(&self, s: usize) -> Field {
        self.slot_field[s]
    }

    /// The slot of field `f`, if it has one.
    pub(crate) fn field_slot(&self, f: Field) -> Option<usize> {
        self.field_slot.get(&f).map(|&s| s as usize)
    }

    /// The slot of the parameter `name`, if it has one.
    pub(crate) fn par_slot(&self, name: &str) -> Option<usize> {
        self.par_slot.get(name).map(|&s| s as usize)
    }

    /// The slot of the field of the primitive quantity at `(c, chr)` for the assignment
    /// commands (§1224, §1228), as [`crate::val::quantity_field`] names
    /// it: a parameter known by name, or a register 0–255.
    pub(crate) fn quantity_slot(&self, c: u8, chr: i32) -> Option<usize> {
        let (base, r) = match c {
            cmd::ASSIGN_INT => (COUNT_BASE, 0),
            cmd::ASSIGN_DIMEN => (SCALED_BASE, 1),
            cmd::ASSIGN_GLUE => (SKIP_BASE, 2),
            cmd::ASSIGN_MU_GLUE => (MU_SKIP_BASE, 3),
            _ => (TOKS_BASE, 4),
        };
        if chr >= base {
            reg_slot(r, chr - base)
        } else {
            self.qpar.get(&(c, chr)).map(|&s| s as usize)
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
        match usize::try_from(c).ok().filter(|&i| i < 256) {
            Some(i) => self.slot_field[CAT0 + i],
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

    /// The field `<fam>:<key>`, kept per thread as [`Self::get_at`]
    /// keeps names (formatted and interned once an epoch).
    #[allow(clippy::unused_self)]
    pub(crate) fn keyed(&self, v: &Values, epoch: u64, fam: Fam, key: &str) -> Field {
        let i = fam as usize;
        if let Some(f) = with_names(v, epoch, |c| c.keyed[i].get(key).copied()) {
            return f;
        }
        let f = v.field(&format!("{}:{key}", fam.prefix()));
        with_names(v, epoch, |c| {
            bounded(&mut c.keyed[i]);
            c.keyed[i].insert(key.into(), f);
        });
        f
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
    /// The fields of the keyed families ([`Fam`]), by family and key.
    keyed: [FxHashMap<Box<str>, Field>; FAMS],
}

/// The field families keyed by a name (`<family>:<key>`): files, the
/// write-and-read-back fields, declarations, a page's effects.
#[derive(Clone, Copy)]
pub(crate) enum Fam {
    File,
    Wfile,
    Wkeys,
    Wdef,
    Wfiledecl,
    Wdefdecl,
    Pagefx,
    Marksdecl,
}

const FAMS: usize = 8;

impl Fam {
    fn prefix(self) -> &'static str {
        match self {
            Fam::File => "file",
            Fam::Wfile => "wfile",
            Fam::Wkeys => "wkeys",
            Fam::Wdef => "wdef",
            Fam::Wfiledecl => "wfiledecl",
            Fam::Wdefdecl => "wdefdecl",
            Fam::Pagefx => "pagefx",
            Fam::Marksdecl => "marksdecl",
        }
    }
}

thread_local! {
    static NAMED: RefCell<Named> = RefCell::default();
}

/// Entries kept per map before it is emptied (a bound, not a policy).
const NAMED_MAX: usize = 1 << 16;

fn bounded<K, V>(m: &mut FxHashMap<K, V>) {
    if m.len() >= NAMED_MAX {
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
