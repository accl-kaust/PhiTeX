//! `utils.c`'s color stacks (`\pdfcolorstackinit`, `\pdfcolorstack`) and
//! the stacks of `\pdfsave` positions and `\pdfsetmatrix` matrices kept
//! while a page or form is shipped.

use crate::LiteralMode;

/// `COLOR_DEFAULT`.
const COLOR_DEFAULT: &[u8] = b"0 g 0 G";
/// `MAX_COLORSTACKS`.
const MAX_COLORSTACKS: usize = 32768;

/// One `colstack_type`; `None` is a null string.
#[derive(Clone, Debug)]
struct ColorStack {
    page_stack: Vec<Option<Vec<u8>>>,
    form_stack: Vec<Option<Vec<u8>>>,
    page_current: Option<Vec<u8>>,
    form_current: Option<Vec<u8>>,
    literal_mode: LiteralMode,
    page_start: bool,
}

/// The color stacks and the position and matrix stacks.
#[derive(Clone, Default)]
pub(crate) struct Stacks {
    /// Empty until first used (`colstacks_first_init`).
    list: Vec<ColorStack>,
    /// `page_mode`: shipping a page, not a form.
    pub page_mode: bool,
    /// `pos_stack`: `\pdfsave` positions and the matrix stack's depth.
    pos: Vec<(i32, i32, usize)>,
    /// `matrix_stack`.
    matrix: Vec<[f64; 6]>,
    /// The rectangle `matrixtransformrect` last saw (`last_llx`, ...).
    last_rect: [i32; 4],
    /// Each color stack's page stack depth as the page began
    /// ([`Stacks::ship_begin`]), and whether the page popped an entry
    /// from below it: one that did not read nothing of the stacks below,
    /// so a replay may place it on deeper ones
    /// (`PdfState::replay_page`).
    floor: Vec<usize>,
    pub(crate) below: bool,
}

/// The stacks as the state's digest and keys see them: the page's own
/// bookkeeping ([`Stacks::below`]) is read by nothing after the page.
impl std::fmt::Debug for Stacks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Stacks")
            .field("list", &self.list)
            .field("page_mode", &self.page_mode)
            .field("pos", &self.pos)
            .field("matrix", &self.matrix)
            .field("last_rect", &self.last_rect)
            .finish()
    }
}

fn nonempty(s: &[u8]) -> Option<Vec<u8>> {
    (!s.is_empty()).then(|| s.to_vec())
}

impl Stacks {
    /// `colstacks_first_init`.
    fn init(&mut self) {
        if self.list.is_empty() {
            let d = Some(COLOR_DEFAULT.to_vec());
            self.list.push(ColorStack {
                page_stack: Vec::new(),
                form_stack: Vec::new(),
                page_current: d.clone(),
                form_current: d,
                literal_mode: LiteralMode::DirectAlways,
                page_start: true,
            });
        }
    }

    /// `colorstackused`.
    pub(crate) fn used(&mut self) -> usize {
        self.init();
        self.list.len()
    }

    /// `newcolorstack`: the new stack's number, `None` when there are too
    /// many.
    pub(crate) fn new_stack(
        &mut self,
        s: &[u8],
        literal_mode: LiteralMode,
        page_start: bool,
    ) -> Option<usize> {
        self.init();
        if self.list.len() == MAX_COLORSTACKS {
            return None;
        }
        let v = nonempty(s);
        self.list.push(ColorStack {
            page_stack: Vec::new(),
            form_stack: Vec::new(),
            page_current: v.clone(),
            form_current: v,
            literal_mode,
            page_start,
        });
        Some(self.list.len() - 1)
    }

    /// `colorstackset`: the literal mode.
    pub(crate) fn set(&mut self, n: usize, s: &[u8]) -> LiteralMode {
        let page = self.page_mode;
        let c = &mut self.list[n];
        // `xstrdup(makecstring(s))`: an empty string, not a null one
        if page {
            c.page_current = Some(s.to_vec());
        } else {
            c.form_current = Some(s.to_vec());
        }
        c.literal_mode
    }

    /// `colorstackcurrent`: the literal mode and the current value.
    pub(crate) fn current(&mut self, n: usize) -> (LiteralMode, Vec<u8>) {
        let page = self.page_mode;
        let c = &self.list[n];
        let v = if page {
            &c.page_current
        } else {
            &c.form_current
        };
        (c.literal_mode, v.clone().unwrap_or_default())
    }

    /// `colorstackpush`.
    pub(crate) fn push(&mut self, n: usize, s: &[u8]) -> LiteralMode {
        let page = self.page_mode;
        let c = &mut self.list[n];
        if page {
            let old = std::mem::replace(&mut c.page_current, nonempty(s));
            c.page_stack.push(old);
        } else {
            let old = std::mem::replace(&mut c.form_current, nonempty(s));
            c.form_stack.push(old);
        }
        c.literal_mode
    }

