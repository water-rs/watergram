//! The pump-driven local executor every runtime without a platform event
//! loop shares.
//!
//! The headless, semantic, and one-shot `run` render paths own their frame
//! cadence — a pump drains queued runnables where an event loop would. The
//! queue is a plain channel: wakers send `Runnable`s from arbitrary threads
//! and the next drain runs them on the runtime's thread. Compiles on every
//! target, wasm32 included — a semantic runtime in the browser has the same
//! pump-driven shape, just no renderer behind it.

use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};

use executor_core::LocalExecutor;
use executor_core::async_task::{AsyncTask, Runnable};

#[derive(Clone, Debug)]
pub(crate) struct HeadlessMainThreadExecutor {
    runnable_tx: mpsc::Sender<Runnable>,
    runnable_rx: Rc<mpsc::Receiver<Runnable>>,
    /// Queued-but-not-yet-run runnable count. Incremented before send and
    /// decremented after each run, so `has_pending` over-reports around the
    /// hand-off instant — the safe direction for a settledness probe. Atomic
    /// because wakers clone the sender onto arbitrary threads.
    pending: Arc<AtomicUsize>,
}

thread_local! {
    /// One executor per thread, shared by every pump-driven runtime on it.
    ///
    /// `try_init_local_executor` installs a single executor per thread and the
    /// first install wins, so per-runtime executors would strand every task a
    /// second runtime spawns on the same thread (perf repetitions,
    /// multi-measure benches, any test mounting twice) in the first runtime's
    /// queue — never drained, and dropped only during thread-local teardown,
    /// where dropping a future whose destructor touches other thread-locals
    /// aborts the process.
    static THREAD_EXECUTOR: HeadlessMainThreadExecutor = HeadlessMainThreadExecutor::new();
}

impl HeadlessMainThreadExecutor {
    fn new() -> Self {
        let (runnable_tx, runnable_rx) = mpsc::channel();
        Self {
            runnable_tx,
            runnable_rx: Rc::new(runnable_rx),
            pending: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// The executor shared by every pump-driven runtime on this thread.
    pub(crate) fn thread_shared() -> Self {
        THREAD_EXECUTOR.with(Clone::clone)
    }

    /// Runs every runnable currently queued, returning whether any ran.
    ///
    /// The offscreen runner must call this while rendering: a `GpuView`'s
    /// `setup` is an async future spawned onto this executor, so a frame
    /// rendered without draining would run `render` against a renderer that has
    /// not built its pipelines yet and would emit nothing.
    pub(super) fn drain(&self) -> bool {
        let mut ran = false;
        loop {
            let Ok(runnable) = self.runnable_rx.try_recv() else {
                return ran;
            };
            ran = true;
            runnable.run();
            self.pending.fetch_sub(1, Ordering::SeqCst);
        }
    }

    /// Whether any spawned work is queued and waiting for the next drain.
    pub(super) fn has_pending(&self) -> bool {
        self.pending.load(Ordering::SeqCst) > 0
    }
}

impl LocalExecutor for HeadlessMainThreadExecutor {
    type Task<T: 'static> = AsyncTask<T>;

    fn spawn_local<Fut>(&self, fut: Fut) -> Self::Task<Fut::Output>
    where
        Fut: std::future::Future + 'static,
    {
        let runnable_tx = self.runnable_tx.clone();
        let pending = Arc::clone(&self.pending);
        let (runnable, task) = executor_core::async_task::spawn_local(fut, move |runnable| {
            pending.fetch_add(1, Ordering::SeqCst);
            if let Err(unsent) = runnable_tx.send(runnable) {
                pending.fetch_sub(1, Ordering::SeqCst);
                // Teardown race: a waker held by another thread (decoder,
                // audio, dispatch callback) fired after the runtime dropped
                // the receiver. The task can never run again, and dropping a
                // `spawn_local` runnable off its spawning thread panics by
                // design (async-task's thread check), so leak it instead —
                // bounded to shutdown, reclaimed at process exit.
                std::mem::forget(unsent);
            }
        });
        runnable.schedule();
        task
    }
}

/// Drains the thread-shared executor when the owning runtime drops.
#[derive(Debug)]
pub(super) struct DrainExecutorOnDrop(pub(super) HeadlessMainThreadExecutor);

impl Drop for DrainExecutorOnDrop {
    fn drop(&mut self) {
        let _ = self.0.drain();
    }
}
