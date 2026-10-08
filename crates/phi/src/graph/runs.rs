//! A sorted list kept in runs (DESIGN 7.5): blocks of at most `2 RB`
//! entries, their lengths in a Fenwick tree (so a block's start index is
//! a prefix sum). Finding a place is a binary search over the blocks then
//! inside one; inserting or removing moves at most a block's entries and
//! updates O(log B) sums, so a name read or defined a million times costs
//! O(log R + RB) a change, not O(R). (A block split, or a middle block
//! emptied, rebuilds the sums: O(B), after at least `RB` changes to that
//! block.)

/// Entries a block holds at most twice.
const RB: usize = 128;

#[derive(Clone, Debug)]
pub(crate) struct Runs<T> {
    blocks: Vec<Vec<T>>,
    /// The blocks' lengths, as a Fenwick tree (`fen[k]`, 1-based: the
    /// lengths of blocks `k - lowbit(k) .. k`).
    fen: Vec<usize>,
    len: usize,
}

impl<T> Default for Runs<T> {
    fn default() -> Self {
        Runs {
            blocks: Vec::new(),
            fen: vec![0],
            len: 0,
        }
    }
}

impl<T> Runs<T> {
    pub(crate) fn len(&self) -> usize {
        self.len
    }

    pub(crate) fn last(&self) -> Option<&T> {
        self.blocks.last().and_then(|b| b.last())
    }

    /// The block holding index `i` (`i < len`), and the index in it.
    fn locate(&self, i: usize) -> (usize, usize) {
        let n = self.blocks.len();
        let (mut pos, mut rem) = (0, i);
        let mut step = if n == 0 { 0 } else { 1 << n.ilog2() };
        while step > 0 {
            if pos + step <= n && self.fen[pos + step] <= rem {
                pos += step;
                rem -= self.fen[pos];
            }
            step >>= 1;
        }
        (pos, rem)
    }

    /// Block `b`'s first entry's index.
    fn start(&self, b: usize) -> usize {
        let (mut k, mut s) = (b, 0);
        while k > 0 {
            s += self.fen[k];
            k &= k - 1;
        }
        s
    }

    /// Block `b`'s length changed by `d` (one, either way).
    fn bump(&mut self, b: usize, up: bool) {
        let mut k = b + 1;
        while k < self.fen.len() {
            if up {
                self.fen[k] += 1;
            } else {
                self.fen[k] -= 1;
            }
            k += k & k.wrapping_neg();
        }
    }

    /// A block appended at the end: its Fenwick entry.
    fn push_block(&mut self, b: Vec<T>) {
        let k = self.blocks.len() + 1;
        let low = k & k.wrapping_neg();
        let before = self.start(k - low);
        self.len += b.len();
        self.blocks.push(b);
        self.fen.push(self.len - before);
    }

    /// The sums from the blocks' lengths, in O(B).
    fn rebuild(&mut self) {
        let n = self.blocks.len();
        self.fen.clear();
        self.fen.push(0);
        self.fen.extend(self.blocks.iter().map(Vec::len));
        for k in 1..=n {
            let p = k + (k & k.wrapping_neg());
            if p <= n {
                self.fen[p] += self.fen[k];
            }
        }
        self.len = self.blocks.iter().map(Vec::len).sum();
    }

    pub(crate) fn get(&self, i: usize) -> Option<&T> {
        if i >= self.len {
            return None;
        }
        let (b, k) = self.locate(i);
        Some(&self.blocks[b][k])
    }

    /// The first index where `pred` is false (`pred` true then false
    /// along the list).
    pub(crate) fn partition_point(&self, mut pred: impl FnMut(&T) -> bool) -> usize {
        let b = self
            .blocks
            .partition_point(|bl| bl.last().is_some_and(&mut pred));
        if b == self.blocks.len() {
            return self.len;
        }
        self.start(b) + self.blocks[b].partition_point(pred)
    }

    pub(crate) fn push(&mut self, x: T) {
        match self.blocks.last_mut() {
            Some(b) if b.len() < 2 * RB => {
                b.push(x);
                self.len += 1;
                self.bump(self.blocks.len() - 1, true);
            }
            _ => {
                let mut b = Vec::with_capacity(2 * RB);
                b.push(x);
                self.push_block(b);
            }
        }
    }

    pub(crate) fn extend(&mut self, xs: impl IntoIterator<Item = T>) {
        for x in xs {
            self.push(x);
        }
    }

    pub(crate) fn insert(&mut self, i: usize, x: T) {
        if i == self.len {
            self.push(x);
            return;
        }
        let (b, k) = self.locate(i);
        self.blocks[b].insert(k, x);
        if self.blocks[b].len() > 2 * RB {
            let tail = self.blocks[b].split_off(RB);
            if b + 1 == self.blocks.len() {
                // (the last block: its entry fixed, the tail appended)
                self.len -= tail.len();
                self.len += 1;
                self.fen.pop();
                let head = self.blocks.pop().expect("a block");
                self.len -= head.len();
                self.push_block(head);
                self.push_block(tail);
            } else {
                self.blocks.insert(b + 1, tail);
                self.rebuild();
            }
        } else {
            self.len += 1;
            self.bump(b, true);
        }
    }

