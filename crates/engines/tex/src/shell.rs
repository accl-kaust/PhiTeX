//! `\write18` through the host ([`ShellHook`], docs/note-shell-hook.md):
//! which command runs and how it is quoted is decided here, the same for
//! every host (web2c's `runsystem` and `shell_cmd_is_allowed`,
//! texmfmp.c); the host only runs the command it is given over the files
//! it is given, and says what it wrote, removed and read.
//!
//! A command runs where it is in TeX's order, in the run that makes it
//! ([`Run::shell_run`]): it is handed the files the job wrote and closed
//! before it and the files earlier commands left ([`Run::shell_view`]),
//! and what it made of each name is the run's definition of
//! `toolout:<name>` (`crate::tools::out_field`), which a later read of
//! that name reads, in this run or after it; a name it removed is absent
//! there. The host's calls are memoised by the command and the files each
//! read ([`Memo`]): a change of another file runs nothing.
//!
//! The files commands make are no loop-back: the build keeps them in a
//! store across its evaluations, as `pdflatex`'s directory is kept across
//! its runs (`crate::api`, [`OUTS`], [`STORE`]).

use std::sync::Arc;

use phitex_defuse::{ValueId, Values};

use crate::run::{R, Run};
use crate::val::{list_of, str_of};

/// What a command did ([`ShellHook::run`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShellRan {
    /// The wait status, as C's `system` returns it (`127 << 8`: not
    /// found).
    pub status: i32,
    /// The files it made or changed, by their names relative to the
    /// working directory (`/`-separated), with their bytes.
    pub wrote: Vec<(String, Vec<u8>)>,
    /// The names of the given files it removed.
    pub removed: Vec<String>,
    /// The names of the given files it read or looked at, if the host
    /// can tell; `None`: all of them.
    pub read: Option<Vec<String>>,
}

/// Runs the commands `\write18` allows ([`crate::Options::shell_hook`]).
/// Called from any engine worker, possibly several at once and for a run
/// later thrown away: a call leaves nothing behind but its [`ShellRan`],
/// and gives the same result for the same command and files.
pub trait ShellHook: Send + Sync {
    /// Runs `command` (quoted by the engine) in a working directory
    /// holding exactly `files` (name relative to it, bytes).
    fn run(&self, command: &str, files: &[(String, Arc<[u8]>)]) -> ShellRan;
}

/// TeX Live 2026's `shell_escape_commands` (texmf.cnf): what restricted
/// mode allows ([`crate::Options::shell_commands`]'s default).
pub const TEXLIVE_COMMANDS: &[&str] = &[
    "bibtex",
    "bibtex8",
    "extractbb",
    "gregorio",
    "kpsewhich",
    "l3sys-query",
    "latexminted",
    "makeindex",
    "memoize-extract.pl",
    "memoize-extract.py",
    "repstopdf",
    "r-mpost",
    "texosquery-jre8",
];

/// [`TEXLIVE_COMMANDS`] as owned names.
#[must_use]
pub fn texlive_commands() -> Vec<String> {
    TEXLIVE_COMMANDS.iter().map(|&s| s.to_owned()).collect()
}

/// What `runsystem` decided for a command (texmfmp.c's return values).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Decision {
    /// -1: a quotation error (a `'`, or a `"` not closed or not followed
    /// by a space); nothing runs.
    QuotationError,
    /// 0: restricted, and the command's first word is not allowed (or
    /// its quoted form has a `|`).
    Restricted,
    /// 1: unrestricted: the command runs as written.
    Any(String),
    /// 2: restricted and allowed: the command runs as quoted here.
    Allowed(String),
}

/// C's `isspace` in the C locale.
fn is_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\x0b' | '\x0c' | '\r')
}

/// texmfmp.c's `runsystem`, with shell escape on: `restricted` is
/// web2c's `restrictedshell`, `allowed` its `shell_escape_commands`.
pub(crate) fn runsystem(cmd: &str, restricted: bool, allowed: &[String]) -> Decision {
    if !restricted {
        return Decision::Any(cmd.to_owned());
    }
    match shell_cmd_is_allowed(cmd, allowed) {
        // (a `|` in the quoted command is refused in restricted mode)
        Ok(Some(safe)) if safe.contains('|') => Decision::Restricted,
        Ok(Some(safe)) => Decision::Allowed(safe),
        Ok(None) => Decision::Restricted,
        Err(()) => Decision::QuotationError,
    }
}

