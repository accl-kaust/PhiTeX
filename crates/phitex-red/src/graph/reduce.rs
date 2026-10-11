// Reducing one thunk to one node; the Ctx implementation (included into graph.rs: one module).
/// The read recorder (split from `Reducing` so the tokenizer's catcode
/// callback can borrow it beside the env).
#[derive(Default)]
struct Rec {
    reads: Vec<(Key, Val)>,
    read_set: HashSet<Key>,
    written: HashSet<Key>,
    written_list: HashSet<(u32, u8)>,
    g_opened: u32,
    g_popped: u32,
    l_opened: u32,
    l_popped: u32,
}

impl Rec {
    fn record(&mut self, k: Key, v: &Val) {
        if self.read_set.insert(k.clone()) {
            self.reads.push((k, v.clone()));
        }
    }
    fn get(&mut self, env: &Env, k: &Key) -> Val {
        let v = env.lookup(k);
        if let Some(w) = list_which(k) {
            if self.l_opened == 0 && !self.written_list.contains(&(self.l_popped, w)) {
                let rk = if self.l_popped == 0 {
                    k.clone()
                } else {
                    Key::Other(pk::TAG, pk::LIST_KEY + 4 * self.l_popped + u32::from(w))
                };
                self.record(rk, &v);
            }
            return v;
        }
        if !self.written.contains(k) {
            self.record(k.clone(), &v);
        }
        v
    }
}

pub struct Reducing<'g> {
    g: &'g Graph,
    start: Thunk,
    env: Env,
    input: Input,
    rec: Rec,
    spans: Vec<Span>,
    emits: Vec<Emit>,
    delta: Vec<Change>,
    /// Per-reduction answers (speculation's snapshot) over the graph's.
    over: Option<&'g dyn Answers>,
    /// Per-reduction cancel flag (a speculative chain's).
    cancel: Option<&'g AtomicBool>,
}

impl<'g> Reducing<'g> {
    fn stopped(&self) -> bool {
        self.g.halted() || self.cancel.is_some_and(|c| c.load(Ordering::Relaxed))
    }

    pub fn new(g: &'g Graph, t: &Thunk) -> Reducing<'g> {
        Reducing {
            g,
            start: t.clone(),
            env: t.env.clone(),
            input: t.input.clone(),
            rec: Rec::default(),
            spans: Vec::new(),
            emits: Vec::new(),
            delta: Vec::new(),
            over: None,
            cancel: None,
        }
    }

    fn start_at(&self) -> NodeId {
        self.start.env.inner.at
    }

    fn change(&mut self, c: Change) {
        let mut r = (*self.env.inner).clone();
        r.apply_change(&c);
        self.env = Env { inner: Arc::new(r) };
        self.delta.push(c);
    }

    /// One raw token with its span.
    fn take(&mut self) -> Option<(Tok, Option<Span>)> {
        if self.stopped() {
            return None;
        }
        let names = self.g.names.clone();
        let Reducing { env, rec, input, .. } = self;
        let mut f = |k: &Key| rec.get(env, k);
        match crate::tokenize::next_token(&mut f, &names, input) {
            Some((t, inp, sp)) => {
                self.input = inp;
                if let Some(s) = sp {
                    merge_span(&mut self.spans, s);
                }
                Some((t, sp))
            }
            None => {
                self.input = Input::default();
                None
            }
        }
    }

    fn relax_op(&self) -> Op {
        match self.g.relax {
            Some(id) => Op::Command(id),
            None => Op::Chars,
        }
    }

    fn tok_text(&self, t: &Tok) -> String {
        match t {
            Tok::Cs(c) => format!("\\{}", self.g.names.name(*c)),
            Tok::Active(c) => c.to_string(),
            Tok::Char { ch, .. } => ch.to_string(),
            Tok::Param(_) => "#".into(),
            Tok::DontExpand => "\\notexpanded:".into(),
        }
    }

    fn mode_name(m: Mode) -> &'static str {
        match m {
            Mode::Vertical { inner: false } => "vertical mode",
            Mode::Vertical { inner: true } => "internal vertical mode",
            Mode::Horizontal { inner: false } => "horizontal mode",
            Mode::Horizontal { inner: true } => "restricted horizontal mode",
            Mode::Math { display: true } => "display math mode",
            Mode::Math { display: false } => "math mode",
        }
    }

