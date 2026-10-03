//! What a build tells the modern command line's renderer (DESIGN.md 2.5):
//! its passes and phases, and the structured diagnostics a session keeps
//! (`session.rs`), saved with a persisted session.

use partex_core::diag::{BoxWarning, Diagnostic, Frame, FrameKind, Severity};
use partex_core::persist::{Loader, Persist, Saver};

/// What a build is doing, beyond running the engine (the live line's
/// phase; the engine's own progress is `partex_core::progress`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Running the document from its start (a cold build: the last
    /// build's totals estimate how far it is).
    Cold,
    /// Loading the build saved by the last run.
    Loading,
    /// Linking the outputs (the PDF) from what the build made.
    Linking,
    /// Writing the output files.
    Writing,
    /// Saving the build for the next run.
    Saving,
}

/// What a converging build has done (`serve_request` in `main.rs`).
pub enum Progress<'a> {
    /// Pass `n` (from 1) begins.
    PassStart(usize),
    /// Pass `n` ended: what it did, or `None` if nothing had changed.
    Pass(usize, Option<&'a crate::session::Report>),
    /// A native tool ran between passes (its report line).
    Tool(&'a str),
    /// The build is now doing this.
    Phase(Phase),
}

/// The code of the note a session logs for each page shipped out (its
/// message is `\count0`), so a build's page count survives resumed passes
/// and spliced rebuilds like the rest of its diagnostics.
pub const PAGE: &str = "page";

fn severity_code(s: Severity) -> u8 {
    match s {
        Severity::Error => 0,
        Severity::Fatal => 1,
        Severity::Warning => 2,
        Severity::Note => 3,
    }
}

fn severity_from(c: u8) -> Option<Severity> {
    Some(match c {
        0 => Severity::Error,
        1 => Severity::Fatal,
        2 => Severity::Warning,
        3 => Severity::Note,
        _ => return None,
    })
}

/// A frame as a tuple: kind, name (or label), number, before, after.
type FrameTuple = (u8, Vec<u8>, i32, Vec<u8>, Vec<u8>);

fn frame_tuple(f: &Frame) -> FrameTuple {
    let (kind, name, n) = match &f.kind {
        FrameKind::File { name, line } => (0, name.clone(), *line),
        FrameKind::Terminal { inserted } => (1, Vec::new(), i32::from(*inserted)),
        FrameKind::Read(n) => (2, Vec::new(), *n),
        FrameKind::Macro { name } => (3, name.clone(), 0),
        FrameKind::TokenList(what) => (4, what.as_bytes().to_vec(), 0),
    };
    (kind, name, n, f.before.clone(), f.after.clone())
}

fn frame_from((kind, name, n, before, after): FrameTuple) -> Option<Frame> {
    let kind = match kind {
        0 => FrameKind::File { name, line: n },
        1 => FrameKind::Terminal { inserted: n != 0 },
        2 => FrameKind::Read(n),
        3 => FrameKind::Macro { name },
        4 => FrameKind::TokenList(token_list_kind(&name)?),
        _ => return None,
    };
    Some(Frame {
        kind,
        before,
        after,
    })
}

/// A token list's kind back as the engine's static name.
fn token_list_kind(s: &[u8]) -> Option<&'static str> {
    const KINDS: &[&str] = &[
        "argument",
        "template",
        "recently read",
        "to be read again",
        "inserted text",
        "output",
        "everypar",
        "everymath",
        "everydisplay",
        "everyhbox",
        "everyvbox",
        "everyjob",
        "everycr",
        "mark",
        "write",
        "?",
    ];
    KINDS.iter().copied().find(|k| k.as_bytes() == s)
}

/// Save `d` for a persisted session.
pub fn save_diagnostic(d: &Diagnostic, s: &mut Saver) {
    severity_code(d.severity).save(s);
    d.code.save(s);
    d.message.save(s);
    d.help.save(s);
    d.frames.iter().map(frame_tuple).collect::<Vec<_>>().save(s);
    d.suggestions.save(s);
    d.boxed
        .as_ref()
        .map(|b| (b.lines.0, b.lines.1, b.amount, b.excerpt.clone()))
        .save(s);
}

/// A diagnostic [`save_diagnostic`] saved.
pub fn load_diagnostic(l: &mut Loader) -> Option<Diagnostic> {
    let severity = severity_from(Persist::load(l)?)?;
    let code: String = Persist::load(l)?;
    let message = Persist::load(l)?;
    let help = Persist::load(l)?;
    let frames: Vec<FrameTuple> = Persist::load(l)?;
    let frames = frames.into_iter().map(frame_from).collect::<Option<_>>()?;
    let suggestions = Persist::load(l)?;
    let boxed: Option<(i32, i32, i32, Vec<u8>)> = Persist::load(l)?;
    Some(Diagnostic {
        severity,
        code: static_code(&code),
        message,
        help,
        frames,
        suggestions,
        boxed: boxed.map(|(a, b, amount, excerpt)| BoxWarning {
            lines: (a, b),
            amount,
            excerpt,
        }),
    })
}

/// A code as a `&'static str`: the engine's codes are few, so each
/// distinct one is leaked once per process.
fn static_code(code: &str) -> &'static str {
    use std::sync::Mutex;
    static CODES: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());
    let mut codes = CODES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(&c) = codes.iter().find(|&&c| c == code) {
        return c;
    }
    let c: &'static str = Box::leak(code.to_owned().into_boxed_str());
    codes.push(c);
    c
}
