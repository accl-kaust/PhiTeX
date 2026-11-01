//! The engine's panics caught: a build whose pieces did not settle (a
//! panic in a worker, then the build's thread's) is an error with the
//! first panic's message, not the process's end.

use std::any::Any;
use std::panic::AssertUnwindSafe;
use std::sync::{Mutex, PoisonError};

/// The panics this process met, first first (the engine's worker thread's
/// before the main thread's that follows it).
static PANICS: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn payload(p: &(dyn Any + Send)) -> String {
    p.downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| p.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "a panic without a message".to_owned())
}

/// Keep the panics for [`catch`] instead of printing them (a stalled
/// scheduler's dump is thousands of lines); `verbose` prints each as it
/// happens, whole.
pub fn capture_panics(verbose: bool) {
    std::panic::set_hook(Box::new(move |info| {
        let msg = payload(info.payload());
        let at = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_default();
        if verbose {
            eprintln!("panic at {at}:\n{msg}");
            // (with RUST_BACKTRACE set, where it came from)
            let bt = std::backtrace::Backtrace::capture();
            if bt.status() == std::backtrace::BacktraceStatus::Captured {
                eprintln!("{bt}");
            }
        }
        PANICS
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(format!("{msg} (at {at})"));
    }));
}

/// `f`, a panic in it (the engine's: a build whose pieces did not settle)
/// an error instead of the process's end: the first panic's message.
pub fn catch<T>(f: impl FnOnce() -> T) -> Result<T, String> {
    PANICS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clear();
    std::panic::catch_unwind(AssertUnwindSafe(f)).map_err(|p| {
        let mut v = PANICS.lock().unwrap_or_else(PoisonError::into_inner);
        let first = v.first().cloned().unwrap_or_else(|| payload(&*p));
        v.clear();
        first
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catches_panics() {
        capture_panics(false);
        assert_eq!(catch(|| 3).unwrap(), 3);
        let e = catch(|| -> u32 { panic!("the sequence of pieces did not settle") }).unwrap_err();
        assert!(
            e.starts_with("the sequence of pieces did not settle"),
            "{e}"
        );
    }
}