/// texmfmp.c's `shell_cmd_is_allowed` (Unix): the command's first word
/// must be in `allowed`; then every argument is quoted with `'`, a
/// user's `"…"` becoming `'…'`. `Ok(None)`: not allowed; `Err`: a
/// quotation error.
fn shell_cmd_is_allowed(cmd: &str, allowed: &[String]) -> Result<Option<String>, ()> {
    const QUOTE: char = '\'';
    let c: Vec<char> = cmd.chars().collect();
    let start = c.iter().position(|&x| !is_space(x)).unwrap_or(c.len());
    let len = c[start..]
        .iter()
        .position(|&x| is_space(x))
        .unwrap_or(c.len() - start);
    let name: String = c[start..start + len].iter().collect();
    if !allowed.contains(&name) {
        return Ok(None);
    }
    let mut d = name;
    let mut s = start + len;
    let at = |s: usize| c.get(s).copied().unwrap_or('\0');
    let mut pre = true;
    while at(s) != '\0' {
        let x = at(s);
        if x == '\'' {
            return Err(());
        }
        if x == '"' {
            // (closing the argument begun before the quotation mark)
            if !pre {
                d.push(QUOTE);
            }
            pre = false;
            d.push(QUOTE);
            s += 1;
            while at(s) != '"' {
                if at(s) == '\'' || at(s) == '\0' {
                    return Err(());
                }
                d.push(at(s));
                s += 1;
            }
            s += 1;
            // (the character after the closing mark: a space or the end)
            if !is_space(at(s)) && at(s) != '\0' {
                return Err(());
            }
        } else if pre && !is_space(x) {
            pre = false;
            d.push(QUOTE);
            d.push(x);
            s += 1;
        } else if !pre && is_space(x) {
            pre = true;
            d.push(QUOTE);
            d.push(x);
            s += 1;
        } else {
            d.push(x);
            s += 1;
        }
    }
    if !pre {
        d.push(QUOTE);
    }
    Ok(Some(d))
}

/// The commands' results in TeX's order, each `[name, made]` (`made` as
/// `toolout:<name>` holds it): what the build folds into its store after
/// an evaluation (`crate::api`).
pub(crate) const OUTS: &str = "shellouts";

/// The names the commands made or removed, in TeX's order: what a later
/// command's view looks up ([`Run::shell_view`]).
pub(crate) const NAMES: &str = "shellnames";

/// The environment's list of the names in the build's store of command
/// files (each also `toolout:<name>` in the environment).
pub(crate) const STORE: &str = "shellstore";

/// TeX's characters as bytes (Latin-1; a character past 255 as `?`).
fn bytes(s: &str) -> Vec<u8> {
    s.chars().map(|c| u8::try_from(c).unwrap_or(b'?')).collect()
}

/// Bytes as TeX's characters (each byte one character).
fn chars(b: &[u8]) -> String {
    b.iter().map(|&c| char::from(c)).collect()
}

/// A name or command in TeX's characters as the host's string (its bytes,
/// UTF-8 where they are).
fn host_str(s: &str) -> String {
    String::from_utf8_lossy(&bytes(s)).into_owned()
}

/// A host's name in TeX's characters, `./` dropped.
fn tex_name(s: &str) -> String {
    chars(s.strip_prefix("./").unwrap_or(s).as_bytes())
}

/// What `toolout:<name>` holds for a file a command wrote: read inline
/// where it is opened (`[path, text, 0, said]`, `crate::tools::ToolRun`).
pub(crate) fn made_value(v: &Values, name: &str, text: ValueId) -> ValueId {
    v.list(vec![
        v.str(&format!("./{name}")),
        text,
        v.int(0),
        v.list(Vec::new()),
    ])
}

/// What `toolout:<name>` holds for a name a command removed: no file.
pub(crate) fn removed_value(v: &Values) -> ValueId {
    v.list(Vec::new())
}

/// A text's key in the [`Memo`].
fn text_key(t: &str) -> u64 {
    phitex_defuse::hash_bytes(0x7368_656c_6c00, t.as_bytes())
}

/// What a call gave, in TeX's characters, sorted by name.
#[derive(Debug)]
pub(crate) struct Made {
    status: i32,
    wrote: Vec<(String, String)>,
    removed: Vec<String>,
}

/// One call remembered: the files it read with their texts' keys (`None`:
/// absent), or all the files it was handed (`all`: the same names, the
/// same texts), and what it made.
#[derive(Debug)]
struct Call {
    all: bool,
    reads: Vec<(String, Option<u64>)>,
    made: Arc<Made>,
}