    fn expandable(&mut self, t: &Tok) -> bool {
        match self.meaning(t) {
            Meaning::Macro(_) => true,
            Meaning::Undefined => matches!(t, Tok::Cs(_) | Tok::Active(_)),
            Meaning::Prim(id) => {
                matches!(crate::prim::registry().get(id).kind, Kind::Expand(_) | Kind::If(_))
            }
            _ => false,
        }
    }

    fn undefined(&mut self) {
        crate::diag::error(
            self,
            "Undefined control sequence",
            &[
                "The control sequence at the end of the top line",
                "of your error message was never \\def'ed. If you have",
                "misspelled it (e.g., `\\hobx'), type `I' and the correct",
                "spelling (e.g., `I\\hbox'). Otherwise just continue,",
                "and I'll forget about whatever was undefined.",
            ],
        );
    }

    fn insert_dollar(&mut self, t: Tok) {
        self.back(t);
        self.push(Arc::from(vec![Tok::Char { cat: 3, ch: '$' }]), TokKind::Inserted);
        crate::diag::error(
            self,
            "Missing $ inserted",
            &["I've inserted a begin-math/end-math symbol since I think", "you left one out. Proceed, with fingers crossed."],
        );
    }

    /// main_control's dispatch for one token: the op of the node it makes.
    pub fn step(&mut self) -> Op {
        let Some(mut t) = self.next() else { return Op::Chars };
        if t == Tok::DontExpand {
            let Some(t2) = self.next() else { return Op::Chars };
            if self.expandable(&t2) {
                // `\noexpand`ed: means `\relax` here.
                return self.relax_op();
            }
            t = t2;
        }
        match self.meaning(&t) {
            Meaning::Macro(f) => self.apply(t, f),
            Meaning::Prim(id) => {
                let d = *crate::prim::registry().get(id);
                match d.kind {
                    Kind::Expand(f) => {
                        f(self, d.code);
                        Op::Expand(id)
                    }
                    Kind::If(_) => {
                        let (taken, untaken) = crate::cond::begin_if(self, id, d.code);
                        Op::Select { taken, untaken }
                    }
                    Kind::Command(f) => {
                        f(self, d.code);
                        Op::Command(id)
                    }
                    Kind::Quantity { .. } | Kind::Computed { .. } => {
                        crate::assign::prefixed_command(self, id, 0);
                        Op::Command(id)
                    }
                    Kind::Prefix => {
                        self.prefixes(d.code as u8);
                        Op::Command(id)
                    }
                }
            }
            Meaning::Undefined => match t {
                Tok::Cs(_) | Tok::Active(_) => {
                    self.undefined();
                    self.relax_op()
                }
                Tok::Char { cat, .. } => self.char_tok(t, cat),
                Tok::Param(_) => self.char_tok(t, 6),
                Tok::DontExpand => Op::Chars,
            },
            Meaning::Char { cat, .. } => self.char_tok(t, cat),
            Meaning::CharGiven(_) => {
                match self.mode() {
                    Mode::Vertical { .. } => {
                        self.back(t);
                        crate::list::new_graf(self, true);
                    }
                    _ => crate::list::char_tok(self, t),
                }
                Op::Chars
            }
            Meaning::MathCharGiven(v) => {
                match self.mode() {
                    Mode::Math { .. } => crate::math::math_given(self, v),
                    _ => self.insert_dollar(t),
                }
                Op::Chars
            }
            Meaning::Reg(..) | Meaning::Font(_) => {
                crate::assign::assign_meaning(self, t, 0);
                self.relax_op()
            }
        }
    }

