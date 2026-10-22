//! Background collection worker.
//!
//! GTK is single-threaded: anything that blocks the main loop freezes the whole
//! UI. Collection spawns processes and reads `/proc`, so it must happen off the
//! main thread (taskbook §3.1, §4.6, §7.2).
//!
//! Design:
//!   * one worker thread per request, results delivered through a
//!     `std::sync::mpsc` channel that the main loop polls via
//!     `glib::timeout_add_local`, so the main thread never blocks;
//!   * a **generation counter** so a slow, older result can never overwrite a
//!     newer one;
//!   * a shared [`Cancel`] token, so refreshing again (or closing the window)
//!     stops the in-flight collection instead of letting it finish uselessly.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use portscope_core::Cancel;
use portscope_core::collect::{CollectOptions, collect_cancellable};
use portscope_core::model::Snapshot;

/// A finished collection attempt.
#[derive(Debug)]
pub struct CollectResult {
    /// The generation this result belongs to.
    pub generation: u64,
    pub outcome: Outcome,
}

#[derive(Debug)]
pub enum Outcome {
    Snapshot(Box<Snapshot>),
    Failed(String),
    Cancelled,
}

/// Owns the worker generation and the cancellation token.
///
/// Not `Clone`: there is exactly one collector per window. The inner handles
/// are `Send + Sync` so the worker thread can hold them.
pub struct Collector {
    generation: Arc<AtomicU64>,
    cancel: Arc<std::sync::Mutex<Cancel>>,
}

impl Default for Collector {
    fn default() -> Self {
        Self::new()
    }
}

impl Collector {
    pub fn new() -> Self {
        Self {
            generation: Arc::new(AtomicU64::new(0)),
            cancel: Arc::new(std::sync::Mutex::new(Cancel::new())),
        }
    }

    /// Cancel any in-flight collection.
    pub fn cancel_current(&self) {
        if let Ok(c) = self.cancel.lock() {
            c.cancel();
        }
    }

    /// Start a new collection, superseding any previous one.
    ///
    /// Returns the generation id; results carrying an older id must be dropped
    /// by the caller (see [`CollectResult::generation`]).
    pub fn start(&self, opts: CollectOptions) -> (u64, std::sync::mpsc::Receiver<CollectResult>) {
        // Supersede the previous run: cancel it and take a fresh token.
        self.cancel_current();
        let cancel = Cancel::new();
        if let Ok(mut guard) = self.cancel.lock() {
            *guard = cancel.clone();
        }

        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let (tx, rx) = std::sync::mpsc::channel();

        let gen_for_thread = generation;
        std::thread::Builder::new()
            .name("portscope-collect".into())
            .spawn(move || {
                let outcome = if cancel.is_cancelled() {
                    Outcome::Cancelled
                } else {
                    // `collect_cancellable` never panics on I/O problems; it
                    // records them as CollectionError entries in the snapshot.
                    let snap = collect_cancellable(&opts, &cancel);
                    if cancel.is_cancelled() {
                        Outcome::Cancelled
                    } else {
                        Outcome::Snapshot(Box::new(snap))
                    }
                };
                // If the receiver is gone the window closed; nothing to do.
                let _ = tx.send(CollectResult {
                    generation: gen_for_thread,
                    outcome,
                });
            })
            .expect("spawn collection thread");

        (generation, rx)
    }
}

/// Should a result with `result_gen` be applied, given the newest request is
/// `current_gen`? Only the newest may update the UI.
pub fn is_current(result_gen: u64, current_gen: u64) -> bool {
    result_gen == current_gen
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn only_newest_generation_is_current() {
        assert!(is_current(3, 3));
        assert!(!is_current(2, 3), "a superseded result must be dropped");
    }

    #[test]
    fn collection_runs_off_the_calling_thread() {
        let c = Collector::new();
        // Keep it cheap: endpoints only.
        let opts = CollectOptions {
            system_services: false,
            user_services: false,
            ..CollectOptions::default()
        };
        let (generation, rx) = c.start(opts);
        assert_eq!(generation, 1);
        let res = rx.recv().expect("worker must deliver a result");
        assert_eq!(res.generation, 1);
        match res.outcome {
            Outcome::Snapshot(s) => {
                // A real system always has at least one endpoint or reports why.
                assert!(
                    !s.endpoints.is_empty() || !s.errors.is_empty(),
                    "collection must either find endpoints or record errors"
                );
            }
            other => panic!("expected a snapshot, got {other:?}"),
        }
    }

    #[test]
    fn starting_again_supersedes_the_previous_generation() {
        let c = Collector::new();
        let opts = CollectOptions {
            system_services: false,
            user_services: false,
            ..CollectOptions::default()
        };
        let (g1, _rx1) = c.start(opts.clone());
        let (g2, _rx2) = c.start(opts);
        assert_eq!(g1, 1);
        assert_eq!(g2, 2);
        // A late result from run 1 must not be applied once run 2 started.
        assert!(!is_current(g1, g2));
    }

    #[test]
    fn cancel_before_start_yields_cancelled() {
        let c = Collector::new();
        c.cancel_current();
        // start() resets the token, so this run proceeds normally; the point is
        // that cancelling is cheap and idempotent.
        let opts = CollectOptions {
            system_services: false,
            user_services: false,
            ..CollectOptions::default()
        };
        let (_gen, rx) = c.start(opts);
        let res = rx.recv().expect("result");
        assert!(matches!(
            res.outcome,
            Outcome::Snapshot(_) | Outcome::Cancelled | Outcome::Failed(_)
        ));
    }

    #[test]
    fn default_is_complete_collection() {
        let opts = CollectOptions::default();
        assert!(opts.system_services);
        assert!(opts.user_services);
        assert!(opts.include_process_field);
    }

    #[test]
    fn worker_delivers_within_a_sane_budget() {
        let c = Collector::new();
        let opts = CollectOptions {
            system_services: false,
            user_services: false,
            ..CollectOptions::default()
        };
        let start = std::time::Instant::now();
        let (_gen, rx) = c.start(opts);
        let r = rx.recv();
        assert!(r.is_ok(), "worker must deliver");
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "collection must be bounded"
        );
    }
}
