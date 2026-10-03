//! pdfTeX's font map (mapfile.c): which Type 1 or TrueType file, encoding
//! and PostScript name each TFM font gets in the PDF.
//!
//! Map files are big (TeX Live's `pdftex.map` has 40 000 lines) and change
//! rarely, so the table sits behind an [`Arc`]: checkpoints share it, and
//! `\pdfmapfile`/`\pdfmapline` copy it on write. Which entries have been
//! used lives outside (it changes as fonts are shipped).

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::sync::Arc;
use alloc::vec::Vec;

/// mapfile.c's `updatemode`: what a map item does with an entry that
/// already exists.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Mode {
    /// `+`: keep the old entry (`FM_DUPIGNORE`).
    DupIgnore,
    /// `=`: replace it (`FM_REPLACE`).
    Replace,
    /// `-`: delete it (`FM_DELETE`).
    Delete,
}

/// ptexmac.h's `F_*` bits of an entry's `type`.
pub(crate) const F_INCLUDED: u8 = 0x01;
pub(crate) const F_SUBSETTED: u8 = 0x02;
pub(crate) const F_STDT1FONT: u8 = 0x04;
pub(crate) const F_TYPE1: u8 = 0x10;
pub(crate) const F_TRUETYPE: u8 = 0x20;
pub(crate) const F_OTF: u8 = 0x40;
pub(crate) const F_PK: u8 = 0x80;

/// mapfile.c's `fm_entry`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct MapEntry {
    pub tfm_name: Vec<u8>,
    pub ps_name: Option<Vec<u8>>,
    /// The font descriptor's `/Flags`; -1 if the line gives none.
    pub fd_flags: i32,
    pub ff_name: Option<Vec<u8>>,
    pub encname: Option<Vec<u8>>,
    pub kind: u8,
    /// `SlantFont` and `ExtendFont`, times 1000.
    pub slant: i32,
    pub extend: i32,
    pub pid: i32,
    pub eid: i32,
}

partex_engine::persist_struct!(MapEntry {
    tfm_name,
    ps_name,
    fd_flags,
    ff_name,
    encname,
    kind,
    slant,
    extend,
    pid,
    eid
});

impl MapEntry {
    fn new() -> Self {
        Self {
            tfm_name: Vec::new(),
            ps_name: None,
            fd_flags: -1,
            ff_name: None,
            encname: None,
            kind: 0,
            slant: 0,
            extend: 0,
            pid: -1,
            eid: -1,
        }
    }

    pub(crate) fn is(&self, bit: u8) -> bool {
        self.kind & bit != 0
    }

    fn is_t1fontfile(&self) -> bool {
        self.ff_name.is_some() && self.is(F_TYPE1)
    }

    fn is_reencoded(&self) -> bool {
        self.encname.is_some()
    }

    /// The `ps_tree` key.
    fn ps_key(&self) -> Option<(Vec<u8>, i32, i32)> {
        Some((self.ps_name.clone()?, self.slant, self.extend))
    }
}

/// The map: `tfm_tree` and `ps_tree`, sharing entries; or a map file
/// read into an empty table, parsed where it is looked up (`lazy`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) struct Table {
    by_tfm: BTreeMap<Vec<u8>, Arc<MapEntry>>,
    by_ps: BTreeMap<(Vec<u8>, i32, i32), Arc<MapEntry>>,
    lazy: Option<Lazy>,
}

partex_engine::persist_struct!(Table {
    by_tfm,
    by_ps,
    lazy
});

/// A map file read with duplicates ignored into an empty table, as its
/// contents: a document looks up a few dozen of TeX Live's 42 000
/// entries, so each is parsed when first looked up. The entry of a TFM
/// name is its first valid line, as registering every line in order
/// would leave it (`ps_tree` is only ever written: nothing to keep).
/// Anything that changes the table first registers every line
/// ([`Table::materialize`]). The table holds only the contents (it is
/// shared between engines); what lookups found is the engine's
/// [`Lookups`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct Lazy {
    data: Arc<[u8]>,
}

partex_engine::persist_struct!(Lazy { data });

/// The lookups in a lazily read map file (see [`Lazy`]): per engine, so
/// the shared table stays immutable. A clone (a checkpoint) shares the
/// index and copies the few entries found (the same `Arc`s: fonts share
/// entries by identity).
#[derive(Debug, Clone, Default)]
pub(crate) struct Lookups {
    /// The first line of each first field's hash, by offset.
    index: Option<Arc<crate::u64map::U64Map<u32>>>,
    /// The entries looked up so far.
    found: BTreeMap<Vec<u8>, Option<Arc<MapEntry>>>,
    /// The `ps_tree` entries looked up so far (by PostScript name, slant
    /// and extend).
    ps_found: BTreeMap<(Vec<u8>, i32, i32), Option<Arc<MapEntry>>>,
}

/// With the entries found: a loaded table must hand out the same ones
/// (the index is built again).
impl partex_engine::persist::Persist for Lookups {
    fn save(&self, s: &mut partex_engine::persist::Saver) {
        self.found.save(s);
    }
    fn load(l: &mut partex_engine::persist::Loader) -> Option<Self> {
        Some(Self {
            index: None,
            found: partex_engine::persist::Persist::load(l)?,
            ps_found: BTreeMap::new(),
        })
    }
}

impl Lazy {
    fn new(data: Arc<[u8]>) -> Self {
        Self { data }
    }

