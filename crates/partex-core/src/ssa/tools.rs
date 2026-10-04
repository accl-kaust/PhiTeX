//! The build's BibTeX and makeindex as nodes of its program (DESIGN 3.7,
//! "Outside tools are nodes").
//!
//! A tool is a node on the edge of the job's cycle: it reads the stores a
//! trip left (the `.aux` stream's BibTeX commands, the `.idx` stream's
//! entries) and the host's files it looks up (the `.bst`, the `.bib`
//! databases, the `.ist`), and it defines a stream the next trip loads:
//! the `.bbl`, the `.ind`. That stream is a name the job stores whose φ
//! the node makes (`ssa::define_stream`), so its loads are served from
//! memory and a trip's end compares it like any other store; the link
//! writes it to the host as a file, with the tool's log (`.blg`, `.ilg`,
//! streams the job does not load).
//!
//! A node runs again if and only if something it read changed: one of the
//! commands it reads in a stream, or one of its files (looked at by stamp
//! once per rebuild, as the job's loads are).

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

use super::SsaTracker;
use crate::host::{FileKind, Host};
use crate::tex::Tex;

/// Shared bytes: a stream's value, a file's contents.
type Bytes = Arc<[u8]>;

/// A file a tool looked up through the host: its name, its kind, and
/// what the host found.
type FileRead = (Vec<u8>, FileKind, Option<Bytes>);

/// The stored names' values at a trip's end, by name.
type Served = BTreeMap<Vec<u8>, Option<Bytes>>;

/// What the host gives the build's tools: BibTeX's options (its
/// `texmf.cnf` capacities and banner), makeindex's banner. A tool left
/// out does not run.
#[derive(Clone, Debug, Default)]
pub struct NativeTools {
    pub bibtex: Option<partex_bibtex::Options>,
    pub makeindex: Option<Vec<u8>>,
}

/// The tools' nodes, by the stream each reads.
#[derive(Default)]
pub(crate) struct Tools {
    bib: BTreeMap<Vec<u8>, BibNode>,
    /// Whether each stored `.aux` asks for BibTeX (a `\bibdata` line),
    /// with the bytes that answer is of.
    asks: BTreeMap<u32, (Arc<[u8]>, bool)>,
    /// What BibTeX reads of each `.aux` file (`aux_commands`), with the
    /// bytes it is of.
    commands: BTreeMap<Vec<u8>, (Bytes, Vec<u8>)>,
}

/// A BibTeX node: what its last run read, and how often it ran.
#[derive(Default)]
struct BibNode {
    /// Each `.aux` file its last run read, with what BibTeX reads of it.
    auxes: Vec<(Vec<u8>, Option<Vec<u8>>)>,
    /// The host's files it read (the style, the databases), as found.
    files: Vec<FileRead>,
    /// One of those files changed since (`look`).
    stale: bool,
    runs: u64,
}

/// What BibTeX reads in an `.aux` file: its `\citation`, `\bibdata`,
/// `\bibstyle` and `\@input` lines (bibtex.web §116: a line whose
/// command is none of those is skipped whole).
fn aux_commands(data: &[u8]) -> Vec<u8> {
    let mut d = Vec::new();
    for line in data.split(|&c| c == b'\n') {
        if [
            &b"\\citation{"[..],
            b"\\bibdata{",
            b"\\bibstyle{",
            b"\\@input{",
        ]
        .iter()
        .any(|p| line.starts_with(p))
        {
            d.extend_from_slice(line);
            d.push(b'\n');
        }
    }
    d
}

impl Tools {
    /// What BibTeX reads of `.aux` file `name`, whose bytes are `data`
    /// (made again only when the bytes are not the ones it was made of).
    fn commands_of(&mut self, name: &[u8], data: Option<&Arc<[u8]>>) -> Option<Vec<u8>> {
        let data = data?;
        if let Some((was, c)) = self.commands.get(name)
            && (Arc::ptr_eq(was, data) || was[..] == data[..])
        {
            return Some(c.clone());
        }
        let c = aux_commands(data);
        self.commands
            .insert(name.to_vec(), (data.clone(), c.clone()));
        Some(c)
    }
}

