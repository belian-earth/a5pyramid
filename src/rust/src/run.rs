//! Running engine work from R: on a pool of the requested size, off R's
//! main thread, so the main thread can watch for an interrupt.
//!
//! The work runs on a scoped thread inside a fresh rayon pool. R's main
//! thread waits for it and checks for a user interrupt (Ctrl-C, or Esc in
//! an IDE) every 100 ms; on one it cancels the work, which stops at its
//! next check, and the call fails with an `interrupted` error. A pool per
//! call costs well under a millisecond and keeps forked R workers safe:
//! a child inherits no threads, and builds its own pool on its first call.

use std::sync::mpsc::{channel, RecvTimeoutError};
use std::time::Duration;

use a5pyramid::Cancel;

/// An error for R: the engine's message, whose prefix the R side maps to
/// a condition class.
pub(crate) fn r_error(e: a5pyramid::Error) -> extendr_api::Error {
    extendr_api::Error::Other(e.to_string())
}

extern "C" {
    fn R_ToplevelExec(fun: unsafe extern "C" fn(*mut std::ffi::c_void), data: *mut std::ffi::c_void)
        -> std::ffi::c_int;
    fn R_CheckUserInterrupt();
}

unsafe extern "C" fn check_interrupt(_: *mut std::ffi::c_void) {
    R_CheckUserInterrupt();
}

/// Whether the user has asked R to interrupt. `R_CheckUserInterrupt()`
/// jumps out when an interrupt is pending; `R_ToplevelExec()` catches the
/// jump and reports it as `FALSE`. Must run on R's main thread.
pub(crate) fn interrupt_pending() -> bool {
    unsafe { R_ToplevelExec(check_interrupt, std::ptr::null_mut()) == 0 }
}

const POLL: Duration = Duration::from_millis(100);

/// Run `work` on `threads` threads, cancelling it if R is interrupted.
pub(crate) fn run<T, F>(threads: usize, work: F) -> extendr_api::Result<T>
where
    T: Send,
    F: FnOnce(&Cancel) -> a5pyramid::Result<T> + Send,
{
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads.max(1))
        .build()
        .map_err(|e| extendr_api::Error::Other(format!("internal error: cannot start {threads} threads: {e}")))?;
    let cancel = Cancel::new();
    std::thread::scope(|s| {
        let (tx, rx) = channel();
        let token = cancel.clone();
        let handle = s.spawn(move || {
            let _ = tx.send(pool.install(|| work(&token)));
        });
        loop {
            match rx.recv_timeout(POLL) {
                Ok(result) => {
                    let _ = handle.join();
                    return result.map_err(r_error);
                }
                Err(RecvTimeoutError::Timeout) => {
                    if !cancel.is_cancelled() && interrupt_pending() {
                        cancel.cancel();
                    }
                }
                Err(RecvTimeoutError::Disconnected) => {
                    let msg = match handle.join() {
                        Err(payload) => payload
                            .downcast_ref::<String>()
                            .cloned()
                            .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
                            .unwrap_or_else(|| "unknown panic".into()),
                        Ok(()) => "the worker stopped without a result".into(),
                    };
                    return Err(extendr_api::Error::Other(format!("internal error: {msg}")));
                }
            }
        }
    })
}

/// CPUs this process may use (respects affinity masks and cgroup quotas).
pub(crate) fn available_cpus() -> usize {
    std::thread::available_parallelism().map_or(1, |n| n.get())
}