    /// The first field of the line at `at` (as `map_file_lines` and
    /// `read_field` would see it; `None` for blank lines and comments),
    /// and where the next line starts.
    fn first_field(data: &[u8], at: usize) -> (Option<&[u8]>, usize) {
        let line_end = data[at..]
            .iter()
            .position(|&c| c == 10 || c == 13)
            .map_or(data.len(), |n| at + n);
        let next = (line_end + 1).min(data.len()).max(line_end);
        let mut i = at;
        while i < line_end && matches!(data[i], b' ' | 9) {
            i += 1;
        }
        if i == line_end || is_cfg_comment(data[i]) {
            return (None, next);
        }
        let end = data[i..line_end]
            .iter()
            .position(|&c| matches!(c, b' ' | 9 | b'<' | b'"'))
            .map_or(line_end, |n| i + n);
        (Some(&data[i..end]), next)
    }

    fn key(name: &[u8]) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for &b in name {
            h = (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3);
        }
        h
    }
}

impl Lookups {
    /// The first line of each first field's hash in `data`, by offset.
    fn build_index(data: &[u8]) -> crate::u64map::U64Map<u32> {
        let mut m = crate::u64map::U64Map::default();
        let mut at = 0;
        while at < data.len() {
            let (f, next) = Lazy::first_field(data, at);
            if let Some(f) = f {
                let k = Lazy::key(f);
                if m.get(k).is_none() {
                    m.insert(k, u32::try_from(at).unwrap_or(u32::MAX));
                }
            }
            at = next;
        }
        m
    }

    fn index(&mut self, data: &[u8]) -> &crate::u64map::U64Map<u32> {
        self.index
            .get_or_insert_with(|| Arc::new(Self::build_index(data)))
    }

    /// The entry for `tfm` in `lazy`: its first line that scans as a
    /// valid entry.
    fn lookup(&mut self, lazy: &Lazy, tfm: &[u8]) -> Option<Arc<MapEntry>> {
        if let Some(e) = self.found.get(tfm) {
            return e.clone();
        }
        let data = &lazy.data[..];
        // (the index holds the first line of the name's hash: no line of
        // the name comes before it)
        let mut at = self
            .index(data)
            .get(Lazy::key(tfm))
            .map_or(data.len(), |a| a as usize);
        let mut found = None;
        let mut warn = Vec::new();
        while at < data.len() {
            let (f, next) = Lazy::first_field(data, at);
            if f == Some(tfm) {
                let mut entry = None;
                for_map_file_lines(&data[at..next.max(at)], |line| {
                    if entry.is_none() {
                        entry = scan_line(line, &mut warn).filter(|fm| fm.tfm_name == tfm);
                    }
                });
                if let Some(fm) = entry {
                    found = Some(Arc::new(fm));
                    break;
                }
            }
            at = next;
        }
        self.found.insert(tfm.to_vec(), found.clone());
        found
    }
}

impl Lookups {
    /// The entry `ps_tree` has for `key` in `lazy`: registering every line
    /// in order puts a line's PostScript name there when the line is the
    /// first valid one of its TFM name, the name is not there yet, and the
    /// line names a Type 1 file to include. So: the first line naming it
    /// (found by its bytes, then scanned) that is all three.
    fn lookup_ps(&mut self, lazy: &Lazy, key: &(Vec<u8>, i32, i32)) -> Option<Arc<MapEntry>> {
        if let Some(e) = self.ps_found.get(key) {
            return e.clone();
        }
        let data = &lazy.data[..];
        let needle = &key.0[..];
        let mut found = None;
        let mut at = 0;
        while !needle.is_empty()
            && let Some(i) = data[at..].windows(needle.len()).position(|w| w == needle)
        {
            let pos = at + i;
            let start = data[..pos]
                .iter()
                .rposition(|&c| c == 10 || c == 13)
                .map_or(0, |p| p + 1);
            let end = data[pos..]
                .iter()
                .position(|&c| c == 10 || c == 13)
                .map_or(data.len(), |p| pos + p);
            let mut entry = None;
            let mut warn = Vec::new();
            for_map_file_lines(&data[start..end], |line| {
                if entry.is_none() {
                    entry = scan_line(line, &mut warn);
                }
            });
            if let Some(fm) = entry
                && fm.ps_key().as_ref() == Some(key)
                && fm.is_t1fontfile()
                && fm.is(F_INCLUDED)
            {
                let first = if fm.tfm_name == b"<nontfm>" {
                    Some(Arc::new(fm.clone()))
                } else {
                    self.lookup(lazy, &fm.tfm_name)
                };
                if let Some(f) = first
                    && *f == fm
                {
                    found = Some(f);
                    break;
                }
            }
            at = end.max(pos + 1);
        }
        self.ps_found.insert(key.clone(), found.clone());
        found
    }
}

/// What a map file's contents give, kept by identity (not state: the
/// engine's cache, as `cs_cache` is): the host hands out the same `Arc`
/// for a file as it was, and the step that ships the first page, which
/// reads the default map, runs again at each keystroke of a one-page
/// document. It hashed the 5 MB file twice and indexed its 42 000 lines
/// each time (two thirds of such a rebuild).
#[derive(Clone, Debug, Default)]
pub(crate) struct MapCache {
    data: Option<Arc<[u8]>>,
    hash: u128,
    index: Option<Arc<crate::u64map::U64Map<u32>>>,
    /// The warnings a full reading of it into an empty table gave, under
    /// `\pdfsuppresswarningdupmap` or not: what the host's cache keeps
    /// across processes (`Host::cache_get`), kept here for a host that
    /// has none.
    warnings: Option<(bool, Vec<Vec<u8>>)>,
}

