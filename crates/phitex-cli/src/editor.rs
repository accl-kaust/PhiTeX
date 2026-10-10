//! The editor a double-click on the live viewer's page opens (DESIGN 4.8):
//! `--editor`, else `PHITEX_EDITOR`, else `VISUAL`, else `EDITOR`. A
//! command with `{file}`, `{line}` and `{col}` (1-based) in it is run as
//! it is written; a known editor's name alone gets its preset. A terminal
//! editor (`vi`, `nano`) cannot open beside a watch that holds the
//! terminal: the place is only shown.

use std::process::{Command, Stdio};

/// An editor command: its words, placeholders in them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Editor {
    words: Vec<String>,
}

/// The preset of an editor named `name` (its program's base name).
fn preset(name: &str) -> Option<&'static str> {
    Some(match name {
        "code" | "code-insiders" | "codium" | "vscodium" | "cursor" => {
            "{cmd} -g {file}:{line}:{col}"
        }
        "subl" | "sublime_text" | "zed" => "{cmd} {file}:{line}:{col}",
        "emacsclient" => "{cmd} -n +{line}:{col} {file}",
        "emacs" => "emacsclient -n +{line}:{col} {file}",
        // (Neovim's server: `$NVIM` inside its terminal, else
        // `PHITEX_NVIM_SERVER`)
        "nvim" => {
            "{cmd} --server {nvim} --remote-send '<C-\\><C-N>:edit +{line} {vimfile}<CR>{col}|'"
        }
        "gvim" | "mvim" => "{cmd} --remote-silent +{line} {file}",
        "kate" | "idea" | "pycharm" | "clion" | "webstorm" => {
            "{cmd} --line {line} --column {col} {file}"
        }
        "gedit" | "gnome-text-editor" => "{cmd} +{line}:{col} {file}",
        _ => return None,
    })
}

/// Split `s` into words as a shell would for plain words, quotes and
/// backslashes (no expansions).
fn words(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut any = false;
    let mut quote = None;
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (None, c) if c.is_whitespace() => {
                if any {
                    out.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            (None, '\'' | '"') => {
                quote = Some(c);
                any = true;
            }
            (Some(q), c) if c == q => quote = None,
            (None | Some('"'), '\\') => {
                if let Some(n) = chars.next() {
                    cur.push(n);
                    any = true;
                }
            }
            (_, c) => {
                cur.push(c);
                any = true;
            }
        }
    }
    if any {
        out.push(cur);
    }
    out
}

/// `file` as Vim's `fnameescape()` makes it: spaces and the characters
/// its command line reads otherwise escaped.
fn vim_escape(file: &str) -> String {
    let mut out = String::new();
    for c in file.chars() {
        if " \t\n*?[{`$\\%#'\"|!<".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

impl Editor {
    /// The editor `spec` names (a command with placeholders, or a known
    /// editor's command), if it can be run from the viewer.
    #[must_use]
    pub fn parse(spec: &str) -> Option<Editor> {
        let w = words(spec);
        let cmd = w.first()?;
        if spec.contains("{file}") || spec.contains("{line}") {
            return Some(Editor { words: w });
        }
        let name = std::path::Path::new(cmd)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(cmd);
        let p = preset(name)?;
        // (the editor's own options before the place: `code --reuse-window`)
        let mut out = Vec::new();
        for t in words(p) {
            if t == "{cmd}" {
                out.extend(w.iter().cloned());
            } else {
                out.push(t);
            }
        }
        Some(Editor { words: out })
    }

    /// The configured editor: `flag` (`--editor`), else `PHITEX_EDITOR`,
    /// `VISUAL` or `EDITOR`. `Err` names the setting that cannot be used.
    pub fn configured(flag: Option<&str>) -> Result<Option<Editor>, String> {
        let from = |name: &str| std::env::var(name).ok().filter(|v| !v.trim().is_empty());
        let spec = flag
            .map(str::to_owned)
            .or_else(|| from("PHITEX_EDITOR"))
            .or_else(|| from("VISUAL"))
            .or_else(|| from("EDITOR"));
        match spec {
            None => Ok(None),
            Some(s) => Editor::parse(&s).map(Some).ok_or(s),
        }
    }

    /// The command for `file` at `line` and `col` (1-based).
    #[must_use]
    pub fn command(&self, file: &str, line: usize, col: usize) -> Vec<String> {
        let nvim = std::env::var("NVIM")
            .or_else(|_| std::env::var("PHITEX_NVIM_SERVER"))
            .unwrap_or_default();
        self.words
            .iter()
            .map(|w| {
                w.replace("{file}", file)
                    .replace("{line}", &line.to_string())
                    .replace("{col}", &col.to_string())
                    .replace("{nvim}", &nvim)
                    .replace("{vimfile}", &vim_escape(file))
            })
            .collect()
    }

    /// Open `file` at `line`, `col` (in the background; the process is
    /// waited for on a thread of its own). The error if it did not start.
    pub fn open(&self, file: &str, line: usize, col: usize) -> Result<(), String> {
        let argv = self.command(file, line, col);
        let (program, rest) = argv.split_first().ok_or("no command")?;
        let mut child = Command::new(program)
            .args(rest)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("{program}: {e}"))?;
        std::thread::spawn(move || child.wait());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(spec: &str) -> Option<Vec<String>> {
        Editor::parse(spec).map(|e| e.command("ch 1.tex", 12, 5))
    }

    #[test]
    fn presets_and_templates() {
        assert_eq!(cmd("code").unwrap(), ["code", "-g", "ch 1.tex:12:5"]);
        assert_eq!(
            cmd("/usr/bin/code --reuse-window").unwrap(),
            ["/usr/bin/code", "--reuse-window", "-g", "ch 1.tex:12:5"]
        );
        assert_eq!(
            cmd("emacsclient").unwrap(),
            ["emacsclient", "-n", "+12:5", "ch 1.tex"]
        );
        assert_eq!(
            cmd("my-ed --at '{line} {col}' {file}").unwrap(),
            ["my-ed", "--at", "12 5", "ch 1.tex"]
        );
        // (a terminal editor: shown, not opened)
        assert_eq!(cmd("vim"), None);
        assert_eq!(cmd("nano -w"), None);
        assert_eq!(cmd(""), None);
    }

    #[test]
    fn nvim_remote() {
        let c = cmd("nvim").unwrap();
        assert_eq!(c[..3], ["nvim", "--server", c[2].as_str()]);
        assert_eq!(c[3], "--remote-send");
        assert_eq!(c[4], "<C-\\><C-N>:edit +12 ch\\ 1.tex<CR>5|");
    }
}
