//! A native port of kpathsea's file lookup, as `tex` and xdvipdfmx use it.
//!
//! Follows the kpathsea sources (`texk/kpathsea`, cited by file and
//! function): reading `texmf.cnf` (`cnf.c`), variable, tilde and brace
//! expansion (`variable.c`, `tilde.c`, `expand.c`), default-path splicing
//! (`kdefault.c`), per-format search paths (`tex-file.c`), the `ls-R`
//! databases (`db.c`), `//` subdirectory expansion (`elt-dirs.c`) and the
//! path search itself (`pathsearch.c`).
//!
//! Not ported (not reachable from `tex` with TeX Live's configuration):
//! the `aliases` database, `mktex*` file generation, Windows path syntax,
//! and the compile-time defaults for paths that `texmf.cnf` always sets
//! (only `TEXMFCNF`'s default is needed to find `texmf.cnf` in the first
//! place, and the program formats', which name the program).

use std::collections::HashMap;
#[cfg(unix)]
use std::os::unix::ffi::{OsStrExt, OsStringExt};
// (the WebAssembly build, `scripts/wasm.sh`: WASI's paths are bytes too)
#[cfg(target_os = "wasi")]
use std::os::wasi::ffi::{OsStrExt, OsStringExt};
use std::path::Path;

type Bytes = Vec<u8>;

/// The file formats `tex` and xdvipdfmx ask for (`kpse_file_format_type`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Format {
    Tex,
    Tfm,
    Fmt,
    Cnf,
    Db,
    /// `kpse_web2c_format`: web2c's own files (TCX translations).
    Web2c,
    /// `kpse_fontmap_format`: font map files (`pdftex.map`).
    FontMap,
    /// `kpse_type1_format`: Type 1 fonts.
    Type1,
    /// `kpse_enc_format`: encoding vectors.
    Enc,
    /// `kpse_vf_format`: virtual fonts.
    Vf,
    /// `kpse_truetype_format`: TrueType fonts.
    TrueType,
    /// `kpse_bib_format`: BibTeX databases.
    Bib,
    /// `kpse_bst_format`: BibTeX styles.
    Bst,
    /// makeindex styles (`kpse_ist_format`).
    Ist,
    /// `kpse_opentype_format`: OpenType fonts.
    OpenType,
    /// `kpse_miscfonts_format`: `XeTeX`'s `TECkit` mappings.
    MiscFonts,
    /// `kpse_ofm_format`: Omega font metrics (and TFMs).
    Ofm,
    /// `kpse_ovf_format`: Omega virtual fonts (and VFs).
    Ovf,
    /// `kpse_cmap_format`: `CMap` files.
    Cmap,
    /// `kpse_sfd_format`: subfont definition files.
    Sfd,
    /// `kpse_pict_format`: graphics.
    Pict,
    /// `kpse_program_text_format`: the program's own text files.
    ProgramText,
    /// `kpse_program_binary_format`: the program's own binary files.
    ProgramBinary,
}

/// `tex-file.c`, `kpathsea_init_format`: how each format is searched.
struct FormatInfo {
    envs: &'static [&'static str],
    suffixes: &'static [&'static str],
    alt_suffixes: &'static [&'static str],
    suffix_search_only: bool,
    default_path: &'static str,
}

/// `paths.h`'s `DEFAULT_TEXMFCNF`, as TeX Live builds it.
const DEFAULT_TEXMFCNF: &str = "{$SELFAUTOLOC,$SELFAUTOLOC/share/texmf-local/web2c,\
$SELFAUTOLOC/share/texmf-dist/web2c,$SELFAUTOLOC/share/texmf/web2c,\
$SELFAUTOLOC/texmf-local/web2c,$SELFAUTOLOC/texmf-dist/web2c,$SELFAUTOLOC/texmf/web2c,\
$SELFAUTODIR,$SELFAUTODIR/share/texmf-local/web2c,$SELFAUTODIR/share/texmf-dist/web2c,\
$SELFAUTODIR/share/texmf/web2c,$SELFAUTODIR/texmf-local/web2c,$SELFAUTODIR/texmf-dist/web2c,\
$SELFAUTODIR/texmf/web2c,$SELFAUTOGRANDPARENT/texmf-local/web2c,$SELFAUTOPARENT,\
$SELFAUTOPARENT/share/texmf-local/web2c,$SELFAUTOPARENT/share/texmf-dist/web2c,\
$SELFAUTOPARENT/share/texmf/web2c,$SELFAUTOPARENT/texmf-local/web2c,\
$SELFAUTOPARENT/texmf-dist/web2c,$SELFAUTOPARENT/texmf/web2c}";

#[allow(clippy::too_many_lines, reason = "a table")]
fn format_info(f: Format) -> FormatInfo {
    match f {
        Format::Tex => FormatInfo {
            envs: &["TEXINPUTS"],
            suffixes: &[".tex"],
            alt_suffixes: &[
                ".sty", ".cls", ".fd", ".aux", ".bbl", ".def", ".clo", ".ldf",
            ],
            suffix_search_only: false,
            default_path: "",
        },
        Format::Tfm => FormatInfo {
            envs: &["TFMFONTS", "TEXFONTS"],
            suffixes: &[".tfm"],
            alt_suffixes: &[],
            suffix_search_only: true,
            default_path: "",
        },
        Format::Fmt => FormatInfo {
            envs: &["TEXFORMATS", "TEXMFINI"],
            suffixes: &[".fmt"],
            alt_suffixes: &[],
            suffix_search_only: false,
            default_path: "",
        },
        Format::Cnf => FormatInfo {
            envs: &["TEXMFCNF"],
            suffixes: &[".cnf"],
            alt_suffixes: &[],
            suffix_search_only: false,
            default_path: DEFAULT_TEXMFCNF,
        },
        Format::Db => FormatInfo {
            envs: &["TEXMFDBS"],
            suffixes: &["ls-R", "ls-r"],
            alt_suffixes: &[],
            suffix_search_only: false,
            default_path: "",
        },
        Format::Web2c => FormatInfo {
            envs: &["WEB2C"],
            suffixes: &[],
            alt_suffixes: &[],
            suffix_search_only: false,
            default_path: "",
        },
        Format::FontMap => FormatInfo {
            envs: &["TEXFONTMAPS", "TEXFONTS"],
            suffixes: &[".map"],
            alt_suffixes: &[],
            suffix_search_only: false,
            default_path: "",
        },
        Format::Type1 => FormatInfo {
            envs: &[
                "T1FONTS",
                "T1INPUTS",
                "TEXFONTS",
                "TEXPSHEADERS",
                "PSHEADERS",
            ],
            suffixes: &[".pfa", ".pfb"],
            alt_suffixes: &[],
            suffix_search_only: false,
            default_path: "",
        },
        Format::Enc => FormatInfo {
            envs: &["ENCFONTS", "TEXFONTS"],
            suffixes: &[".enc"],
            alt_suffixes: &[],
            suffix_search_only: true,
            default_path: "",
        },
        Format::Vf => FormatInfo {
            envs: &["VFFONTS", "TEXFONTS"],
            suffixes: &[".vf"],
            alt_suffixes: &[],
            suffix_search_only: true,
            default_path: "",
        },
        Format::TrueType => FormatInfo {
            envs: &["TTFONTS", "TEXFONTS"],
            suffixes: &[".ttf", ".ttc", ".TTF", ".TTC", ".dfont"],
            alt_suffixes: &[],
            suffix_search_only: false,
            default_path: "",
        },
        Format::OpenType => FormatInfo {
            envs: &["OPENTYPEFONTS", "TEXFONTS"],
            suffixes: &[".otf", ".OTF"],
            alt_suffixes: &[],
            suffix_search_only: true,
            default_path: "",
        },
        Format::MiscFonts => FormatInfo {
            envs: &["MISCFONTS", "TEXFONTS"],
            suffixes: &[],
            alt_suffixes: &[],
            suffix_search_only: false,
            default_path: "",
        },
        Format::Bib => FormatInfo {
            envs: &["BIBINPUTS", "TEXBIB"],
            suffixes: &[".bib"],
            alt_suffixes: &[],
            suffix_search_only: false,
            default_path: "",
        },
        Format::Bst => FormatInfo {
            envs: &["BSTINPUTS"],
            suffixes: &[".bst"],
            alt_suffixes: &[],
            suffix_search_only: false,
            default_path: "",
        },
        Format::Ist => FormatInfo {
            envs: &["INDEXSTYLE"],
            suffixes: &[".ist"],
            alt_suffixes: &[],
            suffix_search_only: false,
            default_path: "",
        },
        Format::Ofm => FormatInfo {
            envs: &["OFMFONTS", "TEXFONTS"],
            suffixes: &[".ofm", ".tfm"],
            alt_suffixes: &[],
            suffix_search_only: true,
            default_path: "",
        },
        Format::Ovf => FormatInfo {
            envs: &["OVFFONTS", "TEXFONTS"],
            suffixes: &[".ovf", ".vf"],
            alt_suffixes: &[],
            suffix_search_only: true,
            default_path: "",
        },
        Format::Cmap => FormatInfo {
            envs: &["CMAPFONTS", "TEXFONTS"],
            suffixes: &[],
            alt_suffixes: &[],
            suffix_search_only: false,
            default_path: "",
        },
        Format::Sfd => FormatInfo {
            envs: &["SFDFONTS", "TEXFONTS"],
            suffixes: &[".sfd"],
            alt_suffixes: &[],
            suffix_search_only: true,
            default_path: "",
        },
        Format::Pict => FormatInfo {
            envs: &["TEXPICTS", "TEXINPUTS"],
            suffixes: &[],
            alt_suffixes: &[".eps", ".epsi"],
            suffix_search_only: false,
            default_path: "",
        },
        // (the variable, `<PROGNAME>INPUTS`, and the default path,
        // `.:$TEXMF/<progname>//`, are the program's: `init_format`)
        Format::ProgramText | Format::ProgramBinary => FormatInfo {
            envs: &[],
            suffixes: &[],
            alt_suffixes: &[],
            suffix_search_only: false,
            default_path: "",
        },
    }
}

