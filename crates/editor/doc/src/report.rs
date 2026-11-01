//! The views as a report: text for a terminal, or JSON.

use std::fmt::Write as _;

use crate::views::{EnvProblem, Keyed, Loc, Views};
use crate::{AssetKind, InputKind, Outline, Project, text};

/// Times measured by the caller, in milliseconds, by phase.
#[derive(Clone, Debug, Default)]
pub struct Timing {
    pub phases: Vec<(String, f64)>,
}

fn at(p: &Project, l: Loc) -> String {
    format!("{}:{}", p.path(l.file), l.line)
}

fn how(h: InputKind) -> &'static str {
    match h {
        InputKind::Input => "input",
        InputKind::Include => "include",
        InputKind::Subfile => "subfile",
        InputKind::Import => "import",
        InputKind::Package => "package",
    }
}

fn asset(h: AssetKind) -> &'static str {
    match h {
        AssetKind::Graphics => "graphics",
        AssetKind::Listing => "listing",
        AssetKind::Pdf => "pdf",
    }
}

/// The views as a text report.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn report(p: &Project, v: &Views<'_>, timing: Option<&Timing>) -> String {
    let mut o = String::new();
    let read = v.files.iter().filter(|f| f.found.is_some()).count();
    let paras: usize = p.files().map(|(id, _)| p.paras(id).len()).sum();
    let _ = writeln!(
        o,
        "{}: {} files read as TeX, {} paragraphs, class {}",
        p.path(p.main()),
        read,
        paras,
        v.class.unwrap_or("unknown")
    );

    let toc = v.toc.iter().filter(|t| t.list == "toc").count();
    let _ = writeln!(
        o,
        "\nOutline: {} headings, {} entries in the table of contents",
        v.outline.len(),
        toc
    );
    let width = v
        .outline
        .iter()
        .map(|h| at(p, h.loc).len())
        .max()
        .unwrap_or(0);
    for h in &v.outline {
        let indent = "  ".repeat(usize::try_from(h.level + 1).unwrap_or(0));
        let number = match (&h.number, h.star) {
            (Some(n), _) => n.clone(),
            (None, true) => "*".to_owned(),
            (None, false) => "-".to_owned(),
        };
        let number = if h.level == -1 && h.number.is_some() {
            format!("Part {number}")
        } else {
            number
        };
        let _ = write!(
            o,
            "  {:width$}  {indent}{number}  {}",
            at(p, h.loc),
            text::display(h.title)
        );
        if let Some(l) = h.label {
            let _ = write!(o, "  [{l}]");
        }
        if !h.guarded {
            o.push_str("  (unguarded)");
        }
        o.push('\n');
    }

    let dups = v.duplicates();
    let _ = writeln!(
        o,
        "\nLabels: {}, {}",
        v.labels.len(),
        match dups.len() {
            0 => "none defined twice".to_owned(),
            n => format!("{n} defined more than once"),
        }
    );
    for (k, ls) in &dups {
        let places: Vec<String> = ls.iter().map(|l| at(p, l.loc)).collect();
        let _ = writeln!(o, "  {k}: {}", places.join(", "));
    }

    let unresolved: Vec<&Keyed> = v.unresolved().collect();
    let _ = writeln!(
        o,
        "\nReferences: {}, {} unresolved",
        v.refs.len(),
        unresolved.len()
    );
    for r in &unresolved {
        let _ = writeln!(
            o,
            "  {} at {} (\\{}){}",
            r.key,
            at(p, r.loc),
            r.fact.cs,
            if r.guarded { "" } else { " (unguarded)" }
        );
    }

    let mut cited: Vec<&str> = v.cites.iter().map(|c| c.key).collect();
    cited.sort_unstable();
    cited.dedup();
    let _ = write!(o, "\nCitations: {}, {} keys", v.cites.len(), cited.len());
    if v.nocite_all {
        o.push_str(", \\nocite{*}");
    }
    o.push('\n');
    for b in &v.bibs {
        match b.keys {
            Some(n) => {
                let _ = writeln!(o, "  database {} ({n} keys)", b.path);
            }
            None => {
                let _ = writeln!(o, "  database {} not found ({})", b.path, at(p, b.loc));
            }
        }
    }
    for c in v.missing_cites() {
        let _ = writeln!(o, "  {} at {}: in no database", c.key, at(p, c.loc));
    }

    let _ = writeln!(o, "\nFiles (reading order):");
    for f in &v.files {
        let indent = "  ".repeat(f.depth + 1);
        let _ = write!(o, "{indent}{}", f.path);
        if let Some(l) = f.loc {
            let _ = write!(o, "  (\\{} at {}", how(f.how), at(p, l));
            if let Some(via) = f.fact.and_then(|x| x.via.as_ref()) {
                let names: Vec<String> = via.iter().map(|m| format!("\\{m}")).collect();
                let _ = write!(o, ", through {}", names.join(", "));
            }
            o.push(')');
        }
        if f.found.is_none() {
            o.push_str("  NOT FOUND");
        }
        if f.excluded {
            o.push_str("  (left out by \\includeonly)");
        }
        if f.cycle {
            o.push_str("  (a cycle: not read again)");
        }
        o.push('\n');
    }
    let missing: Vec<_> = v.assets.iter().filter(|a| !a.found).collect();
    let _ = writeln!(o, "Assets: {}, {} not found", v.assets.len(), missing.len());
    for a in missing {
        let _ = writeln!(o, "  {} ({}) at {}", a.path, asset(a.how), at(p, a.loc));
    }

    if v.envs.is_empty() {
        let _ = writeln!(o, "\nEnvironments: they nest");
    } else {
        let _ = writeln!(o, "\nEnvironments: {} problems", v.envs.len());
        for e in &v.envs {
            let _ = match e {
                EnvProblem::Unclosed { name, loc } => {
                    writeln!(o, "  \\begin{{{name}}} at {} never ended", at(p, *loc))
                }
                EnvProblem::Unopened { name, loc } => {
                    writeln!(o, "  \\end{{{name}}} at {} ends nothing", at(p, *loc))
                }
                EnvProblem::Mismatch {
                    name,
                    loc,
                    open,
                    open_loc,
                } => writeln!(
                    o,
                    "  \\end{{{name}}} at {} while \\begin{{{open}}} at {} is open",
                    at(p, *loc),
                    at(p, *open_loc)
                ),
            };
        }
    }

    let expanded: Vec<String> = v.expanded.keys().map(|m| format!("\\{m}")).collect();
    let _ = writeln!(
        o,
        "\nDefinitions: {} ({} with a name not known){}",
        v.defs.len(),
        v.dynamic.len(),
        if expanded.is_empty() {
            String::new()
        } else {
            format!("; expanded for facts: {}", expanded.join(", "))
        }
    );

    for (name, n) in &v.ambiguous {
        let _ = writeln!(
            o,
            "  \\{name}: {n} different definitions (its calls are not expanded)"
        );
    }
    if v.broken.is_empty() {
        let _ = writeln!(
            o,
            "Guards: kept ({} control sequences assumed to mean LaTeX's)",
            v.uses.len()
        );
    } else {
        let _ = writeln!(o, "Guards: {} broken", v.broken.len());
        for b in &v.broken {
            let _ = writeln!(
                o,
                "  \\{}: {}{} ({} facts unguarded)",
                b.cs,
                b.cause,
                b.loc
                    .map(|l| format!(" at {}", at(p, l)))
                    .unwrap_or_default(),
                b.facts
            );
        }
    }

    if let Some(t) = timing {
        let phases: Vec<String> = t
            .phases
            .iter()
            .map(|(n, ms)| format!("{n} {ms:.3} ms"))
            .collect();
        let _ = writeln!(o, "\nTiming: {}", phases.join(", "));
    }
    o
}

