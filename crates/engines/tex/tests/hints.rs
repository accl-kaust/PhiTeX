//! The hints ([`phitex_tex::Stats::nodes_bounded`]): a macro defined
//! early and used late lets readers between them wait on nothing of it;
//! plain fixtures are mostly bounded; nothing violates a hint.

mod common;

use std::sync::Mutex;

use phitex_tex::{Build, Options};

/// The hint diagnostics of a build, printed; how many there are.
fn hint_diags(b: &Build) -> usize {
    let ds: Vec<String> = b
        .diagnostics()
        .iter()
        .filter(|d| d.message.contains("hint") || d.message.contains("undeclared"))
        .map(|d| format!("{}:{} {}", d.file, d.line, d.message))
        .collect();
    for d in &ds {
        eprintln!("  {d}");
    }
    ds.len()
}

/// The tests toggle the hints: one at a time.
fn lock() -> std::sync::MutexGuard<'static, ()> {
    static L: Mutex<()> = Mutex::new(());
    L.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn with_hints_off<T>(off: bool, f: impl FnOnce() -> T) -> T {
    // (the tests of this file are serialised by `lock`)
    phitex_tex::set_hints_enabled(!off);
    let r = f();
    phitex_tex::set_hints_enabled(true);
    r
}

/// A document whose first line defines `\hello`, used on line 50, with
/// definitions between (each binds to the prelude's `\def`, not to the
/// lines before it).
fn far_use() -> String {
    let mut s = String::from("\\def\\hello{Hello}\n");
    for k in 2..50 {
        use std::fmt::Write as _;
        let _ = writeln!(s, "\\def\\filler{}{{line {k} of filler text.}}", roman(k));
    }
    s.push_str("\\hello\\ world.\n\\bye\n");
    s
}

fn roman(mut n: usize) -> String {
    let mut o = String::new();
    for (v, r) in [(10, "x"), (9, "ix"), (5, "v"), (4, "iv"), (1, "i")] {
        while n >= v {
            o.push_str(r);
            n -= v;
        }
    }
    o
}

fn build(doc: &str, off: bool) -> Build {
    let fx = common::fixtures().join("plain");
    let prelude = common::plain();
    let opts = || Options {
        time_limit: None,
        ..common::options("./far.tex", "far", "", &fx)
    };
    with_hints_off(off, || common::build(opts(), Some(&prelude), doc))
}

#[test]
fn far_definition_is_bounded() {
    let _g = lock();
    let doc = far_use();
    let a = build(&doc, false);
    eprintln!(
        "hints: {} bounded, {} any, {} violations, {} undeclared, stops {}, pending {}",
        a.stats.nodes_bounded,
        a.stats.nodes_any,
        a.stats.regime_violations,
        a.stats.undeclared_definitions,
        a.stats.stops,
        a.stats.pending_waits
    );
    hint_diags(&a);
    assert!(a.stats.nodes_bounded > 0);
    assert_eq!(a.stats.regime_violations, 0);
    assert_eq!(a.stats.undeclared_definitions, 0);
    assert_eq!(a.pages().len(), 1);
    // (the readers of `\hello` bind to its definition and wait on no
    // node between: each node of the filler lines declares what it may
    // define, `\hello` not among it, and that it takes no sibling)
    let bounds = a.node_bounds();
    for (t, md, claim) in &bounds {
        eprintln!("bound {t:?}: {md:?}, claim {claim:?}");
    }
    let fillers: Vec<_> = bounds.iter().filter(|(t, _, _)| t.contains("filler")).collect();
    assert!(!fillers.is_empty(), "no node of the filler lines");
    for (t, md, claim) in fillers {
        let md = md
            .as_ref()
            .unwrap_or_else(|| panic!("{t:?} may define anything"));
        assert!(
            !md.iter()
                .any(|f| "cs:hello".starts_with(f.strip_suffix('*').unwrap_or(f))),
            "{t:?} may define \\hello: {md:?}"
        );
        assert_eq!(*claim, Some(0), "{t:?} may take the siblings after it");
    }
    let b = build(&doc, true);
    eprintln!(
        "any: {} bounded, {} any, stops {}, pending {}",
        b.stats.nodes_bounded, b.stats.nodes_any, b.stats.stops, b.stats.pending_waits
    );
    assert_eq!(a.output().0, b.output().0, "the hints changed the output");
    // (the hints off: no node bounded)
    assert_eq!(b.stats.nodes_bounded, 0, "a node bounded with the hints off");
}

#[test]
fn story_is_mostly_bounded() {
    let _g = lock();
    let fx = common::fixtures().join("plain");
    let src = common::read(&fx.join("story.tex"));
    let prelude = common::plain();
    let opts = common::options("./story.tex", "story", "", &fx);
    let b = common::build(opts, Some(&prelude), &src);
    let s = &b.stats;
    eprintln!(
        "story: {} bounded, {} any, {} violations, {} undeclared",
        s.nodes_bounded, s.nodes_any, s.regime_violations, s.undeclared_definitions
    );
    hint_diags(&b);
    assert_eq!(s.regime_violations, 0);
    assert_eq!(s.undeclared_definitions, 0);
    assert!(s.nodes_bounded > 0);
    // (plain's own macros are not in the document's scope yet: the
    // calls to them are unbounded)
    assert!(
        s.nodes_bounded * 3 >= s.nodes_any,
        "any {} of {}",
        s.nodes_any,
        s.nodes_bounded + s.nodes_any
    );
}

