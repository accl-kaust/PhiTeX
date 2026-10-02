//! Port coverage of `tex.web`: which WEB sections the Rust code cites.
//!
//! A section counts as cited when a `.rs` file under `crates/` mentions it as
//! `§123` (ranges `§281–§283` / `§281-283` cite every section in between).
//! Sections are numbered as WEAVE does: each `@ ` or `@*` starts one.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::sync::LazyLock;

use anyhow::{Context, Result};
use regex::Regex;
use walkdir::WalkDir;

const TEX_WEB: &str = "upstream/texlive-source/texk/web2c/tex.web";

struct Part {
    number: usize,
    title: String,
    /// First and last section of the part.
    sections: (usize, usize),
}

pub fn run(root: &Path, args: &[String]) -> Result<()> {
    let web = fs::read_to_string(root.join(TEX_WEB))
        .with_context(|| format!("{TEX_WEB} missing; run scripts/fetch-upstream.sh"))?;
    let (parts, total) = parse_parts(&web);
    let cited = cited_sections(&root.join("crates"))?;

    // `--missing <part>` lists uncited sections of one part.
    if let [flag, part] = args
        && flag == "--missing"
    {
        let n: usize = part.parse()?;
        let p = parts
            .iter()
            .find(|p| p.number == n)
            .with_context(|| format!("no part {n}"))?;
        let missing: Vec<String> = (p.sections.0..=p.sections.1)
            .filter(|s| !cited.contains(s))
            .map(|s| format!("§{s}"))
            .collect();
        println!("[{}] {}: {} missing", p.number, p.title, missing.len());
        println!("{}", missing.join(" "));
        return Ok(());
    }

    let mut done = 0;
    for p in &parts {
        let n = p.sections.1 - p.sections.0 + 1;
        let c = (p.sections.0..=p.sections.1)
            .filter(|s| cited.contains(s))
            .count();
        done += c;
        println!(
            "[{:>2}] {:<44} §{:<4}–§{:<4} {c:>4}/{n:<4}",
            p.number, p.title, p.sections.0, p.sections.1
        );
    }
    println!("tex.web: {done}/{total} sections cited");
    Ok(())
}

/// Parts (`@*` chapters) with their section ranges, and the total section count.
fn parse_parts(web: &str) -> (Vec<Part>, usize) {
    static STAR: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^@\*\s*\\\[(\d+)\]\s*([^.]*)").unwrap());
    let mut parts: Vec<Part> = Vec::new();
    let mut section = 0;
    for line in web.lines() {
        if !(line == "@"
            || line.starts_with("@ ")
            || line.starts_with("@*")
            || line.starts_with("@\t"))
        {
            continue;
        }
        section += 1;
        if let Some(c) = STAR.captures(line) {
            if let Some(last) = parts.last_mut() {
                last.sections.1 = section - 1;
            }
            parts.push(Part {
                number: c[1].parse().unwrap_or(0),
                title: c[2].trim().to_owned(),
                sections: (section, section),
            });
        }
    }
    if let Some(last) = parts.last_mut() {
        last.sections.1 = section;
    }
    (parts, section)
}

fn cited_sections(dir: &Path) -> Result<BTreeSet<usize>> {
    static CITE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"§(\d+)(?:\s*[–-]\s*§?(\d+))?").unwrap());
    let mut out = BTreeSet::new();
    if !dir.is_dir() {
        return Ok(out);
    }
    for entry in WalkDir::new(dir) {
        let entry = entry?;
        if entry.path().extension().is_none_or(|e| e != "rs") {
            continue;
        }
        // Generated constants cite where each name is defined, not ported code.
        if entry.file_name() == "web.rs" {
            continue;
        }
        let src = fs::read_to_string(entry.path())?;
        for c in CITE.captures_iter(&src) {
            let a: usize = c[1].parse()?;
            let b: usize = c.get(2).map_or(Ok(a), |m| m.as_str().parse())?;
            out.extend(a..=b.max(a));
        }
    }
    Ok(out)
}