/// `c-pathch.h`: `IS_KPSE_SEP` (Unix accepts `;` too).
fn is_kpse_sep(c: u8) -> bool {
    c == b':' || c == b';'
}

/// `kpse_cnf_p`: a true-ish configuration value.
fn cnf_p(v: Option<&[u8]>) -> bool {
    v.and_then(|v| v.first())
        .is_some_and(|c| b"ty1".contains(c))
}

/// `absolute.c`, `kpathsea_absolute_p`.
fn absolute_p(name: &[u8], relative_ok: bool) -> bool {
    name.first() == Some(&b'/')
        || (relative_ok
            && (name.starts_with(b"./")
                || name.starts_with(b"../")
                || name == b"."
                || name == b".."))
}

fn os(b: &[u8]) -> &Path {
    Path::new(std::ffi::OsStr::from_bytes(b))
}

/// `readable.c`: a readable regular file.
fn readable_file(name: &[u8]) -> bool {
    std::fs::metadata(os(name)).is_ok_and(|m| m.is_file()) && std::fs::File::open(os(name)).is_ok()
}

/// `path-elt.c`, `element`: split `path` at separators outside braces.
fn path_elements(path: &[u8]) -> Vec<Bytes> {
    let mut out = Vec::new();
    let mut level = 0i32;
    let mut start = 0;
    for (i, &c) in path.iter().enumerate() {
        match c {
            b'{' => level += 1,
            b'}' => level -= 1,
            c if level == 0 && is_kpse_sep(c) => {
                out.push(path[start..i].to_vec());
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(path[start..].to_vec());
    out
}

/// `kdefault.c`, `kpathsea_expand_default`: splice `fallback` into an
/// extra `:` of `path`.
fn expand_default(path: &[u8], fallback: &[u8]) -> Bytes {
    if path.is_empty() {
        fallback.to_vec()
    } else if path[0] == b':' {
        if path.len() == 1 {
            fallback.to_vec()
        } else {
            [fallback, path].concat()
        }
    } else if path.ends_with(b":") {
        [path, fallback].concat()
    } else if let Some(i) = path.windows(2).position(|w| w == b"::") {
        [&path[..=i], fallback, &path[i + 1..]].concat()
    } else {
        path.to_vec()
    }
}

/// `expand.c`, `brace_expand`: the list of expansions of `{a,b}c`-style
/// text starting at `*pos`, stopping at an unmatched `}`.
fn brace_expand_list(text: &[u8], pos: &mut usize) -> Vec<Bytes> {
    let mut result: Vec<Bytes> = Vec::new();
    let mut partial: Vec<Bytes> = vec![Vec::new()];
    let mut start = *pos;
    let mut p = *pos;
    // `str_list_concat_elements`: append `s` to every partial result
    // (a cross product with a list).
    let cross = |partial: &mut Vec<Bytes>, more: &[Bytes]| {
        if more.is_empty() {
            return;
        }
        // (kpathsea's loop order: alternatives outside, prefixes inside.)
        let mut out = Vec::with_capacity(partial.len() * more.len());
        for b in more {
            for a in partial.iter() {
                out.push([a.as_slice(), b].concat());
            }
        }
        *partial = out;
    };
    while p < text.len() && text[p] != b'}' {
        let c = text[p];
        if c == b':' || c == b',' {
            cross(&mut partial, &[text[start..p].to_vec()]);
            result.append(&mut partial);
            partial = vec![Vec::new()];
            start = p + 1;
        } else if c == b'{' {
            cross(&mut partial, &[text[start..p].to_vec()]);
            p += 1;
            let rec = brace_expand_list(text, &mut p);
            cross(&mut partial, &rec);
            if p >= text.len() || text[p] != b'}' {
                // "Unmatched {": undo the `++p` for the next iteration.
                p -= 1;
            }
            start = p + 1;
        } else if c == b'$' && text.get(p + 1) == Some(&b'{') {
            // Skip ${VAR}.
            p += 2;
            while p < text.len() && text[p] != b'}' {
                p += 1;
            }
            if p >= text.len() {
                break;
            }
        }
        p += 1;
    }
    let end = p.min(text.len());
    cross(&mut partial, &[text[start.min(end)..end].to_vec()]);
    result.append(&mut partial);
    *pos = end;
    result
}

/// `db.c`, `ignore_dir_p`: a directory with a component starting with
/// `.` (other than `./` and `../`-style ones).
fn ignore_dir_p(dir: &[u8]) -> bool {
    (1..dir.len())
        .any(|i| dir[i] == b'.' && dir[i - 1] == b'/' && dir.get(i + 1).is_some_and(|&c| c != b'/'))
}

/// `db.c`, `match`: does the file `filename` lie in path element `elt`
/// (which may contain `//`)?
fn db_match(filename: &[u8], elt: &[u8]) -> bool {
    let (mut f, mut e) = (0, 0);
    let mut matched = false;
    while f < filename.len() && e < elt.len() {
        if filename[f] == elt[e] {
        } else if elt[e] == b'/' && f > 0 && e > 0 && elt[e - 1] == b'/' {
            while e < elt.len() && elt[e] == b'/' {
                e += 1;
            }
            if e == elt.len() {
                // Trailing //: matches anything.
                matched = true;
                break;
            }
            // Intermediate //: match the rest of `elt` at each component.
            while !matched && f < filename.len() {
                if filename[f - 1] == b'/' && filename[f] == elt[e] {
                    matched = db_match(&filename[f..], &elt[e..]);
                }
                f += 1;
            }
            break;
        } else {
            break;
        }
        f += 1;
        e += 1;
    }
    if !matched && e == elt.len() {
        if filename.get(f) == Some(&b'/') {
            f += 1;
        }
        if f == 0 || filename[f - 1] == b'/' {
            matched = !filename[f..].contains(&b'/');
        }
    }
    matched
}

/// `db.c`, `elt_in_db`: is `db_dir` a prefix of `elt`?
fn elt_in_db(db_dir: &[u8], elt: &[u8]) -> bool {
    !db_dir.is_empty() && !elt.is_empty() && elt.starts_with(db_dir)
}

/// The `ls-R` hash table's hasher: eight bytes of a name at a time
/// (a database has hundreds of thousands of names to insert).
#[derive(Default)]
struct Fnv(u64);

impl std::hash::Hasher for Fnv {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        const K: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut h = self.0;
        let (words, rest) = bytes.as_chunks::<8>();
        for w in words {
            h = (h.rotate_left(5) ^ u64::from_le_bytes(*w)).wrapping_mul(K);
        }
        if !rest.is_empty() {
            let mut w = [0u8; 8];
            w[..rest.len()].copy_from_slice(rest);
            h = (h.rotate_left(5) ^ u64::from_le_bytes(w) ^ (rest.len() as u64) << 59)
                .wrapping_mul(K);
        }
        self.0 = h;
    }
}

/// The `ls-R` databases (`kpse->db`): file name → directories (with a
/// trailing `/`) in the order the databases list them. One flat index,
/// by the name's hash and then that order, over the databases' texts:
/// kept as it is in a client's cache ([`set_db_cache`]), so a process
/// reads it back instead of parsing every `ls-R` again.
#[derive(Default)]
struct Db {
    dirs: Vec<Bytes>,
    /// The databases' texts, one after the other: the names in place.
    text: Bytes,
    /// Each file: its name's hash, where its name is in `text` and how
    /// long, its directory; by hash, then in the databases' order.
    index: Vec<(u64, u32, u32, u32)>,
    /// `kpse->roots`: the directories holding an `ls-R`.
    roots: Vec<Bytes>,
}

/// A name's hash in [`Db::index`].
fn name_hash(name: &[u8]) -> u64 {
    use std::hash::Hasher as _;
    let mut h = Fnv::default();
    h.write(name);
    h.finish()
}

impl Db {
    fn lookup<'a>(&'a self, name: &[u8]) -> impl Iterator<Item = &'a [u8]> + 'a {
        let h = name_hash(name);
        let name = name.to_vec();
        let from = self.index.partition_point(|e| e.0 < h);
        self.index[from..]
            .iter()
            .take_while(move |e| e.0 == h)
            .filter(move |e| self.text.get(e.1 as usize..(e.1 + e.2) as usize) == Some(name.as_slice()))
            .map(|e| self.dirs[e.3 as usize].as_slice())
    }

    /// `db.c`, `db_build`: add the `ls-R` file `db_filename` (the index
    /// is put in order by [`Db::finish`]).
    fn build(&mut self, db_filename: &[u8]) -> bool {
        let Ok(text) = std::fs::read(os(db_filename)) else {
            return false;
        };
        let base = self.text.len();
        self.text.extend_from_slice(&text);
        // Keep the `/` before `ls-R`.
        let top_dir = &db_filename[..db_filename.len() - 4];
        // Size the index up front.
        #[allow(clippy::naive_bytecount)] // one pass, no dependency
        let lines = text.iter().filter(|&&c| c == b'\n').count();
        self.index.reserve(lines);
        let mut cur_dir: Option<u32> = None; // first thing might be a file name
        let mut file_count = 0;
        let mut at = base;
        for line in text.split(|&c| c == b'\n') {
            let off = at;
            at += line.len() + 1;
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            if line.ends_with(b":") && absolute_p(line, true) {
                if ignore_dir_p(line) {
                    cur_dir = None;
                } else {
                    let mut d = line[..line.len() - 1].to_vec();
                    d.push(b'/');
                    let full = if d[0] == b'.' {
                        [top_dir, &d[2..]].concat()
                    } else {
                        d
                    };
                    self.dirs.push(full);
                    cur_dir = Some(u32::try_from(self.dirs.len() - 1).expect("ls-R too large"));
                }
            } else if let Some(dir) = cur_dir
                && !line.is_empty()
                && line != b"."
                && line != b".."
            {
                let o = u32::try_from(off).expect("ls-R too large");
                let n = u32::try_from(line.len()).expect("ls-R too large");
                self.index.push((name_hash(line), o, n, dir));
                file_count += 1;
            }
        }
        if file_count > 0 {
            self.roots.push(top_dir.to_vec());
        }
        file_count > 0
    }

    /// The index by hash, each name's entries in the databases' order
    /// (a stable sort).
    fn finish(&mut self) {
        self.index.sort_by_key(|e| e.0);
    }

    /// As bytes, after `key` (the databases' files, lengths and times).
    fn to_bytes(&self, key: &[(Bytes, u64, u128)]) -> Bytes {
        let mut out = DB_MAGIC.to_vec();
        let put = |out: &mut Bytes, b: &[u8]| {
            out.extend_from_slice(&(b.len() as u64).to_le_bytes());
            out.extend_from_slice(b);
        };
        out.extend_from_slice(&(key.len() as u64).to_le_bytes());
        for (f, len, at) in key {
            put(&mut out, f);
            out.extend_from_slice(&len.to_le_bytes());
            out.extend_from_slice(&at.to_le_bytes());
        }
        for list in [&self.dirs, &self.roots] {
            out.extend_from_slice(&(list.len() as u64).to_le_bytes());
            for d in list {
                put(&mut out, d);
            }
        }
        put(&mut out, &self.text);
        out.extend_from_slice(&(self.index.len() as u64).to_le_bytes());
        for &(h, o, n, d) in &self.index {
            out.extend_from_slice(&h.to_le_bytes());
            out.extend_from_slice(&o.to_le_bytes());
            out.extend_from_slice(&n.to_le_bytes());
            out.extend_from_slice(&d.to_le_bytes());
        }
        out
    }

    /// What [`Db::to_bytes`] wrote for `key`; `None` for other bytes or
    /// another key (a database changed since).
    fn from_bytes(bytes: &[u8], key: &[(Bytes, u64, u128)]) -> Option<Db> {
        let mut rd = Rd(bytes.strip_prefix(DB_MAGIC.as_slice())?);
        if rd.len()? != key.len() {
            return None;
        }
        for (file, len, at) in key {
            let size = rd.len()?;
            if rd.take(size)? != file.as_slice() || rd.u64()? != *len || rd.u128()? != *at {
                return None;
            }
        }
        let mut lists = [Vec::new(), Vec::new()];
        for list in &mut lists {
            for _ in 0..rd.len()? {
                let size = rd.len()?;
                list.push(rd.take(size)?.to_vec());
            }
        }
        let [dirs, roots] = lists;
        let size = rd.len()?;
        let text = rd.take(size)?.to_vec();
        let count = rd.len()?;
        let raw = rd.take(count.checked_mul(20)?)?;
        if !rd.0.is_empty() {
            return None;
        }
        let mut index = Vec::with_capacity(count);
        for entry in raw.as_chunks::<20>().0 {
            let word = |a: usize| u32::from_le_bytes([entry[a], entry[a + 1], entry[a + 2], entry[a + 3]]);
            let hash = u64::from(word(0)) | (u64::from(word(4)) << 32);
            let (off, len, dir) = (word(8), word(12), word(16));
            if (off as usize).checked_add(len as usize)? > text.len() || dir as usize >= dirs.len() {
                return None;
            }
            index.push((hash, off, len, dir));
        }
        if !index.is_sorted_by_key(|e| e.0) {
            return None;
        }
        Some(Db { dirs, text, index, roots })
    }
}

/// A reader over a kept database's bytes.
struct Rd<'a>(&'a [u8]);

