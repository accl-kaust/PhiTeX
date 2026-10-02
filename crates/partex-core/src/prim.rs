//! pdfTeX §275–§283: the primitive table behind `\pdfprimitive`, a second
//! hash of every primitive's name with its original meaning in
//! `eqtb[prim_eqtb_base+p]`.

use alloc::collections::BTreeMap;
use alloc::vec;
use alloc::vec::Vec;

use crate::host::Host;
use crate::tex::{Jump, Tex};
use crate::track::Tracker;
use crate::web::{
    BIGGEST_CHAR, CS_TOKEN_FLAG, FROZEN_RELAX, HASH_BASE, PRIM_BASE, PRIM_EQTB_BASE, PRIM_PRIME,
    PRIM_SIZE, SINGLE_BASE, UNDEFINED_PRIMITIVE,
};

/// pdfTeX §275: `prim_next` and `prim_text` (string number plus one; 0 for
/// an empty slot) per slot, and the allocation pointer `prim_used`.
#[derive(Clone, Debug)]
pub(crate) struct PrimTable {
    /// (shared by checkpoints: primitives are only made by INITEX)
    pub(crate) next: alloc::sync::Arc<Vec<i32>>,
    pub(crate) text: alloc::sync::Arc<Vec<i32>>,
    pub(crate) used: i32,
    /// For `prim_name`: the first slot with each original meaning, built
    /// on demand and dropped when a meaning changes (a scan of every slot
    /// per `\meaning` took a tenth of the PGF manual's time).
    /// (shared by checkpoints: a clone is a reference count, DESIGN.md
    /// §7.16.3)
    pub(crate) by_meaning: Option<alloc::sync::Arc<BTreeMap<(i32, i32), i32>>>,
}

partex_engine::persist_struct!(PrimTable {
    next,
    text,
    used,
    by_meaning
});

impl core::hash::Hash for PrimTable {
    /// The table without its derived index.
    fn hash<S: core::hash::Hasher>(&self, h: &mut S) {
        (&self.next, &self.text, self.used).hash(h);
    }
}

impl Default for PrimTable {
    /// pdfTeX §276–§277: empty, and nothing is used.
    fn default() -> Self {
        let n = usize::try_from(PRIM_SIZE).unwrap_or(0) + 1;
        Self {
            next: alloc::sync::Arc::new(vec![0; n]),
            text: alloc::sync::Arc::new(vec![0; n]),
            used: PRIM_SIZE,
            by_meaning: None,
        }
    }
}

#[inline]
fn ix(p: i32) -> usize {
    usize::try_from(p).expect("primitive table index")
}

impl<H: Host, T: Tracker> Tex<H, T> {
    /// pdfTeX §281: search the primitive table for string `s` (which may be
    /// the string being built, `s = str_ptr`), inserting it unless
    /// `no_new_control_sequence`.
    pub(crate) fn prim_lookup(&mut self, s: i32) -> Result<i32, Jump> {
        let su = usize::try_from(s).unwrap_or(0);
        let name: Vec<u8>;
        let mut p;
        if s <= BIGGEST_CHAR {
            if s < 0 {
                return Ok(UNDEFINED_PRIMITIVE);
            }
            name = Vec::new();
            p = (s % PRIM_PRIME) + PRIM_BASE; // we start searching here
        } else {
            let j = self.str_start[su];
            let end = if su == self.str_ptr {
                self.pool_ptr
            } else {
                self.str_start[su + 1]
            };
            name = self.str_pool[j..end].to_vec();
            // pdfTeX §283: compute the primitive code `h`.
            let mut h = i32::from(name[0]);
            for &c in &name[1..] {
                h = h + h + i32::from(c);
                while h >= PRIM_PRIME {
                    h -= PRIM_PRIME;
                }
            }
            p = h + PRIM_BASE; // `0<=h<prim_prime`
        }
        loop {
            let t = self.prims.text[ix(p)];
            if t > 1 + BIGGEST_CHAR {
                // `p` points to a multi-letter primitive
                if self.str_bytes(ix(t - 1)) == name.as_slice() {
                    return Ok(p);
                }
            } else if t == 1 + s {
                return Ok(p); // a single-letter primitive
            }
            if self.prims.next[ix(p)] == 0 {
                if self.no_new_control_sequence {
                    return Ok(UNDEFINED_PRIMITIVE);
                }
                // pdfTeX §282: insert a new primitive after `p`.
                if self.prims.text[ix(p)] > 0 {
                    loop {
                        if self.prims.used == PRIM_BASE {
                            return self.overflow(b"primitive size", PRIM_SIZE);
                        }
                        self.prims.used -= 1;
                        if self.prims.text[ix(self.prims.used)] == 0 {
                            break;
                        }
                    }
                    alloc::sync::Arc::make_mut(&mut self.prims.next)[ix(p)] = self.prims.used;
                    p = self.prims.used;
                }
                alloc::sync::Arc::make_mut(&mut self.prims.text)[ix(p)] = s + 1;
                return Ok(p);
            }
            p = self.prims.next[ix(p)];
        }
    }
}

impl<H: Host, T: Tracker> Tex<H, T> {
    /// The primitive table's entry for the name of `cur_cs` (pdfTeX:
    /// `prim_lookup(text(cur_cs))`, or of the character of a one-letter
    /// name); `undefined_primitive` for none.
    pub(crate) fn prim_of_cur_cs(&mut self) -> Result<i32, Jump> {
        let s = if self.cur_cs < HASH_BASE {
            self.cur_cs - SINGLE_BASE
        } else {
            self.text(self.cur_cs)
        };
        self.prim_lookup(s)
    }

    /// pdfTeX: `prim_eq_type(p)` and `prim_equiv(p)`, the primitive
    /// meaning of entry `p`.
    pub(crate) fn prim_meaning(&self, p: i32) -> (i32, i32) {
        (
            self.eq_type(PRIM_EQTB_BASE + p),
            self.equiv(PRIM_EQTB_BASE + p),
        )
    }

    /// pdfTeX §395: reset `cur_tok` for unexpandable primitives: the next
    /// token with its primitive meaning (`\relax` if it has none).
    pub(crate) fn reset_primitive_tok(&mut self) -> Result<(), Jump> {
        self.get_token()?;
        let p = self.prim_of_cur_cs()?;
        if p == UNDEFINED_PRIMITIVE {
            self.cur_cmd = crate::cmds::RELAX;
            self.cur_chr = 0;
            self.cur_tok = CS_TOKEN_FLAG + FROZEN_RELAX;
            self.cur_cs = FROZEN_RELAX;
        } else {
            (self.cur_cmd, self.cur_chr) = self.prim_meaning(p);
            self.cur_cs = PRIM_EQTB_BASE + p;
            self.cur_tok = CS_TOKEN_FLAG + self.cur_cs;
        }
        Ok(())
    }

    /// The name (a string number, or a character code for a one-letter
    /// name) of the primitive whose original meaning is `cmd`, `chr`.
    pub(crate) fn prim_name(&mut self, cmd: i32, chr: i32) -> Option<i32> {
        if self.prims.by_meaning.is_none() {
            let mut m = BTreeMap::new();
            for p in 0..=PRIM_SIZE {
                if self.prims.text[ix(p)] != 0 {
                    m.entry(self.prim_meaning(p)).or_insert(p);
                }
            }
            self.prims.by_meaning = Some(alloc::sync::Arc::new(m));
        }
        let p = *self.prims.by_meaning.as_ref()?.get(&(cmd, chr))?;
        Some(self.prims.text[ix(p)] - 1)
    }
}
