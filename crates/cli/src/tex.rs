//! TeX Live as the builds see it: kpathsea, the finder a `Build` is given
//! (the engines by their program names are `phitex_live::tex`'s):
//! `\input` files by TEXINPUTS (from the environment, `.` first as
//! texmf.cnf has it), TFM files, map files, fonts and encodings, each a
//! lookup in the `ls-R` databases, no directory walked.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use phitex_kpse::{Format, Kpse};

use phitex_live::tex::samples;
pub use phitex_live::tex::{
    ANY_BYTES, Engine, Fnv, Output, chars, chars_owned, engine, fingerprint,
};

/// kpathsea as program `progname` of `engine`, finding TeX Live as the
/// user's own `tex` does: `SELFAUTOLOC` is where the executable lives; if
/// no tree resolves from there (a development build), where `tex` on
/// `PATH` is. A `tex` that is a wrapper giving the real one its
/// `TEXMFCNF` (nix's TeX Live: a script in the environment's `bin/`, its
/// `share/texmf/web2c` holding the unpatched `texmf.cnf` whose
/// `TEXMFROOT` is `$SELFAUTOPARENT`, `/nix/store`) is asked for it
/// through its `kpsewhich`, wrapped alike ([`wrapped_texmfcnf`]).
pub fn kpse(progname: &str, engine: &str) -> Kpse {
    let exe = std::env::current_exe()
        .ok()
        .and_then(|e| e.canonicalize().ok());
    let path = std::env::var_os("PATH");
    let cnf_set = std::env::var_os("TEXMFCNF").is_some_and(|v| !v.is_empty());
    kpse_in(
        exe.as_deref(),
        path.as_deref(),
        (!cnf_set).then(cache_dir).as_deref(),
        progname,
        engine,
    )
}

/// [`kpse`] with executable `exe`, search path `path`, and `cache` where
/// a wrapper's answer is kept (`None`: no wrapper asked, the user's
/// `TEXMFCNF` set).
fn kpse_in(
    exe: Option<&Path>,
    path: Option<&std::ffi::OsStr>,
    cache: Option<&Path>,
    progname: &str,
    engine: &str,
) -> Kpse {
    let own = exe.and_then(Path::parent);
    if let Some(dir) = own {
        let mut k = Kpse::new(dir, progname, engine);
        if resolves(&mut k) {
            return k;
        }
    }
    // (`tex` on PATH as the shell finds it, a link or a wrapper, and the
    // file it is)
    let tex = path.and_then(|p| {
        std::env::split_paths(p)
            .map(|d| d.join(format!("tex{}", std::env::consts::EXE_SUFFIX)))
            .filter_map(|t| Some((t.canonicalize().ok()?, t)))
            .find(|(c, _)| c.is_file() && Some(c.as_path()) != exe)
    });
    let Some((real, tex)) = tex else {
        return Kpse::new(own.unwrap_or(Path::new("/usr/bin")), progname, engine);
    };
    let dir = real.parent().unwrap_or(Path::new("/usr/bin"));
    let mut k = Kpse::new(dir, progname, engine);
    if resolves(&mut k) {
        return k;
    }
    if let Some(cnf) = cache.and_then(|c| wrapped_texmfcnf(&tex, &real, c)) {
        let mut w = Kpse::new(dir, progname, engine);
        w.set_var("TEXMFCNF", &cnf);
        if resolves(&mut w) {
            return w;
        }
    }
    k
}

/// Whether kpathsea `k` finds a tree: a `texmf.cnf` naming `TEXMF`, and
/// its `TEXMFDIST`, where it names one, a directory. (A `texmf.cnf`
/// found by its default places but meant for another root names trees
/// that are not there: nothing resolves, "can't find pdflatex.ini".)
fn resolves(k: &mut Kpse) -> bool {
    if k.var_value("TEXMF").is_none() {
        return false;
    }
    k.var_value("TEXMFDIST")
        .is_none_or(|d| d.is_empty() || os(&d).is_dir())
}

