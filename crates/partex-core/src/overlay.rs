//! Moving eqtb cells from one state of a job into another (read-set
//! cutoff, DESIGN.md §7.0): a checkpoint of the previous build that
//! differs from a rebuild only in cells the rest of the job never reads
//! takes the rebuild's values for them, and the rebuild goes on as the
//! previous build did.
//!
//! A cell that names something (a macro's or token register's list,
//! glue, a shape, a box) gets a copy of it in this state's stores. The
//! caller checks the result: the state hashes of the two states must
//! then be equal, which also covers what the cells do not hold (the hash
//! table, the save stack, sharing between lists).

use alloc::collections::BTreeSet;
use alloc::vec::Vec;

use crate::host::Host;
use crate::tex::Tex;
use crate::track::Tracker;
use crate::web::*;

/// How one PDF font's characters used differ ([`Tex::chars_differ`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CharsDiffer {
    pub font: usize,
    pub added: [u64; 4],
    pub removed: [u64; 4],
}

/// Names two states give differently ([`Tex::name_differences`]).
#[derive(Clone, Debug, Default)]
pub struct Names {
    /// Hash slots (whose names or links differ).
    pub slots: Vec<i32>,
    /// The slots whose names differ (`Cell::Hash`).
    pub texts: Vec<i32>,
    /// The slots whose links differ (`Cell::HashNext`).
    pub links: Vec<i32>,
    /// Strings whose characters differ.
    pub strings: Vec<usize>,
    /// The cells a search for their characters reads (`Cell::Str`).
    pub searched: Vec<crate::track::Cell>,
    /// The number of strings then.
    pub str_ptr: usize,
}

impl<H: Host, T: Tracker> Tex<H, T> {
    /// Give eqtb locations `cells` their words (and levels) in `from`:
    /// eqtb proper and the control sequences past it (`hash_extra`'s).
    /// False, with nothing changed, if a location is elsewhere (e-TeX's
    /// registers above 255 live in their own table).
    pub fn import_cells(&mut self, from: &Self, cells: &[i32]) -> bool {
        let top = i32::try_from(self.eqtb.len().min(from.eqtb.len())).unwrap_or(0);
        if cells
            .iter()
            .any(|&p| p < ACTIVE_BASE || p >= top || p >= crate::xregs::EXT_BASE)
        {
            return false;
        }
        // (the values themselves, shared: a list several cells share in
        // `from` is shared here too)
        for &p in cells {
            let w = from.peek_eqtb(p);
            let o = from.peek_obj(p).cloned();
            self.set_eqtb_entry(p, w, o);
            if (INT_BASE..=EQTB_SIZE).contains(&p) {
                self.set_xeq_level(p, from.peek_xeq_level(p));
            }
        }
        true
    }

