//! e-TeX's registers 256..=32767 of each kind (pdfTeX part 53a, "sparse
//! arrays"). They live at locations past eqtb ([`reg_loc`]), so that
//! defining, saving, restoring and tracing them is eqtb's code; a map
//! holds the ones that differ from their default.
//!
//! Saving follows e-TeX: the registers saved at one group level form one
//! chain, which comes back as a whole where the level saved its first one,
//! newest first.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::cmds::{BOX_REF, GLUE_REF, UNDEFINED_CS};
use crate::mem::{MemoryWord, NULL};
use crate::web::{
    BOX_BASE, COUNT_BASE, LEVEL_ONE, MU_SKIP_BASE, SCALED_BASE, SKIP_BASE, TOKS_BASE,
};
use crate::web::{BOX_VAL, DIMEN_VAL, GLUE_VAL, INT_VAL, MU_VAL, TOK_VAL};

/// The first location of a register above 255.
pub(crate) const EXT_BASE: i32 = 0x2000_0000;
const KIND_SPAN: i32 = 0x8000;

/// The location of register `n` of `kind` (`int_val` … `tok_val`, with
/// `box_val` for boxes).
pub(crate) fn reg_loc(kind: i32, n: i32) -> i32 {
    if n < 256 {
        n + match kind {
            INT_VAL => COUNT_BASE,
            DIMEN_VAL => SCALED_BASE,
            GLUE_VAL => SKIP_BASE,
            MU_VAL => MU_SKIP_BASE,
            BOX_VAL => BOX_BASE,
            _ => TOKS_BASE,
        }
    } else {
        EXT_BASE + kind * KIND_SPAN + n
    }
}

/// The kind and number of a location from [`reg_loc`] past eqtb.
pub(crate) fn ext_reg(loc: i32) -> (i32, i32) {
    ((loc - EXT_BASE) / KIND_SPAN, (loc - EXT_BASE) % KIND_SPAN)
}

/// Whether register values of `kind` are words (counts and dimens).
pub(crate) fn is_word_kind(kind: i32) -> bool {
    kind <= DIMEN_VAL
}

/// A saved register: its location, value (the word and the object it
/// holds) and level.
#[derive(Clone, Debug)]
pub(crate) struct Saved {
    pub loc: i32,
    pub word: MemoryWord,
    pub obj: Option<crate::objs::Obj>,
    pub level: i32,
}

partex_engine::persist_struct!(Saved {
    loc,
    word,
    obj,
    level
});

#[derive(Clone, Default)]
pub(crate) struct ExtRegs {
    /// Value and level (the level of a word register; the others keep it
    /// in the word, as eqtb does).
    cells: BTreeMap<i32, (MemoryWord, i32)>,
    /// The objects the token, box and glue registers hold (`objs.rs`).
    objs: BTreeMap<i32, crate::objs::Obj>,
    /// e-TeX's `sa_chain` and `sa_level`: saved at the innermost level
    /// that saved any.
    pub chain: Vec<Saved>,
    pub chain_level: i32,
    /// The chains of outer levels.
    pub outer: Vec<Vec<Saved>>,
    /// The same cells by kind and number, read instead of `cells` when
    /// on (`Params::fast` bit `FAST_XREGS`).
    dense: Dense,
}

partex_engine::persist_struct!(ExtRegs {
    cells,
    objs,
    chain,
    chain_level,
    outer,
    dense
});

/// A register's value and level (as `ExtRegs::cells` holds them).
#[derive(Clone, Copy, Default)]
struct Slot {
    word: MemoryWord,
    level: i32,
}

partex_engine::persist_struct!(Slot { word, level });

/// The registers by kind, each a vector by number up to the highest one
/// set, in shared chunks (a checkpoint's copy shares them): an index
/// where the ordered map walks a tree.
#[derive(Clone, Default)]
struct Dense {
    on: bool,
    kinds: Vec<crate::cow::Chunked<Slot>>,
}

partex_engine::persist_struct!(Dense { on, kinds });

impl ExtRegs {
    /// No register set; `dense`: kept by kind and number too.
    pub fn new(dense: bool) -> Self {
        Self {
            dense: Dense {
                on: dense,
                kinds: Vec::new(),
            },
            ..Self::default()
        }
    }

