//! `\write18`'s policy: web2c's `runsystem` and `shell_cmd_is_allowed`
//! (texmfmp.c). Which command runs, and how it is quoted, is decided here,
//! the same for every host; the host only runs the command it is given
//! ([`crate::Host::system`]).

use alloc::vec::Vec;

/// What `runsystem` decided for a command (texmfmp.c's return values).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    /// -1: a quotation error (a `'`, or a `"` not closed or not followed
    /// by a space); nothing runs.
    QuotationError,
    /// 0: shell escape is restricted and the command's first word is not
    /// in `shell_escape_commands` (or its quoted form has a `|`).
    Restricted,
    /// 1: unrestricted: the command runs as written.
    Any(Vec<u8>),
    /// 2: restricted and allowed: the command runs as quoted here.
    Allowed(Vec<u8>),
}

impl Decision {
    /// The command to run, if one runs.
    #[must_use]
    pub fn command(&self) -> Option<&[u8]> {
        match self {
            Self::Any(c) | Self::Allowed(c) => Some(c),
            Self::QuotationError | Self::Restricted => None,
        }
    }
}

/// C's `isspace` in the C locale.
fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

/// texmfmp.c's `runsystem`, called only with shell escape enabled:
/// `restricted` is web2c's `restrictedshell`, `allowed` the
/// `shell_escape_commands` list.
#[must_use]
pub fn runsystem(cmd: &[u8], restricted: bool, allowed: &[Vec<u8>]) -> Decision {
    if !restricted {
        return Decision::Any(cmd.to_vec());
    }
    match shell_cmd_is_allowed(cmd, allowed) {
        // (a `|` in the quoted command is refused in restricted mode)
        Ok(Some(safe)) if safe.contains(&b'|') => Decision::Restricted,
        Ok(Some(safe)) => Decision::Allowed(safe),
        Ok(None) => Decision::Restricted,
        Err(()) => Decision::QuotationError,
    }
}

/// texmfmp.c's `shell_cmd_is_allowed` (Unix): the command's first word
/// must be in `allowed`; then every argument is quoted with `'`, a
/// user's `"…"` becoming `'…'`. `Ok(None)`: not allowed; `Err`: a
/// quotation error.
fn shell_cmd_is_allowed(cmd: &[u8], allowed: &[Vec<u8>]) -> Result<Option<Vec<u8>>, ()> {
    const QUOTE: u8 = b'\'';
    let start = cmd.iter().position(|&c| !is_space(c)).unwrap_or(cmd.len());
    let len = cmd[start..]
        .iter()
        .position(|&c| is_space(c))
        .unwrap_or(cmd.len() - start);
    let name = &cmd[start..start + len];
    if !allowed.iter().any(|a| a[..] == *name) {
        return Ok(None);
    }
    let mut d = Vec::with_capacity(cmd.len() + 8);
    d.extend_from_slice(name);
    let mut s = start + len;
    let at = |s: usize| cmd.get(s).copied().unwrap_or(0);
    let mut pre = true;
    while at(s) != 0 {
        let c = at(s);
        if c == b'\'' {
            return Err(());
        }
        if c == b'"' {
            // (closing the argument begun before the quotation mark)
            if !pre {
                d.push(QUOTE);
            }
            pre = false;
            d.push(QUOTE);
            s += 1;
            while at(s) != b'"' {
                if at(s) == b'\'' || at(s) == 0 {
                    return Err(());
                }
                d.push(at(s));
                s += 1;
            }
            s += 1;
            // (the character after the closing mark: a space or the end)
            if !is_space(at(s)) && at(s) != 0 {
                return Err(());
            }
        } else if pre && !is_space(c) {
            pre = false;
            d.push(QUOTE);
            d.push(c);
            s += 1;
        } else if !pre && is_space(c) {
            pre = true;
            d.push(QUOTE);
            d.push(c);
            s += 1;
        } else {
            d.push(c);
            s += 1;
        }
    }
    if !pre {
        d.push(QUOTE);
    }
    Ok(Some(d))
}

/// web2c's `mk_shellcmdlist`: `shell_escape_commands` split at commas
/// (an empty last item dropped, as a trailing comma leaves).
#[must_use]
pub fn command_list(v: &[u8]) -> Vec<Vec<u8>> {
    let mut l: Vec<Vec<u8>> = v.split(|&c| c == b',').map(<[u8]>::to_vec).collect();
    if l.last().is_some_and(Vec::is_empty) {
        l.pop();
    }
    l
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list() -> Vec<Vec<u8>> {
        command_list(b"bibtex,kpsewhich,latexminted,makeindex,")
    }

    #[test]
    fn quoting() {
        let r = |c: &[u8]| runsystem(c, true, &list());
        assert_eq!(
            r(b"latexminted config  --timestamp 2026 4B2C"),
            Decision::Allowed(b"latexminted 'config'  '--timestamp' '2026' '4B2C'".to_vec())
        );
        assert_eq!(
            r(br#"kpsewhich --format="other text files" config"#),
            Decision::Allowed(b"kpsewhich '--format=''other text files' 'config'".to_vec())
        );
        assert_eq!(r(b"  kpsewhich"), Decision::Allowed(b"kpsewhich".to_vec()));
        assert_eq!(
            r(b"kpsewhich x "),
            Decision::Allowed(b"kpsewhich 'x' ".to_vec())
        );
        assert_eq!(r(b"ls -l"), Decision::Restricted);
        assert_eq!(r(b"kpsewhich 'x'"), Decision::QuotationError);
        assert_eq!(r(br#"kpsewhich "x"y"#), Decision::QuotationError);
        assert_eq!(r(br#"kpsewhich "x"#), Decision::QuotationError);
        assert_eq!(r(b"kpsewhich a|b"), Decision::Restricted);
        assert_eq!(
            runsystem(b"ls -l", false, &[]),
            Decision::Any(b"ls -l".to_vec())
        );
    }
}
