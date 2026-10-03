//! Glyph origins' checks (DESIGN 4.4, `PARTEX_ORIGINS=1`): a run's
//! `<job>.origins.jsonl` against its PDF and its sources.
//!
//! - Each page has as many glyphs as its content stream shows character
//!   codes, counted as `partex_engine::pdftext` counts them (the order
//!   origins are defined in): each code a `Tj`, `TJ`, `'` or `"` shows,
//!   a form's at each `Do` that draws it, again at each use.
//! - A glyph with no source has `file` `u32::MAX`, `start` and `end` 0,
//!   and is synthesized; any other's range is within its file.
//! - A glyph not synthesized whose range is one byte, a letter or a
//!   digit, shows that byte (the fonts are OT1's).
//! - Chosen runs of glyphs have the ranges expected ([`Expect`]).

use std::fs;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow, ensure};
use partex_engine::pdfread::Doc;
use partex_engine::pdftext;

/// A side file: the files by id, and each page's glyphs, `[file, start,
/// end, synthesized]`.
pub struct Origins {
    pub files: Vec<String>,
    pub pages: Vec<Vec<[u64; 4]>>,
}

/// Read side file `path`.
pub fn read(path: &Path) -> Result<Origins> {
    let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let mut lines = text.lines();
    let first: serde_json::Value =
        serde_json::from_str(lines.next().context("an empty side file")?)?;
    let files = first["files"]
        .as_array()
        .context("no files")?
        .iter()
        .map(|f| f.as_str().map(str::to_owned).context("a file's name"))
        .collect::<Result<Vec<_>>>()?;
    let mut pages = Vec::new();
    for (n, l) in lines.enumerate() {
        let v: serde_json::Value = serde_json::from_str(l)?;
        ensure!(
            v["page"].as_u64() == Some(n as u64 + 1),
            "page {} numbered {}",
            n + 1,
            v["page"]
        );
        let glyphs = v["glyphs"]
            .as_array()
            .context("no glyphs")?
            .iter()
            .map(|g| {
                let a = g.as_array().context("a glyph")?;
                ensure!(a.len() == 4, "a glyph of {} numbers", a.len());
                let mut out = [0u64; 4];
                for (o, x) in out.iter_mut().zip(a) {
                    *o = x.as_u64().context("a glyph's number")?;
                }
                Ok(out)
            })
            .collect::<Result<Vec<_>>>()?;
        pages.push(glyphs);
    }
    Ok(Origins { files, pages })
}

/// The codes each page of PDF file `path` shows, in `pdftext`'s order.
pub fn pdf_codes(path: &Path) -> Result<Vec<Vec<u32>>> {
    let data: Arc<[u8]> = fs::read(path)
        .with_context(|| format!("reading {}", path.display()))?
        .into();
    let doc = Doc::open(&data).map_err(|e| anyhow!("{}: {e:?}", path.display()))?;
    Ok((1..=doc.num_pages())
        .map(|n| {
            doc.page(n)
                .map(|p| {
                    pdftext::page_codes(&doc, &p)
                        .into_iter()
                        .map(|s| s.code)
                        .collect()
                })
                .unwrap_or_default()
        })
        .collect())
}

/// A glyph with no source names this file.
const NONE: u64 = u32::MAX as u64;

