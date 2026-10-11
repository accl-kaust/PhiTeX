//! zlib's deflate, copied (madler zlib 1.3, `deflate.c` and `trees.c`):
//! the library pdfTeX links, so a stream compressed here is byte for byte
//! the stream pdfTeX writes (`writezip.c`: `deflateInit(level)`, the
//! default window and memory level, one stream at a time). A pure
//! function of the bytes and the level.
//!
//! What zlib keeps between calls (the window, the hash chains, the
//! pending symbols) is the state of one compression, local to
//! [`compress`]. The input is given whole: zlib's output does not depend
//! on how it is split into calls (`deflate_slow` and `deflate_fast` decide
//! only once `MIN_LOOKAHEAD` bytes are ahead or the input has ended, and
//! a match never reaches past the input's end: `nice_match` is capped by
//! the lookahead).

use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;

const MIN_MATCH: usize = 3;
const MAX_MATCH: usize = 258;
const MIN_LOOKAHEAD: usize = MAX_MATCH + MIN_MATCH + 1;
const W_SIZE: usize = 1 << 15;
const W_MASK: usize = W_SIZE - 1;
const WINDOW_SIZE: usize = 2 * W_SIZE;
/// `memLevel` 8: `hash_bits` = 8 + 7.
const HASH_BITS: u32 = 15;
const HASH_SIZE: usize = 1 << HASH_BITS;
const HASH_MASK: u32 = (1 << HASH_BITS) - 1;
/// `hash_shift`: (`hash_bits` + `MIN_MATCH` - 1) / `MIN_MATCH`.
const HASH_SHIFT: u32 = HASH_BITS.div_ceil(3);
const LIT_BUFSIZE: usize = 1 << (8 + 6);
const MAX_DIST: usize = W_SIZE - MIN_LOOKAHEAD;
const TOO_FAR: usize = 4096;
const NIL: usize = 0;

const LENGTH_CODES: usize = 29;
const LITERALS: usize = 256;
const L_CODES: usize = LITERALS + 1 + LENGTH_CODES;
const D_CODES: usize = 30;
const BL_CODES: usize = 19;
const HEAP_SIZE: usize = 2 * L_CODES + 1;
const MAX_BITS: usize = 15;
const MAX_BL_BITS: usize = 7;
const END_BLOCK: usize = 256;
const REP_3_6: usize = 16;
const REPZ_3_10: usize = 17;
const REPZ_11_138: usize = 18;
const BUF_SIZE: u32 = 16;
const STORED_BLOCK: u32 = 0;
const STATIC_TREES: u32 = 1;
const DYN_TREES: u32 = 2;

const EXTRA_LBITS: [u32; LENGTH_CODES] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const EXTRA_DBITS: [u32; D_CODES] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];
const EXTRA_BLBITS: [u32; BL_CODES] = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 3, 7];
const BL_ORDER: [usize; BL_CODES] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

/// `configuration_table`: good, lazy, nice, chain; levels 1–3 are
/// `deflate_fast`, 4–9 `deflate_slow`.
const CONFIG: [(usize, usize, usize, usize); 10] = [
    (0, 0, 0, 0),
    (4, 4, 8, 4),
    (4, 5, 16, 8),
    (4, 6, 32, 32),
    (4, 4, 16, 16),
    (8, 16, 32, 32),
    (8, 16, 128, 128),
    (8, 32, 128, 256),
    (32, 128, 258, 1024),
    (32, 258, 258, 4096),
];

/// `ct_data`: `fc` is the frequency or the code, `dl` the father or the
/// length (C's unions: `build_tree` writes `Dad` over `Len` and
/// `gen_codes` `Code` over `Freq`, as zlib does).
#[derive(Clone, Copy, Default)]
struct Ct {
    fc: u16,
    dl: u16,
}

/// The tables `tr_static_init` makes.
struct Static {
    ltree: [Ct; L_CODES + 2],
    dtree: [Ct; D_CODES],
    dist_code: [u8; 512],
    length_code: [u8; MAX_MATCH - MIN_MATCH + 1],
    base_length: [usize; LENGTH_CODES],
    base_dist: [usize; D_CODES],
}

/// `bi_reverse`: the first `len` bits of `code` reversed (`len` ≤ 15).
fn bi_reverse(mut code: u16, mut len: usize) -> u16 {
    let mut res = 0;
    loop {
        res |= code & 1;
        code >>= 1;
        res <<= 1;
        len -= 1;
        if len == 0 {
            break;
        }
    }
    res >> 1
}

fn gen_codes(tree: &mut [Ct], max_code: usize, bl_count: &[u16; MAX_BITS + 1]) {
    let mut next_code = [0u16; MAX_BITS + 1];
    let mut code: u32 = 0;
    for bits in 1..=MAX_BITS {
        code = (code + u32::from(bl_count[bits - 1])) << 1;
        // (`gen_codes`' Assert: the counts are those of a complete code
        // of at most `MAX_BITS` bits, so a length's first code fits them)
        next_code[bits] = u16::try_from(code).expect("a code of at most 15 bits");
    }
    for ct in &mut tree[..=max_code] {
        let len = usize::from(ct.dl);
        if len == 0 {
            continue;
        }
        ct.fc = bi_reverse(next_code[len], len);
        next_code[len] = next_code[len].wrapping_add(1);
    }
}

