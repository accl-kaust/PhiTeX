//! Each control sequence's extent: what it consumes, read by its spec,
//! and how sure that is.
//!
//! The readers are TeX's scanners as far as they need no value: an
//! undelimited argument, a delimited one, an optional `[…]`, a number, a
//! dimen, glue and its keywords. Where TeX would expand a macro to decide
//! (a number's next digit, a glue's `plus`, a keyword), a macro this pass
//! cannot see makes the extent pending.

use super::atoms::{Atom, AtomKind};
use super::defs::{Arg, Found, Lit, Shape, skip};
use super::hints::{
    self, Assumption, Claim, Closure, Effect, Hints, MeaningShape, ScopeHint, ScopeKind,
};
use super::lex::Lexed;
use super::table::{Entry, Table, table};
use super::{Cause, Features, FileKind, Item, Scope, Status};

/// What a name resolves to.
enum Res {
    Cmd(Vec<Arg>, Status),
    /// A quantity: its type, its index, how sure.
    Quantity(char, Option<char>, Status),
    Pending(Cause),
}

fn cause(kind: &'static str, detail: &str) -> Cause {
    Cause {
        kind,
        detail: detail.to_owned(),
    }
}

/// The result of reading: how sure, and why not.
struct R {
    status: Status,
    cause: Option<Cause>,
    /// The causes of every prediction made on the way (their names are
    /// what the predictions rest on).
    noted: Vec<(&'static str, String)>,
}

impl R {
    fn new() -> R {
        R {
            status: Status::Known,
            cause: None,
            noted: Vec::new(),
        }
    }

    fn worse(&mut self, s: Status, c: Option<Cause>) {
        if s == Status::Predicted
            && let Some(c) = &c
            && !c.detail.is_empty()
        {
            self.noted.push((c.kind, c.detail.clone()));
        }
        if s > self.status {
            self.status = s;
            self.cause = c;
        }
    }

    fn pending(&mut self, kind: &'static str, detail: &str) {
        self.worse(Status::Pending, Some(cause(kind, detail)));
    }
}

struct Cx<'a> {
    text: &'a str,
    atoms: &'a [Atom],
    lexed: &'a Lexed,
    scope: Scope<'a>,
    table: &'static Table,
    operand: Vec<bool>,
    kind: FileKind,
    features: Features,
    /// Where every name is looked up, instead of where it stands (a text
    /// made up by `\expandafter`'s look-through).
    fixed_pos: Option<usize>,
}

