//! Sorting the entries (sortid.c, qsort.c).
//!
//! The comparison counts itself (the `.ilg` reports it), prints progress
//! dots, and marks the second of two identical entries a duplicate, so the
//! sort is qsort.c's algorithm, comparison for comparison.

use core::cmp::Ordering;

use crate::{DUPLICATE, FIELD_MAX, Field, Mk, SYMBOL, at, group_type, tolower};

const THRESH: usize = 4;
const MTHRESH: usize = 6;

/// `strcmp`'s sign.
fn strcmp(a: &[u8], b: &[u8]) -> i32 {
    match a.cmp(b) {
        Ordering::Less => -1,
        Ordering::Equal => 0,
        Ordering::Greater => 1,
    }
}

/// The comparison's settings.
struct Cmp {
    letter_ordering: bool,
    german_sort: bool,
    ropen: u8,
    rclose: u8,
}

impl Cmp {
    /// `compare_string`.
    fn string(&self, a: &[u8], b: &[u8]) -> i32 {
        let (mut i, mut j) = (0, 0);
        while at(a, i) != 0 || at(b, j) != 0 {
            if at(a, i) == 0 {
                return -1;
            }
            if at(b, j) == 0 {
                return 1;
            }
            if self.letter_ordering {
                if at(a, i) == b' ' {
                    i += 1;
                }
                if at(b, j) == b' ' {
                    j += 1;
                }
            }
            let (al, bl) = (i32::from(tolower(at(a, i))), i32::from(tolower(at(b, j))));
            if al != bl {
                return al - bl;
            }
            i += 1;
            j += 1;
        }
        if self.german_sort {
            new_strcmp(a, b)
        } else {
            strcmp(a, b)
        }
    }

    /// `compare_one`.
    fn one(&self, x: &[u8], y: &[u8]) -> i32 {
        match (x.is_empty(), y.is_empty()) {
            (true, true) => return 0,
            (true, false) => return -1,
            (false, true) => return 1,
            _ => {}
        }
        let (m, n) = (group_type(x), group_type(y));
        if m >= 0 && n >= 0 {
            return m.wrapping_sub(n);
        }
        if m >= 0 {
            return if self.german_sort || n == SYMBOL {
                1
            } else {
                -1
            };
        }
        if n >= 0 {
            return if self.german_sort || m == SYMBOL {
                -1
            } else {
                1
            };
        }
        match (m == SYMBOL, n == SYMBOL) {
            (true, true) => check_mixsym(x, y),
            (true, false) => -1,
            (false, true) => 1,
            _ => self.string(x, y),
        }
    }

    /// `compare_page`: marks `b` a duplicate of an identical `a`.
    fn page(&self, a: &Field, b: &mut Field) -> i32 {
        let mut m = 0i32;
        let mut i = 0;
        while i < a.count && i < b.count {
            m = a.npg[i].wrapping_sub(b.npg[i]);
            if m != 0 {
                break;
            }
            i += 1;
        }
        if m == 0 {
            if i == a.count && i == b.count {
                let isrange = |c: u8| c == self.ropen || c == self.rclose;
                let (ea, eb) = (at(&a.encap, 0), at(&b.encap, 0));
                if isrange(ea) && isrange(eb) {
                    m = a.lc - b.lc;
                } else if a.encap == b.encap {
                    if a.typ != DUPLICATE && b.typ != DUPLICATE {
                        b.typ = DUPLICATE;
                    }
                } else if isrange(ea) || isrange(eb) {
                    m = a.lc - b.lc;
                } else {
                    m = self.string(&a.encap, &b.encap);
                }
            } else if i == a.count && i < b.count {
                m = -1;
            } else if i < a.count && i == b.count {
                m = 1;
            }
        }
        m
    }
}

/// `check_mixsym`.
fn check_mixsym(x: &[u8], y: &[u8]) -> i32 {
    match (at(x, 0).is_ascii_digit(), at(y, 0).is_ascii_digit()) {
        (true, false) => 1,
        (false, true) => -1,
        _ => strcmp(x, y),
    }
}

/// `new_strcmp` for German: at the first difference, upper case last.
fn new_strcmp(a: &[u8], b: &[u8]) -> i32 {
    let mut i = 0;
    while at(a, i) == at(b, i) {
        if at(a, i) == 0 {
            return 0;
        }
        i += 1;
    }
    if at(a, i).is_ascii_uppercase() { 1 } else { -1 }
}