impl MapCache {
    /// The cache for `data`, made anew for other contents.
    fn of(&mut self, data: &Arc<[u8]>) -> &mut Self {
        if !self.data.as_ref().is_some_and(|d| Arc::ptr_eq(d, data)) {
            *self = MapCache {
                data: Some(data.clone()),
                hash: partex_engine::stablehash::StableHasher::of(&data[..]),
                index: None,
                warnings: None,
            };
        }
        self
    }

    /// The hash of `data`.
    fn hash(&mut self, data: &Arc<[u8]>) -> u128 {
        self.of(data).hash
    }

    /// The warnings a full reading of `data` gave, if kept.
    fn warnings(&mut self, data: &Arc<[u8]>, suppress: bool) -> Option<Vec<Vec<u8>>> {
        let w = self.of(data).warnings.as_ref()?;
        (w.0 == suppress).then(|| w.1.clone())
    }

    /// Keep the warnings a full reading of `data` gave.
    fn keep_warnings(&mut self, data: &Arc<[u8]>, suppress: bool, warnings: Vec<Vec<u8>>) {
        self.of(data).warnings = Some((suppress, warnings));
    }

    /// The index of `data` read lazily ([`Lookups`]).
    #[allow(
        clippy::arc_with_non_send_sync,
        reason = "an engine lives on one thread; the index is an Arc for sharing"
    )]
    fn index(&mut self, data: &Arc<[u8]>) -> Arc<crate::u64map::U64Map<u32>> {
        self.of(data)
            .index
            .get_or_insert_with(|| Arc::new(Lookups::build_index(data)))
            .clone()
    }
}

/// The engine's map state (mapfile.c's globals).
#[derive(Clone, Debug)]
pub(crate) struct FontMap {
    /// `mitem->line` while it is the default map file not yet read.
    pub pending: Option<Vec<u8>>,
    /// `tfm_tree != NULL`: some map has been read.
    pub read: bool,
    pub table: Arc<Table>,
    /// A hash of everything that made `table`: equal digests, equal
    /// tables (hashing 40 000 entries at each checkpoint would not do).
    pub digest: u128,
    /// Lookups in `table`'s lazily read file (a cache: not state).
    pub lookups: Lookups,
}

partex_engine::persist_struct!(FontMap {
    pending,
    read,
    table,
    digest,
    lookups
});

impl core::hash::Hash for FontMap {
    fn hash<S: core::hash::Hasher>(&self, h: &mut S) {
        (&self.pending, self.read, self.digest).hash(h);
    }
}

impl FontMap {
    /// The entry for a TFM name.
    pub(crate) fn lookup(&mut self, tfm: &[u8]) -> Option<Arc<MapEntry>> {
        match &self.table.lazy {
            Some(l) => self.lookups.lookup(l, tfm),
            None => self.table.by_tfm.get(tfm).cloned(),
        }
    }

    /// The `ps_tree` entry for a PostScript name, slant and extend.
    pub(crate) fn lookup_ps(&mut self, key: &(Vec<u8>, i32, i32)) -> Option<Arc<MapEntry>> {
        match &self.table.lazy {
            Some(l) => self.lookups.lookup_ps(l, key),
            None => self.table.by_ps.get(key).cloned(),
        }
    }

    /// The `FONTMAP` field's version (`pdf::val`): made from what made
    /// the table (its digest, made at each change), the default map
    /// still pending, whether a map was read.
    pub(crate) fn version(&self) -> u128 {
        partex_ssa::Version::of(self).0
    }

    /// Notes that `what` changed the table.
    fn changed<W: core::hash::Hash + ?Sized>(&mut self, what: &W) {
        self.digest = partex_engine::stablehash::StableHasher::of(&(self.digest, what));
    }
}

impl Default for FontMap {
    /// `pdf_init_map_file('pdftex.map')` (pdfTeX §1337).
    fn default() -> Self {
        Self {
            pending: Some(b"pdftex.map".to_vec()),
            read: false,
            table: Arc::default(),
            digest: 0,
            lookups: Lookups::default(),
        }
    }
}

impl Table {
    pub(crate) fn is_empty(&self) -> bool {
        self.by_tfm.is_empty() && self.by_ps.is_empty() && self.lazy.is_none()
    }

    /// Register every line of a lazily read file (its warnings were
    /// given when it was read).
    fn materialize(&mut self) {
        if let Some(l) = self.lazy.take() {
            let mut warn = Vec::new();
            for_map_file_lines(&l.data, |line| {
                if let Some(fm) = scan_line(line, &mut warn) {
                    self.register(fm, Mode::DupIgnore, |_| false, true, &mut warn);
                }
                warn.clear();
            });
        }
    }

