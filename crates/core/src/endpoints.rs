//! Endpoint collection.
//!
//! Strategy (validated in evidence/logs/probe-*.log):
//!   * Primary:  `ss -tulnHe` — human-stable columns, and `-e` yields
//!     `uid:`, `ino:`, `cgroup:` WITHOUT `-p`.
//!   * Optional: `ss -tulnpe` adds `users:` for processes we own.
//!   * Fallback: parse `/proc/net/{tcp,tcp6,udp,udp6}` when `ss` is missing.
//!   * NOT used:  `ss --json` — unsupported on iproute2 6.1 (Ubuntu 24.04).

use crate::exec::{Cancel, ExecLimits, run_checked};
use crate::model::*;

/// Run a command capturing stdout/stderr, mapping failures to CollectionError.
///
/// Phase 1: this now goes through [`crate::exec`] so every external call has a
/// deadline, a cancellation path and an output cap (audit P0-D1).
pub fn run_capture(tool: &str, args: &[&str]) -> Result<String, CollectionError> {
    let cancel = Cancel::new();
    run_checked(tool, args, &ExecLimits::quick(), &cancel)
}

/// A parsed `ss` row plus the optional pid hint from `-p`.
#[derive(Debug, Clone)]
pub struct SsRow {
    pub endpoint: Endpoint,
    pub pids: Vec<u32>,
}

/// Parse `ss -tulnHe` (and `-p` if present) output.
///
/// The `ss` output format is column-aligned but values can contain no spaces,
/// so whitespace splitting is reliable. Columns:
///   Netid State Recv-Q Send-Q Local Peer [Process|Extended...]
pub fn parse_ss(output: &str) -> (Vec<SsRow>, Vec<CollectionError>) {
    let mut rows = Vec::new();
    let mut errors = Vec::new();

    for line in output.lines() {
        let line = line.trim_end();
        if line.trim().is_empty() {
            continue;
        }
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 5 {
            errors.push(CollectionError::ParseError {
                context: "ss".into(),
                line: line.into(),
                reason: format!("expected >=5 columns, got {}", cols.len()),
            });
            continue;
        }
        // `ss` includes a Netid column when more than one protocol is
        // requested (`-tu`), but omits it for a single protocol (`-t`).
        // Both shapes occur in practice, so detect rather than assume.
        let (proto, rest_cols): (Protocol, &[&str]) = match cols[0] {
            "tcp" => (Protocol::Tcp, &cols[1..]),
            "udp" => (Protocol::Udp, &cols[1..]),
            "raw" => (Protocol::Other, &cols[1..]),
            _ => {
                if cols[0] == "LISTEN" {
                    (Protocol::Tcp, &cols[0..])
                } else if cols[0] == "UNCONN" || cols[0] == "ESTAB" {
                    (Protocol::Udp, &cols[0..])
                } else {
                    errors.push(CollectionError::ParseError {
                        context: "ss".into(),
                        line: line.into(),
                        reason: format!("unrecognised leading column {:?}", cols[0]),
                    });
                    continue;
                }
            }
        };
        if rest_cols.len() < 4 {
            errors.push(CollectionError::ParseError {
                context: "ss".into(),
                line: line.into(),
                reason: format!("expected >=4 columns after netid, got {}", rest_cols.len()),
            });
            continue;
        }
        let state = EndpointState::from_ss_str(proto, rest_cols[0]);

        let Some((local_addr, scope, local_port)) = parse_addr_port(rest_cols[3]) else {
            errors.push(CollectionError::ParseError {
                context: "ss local address".into(),
                line: line.into(),
                reason: format!("unparsable local address {:?}", rest_cols[3]),
            });
            continue;
        };

        let ip_version = if local_addr.contains(':') {
            IpVersion::V6
        } else {
            IpVersion::V4
        };

        let mut uid = None;
        let mut inode = None;
        let mut cgroup = None;
        let mut pids = Vec::new();

        for tok in &rest_cols[4..] {
            if let Some(v) = tok.strip_prefix("uid:") {
                uid = v.parse().ok();
            } else if let Some(v) = tok.strip_prefix("ino:") {
                inode = v.parse().ok();
            } else if let Some(v) = tok.strip_prefix("cgroup:") {
                cgroup = Some(v.to_string());
            } else if let Some(rest) = tok.strip_prefix("users:") {
                pids.extend(parse_pids_from_users(rest));
            }
        }
        pids.sort_unstable();
        pids.dedup();

        rows.push(SsRow {
            endpoint: Endpoint {
                protocol: proto,
                state,
                local_addr,
                scope,
                local_port,
                ip_version,
                inode,
                uid,
                cgroup,
            },
            pids,
        });
    }

    (rows, errors)
}

