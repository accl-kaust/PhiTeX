//! Myers' O(ND) difference algorithm, in linear space (the "middle
//! snake" divide and conquer of Myers 1986, §4b), over slices of keys.
//!
//! The result is the matched pairs of a longest common subsequence. A
//! subproblem whose edit distance exceeds a budget is not searched to the
//! end: its two sides are taken as all deleted and all inserted (a big
//! rewrite is shown as one, not word by word, and its cost stays bounded).

/// How far (in edits) a middle snake is looked for before a subproblem is
/// given up as a whole replacement.
const BUDGET: usize = 4096;

/// The matched index pairs `(i, j)` of a longest common subsequence of `a`
/// and `b` (up to the budget), in increasing order.
#[must_use]
pub fn lcs<T: Eq>(a: &[T], b: &[T]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let max = a.len() + b.len() + 2;
    let mut vf = vec![0usize; 2 * max + 2];
    let mut vb = vec![0usize; 2 * max + 2];
    conquer(a, 0, a.len(), b, 0, b.len(), &mut vf, &mut vb, &mut out);
    out
}

/// Patience diff: the elements that occur once on each side are anchors
/// (their longest increasing run), and Myers' LCS fills in between. Of the
/// common subsequences as long, it picks the one a reader expects: a word
/// common to both stays with its unique neighbors, not with a repeat of it
/// further on.
#[must_use]
pub fn patience<T: Eq + std::hash::Hash>(a: &[T], b: &[T]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    patience_in(a, (0, a.len()), b, (0, b.len()), &mut out);
    out
}

fn patience_in<T: Eq + std::hash::Hash>(
    a: &[T],
    (mut a0, mut a1): (usize, usize),
    b: &[T],
    (mut b0, mut b1): (usize, usize),
    out: &mut Vec<(usize, usize)>,
) {
    while a0 < a1 && b0 < b1 && a[a0] == b[b0] {
        out.push((a0, b0));
        a0 += 1;
        b0 += 1;
    }
    let mut suffix = 0;
    while a0 < a1 && b0 < b1 && a[a1 - 1] == b[b1 - 1] {
        a1 -= 1;
        b1 -= 1;
        suffix += 1;
    }
    if a0 < a1 && b0 < b1 {
        // (each element: its count and index on each side)
        let mut seen: std::collections::HashMap<&T, (usize, usize, usize, usize)> =
            std::collections::HashMap::new();
        for (i, x) in a.iter().enumerate().take(a1).skip(a0) {
            let e = seen.entry(x).or_insert((0, 0, 0, 0));
            e.0 += 1;
            e.1 = i;
        }
        for (j, x) in b.iter().enumerate().take(b1).skip(b0) {
            if let Some(e) = seen.get_mut(x) {
                e.2 += 1;
                e.3 = j;
            }
        }
        let mut uniq: Vec<(usize, usize)> = seen
            .values()
            .filter(|e| e.0 == 1 && e.2 == 1)
            .map(|e| (e.1, e.3))
            .collect();
        uniq.sort_unstable();
        let anchors = lis(&uniq);
        if anchors.is_empty() {
            let sub = lcs(&a[a0..a1], &b[b0..b1]);
            out.extend(sub.into_iter().map(|(i, j)| (a0 + i, b0 + j)));
        } else {
            let (mut i, mut j) = (a0, b0);
            for (x, y) in anchors {
                patience_in(a, (i, x), b, (j, y), out);
                out.push((x, y));
                i = x + 1;
                j = y + 1;
            }
            patience_in(a, (i, a1), b, (j, b1), out);
        }
    }
    for k in 0..suffix {
        out.push((a1 + k, b1 + k));
    }
}

/// The longest run of `pairs` (sorted by their first) increasing in their
/// second.
fn lis(pairs: &[(usize, usize)]) -> Vec<(usize, usize)> {
    // (patience sorting: the piles' tops, and each pair's predecessor)
    let mut tops: Vec<usize> = Vec::new();
    let mut prev = vec![usize::MAX; pairs.len()];
    for (k, p) in pairs.iter().enumerate() {
        let pile = tops.partition_point(|&t| pairs[t].1 < p.1);
        if pile > 0 {
            prev[k] = tops[pile - 1];
        }
        if pile == tops.len() {
            tops.push(k);
        } else {
            tops[pile] = k;
        }
    }
    let mut out = Vec::new();
    let mut k = tops.last().copied().unwrap_or(usize::MAX);
    while k != usize::MAX {
        out.push(pairs[k]);
        k = prev[k];
    }
    out.reverse();
    out
}