/// What `item` found besides the item: the predictions' causes.
type Noted = Vec<(&'static str, String)>;

pub(crate) fn classify(
    text: &str,
    atoms: &[Atom],
    lexed: &Lexed,
    scope: Scope<'_>,
    kind: FileKind,
    features: Features,
) -> Vec<Item> {
    let mut hazard_tok = vec![false; lexed.tokens.len()];
    for h in &lexed.hazards {
        if h.status == Status::Pending && h.cause != "verbatim" {
            hazard_tok[h.token] = true;
        }
    }
    let mut cx = Cx {
        text,
        atoms,
        lexed,
        scope,
        table: table(),
        operand: vec![false; atoms.len()],
        kind,
        features,
        fixed_pos: None,
    };
    let mut items = Vec::new();
    let mut closure = Closure::new(scope.document, scope.prelude);
    for a in 0..atoms.len() {
        if atoms[a].kind == AtomKind::Cs && !cx.operand[a] {
            let (mut it, noted, quantity) = cx.item(a);
            it.hazard = hazard_tok[atoms[a].tok];
            it.hints = cx.hints(&it, &noted, quantity, &mut closure);
            items.push(it);
        }
    }
    items
}

impl Cx<'_> {
    fn kind(&self, p: usize) -> Option<AtomKind> {
        self.atoms.get(p).map(|a| a.kind)
    }

    fn name(&self, p: usize) -> &str {
        self.atoms[p].name(self.text)
    }

    fn is_cs(&self, p: usize, n: &str) -> bool {
        self.kind(p) == Some(AtomKind::Cs) && self.name(p) == n
    }

    /// Past the group at `p`, or past the atom.
    fn jump(&self, p: usize) -> usize {
        let a = &self.atoms[p];
        if a.kind == AtomKind::Open {
            if a.mate == usize::MAX {
                self.atoms.len()
            } else {
                a.mate + 1
            }
        } else {
            p + 1
        }
    }

    /// The keyword `kw` from `p` (spaces first), case-insensitively.
    fn keyword(&self, p: usize, kw: &str) -> Option<usize> {
        let mut q = skip(self.atoms, p);
        for k in kw.chars() {
            match self.kind(q) {
                Some(AtomKind::Char(c)) if c.eq_ignore_ascii_case(&k) => q += 1,
                _ => return None,
            }
        }
        Some(q)
    }

    fn resolve(&self, name: &str, pos: usize, depth: u8) -> Res {
        let pos = self.fixed_pos.unwrap_or(pos);
        self.resolve_in(name, self.scope.file, pos, depth)
    }

    /// What `name` means at byte `pos` of file `file`.
    fn resolve_in(&self, name: &str, file: u32, pos: usize, depth: u8) -> Res {
        if depth > 8 {
            return Res::Pending(cause("\\let chain too long", name));
        }
        // (document order binds: a definition before the use in its own
        // file; else the fixed command or the kernel's, as predicted when
        // the document redefines it somewhere; else the document's own
        // definitions where they agree)
        let doc = self.scope.document.find(name, Some((file, pos)));
        if let Found::One(s) = doc
            && self.scope.document.defined_before(name, file, pos)
        {
            // (a `\let` binds its target where it stands)
            let (f, p) = self
                .scope
                .document
                .bound_at(name, (file, pos))
                .unwrap_or((file, pos));
            return self.shape(s, f, p, depth);
        }
        let redefined = !matches!(doc, Found::None);
        if let Some(e) = self.table.commands.get(name) {
            let st = if redefined {
                Status::Predicted
            } else {
                Status::Known
            };
            return match e {
                Entry::Command(args) => Res::Cmd(args.clone(), st),
                Entry::Quantity(t, ix) => Res::Quantity(*t, *ix, st),
            };
        }
        if !redefined && let Found::One(s) = self.scope.prelude.find(name, None) {
            return self.shape(s, file, pos, depth);
        }
        match doc {
            Found::One(s) => return self.shape(s, file, pos, depth),
            Found::Conflict => {
                if let Found::One(s) = self.scope.prelude.find(name, None) {
                    return self.shape(s, file, pos, depth);
                }
                return Res::Pending(cause("conflicting definitions", name));
            }
            Found::None => {}
        }
        if let Some((base, sig)) = name.rsplit_once(':') {
            return self.signature(name, base, sig);
        }
        if ["l_", "g_", "c_", "q_", "s_"]
            .iter()
            .any(|p| name.starts_with(p))
            && name.len() > 2
        {
            return Res::Quantity('c', None, Status::Predicted);
        }
        if name.starts_with("if") {
            return Res::Pending(cause("conditional defined elsewhere", name));
        }
        if name.chars().count() == 1 && !name.chars().all(char::is_alphabetic) {
            // (an undefined control symbol: `\,` `\;` `\!` `\-` `\/` `\ `
            // and the like are the kernel's, with no arguments)
            return Res::Cmd(Vec::new(), Status::Predicted);
        }
        Res::Pending(cause("macro defined elsewhere", name))
    }

    fn shape(&self, s: &Shape, file: u32, pos: usize, depth: u8) -> Res {
        match s {
            Shape::Macro(args) => Res::Cmd(args.clone(), Status::Predicted),
            Shape::Alias(t) => match self.resolve_in(t, file, pos, depth + 1) {
                Res::Cmd(a, st) => Res::Cmd(a, st.max(Status::Predicted)),
                Res::Quantity(t, ix, st) => Res::Quantity(t, ix, st.max(Status::Predicted)),
                p @ Res::Pending(_) => p,
            },
            Shape::Quantity(t) => Res::Quantity(*t, None, Status::Predicted),
            Shape::Cond => Res::Cmd(Vec::new(), Status::Predicted),
            Shape::Opaque(e) => Res::Pending(cause("definition unreadable", e)),
        }
    }

    /// An expl3 function's arguments, from its signature.
    fn signature(&self, name: &str, base: &str, sig: &str) -> Res {
        if let Some(prim) = base.strip_prefix("tex_")
            && sig == "D"
        {
            return match self.table.commands.get(prim) {
                Some(Entry::Command(a)) => Res::Cmd(a.clone(), Status::Known),
                Some(Entry::Quantity(t, ix)) => Res::Quantity(*t, *ix, Status::Known),
                None => Res::Pending(cause("expl3 D primitive", name)),
            };
        }
        if name == "exp_after:wN" {
            return Res::Pending(cause("\\expandafter", name));
        }
        if sig.contains('w')
            && self.features.w_signatures
            && let Some(r) = self.w_signature(name, base)
        {
            return r;
        }
        if name == "use:c" || name == "cs:w" || base.starts_with("cs_if_exist_use") {
            return Res::Pending(cause("\\csname", name));
        }
        if base.starts_with("peek_") {
            return Res::Pending(cause("lookahead", name));
        }
        let mut args = Vec::new();
        for c in sig.chars() {
            args.push(match c {
                'N' => Arg::Tok,
                'n' | 'c' | 'V' | 'v' | 'o' | 'x' | 'e' | 'f' | 'T' | 'F' => Arg::M,
                'p' => Arg::ParamText,
                'w' => return Res::Pending(cause("expl3 w signature", name)),
                'D' => return Res::Pending(cause("expl3 D signature", name)),
                _ => return Res::Pending(cause("expl3 signature unreadable", name)),
            });
        }
        Res::Cmd(args, Status::Predicted)
    }

    /// The item at `a`, the causes of its predictions, and whether it is
    /// a quantity.
    fn item(&mut self, a: usize) -> (Item, Noted, bool) {
        let name = self.name(a).to_owned();
        let tok = self.lexed.tokens[self.atoms[a].tok];
        let mut r = R::new();
        let item = |end, consumes, r: R| {
            (
                Item {
                    atom: a,
                    name: name.clone(),
                    status: r.status,
                    cause: r.cause,
                    consumes,
                    end,
                    hazard: false,
                    hints: Hints::none(),
                },
                r.noted,
                false,
            )
        };
        if tok.tz == Status::Pending {
            r.pending("tokenization pending (catcode)", &name);
            return item(a + 1, true, r);
        }
        if self.features.lookthrough
            && self.fixed_pos.is_none()
            && matches!(
                name.as_str(),
                "expandafter" | "exp_after:wN" | "exp_after:w"
            )
            && let Some((end, st, noted)) = self.lookthrough(a)
        {
            r.worse(st, None);
            r.worse(
                Status::Predicted,
                Some(cause("\\expandafter look-through", "")),
            );
            r.noted.extend(noted);
            return item(end, true, r);
        }
        match self.resolve(&name, tok.start, 0) {
            Res::Pending(c) => {
                r.worse(Status::Pending, Some(c));
                item(a + 1, true, r)
            }
            Res::Quantity(t, ix, st) => {
                r.worse(st, None);
                let quantity = |x: (Item, Noted, bool)| (x.0, x.1, true);
                if t == 'c' || self.alone_in_group(a) {
                    return quantity(item(a + 1, false, r));
                }
                let mut p = self.index(a + 1, ix, &mut r);
                if matches!(self.kind(skip(self.atoms, p)), None | Some(AtomKind::Close)) {
                    // (a quantity mentioned, not assigned: an argument)
                    return quantity(item(p, false, r));
                }
                p = self.read(p, &Arg::Eq, &mut r);
                p = match t {
                    't' => self.read(p, &Arg::Toks, &mut r),
                    _ => self.value(p, t, &mut r),
                };
                quantity(item(p, true, r))
            }
            Res::Cmd(_, st) if self.alone_in_group(a) => {
                r.worse(st, None);
                item(a + 1, false, r)
            }
            Res::Cmd(args, st) => {
                r.worse(
                    st,
                    (st == Status::Predicted).then(|| cause("visible definition", &name)),
                );
                let consumes = !args.is_empty();
                let mut p = a + 1;
                let mut k = 0;
                while k < args.len() {
                    if args[k] == Arg::StarSkip {
                        let q = skip(self.atoms, p);
                        if self.kind(q) == Some(AtomKind::Char('*')) {
                            p = q + 1;
                            k += 2;
                        } else {
                            k += 1;
                        }
                        continue;
                    }
                    p = self.read(p, &args[k], &mut r);
                    k += 1;
                }
                item(p, consumes, r)
            }
        }
    }

    /// Whether the control sequence at `a` is all its brace group holds:
    /// a name passed as an argument (`{\x}`), not run where it is.
    fn alone_in_group(&self, a: usize) -> bool {
        let mut b = a;
        while b > 0 && self.kind(b - 1) == Some(AtomKind::Space) {
            b -= 1;
        }
        b > 0
            && self.kind(b - 1) == Some(AtomKind::Open)
            && self.kind(skip(self.atoms, a + 1)) == Some(AtomKind::Close)
    }

    /// A quantity's index from `p`.
    fn index(&mut self, p: usize, ix: Option<char>, r: &mut R) -> usize {
        match ix {
            Some('i') => self.value(p, 'i', r),
            Some('F') => {
                let p = self.value(p, 'i', r);
                self.read(p, &Arg::Tok, r)
            }
            _ => p,
        }
    }

    fn mark(&mut self, p: usize) {
        if let Some(o) = self.operand.get_mut(p) {
            *o = true;
        }
    }

    /// A control sequence TeX expands at `p` to look for more of what it
    /// is reading: pending if it is a macro this pass cannot see.
    fn lookahead(&mut self, p: usize, why: &'static str, r: &mut R) {
        if self.kind(p) != Some(AtomKind::Cs) {
            return;
        }
        let n = self.name(p).to_owned();
        match self.resolve(&n, self.atoms[p].start, 0) {
            Res::Pending(_) => r.pending(why, &n),
            Res::Cmd(_, Status::Known) | Res::Quantity(_, _, Status::Known) => {}
            _ => r.worse(
                Status::Predicted,
                Some(cause("lookahead into a visible macro", &n)),
            ),
        }
    }

    #[allow(clippy::too_many_lines)]
    fn read(&mut self, p: usize, arg: &Arg, r: &mut R) -> usize {
        let n = self.atoms.len();
        match arg {
            Arg::Star => {
                let q = skip(self.atoms, p);
                if self.kind(q) == Some(AtomKind::Char('*')) {
                    q + 1
                } else {
                    p
                }
            }
            Arg::OptChar(c) => {
                let q = skip(self.atoms, p);
                if self.kind(q) == Some(AtomKind::Char(*c)) {
                    q + 1
                } else {
                    p
                }
            }
            Arg::StarSkip => p,
            Arg::Opt => {
                let q = skip(self.atoms, p);
                if self.kind(q) != Some(AtomKind::Char('[')) {
                    return p;
                }
                let mut q = q + 1;
                while q < n {
                    match self.atoms[q].kind {
                        AtomKind::Char(']') => return q + 1,
                        AtomKind::Open => q = self.jump(q),
                        AtomKind::Close => break,
                        _ => q += 1,
                    }
                }
                r.pending("optional argument without ]", "");
                q
            }
            Arg::M | Arg::Tok => {
                let q = skip(self.atoms, p);
                match self.kind(q) {
                    None | Some(AtomKind::Close) => {
                        r.pending("argument missing in its group", "");
                        q
                    }
                    Some(AtomKind::Open) if *arg == Arg::M => self.jump(q),
                    Some(AtomKind::Cs) => {
                        self.mark(q);
                        q + 1
                    }
                    _ => q + 1,
                }
            }
            Arg::Relation => {
                let q = skip(self.atoms, p);
                if matches!(self.kind(q), Some(AtomKind::Char(_))) {
                    q + 1
                } else {
                    r.pending("relation via macro", "");
                    q
                }
            }
            Arg::ParamText => {
                let mut q = p;
                while q < n {
                    match self.atoms[q].kind {
                        AtomKind::Open => return q,
                        AtomKind::Close => break,
                        AtomKind::Cs => {
                            self.mark(q);
                            q += 1;
                        }
                        _ => q += 1,
                    }
                }
                r.pending("definition without body", "");
                q
            }
            Arg::UntilOpen => {
                let mut q = p;
                while q < n && self.atoms[q].kind != AtomKind::Open {
                    if self.atoms[q].kind == AtomKind::Close {
                        r.pending("delimiter not in enclosing group", "{");
                        return q;
                    }
                    q += 1;
                }
                q
            }
            Arg::Group => {
                let mut q = skip(self.atoms, p);
                while self.is_cs(q, "relax") {
                    q = skip(self.atoms, q + 1);
                }
                match self.kind(q) {
                    Some(AtomKind::Open) => self.jump(q),
                    Some(AtomKind::Cs) if self.name(q) == "bgroup" => {
                        r.pending("material opened by \\bgroup", "");
                        q + 1
                    }
                    Some(AtomKind::Cs) => {
                        let nm = self.name(q).to_owned();
                        r.pending("general text from a macro", &nm);
                        q
                    }
                    _ => {
                        r.pending("expected {", "");
                        q
                    }
                }
            }
            Arg::BoxSpec => {
                let mut q = p;
                if let Some(e) = self.keyword(p, "to").or_else(|| self.keyword(p, "spread")) {
                    q = self.value(e, 'd', r);
                }
                self.read(q, &Arg::Group, r)
            }
            Arg::Eq => {
                let q = skip(self.atoms, p);
                if self.kind(q) == Some(AtomKind::Char('=')) {
                    q + 1
                } else {
                    p
                }
            }
            Arg::Int => self.value(p, 'i', r),
            Arg::Dimen => self.value(p, 'd', r),
            Arg::Glue => self.value(p, 'g', r),
            Arg::Value => self.value(p, 'v', r),
            Arg::Reg | Arg::Toks => {
                let q = skip(self.atoms, p);
                match self.kind(q) {
                    Some(AtomKind::Open) if *arg == Arg::Toks => self.jump(q),
                    Some(AtomKind::Cs) => {
                        self.mark(q);
                        let nm = self.name(q).to_owned();
                        match self.resolve(&nm, self.atoms[q].start, 0) {
                            Res::Quantity(_, ix, st) => {
                                r.worse(st, None);
                                self.index(q + 1, ix, r)
                            }
                            Res::Pending(_) => {
                                r.pending("register is a macro", &nm);
                                q + 1
                            }
                            Res::Cmd(_, Status::Known) => {
                                r.pending("register is a command", &nm);
                                q + 1
                            }
                            Res::Cmd(_, st) => {
                                r.worse(st, Some(cause("register is a visible macro", &nm)));
                                q + 1
                            }
                        }
                    }
                    _ => {
                        r.pending("missing register", "");
                        q
                    }
                }
            }
            Arg::CsName => {
                let mut q = p;
                while q < n {
                    if self.is_cs(q, "endcsname") {
                        return q + 1;
                    }
                    if self.atoms[q].kind == AtomKind::Close {
                        break;
                    }
                    q = self.jump(q);
                }
                r.pending("\\csname without \\endcsname", "");
                q
            }
            Arg::Expr => self.expr(p, r),
            Arg::File => {
                let mut q = skip(self.atoms, p);
                if self.kind(q) == Some(AtomKind::Open) {
                    return self.jump(q);
                }
                while matches!(self.kind(q), Some(AtomKind::Char(_))) {
                    q += 1;
                }
                self.lookahead(q, "file name reads into macro", r);
                q
            }
            Arg::Rule => {
                let mut q = p;
                loop {
                    let kw = ["width", "height", "depth"]
                        .iter()
                        .find_map(|k| self.keyword(q, k));
                    match kw {
                        Some(e) => q = self.value(e, 'd', r),
                        None => break,
                    }
                }
                let s = skip(self.atoms, q);
                self.lookahead(s, "rule keyword lookahead into macro", r);
                q
            }
            Arg::By => self.keyword(p, "by").unwrap_or(p),
            Arg::To => self.keyword(p, "to").unwrap_or_else(|| {
                r.pending("expected keyword to", "");
                p
            }),
            Arg::LetTarget => {
                let mut q = p;
                if self.kind(q) == Some(AtomKind::Space) {
                    q += 1;
                }
                match self.kind(q) {
                    Some(AtomKind::Cs) => {
                        self.mark(q);
                        q + 1
                    }
                    Some(_) => q + 1,
                    None => q,
                }
            }
            Arg::FontSize => {
                if let Some(e) = self.keyword(p, "at") {
                    self.value(e, 'd', r)
                } else if let Some(e) = self.keyword(p, "scaled") {
                    self.value(e, 'i', r)
                } else {
                    let s = skip(self.atoms, p);
                    self.lookahead(s, "font keyword lookahead into macro", r);
                    p
                }
            }
            Arg::Keywords => {
                r.worse(Status::Predicted, Some(cause("pdfTeX keywords", "")));
                let mut q = p;
                while q < n {
                    match self.atoms[q].kind {
                        AtomKind::Open => return self.jump(q),
                        AtomKind::Close | AtomKind::Par => break,
                        AtomKind::Cs => {
                            self.mark(q);
                            q += 1;
                        }
                        _ => q += 1,
                    }
                }
                r.pending("pdfTeX keyword syntax", "");
                q
            }
            Arg::Tikz => {
                let q = self.read(p, &Arg::Opt, r);
                let s = skip(self.atoms, q);
                if self.kind(s) == Some(AtomKind::Open) {
                    r.worse(Status::Predicted, Some(cause("TikZ syntax", "")));
                    self.jump(s)
                } else {
                    self.read(q, &Arg::Semicolon, r)
                }
            }
            Arg::Semicolon => {
                r.worse(Status::Predicted, Some(cause("TikZ syntax", "")));
                let mut q = p;
                while q < n {
                    match self.atoms[q].kind {
                        AtomKind::Char(';') => return q + 1,
                        AtomKind::Close => break,
                        AtomKind::Open => q = self.jump(q),
                        _ => q += 1,
                    }
                }
                r.pending("TikZ path without ;", "");
                q
            }
            Arg::Verb => {
                if self.kind(p) == Some(AtomKind::Verb) {
                    r.worse(Status::Predicted, Some(cause("verbatim delimiters", "")));
                    p + 1
                } else {
                    r.pending("verbatim run not found", "");
                    p
                }
            }
            Arg::Env => self.env(p, r),
            Arg::UntilCs(c) => {
                let mut q = p;
                while q < n {
                    if self.is_cs(q, c) {
                        return q + 1;
                    }
                    if self.atoms[q].kind == AtomKind::Close {
                        break;
                    }
                    q = self.jump(q);
                }
                r.pending("delimiter not in enclosing group", c);
                q
            }
            Arg::Literal(lits) => {
                if let Some(e) = self.matches(p, lits) {
                    e
                } else {
                    r.pending("parameter text does not match", "");
                    p
                }
            }
            Arg::Delimited(lits) => {
                let mut q = p;
                while q < n {
                    if let Some(e) = self.matches(q, lits) {
                        return e;
                    }
                    if self.atoms[q].kind == AtomKind::Close {
                        break;
                    }
                    q = self.jump(q);
                }
                r.pending("delimiter not in enclosing group", "");
                q
            }
            Arg::Pdf(name) => self.pdf(name, p, r),
            Arg::Array(ty) => self.array(p, *ty, r),
            Arg::Delim => self.delim(p, r),
            Arg::Pending(c) => {
                r.worse(
                    Status::Pending,
                    Some(Cause {
                        kind: static_cause(c),
                        detail: String::new(),
                    }),
                );
                p
            }
        }
    }

    /// Whether `lits` follow at `q`: past them.
    fn matches(&self, q: usize, lits: &[Lit]) -> Option<usize> {
        for (k, l) in lits.iter().enumerate() {
            let a = self.atoms.get(q + k)?;
            let ok = match (l, a.kind) {
                (Lit::Char(' '), AtomKind::Space) => true,
                (Lit::Char(c), AtomKind::Char(d)) => *c == d,
                (Lit::Cs(nm), AtomKind::Cs) => nm == a.name(self.text),
                _ => false,
            };
            if !ok {
                return None;
            }
        }
        Some(q + lits.len())
    }

    /// `\begin`'s `{name}` and that environment's arguments.
    fn env(&mut self, p: usize, r: &mut R) -> usize {
        let q = skip(self.atoms, p);
        if self.kind(q) != Some(AtomKind::Open) {
            r.pending("environment name not braced", "");
            return q;
        }
        let e = self.jump(q);
        let Some(name) = super::atoms::group_text(self.atoms, q) else {
            r.pending("environment name computed", "");
            return e;
        };
        let pos = self.fixed_pos.unwrap_or(self.atoms[q].start);
        let args = match self
            .scope
            .document
            .find_env(&name, Some((self.scope.file, pos)))
        {
            Found::One(a) => {
                r.worse(
                    Status::Predicted,
                    Some(cause("visible environment definition", &name)),
                );
                a.clone()
            }
            Found::Conflict => {
                r.pending("conflicting definitions", &name);
                return e;
            }
            Found::None => {
                if let Some(a) = self.table.envs.get(name.as_str()) {
                    a.clone()
                } else if let Found::One(a) = self.scope.prelude.find_env(&name, None) {
                    r.worse(
                        Status::Predicted,
                        Some(cause("visible environment definition", &name)),
                    );
                    a.clone()
                } else if let Res::Cmd(a, st) = self.resolve(&name, pos, 0) {
                    r.worse(st, Some(cause("environment as its macro", &name)));
                    a
                } else {
                    r.pending("environment defined elsewhere", &name);
                    return e;
                }
            }
        };
        let mut p = e;
        for a in &args {
            p = self.read(p, a, r);
        }
        p
    }

    /// An expression up to `\relax` (or what cannot continue it).
    fn expr(&mut self, mut p: usize, r: &mut R) -> usize {
        loop {
            p = skip(self.atoms, p);
            match self.kind(p) {
                Some(AtomKind::Char(c)) if c.is_ascii_alphanumeric() || "+-*/().,".contains(c) => {
                    p += 1;
                }
                Some(AtomKind::Cs) => {
                    let nm = self.name(p).to_owned();
                    self.mark(p);
                    if nm == "relax" {
                        return p + 1;
                    }
                    if matches!(nm.as_str(), "numexpr" | "dimexpr" | "glueexpr" | "muexpr") {
                        p = self.expr(p + 1, r);
                        continue;
                    }
                    match self.resolve(&nm, self.atoms[p].start, 0) {
                        Res::Quantity(_, ix, st) => {
                            r.worse(st, None);
                            p = self.index(p + 1, ix, r);
                        }
                        Res::Cmd(_, Status::Known) => {
                            self.operand[p] = false;
                            return p;
                        }
                        Res::Cmd(_, _) => {
                            r.worse(
                                Status::Predicted,
                                Some(cause("macro in an expression", &nm)),
                            );
                            p += 1;
                        }
                        Res::Pending(_) => {
                            // (etex.ch `scan_expr`: a macro may supply
                            // any part of the expression; it ends at
                            // `\relax` if one follows on the line)
                            if let Some(e) = self.relax_on_line(p + 1) {
                                r.worse(Status::Predicted, Some(cause("expression", &nm)));
                                return e + 1;
                            }
                            r.pending("expression", &nm);
                            return p + 1;
                        }
                    }
                }
                _ => return p,
            }
        }
    }

    /// The first `\relax` from `p` on the same line, outside groups.
    fn relax_on_line(&self, mut p: usize) -> Option<usize> {
        let line_end = |a: usize| self.text[a..].find('\n').map_or(self.text.len(), |k| a + k);
        let end = line_end(self.atoms.get(p)?.start);
        while p < self.atoms.len() && self.atoms[p].start < end {
            match self.atoms[p].kind {
                AtomKind::Cs if self.name(p) == "relax" => return Some(p),
                AtomKind::Close | AtomKind::Par => return None,
                AtomKind::Open => p = self.jump(p),
                _ => p += 1,
            }
        }
        None
    }

    /// The keyword `kw` from `p`; if absent, a macro at `p` might supply
    /// it (TeX expands while it looks for a keyword, tex.web §407).
    fn pdf_kw(&mut self, p: usize, kw: &str, r: &mut R) -> Option<usize> {
        let e = self.keyword(p, kw);
        if e.is_none() {
            let s = skip(self.atoms, p);
            self.lookahead(s, "keyword lookahead into macro", r);
        }
        e
    }

    /// The first of `kws` found from `p`.
    fn pdf_kws(&mut self, p: usize, kws: &[&str], r: &mut R) -> Option<usize> {
        let e = kws.iter().find_map(|k| self.keyword(p, k));
        if e.is_none() {
            let s = skip(self.atoms, p);
            self.lookahead(s, "keyword lookahead into macro", r);
        }
        e
    }

    /// pdfTeX's `scan_alt_rule` (pdfTeX §1550): `width`, `height`,
    /// `depth` dimens, any number, any order.
    fn pdf_rule(&mut self, mut p: usize, r: &mut R) -> usize {
        while let Some(e) = self.pdf_kws(p, &["width", "height", "depth"], r) {
            p = self.value(e, 'd', r);
        }
        p
    }

    /// An optional `kw` followed by general text.
    fn pdf_opt_text(&mut self, p: usize, kw: &str, r: &mut R) -> usize {
        match self.pdf_kw(p, kw, r) {
            Some(e) => self.read(e, &Arg::Group, r),
            None => p,
        }
    }

    /// pdfTeX's `scan_action` (pdfTeX §1554): `user` text, or `goto` /
    /// `thread`, an optional `file` text, an optional `struct` (text,
    /// `name` text or `num` int), then `page` int and text, `name` text
    /// or `num` int, then an optional `newwindow`/`nonewwindow`.
    fn pdf_action(&mut self, p: usize, r: &mut R) -> usize {
        if let Some(e) = self.pdf_kw(p, "user", r) {
            return self.read(e, &Arg::Group, r);
        }
        let Some(mut q) = self.pdf_kws(p, &["goto", "thread"], r) else {
            r.pending("pdfTeX action without user/goto/thread", "");
            return p;
        };
        let file = self.pdf_kw(q, "file", r);
        if let Some(e) = file {
            q = self.read(e, &Arg::Group, r);
        }
        if let Some(e) = self.pdf_kw(q, "struct", r) {
            q = if file.is_some() {
                self.read(e, &Arg::Group, r)
            } else if let Some(e) = self.pdf_kw(e, "name", r) {
                self.read(e, &Arg::Group, r)
            } else if let Some(e) = self.pdf_kw(e, "num", r) {
                self.value(e, 'i', r)
            } else {
                e
            };
        }
        if let Some(e) = self.pdf_kw(q, "page", r) {
            q = self.value(e, 'i', r);
            q = self.read(q, &Arg::Group, r);
        } else if let Some(e) = self.pdf_kw(q, "name", r) {
            q = self.read(e, &Arg::Group, r);
        } else if let Some(e) = self.pdf_kw(q, "num", r) {
            q = self.value(e, 'i', r);
        } else {
            r.pending("pdfTeX action without page/name/num", "");
            return q;
        }
        match self.pdf_kws(q, &["newwindow", "nonewwindow"], r) {
            Some(e) => self.optional_space(e),
            None => q,
        }
    }

    /// One optional space (tex.web §443 "Scan an optional space").
    fn optional_space(&self, p: usize) -> usize {
        if self.kind(p) == Some(AtomKind::Space) {
            p + 1
        } else {
            p
        }
    }

    /// A pdfTeX command's keyword syntax (pdftex.web; sections as
    /// "pdfTeX §N"), from `p`, past the command.
    #[allow(clippy::too_many_lines)]
    fn pdf(&mut self, name: &str, p: usize, r: &mut R) -> usize {
        let g = |cx: &mut Self, q: usize, r: &mut R| cx.read(q, &Arg::Group, r);
        match name {
            // pdfTeX §1536: [shipout] [direct|page] text
            "pdfliteral" => {
                let q = self.pdf_kw(p, "shipout", r).unwrap_or(p);
                let q = self.pdf_kws(q, &["direct", "page"], r).unwrap_or(q);
                g(self, q, r)
            }
            // pdfTeX §1537: int, then set/push text, or pop/current
            "pdfcolorstack" => {
                let q = self.value(p, 'i', r);
                if let Some(e) = self.pdf_kws(q, &["set", "push"], r) {
                    g(self, e, r)
                } else if let Some(e) = self.pdf_kws(q, &["pop", "current"], r) {
                    e
                } else {
                    r.pending("pdfTeX \\pdfcolorstack without set/push/pop/current", "");
                    q
                }
            }
            // pdfTeX §495 (pdf_colorstack_init_code): [page] [direct|page] text
            "pdfcolorstackinit" => {
                let q = self.pdf_kw(p, "page", r).unwrap_or(p);
                let q = self.pdf_kws(q, &["direct", "page"], r).unwrap_or(q);
                g(self, q, r)
            }
            // pdfTeX §1542: reserveobjnum, or [useobjnum int]
            // [stream [attr text]] [file] text
            "pdfobj" => {
                if let Some(e) = self.pdf_kw(p, "reserveobjnum", r) {
                    return self.optional_space(e);
                }
                let mut q = p;
                if let Some(e) = self.pdf_kw(q, "useobjnum", r) {
                    q = self.value(e, 'i', r);
                }
                if let Some(e) = self.pdf_kw(q, "stream", r) {
                    q = self.pdf_opt_text(e, "attr", r);
                }
                let q = self.pdf_kw(q, "file", r).unwrap_or(q);
                g(self, q, r)
            }
            // pdfTeX §1546: [attr text] [resources text] register number
            "pdfxform" => {
                let q = self.pdf_opt_text(p, "attr", r);
                let q = self.pdf_opt_text(q, "resources", r);
                self.value(q, 'i', r)
            }
            // pdfTeX §1550 `scan_image`: rule spec, [attr text],
            // [named text | page int], [colorspace int], [a page box
            // (`scan_pdf_box_spec`)], file name text
            "pdfximage" => {
                let q = self.pdf_rule(p, r);
                let mut q = self.pdf_opt_text(q, "attr", r);
                if let Some(e) = self.pdf_kw(q, "named", r) {
                    q = g(self, e, r);
                } else if let Some(e) = self.pdf_kw(q, "page", r) {
                    q = self.value(e, 'i', r);
                }
                if let Some(e) = self.pdf_kw(q, "colorspace", r) {
                    q = self.value(e, 'i', r);
                }
                let q = self
                    .pdf_kws(
                        q,
                        &["mediabox", "cropbox", "bleedbox", "trimbox", "artbox"],
                        r,
                    )
                    .unwrap_or(q);
                g(self, q, r)
            }
            // pdfTeX §1556: reserveobjnum, or [useobjnum int] rule spec text
            "pdfannot" => {
                if let Some(e) = self.pdf_kw(p, "reserveobjnum", r) {
                    return self.optional_space(e);
                }
                let mut q = p;
                if let Some(e) = self.pdf_kw(q, "useobjnum", r) {
                    q = self.value(e, 'i', r);
                }
                let q = self.pdf_rule(q, r);
                g(self, q, r)
            }
            // pdfTeX §1558, §1555 `new_annot_whatsit`: rule spec,
            // [attr text], action
            "pdfstartlink" => {
                let q = self.pdf_rule(p, r);
                let q = self.pdf_opt_text(q, "attr", r);
                self.pdf_action(q, r)
            }
            // pdfTeX §1563: [struct int] (num int | name text), then
            // xyz [zoom int] | fitbh | fitbv | fitb | fith | fitv |
            // fitr rule spec | fit (longest first)
            "pdfdest" => {
                let mut q = p;
                if let Some(e) = self.pdf_kw(q, "struct", r) {
                    q = self.value(e, 'i', r);
                }
                if let Some(e) = self.pdf_kw(q, "num", r) {
                    q = self.value(e, 'i', r);
                } else if let Some(e) = self.pdf_kw(q, "name", r) {
                    q = g(self, e, r);
                } else {
                    r.pending("pdfTeX \\pdfdest without num/name", "");
                    return q;
                }
                if let Some(e) = self.pdf_kw(q, "xyz", r) {
                    return match self.pdf_kw(e, "zoom", r) {
                        Some(z) => self.value(z, 'i', r),
                        None => self.optional_space(e),
                    };
                }
                if let Some(e) = self.pdf_kw(q, "fitr", r) {
                    return self.pdf_rule(e, r);
                }
                if let Some(e) =
                    self.pdf_kws(q, &["fitbh", "fitbv", "fitb", "fith", "fitv", "fit"], r)
                {
                    self.optional_space(e)
                } else {
                    r.pending("pdfTeX \\pdfdest without a destination type", "");
                    q
                }
            }
            // pdfTeX §1565, §1566, §1564 `scan_thread_id`: rule spec,
            // [attr text], num int | name text
            "pdfthread" | "pdfstartthread" => {
                let q = self.pdf_rule(p, r);
                let q = self.pdf_opt_text(q, "attr", r);
                if let Some(e) = self.pdf_kw(q, "num", r) {
                    self.value(e, 'i', r)
                } else if let Some(e) = self.pdf_kw(q, "name", r) {
                    g(self, e, r)
                } else {
                    r.pending("pdfTeX thread without num/name", "");
                    q
                }
            }
            // pdfTeX §1561: [attr text] action [count int] text
            "pdfoutline" => {
                let q = self.pdf_opt_text(p, "attr", r);
                let mut q = self.pdf_action(q, r);
                if let Some(e) = self.pdf_kw(q, "count", r) {
                    q = self.value(e, 'i', r);
                }
                g(self, q, r)
            }
            // pdfTeX §1577: text [openaction action]
            "pdfcatalog" => {
                let q = g(self, p, r);
                match self.pdf_kw(q, "openaction", r) {
                    Some(e) => self.pdf_action(e, r),
                    None => q,
                }
            }
            // pdfTeX §495 (pdf_mdfive_sum_code): [file] text
            "pdfmdfivesum" => {
                let q = self.pdf_kw(p, "file", r).unwrap_or(p);
                g(self, q, r)
            }
            // pdfTeX §495 (pdf_file_dump_code): [offset int] [length int] text
            "pdffiledump" => {
                let mut q = p;
                if let Some(e) = self.pdf_kw(q, "offset", r) {
                    q = self.value(e, 'i', r);
                }
                if let Some(e) = self.pdf_kw(q, "length", r) {
                    q = self.value(e, 'i', r);
                }
                g(self, q, r)
            }
            // pdfTeX §495 (pdf_match_code): [icase] [subcount int] text text
            "pdfmatch" => {
                let mut q = self.pdf_kw(p, "icase", r).unwrap_or(p);
                if let Some(e) = self.pdf_kw(q, "subcount", r) {
                    q = self.value(e, 'i', r);
                }
                let q = g(self, q, r);
                g(self, q, r)
            }
            // pdfTeX §703 `read_expand_font`: font, [=], stretch,
            // shrink and step ints, [autoexpand]
            "pdffontexpand" => {
                let q = self.read(p, &Arg::Tok, r);
                let q = self.read(q, &Arg::Eq, r);
                let q = self.value(q, 'i', r);
                let q = self.value(q, 'i', r);
                let q = self.value(q, 'i', r);
                match self.pdf_kw(q, "autoexpand", r) {
                    Some(e) => self.optional_space(e),
                    None => q,
                }
            }
            _ => unreachable!("no pdfTeX syntax for {name}"),
        }
    }

    /// An array parameter (tex.web §1248 `set_shape`; etex.ch's
    /// `\interlinepenalties` and kin): `[=] n`, then `n` values (`ty`
    /// `'P'`: `\parshape`'s `n` pairs of dimens). The count must be
    /// written out: otherwise the extent is pending.
    fn array(&mut self, p: usize, ty: char, r: &mut R) -> usize {
        let p = self.read(p, &Arg::Eq, r);
        let s = skip(self.atoms, p);
        let mut d = s;
        let mut n: usize = 0;
        while let Some(AtomKind::Char(c)) = self.kind(d) {
            let Some(v) = c.to_digit(10) else { break };
            n = n.saturating_mul(10).saturating_add(v as usize);
            d += 1;
        }
        let q = self.value(s, 'i', r);
        if d == s {
            r.pending("array parameter", "count not written out");
            return q;
        }
        let mut q = q;
        for _ in 0..n.min(10_000) {
            if ty == 'P' {
                q = self.value(q, 'd', r);
                q = self.value(q, 'd', r);
            } else {
                q = self.value(q, ty, r);
            }
        }
        q
    }

    /// A delimiter (tex.web §1160 `scan_delimiter`): a character, or
    /// `\delimiter` and an int; a macro may expand to either.
    fn delim(&mut self, p: usize, r: &mut R) -> usize {
        let q = skip(self.atoms, p);
        match self.kind(q) {
            Some(AtomKind::Cs) if self.name(q) == "delimiter" => {
                self.mark(q);
                self.value(q + 1, 'i', r)
            }
            _ => self.read(p, &Arg::Tok, r),
        }
    }

    /// After a number's digits: TeX reads one more token, expanding it.
    fn after_number(&mut self, p: usize, r: &mut R) -> usize {
        match self.kind(p) {
            Some(AtomKind::Space) => p + 1,
            Some(AtomKind::Cs) => {
                self.lookahead(p, "number lookahead into macro", r);
                p
            }
            _ => p,
        }
    }

    /// A unit of measure from `p` (spaces first), `fil`s too if `fil`.
    fn unit(&self, p: usize, fil: bool) -> Option<usize> {
        let p = self.keyword(p, "true").unwrap_or(p);
        for u in [
            "pt", "pc", "in", "bp", "cm", "mm", "dd", "cc", "sp", "em", "ex", "px", "nd", "nc",
            "mu",
        ] {
            if let Some(e) = self.keyword(p, u) {
                return Some(e);
            }
        }
        if fil && let Some(mut e) = self.keyword(p, "fil") {
            while self.kind(e) == Some(AtomKind::Char('l'))
                || self.kind(e) == Some(AtomKind::Char('L'))
            {
                e += 1;
            }
            return Some(e);
        }
        None
    }

    /// A number (`'i'`), dimen (`'d'`), glue (`'g'`), any of them (`'v'`),
    /// or a stretch or shrink (`'f'`: a dimen or `fil`s).
    #[allow(clippy::too_many_lines)]
    fn value(&mut self, p: usize, ty: char, r: &mut R) -> usize {
        let mut p = skip(self.atoms, p);
        while matches!(self.kind(p), Some(AtomKind::Char('+' | '-'))) {
            p = skip(self.atoms, p + 1);
        }
        let mut need_unit = ty != 'i';
        match self.kind(p) {
            Some(AtomKind::Char('`')) => {
                p += 1;
                if self.kind(p) == Some(AtomKind::Cs) {
                    self.mark(p);
                    // (`\^^K`: the control symbol `\^` and `^K`)
                    if self.name(p) == "^" && self.kind(p + 1) == Some(AtomKind::Char('^')) {
                        p += 2;
                    }
                }
                p = self.after_number(p + 1, r);
            }
            Some(AtomKind::Param(_)) => {
                r.pending("parameter in value position", "");
                return p + 1;
            }
            Some(AtomKind::Char(c))
                if c.is_ascii_digit() || matches!(c, '.' | ',' | '\'' | '"') =>
            {
                let radix_digits = |c: char| match c {
                    '\'' => "01234567",
                    '"' => "0123456789ABCDEF",
                    _ => "0123456789.,",
                };
                let set = radix_digits(c);
                if matches!(c, '\'' | '"') {
                    p += 1;
                }
                while let Some(AtomKind::Char(d)) = self.kind(p) {
                    if !set.contains(d) {
                        break;
                    }
                    p += 1;
                }
                if ty == 'i' {
                    p = self.after_number(p, r);
                }
            }
            Some(AtomKind::Cs) => {
                let nm = self.name(p).to_owned();
                self.mark(p);
                if matches!(nm.as_str(), "numexpr" | "dimexpr" | "glueexpr" | "muexpr") {
                    p = self.expr(p + 1, r);
                    need_unit = need_unit && nm == "numexpr";
                } else {
                    match self.resolve(&nm, self.atoms[p].start, 0) {
                        Res::Quantity(t, ix, st) => {
                            r.worse(st, None);
                            p = self.index(p + 1, ix, r);
                            need_unit = need_unit && matches!(t, 'i' | 'c');
                            if t == 'c' && st == Status::Predicted {
                                // (a `\chardef` constant, or a register
                                // whose type the pass assumes)
                                need_unit = false;
                            }
                        }
                        Res::Pending(_) => {
                            r.pending("value is a macro", &nm);
                            return p + 1;
                        }
                        Res::Cmd(_, Status::Known) => {
                            r.pending("command in value position", &nm);
                            return p + 1;
                        }
                        Res::Cmd(_, st) => {
                            r.worse(st, Some(cause("value is a visible macro", &nm)));
                            p += 1;
                            need_unit = false;
                        }
                    }
                }
            }
            _ => {
                r.pending("missing number", "");
                return p;
            }
        }
        if need_unit {
            if let Some(e) = self.unit(p, ty == 'f') {
                p = self.after_number(e, r);
            } else {
                let q = skip(self.atoms, p);
                match self.kind(q) {
                    Some(AtomKind::Cs) => {
                        let nm = self.name(q).to_owned();
                        self.mark(q);
                        match self.resolve(&nm, self.atoms[q].start, 0) {
                            Res::Quantity(_, ix, st) => {
                                r.worse(st, None);
                                p = self.index(q + 1, ix, r);
                            }
                            Res::Pending(_) => {
                                r.pending("unit is a macro", &nm);
                                return q + 1;
                            }
                            Res::Cmd(_, st) => {
                                if ty == 'v' {
                                    self.operand[q] = false;
                                } else {
                                    r.worse(
                                        st.max(Status::Predicted),
                                        Some(cause("unit is a macro", &nm)),
                                    );
                                    p = q + 1;
                                }
                            }
                        }
                    }
                    _ if ty == 'v' => {}
                    _ => r.pending("missing unit", ""),
                }
            }
        }
        if matches!(ty, 'g' | 'v') {
            loop {
                if let Some(e) = self.keyword(p, "plus") {
                    p = self.value(e, 'f', r);
                } else if let Some(e) = self.keyword(p, "minus") {
                    p = self.value(e, 'f', r);
                } else {
                    break;
                }
            }
            let s = skip(self.atoms, p);
            self.lookahead(s, "glue keyword lookahead into macro", r);
        }
        p
    }
}

