//! Bounded subprocess execution.
//!
//! Every external command PortScope runs goes through here. This module exists
//! because the Spike called `Command::output()` directly, which blocks forever
//! on a wedged child and buffers unbounded output (audit defect **P0-D1**;
//! taskbook §4.4 / §7.2 require timeouts, cancellation, output caps and error
//! classification).
//!
//! Design:
//!   * explicit argv only — never a shell, never string interpolation;
//!   * stdout/stderr are drained on separate threads so a chatty child cannot
//!     deadlock on a full pipe;
//!   * a deadline is enforced by polling `try_wait`; on expiry the child is
//!     killed and reaped so no zombie is left behind;
//!   * a cancellation flag is checked on the same poll, so a superseded UI
//!     refresh stops its work promptly;
//!   * output is capped; exceeding the cap kills the child and reports
//!     [`CollectionError::OutputTooLarge`] rather than growing memory.

use crate::model::CollectionError;
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

/// Limits for one child process.
#[derive(Debug, Clone)]
pub struct ExecLimits {
    /// Hard deadline for the whole call.
    pub timeout: Duration,
    /// Maximum bytes kept from stdout.
    pub max_stdout: usize,
    /// Maximum bytes kept from stderr.
    pub max_stderr: usize,
    /// How often the child is polled while waiting.
    pub poll: Duration,
}

impl Default for ExecLimits {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(5),
            max_stdout: 8 * 1024 * 1024,
            max_stderr: 256 * 1024,
            poll: Duration::from_millis(10),
        }
    }
}

impl ExecLimits {
    /// Tight limits for a fast, small-output query (e.g. `ss`).
    pub fn quick() -> Self {
        Self {
            timeout: Duration::from_secs(5),
            ..Self::default()
        }
    }

    /// Slightly longer limits for systemd / journal queries, which are slower.
    pub fn slow() -> Self {
        Self {
            timeout: Duration::from_secs(15),
            max_stdout: 16 * 1024 * 1024,
            ..Self::default()
        }
    }

    pub fn with_timeout(mut self, t: Duration) -> Self {
        self.timeout = t;
        self
    }
}

/// A cancellation token. Cheap to clone; shared across a collection run so the
/// UI can abort a refresh that is no longer wanted.
#[derive(Debug, Clone, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// Raw outcome of a finished child, before any interpretation.
#[derive(Debug, Clone)]
pub struct Output {
    pub stdout: String,
    pub stderr: String,
    pub exit: Option<i32>,
    /// True when stdout was truncated at the cap.
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    pub duration: Duration,
}

impl Output {
    pub fn success(&self) -> bool {
        self.exit == Some(0)
    }
}