impl<'a> Rd<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let (a, rest) = self.0.split_at_checked(n)?;
        self.0 = rest;
        Some(a)
    }
    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
    fn u128(&mut self) -> Option<u128> {
        Some(u128::from_le_bytes(self.take(16)?.try_into().ok()?))
    }
    /// A count, never more than the bytes left.
    fn len(&mut self) -> Option<usize> {
        let n = usize::try_from(self.u64()?).ok()?;
        (n <= self.0.len()).then_some(n)
    }
}

/// The first bytes of a kept database ([`Db::to_bytes`]): its layout.
const DB_MAGIC: &[u8; 8] = b"PHXLSR01";

/// Where the `ls-R` databases are kept between processes (none: they are
/// parsed in each).
static DB_CACHE: std::sync::Mutex<Option<std::path::PathBuf>> = std::sync::Mutex::new(None);

/// Keeps the `ls-R` databases in `dir` (none: not kept): a process reads
/// a database kept there while its files have the length and time they
/// had, instead of parsing them.
pub fn set_db_cache(dir: Option<std::path::PathBuf>) {
    *DB_CACHE.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = dir;
}

/// The file a database of the `ls-R` files `key` is kept in: one per set
/// of files, rewritten when one changes.
fn db_cache_file(key: &[(Bytes, u64, u128)]) -> Option<std::path::PathBuf> {
    let dir = DB_CACHE.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()?;
    let mut names = Vec::new();
    for (f, _, _) in key {
        names.extend_from_slice(f);
        names.push(0);
    }
    Some(dir.join(format!("ls-r-{:016x}.db", name_hash(&names))))
}

