//! Background work: a fixed pool of threads for short tasks (opening a
//! Package, decoding thumbnails and previews, searching), dedicated threads
//! for long batch jobs (the Asset Index build, extractions), and a channel
//! back to the UI thread.
//!
//! Every message sent back wakes the UI with a repaint request; nothing
//! else does, so an idle window does no work.

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;

type Task = Box<dyn FnOnce() + Send>;

/// The number of pool threads: one fewer than the CPU cores, so the UI
/// thread always has a core, and at least one.
#[must_use]
pub fn pool_size() -> usize {
    thread::available_parallelism()
        .map_or(2, std::num::NonZero::get)
        .saturating_sub(1)
        .max(1)
}

/// A fixed set of worker threads running queued tasks in order.
pub struct Pool {
    tx: Option<Sender<Task>>,
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "only tests wait for the workers")
    )]
    threads: Vec<thread::JoinHandle<()>>,
}

impl Pool {
    #[must_use]
    pub fn new(threads: usize) -> Self {
        let (tx, rx) = mpsc::channel::<Task>();
        let rx = Arc::new(Mutex::new(rx));
        let threads = (0..threads.max(1))
            .map(|i| {
                let rx = Arc::clone(&rx);
                thread::Builder::new()
                    .name(format!("ae3-worker-{i}"))
                    .spawn(move || {
                        loop {
                            // Hold the lock only while taking a task.
                            let task = rx.lock().unwrap_or_else(PoisonError::into_inner).recv();
                            match task {
                                Ok(task) => task(),
                                Err(_) => break,
                            }
                        }
                    })
                    .expect("spawning a worker thread")
            })
            .collect();
        Self {
            tx: Some(tx),
            threads,
        }
    }

    /// Queues `task`.
    pub fn spawn(&self, task: impl FnOnce() + Send + 'static) {
        if let Some(tx) = &self.tx {
            // The workers live as long as the pool, so this cannot fail.
            let _ = tx.send(Box::new(task));
        }
    }

    #[cfg(test)]
    #[must_use]
    pub fn threads(&self) -> usize {
        self.threads.len()
    }
}

impl Pool {
    /// Lets queued tasks finish, then stops the workers and waits for them.
    #[cfg(test)]
    pub fn join(mut self) {
        self.tx = None;
        for handle in self.threads.drain(..) {
            let _ = handle.join();
        }
    }
}

impl Drop for Pool {
    /// Stops the workers once their queue is empty, without waiting: a
    /// closing window must not wait for a download in progress.
    fn drop(&mut self) {
        self.tx = None;
    }
}

/// Starts a long job on its own thread.
pub fn spawn_long(name: &str, job: impl FnOnce() + Send + 'static) -> thread::JoinHandle<()> {
    thread::Builder::new()
        .name(name.to_owned())
        .spawn(job)
        .expect("spawning a job thread")
}

/// Sends messages to the UI thread and wakes it.
pub struct Outbox<M> {
    tx: Sender<M>,
    wake: Arc<dyn Fn() + Send + Sync>,
}

impl<M> Clone for Outbox<M> {
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
            wake: Arc::clone(&self.wake),
        }
    }
}

impl<M: Send> Outbox<M> {
    /// An outbox and the receiver the UI drains each frame. `wake` asks
    /// the UI for a repaint.
    pub fn new(wake: impl Fn() + Send + Sync + 'static) -> (Self, Receiver<M>) {
        let (tx, rx) = mpsc::channel();
        (
            Self {
                tx,
                wake: Arc::new(wake),
            },
            rx,
        )
    }

    /// Sends `message` and wakes the UI. A closed UI is not an error.
    pub fn send(&self, message: M) {
        if self.tx.send(message).is_ok() {
            (self.wake)();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use super::*;

    #[test]
    fn pool_runs_every_task() {
        let count = Arc::new(AtomicUsize::new(0));
        {
            let pool = Pool::new(3);
            assert_eq!(pool.threads(), 3);
            for _ in 0..50 {
                let count = Arc::clone(&count);
                pool.spawn(move || {
                    count.fetch_add(1, Ordering::Relaxed);
                });
            }
            pool.join();
        }
        assert_eq!(count.load(Ordering::Relaxed), 50);
        assert!(pool_size() >= 1);
    }

    #[test]
    fn outbox_wakes_once_per_message() {
        let wakes = Arc::new(AtomicUsize::new(0));
        let w = Arc::clone(&wakes);
        let (outbox, rx) = Outbox::new(move || {
            w.fetch_add(1, Ordering::Relaxed);
        });
        let pool = Pool::new(2);
        for i in 0..5 {
            let outbox = outbox.clone();
            pool.spawn(move || outbox.send(i));
        }
        let mut got: Vec<i32> = (0..5)
            .map(|_| rx.recv_timeout(Duration::from_secs(5)).unwrap())
            .collect();
        got.sort_unstable();
        assert_eq!(got, [0, 1, 2, 3, 4]);
        pool.join();
        assert_eq!(wakes.load(Ordering::Relaxed), 5);
        drop(rx);
        // Sending to a closed UI neither panics nor wakes.
        outbox.send(9);
        assert_eq!(wakes.load(Ordering::Relaxed), 5);
    }

    #[test]
    fn long_jobs_run_on_their_own_thread() {
        let (outbox, rx) = Outbox::new(|| {});
        let handle = spawn_long("test-job", move || {
            outbox.send(thread::current().name().map(str::to_owned));
        });
        handle.join().unwrap();
        assert_eq!(rx.recv().unwrap().as_deref(), Some("test-job"));
    }
}