/// A table spec's pending cause as a static string.
fn static_cause(c: &str) -> &'static str {
    match c {
        "\\expandafter" => "\\expandafter",
        "\\futurelet" => "\\futurelet",
        "\\csname" => "\\csname (constructed name)",
        "lookahead" => "lookahead (\\@ifnextchar, \\@ifstar)",
        "\\scantokens" => "\\scantokens",
        "array parameter" => "array parameter (\\parshape)",
        "pdfTeX keyword syntax" => "pdfTeX keyword syntax",
        "plain \\buildrel" => "plain \\buildrel",
        "\\foreach list syntax" => "\\foreach list syntax",
        "picture coordinate syntax" => "picture coordinate syntax",
        "xparse verbatim argument" => "xparse verbatim argument",
        "xparse required delimited argument" => "xparse required delimited argument",
        "xparse g argument" => "xparse g argument",
        "xparse embellishment" => "xparse embellishment",
        "xparse delimited optional argument" => "xparse delimited optional argument",
        _ => "other pending syntax",
    }
}

/// The expl3 `:w` functions whose extent l3kernel fixes (B1).
///
/// The primitive copies are `\tex_let:D`/`\cs_new_eq:NN` of a primitive
/// (l3basics.dtx "Primitive conditionals", "Expansion", "Copying
/// primitives"; l3int.dtx `\int_value:w`, `\if_int_odd:w`, `\if_case:w`;
/// l3skip.dtx `\if_dim:w`; l3prg.dtx `\if_predicate:w`; l3file.dtx
/// `\if_eof:w`; l3expan.dtx `\__kernel_exp_not:w`); `\int_eval:w` is
/// `\tex_the:D \__int_eval:w`, the latter `\tex_numexpr:D` (l3int.dtx).
/// The delimited ones are l3basics.dtx's `\use_none_delimit_by_…:w` and
/// `\use_i_delimit_by_…:nw` (`#1#2 \q_stop {#1}`), l3quark.dtx's
/// `\use_none_delimit_by_s_stop:w` and l3tl.dtx's `\tl_head:w`
/// (`#1#2 \q_stop`).
fn w_primitive(name: &str) -> Option<&'static str> {
    Some(match name {
        "if_meaning:w" => "ifx",
        "if:w" | "if_charcode:w" => "if",
        "if_catcode:w" => "ifcat",
        "if_cs_exist:w" => "ifcsname",
        "exp:w" => "romannumeral",
        "if_int_compare:w" => "ifnum",
        "__kernel_tl_to_str:w" => "detokenize",
        "__kernel_exp_not:w" => "unexpanded",
        "if_eof:w" => "ifeof",
        "if_predicate:w" | "if_int_odd:w" => "ifodd",
        "if_dim:w" => "ifdim",
        "int_value:w" => "number",
        "if_case:w" => "ifcase",
        "int_eval:w" | "__int_eval:w" | "__fp_int_eval:w" | "__seq_int_eval:w"
        | "__regex_int_eval:w" => "numexpr",
        "dim_eval:w" | "__dim_eval:w" | "__box_dim_eval:w" => "dimexpr",
        _ => return None,
    })
}