/// The `TEXMFCNF` the `kpsewhich` beside wrapper `tex` (as `PATH` names
/// it; `real` the file it is) runs with, the wrapper's own default (the
/// environment's `TEXMFCNF` unset for it): asked once, kept in `cache`
/// by the wrapper's file and its time. `None`: no `kpsewhich` there, or
/// no answer.
fn wrapped_texmfcnf(tex: &Path, real: &Path, cache: &Path) -> Option<String> {
    let kw = tex.with_file_name(format!("kpsewhich{}", std::env::consts::EXE_SUFFIX));
    if !kw.is_file() {
        return None;
    }
    let mtime = std::fs::metadata(real)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos());
    let key = Fnv::new()
        .add(real.as_os_str().as_encoded_bytes())
        .add(kw.as_os_str().as_encoded_bytes())
        .add(&mtime.to_le_bytes())
        .0;
    let file = cache.join("texlive").join(format!("texmfcnf-{key:016x}"));
    if let Ok(v) = std::fs::read_to_string(&file)
        && !v.is_empty()
    {
        return Some(v);
    }
    let out = std::process::Command::new(&kw)
        .arg("-var-value=TEXMFCNF")
        .env_remove("TEXMFCNF")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    let v = String::from_utf8(out.stdout).ok()?.trim_end().to_owned();
    if v.is_empty() {
        return None;
    }
    // (kept for the next run: written whole, then put in place)
    if let Some(d) = file.parent()
        && std::fs::create_dir_all(d).is_ok()
    {
        let tmp = file.with_extension(format!("tmp{}", std::process::id()));
        if std::fs::write(&tmp, &v)
            .and_then(|()| std::fs::rename(&tmp, &file))
            .is_err()
        {
            let _ = std::fs::remove_file(&tmp);
        }
    }
    Some(v)
}

/// Whether this machine has a TeX Live kpathsea finds (a `texmf.cnf`
/// naming `TEXMF`, its trees there), as [`kpse`] looks for it.
pub fn has_texlive() -> bool {
    resolves(&mut kpse("pdftex", "pdftex"))
}

/// [`fingerprint`] of the file at `p`, reading only what it needs.
pub fn fingerprint_file(p: &std::path::Path) -> Option<u64> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(p).ok()?;
    let len = usize::try_from(f.metadata().ok()?.len()).ok()?;
    if len <= 1 << 20 {
        let mut b = Vec::with_capacity(len);
        f.read_to_end(&mut b).ok()?;
        return Some(fingerprint(&b));
    }
    let mut h = Fnv::new().add(&(len as u64).to_le_bytes());
    let mut buf = Vec::new();
    for (at, n) in samples(len) {
        buf.resize(n, 0);
        f.seek(SeekFrom::Start(at as u64)).ok()?;
        f.read_exact(&mut buf).ok()?;
        h = h.add(&buf);
    }
    Some(h.0)
}

/// Where a run's TeX files come from (`files` in `phitex.toml`, the
/// editor's setting, `--shelf`): one source a run, never one after the
/// other.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Files {
    /// TeX Live, by kpathsea.
    #[default]
    TexLive,
    /// Shelf's packs (`crate::shelf`).
    Shelf,
}

impl Files {
    /// `texlive` or `shelf`.
    pub fn of(name: &str) -> Result<Files, String> {
        match name {
            "texlive" => Ok(Files::TexLive),
            "shelf" => Ok(Files::Shelf),
            n => Err(format!("files = \"{n}\": texlive or shelf")),
        }
    }
}

/// The cache directory: `PHITEX_CACHE_DIR`, else `$XDG_CACHE_HOME/phitex`,
/// else `~/.cache/phitex` (Windows: `%LOCALAPPDATA%\phitex\cache`,
/// [`crate::dirs`]), else the temporary directory's. Where that one
/// cannot be written (a sandbox that keeps the home read-only), the
/// project's: `.phitex/cache` in the folder phitex runs in, so that its
/// format and records last from run to run ([`local_cache`]).
pub fn cache_dir() -> PathBuf {
    static DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    if let Some(d) = std::env::var_os("PHITEX_CACHE_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(d);
    }
    DIR.get_or_init(|| {
        let d = crate::dirs::phitex(crate::dirs::Kind::Cache)
            .unwrap_or_else(|| std::env::temp_dir().join("phitex-cache"));
        if writable(&d) {
            return d;
        }
        std::env::current_dir()
            .ok()
            .and_then(|here| local_cache(&here))
            .unwrap_or(d)
    })
    .clone()
}

