//! What the modern command line builds (DESIGN.md, "Command line and
//! terminal"): `phitex.toml` if there is one, else the file named on the
//! command line, with the `% !TEX program` and `% !TEX root` magic
//! comments its first lines may hold.
//!
//! `phitex.toml` is read as the small subset of TOML it needs: `key =
//! value` lines with quoted strings, booleans and integers, `#` comments,
//! and `[build]` (or any other) table headers, which are ignored.

use std::path::{Path, PathBuf};

/// The settings of a build.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Config {
    /// The main file.
    pub main: Option<String>,
    /// The engine, as a program name (`pdflatex`, `pdftex`, `latex`,
    /// `tex`).
    pub engine: Option<String>,
    /// Where the outputs go (`-output-directory`).
    pub output_dir: Option<String>,
    pub shell_escape: Option<bool>,
    /// A program to show the PDF with (`phitex watch --open`).
    pub viewer: Option<String>,
    /// Where to copy the final PDF after each successful build (`copy-pdf
    /// = true | "dir"`).
    pub copy_pdf: Option<CopyPdf>,
    /// `phitex watch` in machine mode (`machine = false`: the session
    /// path).
    pub machine: Option<bool>,
    /// `phitex watch` on the dynamic-SSA runtime (`ssa = true`, as
    /// `--ssa`; experimental).
    pub ssa: Option<bool>,
    /// Where machine-mode builds are kept across processes (`store =
    /// "dir"`, relative to `phitex.toml`'s directory; DESIGN.md §7.9).
    pub store: Option<String>,
    /// Whether `phitex build` and `phitex watch` write `<job>.synctex.gz`
    /// (`synctex = false`: not; on by default, DESIGN 4.5).
    pub synctex: Option<bool>,
}

/// `copy-pdf`'s value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CopyPdf {
    /// `false`: don't copy.
    Off,
    /// `true`: into the directory phitex was invoked from.
    Invocation,
    /// A directory; a relative one is relative to `phitex.toml`'s.
    Dir(String),
}

/// The name of the configuration file.
pub const FILE: &str = "phitex.toml";

/// Its name in the builds named partex, still read where there is no
/// [`FILE`].
const LEGACY: &str = "partex.toml";

/// Parse `phitex.toml`'s `text`.
pub fn parse(text: &str) -> Result<Config, String> {
    let mut c = Config::default();
    for (n, line) in text.lines().enumerate() {
        let line = strip_comment(line).trim();
        if line.is_empty() || (line.starts_with('[') && line.ends_with(']')) {
            continue;
        }
        let err = |what: &str| format!("{FILE}:{}: {what}", n + 1);
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| err("expected `key = value`"))?;
        let (key, value) = (key.trim(), value.trim());
        let string = || -> Result<String, String> {
            unquote(value).ok_or_else(|| err(&format!("`{key}` must be a quoted string")))
        };
        let boolean = || -> Result<bool, String> {
            match value {
                "true" => Ok(true),
                "false" => Ok(false),
                _ => Err(err(&format!("`{key}` must be true or false"))),
            }
        };
        match key.trim_matches('"') {
            "main" | "file" => c.main = Some(string()?),
            "engine" | "program" => c.engine = Some(string()?),
            "output-dir" | "output-directory" | "out-dir" | "output_dir" => {
                c.output_dir = Some(string()?);
            }
            "shell-escape" | "shell_escape" => c.shell_escape = Some(boolean()?),
            "viewer" => c.viewer = Some(string()?),
            "machine" => c.machine = Some(boolean()?),
            "ssa" => c.ssa = Some(boolean()?),
            "store" => c.store = Some(string()?),
            "synctex" => c.synctex = Some(boolean()?),
            "copy-pdf" | "copy_pdf" => {
                c.copy_pdf = Some(match value {
                    "true" => CopyPdf::Invocation,
                    "false" => CopyPdf::Off,
                    _ => CopyPdf::Dir(unquote(value).ok_or_else(|| {
                        err("`copy-pdf` must be true, false or a quoted directory")
                    })?),
                });
            }
            k => return Err(err(&format!("unknown key `{k}`"))),
        }
    }
    Ok(c)
}

/// `line` without a `#` comment (outside quotes).
fn strip_comment(line: &str) -> &str {
    let mut quoted = false;
    for (i, c) in line.char_indices() {
        match c {
            '"' => quoted = !quoted,
            '#' if !quoted => return &line[..i],
            _ => {}
        }
    }
    line
}

