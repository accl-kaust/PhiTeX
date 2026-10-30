//! What the modern command line builds (DESIGN.md, "Command line and
//! terminal"): the user's `phitex.toml` (`$XDG_CONFIG_HOME/phitex/`, else
//! `~/.config/phitex/`) under the project's `phitex.toml` if there is one,
//! the file named on the command line, with the `% !TEX program` and `% !TEX root` magic
//! comments its first lines may hold.
//!
//! Both files are TOML (`toml`, `serde`); the project's settings win key by
//! key over the user's ([`Config::over`]).
//!
//! A `phitex.toml` is shared between people on different PhiTeX versions
//! (docs/spec-distribution.md §5): a key this PhiTeX does not know is a
//! warning, never an error (a newer PhiTeX's, or a typo: the nearest key
//! named), and `phitex = ">=X.Y.Z"` states the oldest PhiTeX a project
//! needs, an older one stopping with what it needs.

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
    /// (off by default; `synctex = true` asks for it).
    pub synctex: Option<bool>,
    /// The cache's size limit in bytes (`cache-limit = "4GB"`; 0: no cache).
    #[serde(default, deserialize_with = "size")]
    pub cache_limit: Option<u64>,
    /// Where TeX's files come from: `texlive` (kpathsea, the default) or
    /// `shelf` (Shelf's packs, fetched and kept: `crate::shelf`).
    pub files: Option<String>,
    /// The oldest PhiTeX the project needs (`phitex = ">=0.2"`): checked
    /// when read.
    pub phitex: Option<String>,
    /// The opt-in update check (`crate::upgrade::check`), read from the
    /// user's file only: a project's never turns it on.
    pub update_check: Option<bool>,
    /// What reading the files warned of (unknown keys).
    #[serde(skip)]
    pub warnings: Vec<String>,
}

/// The keys `phitex.toml` takes, with their aliases.
const KEYS: &[&str] = &[
    "main",
    "file",
    "engine",
    "program",
    "output-dir",
    "output-directory",
    "out-dir",
    "shell-escape",
    "viewer",
    "editor",
    "copy-pdf",
    "synctex",
    "cache-limit",
    "files",
    "phitex",
    "update-check",
];

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

/// Parse `text`, its errors and warnings naming it `name`: a key not in
/// [`KEYS`] is left out with a warning; `phitex`'s requirement checked.
fn parse_named(text: &str, name: &str) -> Result<Config, String> {
    let said = |e: &dyn std::fmt::Display| format!("{name}: {}", e.to_string().trim_end());
    let mut table: toml::Table = toml::from_str(text).map_err(|e| said(&e))?;
    let unknown: Vec<String> = table
        .keys()
        .filter(|k| !KEYS.contains(&k.as_str()))
        .cloned()
        .collect();
    let mut warnings = Vec::new();
    for k in unknown {
        table.remove(&k);
        warnings.push(match nearest(&k) {
            Some(n) => format!("{name}: unknown key `{k}` (did you mean `{n}`?), ignored"),
            None => format!("{name}: unknown key `{k}` (a newer PhiTeX's?), ignored"),
        });
    }
    let mut c: Config = table.try_into().map_err(|e| said(&e))?;
    if let Some(req) = &c.phitex {
        needs(req, env!("CARGO_PKG_VERSION")).map_err(|e| format!("{name}: {e}"))?;
    }
    c.warnings = warnings;
    Ok(c)
}

/// The key of [`KEYS`] nearest `k` (two edits at most), if any.
fn nearest(k: &str) -> Option<&'static str> {
    let dist = |a: &str, b: &str| {
        let b: Vec<char> = b.chars().collect();
        let mut row: Vec<usize> = (0..=b.len()).collect();
        for (i, ca) in a.chars().enumerate() {
            let mut prev = row[0];
            row[0] = i + 1;
            for (j, cb) in b.iter().enumerate() {
                let here = row[j + 1];
                row[j + 1] = (prev + usize::from(ca != *cb))
                    .min(row[j] + 1)
                    .min(here + 1);
                prev = here;
            }
        }
        row[b.len()]
    };
    KEYS.iter()
        .map(|&n| (dist(k, n), n))
        .filter(|&(d, _)| d <= 2)
        .min()
        .map(|(_, n)| n)
}