/// A kpathsea instance for one program (`kpathsea_instance`).
pub struct Kpse {
    program_name: Bytes,
    /// Variables kpathsea puts into its environment (`SELFAUTO*`,
    /// `progname`, `engine`); consulted before the real environment.
    env_overlay: HashMap<Bytes, Bytes>,
    cnf: Option<HashMap<Bytes, Bytes>>,
    doing_cnf_init: bool,
    expanding: HashMap<Bytes, bool>,
    db: Option<std::sync::Arc<Db>>,
    /// `kpse->followup_search`: false only for the `texmf.cnf` search.
    followup_search: bool,
    paths: HashMap<Format, Bytes>,
    /// Every search path made, by program name and format: a path is a
    /// function of those (the environment and `texmf.cnf` as the
    /// process found them), so a program name searched as before (a
    /// driver switching names between lookups) does not expand its
    /// variables again. ([`Kpse::drop_cwd`] changes `paths` only.)
    made: HashMap<(Bytes, Format), Bytes>,
    element_dirs_cache: HashMap<Bytes, Vec<Bytes>>,
    /// Inside [`Kpse::find_file_trail`]: each candidate tried and not
    /// found, in order.
    trail: Option<Vec<Bytes>>,
}

impl Kpse {
    /// `kpathsea_set_program_name`: `selfdir` is the directory of the
    /// executable (web2c resolves `argv[0]` along `PATH` and symlinks),
    /// `progname` the program name, `engine` web2c's `TEXMFENGINENAME`.
    #[must_use]
    pub fn new(selfdir: &Path, progname: &str, engine: &str) -> Self {
        let mut env_overlay = HashMap::new();
        let loc = selfdir.as_os_str().as_bytes().to_vec();
        let dir = dirname(&loc);
        let parent = dirname(&dir);
        let grandparent = dirname(&parent);
        env_overlay.insert(b"SELFAUTOLOC".to_vec(), loc);
        env_overlay.insert(b"SELFAUTODIR".to_vec(), dir);
        env_overlay.insert(b"SELFAUTOPARENT".to_vec(), parent);
        env_overlay.insert(b"SELFAUTOGRANDPARENT".to_vec(), grandparent);
        env_overlay.insert(b"progname".to_vec(), progname.as_bytes().to_vec());
        env_overlay.insert(b"engine".to_vec(), engine.as_bytes().to_vec());
        Self {
            program_name: progname.as_bytes().to_vec(),
            env_overlay,
            cnf: None,
            doing_cnf_init: false,
            expanding: HashMap::new(),
            db: None,
            followup_search: false,
            paths: HashMap::new(),
            made: HashMap::new(),
            element_dirs_cache: HashMap::new(),
            trail: None,
        }
    }

    /// The variables kpathsea puts into its process's environment
    /// (`xputenv`: `SELFAUTOLOC` and the others, `progname`, `engine`),
    /// which a command TeX runs inherits.
    pub fn exported(&self) -> impl Iterator<Item = (&[u8], &[u8])> {
        self.env_overlay
            .iter()
            .map(|(k, v)| (k.as_slice(), v.as_slice()))
    }

    /// `kpathsea_reset_program_name`: search as the program `progname`
    /// from now on (the search paths made so far are made again, but for
    /// `texmf.cnf`'s and the databases').
    pub fn reset_program_name(&mut self, progname: &[u8]) {
        if self.program_name == progname {
            return;
        }
        self.program_name = progname.to_vec();
        self.env_overlay
            .insert(b"progname".to_vec(), progname.to_vec());
        self.paths
            .retain(|&f, _| f == Format::Cnf || f == Format::Db);
    }

    /// `getenv`, with kpathsea's own `xputenv` settings first; empty
    /// values count as unset.
    fn getenv(&self, name: &[u8]) -> Option<Bytes> {
        if let Some(v) = self.env_overlay.get(name) {
            return Some(v.clone()).filter(|v| !v.is_empty());
        }
        std::env::var_os(std::ffi::OsStr::from_bytes(name))
            .map(OsStringExt::into_vec)
            .filter(|v| !v.is_empty())
    }

    fn dotted(&self, var: &[u8], sep: u8) -> Bytes {
        [var, &[sep], &self.program_name].concat()
    }

    /// `cnf.c`, `kpathsea_cnf_get`.
    fn cnf_get(&mut self, name: &[u8]) -> Option<Bytes> {
        if self.doing_cnf_init {
            return None;
        }
        if self.cnf.is_none() {
            self.cnf = Some(HashMap::new());
            self.doing_cnf_init = true;
            self.read_all_cnf();
            self.doing_cnf_init = false;
            self.init_db();
        }
        let cnf = self.cnf.as_ref().expect("just read");
        cnf.get(&self.dotted(name, b'.'))
            .or_else(|| cnf.get(name))
            .cloned()
    }

    /// `cnf.c`, `read_all_cnf`.
    fn read_all_cnf(&mut self) {
        let cnf_path = self.init_format(Format::Cnf);
        let files = self.search_list(&cnf_path, &[b"texmf.cnf".to_vec()], true, true);
        let mut table = HashMap::new();
        for f in files {
            let Ok(text) = std::fs::read(os(&f)) else {
                continue;
            };
            let mut lines = text.split(|&c| c == b'\n');
            while let Some(line) = lines.next() {
                let mut line = trim_end(line).to_vec();
                // Concatenate consecutive lines that end with \.
                while line.last() == Some(&b'\\') {
                    line.pop();
                    match lines.next() {
                        Some(next) => line.extend_from_slice(next),
                        None => break,
                    }
                    let len = trim_end(&line).len();
                    line.truncate(len);
                }
                if let Some((lhs, value)) = do_line(&line) {
                    // The first file (and first definition) wins.
                    table.entry(lhs).or_insert(value);
                }
            }
        }
        self.cnf = Some(table);
    }

    /// `variable.c`, `kpathsea_var_value`.
    pub fn var_value(&mut self, var: &str) -> Option<Bytes> {
        let var = var.as_bytes();
        let value = self
            .getenv(&self.dotted(var, b'.'))
            .or_else(|| self.getenv(&self.dotted(var, b'_')))
            .or_else(|| self.getenv(var))
            .or_else(|| self.cnf_get(var))?;
        Some(self.expand(&value))
    }

    /// `expand.c`, `kpathsea_expand`: variable, then tilde expansion.
    fn expand(&mut self, s: &[u8]) -> Bytes {
        let v = self.var_expand(s);
        self.tilde_expand(&v)
    }

    /// `variable.c`, `expand`: append the value of `var` (true if set).
    fn expand_var(&mut self, out: &mut Bytes, var: &[u8]) -> bool {
        if self.expanding.get(var).copied().unwrap_or(false) {
            // "variable references itself (eventually)"
            return false;
        }
        let value = self
            .getenv(&self.dotted(var, b'_'))
            .or_else(|| self.getenv(var))
            .or_else(|| self.cnf_get(var));
        let Some(value) = value else { return false };
        self.expanding.insert(var.to_vec(), true);
        let x = self.expand(&value);
        self.expanding.insert(var.to_vec(), false);
        out.extend_from_slice(&x);
        true
    }

    /// `variable.c`, `kpathsea_var_expand`.
    fn var_expand(&mut self, src: &[u8]) -> Bytes {
        let is_var_char = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
        let mut out = Vec::with_capacity(src.len());
        let mut s = 0;
        while s < src.len() {
            if src[s] != b'$' {
                out.push(src[s]);
                s += 1;
                continue;
            }
            s += 1;
            if s < src.len() && is_var_char(src[s]) {
                let mut end = s;
                while end < src.len() && is_var_char(src[end]) {
                    end += 1;
                }
                if !self.expand_var(&mut out, &src[s..end]) {
                    // Keep the literal $x, for file names with dollars.
                    out.extend_from_slice(&src[s - 1..end]);
                }
                s = end;
            } else if s < src.len() && src[s] == b'{' {
                let start = s + 1;
                let mut end = start;
                while end < src.len() && src[end] != b'}' {
                    end += 1;
                }
                if end < src.len() {
                    self.expand_var(&mut out, &src[start..end]);
                    s = end + 1;
                } else {
                    // "No matching } for ${"
                    s = end;
                }
            } else {
                // "Unrecognized variable construct": keep the characters.
                out.push(b'$');
                if s < src.len() {
                    out.push(src[s]);
                    s += 1;
                }
            }
        }
        out
    }