/// A rebuild begins: the files the tools' last runs read, looked at by
/// stamp, once for the build (as the job's loads are, 3.4): a node one
/// of whose files changed runs at the next trip's end. Whether one did.
pub(super) fn look<H: Host>(tex: &mut Tex<H, SsaTracker>) -> bool {
    let loads: Vec<(Vec<u8>, FileRead)> = {
        let r = tex.tracker.rec.borrow();
        r.st.tools
            .bib
            .iter()
            .flat_map(|(n, b)| b.files.iter().map(move |f| (n.clone(), f.clone())))
            .collect()
    };
    if loads.is_empty() {
        return false;
    }
    let asked: Vec<crate::host::Load<'_>> = loads
        .iter()
        .map(|(_, (f, k, c))| (&f[..], *k, c.as_ref()))
        .collect();
    let same = tex.host.unchanged(&asked);
    let mut r = tex.tracker.rec.borrow_mut();
    let mut any = false;
    for ((node, _), same) in loads.iter().zip(same) {
        if !same && let Some(b) = r.st.tools.bib.get_mut(node) {
            b.stale = true;
            any = true;
        }
    }
    any
}

/// The files of a BibTeX run: the `.aux` files from the stores a trip
/// left (`served`, by name) or the host, the style and databases from the
/// host; each read noted.
struct BibFiles<'a, H: Host> {
    host: &'a mut H,
    served: &'a Served,
    auxes: Vec<(Vec<u8>, Option<Bytes>)>,
    files: Vec<FileRead>,
}

impl<H: Host> BibFiles<'_, H> {
    fn find(&mut self, name: &[u8], kind: FileKind) -> Option<Vec<u8>> {
        let found = self.host.read_file(name, kind).map(|f| f.contents);
        self.files.push((name.to_vec(), kind, found.clone()));
        found.map(|c| c.to_vec())
    }
}

impl<H: Host> partex_bibtex::Files for BibFiles<'_, H> {
    fn aux(&mut self, name: &[u8]) -> Option<Vec<u8>> {
        let data = match self.served.get(name) {
            Some(v) => v.clone(),
            None => self
                .host
                .read_file(name, FileKind::Other)
                .map(|f| f.contents),
        };
        self.auxes.push((name.to_vec(), data.clone()));
        data.map(|d| d.to_vec())
    }

    fn bst(&mut self, name: &[u8]) -> Option<Vec<u8>> {
        self.find(name, FileKind::Bst)
    }

    fn bib(&mut self, name: &[u8]) -> Option<Vec<u8>> {
        self.find(name, FileKind::Bib)
    }
}