/// Whether PhiTeX `have` meets the requirement `req` (`>=X`, `>=X.Y` or
/// `>=X.Y.Z`): why not, if not.
fn needs(req: &str, have: &str) -> Result<(), String> {
    let nums = |s: &str| -> Option<Vec<u64>> {
        let v: Option<Vec<u64>> = s.trim().split('.').map(|p| p.parse().ok()).collect();
        v.filter(|v| (1..=3).contains(&v.len()))
    };
    let Some(want) = req.trim().strip_prefix(">=").and_then(nums) else {
        return Err(format!(
            "`phitex = \"{req}\"`: it takes \">=X.Y.Z\", the oldest PhiTeX the project needs"
        ));
    };
    let have_v = nums(have.split(['-', '+']).next().unwrap_or(have)).unwrap_or_default();
    let at = |v: &[u64], i: usize| v.get(i).copied().unwrap_or(0);
    let older = (0..3)
        .map(|i| at(&have_v, i).cmp(&at(&want, i)))
        .find(|o| o.is_ne())
        .is_some_and(std::cmp::Ordering::is_lt);
    if older {
        return Err(format!(
            "this project needs PhiTeX {} (this is {have}): install a newer one",
            req.trim()
        ));
    }
    Ok(())
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
            files: self.files.or(under.files),
            phitex: self.phitex.or(under.phitex),
            update_check: self.update_check.or(under.update_check),
            warnings: [self.warnings, under.warnings].concat(),
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

/// The packages that run only under XeTeX or LuaTeX, and the engine each
/// asks for (fontspec and unicode-math run under both: XeTeX's is the one
/// PhiTeX runs). LuaTeX's come first: a document with luacode and fontspec
/// needs LuaTeX.
const NATIVE_PACKAGES: [(&str, &str); 14] = [
    ("luacode", "luatex"),
    ("luatexja", "luatex"),
    ("luatexja-fontspec", "luatex"),
    ("luaotfload", "luatex"),
    ("luamplib", "luatex"),
    ("fontspec", "xetex"),
    ("unicode-math", "xetex"),
    ("polyglossia", "xetex"),
    ("xeCJK", "xetex"),
    ("xltxtra", "xetex"),
    ("xunicode", "xetex"),
    ("mathspec", "xetex"),
    ("xgreek", "xetex"),
    ("bidi", "xetex"),
];

/// The commands that exist only under XeTeX or LuaTeX (fontspec's, in a
/// file a shared preamble or class loads it for).
const NATIVE_COMMANDS: [(&str, &str); 6] = [
    ("\\directlua", "luatex"),
    ("\\setmainfont", "xetex"),
    ("\\setsansfont", "xetex"),
    ("\\setmonofont", "xetex"),
    ("\\newfontfamily", "xetex"),
    ("\\setmathfont", "xetex"),
];

/// The tests a document makes of its engine: one that has them runs under
/// pdfTeX too, on its own branch.
const ENGINE_TESTS: [&str; 9] = [
    "\\ifxetex",
    "\\ifXeTeX",
    "\\ifluatex",
    "\\ifLuaTeX",
    "\\iftutex",
    "\\ifPDFTeX",
    "\\ifpdftex",
    "\\ifxetexorluatex",
    "\\sys_if_engine",
];

/// The engine `text` needs beyond pdfTeX, `xetex` or `luatex`, and why
/// (`loads fontspec`), when nothing names one: a package or command that
/// runs only there, outside comments. None for a file that tests its
/// engine (it has a pdfTeX branch).
#[must_use]
pub fn native_engine(text: &str) -> Option<(&'static str, String)> {
    let text = uncommented(text);
    if ENGINE_TESTS.iter().any(|t| has_command(&text, t)) {
        return None;
    }
    let packages = packages(&text);
    for (name, engine) in NATIVE_PACKAGES {
        if packages.contains(&name) {
            return Some((engine, format!("loads {name}")));
        }
    }
    for (cmd, engine) in NATIVE_COMMANDS {
        if has_command(&text, cmd) {
            return Some((engine, format!("uses {cmd}")));
        }
    }
    // (XeTeX's primitives: \XeTeXinputencoding, not the logo \XeTeX)
    let mut rest = text.as_str();
    while let Some(i) = rest.find("\\XeTeX") {
        rest = &rest[i + 6..];
        if rest.starts_with(|c: char| c.is_ascii_lowercase()) {
            let name: String = rest.chars().take_while(char::is_ascii_alphabetic).collect();
            return Some(("xetex", format!("uses \\XeTeX{name}")));
        }
    }
    None
}

/// The program name for a document of `native`'s engine (`xetex`,
/// `luatex`), LaTeX or not.
#[must_use]
pub fn native_program(native: &str, latex: bool) -> &'static str {
    match (native, latex) {
        ("luatex", true) => "lualatex",
        ("luatex", false) => "luatex",
        (_, true) => "xelatex",
        (_, false) => "xetex",
    }
}

/// `text` without its comments (from an unescaped `%` to the line's end).
fn uncommented(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        let mut escaped = false;
        let mut end = line.len();
        for (i, c) in line.char_indices() {
            if c == '%' && !escaped {
                end = i;
                break;
            }
            escaped = c == '\\' && !escaped;
        }
        out.push_str(&line[..end]);
        out.push('\n');
    }
    out
}