impl Static {
    fn new() -> Box<Static> {
        let mut s = Box::new(Static {
            ltree: [Ct::default(); L_CODES + 2],
            dtree: [Ct::default(); D_CODES],
            dist_code: [0; 512],
            length_code: [0; MAX_MATCH - MIN_MATCH + 1],
            base_length: [0; LENGTH_CODES],
            base_dist: [0; D_CODES],
        });
        let mut length = 0usize;
        let mut code = 0usize;
        while code < LENGTH_CODES - 1 {
            s.base_length[code] = length;
            for _ in 0..(1 << EXTRA_LBITS[code]) {
                s.length_code[length] = code_u8(code);
                length += 1;
            }
            code += 1;
        }
        s.length_code[length - 1] = code_u8(code);
        let mut dist = 0usize;
        code = 0;
        while code < 16 {
            s.base_dist[code] = dist;
            for _ in 0..(1 << EXTRA_DBITS[code]) {
                s.dist_code[dist] = code_u8(code);
                dist += 1;
            }
            code += 1;
        }
        dist >>= 7;
        while code < D_CODES {
            s.base_dist[code] = dist << 7;
            for _ in 0..(1 << (EXTRA_DBITS[code] - 7)) {
                s.dist_code[256 + dist] = code_u8(code);
                dist += 1;
            }
            code += 1;
        }
        let mut bl_count = [0u16; MAX_BITS + 1];
        let mut n = 0;
        while n <= 143 {
            s.ltree[n].dl = 8;
            bl_count[8] += 1;
            n += 1;
        }
        while n <= 255 {
            s.ltree[n].dl = 9;
            bl_count[9] += 1;
            n += 1;
        }
        while n <= 279 {
            s.ltree[n].dl = 7;
            bl_count[7] += 1;
            n += 1;
        }
        while n <= 287 {
            s.ltree[n].dl = 8;
            bl_count[8] += 1;
            n += 1;
        }
        gen_codes(&mut s.ltree, L_CODES + 1, &bl_count);
        for (n, ct) in (0u16..).zip(&mut s.dtree) {
            ct.dl = 5;
            ct.fc = bi_reverse(n, 5);
        }
        s
    }

    fn d_code(&self, dist: usize) -> usize {
        if dist < 256 {
            usize::from(self.dist_code[dist])
        } else {
            usize::from(self.dist_code[256 + (dist >> 7)])
        }
    }
}

/// A length or distance code (< `D_CODES`) as `tr_static_init` stores it.
fn code_u8(code: usize) -> u8 {
    u8::try_from(code).expect("a code is below 30")
}

/// A count or length below `1 << length` as `send_bits` takes it.
fn bits(v: usize) -> u32 {
    u32::try_from(v).expect("send_bits' value is at most 16 bits")
}

/// A tree node (< `HEAP_SIZE`) as `ct_data`'s `Dad` stores it.
fn node_u16(n: usize) -> u16 {
    u16::try_from(n).expect("a node is below HEAP_SIZE")
}

/// `smaller`: `n` sorts before `m` (by frequency, then depth).
fn smaller(tree: &[Ct], n: usize, m: usize, depth: &[u8]) -> bool {
    tree[n].fc < tree[m].fc || (tree[n].fc == tree[m].fc && depth[n] <= depth[m])
}

/// `pqdownheap`: restore the heap property from node `k` down.
fn pqdownheap(tree: &[Ct], heap: &mut [usize], heap_len: usize, depth: &[u8], mut k: usize) {
    let v = heap[k];
    let mut j = k << 1;
    while j <= heap_len {
        if j < heap_len && smaller(tree, heap[j + 1], heap[j], depth) {
            j += 1;
        }
        if smaller(tree, v, heap[j], depth) {
            break;
        }
        heap[k] = heap[j];
        k = j;
        j <<= 1;
    }
    heap[k] = v;
}

/// Which tree a `tree_desc` describes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Which {
    L,
    D,
    Bl,
}

/// One compression: zlib's `deflate_state` with its `z_stream`'s input.
struct State<'a> {
    st: Box<Static>,
    level: usize,
    input: &'a [u8],
    next_in: usize,
    adler: u32,
    out: Vec<u8>,
    window: Vec<u8>,
    prev: Vec<u16>,
    head: Vec<u16>,
    ins_h: u32,
    /// `block_start` (C's `long`): negative once the window slid past
    /// the block's start.
    block_start: isize,
    match_length: usize,
    prev_match: usize,
    match_available: bool,
    strstart: usize,
    match_start: usize,
    lookahead: usize,
    prev_length: usize,
    max_chain_length: usize,
    max_lazy_match: usize,
    good_match: usize,
    nice_match: usize,
    dyn_ltree: [Ct; HEAP_SIZE],
    dyn_dtree: [Ct; 2 * D_CODES + 1],
    bl_tree: [Ct; 2 * BL_CODES + 1],
    l_max_code: usize,
    d_max_code: usize,
    bl_max_code: usize,
    bl_count: [u16; MAX_BITS + 1],
    heap: [usize; 2 * L_CODES + 1],
    heap_len: usize,
    heap_max: usize,
    depth: [u8; 2 * L_CODES + 1],
    sym_buf: Vec<u8>,
    sym_next: usize,
    sym_end: usize,
    opt_len: u64,
    static_len: u64,
    insert: usize,
    bi_buf: u16,
    bi_valid: u32,
}

enum Block {
    NeedMore,
    Done,
    FinishDone,
}

/// A window position (< `WINDOW_SIZE` = 64K) as `head` and `prev` keep
/// it (C's `Pos`, an `ush`).
fn pos_u16(s: usize) -> u16 {
    u16::try_from(s).expect("a position in the window")
}

/// A window position as `block_start` (C's `long`) counts it.
fn isize_of(s: usize) -> isize {
    isize::try_from(s).expect("a position in the window")
}

