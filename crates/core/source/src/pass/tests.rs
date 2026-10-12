use super::*;

fn pass(text: &str, kind: FileKind) -> (Pass, Definitions) {
    let mut d = Definitions::new();
    d.harvest(text, kind, 0);
    let p = analyze(
        text,
        kind,
        Scope {
            document: &d,
            file: 0,
            prelude: &Definitions::new(),
        },
    );
    (p, d)
}

fn item<'p>(p: &'p Pass, name: &str) -> &'p Item {
    p.items
        .iter()
        .find(|i| i.name == name)
        .unwrap_or_else(|| panic!("no item {name}"))
}

fn text_of<'t>(t: &'t str, tok: &Token) -> &'t str {
    &t[tok.start..tok.end]
}

#[test]
fn tokens_follow_makeatletter() {
    let t = "\\a@b \\makeatletter \\a@b{\\c@d} \\makeatother \\a@b";
    let (p, _) = pass(t, FileKind::Document);
    let cs: Vec<&str> = p
        .tokens
        .iter()
        .filter(|k| k.kind == TokKind::Cs)
        .map(|k| text_of(t, k))
        .collect();
    assert_eq!(
        cs,
        [
            "\\a",
            "\\makeatletter",
            "\\a@b",
            "\\c@d",
            "\\makeatother",
            "\\a"
        ]
    );
    assert_eq!(p.hazards.len(), 1);
    assert_eq!(p.hazards[0].status, Status::Predicted);
}

#[test]
fn packages_have_at_letters_and_expl_has_underscores() {
    let t = "\\a@b \\ExplSyntaxOn \\tl_set:Nn \\l_x_tl {y} \\ExplSyntaxOff \\a_b";
    let (p, _) = pass(t, FileKind::Package);
    let cs: Vec<&str> = p
        .tokens
        .iter()
        .filter(|k| k.kind == TokKind::Cs)
        .map(|k| text_of(t, k))
        .collect();
    assert_eq!(
        cs,
        [
            "\\a@b",
            "\\ExplSyntaxOn",
            "\\tl_set:Nn",
            "\\l_x_tl",
            "\\ExplSyntaxOff",
            "\\a"
        ]
    );
    let it = item(&p, "tl_set:Nn");
    assert_eq!(it.status, Status::Predicted);
}

#[test]
fn braces_match_and_unmatched_are_reported() {
    let t = "{a{b}c}}{";
    let (p, _) = pass(t, FileKind::Document);
    assert_eq!(p.braces.len(), 3);
    assert_eq!(p.braces[0].close, Some(6));
    assert_eq!(p.braces[1].close, Some(4));
    assert_eq!(
        p.braces[2],
        BracePair {
            open: 8,
            close: None,
            status: Status::Pending
        }
    );
    assert_eq!(p.unmatched_closes, [7]);
}

#[test]
fn group_and_environment_pairs() {
    let t = "\\begin{itemize}\\begingroup x\\endgroup\\end{itemize}{\\begingroup}\\end{foo}";
    let (p, _) = pass(t, FileKind::Document);
    let st: Vec<(GroupKind, Status)> = p
        .groups
        .iter()
        .map(|g| (g.kind.clone(), g.status))
        .collect();
    assert_eq!(
        st,
        [
            (GroupKind::Env("itemize".into()), Status::Known),
            (GroupKind::BeginGroup, Status::Known),
            (GroupKind::BeginGroup, Status::Pending),
            (GroupKind::Env("foo".into()), Status::Pending),
        ]
    );
}

#[test]
fn verbatim_is_a_predicted_hazard() {
    let t = "\\verb|{\\x| \\begin{verbatim}\n}{\\y\n\\end{verbatim}";
    let (p, _) = pass(t, FileKind::Document);
    assert!(p.braces.iter().all(|b| b.close.is_some()));
    assert_eq!(
        p.hazards.iter().filter(|h| h.cause == "verbatim").count(),
        2
    );
    assert_eq!(item(&p, "verb").status, Status::Predicted);
}

#[test]
fn computed_catcode_is_pending_to_the_group_end() {
    let t = "{\\catcode\\count@=13 \\x} \\y \\catcode`\\{=13 \\z";
    let (p, _) = pass(t, FileKind::Package);
    let h: Vec<(&str, Status)> = p.hazards.iter().map(|h| (h.cause, h.status)).collect();
    assert_eq!(
        h,
        [
            ("\\catcode with computed operands", Status::Pending),
            ("\\catcode to a shaping class", Status::Pending),
        ]
    );
    assert_eq!(p.hazards[0].end, t.find('}').unwrap());
    assert_eq!(p.hazards[1].end, t.len());
    assert_eq!(
        item(&p, "x").cause.as_ref().unwrap().kind,
        "tokenization pending (catcode)"
    );
}

#[test]
fn literal_catcode_is_predicted_and_retokenizes() {
    let t = "\\catcode`\\_=11 \\a_b";
    let (p, _) = pass(t, FileKind::Document);
    assert_eq!(p.hazards[0].status, Status::Predicted);
    assert!(p.items.iter().any(|i| i.name == "a_b"));
}

#[test]
fn catcode_change_in_a_body_waits_for_the_call() {
    let t = "\\def\\foo{\\catcode`\\{=12 }\\a@b \\foo \\a@b";
    let (p, d) = pass(t, FileKind::Document);
    assert_eq!(d.changes_catcodes("foo", None).0, 2);
    let t2 = "\\def\\bar{\\catcode`\\|=13 }\\bar x|y|z";
    let (p2, d2) = pass(t2, FileKind::Document);
    assert_eq!(d2.changes_catcodes("bar", None), (3, 1 << b'|'));
    let h = p2
        .hazards
        .iter()
        .find(|h| h.cause == "character with changed catcode")
        .unwrap();
    assert_eq!(&t2[h.start..h.end], "|y|");
    let names: Vec<&str> = p.items.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(names, ["def", "catcode", "a", "foo", "a"]);
    assert!(item(&p, "foo").hazard);
}

