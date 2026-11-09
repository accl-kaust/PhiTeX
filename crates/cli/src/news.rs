//! What is new (docs/spec-distribution.md §4): CHANGELOG.md's entry for
//! this version, built into the binary. The first run of a version in a
//! terminal says the entry's summary once; `phitex news` prints the
//! whole entry. No network: it comes with the binary, however installed.
//!
//! An entry is a `## <version> …` heading; its first paragraph is the
//! summary, one or two sentences for the people who just upgraded.

use std::io::IsTerminal as _;
use std::path::PathBuf;

use crate::ui::{Style, Ui};

const CHANGELOG: &str = include_str!("../../../CHANGELOG.md");

/// `text`'s entry for version `v`: the lines under its `## v` heading,
/// up to the next `## `.
fn entry<'a>(text: &'a str, v: &str) -> Option<&'a str> {
    let mut at = None;
    let mut off = 0;
    for l in text.split_inclusive('\n') {
        if let Some(h) = l.strip_prefix("## ") {
            if let Some(a) = at {
                return Some(text[a..off].trim());
            }
            if h.split_whitespace().next() == Some(v) {
                at = Some(off + l.len());
            }
        }
        off += l.len();
    }
    at.map(|a| text[a..].trim())
}

/// An entry's summary: its first paragraph, on one line.
fn summary(entry: &str) -> String {
    entry
        .lines()
        .take_while(|l| !l.trim().is_empty())
        .map(str::trim)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Where the last version whose news was shown is kept:
/// `$XDG_STATE_HOME/phitex/news`, else `~/.local/state/phitex/news`.
fn seen_file() -> Option<PathBuf> {
    let env = |k| std::env::var_os(k).filter(|d| !d.is_empty());
    let d = env("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| env("HOME").map(|h| PathBuf::from(h).join(".local").join("state")))?;
    Some(d.join("phitex").join("news"))
}

/// This version's entry, else the newest one (a nightly's version has
/// none): its version and text, for the viewer's What's new.
pub fn latest() -> Option<(String, String)> {
    let v = crate::version();
    if let Some(e) = entry(CHANGELOG, v) {
        return Some((v.to_owned(), e.to_owned()));
    }
    let h = CHANGELOG.lines().find_map(|l| l.strip_prefix("## "))?;
    let v2 = h.split_whitespace().next()?;
    entry(CHANGELOG, v2).map(|e| (v2.to_owned(), e.to_owned()))
}

/// `phitex news`: this version's entry, whole, its Markdown drawn for
/// the terminal (colours when standard output is one, as [`Style`] says
/// for standard error).
pub fn print() {
    let v = crate::version();
    let out = std::io::stdout().is_terminal();
    let set = |k: &str| std::env::var_os(k).is_some_and(|x| !x.is_empty() && x != "0");
    let st = Style {
        color: set("CLICOLOR_FORCE") || (out && std::env::var_os("NO_COLOR").is_none()),
        tty: out,
    };
    let width = std::env::var("COLUMNS")
        .ok()
        .and_then(|c| c.parse::<usize>().ok())
        .filter(|&c| c >= 20)
        .unwrap_or(80)
        .min(100);
    match entry(CHANGELOG, v) {
        Some(e) => println!(
            "{}\n\n{}",
            st.bold(&format!("phitex {v}")),
            render(e, st, width)
        ),
        None => println!("phitex {v}: no news for this version"),
    }
}

/// An entry's Markdown drawn for a terminal: headings bold, list items
/// bulleted, paragraphs and items joined and wrapped to `width`, and
/// inline marks ([`inline`]) styled. Fenced code kept as it is, indented.
fn render(md: &str, st: Style, width: usize) -> String {
    let mut out: Vec<String> = Vec::new();
    // (the block being gathered: its first line's prefix, the next lines',
    // and its words)
    let mut block: Option<(String, String, String)> = None;
    let mut fenced = false;
    let flush = |out: &mut Vec<String>, b: &mut Option<(String, String, String)>| {
        if let Some((first, rest, text)) = b.take() {
            out.extend(wrap(&inline(&text, st), &first, &rest, width));
        }
    };
    for l in md.lines() {
        let t = l.trim_end();
        if t.trim_start().starts_with("```") {
            flush(&mut out, &mut block);
            fenced = !fenced;
            continue;
        }
        if fenced {
            out.push(format!("    {t}"));
            continue;
        }
        let lead = t.len() - t.trim_start().len();
        let u = t.trim_start();
        if u.is_empty() {
            flush(&mut out, &mut block);
            if out.last().is_some_and(|x| !x.is_empty()) {
                out.push(String::new());
            }
        } else if let Some(h) = u.strip_prefix('#').map(|h| h.trim_start_matches('#')) {
            flush(&mut out, &mut block);
            out.push(st.bold(&inline(h.trim(), Style { color: false, ..st })));
        } else if let Some(item) = u.strip_prefix("- ").or_else(|| u.strip_prefix("* ")) {
            flush(&mut out, &mut block);
            let pad = " ".repeat(lead);
            let bullet = st.sym("•", "-");
            block = Some((
                format!("{pad}{bullet} "),
                format!("{pad}  "),
                item.to_owned(),
            ));
        } else if let Some((_, _, text)) = block.as_mut() {
            text.push(' ');
            text.push_str(u);
        } else {
            block = Some((String::new(), String::new(), u.to_owned()));
        }
    }
    flush(&mut out, &mut block);
    while out.last().is_some_and(String::is_empty) {
        out.pop();
    }
    out.join("\n")
}

/// Markdown's inline marks in `s`: `code` cyan, **strong** bold, a link
/// `[text](url)` as "text (url)"; without colours the marks just go.
fn inline(s: &str, st: Style) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find(['`', '*', '[']) {
        out.push_str(&rest[..i]);
        let r = &rest[i..];
        let done = if let Some(c) = r.strip_prefix('`') {
            c.find('`').map(|j| (st.cyan(&c[..j]), 1 + j + 1))
        } else if let Some(c) = r.strip_prefix("**") {
            c.find("**").map(|j| (st.bold(&c[..j]), 2 + j + 2))
        } else if let Some(c) = r.strip_prefix('[') {
            c.find("](").and_then(|j| {
                let u = &c[j + 2..];
                u.find(')')
                    .map(|k| (format!("{} ({})", &c[..j], &u[..k]), 1 + j + 2 + k + 1))
            })
        } else {
            None
        };
        if let Some((text, used)) = done {
            out.push_str(&text);
            rest = &r[used..];
        } else {
            out.push_str(&r[..1]);
            rest = &r[1..];
        }
    }
    out.push_str(rest);
    out
}

/// `text` (styled: its escapes take no columns) wrapped to `width`, its
/// first line after `first`, the others after `rest`.
fn wrap(text: &str, first: &str, rest: &str, width: usize) -> Vec<String> {
    let cols = |w: &str| {
        let mut n = 0;
        let mut esc = false;
        for c in w.chars() {
            match c {
                '\x1b' => esc = true,
                'm' if esc => esc = false,
                _ if esc => {}
                _ => n += 1,
            }
        }
        n
    };
    let mut lines = Vec::new();
    let mut cur = first.to_owned();
    let mut used = cols(first);
    let mut empty = true;
    for w in text.split_whitespace() {
        let n = cols(w);
        if !empty && used + 1 + n > width {
            lines.push(std::mem::replace(&mut cur, rest.to_owned()));
            used = cols(rest);
            empty = true;
        }
        if !empty {
            cur.push(' ');
            used += 1;
        }
        cur.push_str(w);
        used += n;
        empty = false;
    }
    lines.push(cur);
    lines
}

/// The first run of this version in a terminal: its summary, once (not
/// under CI, `PHITEX_NO_NEWS`, or for a version without an entry).
pub fn once(ui: &Ui) {
    let v = crate::version();
    if !std::io::stderr().is_terminal()
        || std::env::var_os("CI").is_some()
        || std::env::var_os("PHITEX_NO_NEWS").is_some()
    {
        return;
    }
    let (Some(e), Some(f)) = (entry(CHANGELOG, v), seen_file()) else {
        return;
    };
    if std::fs::read_to_string(&f).is_ok_and(|s| s.trim() == v) {
        return;
    }
    // (said only once it is kept as said: an unwritable state directory
    // would say it every run)
    let kept = f
        .parent()
        .is_some_and(|d| std::fs::create_dir_all(d).is_ok())
        && std::fs::write(&f, format!("{v}\n")).is_ok();
    if kept {
        let st = ui.st;
        ui.line(&format!(
            "{} {} {}",
            st.cyan(&format!("phitex {v}:")),
            inline(&summary(e), st),
            st.dim("(phitex news for more)")
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOG: &str = "# Changelog\n\nIntro.\n\n## 1.61 (2027-01-02)\nFaster.\nAnd more.\n\n- one\n\n## 1.6 (2026-11-02)\nFirst.\n\n### Detail\n- two\n";

    #[test]
    fn entries_by_version() {
        assert_eq!(entry(LOG, "1.61"), Some("Faster.\nAnd more.\n\n- one"));
        assert_eq!(entry(LOG, "1.6"), Some("First.\n\n### Detail\n- two"));
        assert_eq!(entry(LOG, "1.618"), None);
        assert_eq!(summary(entry(LOG, "1.61").unwrap()), "Faster. And more.");
    }

    #[test]
    fn markdown_drawn_plain() {
        let plain = Style {
            color: false,
            tty: false,
        };
        let md = "Builds **twice** as fast with `-j 4`.\nSee [the docs](https://phitex.org).\n\n### What works\n\n- one item that is\n  long\n- `two`\n\n```\nphitex build\n```";
        assert_eq!(
            render(md, plain, 80),
            "Builds twice as fast with -j 4. See the docs (https://phitex.org).\n\nWhat works\n\n- one item that is long\n- two\n\n    phitex build"
        );
        assert_eq!(render("- a b c d e f", plain, 8), "- a b c\n  d e f");
        // (a lone mark is kept as it is)
        assert_eq!(inline("2 * 3 and `x", plain), "2 * 3 and `x");
    }

    #[test]
    fn markdown_drawn_in_colour() {
        let tty = Style {
            color: true,
            tty: true,
        };
        assert_eq!(
            render("## New\n\nRun `phitex`.", tty, 80),
            "\x1b[1mNew\x1b[0m\n\nRun \x1b[36mphitex\x1b[0m."
        );
        // (escapes take no columns: the line is not broken early)
        assert_eq!(render("`ab` cd", tty, 5), "\x1b[36mab\x1b[0m cd");
    }
}