/// Extract every `pid=N` from an `ss -p` users field.
///
/// Input shape: `(("node",pid=1568925,fd=32),("x",pid=2,fd=3))`
/// We only accept digits that immediately follow the literal `pid=`.
fn parse_pids_from_users(s: &str) -> Vec<u32> {
    let mut out = Vec::new();
    let mut rest = s;
    while let Some(pos) = rest.find("pid=") {
        rest = &rest[pos + 4..];
        let end = rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        let digits = &rest[..end];
        if !digits.is_empty() {
            if let Ok(p) = digits.parse::<u32>() {
                if p > 0 {
                    out.push(p);
                }
            }
        }
        rest = &rest[end..];
    }
    out
}

/// Split `addr:port`, `[v6]:port`, `addr%if:port`, `[v6%if]:port`.
fn parse_addr_port(s: &str) -> Option<(String, Option<String>, u16)> {
    // Find the last ':' that separates port (rightmost).
    let idx = s.rfind(':')?;
    let (addr_part, port_part) = s.split_at(idx);
    let port_part = &port_part[1..];
    let port: u16 = port_part.parse().ok()?;

    let addr_part = addr_part.trim_start_matches('[').trim_end_matches(']');
    let (addr, scope) = match addr_part.split_once('%') {
        Some((a, ifc)) => (a.to_string(), Some(ifc.to_string())),
        None => (addr_part.to_string(), None),
    };
    Some((addr, scope, port))
}

/// Collect endpoints. Returns rows + errors; never panics, never requires root.
pub fn collect_endpoints(include_processes: bool) -> (Vec<SsRow>, Vec<CollectionError>) {
    collect_endpoints_cancellable(include_processes, &Cancel::new())
}

/// Cancellable variant used by the collector.
pub fn collect_endpoints_cancellable(
    include_processes: bool,
    cancel: &Cancel,
) -> (Vec<SsRow>, Vec<CollectionError>) {
    let mut args = ["-tulneH".to_string()];
    if include_processes {
        args[0].push('p');
    }
    let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();

    match run_checked("ss", &arg_refs, &ExecLimits::quick(), cancel) {
        Ok(out) => parse_ss(&out),
        Err(CollectionError::ToolMissing { .. }) => collect_endpoints_proc(),
        Err(e) => (Vec::new(), vec![e]),
    }
}

/// Fallback via `/proc/net/{tcp,tcp6,udp,udp6}`. No process info at all;
/// only uid + inode, which is still enough for cgroup-less association work
/// (we can still match inode against `/proc/<pid>/fd`).
pub fn collect_endpoints_proc() -> (Vec<SsRow>, Vec<CollectionError>) {
    let mut rows = Vec::new();
    let mut errors = Vec::new();
    let files = [
        ("/proc/net/tcp", Protocol::Tcp, IpVersion::V4),
        ("/proc/net/tcp6", Protocol::Tcp, IpVersion::V6),
        ("/proc/net/udp", Protocol::Udp, IpVersion::V4),
        ("/proc/net/udp6", Protocol::Udp, IpVersion::V6),
    ];
    for (path, proto, ipv) in files {
        let Ok(text) = std::fs::read_to_string(path) else {
            errors.push(CollectionError::Io {
                path: path.into(),
                message: "cannot read".into(),
            });
            continue;
        };
        for line in text.lines().skip(1) {
            let cols: Vec<&str> = line.split_whitespace().collect();
            if cols.len() < 10 {
                continue;
            }
            let Some((addr_hex, port_hex)) = cols[1].split_once(':') else {
                errors.push(CollectionError::ParseError {
                    context: path.into(),
                    line: line.into(),
                    reason: "bad local_address".into(),
                });
                continue;
            };
            let Ok(port) = u16::from_str_radix(port_hex, 16) else {
                errors.push(CollectionError::ParseError {
                    context: path.into(),
                    line: line.into(),
                    reason: "bad port hex".into(),
                });
                continue;
            };
            let Some(addr) = decode_proc_addr(addr_hex, ipv) else {
                errors.push(CollectionError::ParseError {
                    context: path.into(),
                    line: line.into(),
                    reason: "bad addr hex".into(),
                });
                continue;
            };
            let state_code = u8::from_str_radix(cols[3], 16).unwrap_or(0);
            let state = match proto {
                Protocol::Tcp => EndpointState::from_tcp_code(state_code),
                Protocol::Udp => EndpointState::from_udp_code(state_code),
                _ => EndpointState::Unknown,
            };
            let uid = cols[7].parse().ok();
            let inode = cols[9].parse::<u64>().ok().filter(|v| *v != 0);
            rows.push(SsRow {
                endpoint: Endpoint {
                    protocol: proto,
                    state,
                    local_addr: addr,
                    scope: None,
                    local_port: port,
                    ip_version: ipv,
                    inode,
                    uid,
                    cgroup: None,
                },
                pids: vec![],
            });
        }
    }
    (rows, errors)
}