    /// `tilde.c`, `kpathsea_tilde_expand` (without `~user`).
    fn tilde_expand(&self, name: &[u8]) -> Bytes {
        let (prefix, rest): (&[u8], &[u8]) = match name.strip_prefix(b"!!") {
            Some(r) => (b"!!", r),
            None => (b"", name),
        };
        if rest.first() != Some(&b'~') || rest.get(1).is_some_and(|&c| c != b'/') {
            return name.to_vec();
        }
        let mut home = self.getenv(b"HOME").unwrap_or_else(|| b".".to_vec());
        if home.starts_with(b"//") {
            home.remove(0);
        }
        let mut c = 1;
        if rest.len() > 1 && home.ends_with(b"/") {
            c += 1;
        }
        [prefix, &home, &rest[c..]].concat()
    }

    /// `expand.c`, `kpathsea_brace_expand_element`.
    fn brace_expand_element(&mut self, elt: &[u8]) -> Bytes {
        let mut pos = 0;
        let expansions = brace_expand_list(elt, &mut pos);
        let mut ret: Vec<Bytes> = Vec::new();
        for e in expansions {
            let mut x = self.expand(&e);
            if x != e {
                x = self.brace_expand_element(&x);
            }
            ret.push(x);
        }
        ret.join(&b':')
    }

    /// `expand.c`, `kpathsea_brace_expand` (with `KPSE_DOT` handling).
    fn brace_expand(&mut self, path: &[u8]) -> Bytes {
        let xpath = self.var_expand(path);
        let parts: Vec<Bytes> = path_elements(&xpath)
            .into_iter()
            .map(|e| self.brace_expand_element(&e))
            .collect();
        let ret = parts.join(&b':');
        match self.getenv(b"KPSE_DOT") {
            None => ret,
            Some(dot) => {
                let mut out: Vec<Bytes> = Vec::new();
                for elt in path_elements(&ret) {
                    if absolute_p(&elt, false) || elt.starts_with(b"!!") {
                        out.push(elt);
                    } else if elt == b"." {
                        out.push(dot.clone());
                    } else if elt.starts_with(b"./") {
                        out.push([dot.as_slice(), &elt[1..]].concat());
                    } else if !elt.is_empty() {
                        out.push([dot.as_slice(), b"/", &elt].concat());
                    }
                }
                out.join(&b':')
            }
        }
    }

    /// `tex-file.c`, `init_path` and `kpathsea_init_format`: the search
    /// path of `format`.
    fn init_format(&mut self, format: Format) -> Bytes {
        if let Some(p) = self.paths.get(&format) {
            return p.clone();
        }
        if let Some(p) = self.made.get(&(self.program_name.clone(), format)) {
            let p = p.clone();
            self.paths.insert(format, p.clone());
            return p;
        }
        let info = format_info(format);
        let (envs, default_path): (Vec<Bytes>, Bytes) =
            if matches!(format, Format::ProgramText | Format::ProgramBinary) {
                let prog = &self.program_name;
                (
                    vec![[prog.to_ascii_uppercase().as_slice(), b"INPUTS"].concat()],
                    [b".:$TEXMF/", prog.as_slice(), b"//"].concat(),
                )
            } else {
                (
                    info.envs.iter().map(|e| e.as_bytes().to_vec()).collect(),
                    info.default_path.as_bytes().to_vec(),
                )
            };
        let mut env_value: Option<Bytes> = None;
        let mut cnf_path: Option<Bytes> = None;
        for env in &envs {
            let env = env.as_slice();
            if env_value.is_none() {
                env_value = self
                    .getenv(&self.dotted(env, b'.'))
                    .or_else(|| self.getenv(&self.dotted(env, b'_')))
                    .or_else(|| self.getenv(env));
            }
            // Don't read the cnf files to initialize the cnf path.
            if cnf_path.is_none() && format != Format::Cnf {
                cnf_path = self.cnf_get(env);
            }
            if env_value.is_some() && cnf_path.is_some() {
                break;
            }
        }
        let mut path = default_path;
        if let Some(c) = &cnf_path {
            path = expand_default(c, &path);
        }
        if let Some(mut e) = env_value {
            for c in &mut e {
                if *c == b';' {
                    *c = b':';
                }
            }
            path = expand_default(&e, &path);
        }
        let mut path = self.brace_expand(&path);
        if format == Format::Db {
            // `remove_dbonly`
            path = path_elements(&path)
                .into_iter()
                .map(|e| e.strip_prefix(b"!!").map_or(e.clone(), <[u8]>::to_vec))
                .collect::<Vec<_>>()
                .join(&b':');
        }
        self.paths.insert(format, path.clone());
        self.made
            .insert((self.program_name.clone(), format), path.clone());
        path
    }

