//! B2: macros as function values: `\def` family (with `\long` `\outer`
//! `\protected`), parameter matching, `Apply`.
//!
//! `\def\f#1.#2{b}` is a delta `Key::Cs(f) := Meaning::Macro(..)`; a call
//! matches the arguments from the input (tex.web §389–400) and pushes the
//! substituted body: `substitute(body, args)` is a pure function.
use std::sync::Arc;

use crate::ctx::Ctx;
use crate::diag::error;
use crate::prim::{Kind, Registry};
use crate::term::TokKind;
use crate::tok::{Tok, Toks};
use crate::val::{Key, Macro, Meaning, Val};

pub fn register(r: &mut Registry) {
    r.add("def", 0, Kind::Command(def_cmd), 0);
    r.add("gdef", 1, Kind::Command(def_cmd), 0);
    r.add("edef", 2, Kind::Command(def_cmd), 0);
    r.add("xdef", 3, Kind::Command(def_cmd), 0);
}

fn def_cmd(ctx: &mut dyn Ctx, code: u16) {
    define_macro(ctx, code, 0);
}

const SPACE: Tok = Tok::Char { cat: 10, ch: ' ' };

fn is_left(t: &Tok) -> bool {
    matches!(t, Tok::Char { cat: 1, .. })
}
fn is_right(t: &Tok) -> bool {
    matches!(t, Tok::Char { cat: 2, .. })
}
fn is_param_char(t: &Tok) -> bool {
    matches!(t, Tok::Char { cat: 6, .. })
}

/// `\x` as TeX prints it in messages.
pub fn tok_name(ctx: &dyn Ctx, t: Tok) -> String {
    match t {
        Tok::Cs(c) => format!("\\{}", ctx.names().name(c)),
        Tok::Active(ch) => ch.to_string(),
        Tok::Char { ch, .. } => ch.to_string(),
        Tok::Param(n) => format!("#{n}"),
        Tok::DontExpand => String::new(),
    }
}

fn par_tok(ctx: &dyn Ctx) -> Tok {
    Tok::Cs(ctx.names().intern("par"))
}

