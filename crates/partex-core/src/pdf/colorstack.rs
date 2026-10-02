//! utils.c's color stacks (`\pdfcolorstackinit`, `\pdfcolorstack`) and
//! the `\pdfsave`/`\pdfsetmatrix` stacks used while shipping out.

use alloc::vec::Vec;

/// utils.c's `COLOR_DEFAULT`.
const COLOR_DEFAULT: &[u8] = b"0 g 0 G";
/// utils.c's `MAX_COLORSTACKS`.
const MAX_COLORSTACKS: usize = 32768;

/// One `colstack_type`. `None` is a null string.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ColorStack {
    pub page_stack: Vec<Option<Vec<u8>>>,
    pub form_stack: Vec<Option<Vec<u8>>>,
    pub page_current: Option<Vec<u8>>,
    pub form_current: Option<Vec<u8>>,
    pub form_init: Option<Vec<u8>>,
    pub literal_mode: i32,
    pub page_start: bool,
}

partex_engine::persist_struct!(ColorStack {
    page_stack,
    form_stack,
    page_current,
    form_current,
    form_init,
    literal_mode,
    page_start
});

super::val::record_by_hash!(Stacks);

/// The color stacks and the position and matrix stacks (the `STACKS`
/// field).
#[derive(Clone, Debug, Default, PartialEq, Hash)]
pub(crate) struct Stacks {
    /// Empty until first used (`colstacks_first_init`).
    list: Vec<ColorStack>,
    /// `page_mode`: shipping a page (not a form).
    pub page_mode: bool,
    /// `pos_stack`: `\pdfsave` positions and matrix stack depths.
    pub pos: Vec<(i32, i32, usize)>,
    /// `matrix_stack` (entries as their bits, for hashing).
    matrix: Vec<[u64; 6]>,
    /// The rectangle `matrixtransformrect` last saw.
    last_rect: [i32; 4],
}

partex_engine::persist_struct!(Stacks {
    list,
    page_mode,
    pos,
    matrix,
    last_rect
});

fn nonempty(s: &[u8]) -> Option<Vec<u8>> {
    (!s.is_empty()).then(|| s.to_vec())
}

impl Stacks {
    fn init(&mut self) {
        if self.list.is_empty() {
            let d = Some(COLOR_DEFAULT.to_vec());
            self.list.push(ColorStack {
                page_stack: Vec::new(),
                form_stack: Vec::new(),
                page_current: d.clone(),
                form_current: d.clone(),
                form_init: d,
                literal_mode: super::DIRECT_ALWAYS,
                page_start: true,
            });
        }
    }

    /// `colorstackused`.
    pub(crate) fn used(&mut self) -> usize {
        self.init();
        self.list.len()
    }

    /// `newcolorstack`: the new stack's number, or `None` if there are
    /// too many.
    pub(crate) fn new_stack(
        &mut self,
        s: &[u8],
        literal_mode: i32,
        page_start: bool,
    ) -> Option<i32> {
        self.init();
        if self.list.len() == MAX_COLORSTACKS {
            return None;
        }
        let v = nonempty(s);
        self.list.push(ColorStack {
            page_stack: Vec::new(),
            form_stack: Vec::new(),
            page_current: v.clone(),
            form_current: v.clone(),
            form_init: v,
            literal_mode,
            page_start,
        });
        i32::try_from(self.list.len() - 1).ok()
    }

    fn get(&mut self, n: i32) -> &mut ColorStack {
        &mut self.list[usize::try_from(n).unwrap_or(0)]
    }

    /// `colorstackset`: the literal mode.
    pub(crate) fn set(&mut self, n: i32, s: &[u8]) -> i32 {
        let page = self.page_mode;
        let c = self.get(n);
        // (`xstrdup(makecstring(s))`: an empty string, not null)
        if page {
            c.page_current = Some(s.to_vec());
        } else {
            c.form_current = Some(s.to_vec());
        }
        c.literal_mode
    }

    /// `colorstackcurrent`: the literal mode and the current value.
    pub(crate) fn current(&mut self, n: i32) -> (i32, Vec<u8>) {
        let page = self.page_mode;
        let c = self.get(n);
        let v = if page {
            &c.page_current
        } else {
            &c.form_current
        };
        (c.literal_mode, v.clone().unwrap_or_default())
    }

    /// `colorstackpush`.
    pub(crate) fn push(&mut self, n: i32, s: &[u8]) -> i32 {
        let page = self.page_mode;
        let c = self.get(n);
        if page {
            let old = core::mem::replace(&mut c.page_current, nonempty(s));
            c.page_stack.push(old);
        } else {
            let old = core::mem::replace(&mut c.form_current, nonempty(s));
            c.form_stack.push(old);
        }
        c.literal_mode
    }

    /// `colorstackpop`: the literal mode and the restored value, or
    /// `None` (with the mode) if the stack is empty.
    pub(crate) fn pop(&mut self, n: i32) -> (i32, Option<Vec<u8>>) {
        let page = self.page_mode;
        let c = self.get(n);
        let (stack, current) = if page {
            (&mut c.page_stack, &mut c.page_current)
        } else {
            (&mut c.form_stack, &mut c.form_current)
        };
        match stack.pop() {
            None => (c.literal_mode, None),
            Some(v) => {
                *current = v;
                (c.literal_mode, Some(current.clone().unwrap_or_default()))
            }
        }
    }

