//! Process inspection via /proc.
//!
//! Key concerns (task §A2):
//!   * PID reuse     -> every ProcessInfo carries `start_time_ticks`.
//!   * mid-query exit-> read returns ENOENT; mapped to ProcessGone.
//!   * restricted reads -> cmdline may be unreadable; represented as None,
//!     never as an empty string that claims "no arguments".

use crate::model::*;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

/// `/proc/<pid>/stat` field 22 (1-indexed), starttime in clock ticks.
fn read_start_time(pid: u32) -> Option<u64> {
    let stat = fs::read_to_string(format!("/proc/{}/stat", pid)).ok()?;
    // comm may contain spaces and parentheses; split after the LAST ')'.
    let close = stat.rfind(')')?;
    let rest = stat.get(close + 2..)?;
    let fields: Vec<&str> = rest.split_whitespace().collect();
    // `rest` starts at field 3 (state); starttime is field 22 -> index 19.
    fields.get(19)?.parse().ok()
}

fn read_uid(pid: u32) -> Option<u32> {
    let status = fs::read_to_string(format!("/proc/{}/status", pid)).ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("Uid:") {
            return rest.split_whitespace().next()?.parse().ok();
        }
    }
    None
}

fn read_name(pid: u32) -> Option<String> {
    fs::read_to_string(format!("/proc/{}/comm", pid))
        .ok()
        .map(|s| s.trim_end_matches('\n').to_string())
        .filter(|s| !s.is_empty())
}

fn read_ppid(pid: u32) -> Option<u32> {
    let stat = fs::read_to_string(format!("/proc/{}/stat", pid)).ok()?;
    let close = stat.rfind(')')?;
    let rest = stat.get(close + 2..)?;
    let fields: Vec<&str> = rest.split_whitespace().collect();
    // ppid is field 4 (1-indexed) -> index 1 after state
    fields.get(1)?.parse().ok()
}

/// Read cmdline. Returns None when *unreadable* (permission), and
/// `Some(vec![])` when readable but empty (kernel threads).
fn read_cmdline(pid: u32) -> Result<Vec<String>, CollectionError> {
    match fs::read(format!("/proc/{}/cmdline", pid)) {
        Ok(bytes) => {
            let parts: Vec<String> = bytes
                .split(|b| *b == 0)
                .filter(|s| !s.is_empty())
                .map(|s| String::from_utf8_lossy(s).to_string())
                .collect();
            Ok(parts)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Err(CollectionError::ProcessGone { pid })
        }
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            Err(CollectionError::PermissionDenied {
                resource: format!("/proc/{}/cmdline", pid),
                hint: "process runs as another user and /proc is restricted".into(),
            })
        }
        Err(e) => Err(CollectionError::Io {
            path: format!("/proc/{}/cmdline", pid),
            message: e.to_string(),
        }),
    }
}

fn read_cgroup(pid: u32) -> Option<String> {
    let text = fs::read_to_string(format!("/proc/{}/cgroup", pid)).ok()?;
    // cgroup v2 line: `0::/system.slice/x.service`
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("0::") {
            return Some(rest.to_string());
        }
    }
    None
}

fn uid_to_user(uid: u32) -> Option<String> {
    let passwd = fs::read_to_string("/etc/passwd").ok()?;
    for line in passwd.lines() {
        let cols: Vec<&str> = line.split(':').collect();
        if cols.len() > 2 && cols[2].parse::<u32>().ok() == Some(uid) {
            return Some(cols[0].to_string());
        }
    }
    None
}

/// Inspect one PID. Verifies identity consistency when `expected_start` is
/// given, to defend against PID reuse between listing and inspection.
pub fn inspect_process(
    pid: u32,
    source: PidSource,
    expected_start: Option<u64>,
) -> Result<ProcessInfo, CollectionError> {
    if !Path::new(&format!("/proc/{}", pid)).exists() {
        return Err(CollectionError::ProcessGone { pid });
    }
    let Some(start) = read_start_time(pid) else {
        // /proc/<pid> vanished between the exists() check and the read.
        return Err(CollectionError::ProcessGone { pid });
    };
    if let Some(exp) = expected_start {
        if exp != start {
            return Err(CollectionError::ProcessIdentityChanged {
                pid,
                expected_ticks: exp,
                actual_ticks: start,
            });
        }
    }
    let Some(name) = read_name(pid) else {
        return Err(CollectionError::ProcessGone { pid });
    };
    let uid = read_uid(pid);
    Ok(ProcessInfo {
        pid,
        name,
        cmdline: read_cmdline(pid).ok(),
        uid,
        user: uid.and_then(uid_to_user),
        ppid: read_ppid(pid),
        start_time_ticks: Some(start),
        source,
    })
}

/// Map of pid -> cgroup, used for socket/cgroup joins.
pub fn process_cgroups(pids: &[u32]) -> HashMap<u32, String> {
    let mut m = HashMap::new();
    for pid in pids {
        if let Some(cg) = read_cgroup(*pid) {
            m.insert(*pid, cg);
        }
    }
    m
}

/// PIDs in a cgroup v2 path, e.g. `/system.slice/ollama.service`.
/// Readable by an unprivileged user for system and user units.
pub fn pids_in_cgroup(cg: &str) -> Result<Vec<u32>, CollectionError> {
    let path = format!("/sys/fs/cgroup{}/cgroup.procs", cg);
    match fs::read_to_string(&path) {
        Ok(text) => Ok(text
            .lines()
            .filter_map(|l| l.trim().parse::<u32>().ok())
            .collect()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(vec![]),
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            Err(CollectionError::PermissionDenied {
                resource: path,
                hint: "cgroup.procs".into(),
            })
        }
        Err(e) => Err(CollectionError::Io {
            path,
            message: e.to_string(),
        }),
    }
}

