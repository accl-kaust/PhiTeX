//! The token lists a node list holds, changed in place by a function
//! (relocatable numbers, `partex_core::reloc`): marks, `\write` and
//! `\special` texts, pdfTeX's whatsits, in boxes and every other list
//! nested in a node.

use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::node::{Action, Node, PdfId, PdfWhatsit, Tokens, Whatsit};

/// What becomes of a token list: `Ok(None)` unchanged, `Ok(Some(new))`
/// replaced, `Err` the whole walk fails.
pub type TokFn<'a> = dyn FnMut(&Tokens) -> Result<Option<Tokens>, ()> + 'a;

/// `nodes` with each token list `f` changes changed (`None`: nothing
/// does). A box changed is versioned again.
///
/// # Errors
///
/// When `f` fails.
#[allow(
    clippy::result_unit_err,
    reason = "the walk fails, nothing more to say"
)]
pub fn map_list(nodes: &[Node], f: &mut TokFn<'_>) -> Result<Option<Vec<Node>>, ()> {
    let mut out: Option<Vec<Node>> = None;
    for (i, n) in nodes.iter().enumerate() {
        if let Some(m) = map_node(n, f)? {
            out.get_or_insert_with(|| nodes[..i].to_vec()).push(m);
        } else if let Some(o) = &mut out {
            o.push(n.clone());
        }
    }
    Ok(out)
}

fn tok(t: &Tokens, f: &mut TokFn<'_>) -> Result<Option<Tokens>, ()> {
    f(t)
}

/// The list that replaces `t`, if there is one and `f` changes it.
fn opt_tok(t: Option<&Tokens>, f: &mut TokFn<'_>) -> Result<Option<Tokens>, ()> {
    t.map_or(Ok(None), f)
}

fn id(i: &PdfId, f: &mut TokFn<'_>) -> Result<Option<PdfId>, ()> {
    match i {
        PdfId::Name(t) => Ok(f(t)?.map(PdfId::Name)),
        PdfId::Num(_) => Ok(None),
    }
}

fn action(a: &Action, f: &mut TokFn<'_>) -> Result<Option<Action>, ()> {
    let tokens = opt_tok(a.tokens.as_ref(), f)?;
    let file = opt_tok(a.file.as_ref(), f)?;
    let ident = id(&a.id, f)?;
    let st = match &a.struct_id {
        Some(s) => id(s, f)?.map(Some),
        None => None,
    };
    if tokens.is_none() && file.is_none() && ident.is_none() && st.is_none() {
        return Ok(None);
    }
    let mut b = a.clone();
    if let Some(t) = tokens {
        b.tokens = Some(t);
    }
    if let Some(t) = file {
        b.file = Some(t);
    }
    if let Some(i) = ident {
        b.id = i;
    }
    if let Some(s) = st {
        b.struct_id = s;
    }
    Ok(Some(b))
}

fn pdf(p: &PdfWhatsit, f: &mut TokFn<'_>) -> Result<Option<PdfWhatsit>, ()> {
    let mut q = p.clone();
    let changed = match &mut q {
        PdfWhatsit::Literal { data, .. }
        | PdfWhatsit::SetMatrix { data }
        | PdfWhatsit::Annot { data, .. } => match tok(data, f)? {
            Some(t) => {
                *data = t;
                true
            }
            None => false,
        },
        PdfWhatsit::ColorStack { data, .. } => match opt_tok(data.as_ref(), f)? {
            Some(t) => {
                *data = Some(t);
                true
            }
            None => false,
        },
        PdfWhatsit::StartLink {
            attr, action: a, ..
        } => {
            let t = opt_tok(attr.as_ref(), f)?;
            let b = action(a, f)?;
            let c = t.is_some() || b.is_some();
            if let Some(t) = t {
                *attr = Some(t);
            }
            if let Some(b) = b {
                *a = Arc::new(b);
            }
            c
        }
        PdfWhatsit::Dest { id: i, .. } => match id(i, f)? {
            Some(n) => {
                *i = n;
                true
            }
            None => false,
        },
        PdfWhatsit::Thread { attr, id: i, .. } => {
            let t = opt_tok(attr.as_ref(), f)?;
            let n = id(i, f)?;
            let c = t.is_some() || n.is_some();
            if let Some(t) = t {
                *attr = Some(t);
            }
            if let Some(n) = n {
                *i = n;
            }
            c
        }
        PdfWhatsit::Save
        | PdfWhatsit::Restore
        | PdfWhatsit::RefObj { .. }
        | PdfWhatsit::RefXForm { .. }
        | PdfWhatsit::RefXImage { .. }
        | PdfWhatsit::EndLink
        | PdfWhatsit::EndThread
        | PdfWhatsit::SavePos
        | PdfWhatsit::SnapRefPoint
        | PdfWhatsit::SnapY { .. }
        | PdfWhatsit::SnapYComp { .. }
        | PdfWhatsit::InterwordSpaceOn
        | PdfWhatsit::InterwordSpaceOff
        | PdfWhatsit::FakeSpace
        | PdfWhatsit::RunningLinkOff
        | PdfWhatsit::RunningLinkOn => false,
    };
    Ok(changed.then_some(q))
}