#[allow(clippy::too_many_arguments)]
fn conquer<T: Eq>(
    a: &[T],
    mut a0: usize,
    mut a1: usize,
    b: &[T],
    mut b0: usize,
    mut b1: usize,
    vf: &mut [usize],
    vb: &mut [usize],
    out: &mut Vec<(usize, usize)>,
) {
    while a0 < a1 && b0 < b1 && a[a0] == b[b0] {
        out.push((a0, b0));
        a0 += 1;
        b0 += 1;
    }
    let mut suffix = 0;
    while a0 < a1 && b0 < b1 && a[a1 - 1] == b[b1 - 1] {
        a1 -= 1;
        b1 -= 1;
        suffix += 1;
    }
    if a0 < a1
        && b0 < b1
        && let Some((x, y)) = middle_snake(a, a0, a1, b, b0, b1, vf, vb)
    {
        conquer(a, a0, x, b, b0, y, vf, vb, out);
        conquer(a, x, a1, b, y, b1, vf, vb, out);
    }
    for k in 0..suffix {
        out.push((a1 + k, b1 + k));
    }
}

/// A point on an optimal path through `a[a0..a1]` × `b[b0..b1]` that
/// splits it in two smaller problems (`None`: past the budget).
#[allow(
    clippy::too_many_arguments,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::many_single_char_names
)]
fn middle_snake<T: Eq>(
    a: &[T],
    a0: usize,
    a1: usize,
    b: &[T],
    b0: usize,
    b1: usize,
    vf: &mut [usize],
    vb: &mut [usize],
) -> Option<(usize, usize)> {
    let n = (a1 - a0) as isize;
    let m = (b1 - b0) as isize;
    let delta = n - m;
    let odd = delta & 1 == 1;
    // (V indexed by diagonal k, offset so that k may be negative)
    let off = (vf.len() / 2) as isize;
    let at = |k: isize| (k + off) as usize;
    vf[at(1)] = 0;
    vb[at(1)] = 0;
    let d_max = ((n + m + 1) / 2 + 1).min(BUDGET as isize);
    for d in 0..d_max {
        let mut k = -d;
        while k <= d {
            let mut x = if k == -d || (k != d && vf[at(k - 1)] < vf[at(k + 1)]) {
                vf[at(k + 1)] as isize
            } else {
                vf[at(k - 1)] as isize + 1
            };
            let mut y = x - k;
            let (x0, y0) = (x, y);
            while x < n && y < m && a[a0 + x as usize] == b[b0 + y as usize] {
                x += 1;
                y += 1;
            }
            vf[at(k)] = x as usize;
            if odd && (k - delta).abs() < d && x + vb[at(-(k - delta))] as isize >= n {
                return Some((a0 + x0 as usize, b0 + y0 as usize));
            }
            k += 2;
        }
        let mut k = -d;
        while k <= d {
            let mut x = if k == -d || (k != d && vb[at(k - 1)] < vb[at(k + 1)]) {
                vb[at(k + 1)] as isize
            } else {
                vb[at(k - 1)] as isize + 1
            };
            let mut y = x - k;
            while x < n && y < m && a[a0 + (n - x - 1) as usize] == b[b0 + (m - y - 1) as usize] {
                x += 1;
                y += 1;
            }
            vb[at(k)] = x as usize;
            if !odd && (k - delta).abs() <= d && x + vf[at(-(k - delta))] as isize >= n {
                return Some((a0 + (n - x) as usize, b0 + (m - y) as usize));
            }
            k += 2;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dp_len(a: &[u8], b: &[u8]) -> usize {
        let mut t = vec![vec![0usize; b.len() + 1]; a.len() + 1];
        for i in (0..a.len()).rev() {
            for j in (0..b.len()).rev() {
                t[i][j] = if a[i] == b[j] {
                    t[i + 1][j + 1] + 1
                } else {
                    t[i + 1][j].max(t[i][j + 1])
                };
            }
        }
        t[0][0]
    }

    /// Random pairs: the matches are a common subsequence, as long as the
    /// dynamic program's.
    #[test]
    fn random_against_dp() {
        let mut seed = 0x1234_5678_9abc_def1u64;
        let mut rand = |n: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % n
        };
        for _ in 0..3000 {
            let la = usize::try_from(rand(30)).unwrap();
            let lb = usize::try_from(rand(30)).unwrap();
            let alpha = rand(4) + 1;
            let a: Vec<u8> = (0..la)
                .map(|_| u8::try_from(rand(alpha)).unwrap())
                .collect();
            let b: Vec<u8> = (0..lb)
                .map(|_| u8::try_from(rand(alpha)).unwrap())
                .collect();
            let m = lcs(&a, &b);
            for w in m.windows(2) {
                assert!(w[0].0 < w[1].0 && w[0].1 < w[1].1);
            }
            for &(i, j) in &m {
                assert_eq!(a[i], b[j]);
            }
            assert_eq!(m.len(), dp_len(&a, &b), "{a:?} {b:?}");
            // (patience: a common subsequence, maybe shorter)
            let p = patience(&a, &b);
            for w in p.windows(2) {
                assert!(w[0].0 < w[1].0 && w[0].1 < w[1].1);
            }
            for &(i, j) in &p {
                assert_eq!(a[i], b[j]);
            }
        }
    }
}
