//! D4: BibTeX and makeindex as functions inside the fixpoint
//! (`.aux` → `.bbl`, `.idx` → `.ind`), via phitex-bibtex / phitex-makeindex.
//!
//! [`derive`] is a pure function of the files the job wrote and the files
//! it reads (`.bst`, `.bib`, `.ist`, `\@input` aux files); it is memoised
//! on exactly those inputs: a cached result is reused when every file the
//! earlier run read reads the same now.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

type Written = [(Arc<str>, Arc<str>)];
type Lookup<'a> = &'a dyn Fn(&str) -> Option<Arc<[u8]>>;

/// One run's record: what it read (name → contents, `None` = absent) and
/// what it produced.
struct Run {
    reads: Vec<(String, Option<Arc<[u8]>>)>,
    out: Vec<(Arc<str>, Arc<str>)>,
}

/// Memo per (program, primary file name).
fn memo() -> &'static Mutex<HashMap<(u8, String), Run>> {
    static M: std::sync::OnceLock<Mutex<HashMap<(u8, String), Run>>> = std::sync::OnceLock::new();
    M.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Reads through `written` first, then `files`, recording every read.
struct Reader<'a> {
    written: &'a Written,
    files: Lookup<'a>,
    reads: Vec<(String, Option<Arc<[u8]>>)>,
}

impl Reader<'_> {
    fn get(&mut self, name: &str) -> Option<Arc<[u8]>> {
        if let Some((_, v)) = self.reads.iter().find(|(n, _)| n == name) {
            return v.clone();
        }
        let v = lookup(self.written, self.files, name);
        self.reads.push((name.to_owned(), v.clone()));
        v
    }
    fn get_vec(&mut self, name: &[u8]) -> Option<Vec<u8>> {
        let n = String::from_utf8_lossy(name).into_owned();
        self.get(&n).map(|b| b.to_vec())
    }
}

fn lookup(written: &Written, files: Lookup<'_>, name: &str) -> Option<Arc<[u8]>> {
    let bare = name.strip_prefix("./").unwrap_or(name);
    written
        .iter()
        .rev()
        .find(|(n, _)| n.strip_prefix("./").unwrap_or(n) == bare)
        .map(|(_, c)| Arc::<[u8]>::from(c.as_bytes()))
        .or_else(|| files(name))
}

impl phitex_bibtex::Files for Reader<'_> {
    fn aux(&mut self, name: &[u8]) -> Option<Vec<u8>> {
        self.get_vec(name)
    }
    fn bst(&mut self, name: &[u8]) -> Option<Vec<u8>> {
        let mut n = name.to_vec();
        n.extend_from_slice(b".bst");
        self.get_vec(&n)
    }
    fn bib(&mut self, name: &[u8]) -> Option<Vec<u8>> {
        let mut n = name.to_vec();
        if !n.ends_with(b".bib") {
            n.extend_from_slice(b".bib");
        }
        self.get_vec(&n)
    }
}

impl phitex_makeindex::Files for Reader<'_> {
    fn read(&mut self, name: &[u8]) -> Option<Vec<u8>> {
        self.get_vec(name)
    }
    fn readable(&mut self, name: &[u8]) -> bool {
        self.get_vec(name).is_some()
    }
    fn style(&mut self, name: &[u8]) -> Option<(Vec<u8>, Option<Vec<u8>>)> {
        let mut n = name.to_vec();
        if !n.ends_with(b".ist") {
            n.extend_from_slice(b".ist");
        }
        self.get_vec(&n).map(|c| (n, Some(c)))
    }
    fn out_name_ok(&mut self, _name: &[u8]) -> Result<(), Vec<u8>> {
        Ok(())
    }
    fn stdin(&mut self) -> Vec<u8> {
        Vec::new()
    }
}

fn text(b: Vec<u8>) -> Arc<str> {
    Arc::from(String::from_utf8_lossy(&b).as_ref())
}