/// The calls of the hook made in this build, by command: a command whose
/// files it read are as one call found them is not run again, whichever
/// other file changed (the host's `read`). Results only, never state: a
/// command's result is a function of its command and the files it read
/// (docs/note-shell-hook.md, rule 2), so any call that matches gives the
/// same result.
#[derive(Debug, Default)]
pub struct Memo(crate::FxHashMap<String, Vec<Call>>);

/// The calls kept per command (the latest).
const MEMO_PER_COMMAND: usize = 64;

impl Memo {
    fn find(&self, cmd: &str, files: &[(String, String)]) -> Option<Arc<Made>> {
        let key = |n: &str| files.iter().find(|(m, _)| m == n).map(|(_, t)| text_key(t));
        self.0.get(cmd)?.iter().rev().find_map(|c| {
            let same = if c.all {
                c.reads.len() == files.len() && c.reads.iter().all(|(n, k)| key(n) == *k)
            } else {
                c.reads.iter().all(|(n, k)| key(n) == *k)
            };
            same.then(|| c.made.clone())
        })
    }

    fn add(&mut self, cmd: &str, call: Call) {
        let calls = self.0.entry(cmd.to_owned()).or_default();
        if calls.len() >= MEMO_PER_COMMAND {
            calls.remove(0);
        }
        calls.push(call);
    }
}

/// The text of file `name` as the job wrote it, from its stream's entries
/// in TeX's order, if the job opened it and closed it since (a file still
/// open is not handed: pdfTeX's command sees what stdio flushed).
fn closed_text(v: &Values, entries: &[ValueId], name: &str) -> Option<String> {
    let mut state: Option<(String, bool)> = None;
    for e in crate::api::doc_order(v, entries) {
        if crate::api::is_cand_marker(v, e) {
            continue;
        }
        let xs = list_of(v, crate::api::stream_entry(v, e));
        if xs.first().is_none_or(|&n| str_of(v, n) != name) {
            continue;
        }
        if xs
            .get(2)
            .is_some_and(|&c| v.as_str(c).as_deref() == Some("close"))
        {
            if let Some(s) = state.as_mut() {
                s.1 = true;
            }
        } else if let Some(&t) = xs.get(1) {
            if let Some(s) = state.as_mut()
                && !s.1
            {
                s.0.push_str(&str_of(v, t));
            }
        } else {
            state = Some((String::new(), false));
        }
    }
    state.filter(|s| s.1).map(|s| s.0)
}

/// The host's call: its result for `cmd` over `files` (TeX's characters),
/// from the memo where a call before read the same; `None` with no hook.
/// No lock is held while the host runs.
fn call(ops: &crate::run::Ops, cmd: &str, files: &[(String, String)]) -> Option<Arc<Made>> {
    let (hook, memo) = {
        let o = ops
            .tool_opts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        (o.shell_hook.clone()?, o.shell_memo.clone())
    };
    if let Some(m) = memo
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .find(cmd, files)
    {
        return Some(m);
    }
    let handed: Vec<(String, Arc<[u8]>)> = files
        .iter()
        .map(|(n, t)| (host_str(n), Arc::from(bytes(t))))
        .collect();
    let r = hook.run(&host_str(cmd), &handed);
    let mut wrote: Vec<(String, String)> = r
        .wrote
        .iter()
        .map(|(n, b)| (tex_name(n), chars(b)))
        .collect();
    wrote.sort();
    wrote.dedup_by(|a, b| a.0 == b.0);
    let mut removed: Vec<String> = r.removed.iter().map(|n| tex_name(n)).collect();
    removed.sort();
    removed.dedup();
    let key = |n: &str| files.iter().find(|(m, _)| m == n).map(|(_, t)| text_key(t));
    let (all, reads) = match &r.read {
        Some(ns) => (
            false,
            ns.iter()
                .map(|n| tex_name(n))
                .map(|n| {
                    let k = key(&n);
                    (n, k)
                })
                .collect(),
        ),
        None => (
            true,
            files
                .iter()
                .map(|(n, t)| (n.clone(), Some(text_key(t))))
                .collect(),
        ),
    };
    if crate::exec::obs_flag!("PHITEX_AUX_TRACE") {
        eprintln!(
            "shell: {cmd:?} over {} files: status {}, wrote {:?}, removed {removed:?}, read {:?}",
            files.len(),
            r.status,
            wrote.iter().map(|w| &w.0).collect::<Vec<_>>(),
            r.read
        );
    }
    let made = Arc::new(Made {
        status: r.status,
        wrote,
        removed,
    });
    memo.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .add(
            cmd,
            Call {
                all,
                reads,
                made: made.clone(),
            },
        );
    Some(made)
}

