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
use super::lex::Lexed;
use super::table::{Entry, Table, table};
use super::{Cause, Item, Scope, Status};

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
}

impl R {
    fn worse(&mut self, s: Status, c: Option<Cause>) {
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
}

pub(crate) fn classify(text: &str, atoms: &[Atom], lexed: &Lexed, scope: Scope<'_>) -> Vec<Item> {
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
    };
    let mut items = Vec::new();
    for a in 0..atoms.len() {
        if atoms[a].kind == AtomKind::Cs && !cx.operand[a] {
            let mut it = cx.item(a);
            it.hazard = hazard_tok[atoms[a].tok];
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

    #[allow(clippy::too_many_lines)]
    fn resolve(&self, name: &str, pos: usize, depth: u8) -> Res {
        if depth > 8 {
            return Res::Pending(cause("\\let chain too long", name));
        }
        // (document order binds: a definition before the use in its own
        // file; else the fixed command or the kernel's, as predicted when
        // the document redefines it somewhere; else the document's own
        // definitions where they agree)
        let doc = self.scope.document.find(name, Some((self.scope.file, pos)));
        if let Found::One(s) = doc
            && self
                .scope
                .document
                .defined_before(name, self.scope.file, pos)
        {
            return self.shape(s, pos, depth);
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
            return self.shape(s, pos, depth);
        }
        match doc {
            Found::One(s) => return self.shape(s, pos, depth),
            Found::Conflict => {
                if let Found::One(s) = self.scope.prelude.find(name, None) {
                    return self.shape(s, pos, depth);
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

    fn shape(&self, s: &Shape, pos: usize, depth: u8) -> Res {
        match s {
            Shape::Macro(args) => Res::Cmd(args.clone(), Status::Predicted),
            Shape::Alias(t) => match self.resolve(t, pos, depth + 1) {
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

    fn item(&mut self, a: usize) -> Item {
        let name = self.name(a).to_owned();
        let tok = self.lexed.tokens[self.atoms[a].tok];
        let mut r = R {
            status: Status::Known,
            cause: None,
        };
        let item = |end, consumes, r: R| Item {
            atom: a,
            name: name.clone(),
            status: r.status,
            cause: r.cause,
            consumes,
            end,
            hazard: false,
        };
        if tok.tz == Status::Pending {
            r.pending("tokenization pending (catcode)", &name);
            return item(a + 1, true, r);
        }
        match self.resolve(&name, tok.start, 0) {
            Res::Pending(c) => {
                r.worse(Status::Pending, Some(c));
                item(a + 1, true, r)
            }
            Res::Quantity(t, ix, st) => {
                r.worse(st, None);
                if t == 'c' || self.alone_in_group(a) {
                    return item(a + 1, false, r);
                }
                let mut p = self.index(a + 1, ix, &mut r);
                if matches!(self.kind(skip(self.atoms, p)), None | Some(AtomKind::Close)) {
                    // (a quantity mentioned, not assigned: an argument)
                    return item(p, false, r);
                }
                p = self.read(p, &Arg::Eq, &mut r);
                p = match t {
                    't' => self.read(p, &Arg::Toks, &mut r),
                    _ => self.value(p, t, &mut r),
                };
                item(p, true, r)
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
        let pos = self.atoms[q].start;
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
                            r.pending("expression lookahead into macro", &nm);
                            return p + 1;
                        }
                    }
                }
                _ => return p,
            }
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