/// Run `f` for (`prog`, `key`) unless a memoised run read the same files.
fn memoised(
    prog: u8,
    key: &str,
    written: &Written,
    files: Lookup<'_>,
    f: impl FnOnce(&mut Reader<'_>) -> Vec<(Arc<str>, Arc<str>)>,
) -> Vec<(Arc<str>, Arc<str>)> {
    let k = (prog, key.to_owned());
    if let Ok(m) = memo().lock()
        && let Some(run) = m.get(&k)
        && run
            .reads
            .iter()
            .all(|(n, v)| lookup(written, files, n).as_deref() == v.as_deref())
    {
        return run.out.clone();
    }
    let mut r = Reader { written, files, reads: Vec::new() };
    let out = f(&mut r);
    if let Ok(mut m) = memo().lock() {
        m.insert(k, Run { reads: r.reads, out: out.clone() });
    }
    out
}

/// Does this `.aux` text ask for BibTeX?
fn wants_bibtex(aux: &str) -> bool {
    aux.contains("\\bibdata{") || aux.contains("\\bibstyle{") || aux.contains("\\citation{")
}

/// The derived files: for each `JOB.aux` the job wrote that asks for
/// BibTeX, `JOB.bbl` (and `JOB.blg`); for each `JOB.idx`, `JOB.ind` (and
/// `JOB.ilg`). Included aux files (`\@input`) are read by BibTeX through
/// the main one, so only aux files not `\@input` by another are run.
#[must_use]
pub fn derive(
    written: &[(Arc<str>, Arc<str>)],
    files: &dyn Fn(&str) -> Option<Arc<[u8]>>,
) -> Vec<(Arc<str>, Arc<str>)> {
    let mut out = Vec::new();
    let included: Vec<String> = written
        .iter()
        .filter(|(n, _)| n.ends_with(".aux"))
        .flat_map(|(_, c)| args_of(c, "\\@input"))
        .collect();
    let mut seen: Vec<&str> = Vec::new();
    for (name, contents) in written.iter().rev() {
        if seen.contains(&name.as_ref()) {
            continue;
        }
        seen.push(name);
        if let Some(job) = name.strip_suffix(".aux") {
            if included.iter().any(|i| i == name.as_ref()) || !wants_bibtex(contents) {
                continue;
            }
            out.extend(memoised(0, name, written, files, |r| {
                let o = phitex_bibtex::run(name.as_bytes(), &phitex_bibtex::Options::default(), r);
                let mut v = Vec::new();
                if let Some(b) = o.bbl {
                    v.push((Arc::from(format!("{job}.bbl")), text(b.contents)));
                }
                if let Some(b) = o.blg {
                    v.push((Arc::from(format!("{job}.blg")), text(b.contents)));
                }
                v
            }));
        } else if let Some(job) = name.strip_suffix(".idx") {
            out.extend(memoised(1, name, written, files, |r| {
                let args = vec![name.as_bytes().to_vec()];
                let o = phitex_makeindex::run(&args, b" (TeX Live 2026)", r);
                let mut v = Vec::new();
                if let Some(f) = o.ind {
                    v.push((Arc::from(format!("{job}.ind")), text(f.contents)));
                }
                if let Some(f) = o.ilg {
                    v.push((Arc::from(format!("{job}.ilg")), text(f.contents)));
                }
                v
            }));
        }
    }
    out
}

/// The first brace-delimited arguments of every `cs` (followed by `{`).
fn args_of(text: &str, cs: &str) -> Vec<String> {
    args_n(text, cs, 1).into_iter().map(|mut a| a.swap_remove(0)).collect()
}

/// For each occurrence of `cs{`, its `n` balanced brace groups (contents).
fn args_n(text: &str, cs: &str, n: usize) -> Vec<Vec<String>> {
    let b = text.as_bytes();
    let mut res = Vec::new();
    let mut from = 0;
    while let Some(p) = text[from..].find(cs) {
        let mut i = from + p + cs.len();
        from = i;
        let mut args = Vec::new();
        while args.len() < n {
            if b.get(i) != Some(&b'{') {
                break;
            }
            let start = i + 1;
            let mut depth = 0usize;
            let mut end = None;
            let mut j = i;
            while j < b.len() {
                match b[j] {
                    b'\\' => j += 1,
                    b'{' => depth += 1,
                    b'}' => {
                        depth -= 1;
                        if depth == 0 {
                            end = Some(j);
                            break;
                        }
                    }
                    _ => {}
                }
                j += 1;
            }
            let Some(e) = end else { break };
            args.push(text[start..e].to_owned());
            i = e + 1;
        }
        if args.len() == n {
            from = i;
            res.push(args);
        }
    }
    res
}

/// `\newlabel{key}{value}` entries of an aux file, in order.
#[must_use]
pub fn labels(aux: &str) -> Vec<(String, String)> {
    args_n(aux, "\\newlabel", 2)
        .into_iter()
        .map(|mut a| {
            let v = a.pop().unwrap_or_default();
            (a.pop().unwrap_or_default(), v)
        })
        .collect()
}

/// `\@writefile{ext}{line}` entries of an aux file, in order.
#[must_use]
pub fn writefiles(aux: &str) -> Vec<(String, String)> {
    args_n(aux, "\\@writefile", 2)
        .into_iter()
        .map(|mut a| {
            let v = a.pop().unwrap_or_default();
            (a.pop().unwrap_or_default(), v)
        })
        .collect()
}

/// `\citation{keys}` keys of an aux file, split at commas, in order.
#[must_use]
pub fn citations(aux: &str) -> Vec<String> {
    args_of(aux, "\\citation")
        .iter()
        .flat_map(|s| s.split(',').map(|k| k.trim().to_owned()))
        .filter(|k| !k.is_empty())
        .collect()
}
