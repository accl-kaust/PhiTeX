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
    "pdf:fold",
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
    "pdf:xpages",
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

/// The fixed locations of a run's state table ([`crate::runstate`]):
/// the parameters known by name ([`Par`]), the category codes of
/// characters 0–255, registers 0–255 of each kind (count, dimen, skip,
/// muskip, toks), the code tables `lc`, `uc`, `sf`, `mc`, `dc` of
/// characters 0–255, box registers 0–255, and the state cells with fixed
/// names. A location's field is fixed for the build. After them, the
/// meanings: active characters 0–255 ([`MEAN_ACTIVE`]), then one per
/// interned control sequence ([`MEAN_BASE`] + [`crate::val::CsId::index`]).
pub(crate) const CAT0: usize = PARAMS.len();
pub(crate) const REG0: usize = CAT0 + 256;
/// The code tables (`lc`, `uc`, `sf`, `mc`, `dc`), 256 each.
pub(crate) const CODE0: usize = REG0 + 5 * 256;
const CODES: [&str; 5] = ["lc", "uc", "sf", "mc", "dc"];
/// Box registers 0–255.
pub(crate) const BOX0: usize = CODE0 + 5 * 256;
/// The state cells with fixed names ([`STATIC`]).
pub(crate) const STATIC0: usize = BOX0 + 256;
/// The number of fixed locations (the old name: slots).
pub(crate) const NSLOT: usize = STATIC0 + STATIC.len();
/// The index of `n` in [`STATIC`] (at compile time).
const fn static_index(n: &str) -> usize {
    let n = n.as_bytes();
    let mut i = 0;
    while i < STATIC.len() {
        let s = STATIC[i].as_bytes();
        if s.len() == n.len() {
            let mut k = 0;
            while k < n.len() && s[k] == n[k] {
                k += 1;
            }
            if k == n.len() {
                return i;
            }
        }
        i += 1;
    }
    panic!("not a state cell")
}