#[test]
fn primitives_are_known() {
    let t = "\\def\\x#1#2{#1}\\hbox to 3pt{a}\\hskip 1pt plus 1fil\\relax \\count0=5 \\advance\\parindent by -2\\parindent";
    let (p, _) = pass(t, FileKind::Document);
    for n in ["def", "hbox", "hskip", "count", "advance"] {
        assert_eq!(item(&p, n).status, Status::Known, "{n}");
    }
    // (\x is \def's operand, not an item)
    assert!(p.items.iter().all(|i| i.name != "x"));
}

#[test]
fn kernel_commands_are_known() {
    let t = "\\section*[short]{Long} \\begin{tabular}{ll} a \\end{tabular}";
    let (p, _) = pass(t, FileKind::Document);
    let s = item(&p, "section");
    assert_eq!(s.status, Status::Known);
    assert_eq!(p.atoms[s.end - 1].kind, AtomKind::Close);
    assert_eq!(item(&p, "begin").status, Status::Known);
}

#[test]
fn visible_definitions_predict() {
    let t = "\\def\\pair(#1,#2){}\\pair(a,{b,c}) \\newcommand\\two[2][x]{} \\two[y]{z} \\other";
    let (p, _) = pass(t, FileKind::Document);
    let pair = item(&p, "pair");
    assert_eq!(pair.status, Status::Predicted);
    assert_eq!(
        &t[p.atoms[pair.end - 1].start..p.atoms[pair.end - 1].end],
        ")"
    );
    let two = item(&p, "two");
    assert_eq!(two.status, Status::Predicted);
    assert_eq!(
        &t[p.atoms[two.end - 1].start..p.atoms[two.end - 1].end],
        "}"
    );
    let o = item(&p, "other");
    assert_eq!(o.status, Status::Pending);
    assert_eq!(o.cause.as_ref().unwrap().kind, "macro defined elsewhere");
}

#[test]
fn values_and_lookaheads_are_pending() {
    let t = "\\expandafter\\a\\b \\csname x\\endcsname \\@ifnextchar[\\y\\z \\hskip 3pt\\mymacro";
    let (p, _) = pass(t, FileKind::Package);
    assert_eq!(item(&p, "expandafter").status, Status::Pending);
    assert_eq!(item(&p, "csname").status, Status::Pending);
    assert_eq!(item(&p, "@ifnextchar").status, Status::Pending);
    let h = item(&p, "hskip");
    assert_eq!(h.status, Status::Pending);
    assert_eq!(
        h.cause.as_ref().unwrap().kind,
        "number lookahead into macro"
    );
}

#[test]
fn delimited_parameters_and_their_extent() {
    let t = "\\def\\x#1.#2\\end{}\\x a{.}b. c d\\end e";
    let (p, _) = pass(t, FileKind::Document);
    let x = p.items.iter().find(|i| i.name == "x").unwrap();
    assert_eq!(x.status, Status::Predicted);
    assert_eq!(&t[p.atoms[x.end].start..], "e");
}

#[test]
fn chain_counts_pending_inside_reach() {
    let t = "\\u{\\v} \\w\n\n\\y";
    let (p, _) = pass(t, FileKind::Document);
    let s = Stats::of(&p, t);
    assert_eq!(s.pending_items, 4);
    // (\u, \w, \y in sequence; \v inside a group \u reaches)
    assert_eq!(s.chain, 3);
    assert_eq!(s.chain_par, 2);
}