/// `n` with each token list `f` changes changed (`None`: nothing does).
///
/// # Errors
///
/// When `f` fails.
#[allow(
    clippy::result_unit_err,
    reason = "the walk fails, nothing more to say"
)]
pub fn map_node(n: &Node, f: &mut TokFn<'_>) -> Result<Option<Node>, ()> {
    Ok(match n {
        Node::Box(b) => map_list(&b.list, f)?.map(|l| {
            let mut c = (**b).clone();
            c.list = l;
            c.reversion();
            Node::Box(Arc::new(c))
        }),
        Node::Leaders(l) => map_node(&l.leader, f)?.map(|m| {
            let mut c = (**l).clone();
            c.leader = m;
            Node::Leaders(alloc::boxed::Box::new(c))
        }),
        Node::Disc(d) => {
            let pre = map_list(&d.pre, f)?;
            let post = map_list(&d.post, f)?;
            let replace = map_list(&d.replace, f)?;
            if pre.is_none() && post.is_none() && replace.is_none() {
                None
            } else {
                let mut c = (**d).clone();
                if let Some(l) = pre {
                    c.pre = l;
                }
                if let Some(l) = post {
                    c.post = l;
                }
                if let Some(l) = replace {
                    c.replace = l;
                }
                Some(Node::Disc(alloc::boxed::Box::new(c)))
            }
        }
        Node::Ins(i) => map_list(&i.list, f)?.map(|l| {
            let mut c = (**i).clone();
            c.list = l;
            Node::Ins(alloc::boxed::Box::new(c))
        }),
        Node::Mark(m) => tok(&m.tokens, f)?.map(|t| {
            let mut c = (**m).clone();
            c.tokens = t;
            Node::Mark(alloc::boxed::Box::new(c))
        }),
        Node::Adjust(a) => map_list(&a.list, f)?.map(|l| {
            let mut c = (**a).clone();
            c.list = l;
            Node::Adjust(alloc::boxed::Box::new(c))
        }),
        Node::Unset(u) => map_list(&u.list, f)?.map(|l| {
            let mut c = (**u).clone();
            c.list = l;
            Node::Unset(alloc::boxed::Box::new(c))
        }),
        Node::Whatsit(w) => {
            let m = match &**w {
                Whatsit::Write { stream, tokens } => tok(tokens, f)?.map(|t| Whatsit::Write {
                    stream: *stream,
                    tokens: t,
                }),
                Whatsit::Special { tokens } => {
                    tok(tokens, f)?.map(|t| Whatsit::Special { tokens: t })
                }
                Whatsit::LateSpecial { tokens } => {
                    tok(tokens, f)?.map(|t| Whatsit::LateSpecial { tokens: t })
                }
                Whatsit::Pdf(p) => pdf(p, f)?.map(|q| Whatsit::Pdf(alloc::boxed::Box::new(q))),
                Whatsit::Open { .. }
                | Whatsit::Close { .. }
                | Whatsit::Language { .. }
                | Whatsit::NativeWord(_)
                | Whatsit::Glyph(_)
                | Whatsit::Pic(_) => None,
            };
            m.map(|w| Node::Whatsit(alloc::boxed::Box::new(w)))
        }
        Node::Glyphs(_)
        | Node::Rule { .. }
        | Node::Glue { .. }
        | Node::Kern { .. }
        | Node::MarginKern { .. }
        | Node::Penalty(_)
        | Node::Math { .. }
        | Node::Ligature(_) => None,
    })
}