/// The locations of state cells a command reads on its path.
#[allow(clippy::cast_possible_truncation, dead_code)]
pub(crate) mod cell {
    use super::{BOX0, STATIC0, static_index};
    /// Box register 255.
    pub(crate) const BOX255: u32 = (BOX0 + 255) as u32;
    pub(crate) const AFTERTOKEN: u32 = (STATIC0 + static_index("aftertoken")) as u32;
    pub(crate) const ALIGN_OPEN: u32 = (STATIC0 + static_index("align:open")) as u32;
    pub(crate) const ALIGN_STATE: u32 = (STATIC0 + static_index("align:state")) as u32;
    pub(crate) const BADNESS: u32 = (STATIC0 + static_index("badness")) as u32;
    pub(crate) const DEADCYCLES: u32 = (STATIC0 + static_index("deadcycles")) as u32;
    pub(crate) const DIAGS: u32 = (STATIC0 + static_index("diags")) as u32;
    pub(crate) const DVICOMMENT: u32 = (STATIC0 + static_index("dvicomment")) as u32;
    pub(crate) const FONT: u32 = (STATIC0 + static_index("font")) as u32;
    pub(crate) const FONTCOUNT: u32 = (STATIC0 + static_index("fontcount")) as u32;
    pub(crate) const IFSTACK: u32 = (STATIC0 + static_index("ifstack")) as u32;
    pub(crate) const INSPEN: u32 = (STATIC0 + static_index("inspen")) as u32;
    pub(crate) const INTERACTION: u32 = (STATIC0 + static_index("interaction")) as u32;
    pub(crate) const ITEMS: u32 = (STATIC0 + static_index("items")) as u32;
    pub(crate) const JOBNAME: u32 = (STATIC0 + static_index("jobname")) as u32;
    pub(crate) const LINES: u32 = (STATIC0 + static_index("lines")) as u32;
    pub(crate) const LOG: u32 = (STATIC0 + static_index("log")) as u32;
    pub(crate) const MARK_BOT: u32 = (STATIC0 + static_index("mark:bot")) as u32;
    pub(crate) const MARK_TOP: u32 = (STATIC0 + static_index("mark:top")) as u32;
    pub(crate) const MARKCLASSES: u32 = (STATIC0 + static_index("markclasses")) as u32;
    pub(crate) const NEEDS: u32 = (STATIC0 + static_index("needs")) as u32;
    pub(crate) const PAGEHASH: u32 = (STATIC0 + static_index("pagehash")) as u32;
    pub(crate) const PAGES: u32 = (STATIC0 + static_index("pages")) as u32;
    pub(crate) const PARSHAPE: u32 = (STATIC0 + static_index("parshape")) as u32;
    pub(crate) const PARTOKEN: u32 = (STATIC0 + static_index("partoken")) as u32;
    pub(crate) const PATTERNS: u32 = (STATIC0 + static_index("patterns")) as u32;
    pub(crate) const PDF_CARRY: u32 = (STATIC0 + static_index("pdf:carry")) as u32;
    pub(crate) const PDF_COLORSTACKS: u32 = (STATIC0 + static_index("pdf:colorstacks")) as u32;
    pub(crate) const PDF_EXACT: u32 = (STATIC0 + static_index("pdf:exact")) as u32;
    pub(crate) const PDF_FIXED: u32 = (STATIC0 + static_index("pdf:fixed")) as u32;
    pub(crate) const PDF_FOLD: u32 = (STATIC0 + static_index("pdf:fold")) as u32;
    pub(crate) const PDF_FONTCOUNT: u32 = (STATIC0 + static_index("pdf:fontcount")) as u32;
    pub(crate) const PDF_LASTANNOT: u32 = (STATIC0 + static_index("pdf:lastannot")) as u32;
    pub(crate) const PDF_LASTLINK: u32 = (STATIC0 + static_index("pdf:lastlink")) as u32;
    pub(crate) const PDF_LASTOBJ: u32 = (STATIC0 + static_index("pdf:lastobj")) as u32;
    pub(crate) const PDF_LASTPH: u32 = (STATIC0 + static_index("pdf:lastph")) as u32;
    pub(crate) const PDF_LASTXFORM: u32 = (STATIC0 + static_index("pdf:lastxform")) as u32;
    pub(crate) const PDF_LASTXIMAGE: u32 = (STATIC0 + static_index("pdf:lastximage")) as u32;
    pub(crate) const PDF_LASTXIMAGECOLORDEPTH: u32 =
        (STATIC0 + static_index("pdf:lastximagecolordepth")) as u32;
    pub(crate) const PDF_LASTXIMAGEPAGES: u32 =
        (STATIC0 + static_index("pdf:lastximagepages")) as u32;
    pub(crate) const PDF_LASTXPOS: u32 = (STATIC0 + static_index("pdf:lastxpos")) as u32;
    pub(crate) const PDF_LASTYPOS: u32 = (STATIC0 + static_index("pdf:lastypos")) as u32;
    pub(crate) const PDF_MAPS: u32 = (STATIC0 + static_index("pdf:maps")) as u32;
    pub(crate) const PDF_MATCH: u32 = (STATIC0 + static_index("pdf:match")) as u32;
    pub(crate) const PDF_OBJNUM: u32 = (STATIC0 + static_index("pdf:objnum")) as u32;
    pub(crate) const PDF_OPENACTION: u32 = (STATIC0 + static_index("pdf:openaction")) as u32;
    pub(crate) const PDF_PIECES: u32 = (STATIC0 + static_index("pdf:pieces")) as u32;
    pub(crate) const PDF_RANDOM: u32 = (STATIC0 + static_index("pdf:random")) as u32;
    pub(crate) const PDF_RELAXED: u32 = (STATIC0 + static_index("pdf:relaxed")) as u32;
    pub(crate) const PDF_RETVAL: u32 = (STATIC0 + static_index("pdf:retval")) as u32;
    pub(crate) const PDF_SPACEFONT: u32 = (STATIC0 + static_index("pdf:spacefont")) as u32;
    pub(crate) const PDF_STATE: u32 = (STATIC0 + static_index("pdf:state")) as u32;
    pub(crate) const PDF_TRAILERID: u32 = (STATIC0 + static_index("pdf:trailerid")) as u32;
    pub(crate) const PDF_XFORMCOUNT: u32 = (STATIC0 + static_index("pdf:xformcount")) as u32;
    pub(crate) const PDFDATE: u32 = (STATIC0 + static_index("pdfdate")) as u32;
    pub(crate) const STREAMS: u32 = (STATIC0 + static_index("streams")) as u32;
    pub(crate) const WRITES: u32 = (STATIC0 + static_index("writes")) as u32;
}

/// The meanings of active characters 0–255.
pub(crate) const MEAN_ACTIVE: usize = NSLOT;
/// The meanings of control sequences, by number.
pub(crate) const MEAN_BASE: usize = MEAN_ACTIVE + 256;

/// The location of the meaning of token `t`, if it has one below the
/// spill (a control sequence, an active character below 256).
#[inline]
pub(crate) fn mean_loc(t: Tok) -> Option<usize> {
    if let Some(i) = t.cs_index() {
        return Some(MEAN_BASE + i);
    }
    (t.cat() == Some(13) && t.chr() < 256).then(|| MEAN_ACTIVE + t.chr() as usize)
}