/// A JSON string.
fn s(x: &str) -> String {
    let mut o = String::with_capacity(x.len() + 2);
    o.push('"');
    for c in x.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(o, "\\u{:04x}", c as u32);
            }
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

fn opt(x: Option<&str>) -> String {
    x.map_or_else(|| "null".to_owned(), s)
}

fn loc(p: &Project, l: Loc) -> String {
    format!(r#""file":{},"line":{}"#, s(p.path(l.file)), l.line)
}

fn keyed(p: &Project, k: &Keyed<'_>) -> String {
    format!(
        r#"{{"key":{},{},"cs":{},"guarded":{}}}"#,
        s(k.key),
        loc(p, k.loc),
        s(k.fact.cs),
        k.guarded
    )
}

fn array(items: impl Iterator<Item = String>) -> String {
    let v: Vec<String> = items.collect();
    format!("[{}]", v.join(","))
}

/// The views as JSON; each heading with where the PDF shows it when
/// `placed` is given (an outline [`Outline::place`]d, made from the same
/// project).
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn json(
    p: &Project,
    v: &Views<'_>,
    timing: Option<&Timing>,
    placed: Option<&Outline>,
) -> String {
    let mut fields: Vec<String> = Vec::new();
    fields.push(format!(r#""main":{}"#, s(p.path(p.main()))));
    fields.push(format!(r#""class":{}"#, opt(v.class)));
    fields.push(format!(
        r#""outline":{}"#,
        array(v.outline.iter().enumerate().map(|(i, h)| {
            let (ts, te) = h.title_range.unwrap_or((h.start, h.start));
            let pdf = placed
                .and_then(|o| o.entries.get(i))
                .map(|e| {
                    let num = |x: Option<f64>| x.map_or_else(|| "null".to_owned(), |x| format!("{x:.3}"));
                    format!(
                        r#","page":{},"x":{},"y":{}"#,
                        e.page.map_or_else(|| "null".to_owned(), |n| n.to_string()),
                        num(e.x),
                        num(e.y)
                    )
                })
                .unwrap_or_default();
            format!(
                r#"{{"level":{},"star":{},"number":{},"title":{},"short":{},{},"start":{},"end":{},"titleStart":{ts},"titleEnd":{te},"label":{},"guarded":{}{pdf}}}"#,
                h.level,
                h.star,
                opt(h.number.as_deref()),
                s(&text::display(h.title)),
                opt(h.short),
                loc(p, h.loc),
                h.start,
                h.end,
                opt(h.label),
                h.guarded
            )
        }))
    ));
    fields.push(format!(
        r#""toc":{}"#,
        array(v.toc.iter().map(|t| {
            format!(
                r#"{{"list":{},"level":{},"number":{},"title":{},{}}}"#,
                s(t.list),
                s(t.level),
                opt(t.number.as_deref()),
                s(&text::display(t.title)),
                loc(p, t.loc)
            )
        }))
    ));
    fields.push(format!(
        r#""labels":{}"#,
        array(v.labels.iter().map(|k| keyed(p, k)))
    ));
    fields.push(format!(
        r#""duplicateLabels":{}"#,
        array(v.duplicates().iter().map(|(k, _)| s(k)))
    ));
    fields.push(format!(
        r#""references":{}"#,
        array(v.refs.iter().map(|k| keyed(p, k)))
    ));
    fields.push(format!(
        r#""unresolved":{}"#,
        array(v.unresolved().map(|k| keyed(p, k)))
    ));
    fields.push(format!(
        r#""citations":{}"#,
        array(v.cites.iter().map(|k| keyed(p, k)))
    ));
    fields.push(format!(
        r#""missingCitations":{}"#,
        array(v.missing_cites().map(|k| keyed(p, k)))
    ));
    fields.push(format!(
        r#""databases":{}"#,
        array(v.bibs.iter().map(|b| {
            format!(
                r#"{{"path":{},"keys":{},{}}}"#,
                s(b.path),
                b.keys.map_or_else(|| "null".to_owned(), |n| n.to_string()),
                loc(p, b.loc)
            )
        }))
    ));
    fields.push(format!(
        r#""files":{}"#,
        array(v.files.iter().map(|f| {
            format!(
                r#"{{"path":{},"depth":{},"how":{},"found":{},"at":{},"excluded":{},"cycle":{},"guarded":{}}}"#,
                s(&f.path),
                f.depth,
                s(how(f.how)),
                f.found.is_some(),
                f.loc
                    .map_or_else(|| "null".to_owned(), |l| format!("{{{}}}", loc(p, l))),
                f.excluded,
                f.cycle,
                f.guarded
            )
        }))
    ));
    fields.push(format!(
        r#""assets":{}"#,
        array(v.assets.iter().map(|a| {
            format!(
                r#"{{"path":{},"kind":{},"found":{},{}}}"#,
                s(&a.path),
                s(asset(a.how)),
                a.found,
                loc(p, a.loc)
            )
        }))
    ));
    fields.push(format!(
        r#""environments":{}"#,
        array(v.envs.iter().map(|e| match e {
            EnvProblem::Unclosed { name, loc: l } => {
                format!(
                    r#"{{"problem":"unclosed","name":{},{}}}"#,
                    s(name),
                    loc(p, *l)
                )
            }
            EnvProblem::Unopened { name, loc: l } => {
                format!(
                    r#"{{"problem":"unopened","name":{},{}}}"#,
                    s(name),
                    loc(p, *l)
                )
            }
            EnvProblem::Mismatch {
                name,
                loc: l,
                open,
                open_loc,
            } => format!(
                r#"{{"problem":"mismatch","name":{},{},"open":{},"openLine":{}}}"#,
                s(name),
                loc(p, *l),
                s(open),
                open_loc.line
            ),
        }))
    ));
    fields.push(format!(
        r#""definitions":{}"#,
        array(v.defs.iter().map(|d| {
            format!(
                r#"{{"name":{},"environment":{},"by":{},{}}}"#,
                s(&d.def.name),
                d.def.env,
                s(&d.def.cs),
                loc(p, d.loc)
            )
        }))
    ));
    fields.push(format!(
        r#""guards":{{"assumed":{},"broken":{}}}"#,
        array(v.uses.keys().map(|cs| s(cs))),
        array(v.broken.iter().map(|b| {
            format!(
                r#"{{"cs":{},"cause":{},"at":{},"facts":{}}}"#,
                s(&b.cs),
                s(&b.cause),
                b.loc
                    .map_or_else(|| "null".to_owned(), |l| format!("{{{}}}", loc(p, l))),
                b.facts
            )
        }))
    ));
    if let Some(t) = timing {
        fields.push(format!(
            r#""timingMs":{{{}}}"#,
            t.phases
                .iter()
                .map(|(n, ms)| format!("{}:{ms:.3}", s(n)))
                .collect::<Vec<_>>()
                .join(",")
        ));
    }
    format!("{{{}}}\n", fields.join(","))
}
