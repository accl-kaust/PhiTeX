//! `partex -makeindex [options] IDX...` (or partex invoked as `makeindex`):
//! `MakeIndex`, in process (`partex-makeindex`), with kpathsea's style
//! lookup, writing the `.ind` and `.ilg` files and the terminal output as
//! `makeindex` would.

use std::collections::HashMap;
use std::io::{Read, Write};

use partex_kpse::{Format, Kpse};
use partex_makeindex::Files;

/// Whether this invocation asks for makeindex.
pub fn wanted() -> bool {
    let argv0 = std::env::args().next().unwrap_or_default();
    std::path::Path::new(&argv0)
        .file_stem()
        .is_some_and(|s| s == "makeindex")
        || std::env::args()
            .nth(1)
            .is_some_and(|a| a == "-makeindex" || a == "--makeindex")
}

/// The banner's version: makeindex 2.18 as TeX Live builds it.
fn version() -> Vec<u8> {
    let suffix = env!("PARTEX_ORACLE_VERSION_SUFFIX");
    let year: String = suffix
        .split_once("TeX Live ")
        .map(|(_, r)| r.chars().take_while(char::is_ascii_digit).collect())
        .unwrap_or_default();
    format!("version 2.18 [TeX Live {year}] (kpathsea + Thai support)").into_bytes()
}

/// What a run looked up, and what it found.
#[derive(Clone, PartialEq, Eq)]
enum Lookup {
    Read(Vec<u8>, Option<Vec<u8>>),
    Readable(Vec<u8>, bool),
    Style(Vec<u8>, Option<(Vec<u8>, Option<Vec<u8>>)>),
}

/// kpathsea's style lookup; other files as named.
pub struct KpseFiles {
    kpse: Kpse,
    /// Every lookup made.
    read: Vec<Lookup>,
    /// The directory the run acts as if started in (`cd dir && makeindex
    /// job.idx`, as latexmk runs it for an output directory): relative
    /// names are in it.
    dir: Option<Vec<u8>>,
}

fn read_file(name: &[u8]) -> Option<Vec<u8>> {
    let p = crate::native::path(name);
    if std::fs::metadata(&p).is_ok_and(|m| m.is_dir()) {
        // (`fopen` opens a directory; reading it gives nothing)
        return Some(Vec::new());
    }
    std::fs::read(&p).ok()
}

fn readable(name: &[u8]) -> bool {
    std::fs::metadata(crate::native::path(name)).is_ok()
}

impl KpseFiles {
    /// Where `name` is, relative names being in [`KpseFiles::dir`].
    fn at(&self, name: &[u8]) -> Vec<u8> {
        match &self.dir {
            Some(d) if name.first() != Some(&b'/') => [d.as_slice(), name].concat(),
            _ => name.to_vec(),
        }
    }

    fn find_style(&mut self, name: &[u8]) -> Option<(Vec<u8>, Option<Vec<u8>>)> {
        let found = self.kpse.find_file(name, Format::Ist, true)?;
        let data = read_file(&found);
        Some((found, data))
    }

    /// Whether every lookup in `read` finds the same now.
    fn unchanged(&mut self, read: &[Lookup]) -> bool {
        read.iter().all(|l| match l {
            Lookup::Read(n, was) => read_file(n) == *was,
            Lookup::Readable(n, was) => readable(n) == *was,
            Lookup::Style(n, was) => self.find_style(n) == *was,
        })
    }

    /// kpathsea's `kpse_out_name_ok` for `openout_any`.
    fn out_ok(&mut self, name: &[u8]) -> Result<(), Vec<u8>> {
        let choice = self
            .kpse
            .var_value("openout_any")
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| b"p".to_vec());
        if matches!(choice[0], b'a' | b'y' | b'1') {
            return Ok(());
        }
        let base = name.rsplit(|&c| c == b'/').next().unwrap_or(name);
        let dotfile =
            base.first() == Some(&b'.') && base != b"." && base != b".." && base != b".tex";
        let paranoid = !matches!(choice[0], b'r' | b'n' | b'0');
        let absolute = name.first() == Some(&b'/')
            && self
                .kpse
                .var_value("TEXMFOUTPUT")
                .is_none_or(|t| t.is_empty() || !name.starts_with(&t));
        let dotdot = name.split(|&c| c == b'/').any(|c| c == b"..");
        if dotfile || paranoid && (absolute || dotdot) {
            let mut msg = b"\nmakeindex: Not writing to ".to_vec();
            msg.extend_from_slice(name);
            msg.extend_from_slice(b" (openout_any = ");
            msg.extend_from_slice(&choice);
            msg.extend_from_slice(b"; no extended check).\n");
            return Err(msg);
        }
        Ok(())
    }
}