/// The corpus report: `PHITEX_CORPUS` lists documents, one per line, as
/// `label<TAB>path<TAB>path…` (a directory: its `.tex`, `.sty`, `.cls`,
/// `.ltx`, `.dtx` files, recursively); `PHITEX_PRELUDE` is the kernel
/// (`latex.ltx`) whose definitions every document may bind to.
#[allow(
    clippy::too_many_lines,
    clippy::cast_precision_loss,
    clippy::format_collect
)]
#[test]
#[ignore = "reads the corpus named by PHITEX_CORPUS"]
fn corpus_report() {
    const CLASSES: [&str; 4] = [
        "kernel",
        "packages/classes",
        "document preambles",
        "document bodies",
    ];
    use std::fmt::Write as _;
    fn files(p: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        if p.is_dir() {
            let mut es: Vec<_> = std::fs::read_dir(p)
                .unwrap()
                .flatten()
                .map(|e| e.path())
                .collect();
            es.sort();
            for e in es {
                files(&e, out);
            }
        } else if p
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| ["tex", "sty", "cls", "ltx", "dtx"].contains(&e))
        {
            out.push(p.to_path_buf());
        }
    }
    let read =
        |p: &std::path::Path| String::from_utf8_lossy(&std::fs::read(p).unwrap()).into_owned();
    let mut prelude = Definitions::new();
    if let Ok(ps) = std::env::var("PHITEX_PRELUDE") {
        for (k, p) in ps.split(':').enumerate() {
            let t = read(std::path::Path::new(p));
            prelude.harvest(&t, FileKind::from_path(p), u32::try_from(k).unwrap());
        }
    }
    let sample = std::env::var("PHITEX_SAMPLE").ok();
    let corpus =
        std::fs::read_to_string(std::env::var("PHITEX_CORPUS").expect("PHITEX_CORPUS")).unwrap();
    let pc = |a: usize, all: &[usize; 3]| {
        let n: usize = all.iter().sum();
        if n == 0 {
            0.0
        } else {
            100.0 * a as f64 / n as f64
        }
    };
    let mut out = String::new();
    let _ = writeln!(
        out,
        "| document | files | tokens | cs | brace pairs | extents K/P/Pd % | groups K/P/Pd % | pending | chain | chain % cs | chain (par) | catcode-pending bytes % |"
    );
    let _ = writeln!(out, "|---|---|---|---|---|---|---|---|---|---|---|---|");
    let mut total = Stats::default();
    let row = |out: &mut String, label: &str, nf: usize, s: &Stats| {
        let _ = writeln!(
            out,
            "| {label} | {nf} | {} | {} | {} | {:.1}/{:.1}/{:.1} | {:.1}/{:.1}/{:.1} | {} | {} | {:.2} | {} | {:.1} |",
            s.tokens,
            s.cs,
            s.brace_pairs,
            pc(s.args[0], &s.args),
            pc(s.args[1], &s.args),
            pc(s.args[2], &s.args),
            pc(s.groups[0], &s.groups),
            pc(s.groups[1], &s.groups),
            pc(s.groups[2], &s.groups),
            s.pending_items,
            s.chain,
            100.0 * s.chain as f64 / s.cs.max(1) as f64,
            s.chain_par,
            100.0 * s.hazard_bytes[0] as f64 / s.bytes.max(1) as f64,
        );
    };
    let mut nfiles = 0;
    // (by class: the kernel, packages and classes, document preambles,
    // document bodies)
    let mut by_class: [Stats; 4] = Default::default();
    let mut class_files = [0usize; 4];
    // (pending items: all improvements, and without B1, B2, B3)
    let mut ab = [[0usize; 4]; 4];
    for line in corpus.lines().filter(|l| !l.trim().is_empty()) {
        let mut parts = line.split('\t');
        let label = parts.next().unwrap();
        let mut fs = Vec::new();
        // (a path after `+` only lends its definitions: code the
        // document loads, reported on its own line)
        let mut lent = Vec::new();
        for p in parts {
            match p.strip_prefix('+') {
                Some(p) => files(std::path::Path::new(p), &mut lent),
                None => files(std::path::Path::new(p), &mut fs),
            }
        }
        let texts: Vec<(String, String)> = fs
            .iter()
            .map(|f| (f.display().to_string(), read(f)))
            .collect();
        let mut defs = Definitions::new();
        for (k, (n, t)) in texts.iter().enumerate() {
            defs.harvest(t, FileKind::from_path(n), u32::try_from(k).unwrap());
        }
        for (k, f) in lent.iter().enumerate() {
            let n = f.display().to_string();
            defs.harvest(
                &read(f),
                FileKind::from_path(&n),
                u32::try_from(texts.len() + k).unwrap(),
            );
        }
        // (load order: the code the document loads, then its own files
        // in the order listed; B3 off: no order)
        let unordered = defs.clone();
        let order: Vec<u32> = (texts.len()..texts.len() + lent.len())
            .chain(0..texts.len())
            .map(|k| u32::try_from(k).unwrap())
            .collect();
        defs.set_load_order(&order);
        let mut s = Stats::default();
        for (k, (n, t)) in texts.iter().enumerate() {
            let kind = FileKind::from_path(n);
            let scope = Scope {
                document: &defs,
                file: u32::try_from(k).unwrap(),
                prelude: &prelude,
            };
            let p = analyze(t, kind, scope);
            s.add(&Stats::of(&p, t));
            // (the file's classes and their byte ranges)
            let classes: Vec<(usize, std::ops::Range<usize>)> =
                if n.ends_with("latex.ltx") || n.ends_with("plain.tex") {
                    vec![(0, 0..t.len())]
                } else if kind == FileKind::Package {
                    vec![(1, 0..t.len())]
                } else if let Some(d) = t.find("\\begin{document}") {
                    vec![(2, 0..d), (3, d..t.len())]
                } else {
                    vec![(3, 0..t.len())]
                };
            let variants = [
                analyze_with(
                    t,
                    kind,
                    scope,
                    Features {
                        w_signatures: false,
                        ..Features::default()
                    },
                ),
                analyze_with(
                    t,
                    kind,
                    scope,
                    Features {
                        lookthrough: false,
                        ..Features::default()
                    },
                ),
                analyze(
                    t,
                    kind,
                    Scope {
                        document: &unordered,
                        ..scope
                    },
                ),
            ];
            for (c, r) in classes {
                by_class[c].add(&Stats::of_range(&p, t, r.clone()));
                class_files[c] += 1;
                ab[0][c] += Stats::of_range(&p, t, r.clone()).pending_items;
                for (v, q) in variants.iter().enumerate() {
                    ab[v + 1][c] += Stats::of_range(q, t, r.clone()).pending_items;
                }
            }
            if std::env::var("PHITEX_HAZARDS").is_ok() {
                for h in p
                    .hazards
                    .iter()
                    .filter(|h| h.status == Status::Pending && h.end - h.start > 2000)
                    .take(2)
                {
                    let e = (h.start + 60).min(t.len());
                    let e = (e..t.len())
                        .find(|&e| t.is_char_boundary(e))
                        .unwrap_or(t.len());
                    println!(
                        "HAZARD {n} {} {} len {}: {:?}",
                        h.cause,
                        h.detail,
                        h.end - h.start,
                        &t[h.start..e]
                    );
                }
            }
            if let Some(c) = &sample {
                for it in p
                    .items
                    .iter()
                    .filter(|i| i.cause.as_ref().is_some_and(|x| x.kind == c))
                    .take(3)
                {
                    let at = p.atoms[it.atom].start;
                    let e = (at + 70).min(t.len());
                    let e = (e..t.len())
                        .find(|&e| t.is_char_boundary(e))
                        .unwrap_or(t.len());
                    println!("SAMPLE {n}: {:?}", &t[at..e]);
                }
            }
        }
        row(&mut out, label, texts.len(), &s);
        nfiles += texts.len();
        total.add(&s);
    }
    row(&mut out, "**total**", nfiles, &total);
    let _ = writeln!(out, "\nBy file class:\n");
    let _ = writeln!(
        out,
        "| class | files | tokens | cs | brace pairs | extents K/P/Pd % | groups K/P/Pd % | pending | chain | chain % cs | chain (par) | catcode-pending bytes % |"
    );
    let _ = writeln!(out, "|---|---|---|---|---|---|---|---|---|---|---|---|");
    for (k, c) in CLASSES.iter().enumerate() {
        row(&mut out, c, class_files[k], &by_class[k]);
    }
    let _ = writeln!(
        out,
        "\nHints by file class (% of items; scope and paragraph: % of items and marks):\n\n| class | items | marks | claim K/P/Pd % | may_define Nothing/Names/Prefixes/Any % | scope None/Open/Close/Predicted/Pending % | paragraph None/Starts/Ends % |\n|---|---|---|---|---|---|---|"
    );
    let pcs = |v: &[usize]| {
        let n: usize = v.iter().sum();
        v.iter()
            .map(|&x| format!("{:.1}", 100.0 * x as f64 / n.max(1) as f64))
            .collect::<Vec<_>>()
            .join("/")
    };
    for (k, c) in CLASSES.iter().enumerate() {
        let s = &by_class[k];
        let _ = writeln!(
            out,
            "| {c} | {} | {} | {} | {} | {} | {} |",
            s.claims.iter().sum::<usize>(),
            s.marks,
            pcs(&s.claims),
            pcs(&s.may_define),
            pcs(&s.scope),
            pcs(&s.implicit),
        );
    }
    let _ = writeln!(
        out,
        "\nPending items before/after each improvement (before: that one off, the others on):\n\n| class | all on | B1 off (expl3 :w) | B2 off (\\expandafter look-through) | B3 off (load order) |\n|---|---|---|---|---|"
    );
    for (k, c) in CLASSES.iter().enumerate() {
        let _ = writeln!(
            out,
            "| {c} | {} | {} | {} | {} |",
            ab[0][k], ab[1][k], ab[2][k], ab[3][k]
        );
    }
    let set = |s: &DefSet| {
        let g = |m: &std::collections::BTreeMap<String, bool>| {
            m.iter()
                .map(|(f, g)| if *g { format!("{f} (G)") } else { f.clone() })
                .collect::<Vec<_>>()
                .join(", ")
        };
        format!(
            "names ({}): {}\n\nprefixes ({}): {}\n\nunbounded by ({}): {}\n",
            s.names.len(),
            g(&s.names),
            s.prefixes.len(),
            g(&s.prefixes),
            s.unbounded.len(),
            s.unbounded.join("; ")
        )
    };
    if let Some(o) = prelude.output_defines(&Definitions::new()) {
        let _ = writeln!(
            out,
            "\n`\\output` of the prelude (latex.ltx):\n\n{}",
            set(&o)
        );
        let op = prelude.call_defines(&Definitions::new(), "@outputpage");
        let _ = writeln!(out, "`\\@outputpage` alone:\n\n{}", set(&op));
    }
    if let Ok(lo) = std::env::var("PHITEX_LTOUTPUT") {
        // (ltoutput.dtx's `\\output` alone, its callees in the prelude)
        let mut d = Definitions::new();
        d.harvest(&read(std::path::Path::new(&lo)), FileKind::Package, 0);
        if let Some(o) = d.output_defines(&prelude) {
            let _ = writeln!(
                out,
                "`\\output` of ltoutput.dtx (callees in the prelude):\n\n{}",
                set(&o)
            );
        }
        let op = d.call_defines(&prelude, "@outputpage");
        let _ = writeln!(out, "`\\@outputpage` of ltoutput.dtx:\n\n{}", set(&op));
    }
    if let Ok(pl) = std::env::var("PHITEX_PLAIN") {
        let mut d = Definitions::new();
        d.harvest(&read(std::path::Path::new(&pl)), FileKind::Package, 0);
        if let Some(o) = d.output_defines(&Definitions::new()) {
            let _ = writeln!(out, "`\\output` of plain.tex:\n\n{}", set(&o));
        }
    }
    let top = |m: &std::collections::BTreeMap<String, usize>, n: usize| {
        let mut v: Vec<_> = m.iter().collect();
        v.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        v.into_iter()
            .take(n)
            .map(|(k, c)| format!("| {k} | {c} |\n"))
            .collect::<String>()
    };
    let _ = writeln!(
        out,
        "\nTop causes of Pending (of {}):\n\n| cause | count |\n|---|---|\n{}",
        total.pending_items,
        top(&total.causes, 20)
    );
    let _ = writeln!(
        out,
        "Top causes of Pending in document bodies (of {}):\n\n| cause | count |\n|---|---|\n{}",
        by_class[3].pending_items,
        top(&by_class[3].causes, 5)
    );
    let _ = writeln!(
        out,
        "Top pending names:\n\n| cause: name | count |\n|---|---|\n{}",
        top(&total.pending_names, 20)
    );
    println!("{out}");
}