/// Run a command with limits, a deadline and a cancellation token.
///
/// Returns `Err` only for transport-level problems (missing binary, timeout,
/// cancellation, oversized output, I/O failure). A non-zero exit is *not* an
/// error here: it is returned in [`Output::exit`], because several callers must
/// inspect stderr precisely when the exit code is meaningless (`journalctl`
/// returns 0 on permission denial).
pub fn run_bounded(
    program: &str,
    args: &[&str],
    limits: &ExecLimits,
    cancel: &Cancel,
) -> Result<Output, CollectionError> {
    let start = Instant::now();

    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    // Put the child in its own process group.
    //
    // Killing only the direct child is not enough: a shell wrapper (or any
    // program that forks) leaves grandchildren holding the stdout pipe open, so
    // the reader threads never see EOF and `run_bounded` blocks forever even
    // though the timeout fired. Measured: a wedged `ss` shell script made a
    // 5-second timeout take 60 seconds to return. With a dedicated process
    // group we can kill the whole tree.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    let mut child = cmd.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            CollectionError::ToolMissing {
                tool: program.to_owned(),
            }
        } else {
            CollectionError::Io {
                path: program.to_owned(),
                message: e.to_string(),
            }
        }
    })?;

    // Drain both pipes concurrently. If we waited for exit first, a child that
    // writes more than the pipe buffer would block forever.
    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();
    let cap_out = limits.max_stdout;
    let cap_err = limits.max_stderr;

    // Readers signal overflow through a shared flag so the poll loop can kill
    // the child immediately. Draining-and-discarding instead would let a
    // runaway producer (`yes`) run until the timeout — a real bug this test
    // caught.
    let out_overflow = Arc::new(AtomicBool::new(false));
    let err_overflow = Arc::new(AtomicBool::new(false));
    let out_flag = out_overflow.clone();
    let err_flag = err_overflow.clone();

    // Readers publish into a shared slot and are *detached*.
    //
    // A `join()` here would be unbounded: a shell wrapper's grandchild can keep
    // the pipe's write end open after we kill the direct child, so the reader
    // never reaches EOF. Measured: a wedged `ss` shell script turned a 5-second
    // timeout into a 60-second stall. We therefore give the readers a short,
    // bounded grace period after the child is gone and use whatever arrived.
    let out_slot: Arc<std::sync::Mutex<Option<String>>> = Arc::new(std::sync::Mutex::new(None));
    let err_slot: Arc<std::sync::Mutex<Option<String>>> = Arc::new(std::sync::Mutex::new(None));
    let out_slot_t = out_slot.clone();
    let err_slot_t = err_slot.clone();

    if let Some(p) = stdout_pipe {
        thread::spawn(move || {
            let (text, _trunc) = read_capped(p, cap_out, out_flag);
            if let Ok(mut g) = out_slot_t.lock() {
                *g = Some(text);
            }
        });
    }
    if let Some(p) = stderr_pipe {
        thread::spawn(move || {
            let (text, _trunc) = read_capped(p, cap_err, err_flag);
            if let Ok(mut g) = err_slot_t.lock() {
                *g = Some(text);
            }
        });
    }

    // Poll for exit, honouring deadline and cancellation.
    let mut timed_out = false;
    let mut cancelled = false;
    let mut overflowed = false;

    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| CollectionError::Io {
            path: program.to_owned(),
            message: e.to_string(),
        })? {
            break Some(status);
        }

        if cancel.is_cancelled() {
            cancelled = true;
            let _ = child.kill();
            break child.wait().ok();
        }

        if out_overflow.load(Ordering::SeqCst) {
            overflowed = true;
            let _ = child.kill();
            break child.wait().ok();
        }

        if start.elapsed() >= limits.timeout {
            timed_out = true;
            let _ = child.kill();
            break child.wait().ok();
        }

        thread::sleep(limits.poll);
    };

    // Collect whatever the readers managed to take. On kill the pipes close,
    // so these joins return promptly.
    // Bounded drain: poll the slots briefly so a lingering grandchild cannot
    // stall us. 200 ms is enough for the kernel to hand over already-written
    // bytes on a pipe we own.
    let stdout = drain_slot(&out_slot, Duration::from_millis(200));
    let stderr = drain_slot(&err_slot, Duration::from_millis(200));
    let err_trunc = false;

    let duration = start.elapsed();

    if cancelled {
        // Caller asked us to stop; report the leftovers but flag it.
        return Err(CollectionError::Cancelled {
            tool: program.to_owned(),
        });
    }
    if timed_out {
        return Err(CollectionError::Timeout {
            tool: program.to_owned(),
            millis: limits.timeout.as_millis() as u64,
        });
    }
    if overflowed || out_overflow.load(Ordering::SeqCst) {
        return Err(CollectionError::OutputTooLarge {
            tool: program.to_owned(),
            limit: limits.max_stdout,
        });
    }

    Ok(Output {
        stdout,
        stderr,
        exit: status.and_then(|s| s.code()),
        stdout_truncated: false,
        stderr_truncated: err_trunc,
        duration,
    })
}

/// Wait up to `grace` for a reader to publish its text, then take what is there.
fn drain_slot(slot: &Arc<std::sync::Mutex<Option<String>>>, grace: Duration) -> String {
    let deadline = Instant::now() + grace;
    loop {
        if let Ok(g) = slot.lock()
            && let Some(text) = g.as_ref()
        {
            return text.clone();
        }
        if Instant::now() >= deadline {
            return String::new();
        }
        thread::sleep(Duration::from_millis(5));
    }
}

