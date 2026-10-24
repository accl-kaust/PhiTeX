//! What the modern command line builds (DESIGN.md, "Command line and
//! terminal"): the user's `phitex.toml` (`$XDG_CONFIG_HOME/phitex/`, else
//! `~/.config/phitex/`) under the project's `phitex.toml` if there is one,
//! the file named on the command line, with the `% !TEX program` and `% !TEX root` magic
//! comments its first lines may hold.
//!
//! Both files are TOML (`toml`, `serde`); the project's settings win key by
//! key over the user's ([`Config::over`]).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Deserializer};

/// The settings of a build.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Config {
    /// The main file.
    #[serde(alias = "file")]
    pub main: Option<String>,
    /// The engine, as a program name (`pdflatex`, `pdftex`, `latex`,
    /// `tex`).
    #[serde(alias = "program")]
    pub engine: Option<String>,
    /// Where the outputs go (`-output-directory`).
    #[serde(alias = "output-directory", alias = "out-dir")]
    pub output_dir: Option<String>,
    pub shell_escape: Option<bool>,
    /// A program to show the PDF with (`phitex watch --open`).
    pub viewer: Option<String>,
    /// The editor a double-click on the viewer's page opens (`--editor`).
    pub editor: Option<String>,
    /// Where to copy the final PDF after each successful build (`copy-pdf
    /// = true | "dir"`).
    #[serde(default, deserialize_with = "copy_pdf")]
    pub copy_pdf: Option<CopyPdf>,
    /// Whether `phitex build` and `phitex watch` write `<job>.synctex.gz`
    /// (off by default until PhiTeX writes SyncTeX; `synctex = true`
    /// asks for it, and the build notes that it is not written yet).
    pub synctex: Option<bool>,
    /// The cache's size limit in bytes (`cache-limit = "4GB"`; 0: no cache).
    #[serde(default, deserialize_with = "size")]
    pub cache_limit: Option<u64>,
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

/// `copy-pdf = true | false | "dir"`.
fn copy_pdf<'de, D: Deserializer<'de>>(d: D) -> Result<Option<CopyPdf>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        Bool(bool),
        Dir(String),
    }
    Ok(Some(match Raw::deserialize(d)? {
        Raw::Bool(true) => CopyPdf::Invocation,
        Raw::Bool(false) => CopyPdf::Off,
        Raw::Dir(s) => CopyPdf::Dir(s),
    }))
}

/// `cache-limit = "4GB" | bytes`.
fn size<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        Bytes(u64),
        Text(String),
    }
    match Raw::deserialize(d)? {
        Raw::Bytes(n) => Ok(Some(n)),
        Raw::Text(t) => parse_size(&t).map(Some).ok_or_else(|| {
            serde::de::Error::custom(format!(
                "`{t}` is not a size (\"500MB\", \"4GB\", \"2GiB\", bytes, or 0: no cache)"
            ))
        }),
    }
}

/// Parse `phitex.toml`'s `text`.
pub fn parse(text: &str) -> Result<Config, String> {
    parse_named(text, FILE)
}

/// Parse `text`, its errors naming it `name`.
fn parse_named(text: &str, name: &str) -> Result<Config, String> {
    toml::from_str(text).map_err(|e| format!("{name}: {}", e.to_string().trim_end()))
}

/// A size: bytes, or a number with a unit (`500MB`, `4 GB`, `2GiB`).
#[must_use]
pub fn parse_size(s: &str) -> Option<u64> {
    s.trim()
        .parse::<bytesize::ByteSize>()
        .ok()
        .map(|b| b.as_u64())
}

/// The user's settings file: `$XDG_CONFIG_HOME/phitex/phitex.toml`, else
/// `~/.config/phitex/phitex.toml`.
#[must_use]
pub fn user_file() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .filter(|h| !h.is_empty())
                .map(|h| PathBuf::from(h).join(".config"))
        })?;
    Some(base.join("phitex").join(FILE))
}

/// The user's settings (none there: the defaults). They name no main
/// file: that is the project's.
pub fn user() -> Result<Config, String> {
    let Some(p) = user_file() else {
        return Ok(Config::default());
    };
    let Ok(text) = std::fs::read_to_string(&p) else {
        return Ok(Config::default());
    };
    let c = parse_named(&text, &p.display().to_string())?;
    if c.main.is_some() {
        return Err(format!(
            "{}: `main` belongs in a project's phitex.toml, not the user's",
            p.display()
        ));
    }
    Ok(c)
}

impl Config {
    /// These settings over `under`'s: each one set here wins.
    #[must_use]
    pub fn over(self, under: Config) -> Config {
        Config {
            main: self.main.or(under.main),
            engine: self.engine.or(under.engine),
            output_dir: self.output_dir.or(under.output_dir),
            shell_escape: self.shell_escape.or(under.shell_escape),
            viewer: self.viewer.or(under.viewer),
            editor: self.editor.or(under.editor),
            copy_pdf: self.copy_pdf.or(under.copy_pdf),
            synctex: self.synctex.or(under.synctex),
            cache_limit: self.cache_limit.or(under.cache_limit),
        }
    }
}

/// `phitex.toml` in `dir` or the nearest directory above it: the
/// directory it is in, and its settings.
pub fn find(dir: &Path) -> Result<Option<(PathBuf, Config)>, String> {
    for d in dir.ancestors() {
        if let Ok(text) = std::fs::read_to_string(d.join(FILE)) {
            return parse(&text).map(|c| Some((d.to_path_buf(), c)));
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

/// The files `text` inputs by name in braces (`\input{name}`,
/// `\include{name}`), in order.
#[must_use]
pub fn braced_inputs(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    for cmd in ["\\input{", "\\include{"] {
        let mut rest = text;
        while let Some(i) = rest.find(cmd) {
            rest = &rest[i + cmd.len()..];
            if let Some(j) = rest.find('}') {
                let name = rest[..j].trim();
                if !name.is_empty() && !name.contains(['\\', '{', '\n']) {
                    out.push(name);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toml() {
        let c = parse(
            "# my paper\nmain = \"paper.tex\"  # the root\nengine = 'pdflatex'\n\
             output-dir = \"out\"\nshell-escape = false\nviewer = \"zathura\"\neditor = \"code\"\n",
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
                editor: Some("code".into()),
                copy_pdf: None,
                synctex: None,
                cache_limit: None,
            }
        );
        assert_eq!(parse("synctex = false").unwrap().synctex, Some(false));
        assert_eq!(
            parse("cache-limit = \"500MB\"").unwrap().cache_limit,
            Some(500_000_000)
        );
        assert_eq!(parse("cache-limit = 0").unwrap().cache_limit, Some(0));
        assert!(parse("cache-limit = \"lots\"").is_err());
        let user = parse("engine = \"tex\"\nsynctex = false").unwrap();
        let project = parse("engine = \"pdflatex\"").unwrap();
        let c = project.over(user);
        assert_eq!(c.engine.as_deref(), Some("pdflatex"));
        assert_eq!(c.synctex, Some(false));
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
        let e = parse("mian = \"x\"").unwrap_err();
        assert!(e.starts_with("phitex.toml: ") && e.contains("mian"), "{e}");
        assert_eq!(
            parse("cache-limit = \"2GiB\"").unwrap().cache_limit,
            Some(2 << 30)
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
        assert_eq!(braced_inputs("\\def\\x{}\n\\input{course}"), vec!["course"]);
        // the class in a shared preamble
        assert_eq!(
            default_engine("\\input{preamble}\n\\begin{document}\nx\n\\end{document}"),
            "pdflatex"
        );
    }
}
