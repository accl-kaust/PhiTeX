//! XeTeX (docs/spec-xetex.md): the XDV file equals `xetex -no-pdf`'s,
//! at one thread and at eight. The oracles are TeX Live 2026's
//! (`oracle/fixtures/xetex/bless.sh`, `SOURCE_DATE_EPOCH=0`,
//! `FORCE_SOURCE_DATE=1`): `ini/NAME.xdv` from `xetex -ini -etex
//! NAME.tex`; `plain/NAME.xdv` from `xetex -ini -etex` on plain.tex as
//! the plain gates load it followed by `oracle/fixtures/plain/NAME.tex`
//! (`-default`: with `\output={}` between them), one file.
//!
//! `PHITEX_GATE=NAME` runs that case alone; `PHITEX_GATE_OUT=DIR`
//! writes each XDV made there.

mod common;

use std::path::Path;

use std::sync::{Arc, Mutex, OnceLock};

use phitex_tex::{Finder, FontIndex, FontSource, KpseFormat, Options, XeTeX};

/// The font files as XeTeX finds them: kpathsea as `xetex`, and the
/// bytes of a path.
struct Kpse(Mutex<Option<phitex_kpse::Kpse>>);

impl FontSource for Kpse {
    fn read(&self, path: &str) -> Option<Arc<[u8]>> {
        std::fs::read(path).ok().map(Arc::from)
    }

    fn find_file(&self, name: &str, format: KpseFormat) -> Option<String> {
        let fmt = match format {
            KpseFormat::OpenType => phitex_kpse::Format::OpenType,
            KpseFormat::TrueType => phitex_kpse::Format::TrueType,
            KpseFormat::Type1 => phitex_kpse::Format::Type1,
            KpseFormat::MiscFonts => phitex_kpse::Format::MiscFonts,
        };
        let mut k = self.0.lock().ok()?;
        let p = k.as_mut()?.find_file(name.as_bytes(), fmt, false)?;
        String::from_utf8(p).ok()
    }
}

/// The build's XeTeX: TeX Live's fonts through kpathsea, the name index
/// of `PHITEX_FONT_INDEX` (`phitex-otf`'s `otf-index`, fontconfig's list
/// of the fonts the oracle saw), made once.
fn xetex() -> XeTeX {
    static X: OnceLock<XeTeX> = OnceLock::new();
    X.get_or_init(|| {
        let path = std::env::var("PATH").unwrap_or_default();
        let dir = std::env::split_paths(&path).find(|d| d.join("tex").is_file());
        let kpse = dir.map(|d| phitex_kpse::Kpse::new(&d, "xetex", "xetex"));
        let index = std::env::var("PHITEX_FONT_INDEX")
            .ok()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|b| FontIndex::from_bytes(&b))
            .map(Arc::new);
        XeTeX {
            fonts: Some(Arc::new(Kpse(Mutex::new(kpse)))),
            font_index: index,
            pdf: None,
        }
    })
    .clone()
}

/// One document, its XDV against the oracle's at `threads`.
fn gate(dir: &Path, name: &str, src: &str, want: &[u8], threads: usize) -> Result<String, String> {
    let opts = Options {
        epoch: Some(0),
        threads: Some(threads),
        xetex: Some(xetex()),
        ..common::options(
            &format!("./{name}.tex"),
            name,
            &common::dvi_comment(want),
            dir,
        )
    };
    let b = common::build(opts, None, src);
    let (xdv, log) = b.output();
    if let Ok(d) = std::env::var("PHITEX_GATE_OUT") {
        let _ = std::fs::write(format!("{d}/{name}-j{threads}.xdv"), &xdv);
        let _ = std::fs::write(format!("{d}/{name}-j{threads}.log"), log.as_bytes());
    }
    match common::first_difference(&xdv, want) {
        None => Ok(format!("{name} -j{threads}: {} bytes", xdv.len())),
        Some(d) => {
            let diags: Vec<String> = b
                .diagnostics()
                .iter()
                .take(12)
                .map(|d| {
                    format!(
                        "  {}:{} {}",
                        d.file,
                        d.line,
                        d.message.chars().take(300).collect::<String>()
                    )
                })
                .collect();
            let tail: String = log
                .chars()
                .rev()
                .take(1500)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            Err(format!(
                "{name} -j{threads}: FAIL {d}\n{}\n--- log tail ---\n{tail}",
                diags.join("\n")
            ))
        }
    }
}