impl Mk<'_> {
    /// `compare`, on entries `a` and `b`.
    fn compare(&mut self, cmp: &Cmp, a: usize, b: usize) -> i32 {
        self.idx_gc += 1;
        self.idx_dot_tick(crate::CMP_MAX);
        let (ea, eb) = (&self.entries[a], &self.entries[b]);
        for i in 0..FIELD_MAX {
            let d = cmp.one(&ea.sf[i], &eb.sf[i]);
            if d != 0 {
                return d;
            }
            let d = cmp.one(&ea.af[i], &eb.af[i]);
            if d != 0 {
                return d;
            }
        }
        if a == b {
            let e = self.entries[a].clone();
            return cmp.page(&e, &mut self.entries[b]);
        }
        let (lo, hi) = self.entries.split_at_mut(a.max(b));
        let (fa, fb) = if a < b {
            (&lo[a], &mut hi[0])
        } else {
            // (b < a: `b` in the lower half)
            let (x, y) = (&hi[0], &mut lo[b]);
            (x, y)
        };
        cmp.page(fa, fb)
    }

    /// `sort_idx`.
    pub(crate) fn sort_idx(&mut self) {
        self.message("Sorting entries...", &[]);
        self.idx_dc = 0;
        self.idx_gc = 0;
        let cmp = Cmp {
            letter_ordering: self.letter_ordering,
            german_sort: self.german_sort,
            ropen: self.st.idx_ropen,
            rclose: self.st.idx_rclose,
        };
        let mut v = core::mem::take(&mut self.idx_key);
        self.qqsort(&cmp, &mut v);
        self.idx_key = v;
        let gc = self.idx_gc;
        self.message("done (%ld comparisons).\n", &[crate::P::I(gc)]);
    }

    /// `qqsort`: quicksort down to `THRESH`, then insertion sort.
    fn qqsort(&mut self, cmp: &Cmp, v: &mut [usize]) {
        let n = v.len();
        if n <= 1 {
            return;
        }
        let hi = if n >= THRESH {
            self.qst(cmp, v, 0, n);
            THRESH
        } else {
            n
        };
        // the smallest of the first `hi` first, as a sentinel
        let mut j = 0;
        for lo in 1..hi {
            if self.compare(cmp, v[j], v[lo]) > 0 {
                j = lo;
            }
        }
        if j != 0 {
            v.swap(0, j);
        }
        for min in 1..n {
            let mut h = min;
            loop {
                h -= 1;
                if self.compare(cmp, v[h], v[min]) <= 0 {
                    h += 1;
                    break;
                }
                if h == 0 {
                    break;
                }
            }
            if h != min {
                v[h..=min].rotate_right(1);
            }
        }
    }

    /// `qst`: sort `v[base..max]`, leaving partitions under `THRESH`.
    fn qst(&mut self, cmp: &Cmp, v: &mut [usize], mut base: usize, mut max: usize) {
        let mut lo = max - base;
        loop {
            let mut mid = base + (lo >> 1);
            let mut i = mid;
            if lo >= MTHRESH {
                let jj = base;
                let mut j = if self.compare(cmp, v[jj], v[i]) > 0 {
                    jj
                } else {
                    i
                };
                let tmp = max - 1;
                if self.compare(cmp, v[j], v[tmp]) > 0 {
                    j = if j == jj { i } else { jj };
                    if self.compare(cmp, v[j], v[tmp]) < 0 {
                        j = tmp;
                    }
                }
                if j != i {
                    v.swap(i, j);
                }
            }
            i = base;
            let mut j = max - 1;
            loop {
                while i < mid && self.compare(cmp, v[i], v[mid]) <= 0 {
                    i += 1;
                }
                let mut swap = None;
                while j > mid {
                    if self.compare(cmp, v[mid], v[j]) <= 0 {
                        j -= 1;
                        continue;
                    }
                    let tmp = i + 1;
                    let jj;
                    if i == mid {
                        mid = j;
                        jj = j;
                    } else {
                        jj = j;
                        j -= 1;
                    }
                    swap = Some((jj, tmp));
                    break;
                }
                let (jj, tmp) = match swap {
                    Some(s) => s,
                    None => {
                        if i == mid {
                            break;
                        }
                        let jj = mid;
                        mid = i;
                        j -= 1;
                        (jj, i)
                    }
                };
                v.swap(i, jj);
                i = tmp;
            }
            j = mid;
            i = mid + 1;
            let lo_n = j - base;
            let hi_n = max - i;
            if lo_n <= hi_n {
                if lo_n >= THRESH {
                    self.qst(cmp, v, base, j);
                }
                base = i;
                lo = hi_n;
            } else {
                if hi_n >= THRESH {
                    self.qst(cmp, v, i, max);
                }
                max = j;
                lo = lo_n;
            }
            if lo < THRESH {
                break;
            }
        }
    }
}