    /// `\global` & co: gather prefixes, then the assignment they apply to.
    fn prefixes(&mut self, first: u8) {
        let mut p = first;
        loop {
            let Some(t) = self.next_x() else { return };
            if matches!(t, Tok::Char { cat: 10, .. }) {
                continue;
            }
            match self.meaning(&t) {
                Meaning::Prim(id) => {
                    let d = *crate::prim::registry().get(id);
                    if matches!(d.kind, Kind::Prefix) {
                        p |= d.code as u8;
                        continue;
                    }
                    if Some(id) == self.g.relax {
                        continue;
                    }
                    crate::assign::prefixed_command(self, id, p);
                    return;
                }
                Meaning::Reg(..) | Meaning::Font(_) => {
                    crate::assign::assign_meaning(self, t, p);
                    return;
                }
                _ => {
                    let msg = format!("You can't use a prefix with `{}'", self.tok_text(&t));
                    let help: &[&str] = if self.g.etex {
                        &["I'll pretend you didn't say \\long or \\outer or \\global or \\protected."]
                    } else {
                        &["I'll pretend you didn't say \\long or \\outer or \\global."]
                    };
                    self.back(t);
                    crate::diag::error(self, &msg, help);
                    return;
                }
            }
        }
    }

    /// A macro call at the top: its own `Apply` node.
    fn apply(&mut self, t: Tok, f: Arc<crate::val::Macro>) -> Op {
        match crate::macros::match_args(self, t, &f) {
            Some(args) => {
                let args: Arc<[Toks]> = args.into();
                let body = self.g.memo.apply(&f, &args, || crate::macros::substitute(&f.body, &args));
                self.push(body, TokKind::Macro);
                Op::Apply { f, args }
            }
            None => Op::Apply { f, args: Arc::from(Vec::<Toks>::new()) },
        }
    }

    /// A character token (or a `\let` copy of one) at the top.
    fn char_tok(&mut self, t: Tok, cat: u8) -> Op {
        let mode = self.mode();
        match cat {
            1 => crate::groups::left_brace(self),
            2 => crate::groups::right_brace(self),
            3 => match mode {
                Mode::Vertical { .. } => {
                    self.back(t);
                    crate::list::new_graf(self, true);
                }
                _ => crate::math::math_shift(self, t),
            },
            4 => crate::align::tab_mark(self, t),
            6 => {
                let msg = format!("You can't use `macro parameter character #' in {}", Self::mode_name(mode));
                crate::diag::error(self, &msg, &["Sorry, but I'm not programmed to handle this case;", "I'll just pretend that you didn't ask for it."]);
            }
            7 | 8 => match mode {
                Mode::Math { .. } => crate::math::sub_sup(self, t, cat == 7),
                _ => self.insert_dollar(t),
            },
            10 => {
                if let Mode::Horizontal { .. } = mode {
                    crate::list::char_tok(self, t);
                }
            }
            11 | 12 => match mode {
                Mode::Vertical { .. } => {
                    self.back(t);
                    crate::list::new_graf(self, true);
                }
                _ => crate::list::char_tok(self, t),
            },
            _ => {}
        }
        Op::Chars
    }

    /// The node: id, next thunk.
    pub fn finish(self, op: Op) -> Arc<Node> {
        let g = self.g;
        let mut a = g.arena.write().unwrap();
        let id = a.nodes.len() as NodeId;
        let n = Arc::new(Node {
            op,
            spans: self.spans,
            reads: self.rec.reads,
            out: Out { delta: Delta(self.delta), emits: self.emits },
            next: Thunk { env: env_at(&self.env, id), input: self.input },
        });
        a.nodes.push(n.clone());
        a.prev.push(self.start.env.inner.at);
        n
    }

    // ---- lists