    /// mapfile.c's `avl_do_entry`; `used` says whether an entry has been
    /// used (`in_use`).
    pub(crate) fn register(
        &mut self,
        fm: MapEntry,
        mode: Mode,
        used: impl Fn(&MapEntry) -> bool,
        suppress_dup_warning: bool,
        warn: &mut Vec<Vec<u8>>,
    ) {
        self.materialize();
        let fm = Arc::new(fm);
        if fm.tfm_name != b"<nontfm>" {
            if let Some(p) = self.by_tfm.get(&fm.tfm_name).cloned() {
                match mode {
                    Mode::DupIgnore => {
                        if !suppress_dup_warning {
                            warn.push(
                                [
                                    b"fontmap entry for `".as_slice(),
                                    &fm.tfm_name,
                                    b"' already exists, duplicates ignored",
                                ]
                                .concat(),
                            );
                        }
                        return;
                    }
                    Mode::Replace | Mode::Delete => {
                        if used(&p) {
                            warn.push(
                                [
                                    b"fontmap entry for `".as_slice(),
                                    &fm.tfm_name,
                                    b"' has been used, replace/delete not allowed",
                                ]
                                .concat(),
                            );
                            return;
                        }
                        self.by_tfm.remove(&fm.tfm_name);
                    }
                }
            }
            if mode != Mode::Delete {
                self.by_tfm.insert(fm.tfm_name.clone(), fm.clone());
            }
        }
        if let Some(key) = fm.ps_key() {
            if let Some(p) = self.by_ps.get(&key).cloned() {
                match mode {
                    Mode::DupIgnore => return,
                    Mode::Replace | Mode::Delete => {
                        if used(&p) {
                            return;
                        }
                        self.by_ps.remove(&key);
                    }
                }
            }
            if mode != Mode::Delete && fm.is_t1fontfile() && fm.is(F_INCLUDED) {
                self.by_ps.insert(key, fm);
            }
        }
    }
}

/// C's `strtol(s, &end, 10)`: the value (as `(int)` of a `long`) and
/// where it ends (0 if no number).
fn strtol(s: &[u8]) -> (i32, usize) {
    let mut i = s
        .iter()
        .take_while(|c| matches!(c, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r'))
        .count();
    let neg = match s.get(i) {
        Some(b'-') => {
            i += 1;
            true
        }
        Some(b'+') => {
            i += 1;
            false
        }
        _ => false,
    };
    let digits = s[i.min(s.len())..].iter().take_while(|c| c.is_ascii_digit()).count();
    if digits == 0 {
        return (0, 0);
    }
    let v = s[i..i + digits].iter().fold(0i64, |v, &c| {
        v.saturating_mul(10).saturating_add(i64::from(c - b'0'))
    });
    let v = if neg { -v } else { v };
    #[allow(clippy::cast_possible_truncation, reason = "C's (int) of a long")]
    (v as i32, i + digits)
}

/// `lookup_fontmap`'s reading of `<name>-Slant_<n>`, `<name>-Slant_<n>
/// ...-Extend_<n>` and `<name>-Extend_<n>` (each number running to the
/// end): the name and the slant and extend.
fn slant_extend(s: &[u8]) -> (&[u8], i32, i32) {
    let find = |h: &[u8], n: &[u8]| h.windows(n.len()).position(|w| w == n);
    if let Some(a) = find(s, b"-Slant_") {
        let b = a + 7;
        let (sl, n) = strtol(&s[b..]);
        if n != 0 && b + n == s.len() {
            return (&s[..a], sl, 0);
        }
        if n != 0
            && let Some(c) = find(&s[b + n..], b"-Extend_")
        {
            let d = b + n + c + 8;
            let (ex, m) = strtol(&s[d..]);
            if m != 0 && d + m == s.len() {
                return (&s[..a], sl, ex);
            }
        }
    } else if let Some(a) = find(s, b"-Extend_") {
        let b = a + 8;
        let (ex, n) = strtol(&s[b..]);
        if n != 0 && b + n == s.len() {
            return (&s[..a], 0, ex);
        }
    }
    (s, 0, 0)
}

/// ptexmac.h's `is_cfg_comment`.
fn is_cfg_comment(c: u8) -> bool {
    matches!(c, 10 | b'*' | b'#' | b';' | b'%')
}

/// The lines of a map file as `fm_scan_line` reads them
/// (`append_char_to_buf`): tabs become spaces, a CR ends a line as an LF
/// does, and runs of spaces (and leading ones) collapse.
pub(crate) fn map_file_lines(data: &[u8]) -> Vec<Vec<u8>> {
    let mut lines = Vec::new();
    for_map_file_lines(data, |l| lines.push(l.to_vec()));
    lines
}

/// [`map_file_lines`], each line lent to `f` in turn (one buffer).
pub(crate) fn for_map_file_lines(data: &[u8], mut f: impl FnMut(&[u8])) {
    let mut line = Vec::new();
    // `while (!fm_eof())`: a line is read while the end has not been
    // hit; the read that hits it ends a last (possibly empty) line
    let mut rest = data;
    loop {
        let end = rest.iter().position(|&c| c == 10 || c == 13);
        let chunk = &rest[..end.unwrap_or(rest.len())];
        line.clear();
        for &c in chunk {
            let c = if c == 9 { b' ' } else { c };
            if c != b' ' || line.last().is_some_and(|&l| l != b' ') {
                line.push(c);
            }
        }
        f(&line);
        match end {
            Some(e) => rest = &rest[e + 1..],
            None => break,
        }
    }
}

/// `read_field`: bytes up to a blank, `<`, `"` or the end, then one
/// blank skipped.
fn read_field<'a>(r: &mut &'a [u8]) -> &'a [u8] {
    let n = r
        .iter()
        .position(|&c| c == b' ' || c == b'<' || c == b'"')
        .unwrap_or(r.len());
    let (f, rest) = r.split_at(n);
    *r = rest;
    skip(r, b' ');
    f
}

/// ptexmac.h's `skip`: one `c`, if there.
fn skip(r: &mut &[u8], c: u8) {
    if r.first() == Some(&c) {
        *r = &r[1..];
    }
}