impl Files for KpseFiles {
    fn read(&mut self, name: &[u8]) -> Option<Vec<u8>> {
        let name = self.at(name);
        let d = read_file(&name);
        self.read.push(Lookup::Read(name, d.clone()));
        d
    }

    fn readable(&mut self, name: &[u8]) -> bool {
        let name = self.at(name);
        let r = readable(&name);
        self.read.push(Lookup::Readable(name, r));
        r
    }

    fn style(&mut self, name: &[u8]) -> Option<(Vec<u8>, Option<Vec<u8>>)> {
        let s = self.find_style(name);
        self.read.push(Lookup::Style(name.to_vec(), s.clone()));
        s
    }

    fn out_name_ok(&mut self, name: &[u8]) -> Result<(), Vec<u8>> {
        self.out_ok(name)
    }

    fn stdin(&mut self) -> Vec<u8> {
        let mut v = Vec::new();
        let _ = std::io::stdin().read_to_end(&mut v);
        v
    }
}

/// Write the files a run produced; an error line if one can't be.
fn write_outputs(out: &partex_makeindex::Outcome, files: &KpseFiles) -> Option<String> {
    for f in [&out.ind, &out.ilg].into_iter().flatten() {
        if let Err(e) = std::fs::write(crate::native::path(&files.at(&f.name)), &f.contents) {
            return Some(format!(
                "makeindex: can't write {}: {e}",
                String::from_utf8_lossy(&f.name)
            ));
        }
    }
    None
}

/// makeindex runs made by a converging build.
#[derive(Default)]
pub struct Runs {
    files: Option<KpseFiles>,
    /// For each `.idx` file, the lookups its last run made.
    last: HashMap<Vec<u8>, Vec<Lookup>>,
}

/// After a pass of a converging build (its outputs written): run
/// makeindex on each `.idx` file of the job, as `makeindex job.idx`,
/// unless what its last run read is unchanged. A report line for each run.
pub fn after_pass(runs: &mut Runs, idxes: &[(Vec<u8>, Vec<u8>)]) -> Vec<String> {
    let mut reports = Vec::new();
    for (name, _) in idxes {
        let files = runs.files.get_or_insert_with(|| KpseFiles {
            kpse: crate::kpse_instance("makeindex", ""),
            read: Vec::new(),
            dir: None,
        });
        let slash = name.iter().rposition(|&c| c == b'/');
        files.dir = slash.map(|i| name[..=i].to_vec());
        let base = slash.map_or(name.as_slice(), |i| &name[i + 1..]).to_vec();
        if let Some(read) = runs.last.get(name)
            && files.unchanged(read)
        {
            continue;
        }
        let t0 = std::time::Instant::now();
        let _p = crate::timeline::phase("makeindex").map(|mut p| {
            p.detail(String::from_utf8_lossy(name));
            p
        });
        files.read.clear();
        let out = partex_makeindex::run(&[base], &version(), files);
        runs.last
            .insert(name.clone(), std::mem::take(&mut files.read));
        reports.extend(write_outputs(&out, files));
        let last_line = crate::native::tool_summary(&out.stderr, out.status != 0);
        reports.push(format!(
            "partex: makeindex {} in {:.1} ms: {}",
            String::from_utf8_lossy(name),
            t0.elapsed().as_secs_f64() * 1e3,
            last_line
        ));
    }
    reports
}

/// Run as `makeindex`.
pub fn main() -> ! {
    use std::os::unix::ffi::OsStringExt;
    let mut args: Vec<Vec<u8>> = std::env::args_os()
        .skip(1)
        .map(OsStringExt::into_vec)
        .collect();
    if args
        .first()
        .is_some_and(|a| a == b"-makeindex" || a == b"--makeindex")
    {
        args.remove(0);
    }
    let mut files = KpseFiles {
        kpse: crate::kpse_instance("makeindex", ""),
        read: Vec::new(),
        dir: None,
    };
    let out = partex_makeindex::run(&args, &version(), &mut files);
    let err = write_outputs(&out, &files);
    let _ = std::io::stdout().write_all(&out.stdout);
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().write_all(&out.stderr);
    if let Some(e) = err {
        eprintln!("{e}");
        std::process::exit(1);
    }
    std::process::exit(out.status);
}