/// `\def` 0, `\gdef` 1, `\edef` 2, `\xdef` 3; prefixes 1 global 2 long 4 outer 8 protected.
pub fn define_macro(ctx: &mut dyn Ctx, which: u16, prefixes: u8) {
    let global = which & 1 != 0 || prefixes & 1 != 0;
    let xpand = which >= 2;
    let name = crate::assign::get_r_token(ctx);
    let nm = tok_name(ctx, name);

    // ---- parameter text (§474–476)
    let mut params: Vec<Tok> = Vec::new();
    let mut nparams: u8 = 0;
    let mut brace_end: Option<Tok> = None;
    let mut body_missing = false; // `}` met before `{`: empty body
    loop {
        let Some(t) = ctx.next() else {
            body_missing = true;
            break;
        };
        if is_left(&t) {
            break;
        }
        if is_right(&t) {
            error(ctx, "Missing { inserted", &[
                "Where was the left brace? You said something like `\\def\\a}',",
                "which I'm going to interpret as `\\def\\a{}'.",
            ]);
            body_missing = true;
            break;
        }
        if is_param_char(&t) {
            let Some(u) = ctx.next() else {
                body_missing = true;
                break;
            };
            if is_left(&u) {
                brace_end = Some(u);
                break;
            }
            if nparams == 9 {
                error(ctx, "You already have nine parameters", &[
                    "I'm going to ignore the # sign you just used,",
                    "as well as the token that followed it.",
                ]);
                continue;
            }
            nparams += 1;
            let want = Tok::Char { cat: 12, ch: char::from(b'0' + nparams) };
            if u != want {
                ctx.back(u);
                error(ctx, "Parameters must be numbered consecutively", &[
                    "I've inserted the digit you should have used after the #.",
                    "Type `1' to delete what you did use.",
                ]);
            }
            params.push(Tok::Param(nparams));
            continue;
        }
        params.push(t);
    }

    // ---- body (§473, 477–479)
    let mut raw: Vec<Tok> = Vec::new();
    if !body_missing {
        let mut depth = 1u32;
        while let Some(t) = ctx.next() {
            if is_left(&t) {
                depth += 1;
            } else if is_right(&t) {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            raw.push(t);
        }
    }
    let raw: Toks = Arc::from(raw);
    let expanded = if xpand { crate::scan::expand_toks(ctx, &raw) } else { raw };
    let mut body: Vec<Tok> = Vec::with_capacity(expanded.len() + 1);
    let mut i = 0;
    while i < expanded.len() {
        let t = expanded[i];
        i += 1;
        if !is_param_char(&t) {
            body.push(t);
            continue;
        }
        let Some(&u) = expanded.get(i) else {
            body.push(t);
            break;
        };
        if is_param_char(&u) {
            body.push(u);
            i += 1;
            continue;
        }
        match u {
            Tok::Char { cat: 12, ch } if ('1'..='9').contains(&ch) && (ch as u8 - b'0') <= nparams => {
                body.push(Tok::Param(ch as u8 - b'0'));
                i += 1;
            }
            _ => {
                // back_error: the token is read again; the `#` stays a character.
                error(ctx, &format!("Illegal parameter number in definition of {nm}"), &[
                    "You meant to type ## instead of #, right?",
                    "Or maybe a } was forgotten somewhere earlier, and things",
                    "are all screwed up? I'm going to assume that you meant ##.",
                ]);
                body.push(t);
            }
        }
    }
    if let Some(b) = brace_end {
        body.push(b);
    }

    let m = Macro {
        params: Arc::from(params),
        body: Arc::from(body),
        brace_end: brace_end.is_some(),
        long: prefixes & 2 != 0,
        outer: prefixes & 4 != 0,
        protected: prefixes & 8 != 0,
    };
    let key = match name {
        Tok::Active(c) => Key::Active(c),
        Tok::Cs(c) => Key::Cs(c),
        _ => return,
    };
    crate::assign::assign(ctx, key, Val::Meaning(Meaning::Macro(Arc::new(m))), global);
}

/// The substituted body: `Param(n)` → `args[n-1]`. Pure.
pub fn substitute(body: &[Tok], args: &[Toks]) -> Toks {
    let mut out = Vec::with_capacity(body.len() + args.iter().map(|a| a.len()).sum::<usize>());
    for &t in body {
        match t {
            Tok::Param(0) => out.push(Tok::Char { cat: 6, ch: '#' }),
            Tok::Param(n) => {
                if let Some(a) = args.get(n as usize - 1) {
                    out.extend_from_slice(a);
                }
            }
            t => out.push(t),
        }
    }
    Arc::from(out)
}

#[derive(Clone, Copy, PartialEq)]
enum P {
    T(Tok),
    Match,
    End,
}

fn is_outer(ctx: &mut dyn Ctx, t: &Tok) -> bool {
    matches!(t, Tok::Cs(_) | Tok::Active(_))
        && matches!(ctx.meaning(t), Meaning::Macro(m) if m.outer)
}

const RUNAWAY_HELP: &[&str] = &[
    "I suspect you have forgotten a `}', causing me",
    "to read past where you wanted me to stop.",
    "I'll try to recover; but if the error is serious,",
    "you'd better type `E' or `X' now and fix your file.",
];

/// Matches `m`'s parameters against the input and pushes the substituted body.
pub fn expand_macro(ctx: &mut dyn Ctx, name: Tok, m: &Macro) {
    if let Some(args) = match_args(ctx, name, m) {
        let body = substitute(&m.body, &args);
        ctx.push(body, TokKind::Macro);
    }
}

/// tex.web §391–400: the arguments, or `None` when the call was abandoned
/// (error reported, recovery done).
pub fn match_args(ctx: &mut dyn Ctx, name: Tok, m: &Macro) -> Option<Vec<Toks>> {
    let mut args: Vec<Toks> = Vec::new();
    if m.params.is_empty() && !m.brace_end {
        return Some(args);
    }
    let mut pat: Vec<P> = m
        .params
        .iter()
        .map(|t| if matches!(t, Tok::Param(_)) { P::Match } else { P::T(*t) })
        .collect();
    if m.brace_end {
        pat.push(P::T(m.body.last().copied().unwrap_or(Tok::Char { cat: 1, ch: '{' })));
    }
    pat.push(P::End);

    let par = par_tok(ctx);
    let mut long = m.long;
    let mut r = 0usize;
    loop {
        // s: start of the current parameter's delimiter (None: matching the prefix)
        let s: Option<usize>;
        let mut cur: Vec<Tok> = Vec::new();
        let mut units = 0u32;
        if let P::Match = pat[r] {
            s = Some(r + 1);
            r += 1;
        } else {
            s = None;
        }
        'cont: loop {
            let Some(mut t) = ctx.next() else {
                let nm = tok_name(ctx, name);
                error(ctx, &format!("File ended while scanning use of {nm}"), RUNAWAY_HELP);
                return None;
            };
            if is_outer(ctx, &t) {
                // check_outer_validity in `matching`: reread the name after an inserted \par.
                ctx.back(t);
                ctx.push(Arc::from(vec![par]), TokKind::Inserted);
                let nm = tok_name(ctx, name);
                error(ctx, &format!("Forbidden control sequence found while scanning use of {nm}"), RUNAWAY_HELP);
                long = false;
                t = SPACE;
            }
            // §394
            if pat[r] == P::T(t) {
                r += 1;
                if matches!(pat[r], P::Match | P::End) {
                    break 'cont;
                }
                continue 'cont;
            }
            // §397: a partial delimiter match failed
            if s != Some(r) {
                let Some(s0) = s else {
                    let nm = tok_name(ctx, name);
                    error(ctx, &format!("Use of {nm} doesn't match its definition"), &[
                        "If you say, e.g., `\\def\\a1{...}', then you must always",
                        "put `1' after `\\a', since control sequence names are",
                        "made up of letters only. The macro here has not been",
                        "followed by the required stuff, so I'm ignoring it.",
                    ]);
                    return None;
                };
                let mut tt = s0;
                let mut resumed = false;
                while tt != r {
                    let P::T(x) = pat[tt] else { unreachable!() };
                    cur.push(x);
                    units += 1;
                    let mut u = tt + 1;
                    let mut v = s0;
                    loop {
                        if u == r {
                            if pat[v] == P::T(t) {
                                r = v + 1;
                                resumed = true;
                            }
                            break;
                        }
                        if pat[u] != pat[v] {
                            break;
                        }
                        u += 1;
                        v += 1;
                    }
                    if resumed {
                        break;
                    }
                    tt += 1;
                }
                if resumed {
                    // `goto continue` (TeX does not test for a full match here)
                    continue 'cont;
                }
                r = s0;
            }
            if t == par && !long {
                let nm = tok_name(ctx, name);
                ctx.back(par);
                error(ctx, &format!("Paragraph ended before {nm} was complete"), &[
                    "I suspect you've forgotten a `}', causing me to apply this",
                    "control sequence to too much text. How can we recover?",
                    "My plan is to forget the whole thing and hope for the best.",
                ]);
                return None;
            }
            if is_left(&t) {
                // §399: a whole group
                let mut unbalance = 1u32;
                cur.push(t);
                loop {
                    let Some(mut g) = ctx.next() else {
                        let nm = tok_name(ctx, name);
                        error(ctx, &format!("File ended while scanning use of {nm}"), RUNAWAY_HELP);
                        return None;
                    };
                    if is_outer(ctx, &g) {
                        ctx.back(g);
                        ctx.push(Arc::from(vec![par]), TokKind::Inserted);
                        let nm = tok_name(ctx, name);
                        error(ctx, &format!("Forbidden control sequence found while scanning use of {nm}"), RUNAWAY_HELP);
                        long = false;
                        g = SPACE;
                    }
                    if g == par && !long {
                        let nm = tok_name(ctx, name);
                        ctx.back(par);
                        error(ctx, &format!("Paragraph ended before {nm} was complete"), &[
                            "I suspect you've forgotten a `}', causing me to apply this",
                            "control sequence to too much text. How can we recover?",
                            "My plan is to forget the whole thing and hope for the best.",
                        ]);
                        return None;
                    }
                    cur.push(g);
                    if is_left(&g) {
                        unbalance += 1;
                    } else if is_right(&g) {
                        unbalance -= 1;
                        if unbalance == 0 {
                            break;
                        }
                    }
                }
            } else if is_right(&t) {
                // §395: an extra `}`: insert \par (which then runs away)
                ctx.back(t);
                let nm = tok_name(ctx, name);
                error(ctx, &format!("Argument of {nm} has an extra }}"), &[
                    "I've run across a `}' that doesn't seem to match anything.",
                    "For example, `\\def\\a#1{...}' and `\\a}' would produce",
                    "this error. If you simply proceed now, the `\\par' that",
                    "I've just inserted will cause me to report a runaway",
                    "argument that might be the root of the problem. But if",
                    "your `}' was spurious, just type `2' and it will go away.",
                ]);
                long = false;
                ctx.push(Arc::from(vec![par]), TokKind::Inserted);
                continue 'cont;
            } else {
                // §393: a blank before an undelimited argument is skipped
                if t == SPACE && matches!(pat[r], P::Match | P::End) {
                    continue 'cont;
                }
                cur.push(t);
            }
            units += 1;
            if matches!(pat[r], P::T(_)) {
                continue 'cont;
            }
            break 'cont;
        }
        // found: §400
        if s.is_some() {
            let n = cur.len();
            if units == 1 && n >= 2 && is_right(&cur[n - 1]) && is_left(&cur[0]) {
                args.push(Arc::from(&cur[1..n - 1]));
            } else {
                args.push(Arc::from(cur));
            }
        }
        if pat[r] == P::End {
            return Some(args);
        }
    }
}