impl State<'_> {
    // ---- trees.c ----

    fn put_byte(&mut self, c: u8) {
        self.out.push(c);
    }

    fn put_short(&mut self, w: u16) {
        self.out.extend_from_slice(&w.to_le_bytes());
    }

    /// `send_bits`: `value` (below `1 << length`, `length` ≤ 16, as
    /// zlib's Assert in `send_bits` has it) appended to the bit buffer,
    /// its low 16 bits written once they are full. With `bi_valid` ≤ 16
    /// the buffer and the new bits fit a `u32`: its low half is what C's
    /// `bi_buf |= (ush)value << bi_valid` keeps, its high half the
    /// `(ush)value >> (Buf_size - bi_valid)` carried over.
    fn send_bits(&mut self, value: u32, length: u32) {
        let acc = u32::from(self.bi_buf) | value << self.bi_valid;
        let [b0, b1, b2, b3] = acc.to_le_bytes();
        let total = self.bi_valid + length;
        if total > BUF_SIZE {
            self.out.extend_from_slice(&[b0, b1]);
            self.bi_buf = u16::from_le_bytes([b2, b3]);
            self.bi_valid = total - BUF_SIZE;
        } else {
            self.bi_buf = u16::from_le_bytes([b0, b1]);
            self.bi_valid = total;
        }
    }

    fn send_code(&mut self, c: usize, tree: Tree) {
        let ct = match tree {
            Tree::StaticL => self.st.ltree[c],
            Tree::StaticD => self.st.dtree[c],
            Tree::DynL => self.dyn_ltree[c],
            Tree::DynD => self.dyn_dtree[c],
            Tree::Bl => self.bl_tree[c],
        };
        self.send_bits(u32::from(ct.fc), u32::from(ct.dl));
    }

    fn bi_windup(&mut self) {
        if self.bi_valid > 8 {
            let b = self.bi_buf;
            self.put_short(b);
        } else if self.bi_valid > 0 {
            let [b, _] = self.bi_buf.to_le_bytes();
            self.put_byte(b);
        }
        self.bi_buf = 0;
        self.bi_valid = 0;
    }

    fn init_block(&mut self) {
        for n in 0..L_CODES {
            self.dyn_ltree[n].fc = 0;
        }
        for n in 0..D_CODES {
            self.dyn_dtree[n].fc = 0;
        }
        for n in 0..BL_CODES {
            self.bl_tree[n].fc = 0;
        }
        self.dyn_ltree[END_BLOCK].fc = 1;
        self.opt_len = 0;
        self.static_len = 0;
        self.sym_next = 0;
    }

    fn tree(&mut self, w: Which) -> &mut [Ct] {
        match w {
            Which::L => &mut self.dyn_ltree,
            Which::D => &mut self.dyn_dtree,
            Which::Bl => &mut self.bl_tree,
        }
    }

    fn pqdownheap(&mut self, w: Which, k: usize) {
        let tree: &[Ct] = match w {
            Which::L => &self.dyn_ltree,
            Which::D => &self.dyn_dtree,
            Which::Bl => &self.bl_tree,
        };
        pqdownheap(tree, &mut self.heap, self.heap_len, &self.depth, k);
    }

    fn gen_bitlen(&mut self, w: Which) {
        let max_code = match w {
            Which::L => self.l_max_code,
            Which::D => self.d_max_code,
            Which::Bl => self.bl_max_code,
        };
        let (extra, base, max_length): (&[u32], usize, usize) = match w {
            Which::L => (&EXTRA_LBITS, LITERALS + 1, MAX_BITS),
            Which::D => (&EXTRA_DBITS, 0, MAX_BITS),
            Which::Bl => (&EXTRA_BLBITS, 0, MAX_BL_BITS),
        };
        let mut overflow = 0i32;
        for bits in 0..=MAX_BITS {
            self.bl_count[bits] = 0;
        }
        let root = self.heap[self.heap_max];
        self.tree(w)[root].dl = 0;
        let mut h = self.heap_max + 1;
        while h < HEAP_SIZE {
            let n = self.heap[h];
            let tree = self.tree(w);
            let mut bits = usize::from(tree[usize::from(tree[n].dl)].dl) + 1;
            if bits > max_length {
                bits = max_length;
                overflow += 1;
            }
            tree[n].dl = node_u16(bits);
            h += 1;
            if n > max_code {
                continue;
            }
            let f = u64::from(tree[n].fc);
            self.bl_count[bits] += 1;
            let xbits = if n >= base {
                u64::from(extra[n - base])
            } else {
                0
            };
            self.opt_len = self.opt_len.wrapping_add(f * (bits as u64 + xbits));
            let stree_len = match w {
                Which::L => Some(u64::from(self.st.ltree[n].dl)),
                Which::D => Some(u64::from(self.st.dtree[n].dl)),
                Which::Bl => None,
            };
            if let Some(sl) = stree_len {
                self.static_len = self.static_len.wrapping_add(f * (sl + xbits));
            }
        }
        if overflow == 0 {
            return;
        }
        loop {
            let mut bits = max_length - 1;
            while self.bl_count[bits] == 0 {
                bits -= 1;
            }
            self.bl_count[bits] -= 1;
            self.bl_count[bits + 1] += 2;
            self.bl_count[max_length] -= 1;
            overflow -= 2;
            if overflow <= 0 {
                break;
            }
        }
        let mut h = HEAP_SIZE;
        let mut bits = max_length;
        while bits != 0 {
            let mut n = self.bl_count[bits];
            while n != 0 {
                h -= 1;
                let m = self.heap[h];
                if m > max_code {
                    continue;
                }
                let tree = self.tree(w);
                if usize::from(tree[m].dl) != bits {
                    let fr = u64::from(tree[m].fc);
                    let old = u64::from(tree[m].dl);
                    tree[m].dl = node_u16(bits);
                    self.opt_len = self
                        .opt_len
                        .wrapping_add((bits as u64).wrapping_sub(old).wrapping_mul(fr));
                }
                n -= 1;
            }
            bits -= 1;
        }
    }

    fn build_tree(&mut self, w: Which) {
        let elems = match w {
            Which::L => L_CODES,
            Which::D => D_CODES,
            Which::Bl => BL_CODES,
        };
        // (C's `max_code = -1`: `None`; at least two codes are made
        // nonzero below, so it is a code after)
        let mut max_code: Option<usize> = None;
        self.heap_len = 0;
        self.heap_max = HEAP_SIZE;
        for n in 0..elems {
            if self.tree(w)[n].fc != 0 {
                self.heap_len += 1;
                self.heap[self.heap_len] = n;
                max_code = Some(n);
                self.depth[n] = 0;
            } else {
                self.tree(w)[n].dl = 0;
            }
        }
        while self.heap_len < 2 {
            let node = match max_code {
                Some(m) if m >= 2 => 0,
                m => {
                    let next = m.map_or(0, |m| m + 1);
                    max_code = Some(next);
                    next
                }
            };
            self.heap_len += 1;
            self.heap[self.heap_len] = node;
            self.tree(w)[node].fc = 1;
            self.depth[node] = 0;
            self.opt_len = self.opt_len.wrapping_sub(1);
            let static_bits = match w {
                Which::L => u64::from(self.st.ltree[node].dl),
                Which::D => u64::from(self.st.dtree[node].dl),
                Which::Bl => 0,
            };
            self.static_len = self.static_len.wrapping_sub(static_bits);
        }
        let max_code = max_code.expect("two codes at least");
        match w {
            Which::L => self.l_max_code = max_code,
            Which::D => self.d_max_code = max_code,
            Which::Bl => self.bl_max_code = max_code,
        }
        let mut n = self.heap_len / 2;
        while n >= 1 {
            self.pqdownheap(w, n);
            n -= 1;
        }
        let mut node = elems;
        loop {
            // (pqremove)
            let n = self.heap[1];
            self.heap[1] = self.heap[self.heap_len];
            self.heap_len -= 1;
            self.pqdownheap(w, 1);
            let m = self.heap[1];
            self.heap_max -= 1;
            self.heap[self.heap_max] = n;
            self.heap_max -= 1;
            self.heap[self.heap_max] = m;
            let (nu, mu) = (n, m);
            let (dn, dm) = (self.depth[nu], self.depth[mu]);
            let tree = self.tree(w);
            tree[node].fc = tree[nu].fc.wrapping_add(tree[mu].fc);
            tree[nu].dl = node_u16(node);
            tree[mu].dl = node_u16(node);
            self.depth[node] = (if dn >= dm { dn } else { dm }).wrapping_add(1);
            self.heap[1] = node;
            node += 1;
            self.pqdownheap(w, 1);
            if self.heap_len < 2 {
                break;
            }
        }
        self.heap_max -= 1;
        self.heap[self.heap_max] = self.heap[1];
        self.gen_bitlen(w);
        let bl_count = self.bl_count;
        gen_codes(self.tree(w), max_code, &bl_count);
    }

    fn scan_tree(&mut self, w: Which, max_code: usize) {
        // (C's `prevlen = -1`: no length yet)
        let mut prevlen: Option<usize> = None;
        let mut nextlen = usize::from(self.tree(w)[0].dl);
        let mut count: usize = 0;
        let (mut max_count, mut min_count) = (7, 4);
        if nextlen == 0 {
            max_count = 138;
            min_count = 3;
        }
        self.tree(w)[max_code + 1].dl = 0xffff;
        for n in 0..=max_code {
            let curlen = nextlen;
            nextlen = usize::from(self.tree(w)[n + 1].dl);
            count += 1;
            if count < max_count && curlen == nextlen {
                continue;
            }
            if count < min_count {
                // (`count` < `min_count` ≤ 4)
                let c = u16::try_from(count).expect("a short run");
                self.bl_tree[curlen].fc = self.bl_tree[curlen].fc.wrapping_add(c);
            } else if curlen != 0 {
                if Some(curlen) != prevlen {
                    self.bl_tree[curlen].fc = self.bl_tree[curlen].fc.wrapping_add(1);
                }
                self.bl_tree[REP_3_6].fc = self.bl_tree[REP_3_6].fc.wrapping_add(1);
            } else if count <= 10 {
                self.bl_tree[REPZ_3_10].fc = self.bl_tree[REPZ_3_10].fc.wrapping_add(1);
            } else {
                self.bl_tree[REPZ_11_138].fc = self.bl_tree[REPZ_11_138].fc.wrapping_add(1);
            }
            count = 0;
            prevlen = Some(curlen);
            if nextlen == 0 {
                max_count = 138;
                min_count = 3;
            } else if curlen == nextlen {
                max_count = 6;
                min_count = 3;
            } else {
                max_count = 7;
                min_count = 4;
            }
        }
    }

    fn send_tree(&mut self, w: Which, max_code: usize) {
        // (C's `prevlen = -1`: no length yet)
        let mut prevlen: Option<usize> = None;
        let mut nextlen = usize::from(self.tree(w)[0].dl);
        let mut count: usize = 0;
        let (mut max_count, mut min_count) = (7, 4);
        if nextlen == 0 {
            max_count = 138;
            min_count = 3;
        }
        for n in 0..=max_code {
            let curlen = nextlen;
            nextlen = usize::from(self.tree(w)[n + 1].dl);
            count += 1;
            if count < max_count && curlen == nextlen {
                continue;
            }
            if count < min_count {
                loop {
                    self.send_code(curlen, Tree::Bl);
                    count -= 1;
                    if count == 0 {
                        break;
                    }
                }
            } else if curlen != 0 {
                if Some(curlen) != prevlen {
                    self.send_code(curlen, Tree::Bl);
                    count -= 1;
                }
                self.send_code(REP_3_6, Tree::Bl);
                self.send_bits(bits(count - 3), 2);
            } else if count <= 10 {
                self.send_code(REPZ_3_10, Tree::Bl);
                self.send_bits(bits(count - 3), 3);
            } else {
                self.send_code(REPZ_11_138, Tree::Bl);
                self.send_bits(bits(count - 11), 7);
            }
            count = 0;
            prevlen = Some(curlen);
            if nextlen == 0 {
                max_count = 138;
                min_count = 3;
            } else if curlen == nextlen {
                max_count = 6;
                min_count = 3;
            } else {
                max_count = 7;
                min_count = 4;
            }
        }
    }

    fn build_bl_tree(&mut self) -> usize {
        self.scan_tree(Which::L, self.l_max_code);
        self.scan_tree(Which::D, self.d_max_code);
        self.build_tree(Which::Bl);
        let mut max_blindex = BL_CODES - 1;
        while max_blindex >= 3 {
            if self.bl_tree[BL_ORDER[max_blindex]].dl != 0 {
                break;
            }
            max_blindex -= 1;
        }
        self.opt_len = self
            .opt_len
            .wrapping_add(3 * (max_blindex as u64 + 1) + 5 + 5 + 4);
        max_blindex
    }

    fn send_all_trees(&mut self, lcodes: usize, dcodes: usize, blcodes: usize) {
        self.send_bits(bits(lcodes - 257), 5);
        self.send_bits(bits(dcodes - 1), 5);
        self.send_bits(bits(blcodes - 4), 4);
        for &r in &BL_ORDER[..blcodes] {
            let l = u32::from(self.bl_tree[r].dl);
            self.send_bits(l, 3);
        }
        self.send_tree(Which::L, lcodes - 1);
        self.send_tree(Which::D, dcodes - 1);
    }

    fn stored_block(&mut self, buf: Option<usize>, stored_len: usize, last: bool) {
        self.send_bits((STORED_BLOCK << 1) + u32::from(last), 3);
        self.bi_windup();
        // (`_tr_stored_block` writes `(ush)stored_len`: a block is at most
        // the window, and a stored one at most `MAX_STORED`)
        let len = u16::try_from(stored_len).expect("a stored block fits 16 bits");
        self.put_short(len);
        self.put_short(!len);
        if let Some(b) = buf {
            let w = self.window[b..b + stored_len].to_vec();
            self.out.extend_from_slice(&w);
        }
    }

    fn compress_block(&mut self, ltree: Tree, dtree: Tree) {
        let mut sx = 0;
        if self.sym_next != 0 {
            loop {
                let mut dist = usize::from(self.sym_buf[sx]);
                dist += usize::from(self.sym_buf[sx + 1]) << 8;
                let mut lc = usize::from(self.sym_buf[sx + 2]);
                sx += 3;
                if dist == 0 {
                    self.send_code(lc, ltree);
                } else {
                    let code = usize::from(self.st.length_code[lc]);
                    self.send_code(code + LITERALS + 1, ltree);
                    let extra = EXTRA_LBITS[code];
                    if extra != 0 {
                        lc -= self.st.base_length[code];
                        self.send_bits(bits(lc), extra);
                    }
                    dist -= 1;
                    let code = self.st.d_code(dist);
                    self.send_code(code, dtree);
                    let extra = EXTRA_DBITS[code];
                    if extra != 0 {
                        dist -= self.st.base_dist[code];
                        self.send_bits(bits(dist), extra);
                    }
                }
                if sx >= self.sym_next {
                    break;
                }
            }
        }
        self.send_code(END_BLOCK, ltree);
    }

    fn flush_block(&mut self, buf: Option<usize>, stored_len: usize, last: bool) {
        self.build_tree(Which::L);
        self.build_tree(Which::D);
        let max_blindex = self.build_bl_tree();
        let mut opt_lenb = self.opt_len.wrapping_add(3 + 7) >> 3;
        let static_lenb = self.static_len.wrapping_add(3 + 7) >> 3;
        if static_lenb <= opt_lenb {
            opt_lenb = static_lenb;
        }
        if stored_len as u64 + 4 <= opt_lenb && buf.is_some() {
            self.stored_block(buf, stored_len, last);
        } else if static_lenb == opt_lenb {
            self.send_bits((STATIC_TREES << 1) + u32::from(last), 3);
            self.compress_block(Tree::StaticL, Tree::StaticD);
        } else {
            self.send_bits((DYN_TREES << 1) + u32::from(last), 3);
            let (l, d) = (self.l_max_code + 1, self.d_max_code + 1);
            self.send_all_trees(l, d, max_blindex + 1);
            self.compress_block(Tree::DynL, Tree::DynD);
        }
        self.init_block();
        if last {
            self.bi_windup();
        }
    }

    fn tally_lit(&mut self, c: u8) -> bool {
        self.sym_buf[self.sym_next] = 0;
        self.sym_buf[self.sym_next + 1] = 0;
        self.sym_buf[self.sym_next + 2] = c;
        self.sym_next += 3;
        self.dyn_ltree[usize::from(c)].fc = self.dyn_ltree[usize::from(c)].fc.wrapping_add(1);
        self.sym_next == self.sym_end
    }

    fn tally_dist(&mut self, dist: usize, len: usize) -> bool {
        // (`_tr_tally_dist`: `dist` ≤ `MAX_DIST`, `len` ≤ `MAX_MATCH` - `MIN_MATCH`)
        let [d0, d1] = u16::try_from(dist)
            .expect("dist below the window")
            .to_le_bytes();
        self.sym_buf[self.sym_next] = d0;
        self.sym_buf[self.sym_next + 1] = d1;
        self.sym_buf[self.sym_next + 2] = u8::try_from(len).expect("len below 256");
        self.sym_next += 3;
        let lc = usize::from(self.st.length_code[len]) + LITERALS + 1;
        self.dyn_ltree[lc].fc = self.dyn_ltree[lc].fc.wrapping_add(1);
        let dc = self.st.d_code(dist - 1);
        self.dyn_dtree[dc].fc = self.dyn_dtree[dc].fc.wrapping_add(1);
        self.sym_next == self.sym_end
    }

    // ---- deflate.c ----

    fn update_hash(&mut self, c: u8) {
        self.ins_h = ((self.ins_h << HASH_SHIFT) ^ u32::from(c)) & HASH_MASK;
    }

    /// `INSERT_STRING`: returns the chain's previous head.
    fn insert_string(&mut self, s: usize) -> usize {
        let c = self.window[s + MIN_MATCH - 1];
        self.update_hash(c);
        let h = self.head[self.ins_h as usize];
        self.prev[s & W_MASK] = h;
        self.head[self.ins_h as usize] = pos_u16(s);
        usize::from(h)
    }

    fn slide_hash(&mut self) {
        // (`slide_hash`: `m >= wsize ? m - wsize : NIL`, `NIL` being 0)
        let w = pos_u16(W_SIZE);
        for p in self.head.iter_mut().chain(&mut self.prev) {
            *p = p.saturating_sub(w);
        }
    }

    fn read_buf(&mut self, at: usize, size: usize) -> usize {
        let len = (self.input.len() - self.next_in).min(size);
        if len == 0 {
            return 0;
        }
        let src = &self.input[self.next_in..self.next_in + len];
        self.window[at..at + len].copy_from_slice(src);
        self.adler = adler32(self.adler, src);
        self.next_in += len;
        len
    }

    fn avail_in(&self) -> usize {
        self.input.len() - self.next_in
    }

    fn fill_window(&mut self) {
        loop {
            let mut more = WINDOW_SIZE - self.lookahead - self.strstart;
            if self.strstart >= W_SIZE + MAX_DIST {
                self.window.copy_within(W_SIZE..W_SIZE + W_SIZE - more, 0);
                self.match_start = self.match_start.wrapping_sub(W_SIZE);
                self.strstart -= W_SIZE;
                self.block_start -= isize_of(W_SIZE);
                if self.insert > self.strstart {
                    self.insert = self.strstart;
                }
                self.slide_hash();
                more += W_SIZE;
            }
            if self.avail_in() == 0 {
                break;
            }
            let n = self.read_buf(self.strstart + self.lookahead, more);
            self.lookahead += n;
            if self.lookahead + self.insert >= MIN_MATCH {
                let mut str = self.strstart - self.insert;
                self.ins_h = u32::from(self.window[str]);
                let c = self.window[str + 1];
                self.update_hash(c);
                while self.insert != 0 {
                    let c = self.window[str + MIN_MATCH - 1];
                    self.update_hash(c);
                    self.prev[str & W_MASK] = self.head[self.ins_h as usize];
                    self.head[self.ins_h as usize] = pos_u16(str);
                    str += 1;
                    self.insert -= 1;
                    if self.lookahead + self.insert < MIN_MATCH {
                        break;
                    }
                }
            }
            if !(self.lookahead < MIN_LOOKAHEAD && self.avail_in() != 0) {
                break;
            }
        }
        // (the window is zeroed beyond the data from the start: zlib's
        // `high_water` zeroing changes nothing a match can read, as the
        // matches are capped by the lookahead)
    }

    fn longest_match(&mut self, mut cur_match: usize) -> usize {
        let mut chain_length = self.max_chain_length;
        let scan0 = self.strstart;
        let mut best_len = self.prev_length;
        let mut nice_match = self.nice_match;
        let limit = if self.strstart > MAX_DIST {
            self.strstart - MAX_DIST
        } else {
            NIL
        };
        let strend = self.strstart + MAX_MATCH;
        let w = &self.window;
        let mut scan_end1 = w[scan0 + best_len - 1];
        let mut scan_end = w[scan0 + best_len];
        if self.prev_length >= self.good_match {
            chain_length >>= 2;
        }
        if nice_match > self.lookahead {
            nice_match = self.lookahead;
        }
        let mut match_start = self.match_start;
        loop {
            let m = cur_match;
            let skip = w[m + best_len] != scan_end
                || w[m + best_len - 1] != scan_end1
                || w[m] != w[scan0]
                || w[m + 1] != w[scan0 + 1];
            if !skip {
                // (the bytes from 2 on: zlib compares eight at a time and
                // then stops at `strend`; the length is where they differ)
                let mut scan = scan0 + 2;
                let mut mat = m + 2;
                loop {
                    let mut k = 0;
                    let mut stop = false;
                    while k < 8 {
                        scan += 1;
                        mat += 1;
                        if w[scan] != w[mat] {
                            stop = true;
                            break;
                        }
                        k += 1;
                    }
                    if stop || scan >= strend {
                        break;
                    }
                }
                let len = MAX_MATCH - (strend - scan);
                if len > best_len {
                    match_start = cur_match;
                    best_len = len;
                    if len >= nice_match {
                        break;
                    }
                    scan_end1 = w[scan0 + best_len - 1];
                    scan_end = w[scan0 + best_len];
                }
            }
            cur_match = usize::from(self.prev[cur_match & W_MASK]);
            if cur_match <= limit {
                break;
            }
            chain_length -= 1;
            if chain_length == 0 {
                break;
            }
        }
        self.match_start = match_start;
        if best_len <= self.lookahead {
            best_len
        } else {
            self.lookahead
        }
    }

    fn block_ptr(&self) -> Option<usize> {
        usize::try_from(self.block_start).ok()
    }

    fn flush_block_only(&mut self, last: bool) {
        let buf = self.block_ptr();
        let len = usize::try_from(isize_of(self.strstart) - self.block_start)
            .expect("the block starts before strstart");
        self.flush_block(buf, len, last);
        self.block_start = isize_of(self.strstart);
    }

    fn deflate_fast(&mut self, finish: bool) -> Block {
        loop {
            if self.lookahead < MIN_LOOKAHEAD {
                self.fill_window();
                if self.lookahead < MIN_LOOKAHEAD && !finish {
                    return Block::NeedMore;
                }
                if self.lookahead == 0 {
                    break;
                }
            }
            let mut hash_head = NIL;
            if self.lookahead >= MIN_MATCH {
                hash_head = self.insert_string(self.strstart);
            }
            if hash_head != NIL && self.strstart - hash_head <= MAX_DIST {
                self.match_length = self.longest_match(hash_head);
            }
            let bflush;
            if self.match_length >= MIN_MATCH {
                bflush = self.tally_dist(
                    self.strstart - self.match_start,
                    self.match_length - MIN_MATCH,
                );
                self.lookahead -= self.match_length;
                // (`max_insert_length` is `max_lazy_match`)
                if self.match_length <= self.max_lazy_match && self.lookahead >= MIN_MATCH {
                    self.match_length -= 1;
                    loop {
                        self.strstart += 1;
                        self.insert_string(self.strstart);
                        self.match_length -= 1;
                        if self.match_length == 0 {
                            break;
                        }
                    }
                    self.strstart += 1;
                } else {
                    self.strstart += self.match_length;
                    self.match_length = 0;
                    self.ins_h = u32::from(self.window[self.strstart]);
                    let c = self.window[self.strstart + 1];
                    self.update_hash(c);
                }
            } else {
                let c = self.window[self.strstart];
                bflush = self.tally_lit(c);
                self.lookahead -= 1;
                self.strstart += 1;
            }
            if bflush {
                self.flush_block_only(false);
            }
        }
        self.insert = if self.strstart < MIN_MATCH - 1 {
            self.strstart
        } else {
            MIN_MATCH - 1
        };
        if finish {
            self.flush_block_only(true);
            return Block::FinishDone;
        }
        if self.sym_next != 0 {
            self.flush_block_only(false);
        }
        Block::Done
    }

    fn deflate_slow(&mut self, finish: bool) -> Block {
        loop {
            if self.lookahead < MIN_LOOKAHEAD {
                self.fill_window();
                if self.lookahead < MIN_LOOKAHEAD && !finish {
                    return Block::NeedMore;
                }
                if self.lookahead == 0 {
                    break;
                }
            }
            let mut hash_head = NIL;
            if self.lookahead >= MIN_MATCH {
                hash_head = self.insert_string(self.strstart);
            }
            self.prev_length = self.match_length;
            self.prev_match = self.match_start;
            self.match_length = MIN_MATCH - 1;
            if hash_head != NIL
                && self.prev_length < self.max_lazy_match
                && self.strstart - hash_head <= MAX_DIST
            {
                self.match_length = self.longest_match(hash_head);
                if self.match_length <= 5
                    && self.match_length == MIN_MATCH
                    && self.strstart.wrapping_sub(self.match_start) > TOO_FAR
                {
                    self.match_length = MIN_MATCH - 1;
                }
            }
            if self.prev_length >= MIN_MATCH && self.match_length <= self.prev_length {
                let max_insert = self.strstart + self.lookahead - MIN_MATCH;
                let bflush = self.tally_dist(
                    self.strstart - 1 - self.prev_match,
                    self.prev_length - MIN_MATCH,
                );
                self.lookahead -= self.prev_length - 1;
                self.prev_length -= 2;
                loop {
                    self.strstart += 1;
                    if self.strstart <= max_insert {
                        self.insert_string(self.strstart);
                    }
                    self.prev_length -= 1;
                    if self.prev_length == 0 {
                        break;
                    }
                }
                self.match_available = false;
                self.match_length = MIN_MATCH - 1;
                self.strstart += 1;
                if bflush {
                    self.flush_block_only(false);
                }
            } else if self.match_available {
                let c = self.window[self.strstart - 1];
                if self.tally_lit(c) {
                    self.flush_block_only(false);
                }
                self.strstart += 1;
                self.lookahead -= 1;
            } else {
                self.match_available = true;
                self.strstart += 1;
                self.lookahead -= 1;
            }
        }
        if self.match_available {
            let c = self.window[self.strstart - 1];
            self.tally_lit(c);
            self.match_available = false;
        }
        self.insert = if self.strstart < MIN_MATCH - 1 {
            self.strstart
        } else {
            MIN_MATCH - 1
        };
        if finish {
            self.flush_block_only(true);
            return Block::FinishDone;
        }
        if self.sym_next != 0 {
            self.flush_block_only(false);
        }
        Block::Done
    }
}