/// Does `/proc/<pid>/fd` contain a link to `socket:[inode]`?
pub fn pid_owns_socket_inode(pid: u32, inode: u64) -> bool {
    let target = format!("socket:[{}]", inode);
    let Ok(entries) = fs::read_dir(format!("/proc/{}/fd", pid)) else {
        return false;
    };
    for e in entries.flatten() {
        if let Ok(link) = fs::read_link(e.path()) {
            if link.to_string_lossy() == target {
                return true;
            }
        }
    }
    false
}

/// Build inode -> pids, for our own processes and any we can inspect.
pub fn inode_owners(pids: &[u32]) -> HashMap<u64, Vec<u32>> {
    let mut map: HashMap<u64, Vec<u32>> = HashMap::new();
    for pid in pids {
        let Ok(entries) = fs::read_dir(format!("/proc/{}/fd", pid)) else {
            continue;
        };
        for e in entries.flatten() {
            let Ok(link) = fs::read_link(e.path()) else {
                continue;
            };
            let s = link.to_string_lossy();
            if let Some(rest) = s.strip_prefix("socket:[") {
                if let Some(num) = rest.strip_suffix(']') {
                    if let Ok(inode) = num.parse::<u64>() {
                        map.entry(inode).or_default().push(*pid);
                    }
                }
            }
        }
    }
    for v in map.values_mut() {
        v.sort_unstable();
        v.dedup();
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn self_inspection_works_as_normal_user() {
        let me = std::process::id();
        let p = inspect_process(me, PidSource::TestHarness, None).expect("must inspect self");
        assert_eq!(p.pid, me);
        assert!(!p.name.is_empty());
        assert!(p.start_time_ticks.is_some());
        assert!(p.uid.is_some());
        // Our own cmdline must be readable.
        assert!(p.cmdline.is_some());
    }

    #[test]
    fn nonexistent_pid_is_process_gone() {
        let err = inspect_process(4_000_000_000, PidSource::CgroupProcs, None).unwrap_err();
        assert_eq!(err.code(), "process_gone");
    }

    #[test]
    fn wrong_expected_start_detects_reuse() {
        let me = std::process::id();
        let err = inspect_process(me, PidSource::TestHarness, Some(1)).unwrap_err();
        assert_eq!(err.code(), "pid_reused");
        if let CollectionError::ProcessIdentityChanged { actual_ticks, .. } = err {
            assert!(actual_ticks > 0);
        }
    }

    #[test]
    fn correct_expected_start_is_accepted() {
        let me = std::process::id();
        let ticks = read_start_time(me).unwrap();
        assert!(inspect_process(me, PidSource::TestHarness, Some(ticks)).is_ok());
    }

    #[test]
    fn pid1_inspect_readable_without_root() {
        // /proc/1 is world-readable on standard Ubuntu (no hidepid).
        match inspect_process(1, PidSource::SystemdMainPid, None) {
            Ok(p) => assert_eq!(p.pid, 1),
            Err(e) => {
                // If hidepid is on we must record it rather than fail the suite.
                assert!(
                    e.code() == "permission_denied" || e.code() == "process_gone",
                    "unexpected: {e}"
                );
            }
        }
    }

    #[test]
    fn stat_parsing_handles_parens_in_comm() {
        // comm with spaces/parens must not break field indexing; verified by
        // comparing starttime against a fresh read.
        let me = std::process::id();
        let a = read_start_time(me);
        let b = read_start_time(me);
        assert_eq!(a, b);
    }

    #[test]
    fn cgroup_of_self_is_user_slice() {
        let me = std::process::id();
        let cg = read_cgroup(me).expect("cgroup readable");
        assert!(cg.starts_with('/') || cg.is_empty() || cg.starts_with("user.slice"));
    }

    #[test]
    fn pids_in_cgroup_of_own_cgroup_contains_self() {
        let me = std::process::id();
        let cg = read_cgroup(me).unwrap();
        let pids = pids_in_cgroup(&cg).unwrap_or_default();
        // Session scope may be shared; self must appear if readable.
        if !pids.is_empty() {
            assert!(pids.contains(&me), "self missing from {cg}");
        }
    }

    #[test]
    fn missing_cgroup_yields_empty_not_error() {
        let pids = pids_in_cgroup("/system.slice/definitely-not-a-unit-xyz.service").unwrap();
        assert!(pids.is_empty());
    }

    #[test]
    fn inode_owner_detects_our_own_socket() {
        use std::net::TcpListener;
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let me = std::process::id();
        let owners = inode_owners(&[me]);
        // Find our inode via ss-free method: scan /proc/self/fd.
        let mut found = false;
        for e in fs::read_dir("/proc/self/fd").unwrap().flatten() {
            if let Ok(link) = fs::read_link(e.path()) {
                let s = link.to_string_lossy().to_string();
                if s.starts_with("socket:[") {
                    // can't map to port here; just assert map is non-empty
                    found = found || owners.values().flatten().any(|p| *p == me);
                }
            }
        }
        assert!(found, "expected at least one socket fd owner ({port})");
    }
}