#[test]
fn a_brace_made_other_is_read_as_text_until_restored() {
    let t = "\\catcode`\\{=12 \\def\\x{ \\catcode`\\{=1 {a}";
    let (p, _) = pass(t, FileKind::Document);
    assert_eq!(p.braces.len(), 1);
    assert_eq!(
        p.braces[0].close.map(|c| &t[p.tokens[c].start..]),
        Some("}")
    );
    assert_eq!(p.hazards[0].status, Status::Predicted);
    assert_eq!(p.hazards[0].end, t.rfind("\\catcode").unwrap());
}

#[test]
fn begingroup_ends_a_catcode_change() {
    let t = "\\begingroup\\catcode`\\^^M=\\active \\x\\endgroup \\y";
    let (p, _) = pass(t, FileKind::Document);
    assert_eq!(p.hazards[0].status, Status::Pending);
    assert_eq!(p.hazards[0].end, t.find("\\endgroup").unwrap());
    assert_eq!(
        item(&p, "y").cause.as_ref().unwrap().kind,
        "macro defined elsewhere"
    );
}

#[test]
fn nested_definitions_double_their_parameters() {
    let t = "\\def\\outer{\\def\\meta##1{##1}}\\meta{x}";
    let (p, _) = pass(t, FileKind::Document);
    assert_eq!(item(&p, "meta").status, Status::Predicted);
}