    /// The names two states of a job give differently (a `\label` renamed
    /// puts another control sequence's name in the `.aux` file): the hash
    /// slots whose words or whose strings' characters differ, and the
    /// strings whose characters differ, with the cells a search for their
    /// characters in either state reads. `None` if the tables differ
    /// otherwise (their sizes, the overflow area's use, the number of
    /// strings), or if a string that differs is not only a control
    /// sequence's name: one that a file name, a font or the input stack
    /// also holds is refused (its characters would be read untracked).
    #[must_use]
    pub fn name_differences(&self, from: &Self) -> Option<Names> {
        if self.hash.len() != from.hash.len()
            || (self.hash_used, self.hash_top, self.hash_high)
                != (from.hash_used, from.hash_top, from.hash_high)
            || self.str_ptr != from.str_ptr
            || (self.init_str_ptr, self.init_pool_ptr) != (from.init_str_ptr, from.init_pool_ptr)
        {
            return None;
        }
        let (pa, sa) = self.strings_now();
        let (pb, sb) = from.strings_now();
        let strings: Vec<usize> = (self.init_str_ptr..self.str_ptr)
            .filter(|&i| pa[sa[i]..sa[i + 1]] != pb[sb[i]..sb[i + 1]])
            .collect();
        let differ: BTreeSet<i32> = strings
            .iter()
            .filter_map(|&i| i32::try_from(i).ok())
            .collect();
        let (mut texts, mut links) = (BTreeSet::new(), BTreeSet::new());
        for i in self
            .hash
            .differences(&from.hash, |a, b| a.bits() == b.bits())
        {
            let p = i32::try_from(i).ok()? + HASH_BASE;
            let (a, b) = (self.hash[i], from.hash[i]);
            if a.rh() != b.rh() {
                texts.insert(p);
            }
            if a.lh() != b.lh() {
                links.insert(p);
            }
        }
        if !differ.is_empty() {
            // each string that differs is some slot's name in both states
            // and nothing else's
            let (mut named_a, mut named_b) = (BTreeSet::new(), BTreeSet::new());
            for (i, (a, b)) in self
                .hash
                .slices()
                .flatten()
                .zip(from.hash.slices().flatten())
                .enumerate()
            {
                let (ta, tb) = (a.rh(), b.rh());
                if differ.contains(&ta) || differ.contains(&tb) {
                    texts.insert(i32::try_from(i).ok()? + HASH_BASE);
                    named_a.insert(ta);
                    named_b.insert(tb);
                }
            }
            if !differ
                .iter()
                .all(|s| named_a.contains(s) && named_b.contains(s))
            {
                return None;
            }
            for t in [self, from] {
                if t.string_holders().any(|s| differ.contains(&s)) {
                    return None;
                }
            }
        }
        // (a lookup of a name reads its characters' cell: the names of the
        // slots that differ, in either state, and the strings that differ)
        let chars = |pool: &[u8], start: &[usize], s: usize| {
            crate::strings::str_cell(&pool[start[s]..start[s + 1]])
        };
        let named = |t: &Self, p: i32| {
            let w = t.hash[usize::try_from(p - HASH_BASE).unwrap_or(0)].rh();
            usize::try_from(w).ok().filter(|&s| s > 0 && s < t.str_ptr)
        };
        let mut searched: Vec<crate::track::Cell> = strings
            .iter()
            .flat_map(|&i| [chars(&pa, &sa, i), chars(&pb, &sb, i)])
            .collect();
        for &p in &texts {
            searched.extend(named(self, p).map(|s| chars(&pa, &sa, s)));
            searched.extend(named(from, p).map(|s| chars(&pb, &sb, s)));
        }
        searched.sort_unstable();
        searched.dedup();
        Some(Names {
            slots: texts.union(&links).copied().collect(),
            texts: texts.into_iter().collect(),
            links: links.into_iter().collect(),
            strings,
            searched,
            str_ptr: self.str_ptr,
        })
    }

    /// The strings' characters and starts, flat.
    fn strings_now(&self) -> (Vec<u8>, Vec<usize>) {
        (
            self.str_pool
                .prefix(self.pool_ptr)
                .iter()
                .copied()
                .collect(),
            self.str_start
                .prefix(self.str_ptr + 1)
                .iter()
                .copied()
                .collect(),
        )
    }