    /// The node's own emits that belong to the current list: (the list was
    /// opened by this node, the segment).
    fn own_segment(&self) -> (bool, Vec<Seg>) {
        let mut d = 0u32;
        let mut open_at = None;
        for (i, e) in self.emits.iter().enumerate().rev() {
            match e {
                Emit::Close => d += 1,
                Emit::Open(_) => {
                    if d == 0 {
                        open_at = Some(i);
                        break;
                    }
                    d -= 1;
                }
                _ => {}
            }
        }
        let (from, target) = match open_at {
            Some(i) => (i + 1, 0),
            None => (0, self.rec.l_popped),
        };
        let mut seg = Vec::new();
        let (mut pos, mut depth) = (0u32, 0u32);
        for e in &self.emits[from..] {
            match e {
                Emit::Open(_) => depth += 1,
                Emit::Close => {
                    if depth > 0 {
                        depth -= 1;
                    } else {
                        pos += 1;
                    }
                }
                Emit::Item(it) if depth == 0 && pos == target => seg.push(Seg::Item(it.clone())),
                Emit::RemoveTail(k) if depth == 0 && pos == target => seg.push(Seg::Rem(*k)),
                _ => {}
            }
        }
        (open_at.is_some(), seg)
    }

    fn record_ext(&mut self, c: u32, ext: &ItemsDatum) {
        let key = if c == 0 { Key::Tail } else { Key::Other(pk::TAG, pk::CHAIN + c) };
        let v = Val::Data(Arc::new(ext.clone()));
        if let Some(slot) = self.rec.reads.iter_mut().find(|(k, _)| *k == key) {
            let more = match &slot.1 {
                Val::Data(d) => d.as_any().downcast_ref::<ItemsDatum>().is_none_or(|o| {
                    (ext.whole && !o.whole) || (!o.whole && ext.items.len() > o.items.len())
                }),
                _ => true,
            };
            if more {
                slot.1 = v;
            }
        } else {
            self.rec.record(key, &v);
        }
    }

    // ---- reads with a graph answer

    fn line_now(&self) -> i32 {
        line_of(&self.input)
    }
}