/// The location of character `c`'s entry in code table `table` (0 `lc`,
/// 1 `uc`, 2 `sf`, 3 `mc`, 4 `dc`), if `c` is below 256.
pub(crate) fn code_slot(table: usize, c: u32) -> Option<usize> {
    (c < 256).then(|| CODE0 + table * 256 + c as usize)
}

/// The handles of one build's fields.
pub(crate) struct Fields {
    names: FxHashMap<&'static str, Field>,
    params: FxHashMap<&'static str, Field>,
    /// The field of each fixed location.
    slot_field: Vec<Field>,
    /// The location of a field that has a fixed one.
    field_slot: FxHashMap<Field, u32>,
    /// The location of a parameter by its name.
    par_slot: FxHashMap<&'static str, u32>,
    /// The location of a parameter by its assignment command and code.
    qpar: FxHashMap<(u8, i32), u32>,
}

impl Fields {
    pub(crate) fn new(v: &Values) -> Fields {
        let names: FxHashMap<&'static str, Field> =
            STATIC.iter().map(|&n| (n, v.field(n))).collect();
        let params: FxHashMap<&'static str, Field> = PARAMS
            .iter()
            .map(|&n| (n, v.field(&format!("param:{n}"))))
            .collect();
        let mut slot_field: Vec<Field> = PARAMS.iter().map(|n| params[n]).collect();
        slot_field.extend((0..256).map(|c| v.field(&format!("cat:{c}"))));
        for reg in REGS {
            slot_field.extend((0..256).map(|n| v.field(&format!("{reg}:{n}"))));
        }
        for t in CODES {
            slot_field.extend((0..256).map(|n| v.field(&format!("{t}:{n}"))));
        }
        slot_field.extend((0..256).map(|n| v.field(&format!("box:{n}"))));
        slot_field.extend(STATIC.iter().map(|n| names[n]));
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

    /// The location of field `f` below the spill: a fixed one, or a
    /// meaning's (kept per thread at `epoch`; a name is looked at once an
    /// epoch). `None`: the field is in no fixed range.
    pub(crate) fn loc_of(&self, v: &Values, epoch: u64, f: Field) -> Option<usize> {
        if let Some(&s) = self.field_slot.get(&f) {
            return Some(s as usize);
        }
        if let Some(l) = with_names(v, epoch, |c| c.locs.get(&f).copied()) {
            return l.map(|l| l as usize);
        }
        let name = crate::val::field_name(v, f);
        let l = if let Some(n) = name.strip_prefix("cs:") {
            Some(MEAN_BASE + crate::val::CsId::intern(n).index())
        } else if let Some(c) = name.strip_prefix("act:") {
            c.parse::<u32>()
                .ok()
                .filter(|&c| c < 256)
                .map(|c| MEAN_ACTIVE + c as usize)
        } else {
            None
        };
        let l32 = l.and_then(|l| u32::try_from(l).ok());
        with_names(v, epoch, |c| {
            bounded(&mut c.locs);
            c.locs.insert(f, l32);
        });
        l
    }

    /// The field of location `loc` below the spill ([`Self::loc_of`]).
    pub(crate) fn field_of(&self, v: &Values, epoch: u64, loc: usize) -> Field {
        if loc < NSLOT {
            return self.slot_field[loc];
        }
        let t = if loc >= MEAN_BASE {
            Tok::of_cs(crate::val::CsId::from_index(loc - MEAN_BASE))
        } else {
            Tok::ch(13, u32::try_from(loc - MEAN_ACTIVE).unwrap_or(0))
        };
        let f = self
            .meaning_at(v, epoch, t)
            .unwrap_or_else(|| v.field("cs:"));
        with_names(v, epoch, |c| {
            bounded(&mut c.locs);
            c.locs.insert(f, u32::try_from(loc).ok());
        });
        f
    }

    /// Every handle held here: the engine must keep them alive across
    /// its value sweeps (they are held outside the graph).
    pub(crate) fn handles(&self) -> impl Iterator<Item = Field> + '_ {
        self.slot_field.iter().copied()
    }

    /// The field of slot `s`.
    pub(crate) fn slot_field(&self, s: usize) -> Field {
        self.slot_field[s]
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
    pub(crate) fn meaning_at(&self, v: &Values, epoch: u64, t: Tok) -> Option<Field> {
        if let Some(f) = with_names(v, epoch, |c| c.toks.get(&t).copied()) {
            return Some(f);
        }
        let f = v.field(&t.field_name()?);
        with_names(v, epoch, |c| {
            bounded(&mut c.toks);
            c.toks.insert(t, f);
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
    /// The locations of fields that have one below the spill (`None`:
    /// the field has none), [`Fields::loc_of`].
    locs: FxHashMap<Field, Option<u32>>,
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
