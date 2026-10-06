//! The terminal the modern command line draws on (DESIGN.md 2.5): what it
//! can show (colours, Unicode, hyperlinks), its size, the keys `partex
//! watch` reads, and the modes it changes, always restored.
//!
//! partex has no dependency for this: the terminal's size and modes are
//! `stty`'s, run on the terminal's descriptor (once per mode change, and
//! for the size at most once a second while something moves). A watch
//! that reads keys turns echo, line editing and the signal keys off (so
//! Ctrl-C is a key: the watch stops after saving its build) and hides the
//! cursor. A small `sh` started with it waits on a pipe from partex: if
//! partex dies without restoring the terminal (killed, crashed), the pipe
//! closes and the `sh` puts the modes and the cursor back.

use std::io::{IsTerminal, Write};
use std::os::fd::AsFd;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// When to colour (`--color`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorChoice {
    Auto,
    Always,
    Never,
}

/// What the terminal (standard error) can show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)]
pub struct Caps {
    /// A terminal that redraws (not a pipe, a file or `TERM=dumb`).
    pub tty: bool,
    pub color: bool,
    /// Symbols beyond ASCII (`✓`, `·`, the spinner's braille).
    pub unicode: bool,
    /// OSC 8 hyperlinks on file names.
    pub links: bool,
}

/// The value of environment variable `name`, if set and not empty.
fn var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

impl Caps {
    /// The capabilities of standard error, as `color` and the environment
    /// say: `--color` first, then `NO_COLOR`, `CLICOLOR_FORCE`,
    /// `CLICOLOR=0`, and whether it is a terminal.
    #[must_use]
    pub fn detect(color: ColorChoice) -> Self {
        let term = var("TERM").unwrap_or_default();
        let dumb = term == "dumb";
        let tty = std::io::stderr().is_terminal() && !dumb;
        let color = match color {
            ColorChoice::Always => true,
            ColorChoice::Never => false,
            ColorChoice::Auto if var("NO_COLOR").is_some() => false,
            ColorChoice::Auto if var("CLICOLOR_FORCE").is_some_and(|v| v != "0") => true,
            ColorChoice::Auto if var("CLICOLOR").is_some_and(|v| v == "0") => false,
            ColorChoice::Auto => tty,
        };
        // (the locale's character set, else UTF-8 on a terminal)
        let locale = var("LC_ALL")
            .or_else(|| var("LC_CTYPE"))
            .or_else(|| var("LANG"));
        let utf8 = locale.map_or(tty, |l| {
            let l = l.to_ascii_lowercase();
            l.contains("utf-8") || l.contains("utf8")
        });
        let unicode = utf8 && !dumb && term != "linux";
        // (GNU screen without tmux and the Linux console show OSC 8 as
        // text; other terminals link it or skip it)
        let links = match var("PARTEX_HYPERLINKS").as_deref() {
            Some("0") => false,
            Some(_) => true,
            None => {
                tty && term != "linux" && !(term.starts_with("screen") && var("TMUX").is_none())
            }
        };
        Self {
            tty,
            color,
            unicode,
            links: links && tty,
        }
    }
}