impl Ctx for Reducing<'_> {
    fn next(&mut self) -> Option<Tok> {
        self.take().map(|(t, _)| t)
    }

    fn next_x(&mut self) -> Option<Tok> {
        loop {
            let t = self.next()?;
            if t == Tok::DontExpand {
                // The marked token is not expanded (it acts as `\relax` if expandable).
                return self.next();
            }
            match self.meaning(&t) {
                Meaning::Macro(f) => crate::macros::expand_macro(self, t, &f),
                Meaning::Prim(id) => {
                    let d = *crate::prim::registry().get(id);
                    match d.kind {
                        Kind::Expand(f) => f(self, d.code),
                        Kind::If(_) => {
                            crate::cond::begin_if(self, id, d.code);
                        }
                        _ => return Some(t),
                    }
                }
                Meaning::Undefined if matches!(t, Tok::Cs(_) | Tok::Active(_)) => self.undefined(),
                _ => return Some(t),
            }
            if self.stopped() {
                return None;
            }
        }
    }

    fn back(&mut self, t: Tok) {
        self.input = self.input.push(Src::Toks { toks: Arc::from(vec![t]), at: 0, kind: TokKind::Backed });
    }

    fn push(&mut self, toks: Toks, kind: TokKind) {
        if toks.is_empty() && !matches!(kind, TokKind::UTemplate | TokKind::VTemplate) {
            return;
        }
        self.input = self.input.push(Src::Toks { toks, at: 0, kind });
    }

    fn push_text(&mut self, name: Arc<str>, data: Arc<str>) -> TextId {
        let id = self.g.intern_text(name, data.clone());
        self.input = self.input.push(crate::tokenize::text_source(id, data));
        id
    }

    fn end_input(&mut self) {
        let mut above = Vec::new();
        let mut cur = self.input.clone();
        loop {
            match cur.top() {
                None => return,
                Some(Src::Text { .. }) => break,
                Some(s) => {
                    above.push(s.clone());
                    cur = cur.below();
                }
            }
        }
        let Some(Src::Text { text, data, pos, line, line_end, eol, .. }) = cur.top().cloned() else { return };
        let mut inp = cur.below().push(Src::Text { text, data, pos, line, line_end, eol, ending: true });
        for s in above.into_iter().rev() {
            inp = inp.push(s);
        }
        self.input = inp;
    }

    fn reading(&self) -> Option<TokKind> {
        let mut i = self.input.clone();
        loop {
            match i.top() {
                Some(Src::Toks { toks, at, kind }) => {
                    if (*at as usize) < toks.len() {
                        return Some(*kind);
                    }
                    i = i.below();
                }
                _ => return None,
            }
        }
    }

    fn line_no(&mut self) -> i32 {
        let start = line_of(&self.start.input);
        self.rec.record(Key::LineNo, &Val::Int(start));
        self.line_now()
    }

    fn skip_raw(&mut self, stop: &mut dyn FnMut(&Tok, i32) -> bool, nest: &mut dyn FnMut(&Tok) -> i32) -> Vec<Span> {
        let mut out = Vec::new();
        let mut depth = 0i32;
        while let Some((t, sp)) = self.take() {
            if let Some(s) = sp {
                merge_span(&mut out, s);
            }
            if stop(&t, depth) {
                break;
            }
            depth += nest(&t);
        }
        out
    }

    fn get(&mut self, key: &Key) -> Val {
        match key {
            Key::Tail => {
                let t = self.tail();
                Val::Data(Arc::new(ItemsDatum { items: t.into_iter().collect(), whole: false }))
            }
            Key::LineNo => Val::Int(self.line_no()),
            Key::Page(_) | Key::File(_) | Key::JobName | Key::Given(_) | Key::Other(crate::prim::tag::PAGE, _) => {
                if self.rec.written.contains(key) {
                    return self.env.lookup(key);
                }
                let v = match self.over.and_then(|a| a.answer(self.g, key, &self.start)) {
                    Some(v) => v,
                    None => self.g.answer(key, &self.start),
                };
                self.rec.record(key.clone(), &v);
                v
            }
            _ => self.rec.get(&self.env, key),
        }
    }

    fn meaning(&mut self, t: &Tok) -> Meaning {
        let key = match t {
            Tok::Cs(c) => Key::Cs(*c),
            Tok::Active(c) => Key::Active(*c),
            Tok::Char { cat, ch } => return Meaning::Char { cat: *cat, ch: *ch },
            Tok::Param(_) => return Meaning::Char { cat: 6, ch: '#' },
            Tok::DontExpand => return Meaning::Undefined,
        };
        match self.get(&key) {
            Val::Meaning(m) => m,
            _ => Meaning::Undefined,
        }
    }

    fn define(&mut self, key: Key, val: Val, global: bool) {
        if let Some(w) = list_which(&key) {
            if self.rec.l_opened == 0 {
                self.rec.written_list.insert((self.rec.l_popped, w));
            }
        } else {
            self.rec.written.insert(key.clone());
        }
        self.change(Change::Def { key, val, global });
    }

    fn replace(&mut self, key: Key, val: Val) {
        self.rec.written.insert(key.clone());
        self.change(Change::Replace { key, val });
    }

    fn open_group(&mut self, kind: GroupKind) {
        self.rec.g_opened += 1;
        self.change(Change::OpenGroup(kind));
    }

    fn close_group(&mut self) {
        let Some(frame) = self.env.inner.group.clone() else { return };
        if self.rec.g_opened > 0 {
            // Restored entries are values from before the group: read again if used.
            for (k, _) in frame.saves_vec() {
                self.rec.written.remove(&k);
            }
            self.rec.g_opened -= 1;
        } else {
            let c = self.rec.g_popped;
            let key = Key::Other(pk::TAG, pk::FRAME + c);
            let v = self.start.env.lookup(&key);
            if let Val::Data(d) = &v {
                if let Some(fd) = d.as_any().downcast_ref::<FrameDatum>() {
                    for (k, _) in &fd.saves {
                        self.rec.written.insert(k.clone());
                    }
                }
            }
            self.rec.record(key, &v);
            self.rec.g_popped += 1;
        }
        let toks: Toks = Arc::from(frame.after_toks());
        self.change(Change::CloseGroup);
        self.push(toks, TokKind::AfterGroup);
    }

    fn after_group(&mut self, t: Tok) {
        self.change(Change::AfterGroup(t));
    }

    fn group_kind(&mut self) -> GroupKind {
        let k = self.env.group_kind();
        if self.rec.g_opened == 0 {
            self.rec.record(Key::Other(pk::TAG, pk::GROUP_KIND + self.rec.g_popped), &Val::Int(k as i32));
        }
        k
    }

    fn group_depth(&mut self) -> u32 {
        let d = self.env.group_depth();
        let start = d + self.rec.g_popped - self.rec.g_opened;
        self.rec.record(Key::Other(pk::TAG, pk::GROUP_DEPTH), &Val::Int(start as i32));
        d
    }

    fn mode(&mut self) -> Mode {
        let m = self.env.mode();
        if self.rec.l_opened == 0 {
            self.rec.record(Key::Other(pk::TAG, pk::MODE + self.rec.l_popped), &Val::Int(mode_code(m)));
        }
        m
    }

    fn open_list(&mut self, mode: Mode) {
        self.rec.l_opened += 1;
        self.emits.push(Emit::Open(mode));
        self.change(Change::OpenList(mode));
    }

    fn close_list(&mut self) -> Vec<Item> {
        let (internal, seg) = self.own_segment();
        let (own, under) = process(seg);
        let items = if internal {
            own
        } else {
            let c = self.rec.l_popped;
            let ext = self.g.ext_suffix(self.start_at(), c, usize::MAX);
            self.record_ext(c, &ext);
            combine(ext.items.to_vec(), &under, own)
        };
        if self.env.list_depth() == 0 {
            return items;
        }
        self.emits.push(Emit::Close);
        self.change(Change::CloseList);
        if self.rec.l_opened > 0 {
            self.rec.l_opened -= 1;
        } else {
            self.rec.l_popped += 1;
        }
        items
    }

    fn tail(&mut self) -> Option<Item> {
        let (internal, seg) = self.own_segment();
        let (own, under) = process(seg);
        if let Some(l) = own.last() {
            return Some(l.clone());
        }
        if internal {
            return None;
        }
        let c = self.rec.l_popped;
        let ext = self.g.ext_suffix(self.start_at(), c, under.len() + 1);
        self.record_ext(c, &ext);
        combine(ext.items.to_vec(), &under, Vec::new()).last().cloned()
    }

    fn emit(&mut self, e: Emit) {
        self.emits.push(e);
    }

    fn names(&self) -> &Names {
        &self.g.names
    }

    fn file(&mut self, name: &str) -> Option<(Arc<str>, Arc<[u8]>)> {
        let cands: Vec<String> = if std::path::Path::new(name).extension().is_some() {
            vec![name.to_string()]
        } else {
            vec![format!("{name}.tex"), name.to_string()]
        };
        for c in cands {
            let n: Arc<str> = c.as_str().into();
            if let Val::Bytes(b) = self.get(&Key::File(n.clone())) {
                return Some((n, b));
            }
        }
        None
    }

    fn etex(&self) -> bool {
        self.g.etex
    }
    fn pdf(&self) -> bool {
        self.g.pdf
    }
    fn halted(&self) -> bool {
        self.stopped()
    }
}