/// Whether files can be made in folder `d` (made if missing).
fn writable(d: &Path) -> bool {
    let probe = d.join(format!(".probe-{}", std::process::id()));
    let ok = std::fs::create_dir_all(d)
        .and_then(|()| std::fs::write(&probe, b""))
        .is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

/// The project's own cache in `here`, `.phitex/cache` (`.phitex/` kept
/// out of git, [`crate::review::make_dir`]), if it can be written.
fn local_cache(here: &Path) -> Option<PathBuf> {
    let d = here.join(".phitex").join("cache");
    if !writable(&d) {
        return None;
    }
    let _ = crate::review::make_dir(here);
    Some(d)
}

/// Whether the cache is the project's ([`cache_dir`]'s last resort).
pub fn cache_is_local() -> bool {
    std::env::var_os("PHITEX_CACHE_DIR").is_none_or(|d| d.is_empty())
        && cache_dir().ends_with(Path::new(".phitex").join("cache"))
}

/// TeX Live for one program name: the finder a build is given.
pub struct Tree {
    /// kpathsea as the engine's program (its `progname`, `$engine`).
    kpse: Mutex<Kpse>,
    /// The glyph settings kpathsea was last given (`kpse_init_prog`).
    glyph: Mutex<Option<(u32, Option<String>)>>,
}

impl Tree {
    /// The tree for engine `e`: kpathsea as its program, made once (its
    /// `texmf.cnf` and `ls-R` read at the first lookup), with the search
    /// variables `vars` set over the environment's (a `.latexmkrc`'s
    /// TEXINPUTS, as latexmk sets it for the engine it runs).
    pub fn open(e: &Engine, vars: &[(String, String)]) -> Tree {
        let mut k = kpse(e.name, e.kpse_engine);
        for (name, value) in vars {
            k.set_var(name, value);
        }
        Tree {
            kpse: Mutex::new(k),
            glyph: Mutex::new(None),
        }
    }

    /// The tree a format of engine `e` is built with: its search paths
    /// without the current directory, as fmtutil runs the build in a
    /// scratch directory (a document named `plain.tex` is not the
    /// format's `plain.tex`).
    pub fn open_format(e: &Engine) -> Tree {
        let mut k = kpse(e.name, e.kpse_engine);
        for f in FORMATS {
            k.drop_cwd(f);
        }
        Tree {
            kpse: Mutex::new(k),
            glyph: Mutex::new(None),
        }
    }

    /// Where the file a finder's key `key` names is
    /// ([`phitex_tex::Finder`]): the font side's by [`Tree::font`], TeX's
    /// own by [`Tree::input`].
    pub fn find(&self, key: &str) -> Option<PathBuf> {
        match phitex_tex::font_name(key) {
            Some(name) => self.font(name),
            None => self.input(key),
        }
    }

    /// Where TeX's file `name` is (`\\input`, `\\openin`): kpathsea's
    /// `tex` format whatever its extension (`kpse_find_tex`).
    pub fn input(&self, name: &str) -> Option<PathBuf> {
        let mut k = self.kpse.lock().unwrap_or_else(PoisonError::into_inner);
        k.find_file(name.as_bytes(), Format::Tex, false)
            .map(|b| os(&b).to_path_buf())
    }

    /// Where the font side's file `name` is, by kpathsea in the format
    /// its extension names (pdfTeX's choices: a PK font, a TFM, a VF, a
    /// map, a Type 1 font, an encoding, an OpenType or TrueType font).
    pub fn font(&self, name: &str) -> Option<PathBuf> {
        if let Some((font, dpi)) = phitex_kpse::pk_name(name) {
            return self.pk(font, dpi, phitex_kpse::pk_settings(name));
        }
        let ext = Path::new(name)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");
        let format = match ext.to_ascii_lowercase().as_str() {
            "tfm" => Format::Tfm,
            "vf" => Format::Vf,
            "map" => Format::FontMap,
            "pfb" | "pfa" => Format::Type1,
            "enc" => Format::Enc,
            "otf" => Format::OpenType,
            "ttf" | "ttc" => Format::TrueType,
            _ => Format::Tex,
        };
        let mut k = self.kpse.lock().unwrap_or_else(PoisonError::into_inner);
        k.find_file(name.as_bytes(), format, false)
            .map(|b| os(&b).to_path_buf())
    }
}

/// kpathsea's variables naming the trees TeX Live installed, or made
/// itself (its formats, its maps, its configuration): the distribution, as
/// a watch knows it ([`Tree::distribution`]). TEXMFHOME, the user's own
/// files, is not among them.
const DISTRIBUTION_VARS: [&str; 8] = [
    "TEXMFROOT",
    "TEXMFDIST",
    "TEXMFMAIN",
    "TEXMFLOCAL",
    "TEXMFSYSVAR",
    "TEXMFSYSCONFIG",
    "TEXMFVAR",
    "TEXMFCONFIG",
];

impl Tree {
    /// The folders of the distribution's trees ([`DISTRIBUTION_VARS`]),
    /// as named and canonical, and the cache's (the fonts the engine
    /// made).
    pub fn distribution(&self) -> Vec<PathBuf> {
        let mut k = self.kpse.lock().unwrap_or_else(PoisonError::into_inner);
        let mut out: Vec<PathBuf> = vec![cache_dir()];
        for var in DISTRIBUTION_VARS {
            let Some(v) = k.var_value(var) else { continue };
            let v = String::from_utf8_lossy(&v).replace("!!", "");
            for d in v.split([',', ':', ';', '{', '}']) {
                if d.starts_with('/') {
                    out.push(PathBuf::from(d));
                }
            }
        }
        // (each as named and canonical: a path kpathsea found is under the
        // first, one a link led to under the second)
        let canon: Vec<PathBuf> = out.iter().filter_map(|d| d.canonicalize().ok()).collect();
        out.extend(canon);
        out.sort();
        out.dedup();
        out
    }
}

impl Tree {
    /// Where font `font`'s PK file at `dpi` dots per inch is (the name
    /// `cmr10.600pk` a PK font's lookup asks for, [`phitex_kpse::pk_name`]):
    /// kpathsea's PK search, `kpse_find_pk` ([`Kpse::find_pk`]) with the
    /// glyph settings `glyph` the writer gave (`kpse_init_prog`'s base
    /// resolution and mode, [`phitex_kpse::pk_settings`]); a miss is the
    /// engine's to make (mktexpk, `phitex_mf::mktex::make`).
    pub fn pk(&self, font: &str, dpi: u32, glyph: Option<(u32, Option<&str>)>) -> Option<PathBuf> {
        let mut k = self.kpse.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((bdpi, mode)) = glyph {
            let mut g = self.glyph.lock().unwrap_or_else(PoisonError::into_inner);
            let want = (bdpi, mode.map(str::to_string));
            if g.as_ref() != Some(&want) {
                k.init_prog(bdpi, mode.map(str::as_bytes));
                *g = Some(want);
            }
        }
        k.find_pk(font.as_bytes(), dpi)
            .map(|(p, _)| os(&p).to_path_buf())
    }

    /// Where a tool's support file `name` is, by kpathsea in the format
    /// of its search (`bst`, `bib`, `ist`, which add their suffixes).
    pub fn tool_input(&self, search: phitex_tex::ToolSearch, name: &str) -> Option<PathBuf> {
        let format = match search {
            phitex_tex::ToolSearch::BibStyle => Format::Bst,
            phitex_tex::ToolSearch::BibData => Format::Bib,
            phitex_tex::ToolSearch::IndexStyle => Format::Ist,
            // (Ghostscript's own fonts, where the system's gs keeps them,
            // as TeX Live's epstopdf runs the system's gs)
            phitex_tex::ToolSearch::GsFont => return gs_font(name),
            // (METAFONT's and web2c's files as `mf-nowin -progname=mf`
            // finds them, for the fonts the engine makes)
            phitex_tex::ToolSearch::Mf => {
                return crate::mkfont::find(name, phitex_mf::mktex::Kind::Mf);
            }
            phitex_tex::ToolSearch::Web2c => {
                return crate::mkfont::find(name, phitex_mf::mktex::Kind::Web2c);
            }
        };
        let mut k = self.kpse.lock().unwrap_or_else(PoisonError::into_inner);
        k.find_file(name.as_bytes(), format, false)
            .map(|b| os(&b).to_path_buf())
    }
}

/// A font of the system Ghostscript's `Resource/Font` by its file name.
pub fn gs_font(name: &str) -> Option<PathBuf> {
    if name.contains('/') {
        return None;
    }
    GS_FONT_DIRS
        .iter()
        .map(|d| Path::new(d).join(name))
        .find(|p| p.is_file())
}

/// Where a system's Ghostscript keeps its `Resource/Font`.
const GS_FONT_DIRS: [&str; 3] = [
    "/usr/share/ghostscript/Resource/Font",
    "/usr/share/ghostscript/10.08.0/Resource/Font",
    "/usr/local/share/ghostscript/Resource/Font",
];

/// The formats [`Tree::input`] looks files up in.
const FORMATS: [Format; 8] = [
    Format::Tfm,
    Format::Vf,
    Format::FontMap,
    Format::Type1,
    Format::Enc,
    Format::OpenType,
    Format::TrueType,
    Format::Tex,
];

pub(crate) fn os(b: &[u8]) -> std::borrow::Cow<'_, Path> {
    phitex_kpse::path_of(b)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// A TeX Live as nix's `texlive.withPackages` makes it (issue #58):
    /// `env/bin/tex` and `env/bin/kpsewhich` scripts that set
    /// `TEXMFCNF` (unless set) to the patched `env/share/texmf-var/web2c`,
    /// `env/share/texmf` a link to the distribution whose `web2c` holds
    /// kpathsea's own `texmf.cnf` (`TEXMFROOT = $SELFAUTOPARENT`, the
    /// store), and a profile's `bin/` linking to them.
    #[test]
    fn a_wrapped_tex_finds_its_own_tree() {
        use std::os::unix::fs::{PermissionsExt as _, symlink};
        let root = std::env::temp_dir().join(format!("phitex-wrapped-tl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let store = root.join("store");
        let (env, dist) = (store.join("env"), store.join("dist"));
        let patched = env.join("share/texmf-var/web2c");
        for d in [
            env.join("bin"),
            env.join("share"),
            patched.clone(),
            dist.join("web2c"),
            dist.join("tex/latex/hello"),
            root.join("profile/bin"),
            root.join("cache"),
        ] {
            std::fs::create_dir_all(d).unwrap();
        }
        symlink(&dist, env.join("share/texmf")).unwrap();
        let tree =
            "TEXMF = {!!$TEXMFDIST}\nTEXMFDBS = {!!$TEXMFDIST}\nTEXINPUTS = .;$TEXMF/tex//\n";
        std::fs::write(
            dist.join("web2c/texmf.cnf"),
            format!("TEXMFROOT = $SELFAUTOPARENT\nTEXMFDIST = $TEXMFROOT/texmf-dist\n{tree}"),
        )
        .unwrap();
        std::fs::write(
            patched.join("texmf.cnf"),
            format!("TEXMFDIST = {}\n{tree}", dist.display()),
        )
        .unwrap();
        std::fs::write(
            dist.join("ls-R"),
            "% ls-R -- filename database for kpathsea; do not change this line.\n\
             ./:\nweb2c\ntex\n\n./tex:\nlatex\n\n./tex/latex:\nhello\n\n./tex/latex/hello:\nhello.sty\n",
        )
        .unwrap();
        std::fs::write(dist.join("tex/latex/hello/hello.sty"), "").unwrap();
        let script = |name: &str, body: &str| {
            let p = env.join("bin").join(name);
            std::fs::write(
                &p,
                format!(
                    "#!/bin/sh\nexport TEXMFCNF=${{TEXMFCNF-'{}'}}\n{body}\n",
                    patched.display()
                ),
            )
            .unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
            symlink(&p, root.join("profile/bin").join(name)).unwrap();
        };
        script("tex", "exit 0");
        script(
            "kpsewhich",
            "[ \"$1\" = -var-value=TEXMFCNF ] && echo \"$TEXMFCNF\"",
        );
        let path = root.join("profile/bin");
        let cache = root.join("cache");
        let find = |k: &mut Kpse| k.find_file(b"hello.sty", Format::Tex, false);
        // (by `tex`'s place alone: kpathsea's own texmf.cnf, its trees
        // under the store's parent, none there)
        let mut k = kpse_in(None, Some(path.as_os_str()), None, "pdftex", "pdftex");
        assert!(!resolves(&mut k));
        assert_eq!(find(&mut k), None);
        // (the wrapper asked: its texmf.cnf, the distribution found)
        let mut k = kpse_in(
            None,
            Some(path.as_os_str()),
            Some(&cache),
            "pdftex",
            "pdftex",
        );
        assert!(resolves(&mut k));
        assert_eq!(
            k.var_value("TEXMFDIST").map(|d| os(&d).into_owned()),
            Some(dist.clone())
        );
        assert_eq!(
            find(&mut k).map(|f| os(&f).into_owned()),
            Some(dist.join("tex/latex/hello/hello.sty"))
        );
        // (asked once: the answer kept by the wrapper's file and time)
        std::fs::write(env.join("bin/kpsewhich"), "#!/bin/sh\nexit 1\n").unwrap();
        let mut k = kpse_in(
            None,
            Some(path.as_os_str()),
            Some(&cache),
            "pdftex",
            "pdftex",
        );
        assert!(resolves(&mut k));
        let _ = std::fs::remove_dir_all(&root);
    }
}