    /// `db.c`, `kpathsea_init_db`. The databases are read once per
    /// process while their files stay as they were (by length and time):
    /// another instance of the same tree (a format's finder beside the
    /// document's) shares them.
    fn init_db(&mut self) {
        type Dbs = std::sync::Mutex<HashMap<Vec<(Bytes, u64, u128)>, std::sync::Arc<Db>>>;
        static DBS: std::sync::OnceLock<Dbs> = std::sync::OnceLock::new();
        let db_path = self.init_format(Format::Db);
        let files = self.search_list(&db_path, &[b"ls-R".to_vec(), b"ls-r".to_vec()], true, true);
        let key: Vec<(Bytes, u64, u128)> = files
            .iter()
            .map(|f| {
                let m = std::fs::metadata(os(f)).ok();
                let len = m.as_ref().map_or(0, std::fs::Metadata::len);
                let at = m
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_nanos());
                (f.clone(), len, at)
            })
            .collect();
        let dbs = DBS.get_or_init(Dbs::default);
        let mut held = dbs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(db) = held.get(&key) {
            self.db = Some(db.clone());
            return;
        }
        // (kept by an earlier process: read, not parsed again)
        let kept = db_cache_file(&key);
        if let Some(db) = kept
            .as_ref()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|b| Db::from_bytes(&b, &key))
        {
            let db = std::sync::Arc::new(db);
            held.insert(key, db.clone());
            self.db = Some(db);
            return;
        }
        let mut db = Db::default();
        let mut i = 0;
        while i < files.len() {
            // ls-R and ls-r may be the same file (case-insensitive fs).
            if i + 1 < files.len()
                && files[i].eq_ignore_ascii_case(&files[i + 1])
                && same_file(&files[i], &files[i + 1])
            {
                i += 1;
                continue;
            }
            db.build(&files[i]);
            i += 1;
        }
        db.finish();
        // (written beside, then moved in place: a reader never sees half)
        if let Some(p) = kept
            && let Some(dir) = p.parent()
            && std::fs::create_dir_all(dir).is_ok()
        {
            let tmp = p.with_extension(format!("tmp{}", std::process::id()));
            if std::fs::write(&tmp, db.to_bytes(&key)).is_ok() && std::fs::rename(&tmp, &p).is_err() {
                let _ = std::fs::remove_file(&tmp);
            }
        }
        let db = std::sync::Arc::new(db);
        held.insert(key, db.clone());
        self.db = Some(db);
    }

    /// `db.c`, `kpathsea_db_search_list`: `None` if `elt` is not covered
    /// by a database (so the caller searches the disk).
    fn db_search_list(
        &self,
        names: &[Bytes],
        elt: &[u8],
        all: bool,
        trail: &mut Option<Vec<Bytes>>,
    ) -> Option<Vec<Bytes>> {
        let db = self.db.as_ref()?;
        if !db.roots.iter().any(|d| elt_in_db(d, elt)) {
            return None;
        }
        let mut ret = Vec::new();
        for name in names {
            if absolute_p(name, true) {
                continue;
            }
            let (path, name): (Bytes, &[u8]) = match name.iter().rposition(|&c| c == b'/') {
                Some(i) if i > 0 => ([elt, b"/", &name[..i]].concat(), &name[i + 1..]),
                _ => (elt.to_vec(), name.as_slice()),
            };
            for dir in db.lookup(name) {
                let db_file = [dir, name].concat();
                if db_match(&db_file, &path) {
                    if readable_file(&db_file) {
                        ret.push(db_file);
                        if !all {
                            return Some(ret);
                        }
                    } else {
                        note_trail(trail, &db_file);
                    }
                }
            }
        }
        Some(ret)
    }

    /// `elt-dirs.c`, `kpathsea_element_dirs`: the existing directories
    /// (with trailing `/`) that path element `elt` stands for.
    fn element_dirs(&mut self, elt: &[u8]) -> Vec<Bytes> {
        if elt.is_empty() {
            return Vec::new();
        }
        if let Some(d) = self.element_dirs_cache.get(elt) {
            return d.clone();
        }
        let mut e = elt.to_vec();
        if !e.ends_with(b"/") {
            e.push(b'/');
        }
        let mut out = Vec::new();
        expand_elt(&mut out, &e, 0);
        self.element_dirs_cache.insert(elt.to_vec(), out.clone());
        out
    }

    /// `pathsearch.c`, `kpathsea_path_search_list_generic`.
    fn search_list(
        &mut self,
        path: &[u8],
        names: &[Bytes],
        must_exist: bool,
        all: bool,
    ) -> Vec<Bytes> {
        let mut trail = self.trail.take();
        let ret = self.search_list_in(path, names, must_exist, all, &mut trail);
        self.trail = trail;
        ret
    }

    /// [`Kpse::search_list`], each candidate tried and not found noted in
    /// `trail`.
    fn search_list_in(
        &mut self,
        path: &[u8],
        names: &[Bytes],
        must_exist: bool,
        all: bool,
        trail: &mut Option<Vec<Bytes>>,
    ) -> Vec<Bytes> {
        let mut ret: Vec<Bytes> = Vec::new();
        let mut all_absolute = true;
        for n in names {
            if absolute_p(n, true) {
                // `absolute_search`
                let found = if readable_file(n) {
                    Some(n.clone())
                } else {
                    note_trail(trail, n);
                    self.casefold().then(|| casefold_readable_file(n)).flatten()
                };
                if let Some(f) = found {
                    ret.push(f);
                    if !all {
                        return self.finish_search(ret);
                    }
                }
            } else {
                all_absolute = false;
            }
        }
        if !all_absolute {
            for elt in path_elements(path) {
                let (allow_disk, mut elt) = match elt.strip_prefix(b"!!") {
                    Some(e) => (false, e.to_vec()),
                    None => (true, elt),
                };
                normalize_path(&mut elt);
                let mut found = if self.followup_search {
                    self.db_search_list(names, &elt, all, trail)
                } else {
                    None
                };
                if allow_disk && found.as_ref().is_none_or(|f| must_exist && f.is_empty()) {
                    let dirs = self.element_dirs(&elt);
                    if !dirs.is_empty() {
                        let mut f = dir_list_search_list(&dirs, names, all, &mut |p| {
                            readable_file(p).then(|| p.to_vec()).or_else(|| {
                                note_trail(trail, p);
                                None
                            })
                        });
                        if f.is_empty() && self.casefold() {
                            // (still nothing: again, case-insensitively)
                            f = dir_list_search_list(
                                &dirs,
                                names,
                                all,
                                &mut casefold_readable_file,
                            );
                        }
                        found = Some(f);
                    }
                }
                if let Some(f) = found
                    && !f.is_empty()
                {
                    if all {
                        ret.extend(f);
                    } else {
                        ret.push(f.into_iter().next().expect("nonempty"));
                        break;
                    }
                }
            }
        }
        self.finish_search(ret)
    }

    /// `texmf_casefold_search`: a name not found as it is is looked for
    /// case-insensitively in each directory searched (`pathsearch.c`).
    pub fn casefold(&mut self) -> bool {
        cnf_p(self.var_value("texmf_casefold_search").as_deref())
    }

    /// `tex-file.c`, `kpathsea_out_name_ok` (TeX Live 2026, not
    /// extended): whether a file may be written as `name`, under
    /// `openout_any` (default `p`). `Err` holds the `openout_any` value
    /// for the refusal's message (`Not writing to NAME (openout_any = p;
    /// no extended check).`). Reading is never checked: as of 2026
    /// kpathsea allows any file to be read, whatever `openin_any` says.
    ///
    /// # Errors
    /// The value of `openout_any` when `name` is refused.
    pub fn out_name_ok(&mut self, name: &[u8]) -> Result<(), Bytes> {
        let choice = self
            .var_value("openout_any")
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| b"p".to_vec());
        if matches!(choice[0], b'a' | b'y' | b'1') {
            return Ok(());
        }
        let expanded = self.expand(name);
        // no `.rhosts`, `.ssh/`, `..x`: a dot at the start or after a
        // `/` must begin `./` or `../`
        for (i, &c) in name.iter().enumerate() {
            if c != b'.' || i > 0 && name[i - 1] != b'/' {
                continue;
            }
            let next = name.get(i + 1).copied();
            let dot_slash = next == Some(b'/');
            let dot_dot_slash = next == Some(b'.') && name.get(i + 2) == Some(&b'/');
            if !dot_slash && !dot_dot_slash {
                return Err(choice);
            }
        }
        if matches!(choice[0], b'r' | b'n' | b'0') {
            return Ok(());
        }
        if expanded.first() == Some(&b'/') {
            // (`abs_fname_ok`: inside TEXMF_OUTPUT_DIRECTORY, from the
            // environment only, or TEXMFOUTPUT)
            let inside = |dir: Option<Bytes>| {
                dir.filter(|d| !d.is_empty()).is_some_and(|d| {
                    expanded.starts_with(&d)
                        && (expanded.get(d.len()) == Some(&b'/') || expanded.len() == d.len())
                })
            };
            let outdir = std::env::var_os("TEXMF_OUTPUT_DIRECTORY").map(OsStringExt::into_vec);
            if !inside(outdir) && !inside(self.var_value("TEXMFOUTPUT")) {
                return Err(choice);
            }
        }
        // no `../` at the start, no `/../` anywhere
        if name.starts_with(b"../") || name.windows(4).any(|w| w == b"/../") {
            return Err(choice);
        }
        Ok(())
    }

    fn finish_search(&mut self, mut ret: Vec<Bytes>) -> Vec<Bytes> {
        // `str_list_uniqify`
        let mut seen = std::collections::HashSet::new();
        ret.retain(|r| seen.insert(r.clone()));
        self.followup_search = true;
        ret
    }

    /// The search path of `format`, as `kpsewhich -show-path` prints it.
    pub fn show_path(&mut self, format: Format) -> Bytes {
        self.cnf_get(b"");
        self.init_format(format)
    }

    /// The search path of `format` without its `.` elements (the current
    /// directory) from now on, until the program name changes: a run that
    /// must not read the directory it was started in, as fmtutil keeps a
    /// format's build out of it by running it in a scratch directory.
    pub fn drop_cwd(&mut self, format: Format) {
        let path = self.show_path(format);
        let kept: Vec<Bytes> = path_elements(&path)
            .into_iter()
            .filter(|e| !matches!(e.as_slice(), b"." | b"./"))
            .collect();
        self.paths.insert(format, kept.join(&b':'));
    }

    /// Every directory the search path of `format` covers, in the order
    /// a search tries them, each once: an element's `!!` dropped (its
    /// directories are the database's or the disk's alike) and its `//`
    /// expanded to the directories below it (`elt-dirs.c`). A caller that
    /// looks files up by itself (the flow builds' input directories)
    /// finds what [`Kpse::find_file`] finds, but for `ls-R`'s order among
    /// equal names below one `//`.
    pub fn search_dirs(&mut self, format: Format) -> Vec<Bytes> {
        let path = self.show_path(format);
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for elt in path_elements(&path) {
            let mut elt = elt.strip_prefix(b"!!").map_or(elt.clone(), <[u8]>::to_vec);
            normalize_path(&mut elt);
            for d in self.element_dirs(&elt) {
                if seen.insert(d.clone()) {
                    out.push(d);
                }
            }
        }
        out
    }

    /// [`Kpse::find_file`], and the candidates it tried and did not find,
    /// in order: while none of them is there and the file found still is,
    /// the same search finds the same file, or none.
    pub fn find_file_trail(
        &mut self,
        name: &[u8],
        format: Format,
        must_exist: bool,
    ) -> (Option<Bytes>, Vec<Bytes>) {
        self.trail = Some(Vec::new());
        let found = self.find_file(name, format, must_exist);
        (found, self.trail.take().unwrap_or_default())
    }

    /// `tex-file.c`, `kpathsea_find_file_generic` (`all` = false):
    /// find `name` for `format`; `must_exist` allows a disk search of
    /// database-covered directories when the databases have no match.
    pub fn find_file(&mut self, name: &[u8], format: Format, must_exist: bool) -> Option<Bytes> {
        self.cnf_get(b""); // read texmf.cnf and the databases
        let info = format_info(format);
        let path = self.init_format(format);
        let name = self.expand(name);
        let has_any_suffix = name
            .iter()
            .rposition(|&c| c == b'.')
            .is_some_and(|i| !name[i..].contains(&b'/'));
        let ends_with = |s: &str| name.len() >= s.len() && name.ends_with(s.as_bytes());
        let has_potential_suffix = info.suffixes.iter().any(|s| ends_with(s))
            || info.alt_suffixes.iter().any(|s| ends_with(s));
        let asis = |t: &mut Vec<Bytes>| {
            if has_potential_suffix || !info.suffix_search_only {
                t.push(name.clone());
            }
        };
        let suffixed = |t: &mut Vec<Bytes>| {
            if !has_potential_suffix {
                for s in info.suffixes {
                    t.push([name.as_slice(), s.as_bytes()].concat());
                }
            }
        };
        let mut target = Vec::new();
        let try_std = self.var_value("try_std_extension_first");
        if has_any_suffix && !cnf_p(try_std.as_deref()) {
            asis(&mut target);
            suffixed(&mut target);
        } else {
            suffixed(&mut target);
            asis(&mut target);
        }
        let ret = self.search_list(&path, &target, false, false);
        if let Some(r) = ret.into_iter().next() {
            return Some(r);
        }
        if must_exist {
            let mut target = Vec::new();
            if !has_potential_suffix && info.suffix_search_only {
                for s in info.suffixes {
                    target.push([name.as_slice(), s.as_bytes()].concat());
                }
            }
            if has_potential_suffix || !info.suffix_search_only {
                target.push(name.clone());
            }
            return self
                .search_list(&path, &target, true, false)
                .into_iter()
                .next();
        }
        None
    }
}

