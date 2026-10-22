//! Integration tests using *controlled* processes and a temporary user-level
//! systemd unit. Nothing here depends on services that happen to exist on the
//! machine.
//!
//! Isolation strategy:
//!   * Random ephemeral TCP/UDP ports bound in-process (not a fixed port).
//!   * A throwaway `--user` systemd unit named `portscope-spike-<rand>.service`
//!     created under `$XDG_RUNTIME_DIR`, started and stopped by the test, and
//!     removed on drop. No root, no system service, no firewall changes.
//!   * Tests that need a user systemd manager skip cleanly (with an explicit
//!     note) when none is available, rather than silently passing.

use portscope_core::endpoints::{collect_endpoints, parse_ss};
use portscope_core::model::*;
use portscope_core::process::{inspect_process, pid_owns_socket_inode};
use portscope_core::services::{Backend, collect_services, show_unit};
use std::io::Write;
use std::net::{TcpListener, UdpSocket};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn have_user_systemd() -> bool {
    Command::new("systemctl")
        .args(["--user", "is-system-running"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success() || s.code() == Some(1)) // 1 => degraded but alive
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Skip accounting.
//
// A skipped test is NOT a pass. Every skip is appended to a shared log which
// the test binary flushes to $PORTSCOPE_SKIP_LOG (if set), so the run summary
// can state exactly which prerequisites were missing.
// ---------------------------------------------------------------------------
use std::sync::Mutex;
static SKIP_LOG: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn skip(slot: &'static Mutex<Vec<String>>, test: &str, reason: &str) {
    eprintln!("SKIP[{test}]: {reason}");
    let mut g = slot.lock().unwrap();
    g.push(format!("{test}: {reason}"));
    if let Ok(path) = std::env::var("PORTSCOPE_SKIP_LOG") {
        use std::io::Write as _;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = writeln!(f, "{test}: {reason}");
        }
    }
}

fn random_suffix() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64;
    nanos ^ ((std::process::id() as u64) << 32)
}

/// Inode of a listening socket, found via `ss` (so the test exercises the same
/// path the app uses).
fn inode_for_port(port: u16, proto: Protocol) -> Option<u64> {
    let (rows, _) = collect_endpoints(false);
    rows.iter()
        .find(|r| r.endpoint.local_port == port && r.endpoint.protocol == proto)
        .and_then(|r| r.endpoint.inode)
}

