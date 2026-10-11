//! Tokens and control-sequence names.
use std::sync::{Arc, RwLock};

/// A control sequence's name, interned (`Names`).
pub type Cs = u32;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Tok {
    /// A character token: category (1..=13 but 5, 9, 14, 15) and character.
    Char { cat: u8, ch: char },
    Cs(Cs),
    Active(char),
    /// `#n` in a parameter text or body (n 1..=9); `Param(0)` is `##` → `#`.
    Param(u8),
    /// `\noexpand`'s mark on the next token (only in input, never in a value).
    DontExpand,
}

pub type Toks = Arc<[Tok]>;

/// The interner. Append-only and shared by all threads; a name's number
/// never changes during a session.
#[derive(Default, Debug)]
pub struct Names {
    inner: RwLock<(Vec<Arc<str>>, std::collections::HashMap<Arc<str>, Cs>)>,
}

impl Names {
    pub fn intern(&self, name: &str) -> Cs {
        if let Some(&c) = self.inner.read().unwrap().1.get(name) {
            return c;
        }
        let mut w = self.inner.write().unwrap();
        if let Some(&c) = w.1.get(name) {
            return c;
        }
        let a: Arc<str> = name.into();
        let c = w.0.len() as Cs;
        w.0.push(a.clone());
        w.1.insert(a, c);
        c
    }
    pub fn name(&self, c: Cs) -> Arc<str> {
        self.inner.read().unwrap().0[c as usize].clone()
    }
}
