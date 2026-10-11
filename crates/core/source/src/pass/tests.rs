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
        let mut s = Stats::default();
        for (k, (n, t)) in texts.iter().enumerate() {
            let p = analyze(
                t,
                FileKind::from_path(n),
                Scope {
                    document: &defs,
                    file: u32::try_from(k).unwrap(),
                    prelude: &prelude,
                },
            );
            s.add(&Stats::of(&p, t));
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