    /// The slot of `loc` in `dense`, if it has grown to it.
    #[inline]
    fn dense_at(&self, loc: i32) -> Option<&Slot> {
        let (kind, n) = ext_reg(loc);
        let c = self.dense.kinds.get(usize::try_from(kind).ok()?)?;
        let n = usize::try_from(n).ok()?;
        (n < c.len()).then(|| &c[n])
    }

    /// Record cell `loc` in `dense` (grown with defaults up to it).
    fn dense_put(&mut self, loc: i32, c: (MemoryWord, i32)) {
        let (kind, n) = ext_reg(loc);
        let (Ok(kind), Ok(n)) = (usize::try_from(kind), usize::try_from(n)) else {
            return;
        };
        while self.dense.kinds.len() <= kind {
            self.dense.kinds.push(crate::cow::Chunked::default());
        }
        let v = &mut self.dense.kinds[kind];
        // (a kind's registers all start from the same default)
        let (word, level) = Self::default_cell(loc);
        while v.len() <= n {
            v.push(Slot { word, level });
        }
        v[n] = Slot {
            word: c.0,
            level: c.1,
        };
    }

    pub(crate) fn default_cell(loc: i32) -> (MemoryWord, i32) {
        let (kind, _) = ext_reg(loc);
        let mut w = MemoryWord::default();
        match kind {
            INT_VAL | DIMEN_VAL => {}
            GLUE_VAL | MU_VAL => {
                w.set_b0(GLUE_REF);
                w.set_rh(NULL);
            }
            BOX_VAL => {
                w.set_b0(BOX_REF);
                w.set_rh(NULL);
            }
            _ => {
                debug_assert_eq!(kind, TOK_VAL);
                w.set_b0(UNDEFINED_CS);
                w.set_rh(NULL);
            }
        }
        if !is_word_kind(kind) {
            w.set_b1(LEVEL_ONE);
        }
        (w, LEVEL_ONE)
    }

    pub fn get(&self, loc: i32) -> MemoryWord {
        if self.dense.on {
            return self
                .dense_at(loc)
                .map_or_else(|| Self::default_cell(loc).0, |c| c.word);
        }
        self.cells
            .get(&loc)
            .map_or_else(|| Self::default_cell(loc).0, |c| c.0)
    }

    pub fn level(&self, loc: i32) -> i32 {
        if self.dense.on {
            return self.dense_at(loc).map_or(LEVEL_ONE, |c| c.level);
        }
        self.cells.get(&loc).map_or(LEVEL_ONE, |c| c.1)
    }

    fn cell(&mut self, loc: i32) -> &mut (MemoryWord, i32) {
        self.cells
            .entry(loc)
            .or_insert_with(|| Self::default_cell(loc))
    }

    pub fn set(&mut self, loc: i32, w: MemoryWord) {
        let c = self.cell(loc);
        c.0 = w;
        let c = *c;
        if self.dense.on {
            self.dense_put(loc, c);
        }
    }

    pub fn set_level(&mut self, loc: i32, l: i32) {
        let c = self.cell(loc);
        c.1 = l;
        let c = *c;
        if self.dense.on {
            self.dense_put(loc, c);
        }
    }

    /// The object register `loc` holds (none: void, empty, `zero_glue`).
    pub fn obj(&self, loc: i32) -> Option<&crate::objs::Obj> {
        self.objs.get(&loc)
    }

    /// Put object `o` in register `loc`.
    pub fn set_obj(&mut self, loc: i32, o: Option<crate::objs::Obj>) {
        match o {
            Some(o) => {
                self.objs.insert(loc, o);
            }
            None => {
                self.objs.remove(&loc);
            }
        }
    }

    /// The objects the registers hold, by location (for formats).
    pub fn objs(&self) -> impl Iterator<Item = (i32, &crate::objs::Obj)> + '_ {
        self.objs.iter().map(|(&l, o)| (l, o))
    }

    /// All registers that differ from the default (for formats).
    /// The locations of the registers set (to a default value too).
    pub fn locs(&self) -> impl Iterator<Item = i32> + '_ {
        self.cells.keys().copied()
    }

    pub fn cells(&self) -> impl Iterator<Item = (i32, MemoryWord, i32)> + '_ {
        self.cells.iter().map(|(&l, &(w, x))| (l, w, x))
    }
}
