//! No-op restarts (DESIGN.md §7.9, "No-op restarts"): after a build's
//! outputs are written, a small record next to the store says what it
//! read and wrote, by stamps (length, times, inode) and content hashes,
//! with the result as it was reported. The next process with the same
//! identity looks at the files first: if every input is as it was read
//! (its stamp, else its hash), every file not found is still not found,
//! the day and the date settings are the same, and every output is as it
//! was written, nothing changed, and the recorded result is the result:
//! no load, no link, no write. A watch then loads the saved build in the
//! background, for the first edit. Anything else is the ordinary path.
//! `PARTEX_STORE_QUICK=0` turns it off.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use partex_core::diag::Diagnostic;
use partex_core::host::FileKind;
use partex_core::persist::{Loader, Persist, Saver};

use super::Outcome;

/// What a record holds first (its layout's version).
const TAG: &[u8] = b"partex quick/1";

/// A file's stamp: length, modification and change times (ns), inode and
/// device. A file whose times are this close to the record's writing is
/// racy (a later write could keep them): no stamp, its hash decides.
type Stamp = [i64; 5];

const RACY_NS: i128 = 2_000_000_000;

/// Whether no-op restarts are on (`PARTEX_STORE_QUICK=0`: off).
pub fn on() -> bool {
    std::env::var_os("PARTEX_STORE_QUICK").is_none_or(|v| v != "0")
}

fn path_of(dir: &Path, key: u128) -> PathBuf {
    dir.join("quick").join(format!("{key:032x}"))
}

/// The hash of a file's contents (a `File` cell's version).
pub fn hash(bytes: &[u8]) -> u128 {
    partex_core::StableHasher::of(bytes)
}

fn stamp(m: &std::fs::Metadata, now: i128) -> Option<Stamp> {
    use std::os::unix::fs::MetadataExt;
    let ns = |s: i64, n: i64| i128::from(s) * 1_000_000_000 + i128::from(n);
    let (mt, ct) = (ns(m.mtime(), m.mtime_nsec()), ns(m.ctime(), m.ctime_nsec()));
    if now - mt.max(ct) < RACY_NS {
        return None;
    }
    Some([
        i64::try_from(m.len()).ok()?,
        i64::try_from(mt).ok()?,
        i64::try_from(ct).ok()?,
        i64::try_from(m.ino()).ok()?,
        i64::try_from(m.dev()).ok()?,
    ])
}

fn now_ns() -> i128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i128::try_from(d.as_nanos()).unwrap_or(0))
}

/// Whether the file at `p` holds what hashes to `h`: by its stamp `s` if
/// it has one and it is the same, else by its contents.
fn holds(p: &Path, s: Option<&Stamp>, h: u128) -> bool {
    let Ok(m) = std::fs::metadata(p) else {
        return false;
    };
    // (a stamp is compared with one taken now, however recent the times:
    // a racy stamp was not recorded)
    if let Some(s) = s
        && stamp(&m, i128::MAX) == Some(*s)
    {
        return true;
    }
    std::fs::read(p).is_ok_and(|b| hash(&b) == h)
}

/// What a build read and wrote, and its result, as [`record`] takes it.
pub struct Build {
    /// Each input file the build read (the path key the host serves it
    /// by) and the contents it read.
    pub inputs: Vec<(Vec<u8>, Arc<[u8]>)>,
    /// The files it looked for and did not find: key, name and kind.
    pub missed: Vec<(Vec<u8>, Vec<u8>, FileKind)>,
    /// The clock as it read it, if it did (`machinehost::CLOCK`).
    pub clock: Option<Vec<u8>>,
    /// Each output file as written: name and the hash of its bytes.
    pub outputs: Vec<(Vec<u8>, u128)>,
    pub outcome: Outcome,
}

/// A record as saved.
struct Record {
    inputs: Vec<(Vec<u8>, Option<Stamp>, u128)>,
    missed: Vec<(Vec<u8>, FileKind)>,
    clock: Option<Vec<u8>>,
    outputs: Vec<(Vec<u8>, Option<Stamp>, u128)>,
    term: Vec<u8>,
    history: i32,
    diagnostics: Vec<Diagnostic>,
    sizes: Vec<(Vec<u8>, usize)>,
    reports: Vec<String>,
}

impl Persist for Record {
    fn save(&self, s: &mut Saver) {
        let Self {
            inputs,
            missed,
            clock,
            outputs,
            term,
            history,
            diagnostics,
            sizes,
            reports,
        } = self;
        inputs.save(s);
        missed.save(s);
        clock.save(s);
        outputs.save(s);
        term.save(s);
        history.save(s);
        diagnostics.save(s);
        sizes.save(s);
        reports.save(s);
    }

    fn load(l: &mut Loader) -> Option<Self> {
        Some(Self {
            inputs: Persist::load(l)?,
            missed: Persist::load(l)?,
            clock: Persist::load(l)?,
            outputs: Persist::load(l)?,
            term: Persist::load(l)?,
            history: Persist::load(l)?,
            diagnostics: Persist::load(l)?,
            sizes: Persist::load(l)?,
            reports: Persist::load(l)?,
        })
    }
}