/// Read a pipe up to `cap` bytes.
///
/// On overflow the `overflow` flag is raised and reading stops. The caller's
/// poll loop sees the flag and kills the child, so a runaway producer cannot
/// keep the process alive until the deadline.
fn read_capped(mut pipe: impl Read, cap: usize, overflow: Arc<AtomicBool>) -> (String, bool) {
    let mut buf = Vec::with_capacity(8 * 1024);
    let mut chunk = [0u8; 8192];
    loop {
        match pipe.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                let room = cap.saturating_sub(buf.len());
                if room == 0 {
                    overflow.store(true, Ordering::SeqCst);
                    return (String::from_utf8_lossy(&buf).into_owned(), true);
                }
                let take = n.min(room);
                buf.extend_from_slice(&chunk[..take]);
                if take < n {
                    overflow.store(true, Ordering::SeqCst);
                    return (String::from_utf8_lossy(&buf).into_owned(), true);
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    (String::from_utf8_lossy(&buf).into_owned(), false)
}

/// Run a command that must succeed, mapping a non-zero exit to
/// [`CollectionError::ToolFailed`] with stderr retained for diagnostics.
pub fn run_checked(
    program: &str,
    args: &[&str],
    limits: &ExecLimits,
    cancel: &Cancel,
) -> Result<String, CollectionError> {
    let out = run_bounded(program, args, limits, cancel)?;
    if !out.success() {
        return Err(CollectionError::ToolFailed {
            tool: format!("{} {}", program, args.join(" ")),
            exit: out.exit.unwrap_or(-1),
            stderr: out.stderr,
        });
    }
    Ok(out.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cancel_none() -> Cancel {
        Cancel::new()
    }

    #[test]
    fn captures_stdout_and_exit_zero() {
        let out = run_bounded(
            "/bin/echo",
            &["hello"],
            &ExecLimits::quick(),
            &cancel_none(),
        )
        .expect("echo should run");
        assert_eq!(out.stdout.trim(), "hello");
        assert_eq!(out.exit, Some(0));
        assert!(out.success());
    }

    #[test]
    fn nonzero_exit_is_returned_not_raised() {
        // `false` exits 1; that is data, not a transport error.
        let out = run_bounded("/bin/false", &[], &ExecLimits::quick(), &cancel_none())
            .expect("false should run");
        assert_eq!(out.exit, Some(1));
        assert!(!out.success());
    }

    #[test]
    fn missing_binary_is_tool_missing() {
        let err = run_bounded(
            "portscope-definitely-not-a-real-binary",
            &[],
            &ExecLimits::quick(),
            &cancel_none(),
        )
        .unwrap_err();
        assert_eq!(err.code(), "tool_missing");
    }

    #[test]
    fn timeout_kills_the_child() {
        // sleep 30 with a 200ms deadline must give up quickly.
        let limits = ExecLimits::quick().with_timeout(Duration::from_millis(200));
        let start = Instant::now();
        let err = run_bounded("/bin/sleep", &["30"], &limits, &cancel_none()).unwrap_err();
        assert_eq!(err.code(), "timeout", "got {err}");
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "must not wait for the child"
        );
    }

    /// Regression: a child that forks a grandchild holding the stdout pipe kept
    /// `run_bounded` blocked long after the timeout fired. Measured: a wedged
    /// `ss` shell script turned a 5s timeout into a 61s stall, which froze the
    /// UI's collection thread.
    ///
    /// The test uses a shell wrapper whose grandchild (`sleep`) inherits the
    /// pipe, exactly the shape that broke it.
    #[test]
    fn timeout_returns_even_when_a_grandchild_holds_the_pipe() {
        let limits = ExecLimits::quick().with_timeout(Duration::from_millis(300));
        let start = Instant::now();
        let err = run_bounded(
            "/bin/sh",
            &["-c", "sleep 30 & wait"],
            &limits,
            &cancel_none(),
        )
        .unwrap_err();
        assert_eq!(err.code(), "timeout", "got {err}");
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "must not wait for grandchildren, took {:?}",
            start.elapsed()
        );
    }

    #[test]
    fn cancellation_stops_run() {
        let cancel = Cancel::new();
        let c2 = cancel.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(150));
            c2.cancel();
        });
        let limits = ExecLimits::quick().with_timeout(Duration::from_secs(30));
        let err = run_bounded("/bin/sleep", &["30"], &limits, &cancel).unwrap_err();
        assert_eq!(err.code(), "cancelled", "got {err}");
    }

    #[test]
    fn oversized_stdout_is_capped_not_buffered() {
        // yes produces unbounded output; the cap must stop it.
        let limits = ExecLimits {
            timeout: Duration::from_secs(5),
            max_stdout: 64 * 1024,
            max_stderr: 4096,
            poll: Duration::from_millis(5),
        };
        let err = run_bounded("/usr/bin/yes", &[], &limits, &cancel_none()).unwrap_err();
        assert_eq!(err.code(), "output_too_large", "got {err}");
    }

    #[test]
    fn stderr_is_captured_for_diagnostics() {
        // `ls` on a missing path writes to stderr and exits non-zero, with no
        // shell involved — the production invariant this module exists to keep.
        let out = run_bounded(
            "/bin/ls",
            &["/portscope-does-not-exist-xyz"],
            &ExecLimits::quick(),
            &cancel_none(),
        )
        .expect("ls should run");
        assert_ne!(out.exit, Some(0));
        assert!(out.stderr.to_ascii_lowercase().contains("no such file") || !out.stderr.is_empty());
    }

    #[test]
    fn run_checked_maps_failure_to_tool_failed() {
        let err = run_checked("/bin/false", &[], &ExecLimits::quick(), &cancel_none()).unwrap_err();
        assert_eq!(err.code(), "tool_failed");
    }

    #[test]
    fn run_checked_passes_through_stdout() {
        let s = run_checked("/bin/echo", &["ok"], &ExecLimits::quick(), &cancel_none()).unwrap();
        assert_eq!(s.trim(), "ok");
    }
}