/// `stty args` on the terminal behind `fd`: its output, if it succeeded.
fn stty(fd: impl AsFd, args: &[&str]) -> Option<String> {
    let tty = fd.as_fd().try_clone_to_owned().ok()?;
    let out = Command::new("stty")
        .args(args)
        .stdin(Stdio::from(tty))
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// The size of standard error's terminal: rows and columns.
fn query_size() -> Option<(usize, usize)> {
    let s = stty(std::io::stderr(), &["size"])?;
    let (r, c) = s.split_once(' ')?;
    let (r, c) = (r.trim().parse().ok()?, c.trim().parse().ok()?);
    (r > 0 && c > 0).then_some((r, c))
}

/// The terminal's size, asked again at most once a second.
static SIZE: Mutex<Option<(Instant, (usize, usize))>> = Mutex::new(None);

/// The terminal's rows and columns (`COLUMNS`, else 24 by 80, if
/// `stty` cannot tell).
#[must_use]
pub fn size() -> (usize, usize) {
    let mut s = SIZE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some((at, size)) = *s
        && at.elapsed() < Duration::from_secs(1)
    {
        return size;
    }
    let size = query_size().unwrap_or_else(|| {
        let cols = var("COLUMNS").and_then(|c| c.parse().ok()).unwrap_or(80);
        let rows = var("LINES").and_then(|r| r.parse().ok()).unwrap_or(24);
        (rows, cols)
    });
    *s = Some((Instant::now(), size));
    size
}

/// Forget the size, so the next [`size`] asks (the terminal may have
/// been resized while nothing was drawn).
pub fn resized() {
    if let Ok(mut s) = SIZE.lock() {
        *s = None;
    }
}

/// Whether this process may read keys from standard input: it is a
/// terminal, standard error is one, and this process is in the
/// terminal's foreground (a background job that set its modes or read
/// it would be stopped).
#[must_use]
pub fn keys_possible() -> bool {
    std::io::stdin().is_terminal() && std::io::stderr().is_terminal() && foreground()
}

/// Whether this process is in its terminal's foreground process group
/// (or has no controlling terminal at all).
fn foreground() -> bool {
    let Ok(stat) = std::fs::read_to_string("/proc/self/stat") else {
        return true;
    };
    // (after the command's name, in parentheses: state, ppid, pgrp,
    // session, tty_nr, tpgid)
    let Some((_, rest)) = stat.rsplit_once(')') else {
        return true;
    };
    let f: Vec<&str> = rest.split_whitespace().collect();
    match (f.get(2), f.get(5)) {
        (Some(pgrp), Some(tpgid)) => tpgid == pgrp || *tpgid == "-1",
        _ => true,
    }
}

/// The modes partex changed, and the guard that puts them back.
struct Changed {
    /// `stty -g` before partex changed the modes.
    saved: String,
    /// The `sh` that restores the terminal if partex dies first.
    watchdog: Option<Child>,
    /// The cursor is hidden.
    hidden: bool,
}

static CHANGED: Mutex<Option<Changed>> = Mutex::new(None);

/// The watchdog: it waits for a line; end of file (partex is gone)
/// restores the cursor and the modes, `$1`.
const WATCHDOG: &str = r#"trap '' INT QUIT TERM TTOU
IFS= read -r line
[ -n "$line" ] && exit 0
printf '\033[?25h\033[?2026l\n' >&2
[ -n "$1" ] && stty "$1" <&2 2>/dev/null
exit 0"#;

/// The modes of a watch reading keys: no echo, no line editing, no
/// signal keys (Ctrl-C, Ctrl-Z and Ctrl-\ are read as keys).
const KEY_MODES: &[&str] = &["-icanon", "-echo", "-isig", "min", "1", "time", "0"];

/// Read keys from standard input: its modes changed, the cursor hidden,
/// the watchdog started, and the panic hook set to restore them. Whether
/// it could.
pub fn keys_on() -> bool {
    if !keys_possible() {
        return false;
    }
    let Some(saved) = stty(std::io::stdin(), &["-g"]) else {
        return false;
    };
    let watchdog = Command::new("sh")
        .args(["-c", WATCHDOG, "partex-terminal", &saved])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .ok();
    if stty(std::io::stdin(), KEY_MODES).is_none() {
        if let Some(mut w) = watchdog {
            let _ = w.kill();
            let _ = w.wait();
        }
        return false;
    }
    hide_cursor();
    if let Ok(mut c) = CHANGED.lock() {
        *c = Some(Changed {
            saved,
            watchdog,
            hidden: true,
        });
    }
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore();
        previous(info);
    }));
    true
}

fn hide_cursor() {
    let mut e = std::io::stderr().lock();
    let _ = e.write_all(b"\x1b[?25l");
    let _ = e.flush();
}

fn show_cursor() {
    let mut e = std::io::stderr().lock();
    let _ = e.write_all(b"\x1b[?25h");
    let _ = e.flush();
}

/// Put the terminal back as partex found it (the modes, the cursor) and
/// stop the watchdog. Idempotent.
pub fn restore() {
    let Some(mut c) = CHANGED.lock().ok().and_then(|mut c| c.take()) else {
        return;
    };
    let _ = stty(std::io::stdin(), &[&c.saved]);
    if c.hidden {
        show_cursor();
    }
    if let Some(mut w) = c.watchdog.take() {
        if let Some(mut pipe) = w.stdin.take() {
            let _ = pipe.write_all(b"done\n");
        }
        let _ = w.wait();
    }
}

/// Restore the terminal, then exit with `code`.
pub fn exit(code: i32) -> ! {
    restore();
    std::process::exit(code)
}

/// Ctrl-Z while keys are read: the terminal restored, the job stopped as
/// the shell's Ctrl-Z would (every process of its group), and the keys'
/// modes set again once it goes on. Without job control (no controlling
/// terminal), nothing.
pub fn suspend() {
    let pgrp = std::fs::read_to_string("/proc/self/stat")
        .ok()
        .and_then(|s| {
            let (_, rest) = s.rsplit_once(')')?;
            let f: Vec<String> = rest.split_whitespace().map(str::to_owned).collect();
            (f.get(5).map(String::as_str) != Some("-1")).then(|| f.get(2).cloned())?
        });
    let Some(pgrp) = pgrp else { return };
    let mut c = CHANGED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(changed) = c.as_mut() else { return };
    let _ = stty(std::io::stdin(), &[&changed.saved]);
    show_cursor();
    // (this waits until the job is continued: `kill` stops with it)
    let _ = Command::new("kill")
        .args(["-TSTP", "--", &format!("-{pgrp}")])
        .stderr(Stdio::null())
        .status();
    let _ = stty(std::io::stdin(), KEY_MODES);
    hide_cursor();
    resized();
}

