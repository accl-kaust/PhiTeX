//! The document's own definitions: what each says, and the table of them
//! by name, from which the layer expands the macros that make structure
//! (`\newcommand{\chapterfile}[2]{…\input{#2}}`).
//!
//! The table does not depend on order: a name defined once (or always the
//! same way) has that meaning everywhere; a name defined differently in
//! two places is ambiguous, and its calls are not expanded (DESIGN 4.3.1).

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;

use phitex_syntax::ParaId;

use crate::known::Definer;
use crate::{FileId, FxMap, Name};

/// How a macro reads its arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Args {
    /// LaTeX's `[n][default]`: `n` arguments, the first optional (with
    /// this default) if a default is given.
    Latex { n: u8, default: Option<String> },
    /// xparse's specification, an argument each.
    Xparse(Vec<Spec>),
    /// `\def`'s undelimited parameters.
    Plain(u8),
    /// A parameter text the layer does not read (delimited parameters,
    /// xparse's rarer types): calls are not expanded.
    Unread,
}

/// An xparse argument.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Spec {
    /// `m`: mandatory.
    M,
    /// `o` and `O{default}`: optional, `-NoValue-` or the default when
    /// absent.
    O(Option<String>),
    /// `s`: a star (`\BooleanTrue` or `\BooleanFalse`).
    S,
}

/// A definition the document makes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Def {
    /// The macro's name (no backslash), or the environment's.
    pub name: Name,
    /// An environment's definition.
    pub env: bool,
    /// The command that made it (`newcommand`, `def`, …).
    pub cs: Name,
    pub definer: Definer,
    pub args: Args,
    /// The macro's body, or the environment's begin code.
    pub body: String,
    /// The environment's end code.
    pub end: String,
    /// `\let`'s target, or the one control sequence a macro without
    /// parameters stands for (`\newcommand{\sect}{\section}`).
    pub target: Option<Name>,
    /// What its code calls (as [`call_key`]s): the names its expansion
    /// depends on.
    pub calls: Vec<Name>,
}

/// The key a call is recorded under: a macro's name, or an environment's
/// in braces (`{exercise}`).
#[must_use]
pub fn call_key(env: bool, name: &str) -> Name {
    if env {
        Name::from(format!("{{{name}}}"))
    } else {
        Name::from(name)
    }
}

impl Def {
    /// Whether LaTeX expands it when it writes a title to the `.toc`
    /// (`\protected@write`): not a robust command's nor xparse's.
    #[must_use]
    pub fn expands_in_writes(&self) -> bool {
        !self.env
            && matches!(
                self.definer,
                Definer::NewCommand { robust: false, .. } | Definer::Def
            )
            && self.args != Args::Unread
    }

    /// `\providecommand`'s: it does not replace a definition.
    fn provides(&self) -> bool {
        matches!(self.definer, Definer::NewCommand { provide: true, .. })
    }
}

/// A definition and where it is: the file and paragraph whose text makes it.
#[derive(Clone, Debug)]
pub(crate) struct Placed {
    pub file: FileId,
    pub para: ParaId,
    pub def: Rc<Def>,
}

/// What a name means in the table.
#[derive(Clone, Debug)]
pub enum Meaning<'a> {
    Undefined,
    One(&'a Rc<Def>),
    /// Defined differently in this many places.
    Ambiguous(usize),
}

/// A name in the table: a macro's, or an environment's.
pub(crate) type Key = (bool, Name);

/// The document's definitions, by name.
#[derive(Default)]
pub(crate) struct Defs {
    macros: FxMap<Name, Vec<Placed>>,
    envs: FxMap<Name, Vec<Placed>>,
    /// Whether each name's expansion makes facts (`scan.rs`), by
    /// [`call_key`], cleared at every change.
    pub structural: RefCell<FxMap<Name, bool>>,
}

impl Defs {
    fn map(&self, env: bool) -> &FxMap<Name, Vec<Placed>> {
        if env { &self.envs } else { &self.macros }
    }

    fn map_mut(&mut self, env: bool) -> &mut FxMap<Name, Vec<Placed>> {
        if env {
            &mut self.envs
        } else {
            &mut self.macros
        }
    }

