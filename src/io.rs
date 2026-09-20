//! Ordered blocking I/O, isolated from GPUI and the network runtime.
//!
//! Submission is eager: dropping a reply does not cancel an accepted write.
//! Jobs must capture owned snapshots (including their workspace identity), never
//! GPUI entities. Await replies on the foreground executor and apply the result
//! there. Do not synchronously wait for a nested job from this worker.

use std::{
    future::Future,
    panic::{AssertUnwindSafe, catch_unwind},
    pin::Pin,
    sync::{Mutex, OnceLock},
    task::{Context, Poll},
    thread,
};

use tokio::sync::{mpsc, oneshot};

type Job = Box<dyn FnOnce() + Send + 'static>;

/// Failure to execute a job, distinct from the operation's own returned error.
#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
pub enum IoError {
    #[error("the I/O worker is unavailable")]
    Unavailable,
    #[error("the I/O operation panicked")]
    Panicked,
}

/// A reply channel. Dropping it abandons the reply, not the queued operation.
pub struct IoTask<T> {
    reply: oneshot::Receiver<Result<T, IoError>>,
}

impl<T> Future for IoTask<T> {
    type Output = Result<T, IoError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match Pin::new(&mut self.reply).poll(cx) {
            Poll::Ready(Ok(result)) => Poll::Ready(result),
            Poll::Ready(Err(_)) => Poll::Ready(Err(IoError::Unavailable)),
            Poll::Pending => Poll::Pending,
        }
    }
}

struct IoWorker {
    sender: Mutex<Option<mpsc::UnboundedSender<Job>>>,
}

impl IoWorker {
    fn start() -> Result<Self, IoError> {
        let (sender, mut receiver) = mpsc::unbounded_channel::<Job>();
        thread::Builder::new()
            .name("resolved-io".into())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        tracing::error!("could not start I/O runtime: {error}");
                        return;
                    }
                };
                runtime.block_on(async move {
                    while let Some(job) = receiver.recv().await {
                        job();
                    }
                });
            })
            .map_err(|_| IoError::Unavailable)?;
        Ok(Self {
            sender: Mutex::new(Some(sender)),
        })
    }

    fn run<T: Send + 'static>(&self, operation: impl FnOnce() -> T + Send + 'static) -> IoTask<T> {
        let (reply, receiver) = oneshot::channel();
        // An unbounded inbox keeps submission nonblocking and preserves enqueue
        // order even before callers poll their replies. Producers should debounce
        // replaceable snapshots rather than enqueueing a write per keystroke.
        let job = Box::new(move || {
            let result = catch_unwind(AssertUnwindSafe(operation)).map_err(|_| IoError::Panicked);
            let _ = reply.send(result);
        });
        // If the worker has exited, dropping the rejected job closes its reply.
        if let Some(sender) = self
            .sender
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
        {
            let _ = sender.send(job);
        }
        IoTask { reply: receiver }
    }

    fn shutdown(&self) -> IoTask<()> {
        let (reply, receiver) = oneshot::channel();
        // Taking the sole sender under the submission lock prevents producers
        // from accepting more work after the final barrier.
        if let Some(sender) = self.sender.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = sender.send(Box::new(move || {
                let _ = reply.send(Ok(()));
            }));
        }
        IoTask { reply: receiver }
    }
}

static WORKER: OnceLock<Result<IoWorker, IoError>> = OnceLock::new();

/// Enqueue a blocking operation on the shared I/O thread, in submission order.
pub fn run<T: Send + 'static>(operation: impl FnOnce() -> T + Send + 'static) -> IoTask<T> {
    match WORKER.get_or_init(IoWorker::start) {
        Ok(worker) => worker.run(operation),
        Err(error) => {
            let (reply, receiver) = oneshot::channel();
            let _ = reply.send(Err(error.clone()));
            IoTask { reply: receiver }
        }
    }
}