/// A run of glyphs, part of an [`Expect`].
#[derive(Clone, Copy, Debug)]
pub enum Seg {
    /// Glyphs showing these codes, none synthesized, their ranges one
    /// after the other making this text's next occurrence (a ligature's
    /// covering its characters).
    Text(&'static [u8], &'static str),
    /// [`Seg::Text`] of the text's first occurrence after
    /// `\begin{document}` (a box used again).
    Again(&'static [u8], &'static str),
    /// Synthesized glyphs showing these codes, each's range this text's
    /// next occurrence (the call).
    Synth(&'static [u8], &'static str),
    /// A line break's hyphen: synthesized, the range of the character
    /// before it.
    Hyphen,
    /// Glyphs with no source showing these codes (a literal's).
    None(&'static [u8]),
    /// So many glyphs with no source (an included page's text).
    Nones(usize),
}

/// Runs of glyphs, one after the other, the first time their codes are
/// shown on a page (0-based), their text in the job's main file.
#[derive(Clone, Copy, Debug)]
pub struct Expect {
    pub page: usize,
    pub segs: &'static [Seg],
}

/// `tests/e2e/glyphs.tex`'s expectations.
pub const GLYPHS: &[Expect] = &[
    Expect {
        page: 0,
        segs: &[
            Seg::Synth(b"1", "\\section{Intro}"),
            Seg::Text(b"Intro", "Intro"),
        ],
    },
    Expect {
        page: 0,
        segs: &[
            Seg::Text(b"Hello", "Hello"),
            Seg::Text(b"world,", "world,"),
            Seg::Text(b"\x0crst", "first"),
            Seg::Text(b"\x0cner", "finer"),
            Seg::Text(b"o\x0ece.", "office."),
        ],
    },
    Expect {
        page: 0,
        segs: &[
            Seg::Text(b"macro:", "macro:"),
            Seg::Synth(b"bar", "\\foo"),
            Seg::Text(b"and", "and"),
            Seg::Text(b"section", "section"),
            Seg::Synth(b"1", "\\thesection"),
            Seg::Text(b".", "."),
        ],
    },
    Expect {
        page: 0,
        segs: &[
            Seg::Text(b"Saved", "Saved"),
            Seg::Text(b"and", "and"),
            Seg::Again(b"Saved", "Saved"),
            Seg::Text(b".", "."),
        ],
    },
    Expect {
        page: 0,
        segs: &[Seg::Text(b"Tikz", "Tikz")],
    },
    Expect {
        page: 0,
        segs: &[
            Seg::Text(b"charac", "charac"),
            Seg::Hyphen,
            Seg::Text(b"teris", "teris"),
            Seg::Hyphen,
            Seg::Text(b"tically", "tically"),
        ],
    },
    Expect {
        page: 0,
        segs: &[
            Seg::Text(b"Form", "Form"),
            Seg::Text(b"twice", "twice"),
            Seg::Again(b"Form", "Form"),
            Seg::Text(b"done.", "done."),
            Seg::None(b"lit"),
            Seg::Text(b"After", "After"),
        ],
    },
    Expect {
        page: 0,
        segs: &[
            Seg::Text(b"literal.", "literal."),
            Seg::Nones(19),
            Seg::Synth(b"1", "\\newpage"),
        ],
    },
    Expect {
        page: 1,
        segs: &[
            Seg::Text(b"Second", "Second"),
            Seg::Text(b"page.", "page."),
            Seg::Synth(b"2", "\\end{document}"),
        ],
    },
];

impl Expect {
    /// The codes these runs show (a glyph with no source: any code).
    fn needle(&self) -> Vec<Option<u32>> {
        let mut needle = Vec::new();
        for seg in self.segs {
            match seg {
                Seg::Text(c, _) | Seg::Again(c, _) | Seg::Synth(c, _) | Seg::None(c) => {
                    needle.extend(c.iter().map(|&b| Some(u32::from(b))));
                }
                Seg::Hyphen => needle.push(Some(u32::from(b'-'))),
                Seg::Nones(n) => needle.extend(std::iter::repeat_n(None, *n)),
            }
        }
        needle
    }

    /// Check these runs on page `page` of `origins` (its codes `codes`),
    /// the main file's text `text` (file `file`).
    fn check(
        &self,
        origins: &Origins,
        codes: &[Vec<u32>],
        file: u64,
        text: &[u8],
    ) -> Result<(), String> {
        let what = format!("page {}, {:?}", self.page + 1, self.segs);
        let (Some(glyphs), Some(shown)) = (origins.pages.get(self.page), codes.get(self.page))
        else {
            return Err(format!("{what}: no such page"));
        };
        let needle = self.needle();
        let first = (0..shown.len().saturating_sub(needle.len() - 1)).find(|&at| {
            needle.iter().enumerate().all(|(i, c)| match c {
                Some(c) => shown[at + i] == *c,
                None => glyphs.get(at + i).is_some_and(|g| g[0] == NONE),
            })
        });
        let Some(at) = first else {
            return Err(format!("{what}: codes not shown"));
        };
        let mut run = Run {
            glyphs,
            at,
            file,
            text,
            body: find(text, b"\\begin{document}", 0).map_or(0, |(_, e)| e),
            pos: 0,
        };
        run.pos = run.body;
        for seg in self.segs {
            run.seg(*seg).map_err(|m| format!("{what}: {m}"))?;
        }
        Ok(())
    }
}

/// An [`Expect`]'s runs being checked: the page's glyphs, the next one's
/// index, the main file and its text, where its body begins, and where
/// the next run's text is looked for.
struct Run<'a> {
    glyphs: &'a [[u64; 4]],
    at: usize,
    file: u64,
    text: &'a [u8],
    body: usize,
    pos: usize,
}