// ---------------------------------------------------------------------
// Hints.

fn hints<'p>(p: &'p Pass, name: &str) -> &'p Hints {
    &item(p, name).hints
}

fn mark_at<'p>(p: &'p Pass, t: &str, at: &str) -> &'p Mark {
    let b = t.find(at).unwrap_or_else(|| panic!("no {at}"));
    p.marks
        .iter()
        .find(|m| p.atoms[m.atom].start == b)
        .unwrap_or_else(|| panic!("no mark at {at}"))
}

fn names(m: &MayDefine) -> Vec<(String, bool)> {
    match m {
        MayDefine::Names(v) | MayDefine::Prefixes(v) => {
            v.iter().map(|d| (d.field.clone(), d.global)).collect()
        }
        _ => Vec::new(),
    }
}

#[test]
fn claims_known_predicted_pending() {
    let t = "\\def\\foo#1{x}\\foo{ab} \\zork x \\relax";
    let (p, _) = pass(t, FileKind::Document);
    let foo = p.items.iter().rfind(|i| i.name == "foo").unwrap();
    assert_eq!(
        foo.hints.claim,
        Claim::Predicted {
            n_tokens: 4,
            confirm: Assumption::MeaningIs {
                cs: "foo".into(),
                expected: MeaningShape::Macro { params: "m".into() }
            },
            also: vec![]
        }
    );
    assert!(
        matches!(hints(&p, "zork").claim, Claim::Pending { .. }),
        "{:?}",
        hints(&p, "zork")
    );
    assert_eq!(hints(&p, "relax").claim, Claim::Known(0));
}

#[test]
fn prediction_rests_on_the_weakest_assumption() {
    // (`\x` is redefined by the document: the kernel's meaning of
    // `\textbf` is held more firmly than the document's of `\x`)
    let t = "\\def\\x{1pt}\\hskip\\x";
    let (p, _) = pass(t, FileKind::Document);
    match &hints(&p, "hskip").claim {
        Claim::Predicted { confirm, .. } => assert_eq!(
            confirm,
            &Assumption::MeaningIs {
                cs: "x".into(),
                expected: MeaningShape::Macro {
                    params: String::new()
                }
            }
        ),
        c => panic!("{c:?}"),
    }
}

#[test]
fn definitions_name_what_they_define() {
    let t = "\\def\\foo{}\\global\\def\\bar{}\\count12=3 \\tolerance=200 \\catcode`\\|=12 ";
    let (p, _) = pass(t, FileKind::Document);
    assert_eq!(
        hints(&p, "def").may_define,
        MayDefine::Names(vec![Def {
            field: "cs:\\foo".into(),
            global: false
        }])
    );
    let d2 = p.items.iter().filter(|i| i.name == "def").nth(1).unwrap();
    assert_eq!(names(&d2.hints.may_define), [("cs:\\bar".into(), true)]);
    assert_eq!(hints(&p, "global").may_define, MayDefine::Nothing);
    assert_eq!(
        names(&hints(&p, "count").may_define),
        [("count:12".into(), false)]
    );
    assert_eq!(
        names(&hints(&p, "tolerance").may_define),
        [("param:\\tolerance".into(), false)]
    );
    assert_eq!(
        names(&hints(&p, "catcode").may_define),
        [("catcode:124".into(), false)]
    );
}

#[test]
fn csname_defines_by_its_literal_prefix() {
    let t = "\\csname e\\the\\x\\endcsname \\csname abc\\endcsname \\csname\\y\\endcsname";
    let (p, _) = pass(t, FileKind::Document);
    let cs: Vec<&Item> = p.items.iter().filter(|i| i.name == "csname").collect();
    assert_eq!(
        cs[0].hints.may_define,
        MayDefine::Prefixes(vec![Def {
            field: "cs:\\e".into(),
            global: false
        }])
    );
    assert_eq!(names(&cs[1].hints.may_define), [("cs:\\abc".into(), false)]);
    assert_eq!(cs[2].hints.may_define, MayDefine::Any);
}

#[test]
fn calls_define_what_their_bodies_define() {
    let t = "\\def\\a{\\relax}\\def\\b{\\def\\@tempa{}}\\def\\c{\\csname\\x\\endcsname}\
             \\def\\d{\\b\\a}\\a\\b\\c\\d\\zzz";
    let (p, _) = pass(t, FileKind::Package);
    let last = |n: &str| p.items.iter().rfind(|i| i.name == n).unwrap();
    assert_eq!(last("a").hints.may_define, MayDefine::Nothing);
    assert_eq!(
        names(&last("b").hints.may_define),
        [("cs:\\@tempa".into(), false)]
    );
    assert_eq!(last("c").hints.may_define, MayDefine::Any);
    assert_eq!(
        names(&last("d").hints.may_define),
        [("cs:\\@tempa".into(), false)]
    );
    assert_eq!(last("zzz").hints.may_define, MayDefine::Any);
}