    /// What `name` means.
    pub fn meaning(&self, env: bool, name: &str) -> Meaning<'_> {
        self.map(env)
            .get(name)
            .map_or(Meaning::Undefined, |v| meaning_of(v))
    }

    /// Every definition of a name, where it is.
    pub fn placed(&self, env: bool, name: &str) -> &[Placed] {
        self.map(env).get(name).map_or(&[], Vec::as_slice)
    }

    /// All the definitions.
    pub fn all(&self) -> impl Iterator<Item = &Placed> {
        self.macros.values().chain(self.envs.values()).flatten()
    }

    /// Replace the definitions paragraph `para` of `file` made (`old`)
    /// with `new`: the names whose meaning changed.
    pub fn replace(
        &mut self,
        file: FileId,
        para: ParaId,
        old: &[Rc<Def>],
        new: &[Rc<Def>],
    ) -> Vec<Key> {
        if old.is_empty() && new.is_empty() {
            return Vec::new();
        }
        let keys: BTreeSet<Key> = old
            .iter()
            .chain(new)
            .map(|d| (d.env, d.name.clone()))
            .collect();
        let before: Vec<Option<Rc<Def>>> = keys.iter().map(|k| self.effective(k)).collect();
        let ambiguous_before: Vec<bool> = keys.iter().map(|k| self.ambiguous(k)).collect();
        for (env, name) in &keys {
            let map = self.map_mut(*env);
            if let Some(v) = map.get_mut(name) {
                v.retain(|p| !(p.file == file && p.para == para));
                if v.is_empty() {
                    map.remove(name);
                }
            }
        }
        for d in new {
            self.map_mut(d.env)
                .entry(d.name.clone())
                .or_default()
                .push(Placed {
                    file,
                    para,
                    def: d.clone(),
                });
        }
        let changed: Vec<Key> = keys
            .into_iter()
            .zip(before)
            .zip(ambiguous_before)
            .filter(|((k, b), a)| self.effective(k) != *b || self.ambiguous(k) != *a)
            .map(|((k, _), _)| k)
            .collect();
        if !changed.is_empty() {
            self.structural.borrow_mut().clear();
        }
        changed
    }

    fn effective(&self, (env, name): &Key) -> Option<Rc<Def>> {
        match self.meaning(*env, name) {
            Meaning::One(d) => Some(d.clone()),
            _ => None,
        }
    }

    fn ambiguous(&self, (env, name): &Key) -> bool {
        matches!(self.meaning(*env, name), Meaning::Ambiguous(_))
    }

    /// The calls whose expansion depends on one of `changed` (as
    /// [`call_key`]s): those, and every definition whose code calls one of
    /// them, again and again.
    pub fn dependents(&self, changed: &[Key]) -> BTreeSet<Name> {
        let mut out: BTreeSet<Name> = changed.iter().map(|(e, n)| call_key(*e, n)).collect();
        loop {
            let before = out.len();
            for p in self.all() {
                let d = &p.def;
                let k = call_key(d.env, &d.name);
                if out.contains(&k) {
                    continue;
                }
                if d.calls.iter().any(|c| out.contains(c))
                    || d.target.as_ref().is_some_and(|t| out.contains(t))
                {
                    out.insert(k);
                }
            }
            if out.len() == before {
                return out;
            }
        }
    }
}

/// What a list of definitions of one name means: the one meaning they
/// all give (`\providecommand`'s only if nothing else defines it), or
/// how many different ones.
fn meaning_of(v: &[Placed]) -> Meaning<'_> {
    let strong = v.iter().any(|p| !p.def.provides());
    let mut first: Option<&Rc<Def>> = None;
    let mut distinct = 0;
    let mut seen: Vec<&Rc<Def>> = Vec::new();
    for p in v.iter().filter(|p| !strong || !p.def.provides()) {
        if !seen.iter().any(|d| ***d == *p.def) {
            seen.push(&p.def);
            distinct += 1;
            first.get_or_insert(&p.def);
        }
    }
    match (distinct, first) {
        (1, Some(d)) => Meaning::One(d),
        (0, _) => Meaning::Undefined,
        (n, _) => Meaning::Ambiguous(n),
    }
}

/// A body with its parameters replaced by the arguments: `#1`…`#9` by
/// theirs, `##` by `#`.
#[must_use]
pub fn substitute(body: &str, args: &[String]) -> String {
    let mut out = String::with_capacity(body.len() + args.iter().map(String::len).sum::<usize>());
    let mut chars = body.char_indices().peekable();
    while let Some((_, c)) = chars.next() {
        if c != '#' {
            out.push(c);
            continue;
        }
        match chars.peek().copied() {
            Some((_, '#')) => {
                chars.next();
                out.push('#');
            }
            Some((_, d @ '1'..='9')) => {
                chars.next();
                let k = (d as usize) - ('1' as usize);
                if let Some(a) = args.get(k) {
                    out.push_str(a);
                }
            }
            _ => out.push('#'),
        }
    }
    out
}

/// An xparse argument specification, if the layer reads all of it.
#[must_use]
pub fn xparse(spec: &str) -> Args {
    let mut out = Vec::new();
    let b: Vec<char> = spec.chars().filter(|c| !c.is_whitespace()).collect();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            'm' => out.push(Spec::M),
            'o' => out.push(Spec::O(None)),
            's' => out.push(Spec::S),
            '+' | '!' => {}
            'O' => {
                // (`O{default}`)
                let Some(len) = group_len(&b[i + 1..]) else {
                    return Args::Unread;
                };
                let default: String = b[i + 2..i + len].iter().collect();
                out.push(Spec::O(Some(default)));
                i += len;
            }
            _ => return Args::Unread,
        }
        i += 1;
    }
    Args::Xparse(out)
}

/// The length of the brace group at the start (braces included).
fn group_len(b: &[char]) -> Option<usize> {
    if b.first() != Some(&'{') {
        return None;
    }
    let mut depth = 0usize;
    for (k, &c) in b.iter().enumerate() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(k + 1);
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substitution() {
        assert_eq!(
            substitute(
                "\\IfFileExists{#2.tex}{\\input{#2}}{}",
                &["1".into(), "ch01".into()]
            ),
            "\\IfFileExists{ch01.tex}{\\input{ch01}}{}"
        );
        assert_eq!(substitute("a##1#", &[]), "a#1#");
    }

    #[test]
    fn xparse_specs() {
        assert_eq!(
            xparse("s O{x} m"),
            Args::Xparse(vec![Spec::S, Spec::O(Some("x".into())), Spec::M])
        );
        assert_eq!(xparse("r()"), Args::Unread);
    }
}