impl Run<'_> {
    /// A glyph and its text, for a message.
    fn show(&self, g: &[u64; 4]) -> String {
        let r = usize::try_from(g[1]).unwrap_or(0)..usize::try_from(g[2]).unwrap_or(0);
        format!(
            "{g:?} {:?}{}",
            self.text
                .get(r)
                .map(String::from_utf8_lossy)
                .unwrap_or_default(),
            if g[3] == 1 { " (synthesized)" } else { "" }
        )
    }

    /// The range of `src`'s first occurrence from `from`.
    fn find(&self, src: &str, from: usize) -> Result<(u64, u64), String> {
        find(self.text, src.as_bytes(), from)
            .map(|(a, e)| (a as u64, e as u64))
            .ok_or_else(|| format!("`{src}` not in the text"))
    }

    /// Check the next glyphs against `seg`.
    fn seg(&mut self, seg: Seg) -> Result<(), String> {
        match seg {
            Seg::Text(c, src) | Seg::Again(c, src) => {
                let again = matches!(seg, Seg::Again(..));
                let (a, e) = self.find(src, if again { self.body } else { self.pos })?;
                let mut next = a;
                for g in &self.glyphs[self.at..self.at + c.len()] {
                    if g[0] != self.file || g[3] != 0 || g[1] != next || g[2] <= g[1] {
                        return Err(format!("`{src}` at {a}..{e}, a glyph {}", self.show(g)));
                    }
                    next = g[2];
                }
                if next != e {
                    return Err(format!("`{src}` at {a}..{e}, glyphs to {next}"));
                }
                if !again {
                    self.pos = usize::try_from(e).unwrap_or(usize::MAX);
                }
                self.at += c.len();
            }
            Seg::Synth(c, src) => {
                // (the text after it may be the call's own: `pos` stays)
                let (a, e) = self.find(src, self.pos)?;
                for g in &self.glyphs[self.at..self.at + c.len()] {
                    if *g != [self.file, a, e, 1] {
                        return Err(format!("`{src}` at {a}..{e}, a glyph {}", self.show(g)));
                    }
                }
                self.at += c.len();
            }
            Seg::Hyphen => {
                let g = self.glyphs[self.at];
                let before = self.at.checked_sub(1).map(|i| self.glyphs[i]);
                if before.is_none_or(|b| g != [self.file, b[2] - 1, b[2], 1]) {
                    return Err(format!("a hyphen {}", self.show(&g)));
                }
                self.at += 1;
            }
            Seg::None(c) => {
                for g in &self.glyphs[self.at..self.at + c.len()] {
                    if *g != [NONE, 0, 0, 1] {
                        return Err(format!("a glyph with no source {g:?}"));
                    }
                }
                self.at += c.len();
            }
            // (matched as such)
            Seg::Nones(n) => self.at += n,
        }
        Ok(())
    }
}

/// The first occurrence of `what` in `text` from `from`: its range.
fn find(text: &[u8], what: &[u8], from: usize) -> Option<(usize, usize)> {
    text.get(from..)?
        .windows(what.len())
        .position(|w| w == what)
        .map(|i| (from + i, from + i + what.len()))
}