/// Wait for all previously submitted jobs to finish.
///
/// This is an ordering barrier, not an aggregate success result: callers must
/// still check write replies. Stop producers before using it for shutdown.
pub fn flush() -> IoTask<()> {
    run(|| ())
}

/// Stop accepting work and drain accepted jobs. Only use at process shutdown,
/// after stopping producers; unlike `flush`, this cannot be undone.
#[cfg(not(test))]
pub fn shutdown() -> IoTask<()> {
    match WORKER.get_or_init(IoWorker::start) {
        Ok(worker) => worker.shutdown(),
        Err(_) => flush(),
    }
}

// GPUI's test harness quits each synthetic App while multiple tests still share
// this process. Its deterministic quit executor cannot park on an external OS
// thread, so drain here and return a ready reply without closing the worker.
// IoWorker::shutdown itself is exercised below with isolated workers.
#[cfg(test)]
pub fn shutdown() -> IoTask<()> {
    let result = futures::executor::block_on(flush());
    let (reply, receiver) = oneshot::channel();
    let _ = reply.send(result);
    IoTask { reply: receiver }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[tokio::test]
    async fn jobs_share_one_thread_separate_from_caller() {
        let worker = IoWorker::start().unwrap();
        let caller = thread::current().id();
        let first = worker.run(|| thread::current().id()).await.unwrap();
        let second = worker.run(|| thread::current().id()).await.unwrap();
        assert_ne!(caller, first);
        assert_eq!(first, second);
    }

    #[tokio::test]
    async fn submission_is_ordered_without_polling_replies() {
        let worker = IoWorker::start().unwrap();
        let values = Arc::new(Mutex::new(Vec::new()));
        let a = Arc::clone(&values);
        let b = Arc::clone(&values);
        let first = worker.run(move || a.lock().unwrap().push(1));
        let second = worker.run(move || b.lock().unwrap().push(2));
        second.await.unwrap();
        first.await.unwrap();
        assert_eq!(*values.lock().unwrap(), [1, 2]);
    }

    #[tokio::test]
    async fn dropping_reply_does_not_cancel_write_and_barrier_drains_it() {
        let worker = IoWorker::start().unwrap();
        let values = Arc::new(Mutex::new(Vec::new()));
        let output = Arc::clone(&values);
        drop(worker.run(move || output.lock().unwrap().push(1)));
        worker.run(|| ()).await.unwrap();
        assert_eq!(*values.lock().unwrap(), [1]);
    }

    #[tokio::test]
    async fn panic_is_reported_without_stopping_later_jobs() {
        let worker = IoWorker::start().unwrap();
        assert_eq!(
            worker.run(|| panic!("test panic")).await,
            Err(IoError::Panicked)
        );
        assert_eq!(worker.run(|| 42).await, Ok(42));
    }

    #[tokio::test]
    async fn operation_errors_are_preserved() {
        let worker = IoWorker::start().unwrap();
        assert_eq!(
            worker.run(|| Err::<(), _>("disk error")).await,
            Ok(Err("disk error"))
        );
    }

    #[tokio::test]
    async fn blocked_io_does_not_block_foreground_executor() {
        let worker = IoWorker::start().unwrap();
        let (started, ready) = oneshot::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let job = worker.run(move || {
            started.send(()).unwrap();
            wait.recv().unwrap();
        });
        ready.await.unwrap();
        // This continuation can run while the I/O thread is blocked.
        release.send(()).unwrap();
        job.await.unwrap();
    }

    #[tokio::test]
    async fn shutdown_drains_accepted_jobs_and_rejects_new_work() {
        let worker = IoWorker::start().unwrap();
        let write = worker.run(|| 42);
        let drained = worker.shutdown();
        assert_eq!(worker.run(|| 100).await, Err(IoError::Unavailable));
        drained.await.unwrap();
        assert_eq!(write.await, Ok(42));
    }
}
