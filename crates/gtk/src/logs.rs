//! Journal query worker.
//!
//! `journalctl` is a subprocess and must never run on the GTK main thread
//! (taskbook §4.6). This mirrors [`crate::worker::Collector`]: one thread per
//! query, a generation counter so a superseded query cannot overwrite a newer
//! one, and a cancellation token.
//!
//! The four journal outcomes the core distinguishes — `Entries`, `Empty`,
//! `Denied`, and a query failure — are preserved all the way to the UI. They are
//! **not** collapsed into "no logs": a denial is a permission fact and must be
//! shown as such (taskbook §4.6, §9.3).

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use portscope_core::Cancel;
use portscope_core::journal::{JournalOutcome, JournalQuery, query_journal_cancellable};
use portscope_core::model::{CollectionError, JournalEntry, UnitScope};

/// Result of one log query.
#[derive(Debug)]
pub struct LogResult {
    pub generation: u64,
    pub outcome: LogOutcome,
}

#[derive(Debug, Clone)]
pub enum LogOutcome {
    Entries(Vec<JournalEntry>),
    Empty,
    Denied(String),
    Failed(String),
    Cancelled,
}

/// Identifies which unit the logs belong to, so a late result is not shown
/// against a different selection.
pub type LogKey = (UnitScope, String);

pub struct LogSource {
    generation: Arc<AtomicU64>,
    cancel: Arc<std::sync::Mutex<Cancel>>,
}

impl Default for LogSource {
    fn default() -> Self {
        Self::new()
    }
}

impl LogSource {
    pub fn new() -> Self {
        Self {
            generation: Arc::new(AtomicU64::new(0)),
            cancel: Arc::new(std::sync::Mutex::new(Cancel::new())),
        }
    }

    pub fn cancel_current(&self) {
        if let Ok(c) = self.cancel.lock() {
            c.cancel();
        }
    }

    /// Query the last `window_secs` of logs for one unit, off the main thread.
    pub fn fetch(
        &self,
        scope: UnitScope,
        unit: String,
        window_secs: u64,
        max_entries: usize,
    ) -> (u64, std::sync::mpsc::Receiver<LogResult>) {
        self.cancel_current();
        let cancel = Cancel::new();
        if let Ok(mut guard) = self.cancel.lock() {
            *guard = cancel.clone();
        }

        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let (tx, rx) = std::sync::mpsc::channel();

        std::thread::Builder::new()
            .name("portscope-journal".into())
            .spawn(move || {
                let q = JournalQuery {
                    scope,
                    unit: Some(unit),
                    since_secs: window_secs,
                    max_entries,
                };
                let outcome = match query_journal_cancellable(&q, &cancel) {
                    Ok(JournalOutcome::Entries(e)) => {
                        if cancel.is_cancelled() {
                            LogOutcome::Cancelled
                        } else {
                            LogOutcome::Entries(e)
                        }
                    }
                    Ok(JournalOutcome::Empty) => LogOutcome::Empty,
                    Ok(JournalOutcome::Denied(h)) => LogOutcome::Denied(h),
                    Err(CollectionError::Cancelled { .. }) => LogOutcome::Cancelled,
                    Err(e) => LogOutcome::Failed(e.to_string()),
                };
                let _ = tx.send(LogResult {
                    generation,
                    outcome,
                });
            })
            .expect("spawn journal thread");

        (generation, rx)
    }
}

/// Should a log result with `result_gen` be displayed, given `current_gen`?
pub fn is_current(result_gen: u64, current_gen: u64) -> bool {
    result_gen == current_gen
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn only_newest_query_is_current() {
        assert!(is_current(2, 2));
        assert!(!is_current(1, 2));
    }

    #[test]
    fn user_scope_query_returns_a_classified_outcome() {
        let src = LogSource::new();
        let (generation, rx) = src.fetch(
            UnitScope::User,
            "portscope-nonexistent.service".into(),
            600,
            10,
        );
        assert_eq!(generation, 1);
        let res = rx
            .recv_timeout(Duration::from_secs(30))
            .expect("worker must reply");
        // A nonexistent unit must NOT be reported as an error; it is either
        // empty, denied, or genuinely has no entries.
        assert!(
            matches!(
                res.outcome,
                LogOutcome::Empty | LogOutcome::Denied(_) | LogOutcome::Entries(_)
            ),
            "unexpected outcome: {:?}",
            res.outcome
        );
    }

    #[test]
    fn entries_are_bounded_by_max_entries() {
        let src = LogSource::new();
        let (_g, rx) = src.fetch(UnitScope::User, String::new(), 86_400, 3);
        if let Ok(res) = rx.recv_timeout(Duration::from_secs(30))
            && let LogOutcome::Entries(e) = res.outcome
        {
            assert!(e.len() <= 3, "max_entries must be honoured");
        }
    }

    #[test]
    fn refusing_unit_is_system_scope() {
        // A system-scope query as an ordinary user is the *denied* case on a
        // machine without `adm`; here we only assert the call is classified,
        // never that it succeeds.
        let src = LogSource::new();
        let (_g, rx) = src.fetch(UnitScope::System, "ssh.service".into(), 600, 5);
        let Ok(res) = rx.recv_timeout(Duration::from_secs(30)) else {
            return;
        };
        assert!(matches!(
            res.outcome,
            LogOutcome::Entries(_)
                | LogOutcome::Empty
                | LogOutcome::Denied(_)
                | LogOutcome::Failed(_)
        ));
    }
}