fn w_delimited(name: &str) -> Option<Vec<Arg>> {
    let until = |q: &str| Arg::Delimited(vec![Lit::Cs(q.to_owned())]);
    Some(match name {
        "use_none_delimit_by_q_nil:w" => vec![until("q_nil")],
        "use_none_delimit_by_q_stop:w" => vec![until("q_stop")],
        "use_none_delimit_by_q_recursion_stop:w" => vec![until("q_recursion_stop")],
        "use_none_delimit_by_s_stop:w" => vec![until("s_stop")],
        "use_i_delimit_by_q_nil:nw" => vec![Arg::M, until("q_nil")],
        "use_i_delimit_by_q_stop:nw" | "tl_head:w" => vec![Arg::M, until("q_stop")],
        "use_i_delimit_by_q_recursion_stop:nw" => vec![Arg::M, until("q_recursion_stop")],
        "exp_end_continue_f:w" => Vec::new(),
        _ => return None,
    })
}

/// Whether `s` ends in a control word (`\abc`).
fn ends_in_control_word(s: &str) -> bool {
    let t = s.trim_end_matches(|c: char| c.is_ascii_alphabetic() || c == '@');
    t.len() < s.len() && t.ends_with('\\') && !t.ends_with("\\\\")
}

impl Cx<'_> {
    fn w_signature(&self, name: &str, _base: &str) -> Option<Res> {
        if let Some(p) = w_primitive(name) {
            return match self.table.commands.get(p) {
                Some(Entry::Command(a)) => Some(Res::Cmd(a.clone(), Status::Predicted)),
                Some(Entry::Quantity(t, ix)) => Some(Res::Quantity(*t, *ix, Status::Predicted)),
                None => None,
            };
        }
        w_delimited(name).map(|a| Res::Cmd(a, Status::Predicted))
    }

    /// The text of the body `name` binds to at `pos`, as [`Cx::resolve`]
    /// binds.
    fn body_of(&self, name: &str, pos: usize) -> Option<String> {
        let doc = self.scope.document;
        let at = Some((self.scope.file, pos));
        if doc.defined_before(name, self.scope.file, pos) {
            return match doc.find_body(name, at) {
                Found::One(b) => Some(b.text.clone()),
                _ => None,
            };
        }
        if self.table.commands.contains_key(name) {
            return None;
        }
        if !doc.defines(name)
            && let Found::One(b) = self.scope.prelude.find_body(name, None)
        {
            return Some(b.text.clone());
        }
        match doc.find_body(name, at) {
            Found::One(b) => Some(b.text.clone()),
            _ => None,
        }
    }

    /// `\expandafter\a\b` with `\b` a macro without parameters whose body
    /// is visible: `\a`'s extent read over `\b`'s body and the text after
    /// (B2). The end atom, how sure, and the names it rests on.
    fn lookthrough(&mut self, a: usize) -> Option<(usize, Status, Noted)> {
        let (t1, t2) = (a + 1, a + 2);
        let x1 = *self.atoms.get(t1)?;
        let x2 = *self.atoms.get(t2)?;
        if x2.kind != AtomKind::Cs || !matches!(x1.kind, AtomKind::Cs | AtomKind::Char(_)) {
            return None;
        }
        let b = x2.name(self.text).to_owned();
        let Res::Cmd(args, _) = self.resolve(&b, x2.start, 0) else {
            return None;
        };
        if !args.is_empty() {
            return None;
        }
        let body = self.body_of(&b, x2.start)?;
        let mut noted: Noted = vec![("\\expandafter look-through", b)];
        if x1.kind != AtomKind::Cs {
            self.mark(t1);
            self.mark(t2);
            return Some((t2 + 1, Status::Predicted, noted));
        }
        let n1 = x1.name(self.text);
        if matches!(n1, "expandafter" | "exp_after:wN" | "exp_after:w") {
            return None;
        }
        let text = self.text;
        let rest_start = self.atoms.get(t2 + 1).map_or(text.len(), |x| x.start);
        let mut rest_end = (rest_start + 4096).min(text.len());
        while !text.is_char_boundary(rest_end) {
            rest_end += 1;
        }
        let rest = &text[rest_start..rest_end];
        if ends_in_control_word(&body) && rest.starts_with(|c: char| c.is_ascii_alphabetic()) {
            return None;
        }
        let head = &text[x1.start..x1.end];
        let sep = if ends_in_control_word(head) { " " } else { "" };
        let syn = format!("{head}{sep}{body}{rest}");
        let boundary = head.len() + sep.len() + body.len();
        let lexed = super::lex::lex(&syn, self.kind, Some(self.scope));
        let atoms = super::atoms::atoms(&syn, &lexed.tokens);
        if atoms.first().map(|x| x.kind) != Some(AtomKind::Cs) {
            return None;
        }
        let mut sub = Cx {
            text: &syn,
            atoms: &atoms,
            lexed: &lexed,
            scope: self.scope,
            table: self.table,
            operand: vec![false; atoms.len()],
            kind: self.kind,
            features: Features {
                lookthrough: false,
                ..self.features
            },
            fixed_pos: Some(self.atoms[a].start),
        };
        let (it, sub_noted, _) = sub.item(0);
        if it.status == Status::Pending {
            return None;
        }
        if it.end >= atoms.len() && rest_end < text.len() {
            // (it ran off the window)
            return None;
        }
        let e_byte = atoms.get(it.end).map_or(syn.len(), |x| x.start);
        let end = if e_byte <= boundary {
            t2 + 1
        } else {
            let real = rest_start + (e_byte - boundary);
            if real >= text.len() {
                self.atoms.len()
            } else {
                self.atoms.binary_search_by_key(&real, |x| x.start).ok()?
            }
        };
        noted.extend(sub_noted);
        self.mark(t1);
        self.mark(t2);
        Some((end.max(t2 + 1), it.status.max(Status::Predicted), noted))
    }

    /// What a prediction about `name` at `pos` rests on, and how weakly
    /// it is held (0 weakest: a definition bound across files by load
    /// order; 1 the document's; 2 the kernel's; 3 a convention).
    fn assumption(&self, name: &str, pos: usize) -> (u8, Assumption) {
        let pos = self.fixed_pos.unwrap_or(pos);
        let mi = |expected| Assumption::MeaningIs {
            cs: name.to_owned(),
            expected,
        };
        let doc = self.scope.document;
        let file = self.scope.file;
        let f = doc.find(name, Some((file, pos)));
        if let Found::One(s) = f
            && doc.defined_before(name, file, pos)
        {
            let rank = u8::from(!doc.bound_across_files(name, file, pos));
            return (rank, mi(shape_of(s)));
        }
        if self.table.commands.contains_key(name) {
            return (2, mi(MeaningShape::Kernel));
        }
        let redefined = !matches!(f, Found::None);
        if !redefined && let Found::One(s) = self.scope.prelude.find(name, None) {
            return (2, mi(shape_of(s)));
        }
        match f {
            Found::One(s) => return (1, mi(shape_of(s))),
            Found::Conflict => {
                if let Found::One(s) = self.scope.prelude.find(name, None) {
                    return (2, mi(shape_of(s)));
                }
            }
            Found::None => {}
        }
        if let Some(base) = name.strip_prefix("tex_")
            && base.ends_with(":D")
        {
            return (3, mi(MeaningShape::Primitive));
        }
        if let Some((_, sig)) = name.rsplit_once(':') {
            return (3, mi(MeaningShape::Expl3(sig.to_owned())));
        }
        if ["l_", "g_", "c_", "q_", "s_"]
            .iter()
            .any(|p| name.starts_with(p))
        {
            return (3, mi(MeaningShape::Register('c')));
        }
        (3, mi(MeaningShape::Kernel))
    }

    /// Whether `\global` stands before the item at `a` (through `\long`,
    /// `\outer`, `\protected`).
    fn prefixed_global(&self, a: usize) -> bool {
        let mut b = a;
        loop {
            while b > 0 && self.kind(b - 1) == Some(AtomKind::Space) {
                b -= 1;
            }
            if b == 0 || self.kind(b - 1) != Some(AtomKind::Cs) {
                return false;
            }
            b -= 1;
            match self.name(b) {
                "global" | "tex_global:D" => return true,
                "long" | "outer" | "protected" | "tex_long:D" | "tex_protected:D" => {}
                _ => return false,
            }
        }
    }

    #[allow(clippy::too_many_lines)]
    fn hints(&self, it: &Item, noted: &Noted, quantity: bool, closure: &mut Closure<'_>) -> Hints {
        let a = it.atom;
        let pos = self.atoms[a].start;
        let name = it.name.as_str();
        let next_open = {
            let q = skip(self.atoms, a + 1);
            (self.kind(q) == Some(AtomKind::Open)).then_some(q)
        };
        let boxk = match name {
            "hbox" => Some(ScopeKind::HBox),
            "vbox" | "vtop" | "vcenter" => Some(ScopeKind::VBox),
            _ => None,
        };
        // (the claim: the atoms consumed; a box claims up to its `{`, the
        // group claims the rest)
        let mut n = if it.consumes {
            it.end.saturating_sub(a + 1)
        } else {
            0
        };
        if boxk.is_some()
            && it.end > a + 1
            && self.kind(it.end - 1) == Some(AtomKind::Close)
            && let o = self.atoms[it.end - 1].mate
            && o != usize::MAX
            && o > a
        {
            n = o - a;
        }
        let mut assume: Vec<(u8, Assumption)> = Vec::new();
        for (kind, d) in noted {
            if *kind == "visible environment definition" {
                let at = Some((self.scope.file, pos));
                let args = match self.scope.document.find_env(d, at) {
                    Found::One(x) => Some(x),
                    _ => match self.scope.prelude.find_env(d, None) {
                        Found::One(x) => Some(x),
                        _ => None,
                    },
                };
                assume.push((
                    1,
                    Assumption::MeaningIs {
                        cs: d.clone(),
                        expected: MeaningShape::Macro {
                            params: args.map_or_else(String::new, |x| hints::params_of(x)),
                        },
                    },
                ));
                assume.push((
                    2,
                    Assumption::MeaningIs {
                        cs: "begin".to_owned(),
                        expected: MeaningShape::Kernel,
                    },
                ));
            } else {
                assume.push(self.assumption(d, pos));
            }
        }
        if it.status == Status::Predicted && assume.is_empty() {
            assume.push(self.assumption(name, pos));
        }
        assume.sort_by_key(|x| x.0);
        let mut seen: Vec<Assumption> = Vec::new();
        for (_, x) in assume {
            if !seen.contains(&x) {
                seen.push(x);
            }
        }
        let claim = match it.status {
            Status::Known => Claim::Known(n),
            Status::Predicted => {
                let mut v = seen.iter().cloned();
                match v.next() {
                    Some(confirm) => Claim::Predicted {
                        n_tokens: n,
                        confirm,
                        also: v.collect(),
                    },
                    None => Claim::Known(n),
                }
            }
            Status::Pending => Claim::Pending {
                cause: it.cause.as_ref().map_or("unknown", |c| c.kind),
            },
        };
        // (what it may define: its own statement, and the bodies of the
        // macros it calls)
        let global = self.prefixed_global(a);
        let set = if quantity && !self.table.commands.contains_key(name) {
            if it.consumes {
                let mut s = closure.assigned(name);
                if global {
                    s.names.values_mut().for_each(|g| *g = true);
                    s.prefixes.values_mut().for_each(|g| *g = true);
                }
                s
            } else {
                hints::DefSet::default()
            }
        } else if quantity && !it.consumes {
            hints::DefSet::default()
        } else {
            // (its arguments may be expanded as it reads them: what
            // they define counts here; a box's content runs after it)
            let hi = if boxk.is_some() { a + 1 } else { it.end.max(a + 1) };
            let e = hints::effects(self.text, self.atoms, a, hi, global);
            closure.of(&e)
        };
        let may_define = set.may_define();
        // (scope)
        let own = |effect: Effect| -> ScopeHint {
            if it.status == Status::Known {
                match effect {
                    Effect::Open(k) => ScopeHint::Open(k),
                    Effect::Close => ScopeHint::Close,
                }
            } else {
                ScopeHint::Predicted {
                    effect,
                    confirm: self.assumption(name, pos).1,
                }
            }
        };
        let alias = |to: &str| Assumption::MeaningIs {
            cs: name.to_owned(),
            expected: MeaningShape::Alias(to.to_owned()),
        };
        let kernel = || Assumption::MeaningIs {
            cs: name.to_owned(),
            expected: MeaningShape::Kernel,
        };
        let env = next_open.map(|o| super::atoms::group_text(self.atoms, o));
        let scope = match name {
            "begingroup" | "group_begin:" | "tex_begingroup:D" => {
                own(Effect::Open(ScopeKind::SemiSimple))
            }
            "endgroup" | "group_end:" | "tex_endgroup:D" => own(Effect::Close),
            "bgroup" => ScopeHint::Predicted {
                effect: Effect::Open(ScopeKind::Simple),
                confirm: alias("{"),
            },
            "egroup" => ScopeHint::Predicted {
                effect: Effect::Close,
                confirm: alias("}"),
            },
            "begin" => match env {
                Some(Some(e)) => ScopeHint::Predicted {
                    effect: Effect::Open(ScopeKind::Env(e)),
                    confirm: kernel(),
                },
                Some(None) => ScopeHint::Pending {
                    cause: "environment name computed",
                },
                None => ScopeHint::Pending {
                    cause: "environment name not braced",
                },
            },
            "end" => match env {
                Some(Some(_)) => ScopeHint::Predicted {
                    effect: Effect::Close,
                    confirm: kernel(),
                },
                Some(None) => ScopeHint::Pending {
                    cause: "environment name computed",
                },
                None => ScopeHint::None,
            },
            _ => match boxk {
                Some(_) if it.status == Status::Pending => ScopeHint::Pending {
                    cause: it.cause.as_ref().map_or("unknown", |c| c.kind),
                },
                Some(k) => own(Effect::Open(k)),
                None => ScopeHint::None,
            },
        };
        let mut implicit = hints::implicit_of(name, next_open.is_some());
        if implicit == hints::Implicit::None
            && (self.scope.document.ends_paragraph(name, 8)
                || self.scope.prelude.ends_paragraph(name, 8))
        {
            // (a macro whose body ends with a paragraph's end)
            implicit = hints::Implicit::EndsParagraph;
        }
        let mut reads = vec![hints::cs(name)];
        for (_, d) in noted {
            let f = hints::cs(d);
            if !reads.contains(&f) {
                reads.push(f);
            }
        }
        if quantity
            && !it.consumes
            && let Some(Entry::Quantity(_, None)) = self.table.commands.get(name)
        {
            reads.push(hints::param(name));
        }
        Hints {
            claim,
            may_define,
            scope,
            implicit,
            reads,
            cause: it.cause.as_ref().map(|c| c.kind),
            relied: set.relied.clone(),
            calls: set.calls.clone(),
        }
    }
}

/// The meaning a harvested shape is.
fn shape_of(s: &Shape) -> MeaningShape {
    match s {
        Shape::Macro(args) => MeaningShape::Macro {
            params: hints::params_of(args),
        },
        Shape::Alias(t) => MeaningShape::Alias(t.clone()),
        Shape::Quantity(t) => MeaningShape::Register(*t),
        Shape::Cond => MeaningShape::Conditional,
        Shape::Opaque(_) => MeaningShape::Kernel,
    }
}