/// The width of `s` on the terminal: its characters, without escape
/// sequences, wide ones counted twice.
#[must_use]
pub fn width(s: &str) -> usize {
    let mut n = 0;
    let mut rest = s;
    while let Some(c) = rest.chars().next() {
        if c == '\x1b' {
            rest = &rest[escape_len(rest)..];
            continue;
        }
        if !c.is_control() {
            n += char_width(c);
        }
        rest = &rest[c.len_utf8()..];
    }
    n
}

/// The length in bytes of the escape sequence `s` begins with (at its
/// `ESC`): a CSI to its final byte, an OSC to its BEL or `ESC \\`.
fn escape_len(s: &str) -> usize {
    let b = s.as_bytes();
    let after = |i: usize| {
        // (the character at byte `i`, whole)
        i + s[i..].chars().next().map_or(0, char::len_utf8)
    };
    match b.get(1) {
        None => 1,
        Some(b'[') => b[2..]
            .iter()
            .position(|c| (0x40..=0x7e).contains(c))
            .map_or(b.len(), |i| i + 3),
        Some(b']') => {
            for i in 2..b.len() {
                match b[i] {
                    0x07 => return i + 1,
                    0x1b if i + 1 < b.len() => return after(i + 1),
                    0x1b => return b.len(),
                    _ => {}
                }
            }
            b.len()
        }
        Some(_) => after(1),
    }
}

/// `s` without its escape sequences.
#[cfg(test)]
#[must_use]
pub fn strip(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(c) = rest.chars().next() {
        if c == '\x1b' {
            rest = &rest[escape_len(rest)..];
            continue;
        }
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    out
}

/// 2 for the wide characters of East Asian scripts and emoji, else 1.
fn char_width(c: char) -> usize {
    let u = u32::from(c);
    let wide = matches!(u,
        0x1100..=0x115F | 0x2E80..=0x303E | 0x3041..=0x33FF | 0x3400..=0x4DBF
        | 0x4E00..=0x9FFF | 0xA000..=0xA4CF | 0xAC00..=0xD7A3 | 0xF900..=0xFAFF
        | 0xFE30..=0xFE4F | 0xFF00..=0xFF60 | 0xFFE0..=0xFFE6 | 0x1F300..=0x1F64F
        | 0x1F900..=0x1F9FF | 0x20000..=0x3FFFD);
    if wide { 2 } else { 1 }
}

/// `s` cut to `max` columns, an ellipsis where it is cut; its escape
/// sequences kept, and closed (colour, hyperlink) if it is cut.
#[must_use]
pub fn truncate(s: &str, max: usize, unicode: bool) -> String {
    if width(s) <= max {
        return s.to_owned();
    }
    let room = max.saturating_sub(1);
    let mut out = String::with_capacity(s.len());
    let mut n = 0;
    let mut link_open = false;
    let mut rest = s;
    while let Some(c) = rest.chars().next() {
        if c == '\x1b' {
            let len = escape_len(rest);
            let seq = &rest[..len];
            if let Some(osc) = seq.strip_prefix("\x1b]8;") {
                // (`ESC ] 8 ; params ; URI ST`: no URI ends the link)
                let uri = osc.split_once(';').map_or("", |(_, u)| u);
                link_open = !(uri.is_empty() || uri.starts_with(['\x1b', '\x07']));
            }
            out.push_str(seq);
            rest = &rest[len..];
            continue;
        }
        let w = if c.is_control() { 0 } else { char_width(c) };
        if n + w > room {
            break;
        }
        n += w;
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    if link_open {
        out.push_str("\x1b]8;;\x1b\\");
    }
    out.push_str(if unicode { "…" } else { "." });
    if s.contains("\x1b[") {
        out.push_str("\x1b[0m");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widths_skip_escapes() {
        assert_eq!(width("\x1b[1;32mFinished\x1b[0m"), 8);
        assert_eq!(width("\x1b]8;;file:///a.pdf\x1b\\a.pdf\x1b]8;;\x1b\\"), 5);
        assert_eq!(width("⠹ ━━╸"), 5);
        assert_eq!(width("日本"), 4);
        assert_eq!(strip("\x1b[1;34m  -->\x1b[0m a.tex:3"), "  --> a.tex:3");
    }

    #[test]
    fn truncation_keeps_and_closes_escapes() {
        assert_eq!(truncate("abcdef", 4, true), "abc…");
        assert_eq!(truncate("abcdef", 6, true), "abcdef");
        let t = truncate("\x1b[1mabcdef\x1b[0m", 4, false);
        assert_eq!(t, "\x1b[1mabc.\x1b[0m");
        assert_eq!(width(&t), 4);
        let l = truncate("\x1b]8;;file:///x\x1b\\abcdef\x1b]8;;\x1b\\", 3, true);
        assert_eq!(l, "\x1b]8;;file:///x\x1b\\ab\x1b]8;;\x1b\\…");
    }
}