/// A TOML basic or literal string's value.
fn unquote(v: &str) -> Option<String> {
    if let Some(s) = v.strip_prefix('\'') {
        return s.strip_suffix('\'').map(str::to_owned);
    }
    let s = v.strip_prefix('"')?.strip_suffix('"')?;
    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next()? {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                c @ ('\\' | '"') => out.push(c),
                _ => return None,
            }
        } else {
            out.push(c);
        }
    }
    Some(out)
}

/// `phitex.toml` (or the older `phitex.toml`) in `dir` or the nearest directory
/// above it: the directory it is in, and its settings.
pub fn find(dir: &Path) -> Result<Option<(PathBuf, Config)>, String> {
    for d in dir.ancestors() {
        for name in [FILE, LEGACY] {
            if let Ok(text) = std::fs::read_to_string(d.join(name)) {
                return parse(&text)
                    .map(|c| Some((d.to_path_buf(), c)))
                    .map_err(|e| e.replacen(FILE, name, 1));
            }
        }
    }
    Ok(None)
}

/// The magic comments in the first lines of a TeX file: `% !TEX program
/// = name` and `% !TEX root = file` (`TeXShop`'s and `TeXstudio`'s), as
/// (program, root).
#[must_use]
pub fn magic_comments(text: &str) -> (Option<String>, Option<String>) {
    let (mut program, mut root) = (None, None);
    for line in text.lines().take(20) {
        let Some(rest) = line.trim_start().strip_prefix('%') else {
            if line.trim().is_empty() {
                continue;
            }
            break;
        };
        let rest = rest.trim_start();
        let Some(rest) = rest.strip_prefix('!') else {
            continue;
        };
        let rest = rest.trim_start();
        let Some((key, value)) = rest.split_once('=') else {
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        let value = value.trim().to_owned();
        match key.as_str() {
            "tex program" | "tex ts-program" | "tex engine" => program = Some(value),
            "tex root" => root = Some(value),
            _ => {}
        }
    }
    (program, root)
}

/// The engine for `text` when nothing names one: `pdflatex` for a LaTeX
/// document, else `pdftex`. `\begin{document}` counts too: the class may
/// be in a shared preamble the main file inputs.
#[must_use]
pub fn default_engine(text: &str) -> &'static str {
    let latex = ["\\documentclass", "\\documentstyle", "\\begin{document}"];
    if latex.iter().any(|m| text.contains(m)) {
        "pdflatex"
    } else {
        "pdftex"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toml_subset() {
        let c = parse(
            "# my paper\n[build]\nmain = \"paper.tex\"  # the root\nengine = 'pdflatex'\n\
             output-dir = \"out\"\nshell-escape = false\nviewer = \"zathura\"\n",
        )
        .unwrap();
        assert_eq!(
            c,
            Config {
                main: Some("paper.tex".into()),
                engine: Some("pdflatex".into()),
                output_dir: Some("out".into()),
                shell_escape: Some(false),
                viewer: Some("zathura".into()),
                copy_pdf: None,
                machine: None,
                ssa: None,
                store: None,
                synctex: None,
            }
        );
        assert_eq!(parse("ssa = true").unwrap().ssa, Some(true));
        assert_eq!(parse("synctex = false").unwrap().synctex, Some(false));
        assert_eq!(
            parse("copy-pdf = true").unwrap().copy_pdf,
            Some(CopyPdf::Invocation)
        );
        assert_eq!(
            parse("copy-pdf = false").unwrap().copy_pdf,
            Some(CopyPdf::Off)
        );
        assert_eq!(
            parse("copy-pdf = \"../pdfs\"  # for Dropbox")
                .unwrap()
                .copy_pdf,
            Some(CopyPdf::Dir("../pdfs".into()))
        );
        assert!(parse("copy-pdf = yes").is_err());
        assert_eq!(
            parse("mian = \"x\"").unwrap_err(),
            "phitex.toml:1: unknown key `mian`"
        );
        assert!(parse("main = paper.tex").is_err());
    }

    #[test]
    fn magic() {
        let text = "% !TEX program = xelatex\n%!TEX root = ../main.tex\n\\documentclass{article}";
        assert_eq!(
            magic_comments(text),
            (Some("xelatex".into()), Some("../main.tex".into()))
        );
        assert_eq!(
            magic_comments("\\relax\n% !TEX program = tex"),
            (None, None)
        );
        assert_eq!(default_engine(text), "pdflatex");
        assert_eq!(default_engine("\\input plain"), "pdftex");
        // the class in a shared preamble
        assert_eq!(
            default_engine("\\input{preamble}\n\\begin{document}\nx\n\\end{document}"),
            "pdflatex"
        );
    }
}
