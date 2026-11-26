//! `phitex doctor`: PhiTeX's setup on this machine, for any editor (the
//! Emacs and Neovim plugins have their own, M-x phitex-doctor and
//! :PhiTeXDoctor, which add the editor's side) and for a bug report: the
//! program and its version, the editor protocol, how it was installed,
//! where TeX's files come from, the cache, the settings' files, whether a
//! watch opens the browser here, and the running watches. It names files
//! and folders, never a document's text, so it can be pasted into an
//! issue whole.

use serde_json::{Value, json};

/// Where a problem is reported, said at the report's end.
pub const ISSUES: &str = "https://github.com/accl-kaust/PhiTeX/issues/new/choose";

/// The report's lines: a title, then `(ok, text)` findings under their
/// sections (`ok` none: a fact, neither good nor bad).
type Section = (&'static str, Vec<(Option<bool>, String)>);

/// What the report finds, as sections, and as JSON (`--json`).
fn findings() -> (Vec<Section>, Value) {
    let exe = std::env::current_exe().ok();
    let record = crate::upgrade::install_record();
    let channel = crate::config::user()
        .ok()
        .and_then(|c| c.update_channel)
        .unwrap_or_else(|| crate::upgrade::installed_channel().to_owned());
    let mut program = vec![
        (
            None,
            format!(
                "phitex {} at {}",
                crate::version(),
                exe.as_ref()
                    .map_or_else(|| "?".to_owned(), |e| e.display().to_string())
            ),
        ),
        (
            None,
            format!("editor protocol {}", phitex_live::lsp::PROTOCOL),
        ),
        (
            None,
            format!(
                "installed by {}, updates from the {channel} channel (phitex upgrade)",
                record
                    .as_ref()
                    .and_then(|r| r["method"].as_str().map(str::to_owned))
                    .map_or_else(
                        || "hand (no install record)".to_owned(),
                        |m| {
                            if m == "script" {
                                "install.sh".to_owned()
                            } else {
                                m
                            }
                        }
                    )
            ),
        ),
    ];
    if crate::source_build() {
        program.push((None, "a source build".to_owned()));
    }
    // (TeX's files: where they come from, and whether they are there)
    let here = std::env::current_dir().ok();
    let project = here
        .as_deref()
        .and_then(|d| crate::config::find(d).ok().flatten());
    let user = crate::config::user().ok();
    let files = project
        .as_ref()
        .and_then(|(_, c)| c.files.clone())
        .or_else(|| user.as_ref().and_then(|u| u.files.clone()))
        .unwrap_or_else(|| "texlive".to_owned());
    let texlive = crate::tex::has_texlive();
    let tex = vec![if files == "shelf" {
        (
            Some(true),
            "files = \"shelf\": TeX's files from Shelf's packs, fetched as needed".to_owned(),
        )
    } else if texlive {
        (
            Some(true),
            "TeX Live found (kpathsea finds pdftex's files)".to_owned(),
        )
    } else {
        (
            Some(false),
            "no TeX Live found: phitex config files shelf (PhiTeX fetches TeX's files itself)"
                .to_owned(),
        )
    }];
    // (the cache)
    let cache = match crate::cache::Cache::open(None) {
        Ok(c) => vec![(
            None,
            format!(
                "{}, {} of {}",
                crate::cache::home_relative(&c.dir),
                crate::cache::size(c.usage()),
                crate::cache::size(c.limit)
            ),
        )],
        Err(e) => vec![(Some(false), format!("no cache: {e}"))],
    };
    // (the settings' files)
    let settings = vec![
        (
            None,
            format!(
                "yours: {}",
                crate::config::user_file().map_or_else(
                    || "none (no home folder)".to_owned(),
                    |p| {
                        let shown = crate::cache::home_relative(&p);
                        if p.is_file() {
                            shown
                        } else {
                            format!("{shown} (not written: the defaults)")
                        }
                    }
                )
            ),
        ),
        (
            None,
            format!(
                "the project's: {}",
                project.as_ref().map_or_else(
                    || "none here or above".to_owned(),
                    |(d, _)| crate::cache::home_relative(&d.join(crate::config::FILE))
                )
            ),
        ),
    ];
    // (the viewer: whether a watch started here opens it)
    let var = |k: &str| std::env::var_os(k).is_some_and(|v| !v.is_empty());
    let opens = crate::modern::opens_by_default(&crate::modern::Opening {
        lsp: false,
        json: false,
        terminal: true,
        desktop: cfg!(any(windows, target_os = "macos")),
        var: &var,
    });
    let viewer = vec![(
        None,
        if opens {
            "phitex watch opens the viewer in the browser (--no-open: not)".to_owned()
        } else if var("SSH_CONNECTION") || var("SSH_TTY") {
            "over SSH: phitex watch prints the viewer's address, opened by hand (forward its port)"
                .to_owned()
        } else {
            "no display: phitex watch prints the viewer's address, opened by hand".to_owned()
        },
    )];
    // (the running watches)
    let folders = crate::attach::read_folders();
    let (live, unreachable) = crate::attach::every_watch(&folders);
    let mut watches: Vec<(Option<bool>, String)> = live
        .iter()
        .map(|v| {
            (
                None,
                format!(
                    "{}  pid {}{}",
                    v["main"].as_str().unwrap_or("(starting)"),
                    v["pid"],
                    v["viewer"]
                        .as_str()
                        .map(|u| format!("  viewer {u}"))
                        .unwrap_or_default()
                ),
            )
        })
        .collect();
    watches.extend(unreachable.iter().map(|v| {
        (
            Some(false),
            format!(
                "{}  pid {}: its socket can't be reached from here (another sandbox?)",
                v["main"].as_str().unwrap_or("(starting)"),
                v["pid"]
            ),
        )
    }));
    if watches.is_empty() {
        watches.push((None, "none".to_owned()));
    }
    let json = json!({
        "version": crate::version(),
        "exe": exe.map(|e| e.display().to_string()),
        "protocol": phitex_live::lsp::PROTOCOL,
        "install": record,
        "channel": channel,
        "files": files,
        "texlive": texlive,
        "opens_viewer": opens,
        "watches": live,
        "issues": ISSUES,
    });
    (
        vec![
            ("The phitex program", program),
            ("TeX's files", tex),
            ("The cache", cache),
            ("Settings", settings),
            ("The viewer", viewer),
            ("Running watches (phitex session list)", watches),
        ],
        json,
    )
}

/// The report as text: its sections, then the issue link.
fn text(sections: &[Section]) -> String {
    let mut out = String::from("PhiTeX doctor\n");
    for (title, lines) in sections {
        out.push('\n');
        out.push_str(title);
        out.push('\n');
        for (ok, l) in lines {
            let mark = match ok {
                Some(true) => "✓ ",
                Some(false) => "✗ ",
                None => "",
            };
            out.push_str(&format!("  {mark}{l}\n"));
        }
    }
    out.push_str(&format!(
        "\nIf something looks wrong, open an issue with this report: {ISSUES}\n"
    ));
    out
}

/// `phitex doctor`: the report on standard output (`--json`: as one JSON
/// object); 0.
pub fn main(as_json: bool) -> i32 {
    let (sections, v) = findings();
    if as_json {
        println!("{v}");
    } else {
        print!("{}", text(&sections));
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The report ends with where to report a problem, and marks each
    /// finding that is good or bad.
    #[test]
    fn the_report_ends_with_the_issue_link() {
        let t = text(&[(
            "TeX's files",
            vec![
                (Some(false), "no TeX Live found".to_owned()),
                (None, "a fact".to_owned()),
            ],
        )]);
        assert!(t.starts_with("PhiTeX doctor\n"), "{t}");
        assert!(t.contains("  ✗ no TeX Live found\n  a fact\n"), "{t}");
        assert!(t.trim_end().ends_with(ISSUES), "{t}");
    }
}
