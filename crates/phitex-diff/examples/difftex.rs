//! Dev tool: diff two project folders, print the marked-up `.tex` (and,
//! with `--changes`, the change list on stderr).
//!
//!     scripts/sandbox cargo run -q -p phitex-diff --example difftex -- OLD NEW [MAIN] [--type=T] [--subtype=S] [--add-color=C] [--del-color=C] [--driver=D] [--changes]
//!
//! (`--cfont` and `--color` are `--type=CFONT` and `--subtype=COLOR`.)

use phitex_diff::{Color, Dir, Driver, Markup, Options, Subtype, diff};
use std::path::PathBuf;

fn main() {
    let mut pos = Vec::new();
    let mut opts = Options::default();
    let mut changes = false;
    for a in std::env::args().skip(1) {
        let bad = |what: &str| -> ! {
            eprintln!("difftex: {what}");
            std::process::exit(2)
        };
        let (flag, value) = a.split_once('=').unwrap_or((a.as_str(), ""));
        match flag {
            "--cfont" => opts.markup = Markup::Cfont,
            "--color" => opts.subtype = Subtype::Color,
            "--type" => {
                opts.markup =
                    Markup::parse(value).unwrap_or_else(|| bad(&format!("no type {value}")));
            }
            "--subtype" => {
                opts.subtype =
                    Subtype::parse(value).unwrap_or_else(|| bad(&format!("no subtype {value}")));
            }
            "--add-color" => opts.add_color = Some(Color::parse(value).unwrap_or_else(|e| bad(&e))),
            "--del-color" => opts.del_color = Some(Color::parse(value).unwrap_or_else(|e| bad(&e))),
            "--driver" => {
                opts.driver = match value {
                    "pdftex" => Driver::Pdftex,
                    "xetex" => Driver::Xetex,
                    "dvips" => Driver::Dvips,
                    _ => bad(&format!("no driver {value}")),
                };
            }
            "--changes" => changes = true,
            _ => pos.push(a),
        }
    }
    if pos.len() < 2 {
        eprintln!("usage: difftex OLD_DIR NEW_DIR [MAIN] [--cfont] [--color] [--changes]");
        std::process::exit(2);
    }
    let main = pos.get(2).map_or("main.tex", String::as_str);
    let old = Dir(PathBuf::from(&pos[0]));
    let new = Dir(PathBuf::from(&pos[1]));
    match diff(&old, &new, main, &opts) {
        Ok(d) => {
            print!("{}", d.tex);
            if changes {
                for c in &d.changes {
                    eprintln!(
                        "{:?} {}:{}..{} -> {}:{}..{} [{}] {:?} -> {:?}",
                        c.kind,
                        c.old.file,
                        c.old.start,
                        c.old.end,
                        c.new.file,
                        c.new.start,
                        c.new.end,
                        c.section.as_deref().unwrap_or("-"),
                        c.old_text,
                        c.new_text
                    );
                }
            }
        }
        Err(e) => {
            eprintln!("difftex: {e}");
            std::process::exit(1);
        }
    }
}
