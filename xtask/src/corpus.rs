//! Index every upstream test case into `corpus/manifest.json`.
//!
//! Suites:
//! - `trip` / `etrip`: Knuth's and e-TeX's torture tests (expected outputs shipped upstream).
//! - `web2c`: engine regression scripts (`*.test`) in TeX Live's web2c tree.
//! - `latex2e` / `latex3`: l3build test files (`.lvt` log tests, `.pvt` PDF tests),
//!   with the engines each l3build config checks and the upstream `.tlg`/`.tpf` files.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use anyhow::{Context, Result};
use regex::Regex;
use serde::Serialize;
use walkdir::WalkDir;

/// l3build's defaults when a config does not override them.
const DEFAULT_ENGINES: &[&str] = &["pdftex", "xetex", "luatex"];
const DEFAULT_STDENGINE: &str = "pdftex";
const DEFAULT_TESTFILEDIR: &str = "testfiles";

#[derive(Serialize)]
struct Manifest {
    upstream: BTreeMap<String, String>,
    summary: BTreeMap<String, usize>,
    tests: Vec<TestCase>,
}

#[derive(Serialize)]
struct TestCase {
    id: String,
    suite: &'static str,
    kind: Kind,
    /// Main input, relative to the workspace root.
    input: String,
    /// Engines the upstream harness runs this test on.
    engines: Vec<String>,
    /// Upstream expected outputs, keyed by engine (`"*"` = default for all engines).
    expected: BTreeMap<String, String>,
    /// How l3build runs this test (`.lvt`/`.pvt` only).
    #[serde(skip_serializing_if = "Option::is_none")]
    l3build: Option<L3Run>,
}

#[derive(Serialize)]
struct L3Run {
    /// Module directory (holds `build.lua`), relative to the workspace root.
    module: String,
    /// `build` or the `config-*` name passed to `l3build check -c`.
    config: String,
    /// Directory where l3build runs the test and leaves its outputs.
    testdir: String,
    stdengine: String,
    /// Whether the module's `checkconfigs` includes this config (upstream CI runs it).
    checked: bool,
}

#[derive(Serialize, Clone, Copy)]
#[serde(rename_all = "kebab-case")]
enum Kind {
    Trip,
    ShellTest,
    Lvt,
    Pvt,
}

/// One l3build configuration: a test directory and the engines it runs.
struct L3Config {
    name: String,
    checked: bool,
    testfiledir: String,
    engines: Vec<String>,
    stdengine: String,
}

pub fn run(root: &Path) -> Result<()> {
    let upstream = root.join("upstream");
    anyhow::ensure!(
        upstream.is_dir(),
        "{} missing; run scripts/fetch-upstream.sh",
        upstream.display()
    );

    let mut tests = Vec::new();
    tests.extend(trip_tests(root));
    tests.extend(web2c_tests(root)?);
    for suite in ["latex2e", "latex3"] {
        tests.extend(l3build_tests(root, suite)?);
    }
    tests.sort_by(|a, b| a.id.cmp(&b.id));

    let mut summary = BTreeMap::new();
    for t in &tests {
        let kind = serde_json::to_value(t.kind)?;
        let key = format!("{}/{}", t.suite, kind.as_str().unwrap_or("?"));
        *summary.entry(key).or_insert(0) += 1;
        for e in &t.engines {
            *summary.entry(format!("engine/{e}")).or_insert(0) += 1;
        }
    }

    let manifest = Manifest {
        upstream: upstream_commits(&upstream)?,
        summary,
        tests,
    };
    let out = root.join("corpus/manifest.json");
    fs::create_dir_all(out.parent().unwrap())?;
    fs::write(&out, serde_json::to_string_pretty(&manifest)? + "\n")?;

    println!("{} tests -> {}", manifest.tests.len(), rel(root, &out));
    for (k, n) in &manifest.summary {
        println!("  {k:<28} {n:>5}");
    }
    Ok(())
}

fn upstream_commits(upstream: &Path) -> Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    for entry in fs::read_dir(upstream)? {
        let dir = entry?.path();
        let head = std::process::Command::new("git")
            .arg("-C")
            .arg(&dir)
            .args(["rev-parse", "HEAD"])
            .output()
            .with_context(|| format!("git rev-parse in {}", dir.display()))?;
        let name = dir.file_name().unwrap().to_string_lossy().into_owned();
        out.insert(
            name,
            String::from_utf8_lossy(&head.stdout).trim().to_owned(),
        );
    }
    Ok(out)
}