/// Whether `text` has the control sequence `cmd` (not a longer name it
/// begins).
fn has_command(text: &str, cmd: &str) -> bool {
    text.match_indices(cmd)
        .any(|(i, _)| !text[i + cmd.len()..].starts_with(|c: char| c.is_ascii_alphabetic()))
}

/// The classes and packages `text` loads by name (`\documentclass`,
/// `\LoadClass`, `\usepackage`, `\RequirePackage`, options skipped,
/// lists split), in order.
#[must_use]
pub fn packages(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    for cmd in [
        "\\documentclass",
        "\\LoadClass",
        "\\usepackage",
        "\\RequirePackage",
    ] {
        for (i, _) in text.match_indices(cmd) {
            let mut rest = text[i + cmd.len()..].trim_start();
            if rest.starts_with(|c: char| c.is_ascii_alphabetic()) {
                continue;
            }
            if let Some(r) = rest.strip_prefix('[') {
                let Some(j) = r.find(']') else { continue };
                rest = r[j + 1..].trim_start();
            }
            let Some(r) = rest.strip_prefix('{') else {
                continue;
            };
            let Some(j) = r.find('}') else { continue };
            out.extend(r[..j].split(',').map(str::trim).filter(|n| !n.is_empty()));
        }
    }
    out
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
                files: None,
                phitex: None,
                update_check: None,
                warnings: vec![],
            }
        );
        assert_eq!(parse("synctex = false").unwrap().synctex, Some(false));
        assert_eq!(
            parse("files = \"shelf\"").unwrap().files.as_deref(),
            Some("shelf")
        );
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
        // (a key this PhiTeX does not know: a warning, the nearest key
        // named; the file read: docs/spec-distribution.md §5)
        let c = parse("mian = \"x\"\nengine = \"tex\"").unwrap();
        assert_eq!(c.engine.as_deref(), Some("tex"));
        assert_eq!(c.warnings.len(), 1);
        assert!(
            c.warnings[0].starts_with("phitex.toml: ")
                && c.warnings[0].contains("did you mean `main`"),
            "{:?}",
            c.warnings
        );
        let c = parse("shelf-mirror-of-2030 = true").unwrap();
        assert!(
            c.warnings[0].contains("a newer PhiTeX's"),
            "{:?}",
            c.warnings
        );
        // (the oldest PhiTeX a project needs)
        assert_eq!(needs(">=0.1", "0.1.0"), Ok(()));
        assert_eq!(needs(">= 0.1.2", "0.2.0"), Ok(()));
        assert!(
            needs(">=0.2", "0.1.9")
                .unwrap_err()
                .contains("needs PhiTeX >=0.2 (this is 0.1.9)")
        );
        assert!(needs(">=1", "0.9.0-alpha").is_err());
        assert!(needs("0.2", "0.1.0").unwrap_err().contains("takes"));
        assert!(
            parse("phitex = \">=999\"")
                .unwrap_err()
                .contains("needs PhiTeX")
        );
        assert!(parse("phitex = \">=0\"").is_ok());
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

    #[test]
    fn native() {
        let e = |t: &str| native_engine(t).map(|(e, why)| format!("{e}: {why}"));
        assert_eq!(e("\\documentclass{article}\n\\usepackage{amsmath}"), None);
        assert_eq!(
            e("\\documentclass{article}\n\\usepackage[no-math]{fontspec}"),
            Some("xetex: loads fontspec".into())
        );
        assert_eq!(
            e("\\usepackage{amsmath, unicode-math}"),
            Some("xetex: loads unicode-math".into())
        );
        assert_eq!(
            e("\\usepackage{fontspec}\\usepackage{luacode}"),
            Some("luatex: loads luacode".into())
        );
        // commented out, a percent escaped, an engine test
        assert_eq!(e("% \\usepackage{fontspec}\n\\usepackage{xcolor}"), None);
        assert_eq!(
            e("50\\% \\setmainfont{Libertinus Serif}"),
            Some("xetex: uses \\setmainfont".into())
        );
        assert_eq!(
            e("\\ifxetex\\usepackage{fontspec}\\else\\usepackage[T1]{fontenc}\\fi"),
            None
        );
        // the logo is not the primitive; a longer name is not the command
        assert_eq!(e("\\XeTeX{} and \\XeLaTeX"), None);
        assert_eq!(
            e("\\XeTeXinputencoding \"bytes\""),
            Some("xetex: uses \\XeTeXinputencoding".into())
        );
        assert_eq!(e("\\directluax"), None);
        assert_eq!(native_program("xetex", true), "xelatex");
        assert_eq!(native_program("luatex", false), "luatex");
        assert_eq!(
            packages("\\documentclass[11pt]{mythesis}\\usepackage{a,b}"),
            vec!["mythesis", "a", "b"]
        );
    }
}
