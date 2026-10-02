//! The native [`PageSink`]: a thread that writes DVI pages while TeX goes
//! on typesetting.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;

use partex_core::dviout::{DviWriter, Summary, TooLong};
use partex_core::pageir::Page;

enum Job {
    Page(Page),
    PageNow(Page),
    Finish(i32),
}

enum Reply {
    Written(Page, Result<i32, TooLong>),
    Finished(Result<Summary, TooLong>),
}

/// The receiver behind `m` (never contended: see `DviThread`).
fn got<T>(m: &mut Mutex<Receiver<T>>) -> &mut Receiver<T> {
    m.get_mut()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The writer thread, with its queue.
pub struct DviThread {
    jobs: Sender<Job>,
    /// (behind locks only to be `Sync`, so that a host holding the thread
    /// can be shared: only `&mut self` receives)
    replies: Mutex<Receiver<Reply>>,
    /// Pages written, to be reused.
    spare: Mutex<Receiver<Page>>,
    thread: Option<JoinHandle<()>>,
}

impl DviThread {
    pub fn spawn(mut writer: DviWriter, file: File) -> Self {
        let (jobs, job_rx) = channel();
        let (reply_tx, replies) = channel();
        let (spare_tx, spare) = channel();
        let thread = std::thread::spawn(move || {
            let mut out = BufWriter::with_capacity(1 << 16, file);
            // (after an error the engine stops sending pages)
            let mut failed = false;
            for job in job_rx {
                match job {
                    Job::Page(page) => {
                        if !failed {
                            failed = writer.page(&page).is_err();
                        }
                        let _ = out.write_all(&writer.drain());
                        let _ = spare_tx.send(page);
                    }
                    Job::PageNow(page) => {
                        let r = writer.page(&page).map(|()| writer.length());
                        let _ = out.write_all(&writer.drain());
                        let _ = reply_tx.send(Reply::Written(page, r));
                    }
                    Job::Finish(mag) => {
                        let r = writer.finish(mag);
                        // TeX has no way to report write errors; neither do we.
                        let _ = out.write_all(&writer.drain());
                        let _ = out.flush();
                        let _ = reply_tx.send(Reply::Finished(r));
                        return;
                    }
                }
            }
        });
        Self {
            jobs,
            replies: Mutex::new(replies),
            spare: Mutex::new(spare),
            thread: Some(thread),
        }
    }

    pub fn page(&mut self, page: Page) -> Page {
        let _ = self.jobs.send(Job::Page(page));
        got(&mut self.spare).try_recv().unwrap_or_default()
    }

    pub fn page_now(&mut self, page: Page) -> (Page, Result<i32, TooLong>) {
        let _ = self.jobs.send(Job::PageNow(page));
        let Ok(Reply::Written(page, r)) = got(&mut self.replies).recv() else {
            panic!("the DVI thread stopped")
        };
        (page, r)
    }

    pub fn finish(&mut self, mag: i32) -> Result<Summary, TooLong> {
        let _ = self.jobs.send(Job::Finish(mag));
        let Ok(Reply::Finished(r)) = got(&mut self.replies).recv() else {
            panic!("the DVI thread stopped")
        };
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        r
    }
}