/// A trip's end (DESIGN 3.7, "Outside tools are nodes"): each tool whose
/// inputs changed runs, on the stores as the trip left them (`phi`, the
/// next trip's φ, by load id); the streams it defines go into `phi`, and
/// those that changed into `changed`. A report line for each run.
pub(super) fn derive<H: Host>(
    tex: &mut Tex<H, SsaTracker>,
    phi: &mut BTreeMap<u32, Option<Arc<[u8]>>>,
    changed: &mut BTreeSet<u32>,
    native: &NativeTools,
    clock: Option<fn() -> u64>,
) -> Vec<String> {
    let mut lines = Vec::new();
    let Some(opts) = &native.bibtex else {
        return lines;
    };
    // (the stored names by name, with the trip's values: the `.aux` files
    // BibTeX reads are served from them)
    let (served, asking): (Served, Vec<Vec<u8>>) = {
        let r = &mut *tex.tracker.rec.borrow_mut();
        let mut served = BTreeMap::new();
        let mut asking = Vec::new();
        for (&id, v) in phi.iter() {
            let Some((name, ..)) = r.st.loads.get(id as usize) else {
                continue;
            };
            served.insert(name.clone(), v.clone());
            if !name.ends_with(b".aux") {
                continue;
            }
            let Some(v) = v else { continue };
            let asks = match r.st.tools.asks.get(&id) {
                Some((was, a)) if Arc::ptr_eq(was, v) => *a,
                _ => {
                    let a = v
                        .split(|&c| c == b'\n')
                        .any(|l| l.starts_with(b"\\bibdata{"));
                    r.st.tools.asks.insert(id, (v.clone(), a));
                    a
                }
            };
            if asks {
                asking.push(name.clone());
            }
        }
        (served, asking)
    };
    for aux in asking {
        // (whether it runs: never ran, a file changed, or what it reads of
        // an `.aux` file)
        let dirty = {
            let r = &mut *tex.tracker.rec.borrow_mut();
            let t = &mut r.st.tools;
            let node = t.bib.entry(aux.clone()).or_default();
            let (runs, stale, auxes) = (node.runs, node.stale, node.auxes.clone());
            runs == 0
                || stale
                || auxes.iter().any(|(n, was)| {
                    let now = match served.get(n) {
                        Some(v) => v.clone(),
                        // (one not stored: the host's, looked at by stamp)
                        None => return false,
                    };
                    t.commands_of(n, now.as_ref()) != *was
                })
        };
        if !dirty {
            continue;
        }
        let t0 = clock.map(|c| c());
        let base = aux.strip_suffix(b".aux").unwrap_or(&aux).to_vec();
        let mut files = BibFiles {
            host: &mut tex.host,
            served: &served,
            auxes: Vec::new(),
            files: Vec::new(),
        };
        let out = partex_bibtex::run(&base, opts, &mut files);
        let (auxes, hfiles) = (files.auxes, files.files);
        {
            let r = &mut *tex.tracker.rec.borrow_mut();
            let read: Vec<(Vec<u8>, Option<Vec<u8>>)> = auxes
                .iter()
                .map(|(n, d)| (n.clone(), r.st.tools.commands_of(n, d.as_ref())))
                .collect();
            // (an `.aux` file not stored is the host's: looked at by stamp)
            let mut hfiles = hfiles;
            for (n, d) in &auxes {
                if !served.contains_key(n) {
                    hfiles.push((n.clone(), FileKind::Other, d.clone()));
                }
            }
            let node = r.st.tools.bib.entry(aux.clone()).or_default();
            node.auxes = read;
            node.files = hfiles;
            node.stale = false;
            node.runs += 1;
        }
        // (the log a stream too, which the job does not load: the link
        // writes it)
        if let Some(blg) = out.blg {
            super::define_stream(tex, &blg.name, Some(Arc::from(blg.contents)));
        }
        if let Some(bbl) = out.bbl {
            let bytes: Arc<[u8]> = Arc::from(bbl.contents);
            super::define_stream(tex, &bbl.name, Some(bytes.clone()));
            let r = tex.tracker.rec.borrow();
            let Some(&id) = r.st.loads_ix.get(&bbl.name[..]) else {
                continue;
            };
            let same = phi
                .get(&id)
                .and_then(Option::as_ref)
                .is_some_and(|was| was[..] == bytes[..]);
            if !same {
                phi.insert(id, Some(bytes));
                changed.insert(id);
            }
        }
        let ms = match (t0, clock) {
            (Some(a), Some(c)) => {
                let us = (c() - a) / 1000;
                alloc::format!(" in {}.{} ms", us / 1000, us % 1000 / 100)
            }
            _ => String::new(),
        };
        let last = out
            .term
            .split(|&c| c == b'\n')
            .rfind(|l| !l.is_empty())
            .map(|l| String::from_utf8_lossy(l).into_owned())
            .unwrap_or_default();
        lines.push(alloc::format!(
            "bibtex {}{ms}: {last}",
            String::from_utf8_lossy(&aux)
        ));
    }
    lines
}
