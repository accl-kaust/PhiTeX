//! The `observe` build's watcher: a thread that lives for one evaluation
//! and writes the scheduler dump every `DEFUSE_DUMP_EVERY` seconds and on
//! SIGINT or SIGTERM (then exits with 128 + the signal), so a hung run
//! still leaves its state behind.
//!
//! Dumps go to the file named by `DEFUSE_DUMP` (appended), else stderr.

use std::io::Write as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use super::Shared;
use crate::sync::Lent;

/// Writes one dump with a header line.
pub(crate) fn emit(sh: &Shared, why: &str) {
    let text = format!("=== defuse dump: {why}\n{}", sh.dump());
    write_out(&text);
}

pub(crate) fn write_out(text: &str) {
    match std::env::var_os("DEFUSE_DUMP") {
        Some(path) => {
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
                let _ = f.write_all(text.as_bytes());
            }
        }
        None => {
            let _ = std::io::stderr().write_all(text.as_bytes());
        }
    }
}

/// The watcher for one evaluation; joined when dropped.
pub(crate) struct Watch {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    #[cfg(unix)]
    prev: [usize; 2],
}

impl Watch {
    pub(crate) fn start(sh: &Shared) -> Watch {
        let every = std::env::var("DEFUSE_DUMP_EVERY")
            .ok()
            .and_then(|s| s.parse::<f64>().ok())
            .filter(|s| *s > 0.0)
            .map(Duration::from_secs_f64);
        #[cfg(unix)]
        let prev = crate::sync::signals::install();
        let stop = Arc::new(AtomicBool::new(false));
        let lent = Lent::new(sh);
        let flag = stop.clone();
        let thread = std::thread::Builder::new()
            .name("defuse-watch".into())
            .spawn(move || {
                let sh = lent.get();
                let t0 = Instant::now();
                let mut last = t0;
                while !flag.load(Ordering::Acquire) {
                    std::thread::sleep(Duration::from_millis(50));
                    #[cfg(unix)]
                    {
                        let sig = crate::sync::signals::CAUGHT.swap(0, Ordering::SeqCst);
                        if sig != 0 {
                            emit(sh, &format!("signal {sig} after {:.1?}", t0.elapsed()));
                            std::process::exit(128 + sig);
                        }
                    }
                    if let Some(every) = every
                        && last.elapsed() >= every
                    {
                        last = Instant::now();
                        emit(sh, &format!("periodic, {:.1?} into the evaluation", t0.elapsed()));
                    }
                }
            })
            .ok();
        Watch {
            stop,
            thread,
            #[cfg(unix)]
            prev,
        }
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        #[cfg(unix)]
        crate::sync::signals::restore(self.prev);
    }
}