fn run(cases: &[(&str, String, Vec<u8>, &Path)]) {
    let only = std::env::var("PHITEX_GATE").ok();
    let mut failed = Vec::new();
    for (name, src, want, dir) in cases {
        if only.as_ref().is_some_and(|o| o != name) {
            continue;
        }
        for threads in [1, 8] {
            match gate(dir, name, src, want, threads) {
                Ok(r) => eprintln!("ok {r}"),
                Err(r) => {
                    eprintln!("{r}");
                    failed.push(format!("{name} -j{threads}"));
                }
            }
        }
    }
    assert!(failed.is_empty(), "failed: {failed:?}");
}

fn ini_cases(names: &[&'static str]) -> Vec<(&'static str, String, Vec<u8>, std::path::PathBuf)> {
    let x = common::fixtures().join("xetex");
    names
        .iter()
        .map(|&n| {
            let src = common::read(&x.join(format!("ini/{n}.tex")));
            let want = std::fs::read(x.join(format!("oracle/ini/{n}.xdv"))).expect("the oracle");
            (n, src, want, x.join("ini"))
        })
        .collect()
}

fn run_owned(cases: &[(&'static str, String, Vec<u8>, std::path::PathBuf)]) {
    let c: Vec<(&str, String, Vec<u8>, &Path)> = cases
        .iter()
        .map(|(n, s, w, d)| (*n, s.clone(), w.clone(), d.as_path()))
        .collect();
    run(&c);
}

/// TFM fonts only: the writer's XDV opcodes, two pages.
#[test]
fn ini_tfm() {
    run_owned(&ini_cases(&["tfm"]));
}

/// The Unicode reader, `^^^^`, wide codes, `\Uchar`, `\Ucharcat`.
#[test]
fn ini_chars() {
    run_owned(&ini_cases(&["chars"]));
}

/// Native fonts: words in Latin Modern, interword glue, the merge of
/// words, discretionaries and hyphens, patterns, `\XeTeXglyph`, actual
/// text, glyph metrics, interword space shaping, the font queries.
#[test]
fn ini_native() {
    run_owned(&ini_cases(&["native"]));
}

/// Fonts by name through the index, styles, features, `TECkit`'s
/// tex-text, letterspace, colour, extend, slant, sizes.
#[test]
fn ini_features() {
    run_owned(&ini_cases(&["features"]));
}

/// `\XeTeXinputencoding`: bytes, UTF-8 again, a malformed sequence.
#[test]
fn ini_encoding() {
    run_owned(&ini_cases(&["encoding"]));
}

/// `\XeTeXinterchartoks`.
#[test]
fn ini_interchar() {
    run_owned(&ini_cases(&["interchar"]));
}

/// Right-to-left and mixed text: the bidi runs of a word.
#[test]
fn ini_bidi() {
    run_owned(&ini_cases(&["bidi"]));
}

/// `\XeTeXlinebreaklocale`: the breaks between words of scripts
/// without spaces.
#[test]
fn ini_linebreak() {
    run_owned(&ini_cases(&["linebreak", "linebreak-cj"]));
}

/// OpenType math: `\Umathchar`, the font's constants, variants and
/// assemblies.
#[test]
fn ini_math() {
    run_owned(&ini_cases(&["math"]));
}

/// `\XeTeXpicfile` and `\XeTeXpdffile`.
#[test]
fn ini_pics() {
    run_owned(&ini_cases(&["pics"]));
}

/// plain.tex's documents under XeTeX (TFM fonts).
#[test]
fn plain_docs() {
    let fx = common::fixtures();
    let plain = common::plain();
    let x = fx.join("xetex/oracle/plain");
    let mut cases = Vec::new();
    for body in ["story", "pages", "display", "table", "hyphens"] {
        let b = common::read(&fx.join(format!("plain/{body}.tex")));
        for default in [false, true] {
            let name: &'static str = Box::leak(
                if default {
                    format!("{body}-default")
                } else {
                    body.to_owned()
                }
                .into_boxed_str(),
            );
            let src = if default {
                format!("{plain}\\output={{}}\n{b}")
            } else {
                format!("{plain}{b}")
            };
            let want = std::fs::read(x.join(format!("{name}.xdv"))).expect("the oracle");
            cases.push((name, src, want, fx.join("plain")));
        }
    }
    run_owned(&cases);
}

/// kpathsea as `xelatex` (`TEXINPUTS.xelatex`, engine `xetex`).
fn xelatex_kpse() -> &'static Mutex<Option<phitex_kpse::Kpse>> {
    static K: OnceLock<Mutex<Option<phitex_kpse::Kpse>>> = OnceLock::new();
    K.get_or_init(|| {
        let path = std::env::var("PATH").unwrap_or_default();
        let dir = std::env::split_paths(&path).find(|d| d.join("tex").is_file());
        Mutex::new(dir.map(|d| phitex_kpse::Kpse::new(&d, "xelatex", "xetex")))
    })
}

/// Files from the document's directories (printed as `./name`), the
/// rest through kpathsea as `xelatex` finds them.
fn xelatex_finder(dirs: Vec<std::path::PathBuf>) -> Finder {
    Arc::new(move |name: &str| {
        for dir in &dirs {
            let p = dir.join(name);
            if p.is_file() {
                return Some((format!("./{name}"), std::fs::read(&p).ok()?));
            }
        }
        let ext = Path::new(name)
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        let fmt = match ext.as_str() {
            "tfm" => phitex_kpse::Format::Tfm,
            "map" => phitex_kpse::Format::FontMap,
            "pfb" | "pfa" => phitex_kpse::Format::Type1,
            "enc" => phitex_kpse::Format::Enc,
            "ttf" | "ttc" => phitex_kpse::Format::TrueType,
            "otf" => phitex_kpse::Format::OpenType,
            "vf" => phitex_kpse::Format::Vf,
            _ => phitex_kpse::Format::Tex,
        };
        let mut k = xelatex_kpse().lock().ok()?;
        let path = k.as_mut()?.find_file(name.as_bytes(), fmt, false)?;
        let path = String::from_utf8(path).ok()?;
        Some((path.clone(), std::fs::read(&path).ok()?))
    })
}

/// LaTeX documents under `xelatex` (the format from `xelatex.ini`, the
/// first run, no `.aux`): `oracle/fixtures/xetex/docs` and
/// `oracle/fixtures/latex/docs`, against `oracle/docs/NAME.xdv`. Heavy:
/// ignored by default, `PHITEX_GATE=NAME` for one. With `TEXMFHOME` set
/// (a tree holding TeX Live's xecjk, ctex, fandol, zhnumber, cjk,
/// cjkpunct and l3packages, which TeX Live's scheme on accl lacks, and
/// the font index made with its fonts: `scripts/xetex/cjk.sh`) also
/// `oracle/fixtures/xetex/cjk`.
#[test]
#[ignore = "the xelatex format: run with --ignored"]
fn xelatex_docs() {
    let fx = common::fixtures();
    let x = fx.join("xetex");
    let only = std::env::var("PHITEX_GATE").ok();
    let mut failed = Vec::new();
    let mut names: Vec<(String, std::path::PathBuf)> = Vec::new();
    let mut dirs = vec![x.join("docs"), fx.join("latex/docs")];
    if std::env::var_os("TEXMFHOME").is_some() {
        dirs.push(x.join("cjk"));
    }
    for d in dirs {
        for e in std::fs::read_dir(&d).expect("the documents").flatten() {
            let f = e.file_name().to_string_lossy().into_owned();
            if let Some(n) = f.strip_suffix(".tex")
                && n != "umath-body"
                && x.join(format!("oracle/docs/{n}.xdv")).is_file()
                && only.as_ref().is_none_or(|o| o.split(',').any(|x| x == n))
            {
                names.push((n.to_owned(), d.clone()));
            }
        }
    }
    names.sort();
    for (name, dir) in &names {
        let want = std::fs::read(x.join(format!("oracle/docs/{name}.xdv"))).expect("the oracle");
        let src = common::read(&dir.join(format!("{name}.tex")));
        for threads in [1, 8] {
            let opts = Options {
                epoch: Some(0),
                threads: Some(threads),
                xetex: Some(xetex()),
                prelude_jobname: "xelatex".to_owned(),
                finder: xelatex_finder(vec![dir.clone()]),
                ..common::options(
                    &format!("./{name}.tex"),
                    name,
                    &common::dvi_comment(&want),
                    dir,
                )
            };
            let b = common::build(opts, Some("\\input xelatex.ini\n"), &src);
            let (xdv, log) = b.output();
            if let Ok(d) = std::env::var("PHITEX_GATE_OUT") {
                let _ = std::fs::write(format!("{d}/{name}-j{threads}.xdv"), &xdv);
                let _ = std::fs::write(format!("{d}/{name}-j{threads}.log"), log.as_bytes());
            }
            match common::first_difference(&xdv, &want) {
                None => eprintln!("ok {name} -j{threads}: {} bytes", xdv.len()),
                Some(d) => {
                    let diags: Vec<String> = b
                        .diagnostics()
                        .iter()
                        .take(12)
                        .map(|d| {
                            format!(
                                "  {}:{} {}",
                                d.file,
                                d.line,
                                d.message.chars().take(300).collect::<String>()
                            )
                        })
                        .collect();
                    let tail: String = log
                        .chars()
                        .rev()
                        .take(1500)
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                        .collect();
                    eprintln!(
                        "{name} -j{threads}: FAIL {d}\n{}\n--- log tail ---\n{tail}",
                        diags.join("\n")
                    );
                    failed.push(format!("{name} -j{threads}"));
                }
            }
        }
    }
    assert!(failed.is_empty(), "failed: {failed:?}");
}

/// Glyph origins of XeLaTeX's documents (`xelatex_docs`' set, with
/// TeX Live's PDF of each, xdvipdfmx's of the oracle's XDV): the XDV
/// writer's tap gives each page as many codes as the PDF's page shows
/// (`pdftext::page_codes`), every glyph has an origin (its source bytes
/// or a call's range synthesized), the same at -j1 and -j8, the XDV the same with them on
/// (docs/note-origins.md). Ignored by default, `PHITEX_GATE=NAME` for
/// one.
#[test]
#[ignore = "the xelatex format: run with --ignored"]
fn xelatex_glyph_origins() {
    let fx = common::fixtures();
    let x = fx.join("xetex");
    let only = std::env::var("PHITEX_GATE").ok();
    let mut failed = Vec::new();
    let mut names: Vec<(String, std::path::PathBuf)> = Vec::new();
    let mut dirs = vec![x.join("docs"), fx.join("latex/docs")];
    if std::env::var_os("TEXMFHOME").is_some() {
        dirs.push(x.join("cjk"));
    }
    for d in dirs {
        for e in std::fs::read_dir(&d).expect("the documents").flatten() {
            let f = e.file_name().to_string_lossy().into_owned();
            if let Some(n) = f.strip_suffix(".tex")
                && n != "umath-body"
                && x.join(format!("oracle/docs/{n}.pdf")).is_file()
                && only.as_ref().is_none_or(|o| o.split(',').any(|x| x == n))
            {
                names.push((n.to_owned(), d.clone()));
            }
        }
    }
    names.sort();
    for (name, dir) in &names {
        let want = std::fs::read(x.join(format!("oracle/docs/{name}.xdv"))).expect("the oracle");
        let pdf = Arc::new(
            std::fs::read(x.join(format!("oracle/docs/{name}.pdf"))).expect("the oracle's PDF"),
        );
        let src = common::read(&dir.join(format!("{name}.tex")));
        let build = |threads: usize| {
            let opts = Options {
                epoch: Some(0),
                threads: Some(threads),
                xetex: Some(xetex()),
                origins: true,
                prelude_jobname: "xelatex".to_owned(),
                finder: xelatex_finder(vec![dir.clone()]),
                ..common::options(
                    &format!("./{name}.tex"),
                    name,
                    &common::dvi_comment(&want),
                    dir,
                )
            };
            let b = common::build(opts, Some("\\input xelatex.ini\n"), &src);
            b.set_origins_pdf(pdf.clone());
            b
        };
        let b = build(1);
        if let Some(d) = common::first_difference(&b.output().0, &want) {
            eprintln!("{name}: the XDV with origins on: {d}");
            failed.push(format!("{name} xdv"));
            continue;
        }
        let f = common::origin_failures_in(&b, &pdf, &src, &format!("{name}.tex"), false, false);
        let pages = b.all_page_origins();
        let codes: usize = pages.iter().flatten().map(|p| p.len()).sum();
        if f.is_empty() {
            eprintln!("ok {name}: {} pages, {codes} codes", pages.len());
        } else {
            eprintln!("{name}: origins:\n{}", f.join("\n"));
            failed.push(name.clone());
        }
        let b8 = build(8);
        if b8.all_page_origins() != pages {
            eprintln!("{name}: -j8's origins are not -j1's");
            failed.push(format!("{name} -j8"));
        }
    }
    assert!(failed.is_empty(), "failed: {failed:?}");
}

/// The native runner of `\write18`'s commands, as the CLI has it.
#[path = "../../../cli/src/shell.rs"]
mod process_hook;

/// With `PHITEX_SHELL_ESCAPE=1`, TeX Live's commands (minted's
/// `latexminted`) run as processes for a document in `dir`, as the CLI
/// runs them; else none.
fn shell_hook(dir: &Path) -> Option<Arc<dyn phitex_tex::ShellHook>> {
    if std::env::var("PHITEX_SHELL_ESCAPE").ok().as_deref() != Some("1") {
        return None;
    }
    let path = std::env::var("PATH").unwrap_or_default();
    let bin = std::env::split_paths(&path).find(|d| d.join("kpsewhich").is_file())?;
    let k = phitex_kpse::Kpse::new(&bin, "xelatex", "xetex");
    Some(Arc::new(process_hook::ProcessHook::new(dir, k.exported())))
}

/// A document of the client's under `xelatex` (the first run): the
/// directory `PHITEX_DOC_DIR` (its main file `PHITEX_DOC`, default
/// `main`), against `oracle/NAME.xdv` in it. Ignored by default.
#[test]
#[ignore = "a document of the client's: PHITEX_DOC_DIR"]
fn xelatex_document() {
    let Ok(dir) = std::env::var("PHITEX_DOC_DIR") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let name = std::env::var("PHITEX_DOC").unwrap_or_else(|_| "main".to_owned());
    let want = std::fs::read(dir.join(format!("oracle/{name}.xdv"))).unwrap_or_default();
    let src = common::read(&dir.join(format!("{name}.tex")));
    for threads in [1, 8] {
        let t0 = std::time::Instant::now();
        let opts = Options {
            epoch: Some(0),
            threads: Some(threads),
            xetex: Some(xetex()),
            prelude_jobname: "xelatex".to_owned(),
            finder: xelatex_finder(vec![dir.clone()]),
            shell_hook: shell_hook(&dir),
            ..common::options(
                &format!("./{name}.tex"),
                &name,
                &common::dvi_comment(&want),
                &dir,
            )
        };
        let b = common::build(opts, Some("\\input xelatex.ini\n"), &src);
        let (xdv, log) = b.output();
        if let Ok(d) = std::env::var("PHITEX_GATE_OUT") {
            let _ = std::fs::write(format!("{d}/{name}-j{threads}.xdv"), &xdv);
            let _ = std::fs::write(format!("{d}/{name}-j{threads}.log"), log.as_bytes());
        }
        let diags: Vec<String> = b
            .diagnostics()
            .iter()
            .take(20)
            .map(|d| {
                format!(
                    "  {}:{} {}",
                    d.file,
                    d.line,
                    d.message.chars().take(300).collect::<String>()
                )
            })
            .collect();
        match common::first_difference(&xdv, &want) {
            None => eprintln!(
                "ok {name} -j{threads}: {} bytes, {:?}",
                xdv.len(),
                t0.elapsed()
            ),
            Some(d) => eprintln!(
                "{name} -j{threads}: FAIL {d} ({:?})\n{}",
                t0.elapsed(),
                diags.join("\n")
            ),
        }
        if b.stats.shell_passes > 0 {
            eprintln!(
                "{name} -j{threads}: {} evaluations again for the commands' files, {} of them",
                b.stats.shell_passes,
                b.shell_files().len()
            );
        }
    }
}