/// `sscanf("%f %n")`: a float, then any white space; the value and the
/// bytes taken.
fn scan_float(s: &[u8]) -> Option<(f32, usize)> {
    let mut i = s.iter().take_while(|c| c.is_ascii_whitespace()).count();
    let start = i;
    if matches!(s.get(i), Some(b'+' | b'-')) {
        i += 1;
    }
    let int = s[i..].iter().take_while(|c| c.is_ascii_digit()).count();
    i += int;
    let mut frac = 0;
    if s.get(i) == Some(&b'.') {
        frac = s[i + 1..].iter().take_while(|c| c.is_ascii_digit()).count();
        i += 1 + frac;
    }
    if int + frac == 0 {
        return None;
    }
    let mut end = i;
    if matches!(s.get(i), Some(b'e' | b'E')) {
        let mut j = i + 1;
        if matches!(s.get(j), Some(b'+' | b'-')) {
            j += 1;
        }
        let digits = s[j..].iter().take_while(|c| c.is_ascii_digit()).count();
        if digits > 0 {
            end = j + digits;
        }
    }
    let text = core::str::from_utf8(&s[start..end]).ok()?;
    let v: f32 = text.trim_end_matches('.').parse().ok().or_else(|| {
        // "5." parses in C; Rust wants a digit after the point
        text.parse().ok()
    })?;
    let blanks = s[end..]
        .iter()
        .take_while(|c| c.is_ascii_whitespace())
        .count();
    Some((v, end + blanks))
}

/// `sscanf(r, "PidEid=%i, %i %n")`.
fn scan_pid_eid(r: &[u8]) -> Option<(i32, i32, usize)> {
    let s = r.strip_prefix(b"PidEid=")?;
    let (a, n) = crate::charset::strtol(s)?;
    let mut i = 7 + n;
    if r.get(i) != Some(&b',') {
        return None;
    }
    i += 1;
    i += r[i..]
        .iter()
        .take_while(|c| c.is_ascii_whitespace())
        .count();
    let (b, n) = crate::charset::strtol(&r[i..])?;
    i += n;
    i += r[i..]
        .iter()
        .take_while(|c| c.is_ascii_whitespace())
        .count();
    #[expect(clippy::cast_possible_truncation, reason = "C's int conversion")]
    Some((a as i32, b as i32, i))
}

/// mapfile.c's `check_std_t1font`: one of the 14 standard fonts.
fn is_std_t1font(s: &[u8]) -> bool {
    [
        "Courier",
        "Courier-Bold",
        "Courier-Oblique",
        "Courier-BoldOblique",
        "Helvetica",
        "Helvetica-Bold",
        "Helvetica-Oblique",
        "Helvetica-BoldOblique",
        "Symbol",
        "Times-Roman",
        "Times-Bold",
        "Times-Italic",
        "Times-BoldItalic",
        "ZapfDingbats",
    ]
    .iter()
    .any(|n| n.as_bytes() == s)
}

fn ends_with_ci(s: &[u8], suffix: &[u8]) -> bool {
    s.len() >= suffix.len() && s[s.len() - suffix.len()..].eq_ignore_ascii_case(suffix)
}

/// C's `%g` of `v` (six significant digits).
pub(crate) fn percent_g(v: f64) -> Vec<u8> {
    if v == 0.0 {
        return b"0".to_vec();
    }
    let e = format!("{v:.5e}");
    let (mantissa, exp) = e.split_once('e').unwrap_or((&e, "0"));
    let x: i32 = exp.parse().unwrap_or(0);
    let strip = |s: &str| -> alloc::string::String {
        if s.contains('.') {
            s.trim_end_matches('0').trim_end_matches('.').into()
        } else {
            s.into()
        }
    };
    if (-4..6).contains(&x) {
        let decimals = usize::try_from(5 - x).unwrap_or(0);
        strip(&format!("{v:.decimals$}")).into_bytes()
    } else {
        let sign = if x < 0 { '-' } else { '+' };
        format!("{}e{sign}{:02}", strip(mantissa), x.abs()).into_bytes()
    }
}

/// `(integer)(d > 0 ? d + 0.5 : d - 0.5)` of `d * 1000`, with `d` a float.
#[expect(clippy::cast_possible_truncation, reason = "C's conversion")]
fn thousandths(d: f32) -> i32 {
    let d = f64::from((f64::from(d) * 1000.0) as f32);
    (if d > 0.0 { d + 0.5 } else { d - 0.5 }) as i32
}

