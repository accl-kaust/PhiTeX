//! Which command line an invocation speaks (DESIGN.md, "Command line and
//! terminal").
//!
//! partex run under an engine's name (`tex`, `pdftex`, `pdflatex`, … as the
//! links in `target/partex-shim` are) is that engine: web2c's command line
//! and TeX's own terminal output, for `latexmk`, editors and scripts. Bare
//! `partex` is the modern command line (`partex build`, `partex watch`, …),
//! unless `--compat=NAME` (first argument) or `PARTEX_COMPAT=NAME` asks for
//! an engine's.

/// What an invocation is.
#[derive(Debug, PartialEq, Eq)]
pub enum Selection {
    /// An engine's command line: the program name TeX sees and the
    /// arguments after it.
    Compat { progname: String, args: Vec<String> },
    /// The modern command line, with its arguments.
    Modern(Vec<String>),
    /// A name reserved for an engine partex does not have yet.
    Unsupported(String),
}

/// Engines whose names are reserved: invoked under one of them, partex
/// says it cannot be that engine yet rather than running as Knuth's TeX.
const RESERVED: &[&str] = &[
    "xetex",
    "xelatex",
    "luatex",
    "lualatex",
    "luahbtex",
    "lualatex-dev",
    "xelatex-dev",
];

/// `XeTeX`'s names, reserved until its engine is complete; `PARTEX_XETEX=1`
/// runs them already (development).
const XETEX: &[&str] = &["xetex", "xelatex", "xelatex-dev"];

/// How partex was invoked: `argv0` (the file name it was run as), the
/// arguments after it, and `PARTEX_COMPAT`.
#[must_use]
pub fn select(argv0: &str, mut args: Vec<String>, env: Option<&str>) -> Selection {
    let stem = std::path::Path::new(argv0)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("partex");
    let named = |name: &str, args: Vec<String>| {
        let xetex_dev = XETEX.contains(&name)
            && std::env::var_os("PARTEX_XETEX").is_some_and(|v| v == "1");
        if RESERVED.contains(&name) && !xetex_dev {
            Selection::Unsupported(name.to_owned())
        } else {
            Selection::Compat {
                progname: name.to_owned(),
                args,
            }
        }
    };
    if stem != "partex" {
        return named(stem, args);
    }
    if let Some(first) = args.first() {
        let opt = first.strip_prefix("--").or_else(|| first.strip_prefix('-'));
        if let Some(name) = opt.and_then(|o| o.strip_prefix("compat=")) {
            let name = name.to_owned();
            args.remove(0);
            return named(&name, args);
        }
        if matches!(opt, Some("compat")) && args.len() > 1 {
            let name = args[1].clone();
            args.drain(..2);
            return named(&name, args);
        }
    }
    match env.filter(|e| !e.is_empty()) {
        Some(name) => named(name, args),
        None => Selection::Modern(args),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(a: &[&str]) -> Vec<String> {
        a.iter().map(|&s| s.to_owned()).collect()
    }

    #[test]
    fn engine_names_are_compat() {
        for name in ["tex", "pdftex", "pdflatex", "etex", "latex", "initex"] {
            assert_eq!(
                select(&format!("/x/partex-shim/{name}"), v(&["a.tex"]), None),
                Selection::Compat {
                    progname: name.into(),
                    args: v(&["a.tex"])
                }
            );
        }
        // (the name wins over the environment)
        assert_eq!(
            select("pdflatex", v(&[]), Some("tex")),
            Selection::Compat {
                progname: "pdflatex".into(),
                args: v(&[])
            }
        );
    }

    #[test]
    fn bare_partex_is_modern() {
        assert_eq!(
            select("target/release/partex", v(&["build", "x.tex"]), None),
            Selection::Modern(v(&["build", "x.tex"]))
        );
        assert_eq!(
            select("partex", v(&[]), Some("")),
            Selection::Modern(v(&[]))
        );
    }

    #[test]
    fn compat_by_option_or_environment() {
        let tex = |args: &[&str]| Selection::Compat {
            progname: "tex".into(),
            args: v(args),
        };
        assert_eq!(
            select("partex", v(&["--compat=tex", "-ini", "x"]), None),
            tex(&["-ini", "x"])
        );
        assert_eq!(
            select("partex", v(&["-compat", "tex", "x"]), None),
            tex(&["x"])
        );
        assert_eq!(select("partex", v(&["x"]), Some("tex")), tex(&["x"]));
        // (the option wins over the environment)
        assert_eq!(
            select("partex", v(&["--compat=tex"]), Some("pdftex")),
            tex(&[])
        );
    }

    #[test]
    fn reserved_names() {
        assert_eq!(
            select("xelatex", v(&["a"]), None),
            Selection::Unsupported("xelatex".into())
        );
        assert_eq!(
            select("partex", v(&["--compat=lualatex"]), None),
            Selection::Unsupported("lualatex".into())
        );
    }
}