#[test]
fn scopes_of_groups_and_environments() {
    let t = "{\\begingroup x\\endgroup}\\bgroup\\egroup\\begin{itemize}\\end{itemize}";
    let (p, _) = pass(t, FileKind::Document);
    assert_eq!(
        mark_at(&p, t, "{\\begingroup").hints.scope,
        ScopeHint::Open(ScopeKind::Simple)
    );
    assert_eq!(mark_at(&p, t, "}\\bgroup").hints.scope, ScopeHint::Close);
    assert_eq!(
        hints(&p, "begingroup").scope,
        ScopeHint::Open(ScopeKind::SemiSimple)
    );
    assert_eq!(hints(&p, "endgroup").scope, ScopeHint::Close);
    assert!(matches!(
        hints(&p, "bgroup").scope,
        ScopeHint::Predicted {
            effect: Effect::Open(ScopeKind::Simple),
            ..
        }
    ));
    assert_eq!(
        hints(&p, "begin").scope,
        ScopeHint::Predicted {
            effect: Effect::Open(ScopeKind::Env("itemize".into())),
            confirm: Assumption::MeaningIs {
                cs: "begin".into(),
                expected: MeaningShape::Kernel
            }
        }
    );
    assert!(matches!(
        hints(&p, "end").scope,
        ScopeHint::Predicted {
            effect: Effect::Close,
            ..
        }
    ));
}

#[test]
fn math_shifts_and_boxes_open_scopes() {
    let t = "a $x$ $$y$$ \\hbox to 3pt{z}\\vbox{w}";
    let (p, _) = pass(t, FileKind::Document);
    let m = mark_at(&p, t, "$x");
    assert_eq!(m.hints.scope, ScopeHint::Open(ScopeKind::Math));
    assert_eq!(mark_at(&p, t, "$ $$").hints.scope, ScopeHint::Close);
    let d = mark_at(&p, t, "$$y");
    assert_eq!(d.hints.scope, ScopeHint::Open(ScopeKind::Display));
    assert_eq!(d.hints.claim, Claim::Known(1));
    assert_eq!(mark_at(&p, t, "$$ \\hbox").hints.scope, ScopeHint::Close);
    let h = hints(&p, "hbox");
    assert_eq!(h.scope, ScopeHint::Open(ScopeKind::HBox));
    // (`to`, `3pt`, `{`: the box claims up to its brace)
    assert_eq!(h.claim, Claim::Known("to 3pt{".len()));
    assert_eq!(hints(&p, "vbox").scope, ScopeHint::Open(ScopeKind::VBox));
    // (the box's own brace is its item's, not a mark)
    assert!(
        p.marks
            .iter()
            .all(|m| &t[p.atoms[m.atom].start..p.atoms[m.atom].end] != "{"
                || p.atoms[m.atom].start != t.find("{z").unwrap())
    );
}

#[test]
fn paragraphs_start_and_end_as_tex_web_says() {
    let t = "\\noindent\\vskip 1pt\\penalty5 \\kern1pt \\hbox{}\\unvbox0 \\vrule\\hrule\\par\n\nab";
    let (p, _) = pass(t, FileKind::Document);
    assert_eq!(hints(&p, "noindent").implicit, Implicit::StartsParagraph);
    assert_eq!(hints(&p, "vskip").implicit, Implicit::EndsParagraph);
    assert_eq!(hints(&p, "penalty").implicit, Implicit::None);
    assert_eq!(hints(&p, "kern").implicit, Implicit::None);
    assert_eq!(hints(&p, "hbox").implicit, Implicit::None);
    assert_eq!(hints(&p, "unvbox").implicit, Implicit::EndsParagraph);
    assert_eq!(hints(&p, "vrule").implicit, Implicit::StartsParagraph);
    assert_eq!(hints(&p, "hrule").implicit, Implicit::EndsParagraph);
    assert_eq!(hints(&p, "par").implicit, Implicit::EndsParagraph);
    assert_eq!(
        mark_at(&p, t, "ab").hints.implicit,
        Implicit::StartsParagraph
    );
    assert!(
        p.marks.iter().any(|m| p.atoms[m.atom].kind == AtomKind::Par
            && m.hints.implicit == Implicit::EndsParagraph)
    );
    // (the second letter of a run is no mark)
    assert!(!p.marks.iter().any(|m| p.atoms[m.atom].start == t.len() - 1));
}

#[test]
fn reads_name_the_meaning_and_what_it_rests_on() {
    let t = "\\def\\x{1pt}\\hskip\\x";
    let (p, _) = pass(t, FileKind::Document);
    let r = &hints(&p, "hskip").reads;
    assert!(r.contains(&"cs:\\hskip".to_owned()) && r.contains(&"cs:\\x".to_owned()));
}