/// mapfile.c's `fm_scan_line` up to (not including) registering: the
/// entry of a map line, if it is one and valid, and the warnings.
pub(crate) fn scan_line(line: &[u8], warn: &mut Vec<Vec<u8>>) -> Option<MapEntry> {
    let mut r = line;
    if r.first().is_none_or(|&c| is_cfg_comment(c)) {
        return None;
    }
    let mut fm = MapEntry::new();
    let mut u = 0u8;
    let mut v = 0u8;
    'fields: {
        fm.tfm_name = read_field(&mut r).to_vec();
        if r.is_empty() {
            break 'fields;
        }
        if !r.first().is_some_and(u8::is_ascii_digit) {
            let f = read_field(&mut r);
            if !f.is_empty() {
                fm.ps_name = Some(f.to_vec());
            }
            if r.is_empty() {
                break 'fields;
            }
        }
        if r.first().is_some_and(u8::is_ascii_digit) {
            let n = r.iter().take_while(|c| c.is_ascii_digit()).count();
            if matches!(r.get(n), None | Some(b' ' | b'"' | b'<')) {
                // `atoi`
                let digits = &r[..n];
                let value = digits.iter().fold(0i32, |a, &c| {
                    a.wrapping_mul(10).wrapping_add(i32::from(c - b'0'))
                });
                fm.fd_flags = value;
                r = &r[n..];
            }
        }
        loop {
            skip(&mut r, b' ');
            match r.first() {
                None => break 'fields,
                Some(b'"') => {
                    r = &r[1..];
                    u = 0;
                    v = 0;
                    loop {
                        skip(&mut r, b' ');
                        if let Some((d, j)) = scan_float(r) {
                            let mut s = j;
                            if matches!(r[s - 1], b'E' | b'e') {
                                s -= 1;
                            }
                            let rest = &r[s..];
                            if rest.starts_with(b"SlantFont") {
                                fm.slant = thousandths(d);
                                r = &rest[9..];
                            } else if rest.starts_with(b"ExtendFont") {
                                fm.extend = thousandths(d);
                                if fm.extend == 1000 {
                                    fm.extend = 0;
                                }
                                r = &rest[10..];
                            } else {
                                let n = rest
                                    .iter()
                                    .position(|&c| c == b' ' || c == b'"')
                                    .unwrap_or(rest.len());
                                warn.push(
                                    [
                                        b"invalid entry for `".as_slice(),
                                        &fm.tfm_name,
                                        b"': unknown name `",
                                        &rest[..n],
                                        b"' ignored",
                                    ]
                                    .concat(),
                                );
                                r = &rest[n..];
                            }
                        } else {
                            let n = r
                                .iter()
                                .position(|&c| c == b' ' || c == b'"')
                                .unwrap_or(r.len());
                            r = &r[n..];
                        }
                        if r.first() != Some(&b' ') {
                            break;
                        }
                    }
                    if r.first() == Some(&b'"') {
                        r = &r[1..];
                    } else {
                        warn.push(
                            [
                                b"invalid entry for `".as_slice(),
                                &fm.tfm_name,
                                b"': closing quote missing",
                            ]
                            .concat(),
                        );
                        return None;
                    }
                }
                Some(&c) => {
                    if c == b'P'
                        && let Some((a, b, n)) = scan_pid_eid(r)
                    {
                        fm.pid = a;
                        fm.eid = b;
                        r = &r[n..];
                        continue;
                    }
                    let (mut a, mut b) = (0u8, 0u8);
                    if c == b'<' {
                        a = b'<';
                        r = &r[1..];
                        if let Some(&d @ (b'<' | b'[')) = r.first() {
                            b = d;
                            r = &r[1..];
                        }
                    }
                    let buf = read_field(&mut r);
                    if buf.len() > 4 && ends_with_ci(buf, b".enc") {
                        fm.encname = Some(buf.to_vec());
                        u = 0;
                        v = 0;
                    } else if !buf.is_empty() {
                        if a == b'<' || u == b'<' {
                            fm.kind |= F_INCLUDED;
                            if (a == b'<' && b == 0) || (a == 0 && v == 0) {
                                fm.kind |= F_SUBSETTED;
                            }
                        }
                        fm.ff_name = Some(buf.to_vec());
                        if r.is_empty() {
                            break 'fields;
                        }
                        u = 0;
                        v = 0;
                    } else {
                        u = a;
                        v = b;
                    }
                }
            }
        }
    }
    // done:
    if fm.ps_name.as_deref().is_some_and(is_std_t1font) {
        fm.kind |= F_STDT1FONT;
    }
    match &fm.ff_name {
        Some(f) if f.len() > 3 => {
            if ends_with_ci(f, b".ttf") || ends_with_ci(f, b".ttc") {
                fm.kind |= F_TRUETYPE;
            } else if ends_with_ci(f, b".otf") {
                fm.kind |= F_OTF;
            } else {
                fm.kind |= F_TYPE1;
            }
        }
        _ if fm.ps_name.is_none() => fm.kind |= F_PK,
        _ => fm.kind |= F_TYPE1,
    }
    if check_entry(&mut fm, warn) != 0 {
        return None;
    }
    Some(fm)
}

/// mapfile.c's `check_fm_entry` (with warnings): zero if the entry is
/// fine.
fn check_entry(fm: &mut MapEntry, warn: &mut Vec<Vec<u8>>) -> i32 {
    let mut a = 0;
    let t = fm.tfm_name.clone();
    let invalid = |w: &mut Vec<Vec<u8>>, m: &[u8]| {
        w.push([b"invalid entry for `".as_slice(), &t, b"': ", m].concat());
    };
    if fm.ff_name.is_some() && !fm.is(F_INCLUDED) {
        warn.push(
            [
                b"ambiguous entry for `".as_slice(),
                &t,
                b"': font file present but not included, will be treated as font file not present",
            ]
            .concat(),
        );
        fm.ff_name = None;
    }
    if t.is_empty() {
        warn.push(b"invalid map entry: tfm missing".to_vec());
        a += 1;
    }
    if fm.is(F_TRUETYPE) && fm.is_reencoded() && !fm.is(F_SUBSETTED) {
        invalid(warn, b"only subsetted TrueType fonts can be reencoded");
        a += 2;
    }
    if (fm.slant != 0 || fm.extend != 0)
        && (t.is_empty() || !(fm.is_t1fontfile() && fm.is(F_INCLUDED)))
    {
        invalid(
            warn,
            b"SlantFont/ExtendFont can be used only with embedded Type1 fonts",
        );
        a += 4;
    }
    if fm.slant.unsigned_abs() > 1000 {
        let v = percent_g(f64::from(fm.slant) / 1000.0);
        invalid(
            warn,
            &[b"SlantFont value too big: ".as_slice(), &v].concat(),
        );
        a += 8;
    }
    if fm.extend.unsigned_abs() > 2000 {
        let v = percent_g(f64::from(fm.extend) / 1000.0);
        invalid(
            warn,
            &[b"ExtendFont value too big: ".as_slice(), &v].concat(),
        );
        a += 16;
    }
    if fm.pid != -1 && !(fm.is(F_TRUETYPE) && fm.is(F_SUBSETTED) && !fm.is_reencoded()) {
        invalid(
            warn,
            b"PidEid can be used only with subsetted non-reencoded TrueType fonts",
        );
        a += 32;
    }
    if fm.is(F_PK) {
        if let Some(f) = &fm.ff_name {
            let m = [
                b"FontFile cannot be specified for bitmap PK font: ".as_slice(),
                f,
            ]
            .concat();
            invalid(warn, &m);
            a += 64;
        }
        if let Some(p) = &fm.ps_name {
            let m = [
                b"PsName cannot be specified for bitmap PK font: ".as_slice(),
                p,
            ]
            .concat();
            invalid(warn, &m);
            a += 128;
        }
    }
    a
}