/// `cnf.c`, `do_line`: parse one (joined) line into (`VAR` or
/// `VAR.prog`, value), `None` for blank, comment or malformed lines.
fn do_line(line: &[u8]) -> Option<(Bytes, Bytes)> {
    let mut line = line.to_vec();
    let start = line.iter().position(|c| !c.is_ascii_whitespace())?;
    line.drain(..start);
    if line.is_empty() || line[0] == b'%' || line[0] == b'#' {
        return None;
    }
    // Remove a trailing comment: a % or # preceded by whitespace (the C
    // code scans from the end, wiping each comment and the whitespace
    // before it).
    let mut v = line.len() - 1;
    while v > 0 {
        if line[v] == b'%' || line[v] == b'#' {
            let mut w = v - 1;
            let mut wiped = false;
            while w > 0 && line[w].is_ascii_whitespace() {
                wiped = true;
                w -= 1;
            }
            if wiped {
                line.truncate(w + 1);
            }
            v = w;
            continue;
        }
        v -= 1;
    }
    let mut i = 0;
    while i < line.len() && !line[i].is_ascii_whitespace() && line[i] != b'=' && line[i] != b'.' {
        i += 1;
    }
    if i == 0 {
        return None; // "No cnf variable name"
    }
    let var = line[..i].to_vec();
    while i < line.len() && line[i].is_ascii_whitespace() {
        i += 1;
    }
    let mut prog = None;
    if line.get(i) == Some(&b'.') {
        i += 1;
        while i < line.len() && line[i].is_ascii_whitespace() {
            i += 1;
        }
        let s = i;
        while i < line.len() && !line[i].is_ascii_whitespace() && line[i] != b'=' {
            i += 1;
        }
        let p = &line[s..i];
        if p.is_empty() || p.iter().any(|&c| b"${}".contains(&c) || is_kpse_sep(c)) {
            return None;
        }
        prog = Some(p.to_vec());
    }
    while i < line.len() && line[i].is_ascii_whitespace() {
        i += 1;
    }
    if line.get(i) == Some(&b'=') {
        i += 1;
        while i < line.len() && line[i].is_ascii_whitespace() {
            i += 1;
        }
    }
    let mut value = trim_end(&line[i..]).to_vec();
    if value.is_empty() {
        return None; // "No cnf value"
    }
    for c in &mut value {
        if *c == b';' {
            *c = b':';
        }
    }
    let lhs = match prog {
        Some(p) => [var.as_slice(), b".", &p].concat(),
        None => var,
    };
    Some((lhs, value))
}

fn trim_end(s: &[u8]) -> &[u8] {
    let n = s
        .iter()
        .rposition(|c| !c.is_ascii_whitespace())
        .map_or(0, |i| i + 1);
    &s[..n]
}

fn dirname(p: &[u8]) -> Bytes {
    match p.iter().rposition(|&c| c == b'/') {
        Some(0) => b"/".to_vec(),
        Some(i) => p[..i].to_vec(),
        None => b".".to_vec(),
    }
}

#[cfg(unix)]
fn same_file(a: &[u8], b: &[u8]) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (std::fs::metadata(os(a)), std::fs::metadata(os(b))) {
        (Ok(x), Ok(y)) => x.dev() == y.dev() && x.ino() == y.ino(),
        _ => false,
    }
}

/// WASI has no device and inode numbers: the same path is the same file.
#[cfg(not(unix))]
fn same_file(a: &[u8], b: &[u8]) -> bool {
    a == b
}

/// `elt-dirs.c`, `kpathsea_normalize_path` (Unix): collapse leading
/// slashes to one.
fn normalize_path(elt: &mut Bytes) {
    let n = elt.iter().take_while(|&&c| c == b'/').count();
    if n > 1 {
        elt.drain(1..n);
    }
}

/// `elt-dirs.c`, `expand_elt`: add the directories for `elt`, whose
/// part before `start` has been expanded already.
fn expand_elt(out: &mut Vec<Bytes>, elt: &[u8], start: usize) {
    let mut d = start;
    while d < elt.len() {
        if elt[d] == b'/' && elt.get(d + 1) == Some(&b'/') {
            let mut post = d + 1;
            while post < elt.len() && elt[post] == b'/' {
                post += 1;
            }
            do_subdir(out, &elt[..=d], &elt[post..]);
            return;
        }
        d += 1;
    }
    // `checked_dir_list_add`
    if std::fs::metadata(os(elt)).is_ok_and(|m| m.is_dir()) && !out.iter().any(|o| o == elt) {
        out.push(elt.to_vec());
    }
}

/// `elt-dirs.c`, `do_subdir`: `name` (ending in `/`) and all its
/// subdirectories, matching `post`. Entries come in `readdir` order,
/// like kpathsea.
fn do_subdir(out: &mut Vec<Bytes>, name: &[u8], post: &[u8]) {
    let Ok(rd) = std::fs::read_dir(os(name)) else {
        return;
    };
    if post.is_empty() {
        if !out.iter().any(|o| o == name) {
            out.push(name.to_vec());
        }
    } else {
        expand_elt(out, &[name, post].concat(), name.len());
    }
    for e in rd.flatten() {
        let fname = e.file_name();
        let fname = fname.as_bytes();
        if fname.first() == Some(&b'.') {
            continue;
        }
        let potential = [name, fname].concat();
        // `kpathsea_dir_links`: only directories (following symlinks).
        if !std::fs::metadata(os(&potential)).is_ok_and(|m| m.is_dir()) {
            continue;
        }
        let mut sub = potential.clone();
        sub.push(b'/');
        if !post.is_empty() {
            expand_elt(out, &[sub.as_slice(), post].concat(), sub.len());
        }
        // TeX Live sets `texmf_nlink_for_leaf` false, so always recurse.
        do_subdir(out, &sub, post);
    }
}