/// `/proc/net/*` stores addresses as 32-bit words in host byte order.
fn decode_proc_addr(hex: &str, ipv: IpVersion) -> Option<String> {
    match ipv {
        IpVersion::V4 => {
            let v = u32::from_str_radix(hex, 16).ok()?;
            let b = v.to_le_bytes();
            Some(format!("{}.{}.{}.{}", b[0], b[1], b[2], b[3]))
        }
        IpVersion::V6 => {
            if hex.len() != 32 {
                return None;
            }
            let mut groups = Vec::new();
            for i in 0..4 {
                let word = u32::from_str_radix(&hex[i * 8..i * 8 + 8], 16).ok()?;
                let b = word.to_le_bytes();
                // Each 32-bit word is stored little-endian in 4 separate bytes.
                let seg = [
                    u16::from_be_bytes([b[0], b[1]]),
                    u16::from_be_bytes([b[2], b[3]]),
                ];
                groups.extend_from_slice(&seg);
            }
            // Compress like the kernel does for :: and ::1.
            let addr = std::net::Ipv6Addr::new(
                groups[0], groups[1], groups[2], groups[3], groups[4], groups[5], groups[6],
                groups[7],
            );
            Some(addr.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Real captured output shape from Ubuntu 24.04 / iproute2 6.1.
    const SAMPLE: &str = "\
tcp LISTEN 0      4096   0.0.0.0:22 0.0.0.0:* ino:72050970 sk:298ee cgroup:/system.slice/ssh.socket <->
tcp LISTEN 0      4096      [::]:22    [::]:* ino:72033197 sk:298ef cgroup:/system.slice/ssh.socket v6only:1 <->
tcp LISTEN 0      511  127.0.0.1:35347 0.0.0.0:* uid:1000 ino:169178294 sk:3fe68 cgroup:/user.slice/user-1000.slice/session-13.scope <->
udp UNCONN 0      0      0.0.0.0%enp4s0:40636 0.0.0.0:* uid:1000 ino:66477482 sk:2b86e cgroup:/user.slice/user-1000.slice/session-13.scope <->
tcp LISTEN 0      511    0.0.0.0:8787 0.0.0.0:* users:((\"node\",pid=1568925,fd=32)) uid:1000 ino:66477482 cgroup:/user.slice/user-1000.slice/session-13.scope <->
";

    #[test]
    fn parses_v4_listener_with_cgroup_no_pid() {
        let (rows, errs) = parse_ss(SAMPLE);
        assert!(errs.is_empty(), "{:?}", errs);
        let r = &rows[0];
        assert_eq!(r.endpoint.local_addr, "0.0.0.0");
        assert_eq!(r.endpoint.local_port, 22);
        assert_eq!(r.endpoint.ip_version, IpVersion::V4);
        assert_eq!(r.endpoint.state, EndpointState::TcpListen);
        assert_eq!(r.endpoint.inode, Some(72050970));
        assert_eq!(
            r.endpoint.cgroup.as_deref(),
            Some("/system.slice/ssh.socket")
        );
        assert!(r.pids.is_empty());
    }

    #[test]
    fn parses_v6_listener_brackets() {
        let (rows, _) = parse_ss(SAMPLE);
        let r = &rows[1];
        assert_eq!(r.endpoint.local_addr, "::");
        assert_eq!(r.endpoint.ip_version, IpVersion::V6);
        assert_eq!(r.endpoint.local_port, 22);
    }

    #[test]
    fn parses_udp_unconn_with_interface_scope() {
        let (rows, _) = parse_ss(SAMPLE);
        let r = &rows[3];
        assert_eq!(r.endpoint.protocol, Protocol::Udp);
        assert_eq!(r.endpoint.state, EndpointState::UdpBound);
        assert_eq!(r.endpoint.local_addr, "0.0.0.0");
        assert_eq!(r.endpoint.scope.as_deref(), Some("enp4s0"));
        assert_eq!(r.endpoint.display_addr(), "0.0.0.0%enp4s0");
        assert_eq!(r.endpoint.local_port, 40636);
    }

    #[test]
    fn parses_pid_from_users_field() {
        let (rows, _) = parse_ss(SAMPLE);
        let r = &rows[4];
        assert_eq!(r.pids, vec![1568925]);
        assert_eq!(r.endpoint.local_port, 8787);
    }

    #[test]
    fn users_field_multiple_pids_and_no_false_positives() {
        let mut out = String::from(
            "tcp LISTEN 0 4096 0.0.0.0:80 0.0.0.0:* users:((\"a\",pid=10,fd=3),(\"b\",pid=200,fd=4))",
        );
        let (rows, _) = parse_ss(&out);
        assert_eq!(rows[0].pids, vec![10, 200]);
        // fd numbers must never be mistaken for pids.
        out = "tcp LISTEN 0 4096 0.0.0.0:81 0.0.0.0:* users:((\"a\",pid=7,fd=99999))".into();
        let (rows, _) = parse_ss(&out);
        assert_eq!(rows[0].pids, vec![7]);
    }

    #[test]
    fn same_port_tcp_and_udp_coexist() {
        let out = "\
tcp LISTEN 0 4096 127.0.0.1:53 0.0.0.0:* ino:1
udp UNCONN 0 0    127.0.0.1:53 0.0.0.0:* ino:2
";
        let (rows, _) = parse_ss(out);
        assert_eq!(rows.len(), 2);
        assert_ne!(rows[0].endpoint.key(), rows[1].endpoint.key());
    }

    #[test]
    fn malformed_line_reports_error_not_panic() {
        let out = "tcp LISTEN 0 4096\n";
        let (rows, errs) = parse_ss(out);
        assert!(rows.is_empty());
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].code(), "parse_error");
    }

    #[test]
    fn unparsable_address_reports_error() {
        let out = "tcp LISTEN 0 4096 0.0.0.0 0.0.0.0:* ino:1\n";
        let (rows, errs) = parse_ss(out);
        assert!(rows.is_empty());
        assert_eq!(errs[0].code(), "parse_error");
    }

    #[test]
    fn unknown_state_preserved_not_dropped() {
        let out = "tcp WEIRD 0 0 127.0.0.1:1 0.0.0.0:* ino:1\n";
        let (rows, _) = parse_ss(out);
        assert_eq!(rows[0].endpoint.state, EndpointState::Unknown);
        assert!(!rows[0].endpoint.state.is_listening());
    }

    #[test]
    fn proc_addr_decode_v4() {
        // 0100007F -> 127.0.0.1
        assert_eq!(
            decode_proc_addr("0100007F", IpVersion::V4).unwrap(),
            "127.0.0.1"
        );
        assert_eq!(
            decode_proc_addr("00000000", IpVersion::V4).unwrap(),
            "0.0.0.0"
        );
    }

    #[test]
    fn proc_addr_decode_v6_loopback() {
        // ::1 as stored by the kernel
        assert_eq!(
            decode_proc_addr("00000000000000000000000001000000", IpVersion::V6).unwrap(),
            "::1"
        );
        assert_eq!(
            decode_proc_addr("00000000000000000000000000000000", IpVersion::V6).unwrap(),
            "::"
        );
    }

    #[test]
    fn header_line_is_skipped() {
        let out = "Netid State Recv-Q Send-Q Local Peer\n";
        let (rows, errs) = parse_ss(out);
        assert!(rows.is_empty());
        // A real header is not a state we model, so it is reported, not guessed.
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].code(), "parse_error");
    }
}