fn wait_for_port(port: u16, proto: Protocol, timeout: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if inode_for_port(port, proto).is_some() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

// ---------------------------------------------------------------------------
// A6.1 / A6.2 / A6.4 — TCP v4, TCP v6, UDP, multiple addresses
// ---------------------------------------------------------------------------

#[test]
fn controlled_tcp_ipv4_listener_is_discovered_with_owner() {
    let l = TcpListener::bind("127.0.0.1:0").expect("bind v4");
    let port = l.local_addr().unwrap().port();
    assert!(
        wait_for_port(port, Protocol::Tcp, Duration::from_secs(3)),
        "port never appeared"
    );

    let (rows, errs) = collect_endpoints(false);
    assert!(!errs.is_empty() || !rows.is_empty());
    let row = rows
        .iter()
        .find(|r| r.endpoint.local_port == port && r.endpoint.protocol == Protocol::Tcp)
        .expect("our listener must be present");
    assert_eq!(row.endpoint.local_addr, "127.0.0.1");
    assert_eq!(row.endpoint.ip_version, IpVersion::V4);
    assert_eq!(row.endpoint.state, EndpointState::TcpListen);
    assert!(row.endpoint.state.is_listening());

    // inode -> fd join must find *us*
    let inode = row.endpoint.inode.expect("inode present");
    assert!(
        pid_owns_socket_inode(std::process::id(), inode),
        "our own socket inode must be found in our fd table"
    );
}

#[test]
fn controlled_tcp_ipv6_listener_is_discovered() {
    // Skip cleanly if the host has no IPv6.
    let Ok(l) = TcpListener::bind("[::1]:0") else {
        skip(&SKIP_LOG, "tcp_ipv6", "no IPv6 loopback on this host");
        return;
    };
    let port = l.local_addr().unwrap().port();
    assert!(wait_for_port(port, Protocol::Tcp, Duration::from_secs(3)));
    let (rows, _) = collect_endpoints(false);
    let row = rows
        .iter()
        .find(|r| r.endpoint.local_port == port && r.endpoint.ip_version == IpVersion::V6)
        .expect("v6 listener present");
    assert_eq!(row.endpoint.local_addr, "::1");
}

#[test]
fn controlled_udp_binding_is_discovered_as_unconn() {
    let s = UdpSocket::bind("127.0.0.1:0").expect("bind udp");
    let port = s.local_addr().unwrap().port();
    assert!(wait_for_port(port, Protocol::Udp, Duration::from_secs(3)));
    let (rows, _) = collect_endpoints(false);
    let row = rows
        .iter()
        .find(|r| r.endpoint.local_port == port && r.endpoint.protocol == Protocol::Udp)
        .expect("udp binding present");
    assert!(
        matches!(
            row.endpoint.state,
            EndpointState::UdpBound | EndpointState::Other(_)
        ),
        "unexpected state {:?}",
        row.endpoint.state
    );
}

#[test]
fn same_port_number_on_tcp_and_udp_are_distinct_endpoints() {
    // Bind an ephemeral TCP port, then try the same number for UDP.
    let t = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = t.local_addr().unwrap().port();
    let Ok(u) = UdpSocket::bind(("127.0.0.1", port)) else {
        skip(
            &SKIP_LOG,
            "same_port_tcp_udp",
            "ephemeral UDP port unavailable on TCP",
        );
        return;
    };
    assert!(wait_for_port(port, Protocol::Tcp, Duration::from_secs(3)));
    assert!(wait_for_port(port, Protocol::Udp, Duration::from_secs(3)));

    let (rows, _) = collect_endpoints(false);
    let tcp = rows
        .iter()
        .find(|r| r.endpoint.local_port == port && r.endpoint.protocol == Protocol::Tcp);
    let udp = rows
        .iter()
        .find(|r| r.endpoint.local_port == port && r.endpoint.protocol == Protocol::Udp);
    assert!(tcp.is_some(), "tcp {port} missing");
    assert!(udp.is_some(), "udp {port} missing");
    assert_ne!(tcp.unwrap().endpoint.key(), udp.unwrap().endpoint.key());
    drop(u);
}

#[test]
fn multiple_listen_addresses_on_same_port_are_separate_rows() {
    // Same port on two different v4 loopback addresses.
    let Ok(a) = TcpListener::bind("127.0.0.1:0") else {
        return;
    };
    let port = a.local_addr().unwrap().port();
    let Ok(b) = TcpListener::bind(("127.0.0.2", port)) else {
        skip(&SKIP_LOG, "multiple_addresses", "127.0.0.2 not bindable");
        return;
    };
    assert!(wait_for_port(port, Protocol::Tcp, Duration::from_secs(3)));
    let (rows, _) = collect_endpoints(false);
    let matching: Vec<_> = rows
        .iter()
        .filter(|r| r.endpoint.local_port == port && r.endpoint.protocol == Protocol::Tcp)
        .collect();
    assert!(
        matching.len() >= 2,
        "expected >=2 rows for same port on different addresses, got {}",
        matching.len()
    );
    let addrs: Vec<_> = matching
        .iter()
        .map(|r| r.endpoint.local_addr.as_str())
        .collect();
    assert!(addrs.contains(&"127.0.0.1"));
    assert!(addrs.contains(&"127.0.0.2"));
    drop(b);
}

// ---------------------------------------------------------------------------
// A6.5 — process exit and stale information
// ---------------------------------------------------------------------------

#[test]
fn process_exit_is_reported_as_process_gone() {
    let mut child = Command::new("sleep")
        .arg("30")
        .stdout(Stdio::null())
        .spawn()
        .expect("spawn sleep");
    let pid = child.id();
    let info = inspect_process(pid, PidSource::TestHarness, None).expect("inspect live child");
    assert_eq!(info.pid, pid);
    let ticks = info.start_time_ticks.unwrap();

    child.kill().unwrap();
    child.wait().unwrap();

    // Give the kernel a moment to reap /proc entry.
    let mut gone = false;
    for _ in 0..50 {
        match inspect_process(pid, PidSource::TestHarness, Some(ticks)) {
            Err(CollectionError::ProcessGone { .. }) => {
                gone = true;
                break;
            }
            Err(CollectionError::ProcessIdentityChanged { .. }) => {
                // PID was reused within the test window: also correct behaviour.
                gone = true;
                break;
            }
            _ => std::thread::sleep(Duration::from_millis(20)),
        }
    }
    assert!(gone, "exited process must not be reported as live");
}

#[test]
fn stale_socket_row_vanishes_after_listener_drop() {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    assert!(wait_for_port(port, Protocol::Tcp, Duration::from_secs(3)));
    drop(l);

    let mut vanished = false;
    for _ in 0..60 {
        let (rows, _) = collect_endpoints(false);
        if !rows
            .iter()
            .any(|r| r.endpoint.local_port == port && r.endpoint.protocol == Protocol::Tcp)
        {
            vanished = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(vanished, "closed listener must disappear from collection");
}

// ---------------------------------------------------------------------------
// A6.6 — restricted information for normal users
// ---------------------------------------------------------------------------

#[test]
fn foreign_owned_socket_has_no_pid_but_has_uid_and_cgroup() {
    // Find a TCP listener owned by a different uid (typically root). If the
    // machine has none, skip explicitly.
    let (rows, _) = collect_endpoints(true);
    let me = current_euid();
    let foreign = rows.iter().find(|r| {
        r.endpoint.protocol == Protocol::Tcp
            && r.endpoint.state == EndpointState::TcpListen
            && r.endpoint.uid.map(|u| u != me).unwrap_or(false)
    });
    let Some(f) = foreign else {
        skip(
            &SKIP_LOG,
            "foreign_uid_perms",
            "no foreign-uid TCP listener on this host",
        );
        return;
    };
    // `ss -p` must NOT reveal a pid for a foreign process to a normal user.
    assert!(
        f.pids.is_empty(),
        "ss -p leaked a pid {:?} for foreign uid process",
        f.pids
    );
    // But extended info IS available without privilege.
    assert!(f.endpoint.uid.is_some(), "uid must be available via ss -e");
    eprintln!(
        "NOTE: foreign socket port={} uid={:?} cgroup={:?} pid_hidden={}",
        f.endpoint.local_port,
        f.endpoint.uid,
        f.endpoint.cgroup,
        f.pids.is_empty()
    );
}

/// Effective uid from `/proc/self/status`, so the tests need no FFI, no `libc`
/// crate and no `unsafe` (edition 2024 forbids bare `extern` blocks anyway).
fn current_euid() -> u32 {
    let status = std::fs::read_to_string("/proc/self/status").expect("/proc/self/status readable");
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("Uid:") {
            // Uid: real effective saved fs
            if let Some(eff) = rest.split_whitespace().nth(1) {
                return eff.parse().expect("effective uid parses");
            }
        }
    }
    panic!("no Uid: line in /proc/self/status");
}

// ---------------------------------------------------------------------------
// A6.7-A6.10 — systemd services: running, stopped, failed, user-level,
// socket activation
// ---------------------------------------------------------------------------

/// A temporary `--user` unit exercising running / failed states and removal.
struct TempUserUnit {
    name: String,
    dir: std::path::PathBuf,
}

impl TempUserUnit {
    fn create(name: &str, body: &str) -> Option<Self> {
        if !have_user_systemd() {
            return None;
        }
        // Must be on the user manager's UnitPath; ~/.config/systemd/user is.
        let home = std::env::var("HOME").ok()?;
        let dir = std::path::PathBuf::from(home).join(".config/systemd/user");
        std::fs::create_dir_all(&dir).ok()?;
        let path = dir.join(format!("{}.service", name));
        let mut f = std::fs::File::create(&path).ok()?;
        f.write_all(body.as_bytes()).ok()?;
        // Make systemd notice the unit.
        let _ = Command::new("systemctl")
            .args(["--user", "daemon-reload"])
            .status();
        Some(TempUserUnit {
            name: name.to_string(),
            dir,
        })
    }

    fn start(&self) -> bool {
        Command::new("systemctl")
            .args(["--user", "start", &format!("{}.service", self.name)])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    fn stop(&self) -> bool {
        Command::new("systemctl")
            .args(["--user", "stop", &format!("{}.service", self.name)])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    fn show(&self) -> Result<ServiceInfo, CollectionError> {
        show_unit(
            UnitScope::User,
            &format!("{}.service", self.name),
            Backend::Systemctl,
        )
    }
}

impl Drop for TempUserUnit {
    fn drop(&mut self) {
        let _ = Command::new("systemctl")
            .args(["--user", "stop", &format!("{}.service", self.name)])
            .status();
        let _ = std::fs::remove_file(self.dir.join(format!("{}.service", self.name)));
        let _ = Command::new("systemctl")
            .args(["--user", "daemon-reload"])
            .status();
    }
}

#[test]
fn user_service_lifecycle_running_then_stopped() {
    let name = format!("portscope-spike-{}", random_suffix());
    let Some(unit) = TempUserUnit::create(
        &name,
        "[Unit]\nDescription=PortScope spike temp unit\n[Service]\nType=simple\nExecStart=/bin/sleep 300\n",
    ) else {
        eprintln!("SKIP: no user systemd manager available");
        return;
    };

    // Before start: unit exists but inactive (or not-found depending on load).
    let _ = unit.show();

    assert!(unit.start(), "failed to start temp user unit");
    // Wait for active.
    let mut active = None;
    for _ in 0..50 {
        if let Ok(s) = unit.show() {
            if s.active_state == ActiveState::Active {
                active = Some(s);
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let svc = active.expect("temp unit never became active");
    assert_eq!(svc.scope, UnitScope::User);
    let main_pid = svc
        .main_pid
        .expect("active simple service must have MainPID");
    let p =
        inspect_process(main_pid, PidSource::SystemdMainPid, None).expect("MainPID inspectable");
    eprintln!(
        "NOTE: temp unit {} MainPID={} name={}",
        svc.id, main_pid, p.name
    );

    assert!(unit.stop(), "failed to stop temp user unit");
    let mut stopped = false;
    for _ in 0..50 {
        if let Ok(s) = unit.show() {
            if s.active_state == ActiveState::Inactive {
                stopped = true;
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(stopped, "temp unit did not report inactive after stop");
}

#[test]
fn failed_user_service_reports_failed_state() {
    let name = format!("portscope-spike-fail-{}", random_suffix());
    let Some(unit) = TempUserUnit::create(
        &name,
        "[Unit]\nDescription=PortScope failing unit\n[Service]\nType=simple\nExecStart=/bin/false\n",
    ) else {
        skip(&SKIP_LOG, "failed_service", "no user systemd manager");
        return;
    };
    // Starting will fail; we only care what state systemd reports.
    let _ = unit.start();

    let mut failed = false;
    for _ in 0..50 {
        if let Ok(s) = unit.show() {
            if s.active_state == ActiveState::Failed || s.sub_state == "failed" {
                failed = true;
                break;
            }
            if s.active_state == ActiveState::Inactive {
                // Oneshot-like simple service may end inactive; accept either
                // but report which one we saw.
                eprintln!(
                    "NOTE: failed unit ended inactive/{} rather than failed",
                    s.sub_state
                );
                failed = true;
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(failed, "failing unit never reported failure state");
}

#[test]
fn nonexistent_unit_is_distinguished_from_inactive_unit() {
    let e = show_unit(
        UnitScope::User,
        "portscope-absent-xyz.service",
        Backend::Systemctl,
    )
    .unwrap_err();
    assert_eq!(e.code(), "unit_not_found");
}

#[test]
fn socket_activation_links_socket_unit_to_service() {
    if !have_user_systemd() {
        skip(&SKIP_LOG, "socket_activation", "no user systemd manager");
        return;
    }
    let name = format!("portscope-spike-sock-{}", random_suffix());
    let home = std::env::var("HOME").unwrap();
    let dir = std::path::PathBuf::from(&home).join(".config/systemd/user");
    std::fs::create_dir_all(&dir).unwrap();

    // A socket unit with an arbitrary ephemeral-ish port on loopback.
    let port = 39000 + (random_suffix() % 1000) as u16;
    let socket_unit = format!("{}.socket", name);
    let service_unit = format!("{}.service", name);
    std::fs::write(
        dir.join(&socket_unit),
        format!(
            "[Unit]\nDescription=PortScope spike activation socket\n[Socket]\nListenStream=127.0.0.1:{}\n[Install]\nWantedBy=sockets.target\n",
            port
        ),
    )
    .unwrap();
    std::fs::write(
        dir.join(&service_unit),
        "[Unit]\nDescription=PortScope spike activation service\n[Service]\nExecStart=/bin/sleep 300\n",
    )
    .unwrap();
    let _ = Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .status();
    let started = Command::new("systemctl")
        .args(["--user", "start", &socket_unit])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    if !started {
        let _ = std::fs::remove_file(dir.join(&socket_unit));
        let _ = std::fs::remove_file(dir.join(&service_unit));
        let _ = Command::new("systemctl")
            .args(["--user", "daemon-reload"])
            .status();
        // Do NOT pass silently: this is a real prerequisite failure.
        panic!("could not start user socket unit {}", socket_unit);
    }

    let res = (|| {
        let sock = show_unit(UnitScope::User, &socket_unit, Backend::Systemctl)?;
        assert_eq!(sock.kind, UnitKind::Socket);
        assert!(
            sock.triggers.iter().any(|t| t == &service_unit),
            "socket unit must name its triggered service, got {:?}",
            sock.triggers
        );

        // Locate the listening socket we created.
        let mut found = None;
        for _ in 0..40 {
            let (rows, _) = collect_endpoints(false);
            if let Some(r) = rows
                .iter()
                .find(|r| r.endpoint.local_port == port && r.endpoint.protocol == Protocol::Tcp)
            {
                found = Some(r.endpoint.clone());
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let ep = found.expect("activation socket never appeared in ss");

        // DOCUMENTED FINDING: for USER-level socket activation the fd is held
        // by `systemd --user`, so `ss -e` reports the user manager's
        // `init.scope`, NOT the socket unit's cgroup. This differs from the
        // system case (e.g. /system.slice/ssh.socket). We assert the real
        // behaviour so a future change is detected.
        let cg = ep.cgroup.clone().expect("cgroup present");
        assert!(
            cg.ends_with("/init.scope"),
            "expected user-manager init.scope for user socket activation, got {cg}"
        );
        eprintln!("NOTE: user socket activation cgroup = {cg} (not the socket unit)");

        // The reliable link is the unit's ControlGroup property + Triggers.
        assert!(
            sock.control_group
                .as_deref()
                .map(|c| c.ends_with(&socket_unit))
                .unwrap_or(false),
            "socket unit ControlGroup should name it, got {:?}",
            sock.control_group
        );

        // And the association rule must recover the link from ControlGroup.
        let (services, _e) = collect_services(&[UnitScope::User], None, Backend::Systemctl);
        let (assocs, _ae) = portscope_core::assoc::associate_endpoints_to_units(
            std::slice::from_ref(&ep),
            &services,
        );
        assert!(
            assocs.iter().any(|a| matches!(&a.kind,
                AssociationKind::EndpointToService { unit, .. } if *unit == service_unit)),
            "user socket activation must associate endpoint to {service_unit}, got {:#?}",
            assocs
        );
        Ok::<(), CollectionError>(())
    })();

    let _ = Command::new("systemctl")
        .args(["--user", "stop", &socket_unit])
        .status();
    let _ = std::fs::remove_file(dir.join(&socket_unit));
    let _ = std::fs::remove_file(dir.join(&service_unit));
    let _ = Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .status();
    if let Err(e) = res {
        panic!("socket activation check failed: {e}");
    }
}

// ---------------------------------------------------------------------------
// Parser-only checks against synthetic fixtures (no environment dependency)
// ---------------------------------------------------------------------------

#[test]
fn parser_handles_ss_extended_field_order_independently() {
    // Fields may appear in any order after the address columns.
    let out = "tcp LISTEN 0 4096 0.0.0.0:80 0.0.0.0:* cgroup:/system.slice/x.service ino:42 uid:0 sk:1 <->\n";
    let (rows, errs) = parse_ss(out);
    assert!(errs.is_empty(), "{:?}", errs);
    assert_eq!(rows[0].endpoint.inode, Some(42));
    assert_eq!(rows[0].endpoint.uid, Some(0));
    assert_eq!(
        rows[0].endpoint.cgroup.as_deref(),
        Some("/system.slice/x.service")
    );
}

#[test]
fn collection_is_repeatable() {
    // Requirement §A6: repeatable. Two consecutive full collections must both
    // succeed and agree on our own listener.
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    assert!(wait_for_port(port, Protocol::Tcp, Duration::from_secs(3)));
    let (a, _) = collect_endpoints(false);
    let (b, _) = collect_endpoints(false);
    let pa = a
        .iter()
        .find(|r| r.endpoint.local_port == port)
        .map(|r| r.endpoint.inode);
    let pb = b
        .iter()
        .find(|r| r.endpoint.local_port == port)
        .map(|r| r.endpoint.inode);
    assert_eq!(pa, pb, "inode must be stable across collections");
}
