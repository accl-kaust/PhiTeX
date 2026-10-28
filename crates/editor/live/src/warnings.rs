//! A build's warnings for the terminal, read from its transcript: box
//! warnings (TeX's own), and the warnings LaTeX and its packages write in
//! their standard forms, grouped and counted.

/// What a group of warnings is about, in the order they are shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    UndefinedReference,
    UndefinedCitation,
    Rerun,
    Font,
    OverfullHbox,
    OverfullVbox,
    UnderfullHbox,
    UnderfullVbox,
    Other,
}

impl Kind {
    fn name(self, n: usize) -> String {
        let s = if n == 1 { "" } else { "s" };
        match self {
            Kind::UndefinedReference => format!("{n} undefined reference{s}"),
            Kind::UndefinedCitation => format!("{n} undefined citation{s}"),
            Kind::Rerun => "LaTeX asks for another run".to_owned(),
            Kind::Font => format!("{n} font warning{s}"),
            Kind::OverfullHbox => format!("{n} overfull \\hbox{}", if n == 1 { "" } else { "es" }),
            Kind::OverfullVbox => format!("{n} overfull \\vbox{}", if n == 1 { "" } else { "es" }),
            Kind::UnderfullHbox => {
                format!("{n} underfull \\hbox{}", if n == 1 { "" } else { "es" })
            }
            Kind::UnderfullVbox => {
                format!("{n} underfull \\vbox{}", if n == 1 { "" } else { "es" })
            }
            Kind::Other => format!("{n} other warning{s}"),
        }
    }
}

/// The kind of warning a transcript line begins, if it begins one.
fn kind(line: &str) -> Option<Kind> {
    if let Some(r) = line.strip_prefix("LaTeX Warning: ") {
        return Some(if r.starts_with("Reference") {
            Kind::UndefinedReference
        } else if r.starts_with("Citation") {
            Kind::UndefinedCitation
        } else if r.starts_with("Label(s) may have changed") || r.contains("Rerun to get") {
            Kind::Rerun
        } else if r.starts_with("There were undefined references")
            || r.starts_with("There were multiply")
        {
            return None;
        } else {
            Kind::Other
        });
    }
    if line.starts_with("LaTeX Font Warning: ") {
        return Some(Kind::Font);
    }
    if line.starts_with("Overfull \\hbox") {
        return Some(Kind::OverfullHbox);
    }
    if line.starts_with("Overfull \\vbox") {
        return Some(Kind::OverfullVbox);
    }
    if line.starts_with("Underfull \\hbox") {
        return Some(Kind::UnderfullHbox);
    }
    if line.starts_with("Underfull \\vbox") {
        return Some(Kind::UnderfullVbox);
    }
    let (head, _) = line.split_once(": ")?;
    let mut w = head.split(' ');
    match (w.next(), w.next(), w.next(), w.next()) {
        (Some("Package" | "Class"), Some(_), Some("Warning"), None) => Some(Kind::Other),
        _ => None,
    }
}

/// The groups of warnings in transcript `log`, each with its count and
/// its first lines (at most three).
pub fn summarize(log: &str) -> Vec<(Kind, usize, Vec<String>)> {
    let mut groups: Vec<(Kind, usize, Vec<String>)> = Vec::new();
    for line in log.lines() {
        let Some(k) = kind(line) else { continue };
        let i = groups.iter().position(|g| g.0 == k).unwrap_or_else(|| {
            groups.push((k, 0, Vec::new()));
            groups.len() - 1
        });
        groups[i].1 += 1;
        if groups[i].2.len() < 3 {
            groups[i].2.push(line.trim().to_owned());
        }
    }
    groups.sort_by_key(|g| g.0);
    groups
}

/// The warnings as lines for the terminal (`verbose`: each group's first
/// lines too).
pub fn lines(log: &str, verbose: bool) -> Vec<String> {
    let mut out = Vec::new();
    for (k, n, first) in summarize(log) {
        out.push(k.name(n));
        if verbose {
            out.extend(first.into_iter().map(|l| format!("    {l}")));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups() {
        let log = "This is TeX\nLaTeX Warning: Reference `x' on page 1 undefined on input line 3.\n\
                   Overfull \\hbox (3.0pt too wide) in paragraph at lines 4--5\n\
                   LaTeX Warning: Reference `y' on page 1 undefined on input line 9.\n\
                   Package hyperref Warning: Token not allowed.\n\
                   LaTeX Warning: There were undefined references.\n";
        let g = summarize(log);
        assert_eq!(g.len(), 3);
        assert_eq!((g[0].0, g[0].1), (Kind::UndefinedReference, 2));
        assert_eq!(g[1].0, Kind::OverfullHbox);
        assert_eq!(g[2].0, Kind::Other);
        assert_eq!(lines(log, false)[0], "2 undefined references");
    }
}