fn trip_tests(root: &Path) -> Vec<TestCase> {
    let web2c = "upstream/texlive-source/texk/web2c";
    let specs = [
        ("trip", "tex", format!("{web2c}/triptrap"), "trip"),
        ("etrip", "etex", format!("{web2c}/etexdir/etrip"), "etrip"),
    ];
    specs
        .into_iter()
        .filter(|(_, _, dir, _)| root.join(dir).is_dir())
        .map(|(suite, engine, dir, stem)| {
            // Expected outputs of the two-pass run: format dump log, then the main run.
            let expected = [
                ("pass1.log", format!("{stem}in.log")),
                ("log", format!("{stem}.log")),
                ("typ", format!("{stem}.typ")),
                ("fot", format!("{stem}.fot")),
                ("pl", format!("{stem}.pl")),
            ]
            .into_iter()
            .filter(|(_, f)| root.join(&dir).join(f).exists())
            .map(|(k, f)| (k.to_owned(), format!("{dir}/{f}")))
            .collect();
            TestCase {
                id: format!("{suite}/{stem}"),
                suite: if suite == "trip" { "trip" } else { "etrip" },
                kind: Kind::Trip,
                input: format!("{dir}/{stem}.tex"),
                engines: vec![engine.to_owned()],
                expected,
                l3build: None,
            }
        })
        .collect()
}

/// Engine-related `*.test` scripts. Utilities (bibtex, tftopl, mf, …) are skipped.
fn web2c_tests(root: &Path) -> Result<Vec<TestCase>> {
    let base = root.join("upstream/texlive-source/texk/web2c");
    let dirs = [
        ("tests", "tex"),
        ("etexdir", "etex"),
        ("pdftexdir", "pdftex"),
        ("xetexdir", "xetex"),
        ("luatexdir", "luatex"),
    ];
    let mut out = Vec::new();
    for (dir, engine) in dirs {
        for entry in WalkDir::new(base.join(dir)).sort_by_file_name() {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "test") {
                continue;
            }
            let name = path.file_stem().unwrap().to_string_lossy();
            if dir == "tests" && name.starts_with("bibtex") || name == "etriptest" {
                continue;
            }
            out.push(TestCase {
                id: format!("web2c/{}", rel(&base, path).trim_end_matches(".test")),
                suite: "web2c",
                kind: Kind::ShellTest,
                input: rel(root, path),
                engines: vec![engine.to_owned()],
                expected: BTreeMap::new(),
                l3build: None,
            });
        }
    }
    Ok(out)
}

fn l3build_tests(root: &Path, suite: &'static str) -> Result<Vec<TestCase>> {
    let mut out = Vec::new();
    // Every directory with a build.lua is an l3build module.
    for entry in WalkDir::new(root.join("upstream").join(suite)).sort_by_file_name() {
        let entry = entry?;
        if entry.file_name() != "build.lua" {
            continue;
        }
        let module = entry.path().parent().unwrap();
        let (configs, maindir) = l3_configs(module)?;
        for cfg in configs {
            let dir = module.join(&cfg.testfiledir);
            if !dir.is_dir() {
                continue;
            }
            // l3build: testdir = maindir/build/test, suffixed with the config name.
            let mut testdir = module.join(&maindir).join("build/test");
            if cfg.name != "build" {
                testdir.as_mut_os_string().push(format!("-{}", cfg.name));
            }
            let run = |root: &Path| L3Run {
                module: rel(root, module),
                config: cfg.name.clone(),
                testdir: normalize(&rel(root, &testdir)),
                stdengine: cfg.stdengine.clone(),
                checked: cfg.checked,
            };
            out.extend(l3_dir_tests(root, suite, &dir, &cfg, run)?);
        }
    }
    Ok(out)
}

fn l3_dir_tests(
    root: &Path,
    suite: &'static str,
    dir: &Path,
    cfg: &L3Config,
    run: impl Fn(&Path) -> L3Run,
) -> Result<Vec<TestCase>> {
    let mut out = Vec::new();
    let mut files: Vec<PathBuf> = fs::read_dir(dir)?
        .map(|e| e.map(|e| e.path()))
        .collect::<Result<_, _>>()?;
    files.sort();
    for path in &files {
        let kind = match path.extension().and_then(|e| e.to_str()) {
            Some("lvt") => Kind::Lvt,
            Some("pvt") => Kind::Pvt,
            _ => continue,
        };
        let stem = path.file_stem().unwrap().to_string_lossy();
        let expect_ext = if matches!(kind, Kind::Pvt) {
            "tpf"
        } else {
            "tlg"
        };

        let mut expected = BTreeMap::new();
        let default = dir.join(format!("{stem}.{expect_ext}"));
        if default.exists() {
            expected.insert("*".to_owned(), rel(root, &default));
        }
        for e in &cfg.engines {
            let p = dir.join(format!("{stem}.{e}.{expect_ext}"));
            if p.exists() {
                expected.insert(e.clone(), rel(root, &p));
            }
        }
        // l3build's `stdengine` owns the unsuffixed file; keep it recorded for clarity.
        if expected.contains_key("*") && !expected.contains_key(&cfg.stdengine) {
            expected.insert(cfg.stdengine.clone(), expected["*"].clone());
            expected.remove("*");
        }

        out.push(TestCase {
            id: format!("{suite}/{}", rel(&root.join("upstream").join(suite), path)),
            suite,
            kind,
            input: rel(root, path),
            engines: cfg.engines.clone(),
            expected,
            l3build: Some(run(root)),
        });
    }
    Ok(out)
}

