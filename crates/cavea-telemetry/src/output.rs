//! Where log records go: a bounded queue, written out by a thread of its own.
//!
//! A task that logs only formats its record and queues it; it never waits for
//! the output and never fails. A record that finds the queue full, or whose
//! write fails (a full disk, a closed pipe, a closed descriptor), is dropped
//! and counted ([`Output::dropped`], the metric
//! `cavea_log_records_dropped_total`). Nothing is ever reported on standard
//! error: that is what made a server whose log output failed stop serving
//! (13 §7.4).

use std::io::{self, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::time::{Duration, Instant};

use tracing_subscriber::fmt::MakeWriter;

/// Records the queue holds at most: about 8 MiB of typical records, and over
/// a second of the busiest server's output.
pub const QUEUE_RECORDS: usize = 16_384;

/// Records written in one `write`, at most.
const BATCH: usize = 256;

enum Message {
    Record(Vec<u8>),
    /// Answer once every record queued before this one has been handled.
    Flush(SyncSender<()>),
}

/// A log output: cheap to clone, each clone queueing to the same thread.
#[derive(Clone)]
pub struct Output {
    queue: SyncSender<Message>,
    dropped: Arc<AtomicU64>,
}

impl std::fmt::Debug for Output {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Output")
            .field("dropped", &self.dropped())
            .finish_non_exhaustive()
    }
}

impl Output {
    /// Write to `out` from a new thread named `cavea-log`, queueing at most
    /// `capacity` records.
    pub fn spawn<W: Write + Send + 'static>(out: W, capacity: usize) -> io::Result<Self> {
        let (queue, rx) = mpsc::sync_channel(capacity.max(1));
        let dropped = Arc::new(AtomicU64::new(0));
        let counted = dropped.clone();
        std::thread::Builder::new()
            .name("cavea-log".into())
            .spawn(move || write_out(out, &rx, &counted))?;
        Ok(Self { queue, dropped })
    }

    /// Records dropped since start-up: queue full or not written.
    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Wait up to `within` for every record queued so far to be written (or
    /// dropped). `false` if that took longer: the output is stalled.
    pub fn flush(&self, within: Duration) -> bool {
        let deadline = Instant::now() + within;
        let (done, wait) = mpsc::sync_channel(1);
        let mut message = Message::Flush(done);
        loop {
            match self.queue.try_send(message) {
                Ok(()) => break,
                Err(TrySendError::Disconnected(_)) => return true,
                Err(TrySendError::Full(back)) if Instant::now() < deadline => {
                    message = back;
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(TrySendError::Full(_)) => return false,
            }
        }
        let left = deadline.saturating_duration_since(Instant::now());
        !matches!(wait.recv_timeout(left), Err(RecvTimeoutError::Timeout))
    }
}

/// The writer thread: batches what is queued into one write and flush.
fn write_out<W: Write>(mut out: W, rx: &Receiver<Message>, dropped: &AtomicU64) {
    let mut buf = Vec::new();
    let mut waiting = Vec::new();
    while let Ok(first) = rx.recv() {
        let mut records = 0u64;
        let mut next = Some(first);
        while let Some(message) = next.take() {
            match message {
                Message::Record(record) => {
                    buf.extend_from_slice(&record);
                    records += 1;
                }
                Message::Flush(done) => waiting.push(done),
            }
            if records < BATCH as u64 {
                next = rx.try_recv().ok();
            }
        }
        if records > 0 && out.write_all(&buf).and_then(|()| out.flush()).is_err() {
            dropped.fetch_add(records, Ordering::Relaxed);
        }
        buf.clear();
        for done in waiting.drain(..) {
            let _ = done.try_send(());
        }
    }
}

/// One record being formatted; queued when the formatter drops it.
pub struct Record<'a> {
    output: &'a Output,
    bytes: Vec<u8>,
}

impl Write for Record<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.bytes.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for Record<'_> {
    fn drop(&mut self) {
        if self.bytes.is_empty() {
            return;
        }
        let record = Message::Record(std::mem::take(&mut self.bytes));
        if self.output.queue.try_send(record).is_err() {
            self.output.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}

impl<'a> MakeWriter<'a> for Output {
    type Writer = Record<'a>;

    fn make_writer(&'a self) -> Record<'a> {
        Record {
            output: self,
            bytes: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "tests")]

    use std::sync::{Condvar, Mutex};

    use super::*;
    use crate::{LogFormat, subscriber};

    /// Fails every write, as a full disk or a closed pipe does.
    struct Failing;

    impl Write for Failing {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::other("no space left on device"))
        }

        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::other("no space left on device"))
        }
    }

    /// Blocks every write until released, as a pipe whose reader stopped.
    #[derive(Clone, Default)]
    struct Stuck(Arc<(Mutex<bool>, Condvar)>);

    impl Stuck {
        fn release(&self) {
            *self.0.0.lock().unwrap() = true;
            self.0.1.notify_all();
        }
    }

    impl Write for Stuck {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            let (released, wake) = &*self.0;
            let _released = wake
                .wait_while(released.lock().unwrap(), |released| !*released)
                .unwrap();
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// Logging through an output that always fails neither panics nor
    /// reports anything: every record is counted as dropped.
    #[test]
    fn records_that_cannot_be_written_are_dropped_and_counted() {
        let output = Output::spawn(Failing, 64).unwrap();
        let logs = subscriber("info", LogFormat::Json, output.clone()).unwrap();
        tracing::subscriber::with_default(logs, || {
            for i in 0..1_000 {
                tracing::info!(i, "a record");
                if i % 50 == 0 {
                    assert!(output.flush(Duration::from_secs(5)));
                }
            }
        });
        assert!(output.flush(Duration::from_secs(5)));
        assert_eq!(output.dropped(), 1_000);
    }

    /// An output that stops taking writes never stalls the code that logs:
    /// once the queue is full, records are dropped and counted, and the
    /// rest are written when the output resumes.
    #[test]
    fn a_stuck_output_does_not_block_logging() {
        let stuck = Stuck::default();
        let output = Output::spawn(stuck.clone(), 16).unwrap();
        let logs = subscriber("info", LogFormat::Text, output.clone()).unwrap();
        let started = Instant::now();
        tracing::subscriber::with_default(logs, || {
            for i in 0..10_000 {
                tracing::info!(i, "a record");
            }
        });
        assert!(started.elapsed() < Duration::from_secs(5));
        // The thread holds one batch, the queue 16 records; the rest dropped.
        assert!(output.dropped() >= 10_000 - 16 - BATCH as u64);
        assert!(!output.flush(Duration::from_millis(50)));
        stuck.release();
        assert!(output.flush(Duration::from_secs(5)));
    }
}
