//! Fictional data for screenshots and UI tests.
//!
//! Taskbook §1.2 permits fictional data for deterministic UI tests and
//! screenshots, provided it is never presented as proof that real collection
//! works. This module exists for exactly one purpose: producing public
//! screenshots that contain **no real machine information** — no private IP
//! addresses, no real PIDs, no real hostnames or unit names from the
//! developer's machine.
//!
//! Enabled with `PORTSCOPE_DEMO_DATA=1` (see `ui::start_collection`). The
//! application says so in the status bar so a screenshot can never be mistaken
//! for live data.

use portscope_core::model::*;

/// Build a fictional snapshot that exercises the interesting UI states:
/// IPv4 and IPv6, TCP and UDP, an owned endpoint, an unattributable one, a
/// socket-activated unit, a failed unit, inactive units, and a realistic
/// mix of service names.
pub fn demo_snapshot() -> Snapshot {
    let mut snap = Snapshot {
        collected_at: Some(std::time::SystemTime::now()),
        ..Default::default()
    };

    // --- services ---------------------------------------------------------
    let svc = |id: &str, kind: UnitKind, state: ActiveState, sub: &str| ServiceInfo {
        id: id.into(),
        scope: UnitScope::System,
        kind,
        active_state: state,
        sub_state: sub.into(),
        load_state: "loaded".into(),
        main_pid: None,
        control_group: None,
        listen: vec![],
        triggers: vec![],
        triggered_by: vec![],
        fragment_path: None,
        source: ServiceSource::DBus,
    };

    let mut web = svc(
        "example-web.service",
        UnitKind::Service,
        ActiveState::Active,
        "running",
    );
    web.main_pid = Some(2101);
    web.control_group = Some("/system.slice/example-web.service".into());

    let mut db = svc(
        "example-db.service",
        UnitKind::Service,
        ActiveState::Active,
        "running",
    );
    db.main_pid = Some(2202);
    db.control_group = Some("/system.slice/example-db.service".into());

    let sock = ServiceInfo {
        listen: vec!["0.0.0.0:8080 (Stream)".into(), "[::]:8080 (Stream)".into()],
        triggers: vec!["example-web.service".into()],
        control_group: Some("/system.slice/example-web.socket".into()),
        ..svc(
            "example-web.socket",
            UnitKind::Socket,
            ActiveState::Active,
            "listening",
        )
    };

    snap.services = vec![
        web.clone(),
        db.clone(),
        sock.clone(),
        svc(
            "example-backup.service",
            UnitKind::Service,
            ActiveState::Failed,
            "failed",
        ),
        svc(
            "example-print.service",
            UnitKind::Service,
            ActiveState::Inactive,
            "dead",
        ),
        svc(
            "example-cache.service",
            UnitKind::Service,
            ActiveState::Inactive,
            "dead",
        ),
    ];

    // --- processes --------------------------------------------------------
    snap.processes = vec![
        ProcessInfo {
            pid: 2101,
            name: "example-web".into(),
            cmdline: Some(vec![
                "/usr/bin/example-web".into(),
                "--port".into(),
                "8080".into(),
            ]),
            uid: Some(0),
            user: Some("root".into()),
            ppid: Some(1),
            start_time_ticks: Some(50_000),
            source: PidSource::SystemdMainPid,
        },
        ProcessInfo {
            pid: 2202,
            name: "example-db".into(),
            cmdline: Some(vec!["/usr/bin/example-db".into()]),
            uid: Some(999),
            user: Some("example-db".into()),
            ppid: Some(1),
            start_time_ticks: Some(60_000),
            source: PidSource::SystemdMainPid,
        },
    ];

    // --- endpoints --------------------------------------------------------
    // Deliberately documentation-range / example addresses only (RFC 5737 and
    // RFC 3849), so no screenshot can leak a real network.
    let ep = |proto: Protocol,
              addr: &str,
              port: u16,
              inode: u64,
              uid: Option<u32>,
              cgroup: Option<&str>| Endpoint {
        protocol: proto,
        state: if proto == Protocol::Tcp {
            EndpointState::TcpListen
        } else {
            EndpointState::UdpBound
        },
        local_addr: addr.into(),
        scope: None,
        local_port: port,
        ip_version: if addr.contains(':') {
            IpVersion::V6
        } else {
            IpVersion::V4
        },
        inode: Some(inode),
        uid,
        cgroup: cgroup.map(str::to_string),
    };

    let e_web4 = ep(
        Protocol::Tcp,
        "0.0.0.0",
        8080,
        1,
        Some(0),
        Some("/system.slice/example-web.socket"),
    );
    let e_web6 = ep(
        Protocol::Tcp,
        "::",
        8080,
        2,
        Some(0),
        Some("/system.slice/example-web.socket"),
    );
    let e_db = ep(
        Protocol::Tcp,
        "127.0.0.1",
        5432,
        3,
        Some(999),
        Some("/system.slice/example-db.service"),
    );
    let e_dns4 = ep(
        Protocol::Udp,
        "192.0.2.10",
        53,
        4,
        Some(998),
        Some("/system.slice/example-dns.service"),
    );
    let e_unknown = ep(Protocol::Tcp, "203.0.113.5", 9000, 5, None, None);
    let e_v6 = ep(Protocol::Tcp, "2001:db8::1", 6379, 6, None, None);

    snap.endpoints = vec![
        e_web4.clone(),
        e_web6.clone(),
        e_db.clone(),
        e_dns4,
        e_unknown,
        e_v6,
    ];

    // --- associations with real evidence shapes ---------------------------
    snap.associations.push(Association::new(
        AssociationKind::EndpointToProcess {
            endpoint: e_db.key(),
            identity: ProcessIdentity {
                pid: 2202,
                start_time_ticks: 60_000,
            },
        },
        vec![Evidence {
            kind: EvidenceKind::CgroupUnitMatch,
            detail: "socket cgroup /system.slice/example-db.service == pid 2202 cgroup".into(),
        }],
    ));
    snap.associations.push(Association::new(
        AssociationKind::EndpointToProcess {
            endpoint: e_web4.key(),
            identity: ProcessIdentity {
                pid: 2101,
                start_time_ticks: 50_000,
            },
        },
        vec![Evidence {
            kind: EvidenceKind::SocketInodeInProcFd,
            detail: "inode 1 in /proc/2101/fd".into(),
        }],
    ));
    for key in [e_web4.key(), e_web6.key()] {
        snap.associations.push(Association::new(
            AssociationKind::EndpointToService {
                endpoint: key,
                unit: "example-web.service".into(),
                scope: UnitScope::System,
            },
            vec![
                Evidence {
                    kind: EvidenceKind::SocketActivation,
                    detail: "example-web.socket Triggers=example-web.service".into(),
                },
                Evidence {
                    kind: EvidenceKind::CgroupUnitMatch,
                    detail: "cgroup:/system.slice/example-web.socket".into(),
                },
            ],
        ));
    }
    snap.associations.push(Association::new(
        AssociationKind::EndpointToService {
            endpoint: e_db.key(),
            unit: "example-db.service".into(),
            scope: UnitScope::System,
        },
        vec![Evidence {
            kind: EvidenceKind::CgroupUnitMatch,
            detail: "cgroup:/system.slice/example-db.service".into(),
        }],
    ));

    // Two endpoints deliberately have no owner, mirroring the real "some
    // information is unavailable" state without exposing anything.
    snap.degraded = true;
    snap.errors.push(CollectionError::PermissionDenied {
        resource: "process info for some endpoints".into(),
        hint: "2/6 endpoints have no owning process; sample data".into(),
    });

    snap
}