/// `pathsearch.c`, `dir_list_search_list`: `readable` is
/// `kpathsea_readable_file` or `casefold_readable_file`, giving the name
/// found.
fn dir_list_search_list(
    dirs: &[Bytes],
    names: &[Bytes],
    all: bool,
    readable: &mut dyn FnMut(&[u8]) -> Option<Bytes>,
) -> Vec<Bytes> {
    let mut ret = Vec::new();
    for dir in dirs {
        for name in names {
            if absolute_p(name, true) {
                continue;
            }
            if let Some(found) = readable(&[dir.as_slice(), name].concat()) {
                ret.push(found);
                if !all {
                    return ret;
                }
            }
        }
    }
    ret
}

/// `pathsearch.c`, `casefold_readable_file`: the first readable file of
/// `name`'s directory, in directory order, whose name equals its last
/// component but for ASCII case (`strcasecmp`).
fn casefold_readable_file(name: &[u8]) -> Option<Bytes> {
    let base = name.rsplit(|&c| c == b'/').next().unwrap_or(name);
    let dir = dirname(name);
    dir_names(&dir)?.iter().find_map(|n| {
        if !n.eq_ignore_ascii_case(base) {
            return None;
        }
        let p = [dir.as_slice(), b"/", n].concat();
        readable_file(&p).then_some(p)
    })
}

/// The names in directory `dir`, in `readdir` order. A name not found
/// looks through its directories case-insensitively, each a `readdir`;
/// a run that looks up the same missing names again (an SSA rebuild's
/// ship-out asking for each font's `.vf`, `\IfFileExists`) reads the
/// listing kept while the directory's modification time is the same.
/// A listing is kept only once that time is settled, two seconds in the
/// past when it was made (git's racy-timestamp rule): an entry made in
/// the same clock tick as the listing can leave the time as it was.
fn dir_names(dir: &[u8]) -> Option<std::rc::Rc<[Bytes]>> {
    use std::time::{Duration, SystemTime};
    /// Each directory's listing, with the modification time it was made at.
    type Listed = HashMap<Bytes, (SystemTime, std::rc::Rc<[Bytes]>)>;
    thread_local! {
        static LISTED: std::cell::RefCell<Listed> = std::cell::RefCell::new(HashMap::new());
    }
    let mtime = std::fs::metadata(os(dir)).ok()?.modified().ok()?;
    if let Some(names) = LISTED.with_borrow(|m| {
        m.get(dir)
            .filter(|(t, _)| *t == mtime)
            .map(|(_, n)| n.clone())
    }) {
        return Some(names);
    }
    let now = SystemTime::now();
    let names: std::rc::Rc<[Bytes]> = std::fs::read_dir(os(dir))
        .ok()?
        .flatten()
        .map(|e| e.file_name().into_vec())
        .collect();
    LISTED.with_borrow_mut(|m| {
        if mtime + Duration::from_secs(2) < now {
            m.insert(dir.to_vec(), (mtime, names.clone()));
        } else {
            m.remove(dir);
        }
    });
    Some(names)
}

/// A candidate `file` not found joins the trail.
fn note_trail(trail: &mut Option<Vec<Bytes>>, file: &[u8]) {
    if let Some(t) = trail {
        t.push(file.to_vec());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brace_expansion() {
        let mut pos = 0;
        let r = brace_expand_list(b"a{b,c}d:e", &mut pos);
        assert_eq!(r, vec![b"abd".to_vec(), b"acd".to_vec(), b"e".to_vec()]);
        let mut pos = 0;
        let r = brace_expand_list(b"x/{p,g,}//", &mut pos);
        assert_eq!(
            r,
            vec![b"x/p//".to_vec(), b"x/g//".to_vec(), b"x///".to_vec()]
        );
    }

    #[test]
    fn default_splicing() {
        assert_eq!(expand_default(b"a::b", b"D"), b"a:D:b");
        assert_eq!(expand_default(b":a", b"D"), b"D:a");
        assert_eq!(expand_default(b"a:", b"D"), b"a:D");
        assert_eq!(expand_default(b"a", b"D"), b"a");
    }

    #[test]
    fn cnf_lines() {
        assert_eq!(
            do_line(b"TEXINPUTS.tex = $A;$B  % comment"),
            Some((b"TEXINPUTS.tex".to_vec(), b"$A:$B".to_vec()))
        );
        assert_eq!(
            do_line(b"foo = a#b  %c"),
            Some((b"foo".to_vec(), b"a#b".to_vec()))
        );
        assert_eq!(do_line(b"  % only a comment"), None);
    }

    #[test]
    fn a_kept_database_reads_back_as_built() {
        let dir = std::env::temp_dir().join(format!("phitex-kpse-db-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let ls = dir.join("ls-R");
        std::fs::write(&ls, b"% ls-R\n./a:\nx.sty\ny.tex\n\n./b:\nx.sty\nz\n").expect("write");
        let f = ls.as_os_str().as_bytes().to_vec();
        let mut db = Db::default();
        assert!(db.build(&f));
        db.finish();
        let dirs = |db: &Db, n: &[u8]| db.lookup(n).map(<[u8]>::to_vec).collect::<Vec<_>>();
        let top = dir.as_os_str().as_bytes().to_vec();
        let d = |s: &str| [top.as_slice(), b"/", s.as_bytes()].concat();
        assert_eq!(dirs(&db, b"x.sty"), [d("a/"), d("b/")]);
        assert_eq!(dirs(&db, b"z"), [d("b/")]);
        assert_eq!(dirs(&db, b"w").len(), 0);
        let key = vec![(f.clone(), 7, 9)];
        let b = db.to_bytes(&key);
        let e = Db::from_bytes(&b, &key).expect("read back");
        assert_eq!(e.to_bytes(&key), b);
        assert_eq!(dirs(&e, b"x.sty"), [d("a/"), d("b/")]);
        assert_eq!(e.roots, db.roots);
        assert!(Db::from_bytes(&b, &[(f, 7, 10)]).is_none());
        assert!(Db::from_bytes(&b[..b.len() - 1], &key).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn db_matching() {
        assert!(db_match(b"/t/tex/plain/base/plain.tex", b"/t/tex//"));
        assert!(db_match(
            b"/t/tex/plain/base/plain.tex",
            b"/t/tex/plain/base"
        ));
        assert!(!db_match(b"/t/tex/plain/base/plain.tex", b"/t/tex/plain"));
        assert!(db_match(
            b"/t/fonts/tfm/public/cm/cmr10.tfm",
            b"/t/fonts//cm"
        ));
    }

    #[test]
    fn out_name_ok_paranoid() {
        let mut k = Kpse::new(Path::new("/nonexistent/bin"), "pdftex", "pdftex");
        k.env_overlay.insert(b"openout_any".to_vec(), b"p".to_vec());
        for ok in ["x.tex", "dir/x.aux", "./x.tex", "a.b.c", "x/./y", "a..b"] {
            assert!(k.out_name_ok(ok.as_bytes()).is_ok(), "{ok}");
        }
        for refused in [
            ".tex", ".bashrc", "d/.ssh/k", "..x", "/etc/x", "../x", "a/../x", ".",
        ] {
            assert!(k.out_name_ok(refused.as_bytes()).is_err(), "{refused}");
        }
        k.env_overlay.insert(b"openout_any".to_vec(), b"r".to_vec());
        assert!(k.out_name_ok(b"/etc/x").is_ok());
        assert!(k.out_name_ok(b".bashrc").is_err());
        k.env_overlay.insert(b"openout_any".to_vec(), b"a".to_vec());
        assert!(k.out_name_ok(b".bashrc").is_ok());
    }

    #[test]
    fn casefolding() {
        let dir = std::env::temp_dir().join(format!("phitex-kpse-casefold-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("TeXGyre.fontspec"), b"").unwrap();
        let d = dir.as_os_str().as_bytes();
        let name = |n: &str| [d, b"/", n.as_bytes()].concat();
        assert_eq!(
            casefold_readable_file(&name("texgyre.FONTSPEC")),
            Some(name("TeXGyre.fontspec"))
        );
        assert_eq!(casefold_readable_file(&name("texgyre.fontspe")), None);
        assert_eq!(casefold_readable_file(&name("none/texgyre.fontspec")), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