    /// `colorstackskippagestart`.
    pub(crate) fn skip_page_start(&mut self, n: i32) -> i32 {
        let c = self.get(n);
        if !c.page_start {
            return 1;
        }
        match &c.page_current {
            Some(v) if v == COLOR_DEFAULT => 2,
            _ => 0,
        }
    }

    /// `pdfshipoutbegin` (its `colorstackpagestart` returns at once: it
    /// is only called when shipping a page, and does nothing for pages).
    pub(crate) fn ship_begin(&mut self, shipping_page: bool) {
        self.pos.clear();
        self.page_mode = shipping_page;
    }

    /// `matrixused`.
    pub(crate) fn matrix_used(&self) -> bool {
        !self.matrix.is_empty()
    }

    /// `checkpdfsave`.
    pub(crate) fn save(&mut self, h: i32, v: i32) {
        let depth = if self.page_mode { self.matrix.len() } else { 0 };
        self.pos.push((h, v, depth));
    }

    /// `checkpdfrestore`: the warning, if any.
    pub(crate) fn restore(&mut self, h: i32, v: i32) -> Option<Vec<u8>> {
        let Some((ph, pv, depth)) = self.pos.pop() else {
            return Some(b"\\pdfrestore: missing \\pdfsave".to_vec());
        };
        if self.page_mode {
            self.matrix.truncate(depth);
        }
        let (dh, dv) = (h.wrapping_sub(ph), v.wrapping_sub(pv));
        // (`%usp`: C prints the differences as unsigned)
        (dh != 0 || dv != 0).then(|| {
            alloc::format!(
                "Misplaced \\pdfrestore by ({}sp, {}sp)",
                dh.cast_unsigned(),
                dv.cast_unsigned()
            )
            .into_bytes()
        })
    }

    /// `pdfsetmatrix`: false if the text is no matrix.
    pub(crate) fn set_matrix(&mut self, text: &[u8], h: i32, v: i32) -> bool {
        if !self.page_mode {
            return true;
        }
        let Some([a, b, c, d]) = scan_four_doubles(text) else {
            return false;
        };
        let (hf, vf) = (f64::from(h), f64::from(v));
        let e = hf * (1.0 - a) - vf * c;
        let f = vf * (1.0 - d) - hf * b;
        let z = if let Some(y) = self.matrix.last().map(|m| m.map(f64::from_bits)) {
            [
                a * y[0] + b * y[2],
                a * y[1] + b * y[3],
                c * y[0] + d * y[2],
                c * y[1] + d * y[3],
                e * y[0] + f * y[2] + y[4],
                e * y[1] + f * y[3] + y[5],
            ]
        } else {
            [a, b, c, d, e, f]
        };
        self.matrix.push(z.map(f64::to_bits));
        true
    }

    fn transform(&self, x: i32, y: i32) -> (i32, i32) {
        let m = self
            .matrix
            .last()
            .map_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0], |m| m.map(f64::from_bits));
        let (x, y) = (f64::from(x), f64::from(y));
        let round = |v: f64| {
            #[expect(clippy::cast_possible_truncation, reason = "C's (scaled) cast")]
            let r = if v > 0.0 { v + 0.5 } else { v - 0.5 } as i32;
            r
        };
        (
            round(x * m[0] + y * m[2] + m[4]),
            round(x * m[1] + y * m[3] + m[5]),
        )
    }

    /// `matrixtransformrect`: the transformed rectangle (llx, lly, urx,
    /// ury).
    pub(crate) fn transform_rect(&mut self, llx: i32, lly: i32, urx: i32, ury: i32) -> [i32; 4] {
        if !(self.page_mode && self.matrix_used()) {
            return [llx, lly, urx, ury];
        }
        self.last_rect = [llx, lly, urx, ury];
        let p = [
            self.transform(llx, lly),
            self.transform(llx, ury),
            self.transform(urx, lly),
            self.transform(urx, ury),
        ];
        let xs = p.map(|q| q.0);
        let ys = p.map(|q| q.1);
        [
            xs.into_iter().min().unwrap_or(0),
            ys.into_iter().min().unwrap_or(0),
            xs.into_iter().max().unwrap_or(0),
            ys.into_iter().max().unwrap_or(0),
        ]
    }

    /// `matrixrecalculate`.
    pub(crate) fn recalculate(&mut self, urx: i32) -> [i32; 4] {
        let [llx, lly, _, ury] = self.last_rect;
        self.transform_rect(llx, lly, urx, ury)
    }
}

/// `sscanf(" %lf %lf %lf %lf %c") == 4`: four numbers and nothing else.
fn scan_four_doubles(s: &[u8]) -> Option<[f64; 4]> {
    let text = core::str::from_utf8(s).ok()?;
    let mut words = text.split_ascii_whitespace();
    let mut v = [0.0; 4];
    for x in &mut v {
        *x = strtod(words.next()?)?;
    }
    words.next().is_none().then_some(v)
}

/// C's `strtod` of a whole word.
fn strtod(w: &str) -> Option<f64> {
    let w = w.strip_prefix('+').unwrap_or(w);
    if w.is_empty() || w.starts_with(['+', '-']) && w.len() == 1 {
        return None;
    }
    w.parse().ok()
}