    pub(crate) fn remove(&mut self, i: usize) -> T {
        let (b, k) = self.locate(i);
        let x = self.blocks[b].remove(k);
        if self.blocks[b].is_empty() {
            self.blocks.remove(b);
            if b == self.blocks.len() {
                // (the last block: its entry goes, no other holds it)
                self.fen.pop();
                self.len -= 1;
            } else {
                self.rebuild();
            }
        } else {
            self.len -= 1;
            self.bump(b, false);
        }
        x
    }

    /// Entries `lo..hi` replaced by `xs`, in order.
    pub(crate) fn splice(&mut self, lo: usize, hi: usize, xs: impl IntoIterator<Item = T>) {
        for _ in lo..hi {
            self.remove(lo);
        }
        for (i, x) in (lo..).zip(xs) {
            self.insert(i, x);
        }
    }

    pub(crate) fn retain(&mut self, mut f: impl FnMut(&T) -> bool) {
        for b in &mut self.blocks {
            b.retain(&mut f);
        }
        self.blocks.retain(|b| !b.is_empty());
        self.rebuild();
    }

    pub(crate) fn iter(&self) -> impl DoubleEndedIterator<Item = &T> {
        self.blocks.iter().flatten()
    }

    pub(crate) fn iter_mut(&mut self) -> impl Iterator<Item = &mut T> {
        self.blocks.iter_mut().flatten()
    }

    /// The entries from index `i` on.
    pub(crate) fn iter_from(&self, i: usize) -> impl Iterator<Item = &T> {
        let (b, k) = if i >= self.len {
            (self.blocks.len(), 0)
        } else {
            self.locate(i)
        };
        self.blocks[b..]
            .iter()
            .enumerate()
            .flat_map(move |(j, bl)| bl[if j == 0 { k } else { 0 }..].iter())
    }

    /// The entries before index `i`, last first.
    pub(crate) fn iter_back(&self, i: usize) -> impl Iterator<Item = &T> {
        let (b, k) = if i >= self.len {
            (self.blocks.len(), 0)
        } else {
            self.locate(i)
        };
        let head = self.blocks.get(b).map_or(&[][..], |bl| &bl[..k]);
        head.iter()
            .rev()
            .chain(self.blocks[..b].iter().rev().flat_map(|bl| bl.iter().rev()))
    }

    pub(crate) fn shrink_to_fit(&mut self) {
        self.blocks.shrink_to_fit();
        self.fen.shrink_to_fit();
    }
}

impl<'a, T> IntoIterator for &'a Runs<T> {
    type Item = &'a T;
    type IntoIter = std::iter::Flatten<std::slice::Iter<'a, Vec<T>>>;
    fn into_iter(self) -> Self::IntoIter {
        self.blocks.iter().flatten()
    }
}

impl<'a, T> IntoIterator for &'a mut Runs<T> {
    type Item = &'a mut T;
    type IntoIter = std::iter::Flatten<std::slice::IterMut<'a, Vec<T>>>;
    fn into_iter(self) -> Self::IntoIter {
        self.blocks.iter_mut().flatten()
    }
}

impl<T> FromIterator<T> for Runs<T> {
    fn from_iter<I: IntoIterator<Item = T>>(it: I) -> Self {
        let mut r = Runs::default();
        r.extend(it);
        r
    }
}

#[cfg(test)]
mod tests {
    use super::Runs;

    #[test]
    fn runs_behave_like_a_sorted_vec() {
        let mut r: Runs<u32> = Runs::default();
        let mut v: Vec<u32> = Vec::new();
        let mut x = 12345u64;
        for step in 0..20_000 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let k = (x % 1000) as u32;
            if step % 3 == 2 && !v.is_empty() {
                let i = (x as usize / 7) % v.len();
                assert_eq!(r.remove(i), v.remove(i));
            } else {
                let i = v.partition_point(|&y| y < k);
                assert_eq!(r.partition_point(|&y| y < k), i);
                v.insert(i, k);
                r.insert(i, k);
            }
            assert_eq!(r.len(), v.len());
        }
        assert!(r.iter().eq(v.iter()));
        for i in [0, 1, 500, v.len() - 1, v.len()] {
            assert!(r.iter_from(i).eq(v[i..].iter()));
            assert!(r.iter_back(i).eq(v[..i].iter().rev()));
            assert_eq!(r.get(i), v.get(i));
        }
        r.splice(10, 400, [1, 2, 3]);
        v.splice(10..400, [1, 2, 3]);
        assert!(r.iter().eq(v.iter()));
        r.retain(|&y| y % 2 == 0);
        v.retain(|&y| y % 2 == 0);
        assert!(r.iter().eq(v.iter()));
        assert_eq!(r.len(), v.len());
    }
}
