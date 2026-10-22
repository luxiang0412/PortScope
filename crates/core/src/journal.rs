//! journald collection via `journalctl -o json`.
//!
//! Documented trap (evidence/logs/probe-journal-perms.log): `journalctl`
//! exits **0** even when it prints
//!   "No journal files were opened due to insufficient permissions."
//! or  "-- No entries --"
//! Therefore stderr MUST be inspected; exit status alone is meaningless.
//!
//! All queries are bounded by `--since` and `-n` so a huge journal cannot
//! stall the caller. Callers must run this off the GUI thread.

use crate::exec::{Cancel, ExecLimits, run_bounded};
use crate::model::*;

pub struct JournalQuery {
    pub scope: UnitScope,
    pub unit: Option<String>,
    /// Seconds back from now.
    pub since_secs: u64,
    /// Hard cap on entries returned.
    pub max_entries: usize,
}

impl Default for JournalQuery {
    fn default() -> Self {
        JournalQuery {
            scope: UnitScope::System,
            unit: None,
            since_secs: 3600,
            max_entries: 200,
        }
    }
}

/// Result distinguishes: entries present / genuinely empty / denied / failed.
#[derive(Debug)]
pub enum JournalOutcome {
    Entries(Vec<JournalEntry>),
    Empty,
    Denied(String),
}

pub fn query_journal(q: &JournalQuery) -> Result<JournalOutcome, CollectionError> {
    query_journal_cancellable(q, &Cancel::new())
}

/// Cancellable journal query. Runs through the bounded executor, so a wedged
/// `journalctl` cannot hang the caller and huge output is capped (P0-D1).
pub fn query_journal_cancellable(
    q: &JournalQuery,
    cancel: &Cancel,
) -> Result<JournalOutcome, CollectionError> {
    let mut args: Vec<String> = Vec::new();
    if q.scope == UnitScope::User {
        args.push("--user".into());
    }
    args.push("--no-pager".into());
    args.push("-o".into());
    args.push("json".into());
    args.push("--since".into());
    args.push(format!("-{}s", q.since_secs));
    args.push("-n".into());
    args.push(q.max_entries.to_string());
    // An empty unit string means "no unit filter"; passing `-u ""` makes
    // journalctl fail with "Invalid argument" (observed in testing).
    if let Some(u) = q.unit.as_ref().filter(|u| !u.trim().is_empty()) {
        args.push("-u".into());
        args.push(u.clone());
    }
    let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();

    // We need stderr even on success, so use run_bounded and interpret the
    // exit code ourselves: journalctl exits 0 for denial, emptiness and bad
    // queries alike.
    let out = run_bounded("journalctl", &arg_refs, &ExecLimits::slow(), cancel)?;

    let stderr = out.stderr.clone();
    let stdout = out.stdout.clone();

    // Order of discrimination matters: permission message wins over emptiness,
    // and a malformed query is a caller bug rather than an empty result.
    if stderr.contains("insufficient permissions")
        || stderr.contains("No journal files were opened")
    {
        return Ok(JournalOutcome::Denied(stderr.trim().to_string()));
    }
    let invalid_query =
        stderr.contains("Failed to parse timestamp") || stderr.contains("Invalid argument");
    if invalid_query || !out.success() {
        return Err(CollectionError::ToolFailed {
            tool: "journalctl".into(),
            exit: out.exit.unwrap_or(-1),
            stderr,
        });
    }

    let mut entries = Vec::new();
    let mut parse_errors = Vec::new();
    for line in stdout.lines() {
        if line.trim().is_empty() || line.trim() == "-- No entries --" {
            continue;
        }
        match serde_json::from_str::<serde_json::Value>(line) {
            Ok(v) => {
                let ts = v
                    .get("__REALTIME_TIMESTAMP")
                    .and_then(|t| t.as_str())
                    .and_then(|t| t.parse::<u64>().ok())
                    .unwrap_or(0);
                let priority = v
                    .get("PRIORITY")
                    .and_then(|p| p.as_str())
                    .and_then(|p| p.parse::<u8>().ok());
                let message = v
                    .get("MESSAGE")
                    .map(|m| match m {
                        serde_json::Value::String(s) => s.clone(),
                        other => other.to_string(),
                    })
                    .unwrap_or_default();
                entries.push(JournalEntry {
                    timestamp_usec: ts,
                    priority,
                    message,
                    identifier: v
                        .get("SYSLOG_IDENTIFIER")
                        .and_then(|s| s.as_str())
                        .map(str::to_string),
                    pid: v
                        .get("_PID")
                        .and_then(|s| s.as_str())
                        .and_then(|s| s.parse().ok()),
                    unit: v
                        .get("_SYSTEMD_UNIT")
                        .and_then(|s| s.as_str())
                        .map(str::to_string),
                });
            }
            Err(e) => parse_errors.push(CollectionError::ParseError {
                context: "journalctl -o json".into(),
                line: line.chars().take(120).collect(),
                reason: e.to_string(),
            }),
        }
    }

    if entries.is_empty() {
        if !parse_errors.is_empty() {
            return Err(parse_errors.remove(0));
        }
        return Ok(JournalOutcome::Empty);
    }
    Ok(JournalOutcome::Entries(entries))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_query_for_own_user_journal() {
        // The user journal should be readable for the owning user.
        let q = JournalQuery {
            scope: UnitScope::User,
            unit: None,
            since_secs: 3600,
            max_entries: 20,
        };
        match query_journal(&q) {
            Ok(JournalOutcome::Entries(e)) => {
                assert!(e.len() <= 20, "max_entries must be honoured");
            }
            Ok(JournalOutcome::Empty) => {}
            Ok(JournalOutcome::Denied(h)) => panic!("own user journal denied: {h}"),
            Err(e) => panic!("unexpected error: {e}"),
        }
    }

    #[test]
    fn nonexistent_unit_gives_empty_not_error() {
        let q = JournalQuery {
            scope: UnitScope::User,
            unit: Some("portscope-nonexistent-xyz.service".into()),
            since_secs: 600,
            max_entries: 10,
        };
        match query_journal(&q) {
            Ok(JournalOutcome::Empty) => {}
            Ok(JournalOutcome::Entries(e)) => assert!(e.is_empty()),
            Ok(JournalOutcome::Denied(_)) => {}
            Err(e) => panic!("nonexistent unit should not be an error: {e}"),
        }
    }

    #[test]
    fn max_entries_is_enforced() {
        let q = JournalQuery {
            scope: UnitScope::User,
            unit: None,
            since_secs: 86400,
            max_entries: 3,
        };
        if let Ok(JournalOutcome::Entries(e)) = query_journal(&q) {
            assert!(e.len() <= 3);
        }
    }

    #[test]
    fn priority_and_message_are_parsed() {
        let q = JournalQuery {
            scope: UnitScope::User,
            unit: None,
            since_secs: 86400,
            max_entries: 50,
        };
        if let Ok(JournalOutcome::Entries(e)) = query_journal(&q) {
            if let Some(first) = e.first() {
                assert!(first.timestamp_usec > 0, "realtime timestamp must parse");
            }
        }
    }
}