    /// `colorstackpop`: the literal mode and the value restored, nothing
    /// for an empty stack ("pop empty color page stack").
    pub(crate) fn pop(&mut self, n: usize) -> (LiteralMode, Vec<u8>) {
        let page = self.page_mode;
        let c = &mut self.list[n];
        let (stack, current) = if page {
            (&mut c.page_stack, &mut c.page_current)
        } else {
            (&mut c.form_stack, &mut c.form_current)
        };
        if page && stack.len() <= self.floor.get(n).copied().unwrap_or(0) {
            self.below = true;
        }
        match stack.pop() {
            None => (c.literal_mode, Vec::new()),
            Some(v) => {
                *current = v;
                (c.literal_mode, current.clone().unwrap_or_default())
            }
        }
    }

    /// `colorstackskippagestart`.
    pub(crate) fn skip_page_start(&self, n: usize) -> i32 {
        let c = &self.list[n];
        if !c.page_start {
            return 1;
        }
        match &c.page_current {
            Some(v) if v == COLOR_DEFAULT => 2,
            _ => 0,
        }
    }

    /// `pdfshipoutbegin`: its `colorstackpagestart` is only called for a
    /// page, where it returns at once (`page_mode` is already set), so
    /// the form values are never reset.
    pub(crate) fn ship_begin(&mut self, shipping_page: bool) {
        self.pos.clear();
        self.page_mode = shipping_page;
        if shipping_page {
            self.floor = self.list.iter().map(|c| c.page_stack.len()).collect();
            self.below = false;
        }
    }

    /// The stacks with their page stacks emptied: what a page that pops
    /// nothing from below its start reads of them ([`Stacks::below`]).
    pub(crate) fn without_page_stacks(&self) -> Stacks {
        let mut s = self.clone();
        for c in &mut s.list {
            c.page_stack.clear();
        }
        s
    }

    /// Each color stack's page stack.
    pub(crate) fn page_stacks(&self) -> Vec<Vec<Option<Vec<u8>>>> {
        self.list.iter().map(|c| c.page_stack.clone()).collect()
    }

    /// Each color stack's page stack depth.
    pub(crate) fn page_depths(&self) -> Vec<usize> {
        self.list.iter().map(|c| c.page_stack.len()).collect()
    }

    /// The page stacks a page left, its own entries (those above `from`,
    /// its start's depths) put on `under` (the stacks it was placed on).
    pub(crate) fn rebase_page(&mut self, under: &[Vec<Option<Vec<u8>>>], from: &[usize]) {
        for (i, c) in self.list.iter_mut().enumerate() {
            let k = from.get(i).copied().unwrap_or(0).min(c.page_stack.len());
            let mut s = under.get(i).cloned().unwrap_or_default();
            s.extend(c.page_stack.drain(k..));
            c.page_stack = s;
        }
    }

    /// `pdfshipoutend`: the number of unmatched `\pdfsave`s, an error.
    pub(crate) fn unmatched_saves(&self) -> usize {
        self.pos.len()
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

    /// `checkpdfrestore` ("missing \pdfsave" and "Misplaced \pdfrestore"
    /// are warnings).
    pub(crate) fn restore(&mut self) {
        let Some((_, _, depth)) = self.pos.pop() else {
            return;
        };
        if self.page_mode {
            self.matrix.truncate(depth);
        }
    }

    /// `pdfsetmatrix`: false if the text is not four numbers.
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
        let z = if let Some(y) = self.matrix.last() {
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
        self.matrix.push(z);
        true
    }

    /// `do_matrixtransform`.
    fn transform(&self, x: i32, y: i32) -> (i32, i32) {
        let m = self
            .matrix
            .last()
            .copied()
            .unwrap_or([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
        let (x, y) = (f64::from(x), f64::from(y));
        #[allow(clippy::cast_possible_truncation)]
        let round = |v: f64| (if v > 0.0 { v + 0.5 } else { v - 0.5 }) as i32;
        (
            round(x * m[0] + y * m[2] + m[4]),
            round(x * m[1] + y * m[3] + m[5]),
        )
    }

    /// `matrixtransformrect`: the rectangle (llx, lly, urx, ury) as the
    /// current matrix moves it.
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

/// `sscanf(s, " %lf %lf %lf %lf %c") == 4`: four numbers and nothing
/// else.
fn scan_four_doubles(s: &[u8]) -> Option<[f64; 4]> {
    let text = std::str::from_utf8(s).ok()?;
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
    if w.is_empty() || w == "-" {
        return None;
    }
    w.parse().ok()
}