/// Resolve the l3build configs of a module: the main `build.lua` (plus the shared
/// `build-config.lua` if any) and each `config-*.lua` overriding it.
/// Also returns the module's `maindir`.
fn l3_configs(module: &Path) -> Result<(Vec<L3Config>, String)> {
    let mut base_src = fs::read_to_string(module.join("build.lua"))?;
    for anc in module.ancestors().skip(1).take(3) {
        if let Ok(s) = fs::read_to_string(anc.join("build-config.lua")) {
            base_src.insert_str(0, &s);
            break;
        }
    }
    // l3build accepts entries with or without the `.lua` suffix.
    let checkconfigs: Vec<String> = lua_string_list(&base_src, "checkconfigs")
        .unwrap_or_else(|| vec!["build".to_owned()])
        .into_iter()
        .map(|c| c.trim_end_matches(".lua").to_owned())
        .collect();
    let maindir = lua_string(&base_src, "maindir").unwrap_or_else(|| ".".to_owned());
    let base = L3Config {
        name: "build".to_owned(),
        checked: checkconfigs.iter().any(|c| c == "build"),
        testfiledir: lua_string(&base_src, "testfiledir")
            .unwrap_or_else(|| DEFAULT_TESTFILEDIR.to_owned()),
        engines: lua_string_list(&base_src, "checkengines")
            .unwrap_or_else(|| DEFAULT_ENGINES.iter().map(|s| (*s).to_owned()).collect()),
        stdengine: lua_string(&base_src, "stdengine")
            .unwrap_or_else(|| DEFAULT_STDENGINE.to_owned()),
    };

    let mut configs = Vec::new();
    let mut names: Vec<PathBuf> = fs::read_dir(module)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.extension().is_some_and(|e| e == "lua")
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("config-"))
        })
        .collect();
    names.sort();
    for path in names {
        let src = fs::read_to_string(&path)?;
        let Some(dir) = lua_string(&src, "testfiledir") else {
            continue;
        };
        let name = path.file_stem().unwrap().to_string_lossy().into_owned();
        configs.push(L3Config {
            checked: checkconfigs.contains(&name),
            name,
            testfiledir: dir.trim_start_matches("./").to_owned(),
            engines: lua_string_list(&src, "checkengines").unwrap_or_else(|| base.engines.clone()),
            stdengine: lua_string(&src, "stdengine").unwrap_or_else(|| base.stdengine.clone()),
        });
    }
    configs.insert(0, base);
    Ok((configs, maindir))
}

/// `name = "value"` on a non-comment line.
fn lua_string(src: &str, name: &str) -> Option<String> {
    let re = Regex::new(&format!(r#"(?m)^[ \t]*{name}\s*=\s*"([^"]*)""#)).ok()?;
    re.captures(src).map(|c| c[1].to_owned())
}

/// `name = {"a", "b"}` (possibly spanning lines) on a non-comment line.
fn lua_string_list(src: &str, name: &str) -> Option<Vec<String>> {
    static ITEM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""([^"]*)""#).unwrap());
    let re = Regex::new(&format!(r"(?ms)^[ \t]*{name}\s*=\s*\{{(.*?)\}}")).ok()?;
    let body = re.captures(src)?.get(1)?.as_str().to_owned();
    // Drop Lua line comments inside the table.
    let body: String = body
        .lines()
        .map(|l| l.split("--").next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n");
    Some(ITEM.captures_iter(&body).map(|c| c[1].to_owned()).collect())
}

/// Drop `.` and resolve `..` components lexically (`a/b/../build` -> `a/build`).
fn normalize(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for p in path.split('/') {
        match p {
            "" | "." => {}
            ".." if parts.last().is_some_and(|l| *l != "..") => {
                parts.pop();
            }
            _ => parts.push(p),
        }
    }
    parts.join("/")
}

fn rel(base: &Path, path: &Path) -> String {
    path.strip_prefix(base)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}