/// Record `b` for the job `key` in the store `dir` (on this thread: the
/// caller runs it off the build's path). Inputs are stamped as they are on
/// disk if their contents are still what the build read; if one is not,
/// the build is not the files' and there is no record.
pub fn record(dir: &Path, key: u128, build: &Build) {
    record_at(dir, key, build, now_ns());
}

/// [`record`] as if at `now` (ns since the epoch: what is racy).
fn record_at(dir: &Path, key: u128, build: &Build, now: i128) {
    let file = path_of(dir, key);
    let mut inputs = Vec::with_capacity(build.inputs.len());
    for (k, read) in &build.inputs {
        let f = crate::native::path(k);
        let (Ok(m1), Ok(disk)) = (std::fs::metadata(&f), std::fs::read(&f)) else {
            let _ = std::fs::remove_file(&file);
            return;
        };
        if disk[..] != read[..] {
            let _ = std::fs::remove_file(&file);
            return;
        }
        // (the stamp taken before the read holds only if the file did not
        // change meanwhile)
        let s = stamp(&m1, now)
            .filter(|s| std::fs::metadata(&f).is_ok_and(|m2| stamp(&m2, now).as_ref() == Some(s)));
        inputs.push((k.clone(), s, hash(read)));
    }
    let mut outputs = Vec::with_capacity(build.outputs.len());
    for (name, h) in &build.outputs {
        let s = std::fs::metadata(crate::native::path(name))
            .ok()
            .and_then(|m| stamp(&m, now));
        outputs.push((name.clone(), s, *h));
    }
    let outcome = &build.outcome;
    let rec = Record {
        inputs,
        missed: build
            .missed
            .iter()
            .map(|(_, n, k)| (n.clone(), *k))
            .collect(),
        clock: build.clock.clone(),
        outputs,
        term: outcome.term.clone(),
        history: outcome.history,
        diagnostics: outcome.diagnostics.clone(),
        sizes: outcome.outputs.clone(),
        reports: outcome.reports.clone(),
    };
    let mut s = Saver::new();
    s.raw(TAG);
    rec.save(&mut s);
    let bytes = s.into_bytes();
    let _ = std::fs::create_dir_all(dir.join("quick"));
    let tmp = file.with_extension(format!("tmp{}", std::process::id()));
    if std::fs::write(&tmp, &bytes).is_ok() && std::fs::rename(&tmp, &file).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

/// Forget the record of job `key` (its outputs are about to change):
/// whether there was one.
pub fn forget(dir: &Path, key: u128) -> bool {
    std::fs::remove_file(path_of(dir, key)).is_ok()
}

/// A restart with nothing changed: the last result, the outputs' hashes
/// (as a watch keeps them) and how many files were looked at.
pub struct Hit {
    pub outcome: Outcome,
    pub written: BTreeMap<Vec<u8>, u128>,
    pub looked: usize,
}

/// What a check asks of the host beyond the files' stamps and contents.
pub trait Look {
    /// Whether the clock the build read (its contents `old`) is not the
    /// clock now.
    fn clock_changed(&mut self, old: &[u8]) -> bool;
    /// Whether a file looked for as `name` of `kind` is found now.
    fn found(&mut self, name: &[u8], kind: FileKind) -> bool;
}

/// Whether nothing changed since the build recorded for job `key`.
pub fn check(dir: &Path, key: u128, look: &mut dyn Look) -> Option<Hit> {
    let bytes = std::fs::read(path_of(dir, key)).ok()?;
    let mut l = Loader::new(&bytes);
    if l.take(TAG.len())? != TAG {
        return None;
    }
    let r = Record::load(&mut l)?;
    if !l.at_end() {
        return None;
    }
    if let Some(c) = &r.clock
        && look.clock_changed(c)
    {
        return None;
    }
    for (k, s, h) in &r.inputs {
        if !holds(&crate::native::path(k), s.as_ref(), *h) {
            return None;
        }
    }
    for (name, kind) in &r.missed {
        if look.found(name, *kind) {
            return None;
        }
    }
    for (name, s, h) in &r.outputs {
        if !holds(&crate::native::path(name), s.as_ref(), *h) {
            return None;
        }
    }
    let looked = r.inputs.len() + r.missed.len() + r.outputs.len();
    Some(Hit {
        written: r.outputs.iter().map(|(n, _, h)| (n.clone(), *h)).collect(),
        outcome: Outcome {
            term: r.term,
            history: r.history,
            diagnostics: r.diagnostics,
            outputs: r.sizes,
            reports: r.reports,
            unsettled: Vec::new(),
        },
        looked,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Stub {
        clock: bool,
        found: bool,
    }

    impl Look for Stub {
        fn clock_changed(&mut self, _: &[u8]) -> bool {
            self.clock
        }
        fn found(&mut self, _: &[u8], _: FileKind) -> bool {
            self.found
        }
    }

    fn outcome() -> Outcome {
        Outcome {
            term: b"term".to_vec(),
            history: 1,
            diagnostics: Vec::new(),
            outputs: vec![(b"o.pdf".to_vec(), 3)],
            reports: vec![String::from("r")],
            unsettled: Vec::new(),
        }
    }

    /// A directory of its own, with inputs `a` and `b`, an output `o.pdf`
    /// and the store under `s`.
    fn setup(name: &str) -> (PathBuf, Build) {
        let d = std::env::temp_dir().join(format!("partex-quick-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let (a, b, o) = (d.join("a"), d.join("b"), d.join("o.pdf"));
        std::fs::write(&a, b"aaa").unwrap();
        std::fs::write(&b, b"bbb").unwrap();
        std::fs::write(&o, b"pdf").unwrap();
        let key = |p: &Path| p.as_os_str().as_encoded_bytes().to_vec();
        let build = Build {
            inputs: vec![
                (key(&a), Arc::from(&b"aaa"[..])),
                (key(&b), Arc::from(&b"bbb"[..])),
            ],
            missed: vec![(b"\0k".to_vec(), b"gone".to_vec(), FileKind::Tex)],
            clock: Some(b"clock".to_vec()),
            outputs: vec![(key(&o), hash(b"pdf"))],
            outcome: outcome(),
        };
        (d, build)
    }

    /// Recorded as if a minute later (every stamp kept), or now (every
    /// file racy: hashed).
    fn rec(s: &Path, b: &Build, later: bool) {
        let dt = if later { 60_000_000_000 } else { 0 };
        record_at(s, 7, b, now_ns() + dt);
    }

    fn quiet() -> Stub {
        Stub {
            clock: false,
            found: false,
        }
    }

    #[test]
    fn nothing_changed_is_the_recorded_result() {
        for later in [false, true] {
            let (d, b) = setup("hit");
            let s = d.join("s");
            rec(&s, &b, later);
            nothing_changed(&d, &s);
        }
    }

    fn nothing_changed(d: &Path, s: &Path) {
        let s = s.to_path_buf();
        let hit = check(&s, 7, &mut quiet()).expect("nothing changed");
        assert_eq!(hit.outcome.term, b"term");
        assert_eq!(hit.outcome.history, 1);
        assert_eq!(hit.looked, 4);
        assert_eq!(
            hit.written.values().copied().collect::<Vec<_>>(),
            vec![hash(b"pdf")]
        );
        // (another job's record is not this one's)
        assert!(check(&s, 8, &mut quiet()).is_none());
        let _ = std::fs::remove_dir_all(d);
    }

    /// A change made to the files of [`setup`].
    type Change = fn(&Path);

    #[test]
    fn any_change_is_not_a_hit() {
        let cases: [(&str, Change); 5] = [
            ("input", |d| std::fs::write(d.join("a"), b"aab").unwrap()),
            // (same length, times kept: the change time still differs)
            ("stamped", |d| {
                let f = d.join("b");
                let t = std::fs::metadata(&f).unwrap().modified().unwrap();
                std::fs::write(&f, b"bbc").unwrap();
                std::fs::File::options()
                    .write(true)
                    .open(&f)
                    .unwrap()
                    .set_modified(t)
                    .unwrap();
            }),
            ("output", |d| {
                std::fs::write(d.join("o.pdf"), b"pdF").unwrap();
            }),
            ("output gone", |d| {
                std::fs::remove_file(d.join("o.pdf")).unwrap();
            }),
            ("input gone", |d| {
                std::fs::remove_file(d.join("b")).unwrap();
            }),
        ];
        for later in [false, true] {
            for (name, change) in cases {
                let (d, b) = setup(name);
                let s = d.join("s");
                rec(&s, &b, later);
                if later {
                    // (past the clock tick of the writes before: recorded
                    // as if later, nothing is racy, as a real minute makes)
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                change(&d);
                assert!(
                    check(&s, 7, &mut quiet()).is_none(),
                    "{name}, later {later}"
                );
                let _ = std::fs::remove_dir_all(&d);
            }
        }
        let (d, b) = setup("clock");
        let s = d.join("s");
        rec(&s, &b, true);
        let mut clock = Stub {
            clock: true,
            found: false,
        };
        assert!(check(&s, 7, &mut clock).is_none(), "the clock");
        let mut found = Stub {
            clock: false,
            found: true,
        };
        assert!(check(&s, 7, &mut found).is_none(), "a file found");
        assert!(check(&s, 7, &mut quiet()).is_some());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_build_not_of_the_files_is_not_recorded() {
        let (d, b) = setup("stale");
        let s = d.join("s");
        rec(&s, &b, true);
        assert!(check(&s, 7, &mut quiet()).is_some());
        // (the file changed after the build read it: no record, and the
        // one before is gone)
        std::thread::sleep(std::time::Duration::from_millis(50));
        std::fs::write(d.join("a"), b"new").unwrap();
        rec(&s, &b, true);
        std::fs::write(d.join("a"), b"aaa").unwrap();
        assert!(check(&s, 7, &mut quiet()).is_none());
        let _ = std::fs::remove_dir_all(&d);
    }
}