/// Check `dir/<job>.origins.jsonl` against `dir/<job>.pdf`, the sources
/// in `dir` and `expect`: the problems found.
pub fn check(dir: &Path, job: &str, expect: &[Expect]) -> Result<Vec<String>> {
    let origins = read(&dir.join(format!("{job}.origins.jsonl")))?;
    let codes = pdf_codes(&dir.join(format!("{job}.pdf")))?;
    let mut bad = Vec::new();
    if origins.pages.len() != codes.len() {
        bad.push(format!(
            "{} pages of origins, {} in the PDF",
            origins.pages.len(),
            codes.len()
        ));
    }
    // (the files in `dir`, as the job asked for them)
    let texts: Vec<Option<Vec<u8>>> = origins
        .files
        .iter()
        .map(|f| {
            fs::read(dir.join(f))
                .or_else(|_| fs::read(dir.join(format!("{f}.tex"))))
                .ok()
        })
        .collect();
    let mut letters = 0;
    for (n, (glyphs, shown)) in origins.pages.iter().zip(&codes).enumerate() {
        if glyphs.len() != shown.len() {
            bad.push(format!(
                "page {}: {} glyphs, {} codes shown",
                n + 1,
                glyphs.len(),
                shown.len()
            ));
            continue;
        }
        for (k, (g, &code)) in glyphs.iter().zip(shown).enumerate() {
            let [file, start, end, synth] = *g;
            if file == NONE {
                if start != 0 || end != 0 || synth != 1 {
                    bad.push(format!("page {}, glyph {k}: no source but {g:?}", n + 1));
                }
                continue;
            }
            let Some(text) = usize::try_from(file).ok().and_then(|f| texts.get(f)) else {
                bad.push(format!("page {}, glyph {k}: file {file} unnamed", n + 1));
                continue;
            };
            let Some(text) = text else {
                continue;
            };
            if start > end || end > text.len() as u64 {
                bad.push(format!(
                    "page {}, glyph {k}: {g:?} past the file's end ({})",
                    n + 1,
                    text.len()
                ));
                continue;
            }
            if synth == 0 && end == start + 1 {
                let b = text[usize::try_from(start)?];
                if b.is_ascii_alphanumeric() {
                    letters += 1;
                    if u32::from(b) != code {
                        bad.push(format!(
                            "page {}, glyph {k}: code {code:#x}, its source `{}`",
                            n + 1,
                            char::from(b)
                        ));
                    }
                }
            }
        }
    }
    if letters < 50 {
        bad.push(format!(
            "only {letters} glyphs checked against their source"
        ));
    }
    if let Some(f) = origins.files.iter().position(|f| f == job)
        && let Some(Some(text)) = texts.get(f)
    {
        for e in expect {
            if let Err(m) = e.check(&origins, &codes, f as u64, text) {
                bad.push(m);
            }
        }
    } else if !expect.is_empty() {
        bad.push(format!("no file `{job}` among the origins' files"));
    }
    Ok(bad)
}

/// `cargo xtask origins DIR JOB [--glyphs] [--dump]`: [`check`], each
/// problem printed (`--dump`: each glyph first, its code and its text).
pub fn run(args: &[String]) -> Result<()> {
    let (Some(dir), Some(job)) = (args.first(), args.get(1)) else {
        anyhow::bail!("usage: cargo xtask origins DIR JOB [--glyphs] [--dump]");
    };
    let expect = if args.iter().any(|a| a == "--glyphs") {
        GLYPHS
    } else {
        &[]
    };
    if args.iter().any(|a| a == "--dump") {
        dump(Path::new(dir), job)?;
    }
    let bad = check(Path::new(dir), job, expect)?;
    for b in &bad {
        println!("{b}");
    }
    ensure!(bad.is_empty(), "{} problems", bad.len());
    println!("origins: ok");
    Ok(())
}

/// Print each glyph of `dir/<job>.origins.jsonl`: its page, its index,
/// the code shown, its origin and the text there.
fn dump(dir: &Path, job: &str) -> Result<()> {
    let o = read(&dir.join(format!("{job}.origins.jsonl")))?;
    let codes = pdf_codes(&dir.join(format!("{job}.pdf")))?;
    for (n, glyphs) in o.pages.iter().enumerate() {
        for (k, g) in glyphs.iter().enumerate() {
            let code = codes.get(n).and_then(|c| c.get(k)).copied();
            let text = o
                .files
                .get(usize::try_from(g[0]).unwrap_or(usize::MAX))
                .and_then(|f| {
                    fs::read(dir.join(f))
                        .or_else(|_| fs::read(dir.join(format!("{f}.tex"))))
                        .ok()
                })
                .and_then(|t| {
                    let r = usize::try_from(g[1]).ok()?..usize::try_from(g[2]).ok()?;
                    t.get(r).map(|b| String::from_utf8_lossy(b).into_owned())
                });
            println!(
                "{} {k} {:?} {:?} {}",
                n + 1,
                code.and_then(char::from_u32),
                g,
                text.map_or_else(String::new, |t| format!("{t:?}"))
            );
        }
    }
    Ok(())
}