/// Which tree `send_code` reads.
#[derive(Clone, Copy)]
enum Tree {
    StaticL,
    StaticD,
    DynL,
    DynD,
    Bl,
}

/// Adler-32 (RFC 1950), continued from `a`.
#[must_use]
pub fn adler32(a: u32, data: &[u8]) -> u32 {
    let (mut s1, mut s2) = (a & 0xffff, a >> 16);
    for chunk in data.chunks(5552) {
        for &c in chunk {
            s1 += u32::from(c);
            s2 += s1;
        }
        s1 %= 65521;
        s2 %= 65521;
    }
    (s2 << 16) | s1
}

/// `data` compressed by zlib at `level` (1–9), as `compress2` and
/// pdfTeX's `writezip` make it: the zlib header, the deflate stream, the
/// Adler-32 of the data.
///
/// # Panics
///
/// Never: its checked conversions hold zlib's own invariants (positions
/// within the 64K window, codes of at most 15 bits).
#[must_use]
pub fn compress(data: &[u8], level: i32) -> Vec<u8> {
    let level = level.clamp(1, 9).unsigned_abs() as usize;
    let (good, lazy, nice, chain) = CONFIG[level];
    let mut s = State {
        st: Static::new(),
        level,
        input: data,
        next_in: 0,
        adler: 1,
        out: Vec::with_capacity(data.len() / 2 + 64),
        window: vec![0; WINDOW_SIZE + MAX_MATCH],
        prev: vec![0; W_SIZE],
        head: vec![0; HASH_SIZE],
        ins_h: 0,
        block_start: 0,
        match_length: MIN_MATCH - 1,
        prev_match: 0,
        match_available: false,
        strstart: 0,
        match_start: 0,
        lookahead: 0,
        prev_length: MIN_MATCH - 1,
        max_chain_length: chain,
        max_lazy_match: lazy,
        good_match: good,
        nice_match: nice,
        dyn_ltree: [Ct::default(); HEAP_SIZE],
        dyn_dtree: [Ct::default(); 2 * D_CODES + 1],
        bl_tree: [Ct::default(); 2 * BL_CODES + 1],
        l_max_code: 0,
        d_max_code: 0,
        bl_max_code: 0,
        bl_count: [0; MAX_BITS + 1],
        heap: [0; 2 * L_CODES + 1],
        heap_len: 0,
        heap_max: 0,
        depth: [0; 2 * L_CODES + 1],
        sym_buf: vec![0; LIT_BUFSIZE * 4],
        sym_next: 0,
        sym_end: (LIT_BUFSIZE - 1) * 3,
        opt_len: 0,
        static_len: 0,
        insert: 0,
        bi_buf: 0,
        bi_valid: 0,
    };
    s.init_block();
    // (the zlib header: deflate, a 32K window, the level's flags)
    let level_flags = if level < 2 {
        0
    } else if level < 6 {
        1
    } else if level == 6 {
        2
    } else {
        3
    };
    let mut header: u16 = (8 + ((15 - 8) << 4)) << 8;
    header |= level_flags << 6;
    header += 31 - header % 31;
    s.out.extend_from_slice(&header.to_be_bytes());
    let _ = if s.level <= 3 {
        s.deflate_fast(true)
    } else {
        s.deflate_slow(true)
    };
    let a = s.adler;
    s.out.extend(a.to_be_bytes());
    s.out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adler() {
        assert_eq!(adler32(1, b"Wikipedia"), 0x11E6_0398);
    }

    #[test]
    fn empty_and_small() {
        // (`python3 -c "import zlib; print(zlib.compress(b'', 9).hex())"`)
        assert_eq!(
            compress(b"", 9),
            [0x78, 0xda, 0x03, 0x00, 0x00, 0x00, 0x00, 0x01]
        );
        assert_eq!(
            compress(b"hello hello hello", 9),
            [
                120, 218, 203, 72, 205, 201, 201, 87, 200, 64, 144, 0, 58, 46, 6, 125
            ]
        );
    }

    /// Every `/FlateDecode` stream pdfTeX wrote into the output tests'
    /// fixtures (zlib at `\pdfcompresslevel` 9) is reproduced byte for byte.
    #[test]
    fn as_pdftex_zlib() {
        // (`hi9.pdf` is the fixture pdfTeX compressed: its page and its
        // embedded Type 1 font.)
        let pdf: &[u8] = include_bytes!("../../output/pdf/tests/fixtures/hi9.pdf");
        let mut checked = 0;
        let mut at = 0;
        while let Some(k) = find(&pdf[at..], b"stream\n") {
            let start = at + k + 7;
            let Some(len) = find(&pdf[start..], b"\nendstream") else {
                break;
            };
            let z = &pdf[start..start + len];
            at = start + len + b"\nendstream".len();
            if z.starts_with(&[0x78, 0xda]) {
                let data = crate::inflate::inflate(z).expect("a zlib stream");
                assert!(compress(&data, 9) == z, "stream at byte {start}");
                checked += 1;
            }
        }
        assert!(checked == 2, "{checked} streams");
    }

    fn find(h: &[u8], n: &[u8]) -> Option<usize> {
        h.windows(n.len()).position(|w| w == n)
    }
}