#[test]
fn input_file_is_bounded() {
    let _g = lock();
    let dir = std::env::temp_dir().join(format!("phitex-hints-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("tmp dir");
    std::fs::write(dir.join("body.tex"), far_use().replace("\\bye\n", "")).expect("body.tex");
    let fx = common::fixtures().join("plain");
    let prelude = common::plain();
    let opts = Options {
        time_limit: None,
        ..common::options("./main.tex", "main", "", &dir)
    };
    let mut o = opts;
    // fonts come from the fixtures' finder, the body from the temp dir
    let inner = common::finder(&fx);
    let d = dir.clone();
    o.finder = std::sync::Arc::new(move |name: &str| {
        let p = d.join(name);
        if p.is_file() {
            return Some((format!("./{name}"), std::fs::read(&p).ok()?));
        }
        inner(name)
    });
    let b = common::build(o, Some(&prelude), "\\input body.tex\n\\bye\n");
    let s = &b.stats;
    eprintln!(
        "input: {} bounded, {} any, {} violations, {} undeclared, {} rounds, {} stops, {} env_provided, {} runs",
        s.nodes_bounded,
        s.nodes_any,
        s.regime_violations,
        s.undeclared_definitions,
        s.rounds,
        s.stops,
        s.env_provided,
        s.runs
    );
    let _ = std::fs::remove_dir_all(&dir);
    hint_diags(&b);
    assert_eq!(s.regime_violations, 0);
    assert_eq!(s.undeclared_definitions, 0);
    assert!(s.nodes_bounded > 50, "bounded {}", s.nodes_bounded);
    assert_eq!(b.pages().len(), 1);
}

/// A cold build, then edits whose nodes define fields through the
/// build's interned handles (catcodes, parameters) in later
/// evaluations: the handles outlive the value sweeps between them, and
/// each edit's output equals a cold build of its text.
#[test]
fn handles_survive_evaluations() {
    let _g = lock();
    let doc = |k: usize| {
        format!(
            "\\catcode`\\|={k} \\parindent={k}pt \\count1={k}\n{}\\par\n\\bye\n",
            far_use().replace("\\bye\n", "")
        )
    };
    let mut a = build(&doc(12), false);
    for k in [11, 12, 13] {
        let text = doc(k).replace("\\catcode`\\|=13", "\\catcode`\\|=12");
        a.edit(&text);
        let cold = build(&text, false);
        assert!(
            common::first_difference(&a.output().0, &cold.output().0).is_none(),
            "edit {k}"
        );
        assert_eq!(a.stats.undeclared_definitions, 0);
    }
}

/// A file read by `\input` where its reader made `@` a letter: the
/// pass reads the file in a document's regime (`@` no letter), as it
/// cannot know its reader's, so `\@x` is to it `\@` (a macro of one
/// argument) and `x`. The same text read inline, where the pass sees the
/// change, is the reference.
const PART: &str = "\\def\\@#1{A}\\def\\@x{B}\n\\@x\n";

fn read_as_file(file: bool) -> Build {
    let d = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("hints-regime");
    let _ = std::fs::create_dir_all(&d);
    std::fs::write(d.join("part.tex"), PART).unwrap();
    let doc = if file {
        "\\catcode`\\@=11 \n\\input part\n\\catcode`\\@=12 \n\\bye\n".to_owned()
    } else {
        format!("\\catcode`\\@=11 \n{PART}\\catcode`\\@=12 \n\\bye\n")
    };
    let opts = Options {
        time_limit: None,
        ..common::options("./main.tex", "main", "", &d)
    };
    common::build(opts, Some(&common::plain()), &doc)
}

/// Where a control sequence's name ends is the pass's reading under the
/// regime it predicts: a cut between `\@` and `x` would split the
/// control word `\@x` in two, which no run joins again. No slice begins
/// right after a name: the file typesets as the inline text does.
#[test]
fn a_mispredicted_regime_cuts_no_token() {
    let _g = lock();
    let file = read_as_file(true);
    let inline = read_as_file(false);
    hint_diags(&file);
    assert_eq!(
        file.output().0,
        inline.output().0,
        "the file typeset otherwise"
    );
}

/// A control symbol's character is read as no letter (or the name would
/// go on, §356): the hint says so, and the run checks it. `\@` read as a
/// control symbol where `@` is a letter is a violation.
#[test]
fn a_control_symbol_assumes_no_letter() {
    let _g = lock();
    let b = read_as_file(true);
    hint_diags(&b);
    assert!(
        b.stats.regime_violations > 0,
        "the run did not check that `@` is no letter"
    );
}