    /// String numbers held outside the hash table: file names, fonts'
    /// names, the input stack's names.
    fn string_holders(&self) -> impl Iterator<Item = i32> + '_ {
        let n = self.fonts.name.len();
        [
            self.cur_name,
            self.cur_area,
            self.cur_ext,
            self.job_name,
            self.output_file_name,
            self.log_name,
        ]
        .into_iter()
        .chain(self.source_filename_stack.iter().copied())
        .chain(self.full_source_filename_stack.iter().copied())
        .chain(self.fonts.name.iter().take(n).copied())
        .chain(self.fonts.area.iter().take(n).copied())
        .chain(
            self.input_stack[..=self.input_ptr]
                .iter()
                .map(|r| r.name)
                .collect::<Vec<_>>(),
        )
        .chain(core::iter::once(self.cur_input.name))
    }

    /// Take hash slots `slots` from `from`, and the strings below `n`
    /// (`from`'s number of strings when the two states met), keeping this
    /// state's later strings, moved: a later checkpoint of the previous
    /// build then names things as the rebuild does. False, with nothing
    /// changed, if a string below `n` other than `strings` (the ones that
    /// differ) is not the same here (this build removed and made strings
    /// since).
    pub fn import_names(&mut self, from: &Self, names: &Names) -> bool {
        let (slots, n, strings) = (&names.slots, names.str_ptr, &names.strings);
        if self.str_ptr < n
            || from.str_ptr < n
            || (self.init_str_ptr, self.init_pool_ptr) != (from.init_str_ptr, from.init_pool_ptr)
            || self.hash.len() != from.hash.len()
        {
            return false;
        }
        if slots
            .iter()
            .any(|&p| usize::try_from(p - HASH_BASE).map_or(true, |i| i >= self.hash.len()))
        {
            return false;
        }
        let (pa, sa) = self.strings_now();
        let (pb, sb) = from.strings_now();
        let differ: BTreeSet<usize> = strings.iter().copied().collect();
        if (self.init_str_ptr..n)
            .any(|i| !differ.contains(&i) && pa[sa[i]..sa[i + 1]] != pb[sb[i]..sb[i + 1]])
        {
            return false;
        }
        if !strings.is_empty() {
            let mut pool = pb[..sb[n]].to_vec();
            pool.extend_from_slice(&pa[sa[n]..]);
            let mut starts = sb[..=n].to_vec();
            starts.extend(sa[n + 1..].iter().map(|&x| x - sa[n] + sb[n]));
            self.pool_ptr = pool.len();
            self.str_pool.thaw();
            *self.str_pool = pool;
            self.str_pool.lower_floor(0);
            self.str_start.thaw();
            *self.str_start = starts;
            self.str_start.lower_floor(0);
            self.str_index.clear();
        }
        for &p in slots {
            let i = usize::try_from(p - HASH_BASE).unwrap_or(0);
            self.hash[i] = from.hash[i];
        }
        true
    }

    /// How the characters of PDF fonts this state has used differ from
    /// `old`'s, by font: those it has and `old` has not, and those `old` has
    /// and it has not. (The fonts' subsets, read only when the fonts are
    /// written at the end, and only ever added to.) `None` if the font
    /// tables differ.
    #[must_use]
    pub fn chars_differ(&self, old: &Self) -> Option<Vec<CharsDiffer>> {
        let (a, b) = (&self.pdf.ship.fonts, &old.pdf.ship.fonts);
        if a.len() != b.len() {
            return None;
        }
        Some(
            a.iter()
                .zip(b)
                .enumerate()
                .filter(|(_, (x, y))| x.chars != y.chars)
                .map(|(font, (x, y))| CharsDiffer {
                    font,
                    added: core::array::from_fn(|k| x.chars[k] & !y.chars[k]),
                    removed: core::array::from_fn(|k| y.chars[k] & !x.chars[k]),
                })
                .collect(),
        )
    }

    /// Change PDF fonts' subsets by `differ` ([`Tex::chars_differ`]).
    pub fn import_chars(&mut self, differ: &[CharsDiffer]) -> bool {
        if differ.iter().any(|d| d.font >= self.pdf.ship.fonts.len()) {
            return false;
        }
        for d in differ {
            let c = &mut self.pdf.ship.fonts[d.font].chars;
            for (k, w) in c.iter_mut().enumerate() {
                *w = (*w | d.added[k]) & !d.removed[k];
            }
        }
        true
    }

    /// How the objects waiting in an open PDF object stream differ from
    /// `old`'s, if only their bytes do (output not yet written).
    #[must_use]
    pub fn objstm_differ(&self, old: &Self) -> Option<crate::pdf::out::ObjStmDiffer> {
        self.pdf.out.objstm_differ(&old.pdf.out)
    }

    /// Whether this later state of the previous build can take `d` (the
    /// object stream was not written since).
    #[must_use]
    pub fn objstm_fits(&self, d: &crate::pdf::out::ObjStmDiffer) -> bool {
        self.pdf.out.objstm_fits(d)
    }

    /// Give this later state of the previous build the rebuild's waiting
    /// objects ([`Tex::objstm_differ`]).
    pub fn import_objstm(&mut self, d: &crate::pdf::out::ObjStmDiffer) -> bool {
        self.pdf.out.import_objstm(d)
    }
}