#[test]
fn hints_serialise_as_json() {
    let t = "\\def\\foo#1{x}\\foo{a}";
    let (p, _) = pass(t, FileKind::Document);
    let j = p.hints_jsonl(t);
    assert!(j.contains(
        r#""claim":{"Predicted":{"n_tokens":3,"confirm":{"MeaningIs":{"cs":"cs:\\foo","expected":{"Macro":{"params":"m"}}}},"also":[]}}"#
    ), "{j}");
    assert!(
        j.contains(r#""may_define":{"Names":[{"field":"cs:\\foo","global":false}]}"#),
        "{j}"
    );
    assert_eq!(j.lines().count(), p.items.len() + p.marks.len());
}

#[test]
fn expl3_w_signatures_are_predicted() {
    let t = "\\ExplSyntaxOn \\if_meaning:w \\a \\b \\fi: \\use_none_delimit_by_q_stop:w a{b}c \\q_stop \\int_value:w 12 \\scan_stop:";
    let (p, _) = pass(t, FileKind::Package);
    let im = item(&p, "if_meaning:w");
    assert_eq!(im.status, Status::Predicted);
    assert_eq!(im.end - im.atom - 1, 2);
    let un = item(&p, "use_none_delimit_by_q_stop:w");
    assert_eq!(un.status, Status::Predicted);
    assert_eq!(p.atoms[un.end - 1].name(t), "q_stop");
    assert_eq!(item(&p, "int_value:w").status, Status::Predicted);
    let off = analyze_with(
        t,
        FileKind::Package,
        Scope {
            document: &Definitions::new(),
            file: 0,
            prelude: &Definitions::new(),
        },
        Features {
            w_signatures: false,
            ..Features::default()
        },
    );
    assert_eq!(item(&off, "if_meaning:w").status, Status::Pending);
}

#[test]
fn expandafter_looks_through_one_step() {
    let t = "\\def\\b{}\\def\\c{{x}}\\expandafter\\textbf\\b{y}z \\expandafter\\textbf\\c w";
    let (p, _) = pass(t, FileKind::Document);
    let ea: Vec<&Item> = p.items.iter().filter(|i| i.name == "expandafter").collect();
    assert_eq!(ea.len(), 2);
    assert_eq!(ea[0].status, Status::Predicted);
    // (`\textbf` reads `{y}` from the text after `\b`'s empty body)
    assert_eq!(&t[p.atoms[ea[0].end].start..p.atoms[ea[0].end].end], "z");
    // (`\c`'s body gives the argument: nothing more is consumed)
    assert_eq!(ea[1].status, Status::Predicted);
    assert_eq!(&t[p.atoms[ea[1].end].start..p.atoms[ea[1].end].end], "w");
    match &ea[0].hints.claim {
        Claim::Predicted { confirm, .. } => assert_eq!(
            confirm,
            &Assumption::MeaningIs {
                cs: "b".into(),
                expected: MeaningShape::Macro {
                    params: String::new()
                }
            }
        ),
        c => panic!("{c:?}"),
    }
    // (`\textbf` and `\b` are its operands, not items)
    assert!(
        p.items
            .iter()
            .all(|i| i.name != "b" || p.atoms[i.atom].start < 20)
    );
}

#[test]
fn conflicting_definitions_bind_by_load_order() {
    let f0 = "\\def\\foo#1{}";
    let f1 = "\\def\\foo#1#2{}";
    let doc = "\\foo{a}{b}";
    let mut d = Definitions::new();
    d.harvest(f0, FileKind::Package, 0);
    d.harvest(f1, FileKind::Package, 1);
    d.harvest(doc, FileKind::Document, 2);
    let run = |d: &Definitions| {
        analyze(
            doc,
            FileKind::Document,
            Scope {
                document: d,
                file: 2,
                prelude: &Definitions::new(),
            },
        )
    };
    let p = run(&d);
    assert_eq!(item(&p, "foo").status, Status::Pending);
    d.set_load_order(&[0, 1, 2]);
    let p = run(&d);
    let it = item(&p, "foo");
    assert_eq!(it.status, Status::Predicted);
    assert_eq!(it.end, p.atoms.len());
    match &it.hints.claim {
        Claim::Predicted { confirm, .. } => assert_eq!(
            confirm,
            &Assumption::MeaningIs {
                cs: "foo".into(),
                expected: MeaningShape::Macro {
                    params: "mm".into()
                }
            }
        ),
        c => panic!("{c:?}"),
    }
    // (the document between the two: the earlier binds)
    d.set_load_order(&[0, 2, 1]);
    let p = run(&d);
    let it = item(&p, "foo");
    assert_eq!(it.status, Status::Predicted);
    assert_eq!(&doc[p.atoms[it.end].start..], "{b}");
}

#[test]
fn output_routine_definition_set() {
    let t = "\\output={\\plainoutput}\\def\\plainoutput{\\shipout\\vbox{\\x}\\global\\advance\\count0 by 1 \\y}\
             \\def\\x{\\def\\z{}}\\def\\y{\\csname q\\the\\count0\\endcsname}";
    let mut d = Definitions::new();
    d.harvest(t, FileKind::Package, 0);
    let s = d.output_defines(&Definitions::new()).unwrap();
    assert_eq!(s.names.get("cs:\\z"), Some(&false));
    assert_eq!(s.names.get("count:0"), Some(&true));
    assert_eq!(s.prefixes.get("cs:\\q"), Some(&false));
    assert!(s.unbounded.is_empty(), "{:?}", s.unbounded);
}

/// The kernel's `\output {` (ltoutput's part of `latex.ltx`) is read as
/// the control sequence, under no catcode hazard.
#[test]
#[ignore = "reads the installed latex.ltx"]
fn latex_ltx_output_is_a_control_sequence() {
    let path = std::env::var("PHITEX_LATEX_LTX")
        .unwrap_or_else(|_| "/usr/share/texmf-dist/tex/latex/base/latex.ltx".into());
    let t = std::fs::read_to_string(path).unwrap();
    let at = t.find("\n\\output {").expect("\\output {") + 1;
    let (p, _) = pass(&t, FileKind::Package);
    for h in &p.hazards {
        assert!(
            !(h.start <= at && at < h.end),
            "hazard {} covers \\output",
            h.cause
        );
    }
    let tok = p
        .tokens
        .iter()
        .find(|k| k.start == at)
        .expect("a token at \\output");
    assert_eq!(
        (tok.kind, &t[tok.start..tok.end]),
        (TokKind::Cs, "\\output")
    );
}

/// latex.ltx 8787: `\lowercase`'s braces are not a group (§1285 reads
/// them with `scan_toks`, §473), so the `\egroup` in them ends the
/// `\bgroup` before, once the argument is read.
#[test]
fn a_group_ended_in_an_argument_ends_after_it() {
    let t = r"\bgroup\catcode`\ =11\relax\lowercase{\egroup}\x";
    let (p, _) = pass(t, FileKind::Package);
    assert_eq!(p.hazards[0].end, t.rfind('}').unwrap());
    let x = p.tokens.last().unwrap();
    assert_eq!((x.kind, x.tz), (TokKind::Cs, Status::Known));
}

/// latex.ltx 9649: `[` and `]` made the group's characters and `{` a
/// letter (§1232); the character after `` \catcode` `` is a number's,
/// never a group's (§442).
#[test]
fn other_characters_can_open_and_close_groups() {
    let t = r"{\catcode`[=1 \catcode`]=2
\catcode`{=11 \catcode`}=11
\gdef\a[{]
]\x";
    let (p, _) = pass(t, FileKind::Package);
    let x = p.tokens.last().unwrap();
    assert_eq!((x.kind, x.tz), (TokKind::Cs, Status::Known));
    let opens: Vec<&str> = p
        .braces
        .iter()
        .map(|b| text_of(t, &p.tokens[b.open]))
        .collect();
    assert_eq!(opens, ["{", "["]);
    assert!(p.hazards.iter().all(|h| h.end == t.rfind(']').unwrap()));
}

