//! Writes settings and usage on a background thread. The meter is a child of the taskbar, so the
//! taskbar waits on this app's UI thread; a slow or busy disk must never hold it up. Writes run one
//! at a time, in the order they were queued.

use std::sync::mpsc::{channel, Sender};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::Duration;

type Job = Box<dyn FnOnce() + Send>;

static WRITER: OnceLock<Mutex<Sender<Job>>> = OnceLock::new();

/// Returns the writer thread's queue, starting the thread the first time.
fn writer() -> &'static Mutex<Sender<Job>> {
    WRITER.get_or_init(|| {
        let (sender, receiver) = channel::<Job>();
        std::thread::spawn(move || receiver.into_iter().for_each(|job| job()));
        Mutex::new(sender)
    })
}

/// Runs a write on the writer thread, after every write queued before it.
pub fn queue(job: impl FnOnce() + Send + 'static) {
    let sender = writer().lock().unwrap_or_else(PoisonError::into_inner).clone();
    if let Err(unsent) = sender.send(Box::new(job)) {
        // The writer thread is gone, so the write happens here rather than not at all.
        (unsent.0)();
    }
}

/// Waits up to `limit` for every queued write to finish, before the app quits or Windows shuts down.
pub fn wait_for_writes(limit: Duration) {
    let (done, finished) = channel();
    queue(move || {
        let _ = done.send(());
    });
    let _ = finished.recv_timeout(limit);
}