/// A `\pdfmapfile` or `\pdfmapline` argument split as `process_map_item`
/// does: the mode, whether it flushes the default map file, and the item
/// (empty if there is none).
pub(crate) fn map_item(s: &[u8], file: bool) -> (Mode, bool, &[u8]) {
    let mut s = s;
    skip(&mut s, b' ');
    let (mode, flush) = match s.first() {
        Some(b'+') => (Mode::DupIgnore, false),
        Some(b'=') => (Mode::Replace, false),
        Some(b'-') => (Mode::Delete, false),
        _ => (Mode::DupIgnore, true),
    };
    if !flush {
        s = &s[1..];
    }
    skip(&mut s, b' ');
    if file {
        let n = s.iter().position(|&c| c == b' ').unwrap_or(s.len());
        s = &s[..n];
    }
    // a C string ends at a null byte
    let n = s.iter().position(|&c| c == 0).unwrap_or(s.len());
    (mode, flush, &s[..n])
}

impl<H: crate::host::Host, T: crate::track::Tracker> crate::tex::Tex<H, T> {
    /// mapfile.c's `fm_read_info` for a map file.
    ///
    /// A file read into an empty table (the default `pdftex.map`, 5 MB)
    /// is kept lazily ([`Lazy`]) once its warnings are known: they come
    /// from the host's cache, keyed by the file, after one full reading.
    #[allow(
        clippy::arc_with_non_send_sync,
        reason = "an engine lives on one thread; the table is an Arc for sharing"
    )]
    fn read_map_file(&mut self, name: &[u8], mode: Mode) {
        use partex_engine::persist::Persist;
        self.fontmap.read = true;
        let found = self.host.read_file(name, crate::host::FileKind::FontMap);
        if T::VALUES {
            // (a map file is a load: its contents are read)
            self.tracker.load(
                name,
                crate::host::FileKind::FontMap,
                found.as_ref().map(|f| &f.contents),
            );
        }
        let Some(f) = found else {
            self.pdftex_warn_in(Some(name), b"cannot open font map file");
            return;
        };
        // (the contents' hash, made once for each contents the host hands
        // out: `MapCache`)
        let hash = self.map_cache.hash(&f.contents);
        self.fontmap.changed(&(0u8, hash, mode));
        self.print_str(b"{");
        self.print_str(&f.name);
        let suppress = self.int_par(crate::web::PDF_SUPPRESS_WARNING_DUP_MAP_CODE) > 0;
        let key = (self.fontmap.table.is_empty() && mode == Mode::DupIgnore).then(|| {
            partex_engine::stablehash::StableHasher::of(&(b"fontmap-warnings/2", hash, suppress))
        });
        let kept = key.and_then(|_| self.map_cache.warnings(&f.contents, suppress));
        let cached = kept.or_else(|| {
            let w = key.and_then(|k| self.host.cache_get(k)).and_then(|b| {
                let mut l = partex_engine::persist::Loader::new(&b);
                Vec::<Vec<u8>>::load(&mut l).filter(|_| l.at_end())
            })?;
            self.map_cache
                .keep_warnings(&f.contents, suppress, w.clone());
            Some(w)
        });
        if let Some(warnings) = cached {
            for w in &warnings {
                self.pdftex_warn_in(Some(&f.name), w);
            }
            self.fontmap.table = Arc::new(Table {
                lazy: Some(Lazy::new(f.contents.clone())),
                ..Table::default()
            });
            self.fontmap.lookups = Lookups {
                index: Some(self.map_cache.index(&f.contents)),
                found: BTreeMap::new(),
                ps_found: BTreeMap::new(),
            };
            self.print_str(b"}");
            return;
        }
        let mut warn = Vec::new();
        let mut warnings = Vec::new();
        for_map_file_lines(&f.contents, |line| {
            // (the file's contents and mode, hashed above, determine the
            // table: no digest per entry)
            self.map_line(line, mode, &mut warn, false);
            for w in warn.drain(..) {
                self.pdftex_warn_in(Some(&f.name), &w);
                if key.is_some() {
                    warnings.push(w);
                }
            }
        });
        if let Some(k) = key {
            let mut s = partex_engine::persist::Saver::new();
            warnings.save(&mut s);
            self.host.cache_put(k, &s.into_bytes());
            self.map_cache
                .keep_warnings(&f.contents, suppress, warnings);
        }
        self.print_str(b"}");
    }

    /// `fm_scan_line` of one line, registering its entry.
    fn map_line(&mut self, line: &[u8], mode: Mode, warn: &mut Vec<Vec<u8>>, digest: bool) {
        if let Some(fm) = scan_line(line, warn) {
            if digest {
                self.fontmap.changed(&(1u8, &fm, mode));
            }
            let suppress = self.int_par(crate::web::PDF_SUPPRESS_WARNING_DUP_MAP_CODE) > 0;
            let used = &self.fonts_mapped;
            Arc::make_mut(&mut self.fontmap.table).register(
                fm,
                mode,
                |e| used.contains(&e.tfm_name),
                suppress,
                warn,
            );
        }
    }

    /// mapfile.c's `lookup_fontmap`: the map entry of an included PDF's
    /// font by its PostScript name (a subset tag dropped, `-Slant_<n>`
    /// and `-Extend_<n>` read off its end), if its Type 1 file is there
    /// (`fm_valid_for_font_replacement`). The default map is read first
    /// if no map was.
    pub(crate) fn lookup_fontmap(&mut self, ps_name: &[u8]) -> Option<Arc<MapEntry>> {
        if !self.fontmap.read {
            self.read_default_map();
        }
        let mut s = ps_name;
        if ps_name.len() > 7 && ps_name[..6].iter().all(u8::is_ascii_uppercase) && ps_name[6] == b'+' {
            s = &ps_name[7..];
        }
        let (name, slant, extend) = slant_extend(s);
        let fm = self.fontmap.lookup_ps(&(name.to_vec(), slant, extend))?;
        let ff = fm.ff_name.clone()?;
        let found = self.host.read_file(&ff, crate::host::FileKind::Type1);
        if T::VALUES {
            self.tracker
                .load(&ff, crate::host::FileKind::Type1, found.as_ref().map(|f| &f.contents));
        }
        found.map(|_| fm)
    }

    /// `fm_read_info` of the default map file, if it is still pending.
    pub(crate) fn read_default_map(&mut self) {
        self.fontmap.read = true;
        if let Some(name) = self.fontmap.pending.take() {
            self.read_map_file(&name, Mode::DupIgnore);
        }
    }

    /// mapfile.c's `process_map_item` (`\pdfmapfile`, `\pdfmapline`):
    /// a write of the font map, which reads the entries used.
    pub(crate) fn process_map_item(&mut self, s: &[u8], file: bool) {
        use crate::pdf::val::{bit, field};
        self.writer_scope(bit(field::FONTS_MAPPED), bit(field::FONTMAP), |t| {
            t.process_map_item_now(s, file);
        });
    }

    fn process_map_item_now(&mut self, s: &[u8], file: bool) {
        let (mode, flush, item) = map_item(s, file);
        if flush {
            self.fontmap.pending = None;
        }
        self.read_default_map();
        if item.is_empty() {
            return;
        }
        if file {
            self.read_map_file(item, mode);
        } else {
            let mut warn = Vec::new();
            self.map_line(item, mode, &mut warn, true);
            for w in warn {
                self.pdftex_warn(&w);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(l: &str) -> (Option<MapEntry>, Vec<Vec<u8>>) {
        let mut w = Vec::new();
        (scan_line(l.as_bytes(), &mut w), w)
    }

    #[test]
    fn typical_lines() {
        let (e, w) = scan("cmr10 CMR10 <cmr10.pfb");
        let e = e.expect("entry");
        assert_eq!(w, Vec::<Vec<u8>>::new());
        assert_eq!(e.ps_name.as_deref(), Some(b"CMR10".as_slice()));
        assert_eq!(e.ff_name.as_deref(), Some(b"cmr10.pfb".as_slice()));
        assert_eq!(e.kind, F_INCLUDED | F_SUBSETTED | F_TYPE1);
        let (e, _) = scan(
            r#"ptmro8r Times-Roman " .167 SlantFont TeXBase1Encoding ReEncodeFont " <8r.enc <utmr8a.pfb"#,
        );
        let e = e.expect("entry");
        assert_eq!(e.slant, 167);
        assert_eq!(e.encname.as_deref(), Some(b"8r.enc".as_slice()));
        let (e, _) = scan("psyr Symbol");
        assert_eq!(e.expect("entry").kind, F_STDT1FONT | F_TYPE1);
        let (e, w) = scan("foo Foo 4 cmr10.pfb");
        assert!(e.is_some());
        assert_eq!(w.len(), 1, "ambiguous entry");
        let (e, w) = scan(r#"foo Foo " 3 SlantFont " <foo.pfb"#);
        assert!(e.is_none());
        assert_eq!(
            w.last().map(Vec::as_slice),
            Some(b"invalid entry for `foo': SlantFont value too big: 3".as_slice())
        );
    }

    #[test]
    fn lines_of_a_file() {
        assert_eq!(
            map_file_lines(b"a\t b\r\n  c  d\n"),
            [b"a b".to_vec(), Vec::new(), b"c d".to_vec(), Vec::new()]
        );
    }

    #[test]
    fn percent_g_like_c() {
        assert_eq!(percent_g(1.5), b"1.5");
        assert_eq!(percent_g(-2.001), b"-2.001");
        assert_eq!(percent_g(2_147_483.647), b"2.14748e+06");
    }
}