/// latex.ltx 15436: `|` made an escape character (§1232) starts control
/// sequences, so `|endgroup` ends the group.
#[test]
fn another_escape_character_starts_control_sequences() {
    let t = r"\begingroup\catcode`|=0 |gdef|a{}|endgroup\x";
    let (p, _) = pass(t, FileKind::Package);
    let names: Vec<&str> = p.items.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(names, ["begingroup", "catcode", "gdef", "endgroup", "x"]);
    assert_eq!(p.tokens.last().unwrap().tz, Status::Known);
}

#[test]
fn a_catcode_set_of_a_macros_argument_is_read_at_the_call() {
    // (`` \catcode\number`#1 ``: the character the argument names, §442,
    // §440; the change made when the macro runs, §1232)
    let t = "\\def\\mk#1{\\catcode\\number`#1=11 }\\mk\\@ \\a@b";
    let (p, d) = pass(t, FileKind::Document);
    assert_eq!(d.changes_catcodes("mk", None).0, 0);
    assert_eq!(d.arg_catcodes("mk", None), &[(1, 11)]);
    assert!(p.hazards.iter().all(|h| h.status != Status::Pending));
    assert!(p.items.iter().any(|i| i.name == "a@b"));
    // (an argument naming no character: pending)
    let t = "\\def\\mk#1{\\catcode`#1=11 }\\mk{ab} \\a@b";
    let (p, _) = pass(t, FileKind::Document);
    assert_eq!(p.hazards[0].status, Status::Pending);
}

#[test]
fn a_catcode_in_a_token_list_or_a_nested_body_is_not_run() {
    // (a token list's value is stored, §1226; a definition's body runs
    // when its macro is called, §1218)
    let t = "\\def\\x{\\toks@{\\catcode`\\{=12 }}\\x {a}";
    let (p, d) = pass(t, FileKind::Package);
    assert_eq!(d.changes_catcodes("x", None).0, 0);
    assert!(p.hazards.iter().all(|h| h.status != Status::Pending));
    let t = "\\def\\x{\\edef\\noexpand\\y{\\catcode`\\{=12 }}\\x {a}";
    let (_, d) = pass(t, FileKind::Package);
    assert_eq!(d.changes_catcodes("x", None).0, 0);
    // (called where it is defined: run)
    let t = "\\def\\x{\\def\\y{\\catcode`\\{=12 }\\y}\\x {a}";
    let (_, d) = pass(t, FileKind::Package);
    assert_eq!(d.changes_catcodes("x", None).0, 2);
}

#[test]
fn a_macro_setting_literal_catcodes_sets_them_at_the_call() {
    // (`\@makeother\[`, `` \catcode`\$=3 ``: the catcodes the call sets,
    // §1232, not characters pending wherever they occur)
    let t = "\\def\\sc{\\@makeother\\[\\catcode`\\$=3 \\catcode`\\|=13 }\\sc [a] |b|";
    let (p, d) = pass(t, FileKind::Package);
    assert_eq!(
        d.set_catcodes("sc", None),
        &[(b'[', 12), (b'$', 3), (b'|', 13)]
    );
    let pending: Vec<_> = p
        .hazards
        .iter()
        .filter(|h| h.status == Status::Pending)
        .map(|h| &t[h.start..h.end])
        .collect();
    // (each `|` pending to the next on its line, the last to the line's end)
    assert_eq!(pending, ["|b|", "|"]);
}

#[test]
fn a_macro_ending_the_callers_group_ends_its_catcode_changes() {
    // (the `\endgroup` the macro runs ends the group and what was set in
    // it, §1063, §283; here in a macro it defines and calls)
    let t = "\\begingroup\\catcode`\\_=11 \\def\\x{\\def\\y{\\endgroup}\\y}\\x \\a_b";
    let (p, d) = pass(t, FileKind::Package);
    assert!(d.ends_group("x", None));
    assert_eq!(p.hazards[0].end, t.rfind("\\x").unwrap());
    assert!(!p.items.iter().any(|i| i.name == "a_b"));
}

#[test]
fn makeother_of_an_other_character_changes_nothing() {
    // (`\@makeother\=`: `=` is other already, §232)
    let t = "\\@makeother\\= a=b";
    let (p, _) = pass(t, FileKind::Package);
    assert_eq!(p.hazards, []);
}

#[test]
fn a_catcode_of_a_named_constant_is_that_characters() {
    // (`\catcode\z@`: the character whose code is the constant, §440)
    let t = "\\catcode\\z@=11 \\a";
    let (p, _) = pass(t, FileKind::Document);
    assert!(p.hazards.iter().all(|h| h.status != Status::Pending));
}