impl Run<'_, '_> {
    /// The seq contributed to `f` before here, this run's own after it.
    fn fold_with_own(&mut self, f: phitex_defuse::Field) -> R<Vec<ValueId>> {
        let v = self.v;
        let before = self.cx.fold(f, None, None)?;
        let mut out = v.seq_to_vec(before);
        for &(g, s) in &self.contribs {
            if g == f {
                out.extend(v.seq_to_vec(s));
            }
        }
        Ok(out)
    }

    /// The files a command here is handed (docs/note-shell-hook.md): each
    /// file the job wrote and closed before here, as it wrote it, else
    /// what the commands before here (or the store: the evaluation
    /// before) left of it, by name.
    pub(crate) fn shell_view(&mut self) -> R<Vec<(String, String)>> {
        let v = self.v;
        let mut names = std::collections::BTreeSet::new();
        for f in [crate::run::WNAMES, NAMES] {
            let xs = self.fold_with_own(v.field(f))?;
            names.extend(xs.into_iter().map(|n| str_of(v, n)));
        }
        if let Some(s) = self.cx.env(v.field(STORE))? {
            names.extend(list_of(v, s).into_iter().map(|n| str_of(v, n)));
        }
        let mut out = Vec::new();
        for n in names {
            let entries = self.fold_with_own(crate::run::stream_bucket(v, &n))?;
            if let Some(t) = closed_text(v, &entries, &n) {
                out.push((n, t));
                continue;
            }
            let made = self.get(&crate::tools::out_name(&n))?;
            if let Some(t) = made.and_then(|x| list_of(v, x).get(1).copied()) {
                out.push((n, str_of(v, t)));
            }
        }
        Ok(out)
    }

    /// A `\write18` the host runs (`cmd` quoted, [`runsystem`]), here in
    /// this run, in TeX's order: handed the files as they are here
    /// ([`Run::shell_view`]), what it made is this run's definition of
    /// each name it wrote or removed (`toolout:<name>`, global, as a file
    /// on disk is), which the reads after it read (`Run::tool_output`),
    /// with the names ([`NAMES`]) and the results ([`OUTS`]) contributed.
    /// The host's call is memoised by the command and the files it read
    /// ([`Memo`]).
    pub(crate) fn shell_run(&mut self, cmd: &str) -> R<()> {
        let v = self.v;
        let files = self.shell_view()?;
        let Some(made) = call(self.ops, cmd, &files) else {
            return Ok(());
        };
        if made.status != 0 && crate::exec::obs_flag!("PHITEX_AUX_TRACE") {
            eprintln!("shell: {cmd:?} returned {}", made.status);
        }
        let mut out: Vec<(&str, ValueId)> = Vec::new();
        for (n, t) in &made.wrote {
            out.push((n, made_value(v, n, v.str(t))));
        }
        for n in &made.removed {
            out.push((n, removed_value(v)));
        }
        for (n, x) in out {
            self.set(&crate::tools::out_name(n), x, true);
            let name = v.str(n);
            self.contribute(NAMES, v.seq_unit(name));
            self.contribute(OUTS, v.seq_unit(v.list(vec![name, x])));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list() -> Vec<String> {
        ["bibtex", "kpsewhich", "latexminted", "makeindex"]
            .map(str::to_owned)
            .to_vec()
    }

    #[test]
    fn quoting() {
        let r = |c: &str| runsystem(c, true, &list());
        assert_eq!(
            r("latexminted config  --timestamp 2026 4B2C"),
            Decision::Allowed("latexminted 'config'  '--timestamp' '2026' '4B2C'".to_owned())
        );
        assert_eq!(
            r(r#"kpsewhich --format="other text files" config"#),
            Decision::Allowed("kpsewhich '--format=''other text files' 'config'".to_owned())
        );
        assert_eq!(r("  kpsewhich"), Decision::Allowed("kpsewhich".to_owned()));
        assert_eq!(
            r("kpsewhich x "),
            Decision::Allowed("kpsewhich 'x' ".to_owned())
        );
        assert_eq!(r("ls -l"), Decision::Restricted);
        assert_eq!(r("kpsewhich 'x'"), Decision::QuotationError);
        assert_eq!(r(r#"kpsewhich "x"y"#), Decision::QuotationError);
        assert_eq!(r(r#"kpsewhich "x"#), Decision::QuotationError);
        assert_eq!(r("kpsewhich a|b"), Decision::Restricted);
        assert_eq!(
            runsystem("ls -l", false, &[]),
            Decision::Any("ls -l".to_owned())
        );
    }
}
